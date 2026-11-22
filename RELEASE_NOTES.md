# Gnune 0.2.0

- Browse artists, albums, songs and saved playlists on a paired phone. Full music
  library sharing is separate from queue sharing, and videos require a separate
  grant. QR pairing is generated locally.
- Remember sharing scope and connection settings across desktop launches, retry
  unavailable addresses, and recover phone credentials when reconnecting. Change
  pairing inside an existing Home Screen app while retaining saved audio and
  progress under their original scope.
- Fetch phone audio only when it becomes current. Played songs can be saved for
  offline use; video streams use the original codec and remain online-only.
- Improve compilation and multi-disc album grouping, collection navigation and
  album context menus. Show connected phones through read-only BlueZ status.
- Include the host-side Tailscale route preview helper, phone regression tests
  and updated dependency notices in the corresponding sources.

The x86_64 Flatpak uses GNOME Platform 50. The x86_64 AppImage requires glibc 2.39
or newer. See the README for installation, sharing setup and local data locations.
Flatpak keeps its library, pairing data and configuration separate from host
installations.

Phone sharing requires a reachable HTTPS origin trusted by the phone, suitable
firewall rules, and the desktop app to remain open. It does not import or sync
the iPhone Music library. Offline storage and codec support depend on the phone
browser. Simulated browser tests and private desktop sessions do not establish
physical-phone, GNOME/Mutter, physical-audio or accessibility acceptance.
