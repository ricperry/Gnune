#!/usr/bin/env python3
"""Isolated host migration tests; never invoke Tailscale, sudo or personal config."""
import contextlib
import copy
import fcntl
import importlib.util
import io
import json
import os
from pathlib import Path
import socket
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True

spec = importlib.util.spec_from_file_location("setup", Path(__file__).parents[1] / "scripts/gnune-tailscale-setup.py")
setup = importlib.util.module_from_spec(spec)
spec.loader.exec_module(setup)
HOST = "fixture.example.ts.net"


class FakeCommands:
    def __init__(self, config=None):
        self.config = copy.deepcopy(config or {})
        self.mutations = []
        self.after_mount = None
        self.fail_mount = False

    def status(self):
        return {"BackendState": "Running", "Self": {"DNSName": HOST + "."}, "CertDomains": [HOST]}

    def serve(self):
        return copy.deepcopy(self.config)

    def mount(self, port, target, remove=False):
        self.mutations.append((port, target, remove))
        if self.fail_mount:
            raise RuntimeError("simulated privilege denial")
        key = f"{HOST}:{port}"
        if remove:
            self.config = setup.without_target(self.config, HOST, port)
        else:
            self.config.setdefault("TCP", {})[str(port)] = {"HTTPS": True}
            self.config.setdefault("Web", {})[key] = {"Handlers": {"/": {"Proxy": target}}}
        if self.after_mount:
            self.after_mount()


class LocalProbe(setup.Probe):
    """Actual HTTP over Unix socket, simulating only Serve's forwarded headers."""
    def ready(self, origin):
        client = socket.socket(socket.AF_UNIX)
        try:
            client.connect(str(self.path))
            request = (f"GET /__gnune_setup/{self.nonce} HTTP/1.1\r\nHost: localhost\r\n"
                       f"X-Forwarded-Host: {setup.authority(origin)}\r\nX-Forwarded-Proto: https\r\n\r\n")
            client.sendall(request.encode())
            response = client.makefile("rb")
            self_response = response.read()
            if not self_response.startswith(b"HTTP/1.0 200") or not self_response.endswith(self.nonce.encode()):
                raise RuntimeError("private proxy probe failed")
        finally:
            client.close()


class MigrationTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="gnune-ts-tests-", dir="/tmp")
        self.root = Path(self.tmp.name)
        self.path = self.root / "sharing.json"
        self.saved = {"enabled": True, "mode": "queue", "include_video": False, "include_podcasts": True,
                      "queue": {"scope": "preserved", "tracks": [], "opaque_extra": 17}, "skipped": 2,
                      "unknown_future_field": {"keep": True},
                      "connection": {"bind": "192.0.2.20:8443", "origin": "https://192.0.2.20:8443",
                                     "cert": "/not/read/certificate.pem", "key": "/not/read/private-key.pem"}}
        self.path.write_text(json.dumps(self.saved))
        self.path.chmod(0o600)
        self.commands = FakeCommands({"TCP": {"9443": {"HTTPS": True}}, "Web": {f"{HOST}:9443": {"Handlers": {"/": {"Text": "unrelated"}}}}})
        self.closed = patch.object(setup, "require_closed")
        self.probe = patch.object(setup, "Probe", LocalProbe)
        self.closed.start()
        self.probe.start()
        self.output = io.StringIO()
        self.stdout = contextlib.redirect_stdout(self.output)
        self.stdout.__enter__()

    def tearDown(self):
        self.stdout.__exit__(None, None, None)
        self.probe.stop()
        self.closed.stop()
        self.tmp.cleanup()

    def run_setup(self, **options):
        setup.migrate(self.path, self.commands, **options)

    def fresh(self):
        self.path.unlink()
        self.path = self.root / "fresh" / "gnune" / "sharing.json"

    def test_fresh_preview_does_not_create_config_directory(self):
        self.fresh()
        self.run_setup()
        self.assertFalse(self.path.parent.exists())
        self.assertEqual(self.commands.mutations, [])

    def test_fresh_apply_is_disabled_without_tls_and_undo_removes_only_settings(self):
        self.fresh()
        before = self.commands.serve()
        with patch.object(setup, "Commands", return_value=self.commands):
            setup.main(["--settings", str(self.path), "--apply"])
        saved = json.loads(self.path.read_text())
        self.assertFalse(saved["enabled"])
        self.assertEqual(saved["mode"], "library")
        self.assertTrue(saved["include_video"] and saved["include_podcasts"])
        self.assertIsNone(saved["queue"])
        self.assertNotIn("cert", saved["connection"])
        self.assertNotIn("key", saved["connection"])
        self.assertEqual(saved["connection"]["origin"], f"https://{HOST}")
        first = self.path.read_bytes()
        self.run_setup(apply=True)
        self.assertEqual(first, self.path.read_bytes())
        grants = self.path.with_name("mobile-devices.json")
        grants.write_text('preserve phone grants')
        before_preview = self.commands.serve()
        self.commands.mutations.clear()
        self.run_setup(undo=True)
        self.assertEqual(first, self.path.read_bytes())
        self.assertEqual(self.commands.serve(), before_preview)
        self.assertEqual(self.commands.mutations, [])
        self.run_setup(apply=True, undo=True)
        self.assertFalse(self.path.exists())
        self.assertEqual(grants.read_text(), 'preserve phone grants')
        self.assertEqual(self.commands.serve(), before)

    def test_fresh_undo_refuses_later_consent_before_mutating_serve(self):
        self.path.unlink()
        self.run_setup(apply=True)
        saved = json.loads(self.path.read_text())
        saved["enabled"] = True
        self.path.write_text(json.dumps(saved))
        self.commands.mutations.clear()
        with self.assertRaisesRegex(RuntimeError, "changed after first-time"):
            self.run_setup(apply=True, undo=True)
        self.assertEqual(self.commands.mutations, [])
        self.assertTrue(json.loads(self.path.read_text())["enabled"])

    def test_fresh_probe_failure_leaves_no_settings_or_mount(self):
        self.path.unlink()
        before = self.commands.serve()
        with patch.object(LocalProbe, "ready", side_effect=RuntimeError("probe unavailable")):
            with self.assertRaisesRegex(RuntimeError, "unavailable"):
                self.run_setup(apply=True)
        self.assertFalse(self.path.exists())
        self.assertFalse(self.path.with_name("tailscale-setup.json").exists())
        self.assertEqual(self.commands.serve(), before)

    def test_fresh_interrupted_apply_without_settings_is_undoable(self):
        self.path.unlink()
        def uncertain(port, target, remove=False):
            FakeCommands.mount(self.commands, port, target, remove)
            raise setup.UncertainServe("timed out")
        before = self.commands.serve()
        self.commands.mount = uncertain
        with self.assertRaises(setup.UncertainServe):
            self.run_setup(apply=True)
        self.assertFalse(self.path.exists())
        del self.commands.mount
        self.run_setup(apply=True, undo=True)
        self.assertEqual(self.commands.serve(), before)
        self.assertFalse(self.path.with_name("tailscale-setup.json").exists())

    def test_fresh_apply_never_overwrites_new_settings(self):
        self.path.unlink()
        def changed():
            self.path.write_text(json.dumps(self.saved))
        self.commands.after_mount = changed
        with self.assertRaisesRegex(RuntimeError, "changed"):
            self.run_setup(apply=True)
        self.assertEqual(json.loads(self.path.read_text()), self.saved)

    def test_preview_has_no_writes_or_mutations(self):
        original = self.path.read_bytes()
        self.run_setup()
        self.assertEqual(self.path.read_bytes(), original)
        self.assertEqual(self.commands.mutations, [])
        self.assertEqual(list(self.root.iterdir()), [self.path])

    def test_apply_preserves_consent_tls_queue_and_unrelated_mount_then_undo(self):
        before = self.commands.serve()
        self.run_setup(apply=True)
        saved = json.loads(self.path.read_text())
        for key, value in self.saved.items():
            if key != "connection":
                self.assertEqual(saved[key], value)
        self.assertEqual(saved["connection"]["key"], self.saved["connection"]["key"])
        self.assertEqual(saved["connection"]["cert"], self.saved["connection"]["cert"])
        self.assertEqual(saved["connection"]["origin"], f"https://{HOST}:8443")
        self.assertEqual(saved["connection"]["bind"], "127.0.0.1:8443")
        self.assertEqual((self.root / "tailscale-private").stat().st_mode & 0o777, 0o700)
        self.assertFalse((self.root / "tailscale-private/api.sock").exists())
        first = self.path.read_bytes()
        self.run_setup(apply=True)
        self.assertEqual(self.path.read_bytes(), first, "idempotent apply must not rewrite settings")
        saved["enabled"] = False
        self.path.write_text(json.dumps(saved))
        self.run_setup(apply=True, undo=True)
        self.assertEqual(self.commands.serve(), before)
        restored = json.loads(self.path.read_text())
        self.assertEqual(restored["connection"], self.saved["connection"])
        self.assertFalse(restored["enabled"], "undo preserves later consent changes")

    def test_probe_failure_rolls_back_only_own_mount_and_keeps_endpoint(self):
        before = self.commands.serve()
        with patch.object(LocalProbe, "ready", side_effect=RuntimeError("probe unavailable")):
            with self.assertRaisesRegex(RuntimeError, "unavailable"):
                self.run_setup(apply=True)
        self.assertEqual(self.commands.serve(), before)
        self.assertEqual(json.loads(self.path.read_text()), self.saved)
        self.assertFalse((self.root / "tailscale-setup.json").exists())

    def test_early_privilege_failure_is_retryable_without_losing_original_settings(self):
        self.commands.fail_mount = True
        with self.assertRaisesRegex(RuntimeError, "denial"):
            self.run_setup(apply=True)
        self.assertFalse((self.root / "tailscale-setup.json").exists())
        self.commands.fail_mount = False
        self.run_setup(apply=True)

    def test_timed_out_mutation_retains_recoverable_receipt(self):
        def uncertain(*_args, **_kwargs):
            raise setup.UncertainServe("timed out")
        self.commands.mount = uncertain
        with self.assertRaises(setup.UncertainServe):
            self.run_setup(apply=True)
        self.assertTrue((self.root / "tailscale-setup.json").exists())
        self.assertEqual(json.loads(self.path.read_text()), self.saved)

    def test_settings_race_is_not_overwritten(self):
        def changed():
            value = json.loads(self.path.read_text())
            value["mode"] = "library"
            self.path.write_text(json.dumps(value))
        self.commands.after_mount = changed
        with self.assertRaisesRegex(RuntimeError, "changed"):
            self.run_setup(apply=True)
        self.assertEqual(json.loads(self.path.read_text())["mode"], "library")
        self.assertEqual(json.loads(self.path.read_text())["connection"], self.saved["connection"])

    def test_existing_conflicting_serve_and_funnel_are_refused(self):
        self.commands.config["TCP"]["8443"] = {"TCPForward": "127.0.0.1:1234"}
        with self.assertRaisesRegex(RuntimeError, "different mount"):
            self.run_setup(apply=True)
        self.assertEqual(self.commands.mutations, [])
        self.commands.config = {"AllowFunnel": {f"{HOST}:8443": True}}
        with self.assertRaisesRegex(RuntimeError, "Funnel"):
            self.run_setup(apply=True)

    def test_adopted_exact_mount_is_not_removed_by_undo(self):
        target = f"unix:{self.root}/tailscale-private/api.sock"
        self.commands.mount(8443, target)
        self.commands.mutations.clear()
        self.run_setup(apply=True)
        self.assertEqual(self.commands.mutations, [])
        self.run_setup(apply=True, undo=True)
        self.assertEqual(self.commands.mutations, [])
        self.assertEqual(setup.target_state(self.commands.serve(), HOST, 8443, target), "exact")

    def test_exclusive_setup_lock_refuses_shared_app_lock(self):
        with open(self.path.with_suffix(".lock"), "w") as lock:
            fcntl.flock(lock.fileno(), fcntl.LOCK_SH | fcntl.LOCK_NB)
            with self.assertRaises(BlockingIOError):
                setup.main(["--settings", str(self.path), "--apply"])
        self.assertEqual(self.commands.mutations, [])

    def test_unrelated_funnel_and_service_ports_are_preserved(self):
        self.commands.config["AllowFunnel"] = {f"{HOST}:9443": True}
        self.commands.config["Services"] = {"svc:other": {"TCP": {"9444": {"HTTPS": True}}}}
        before = self.commands.serve()
        self.run_setup(apply=True)
        self.run_setup(apply=True, undo=True)
        self.assertEqual(self.commands.serve(), before)

    def test_preview_does_not_enable_https_or_request_certificates(self):
        status = self.commands.status()
        status["CertDomains"] = []
        self.commands.status = lambda: status
        self.run_setup()
        with self.assertRaisesRegex(RuntimeError, "Enable HTTPS"):
            self.run_setup(apply=True)
        self.assertEqual(self.commands.mutations, [])
        self.assertNotIn("private-key.pem", self.output.getvalue())


if __name__ == "__main__":
    unittest.main()
