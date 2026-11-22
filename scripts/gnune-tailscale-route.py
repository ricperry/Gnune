#!/usr/bin/env python3
"""Advertise only Gnune's own LAN address through an existing Tailscale setup.

Preview: python3 scripts/gnune-tailscale-route.py --host LAN_IP --phone PHONE_IP
Apply:   repeat with --apply (use sudo only if Tailscale requires it).
Remove:  python3 scripts/gnune-tailscale-route.py --host LAN_IP --remove --apply

Install/review the printed tailnet policy BEFORE applying. This script cannot
inspect or edit that policy. It only changes this node's advertised-route list;
it does not configure forwarding, firewalld, masquerading, or Gnune itself.
Avoid concurrent Tailscale preference changes while running it.
"""

import argparse
import ipaddress
import json
import shlex
import subprocess
import sys


DEFAULT_ROUTES = {"0.0.0.0/0", "::/0"}
PRIVATE_LANS = tuple(ipaddress.ip_network(net) for net in (
    "10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16"
))


def host_address(value):
    """Accept a single RFC1918 IPv4 address, never a whole LAN or URL."""
    try:
        address = ipaddress.IPv4Address(value)
    except ipaddress.AddressValueError as error:
        raise argparse.ArgumentTypeError("expected a LAN IPv4 address, without /prefix") from error
    if not any(address in network for network in PRIVATE_LANS):
        raise argparse.ArgumentTypeError("host must be an RFC1918 LAN IPv4 address")
    return str(address)


def phone_address(value):
    try:
        address = ipaddress.IPv4Address(value)
    except ipaddress.AddressValueError as error:
        raise argparse.ArgumentTypeError("expected the iPhone's Tailscale IPv4 address") from error
    if address not in ipaddress.ip_network("100.64.0.0/10"):
        raise argparse.ArgumentTypeError("phone must be a Tailscale IPv4 address in 100.64.0.0/10")
    return str(address)


def run(command):
    try:
        result = subprocess.run(command, capture_output=True, text=True, check=True, timeout=20)
    except FileNotFoundError as error:
        raise RuntimeError(f"required command not found: {command[0]}") from error
    except subprocess.TimeoutExpired as error:
        raise RuntimeError(f"{command[0]} timed out; inspect its state before retrying") from error
    except subprocess.CalledProcessError as error:
        detail = (error.stderr or "").strip()
        raise RuntimeError(f"{command[0]} failed: {detail or error.returncode}") from error
    return result.stdout


def read_json(command):
    try:
        return json.loads(run(command))
    except json.JSONDecodeError as error:
        raise RuntimeError(f"{command[0]} returned invalid JSON; inspect its state before retrying") from error


def advertised_routes():
    prefs = read_json(["tailscale", "debug", "prefs"])
    if not isinstance(prefs, dict) or "AdvertiseRoutes" not in prefs:
        raise RuntimeError("Tailscale preferences do not contain AdvertiseRoutes")
    values = prefs["AdvertiseRoutes"]
    if values is None:
        values = []
    if not isinstance(values, list):
        raise RuntimeError("unexpected AdvertiseRoutes format")
    routes = []
    for value in values:
        try:
            if not isinstance(value, str):
                raise ValueError("route is not text")
            route = str(ipaddress.ip_network(value, strict=True))
            if route != value:
                raise ValueError("route is not canonical")
        except ValueError as error:
            raise RuntimeError("invalid existing advertised route; refusing to replace preferences") from error
        if route in routes:
            raise RuntimeError("duplicate existing advertised route; refusing to replace preferences")
        routes.append(route)
    defaults = DEFAULT_ROUTES.intersection(routes)
    if defaults and defaults != DEFAULT_ROUTES:
        raise RuntimeError("incomplete exit-node default routes; refusing to change preferences")
    return routes


def require_local_host(host):
    interfaces = read_json(["ip", "-j", "-4", "address", "show"])
    if not isinstance(interfaces, list):
        raise RuntimeError("unexpected ip address output")
    for interface in interfaces:
        if not isinstance(interface, dict) or not isinstance(interface.get("addr_info"), list):
            raise RuntimeError("unexpected ip address output")
        if interface.get("ifname") == "tailscale0":
            continue
        if any(isinstance(info, dict) and info.get("local") == host
               and info.get("scope") == "global" for info in interface["addr_info"]):
            return
    raise RuntimeError(f"{host} is not assigned to this computer; no changes made")


def policy_instructions(host, phone):
    grant = {"src": [phone], "dst": [host], "ip": ["tcp:8443"]}
    print("Before applying, review https://login.tailscale.com/admin/acls")
    print("Merge this entry into the existing grants array (do not replace the policy):")
    print(json.dumps(grant, indent=2))
    print("Grants are additive: remove/restrict overlapping allow-all grants or ACLs if")
    print("only this iPhone and TCP 8443 should have access. This grant alone does not")
    print("restrict a permissive policy. Review route autoApprovers BEFORE advertising:")
    print("an existing auto-approval may make the route active immediately on --apply.")
    print("After --apply, approve ONLY this /32 (unless already auto-approved) under")
    print("the desktop's machine > Edit route settings:")
    print("https://login.tailscale.com/admin/machines")
    print("The phone must use Tailscale with subnet routes enabled. Keep its existing")
    print(f"Gnune origin https://{host}:8443 and existing pairing/certificate trust.")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--host", required=True, type=host_address, help="desktop's own LAN IPv4 address")
    parser.add_argument("--phone", type=phone_address, help="already paired iPhone's Tailscale IPv4 address")
    parser.add_argument("--remove", action="store_true", help="withdraw only this exact /32; phone/local IP may be absent")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--preview", action="store_true", help="read-only preview (the default)")
    mode.add_argument("--apply", action="store_true", help="change advertised routes after reviewing the preview/policy")
    args = parser.parse_args(argv)
    if not args.remove and not args.phone:
        parser.error("--phone is required unless --remove is used")
    if not args.remove:
        require_local_host(args.host)
    before = advertised_routes()
    target = f"{args.host}/32"
    after = [route for route in before if route != target]
    if not args.remove:
        after.append(target)
        policy_instructions(args.host, args.phone)
    else:
        print("Withdraw only the exact /32; broader existing routes may still cover this IP.")
        print("The tailnet grant is left for you to review/remove separately in the admin console.")
    # `tailscale set` preserves the existing exit-node default routes when only
    # --advertise-routes is supplied. Do not toggle --advertise-exit-node.
    # Upstream: cmd/tailscale/cli/set.go, calcAdvertiseRoutesForSet.
    routes_flag = ",".join(route for route in after if route not in DEFAULT_ROUTES)
    command = ["tailscale", "set", f"--advertise-routes={routes_flag}"]
    print(f"\nExact target: {target}; existing exit-node advertisement: {bool(DEFAULT_ROUTES.intersection(before))}")
    print("Planned command:", shlex.join(command))
    print("Other advertised routes and unrelated Tailscale preferences are retained.")
    print("The destination terminates on this host. Existing firewall rules must permit")
    print("incoming Tailscale traffic to TCP 8443; this script changes no firewall or forwarding settings.")
    if set(before) == set(after):
        print("Already in the requested state; no changes needed.")
        return
    if not args.apply:
        print("Preview only. Review the policy first, then repeat with --apply.")
        return
    if advertised_routes() != before:
        raise RuntimeError("advertised routes changed during preview; rerun to inspect the new state")
    run(command)
    if set(advertised_routes()) != set(after):
        raise RuntimeError("post-change routes differ from the plan; inspect Tailscale preferences before retrying")
    print("Advertised routes verified. Tailnet policy/route approval and Gnune reachability require separate verification.")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError) as error:
        print(f"Error: {error}", file=sys.stderr)
        sys.exit(1)
