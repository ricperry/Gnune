#!/usr/bin/env python3
"""Preview or explicitly configure Gnune sharing to private Tailscale HTTPS.

Run on the host as your normal user, with Gnune closed for --apply. The only
privileged command is the exact Serve mount mutation (--privilege sudo/pkexec).
No subnet routes, firewall, tailnet policy, Funnel, or certificate files change.
"""
import argparse
import copy
import fcntl
import http.server
import json
import os
from pathlib import Path
import secrets
import socketserver
import stat
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
from urllib.parse import urlsplit

TAILSCALE = "/usr/bin/tailscale"
MAX_SETTINGS = 32 * 1024 * 1024


def read_settings(path):
    metadata = path.lstat()
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.getuid():
        raise RuntimeError("Settings must be a regular file owned by your user, not a symlink")
    if metadata.st_size > MAX_SETTINGS:
        raise RuntimeError("Sharing settings are unexpectedly large")
    data = path.read_bytes()
    value = json.loads(data)
    connection = value.get("connection")
    if not isinstance(connection, dict) or not all(isinstance(connection.get(k), str) for k in ("bind", "origin")):
        raise RuntimeError("Expected saved Gnune sharing settings with an endpoint")
    if not connection.get("serve_socket") and not all(isinstance(connection.get(k), str) for k in ("cert", "key")):
        raise RuntimeError("Native TLS sharing settings require certificate and key paths")
    return data, value


def atomic_json(path, value, expected=None, expected_missing=False):
    def check_missing():
        if expected_missing and (path.exists() or path.is_symlink()):
            raise RuntimeError("Sharing settings changed during setup; no endpoint update made")
    check_missing()
    if expected is not None and path.read_bytes() != expected:
        raise RuntimeError("Sharing settings changed during setup; no endpoint update made")
    fd, name = tempfile.mkstemp(prefix=".gnune-setup-", dir=path.parent)
    try:
        with os.fdopen(fd, "w") as stream:
            json.dump(value, stream, indent=2)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        if expected is not None and path.read_bytes() != expected:
            raise RuntimeError("Sharing settings changed during setup; no endpoint update made")
        check_missing()
        os.replace(name, path)
        directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        if os.path.exists(name):
            os.unlink(name)


def require_closed():
    for directory in Path("/proc").iterdir():
        if not directory.name.isdecimal():
            continue
        try:
            if directory.stat().st_uid == os.getuid() and (directory / "comm").read_text().strip() == "gnune":
                raise RuntimeError("Close all Gnune instances before applying or undoing setup")
        except (FileNotFoundError, PermissionError, ProcessLookupError):
            continue


class UncertainServe(RuntimeError):
    """The CLI may still complete; keep the recovery receipt for inspection."""


class Commands:
    def __init__(self, privilege="none"):
        self.privilege = privilege

    def call(self, arguments, mutation=False):
        command = [TAILSCALE, *arguments]
        if mutation and self.privilege != "none":
            binary = Path(TAILSCALE).resolve()
            info = binary.stat()
            if info.st_uid != 0 or info.st_mode & 0o022:
                raise RuntimeError("Privileged Serve requires the root-owned system tailscale binary")
            command.insert(0, "/usr/bin/" + self.privilege)
        try:
            result = subprocess.run(command, capture_output=True, text=True, timeout=90 if mutation else 20)
        except subprocess.TimeoutExpired as error:
            if mutation:
                raise UncertainServe("Serve command timed out; inspect Serve status and use --undo before retrying. The recovery receipt was retained") from error
            raise RuntimeError("Tailscale status timed out; no mutation was requested") from error
        except OSError as error:
            raise RuntimeError("Tailscale command unavailable; inspect Serve status before retrying") from error
        if result.returncode:
            # Do not print status payloads, authentication URLs, or arbitrary CLI logs.
            raise RuntimeError("Tailscale command failed. For a Serve permission error, rerun with --privilege pkexec or sudo; only the exact Serve command is elevated")
        return result.stdout

    def status(self):
        return json.loads(self.call(["status", "--json"]))

    def serve(self):
        value = json.loads(self.call(["serve", "status", "--json"]))
        if value is None:
            value = {}
        if not isinstance(value, dict):
            raise RuntimeError("Unrecognized Serve status; refusing changes")
        return value

    def mount(self, port, target, remove=False):
        args = ["serve", f"--https={port}", "--set-path=/"]
        args += ["off"] if remove else ["--bg", target]
        self.call(args, mutation=True)


def authority(origin):
    parsed = urlsplit(origin)
    if parsed.scheme != "https" or not parsed.hostname or parsed.username or parsed.password or parsed.path not in ("", "/") or parsed.query or parsed.fragment:
        raise RuntimeError("Saved origin must be HTTPS without a path or credentials")
    return parsed.netloc


def hostname(status):
    if status.get("BackendState") != "Running":
        raise RuntimeError("Connect the existing Tailscale client before setup")
    name = status.get("Self", {}).get("DNSName", "").rstrip(".")
    if not name.endswith(".ts.net") or not all(label and all(c.isascii() and (c.isalnum() or c == "-") for c in label) for label in name.split(".")):
        raise RuntimeError("A full Tailscale MagicDNS hostname is required")
    return name.lower()


def target_state(config, host, port, target):
    key = f"{host}:{port}"
    # Only this node port is owned by the migration; other ports/configs survive.
    def uses_port(value):
        if not isinstance(value, dict):
            return False
        if str(port) in value.get("TCP", {}) or any(k.endswith(f":{port}") for k in value.get("Web", {})):
            return True
        return any(uses_port(child) for child in value.values())
    for section in ("Foreground", "Services"):
        if uses_port(config.get(section, {})):
            raise RuntimeError("An existing foreground/service mount uses the requested Serve port")
    if any(enabled and name.endswith(f":{port}") for name, enabled in config.get("AllowFunnel", {}).items()):
        raise RuntimeError("Funnel uses the requested port; setup never modifies Funnel")
    tcp = config.get("TCP", {}).get(str(port))
    web = config.get("Web", {}).get(key)
    if tcp is None and web is None:
        # A different hostname on this port is still a conflict.
        if any(k.endswith(f":{port}") for k in config.get("Web", {})):
            raise RuntimeError("Another Serve hostname uses the requested port")
        return "absent"
    if tcp == {"HTTPS": True} and web == {"Handlers": {"/": {"Proxy": target}}}:
        return "exact"
    raise RuntimeError("The requested Serve port already has a different mount; no changes made")


def without_target(config, host, port):
    value = copy.deepcopy(config)
    for section, key in (("TCP", str(port)), ("Web", f"{host}:{port}"), ("AllowFunnel", f"{host}:{port}")):
        if section in value:
            value[section].pop(key, None)
            if not value[section]:
                del value[section]
    return value


def secure_directory(path):
    path.mkdir(mode=0o700, exist_ok=True)
    info = path.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise RuntimeError("The private socket directory must be owned by your user with mode 0700")


class Probe:
    """Temporary, nonce-bound setup responder: never serves the player or tokens."""
    def __init__(self, path, external):
        self.path, self.external = path, external
        self.nonce = secrets.token_hex(24)
        self.server = None
        self.thread = None

    def __enter__(self):
        if self.path.exists() or self.path.is_symlink():
            raise RuntimeError("Socket path is occupied. Close Gnune; remove only a confirmed stale socket before retrying")
        probe = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                valid = (self.path == "/__gnune_setup/" + probe.nonce
                         and self.headers.get("Host") == "localhost"
                         and self.headers.get("X-Forwarded-Host") == probe.external
                         and self.headers.get("X-Forwarded-Proto") == "https"
                         and self.headers.get("Tailscale-Funnel-Request") is None)
                body = probe.nonce.encode() if valid else b"Unavailable"
                self.send_response(200 if valid else 403)
                self.send_header("Content-Length", str(len(body)))
                self.send_header("Cache-Control", "no-store")
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *_):
                pass

        self.server = socketserver.UnixStreamServer(str(self.path), Handler)
        os.chmod(self.path, 0o600)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        return self

    def __exit__(self, *_):
        if self.server:
            self.server.shutdown()
            self.server.server_close()
        if self.thread:
            self.thread.join()
        self.path.unlink(missing_ok=True)

    def ready(self, origin):
        class NoRedirect(urllib.request.HTTPRedirectHandler):
            def redirect_request(self, *_args, **_kwargs):
                return None
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
        deadline = time.monotonic() + 75
        print("Checking managed HTTPS and private proxy headers (up to 75 seconds)…")
        while time.monotonic() < deadline:
            try:
                with opener.open(origin + "/__gnune_setup/" + self.nonce, timeout=min(10, max(1, deadline - time.monotonic()))) as response:
                    if response.status == 200 and response.read(128) == self.nonce.encode():
                        return
            except (OSError, ValueError):
                pass
            time.sleep(min(1, max(0, deadline - time.monotonic())))
        raise RuntimeError("Managed HTTPS/forwarded headers are not ready; saved endpoint was not changed")


def migrate(path, commands, apply=False, undo=False, port=None):
    receipt_path = path.with_name("tailscale-setup.json")
    if undo:
        return undo_setup(path, commands, apply)
    try:
        original, saved = read_settings(path)
    except FileNotFoundError:
        if path.is_symlink():
            raise RuntimeError("Settings must not be a symlink") from None
        original, saved = None, None
    status = commands.status()
    host = hostname(status)
    previous = copy.deepcopy(saved["connection"]) if saved else None
    old_origin = urlsplit(previous["origin"]) if previous else None
    if previous:
        authority(previous["origin"])
    port = port or (old_origin.port if old_origin else None) or 443
    if not 1 <= port <= 65535:
        raise RuntimeError("Port must be in 1..65535")
    socket_path = path.parent / "tailscale-private" / "api.sock"
    if len(os.fsencode(socket_path)) > 103:
        raise RuntimeError("Private Unix socket path is too long; choose a shorter Gnune config path")
    origin = f"https://{host}" + (f":{port}" if port != 443 else "")
    external = authority(origin)
    target = f"unix:{socket_path}"
    before = commands.serve()
    existing = target_state(before, host, port, target)
    endpoint = {**(previous or {}), "bind": f"127.0.0.1:{port}", "origin": origin, "serve_socket": str(socket_path)}
    receipt = None
    if receipt_path.exists():
        receipt = json.loads(receipt_path.read_text())
        if receipt.get("state") != "applied" or receipt.get("after") != endpoint:
            raise RuntimeError("A prior setup receipt needs review; preview --undo before retrying")
        previous = receipt["before"]
    print(f"Settings: {path}")
    print(f"Stable phone origin: {origin}")
    print(f"Backend: private Unix socket {socket_path} (no TCP listener)")
    if previous is None:
        print("First-time setup leaves sharing off. Reopen Gnune and choose Pair phone to share music, videos and podcasts.")
    else:
        print("Existing sharing consent, queue, grants and TLS fallback paths are retained.")
        print("New hostname means one new phone pairing/Home Screen install; old offline audio remains at its old origin.")
    print(f"Serve mount: {existing}; no routes, firewall, tailnet policy or Funnel changes.")
    if not apply:
        print("Preview only. Close Gnune, then repeat with --apply. Enable tailnet HTTPS first if needed.")
        return
    if host not in [str(v).rstrip(".").lower() for v in status.get("CertDomains", [])]:
        raise RuntimeError("Enable HTTPS certificates for this existing tailnet in Tailscale DNS settings first; setup does not enable it or request other certificates")
    require_closed()
    secure_directory(socket_path.parent)
    if receipt and existing == "absent":
        raise RuntimeError("The saved migration mount is missing; preview --undo before recreating it")
    owned = receipt.get("owned", False) if receipt else existing == "absent"
    updated = copy.deepcopy(saved) if saved else {
        "enabled": False, "mode": "library", "include_video": True,
        "include_podcasts": True, "queue": None, "skipped": 0,
    }
    updated["connection"] = endpoint
    record = {"state": "preparing", "before": previous, "after": endpoint, "owned": owned, "host": host, "port": port, "target": target}
    if previous is None:
        record["initial_settings"] = receipt.get("initial_settings", updated) if receipt else updated
    atomic_json(receipt_path, record)
    created = False
    endpoint_saved = False
    try:
        with Probe(socket_path, external) as probe:
            if commands.serve() != before:
                raise RuntimeError("Serve changed during setup; no mount changes made")
            if existing == "absent":
                # Record ownership before the mutation so interrupted applies can
                # safely undo only this exact mount, without a Serve reset.
                created = True
                commands.mount(port, target)
            after = commands.serve()
            if target_state(after, host, port, target) != "exact" or without_target(after, host, port) != without_target(before, host, port):
                raise RuntimeError("Unexpected Serve change; inspect status before continuing")
            probe.ready(origin)
            require_closed()
            if saved is None or saved["connection"] != endpoint:
                atomic_json(path, updated, expected=original, expected_missing=saved is None)
            endpoint_saved = True
        record["state"] = "applied"
        atomic_json(receipt_path, record)
        print("Stable endpoint saved. Reopen the rebuilt Gnune and choose Pair phone. Sharing stays off until then." if previous is None else
              "Stable endpoint saved. Reopen the rebuilt Gnune, then pair at the new QR.")
        print("Keep Tailscale connected on desktop and phone.")
    except Exception as error:
        if isinstance(error, UncertainServe):
            raise
        # Once the endpoint is committed, preserve the verified mount and receipt;
        # --undo can recover if receipt finalization alone was interrupted.
        if not endpoint_saved:
            try:
                current = commands.serve()
                if created and target_state(current, host, port, target) == "exact" and without_target(current, host, port) == without_target(before, host, port):
                    commands.mount(port, target, remove=True)
                if commands.serve() == before:
                    if receipt:
                        atomic_json(receipt_path, receipt)
                    else:
                        receipt_path.unlink(missing_ok=True)
                else:
                    raise RuntimeError("Serve differs from its original state")
            except Exception:
                print("Setup did not finish; exact Serve cleanup needs review with --undo. Other Serve state was left alone.", file=sys.stderr)
        raise


def undo_setup(path, commands, apply):
    receipt_path = path.with_name("tailscale-setup.json")
    record = json.loads(receipt_path.read_text())
    try:
        original, saved = read_settings(path)
    except FileNotFoundError:
        if path.is_symlink() or record["before"] is not None:
            raise
        original, saved = None, None
    bootstrap = record["before"] is None
    if bootstrap:
        if saved is not None and saved != record.get("initial_settings"):
            raise RuntimeError("Sharing settings changed after first-time setup; stop sharing and review settings before undo. Nothing was removed")
    elif saved["connection"] not in (record["before"], record["after"]):
        raise RuntimeError("Saved endpoint changed after setup; undo refuses to overwrite it")
    config = commands.serve()
    existing = target_state(config, record["host"], record["port"], record["target"])
    print("Undo removes unchanged first-time endpoint settings; phone grants and saved media stay unchanged." if bootstrap else
          "Undo restores only the previous endpoint; current sharing scope/consent and unrelated Serve state stay unchanged.")
    print("The helper-created exact Serve mount will be removed." if record["owned"] else "The pre-existing adopted Serve mount will be retained.")
    if not apply:
        print("Preview only. Close Gnune, then repeat --undo --apply.")
        return
    require_closed()
    if record["owned"] and existing == "exact":
        if commands.serve() != config:
            raise RuntimeError("Serve changed during undo; refusing changes")
        commands.mount(record["port"], record["target"], remove=True)
        after = commands.serve()
        if target_state(after, record["host"], record["port"], record["target"]) != "absent" or without_target(after, record["host"], record["port"]) != without_target(config, record["host"], record["port"]):
            raise RuntimeError("Unexpected Serve state after undo; saved endpoint was not changed")
    if bootstrap:
        if original is not None:
            if path.read_bytes() != original:
                raise RuntimeError("Sharing settings changed during undo; no settings were removed")
            path.unlink()
    else:
        saved["connection"] = record["before"]
        atomic_json(path, saved, expected=original)
    receipt_path.unlink()
    print("First-time endpoint removed. Phone grants and saved media were not removed." if bootstrap else
          "Previous endpoint restored. Reopen Gnune; existing old-origin phone data was not removed.")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group()
    group.add_argument("--settings", type=Path, help="explicit saved sharing.json")
    group.add_argument("--flatpak", action="store_true", help="use this user's org.gnune.Player config")
    parser.add_argument("--port", type=int, help="Serve port (default: saved origin port, otherwise 443)")
    parser.add_argument("--privilege", choices=("none", "sudo", "pkexec"), default="none", help="elevate only the exact Serve add/remove command")
    parser.add_argument("--apply", action="store_true")
    parser.add_argument("--undo", action="store_true")
    args = parser.parse_args(argv)
    if os.geteuid() != os.getuid() or os.geteuid() == 0:
        raise RuntimeError("Run this helper as your regular user; use --privilege for the single Serve command")
    path = args.settings or (Path.home() / ".var/app/org.gnune.Player/config/gnune/sharing.json" if args.flatpak else Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config")) / "gnune/sharing.json")
    path = path.absolute()
    if not args.apply:
        migrate(path, Commands(args.privilege), undo=args.undo, port=args.port)
        return
    require_closed()
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    # Compatible with std::fs::File::try_lock_shared in the desktop process.
    lock_fd = os.open(path.with_suffix(".lock"), os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    try:
        fcntl.flock(lock_fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        migrate(path, Commands(args.privilege), apply=True, undo=args.undo, port=args.port)
    finally:
        os.close(lock_fd)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, RuntimeError) as error:
        print(f"Gnune setup: {error}", file=sys.stderr)
        sys.exit(1)
