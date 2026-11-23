# Build Gnune Flatpak

Install GNOME SDK and Platform 50 (`flatpak install flathub org.gnome.Sdk//50
org.gnome.Platform//50`), Rust stable and Python 3.11 or newer. Fetch locked Rust crates with
`cargo fetch --locked` once. Then, from the source checkout:

```sh
python3 packaging/flatpak/build.py --work "$PWD/target/pkg-work/flatpak" --jobs 6
```

The recipe downloads checksum-pinned native sources, vendors Cargo.lock's Rust
sources, and compiles offline inside GNOME SDK 50. All generated output stays in
`--work`; it does not install Gnune or modify Flatpak remotes. Use a new work
directory for a clean rebuild. The SDK and runtime commit IDs, source hashes,
corresponding sources accompany the built package. Configure/build logs remain
in the builder workspace.

The artifact is `target/pkg-work/flatpak/dist/Gnune-0.3.0-x86_64.flatpak`.
Install with `flatpak install --user ./Gnune-0.3.0-x86_64.flatpak`, then run
`flatpak run org.gnune.Player`. Flatpak may prompt to install GNOME Platform 50.

Permissions: Wayland/fallback X11, PulseAudio, direct rendering, shared memory,
network for metadata, feed downloads and local phone pairing, read-only Music and Videos
folders for local imports, and read-only Pictures for artwork.
The file chooser grants access to individually selected files outside those folders.
Phone pairing requires the computer's firewall/network to allow its chosen port.
The named `org.bluez` system-bus permission lets Devices read connected-phone status.
No access to arbitrary home files or removable media is granted by default.

Phone-sharing settings and credentials are separate from a host Cargo/AppImage
installation. Flatpak's config directory is
`~/.var/app/org.gnune.Player/config/gnune` and its collection/pairing data is in
`~/.var/app/org.gnune.Player/data/gnune`. Host settings and phone pairings are not
imported automatically. For Tailscale **Devices → Pair phone**, run the setup
helper on the host with `--flatpak` for preview, apply and undo. This targets the
Flatpak app’s own settings and private Unix socket under its config directory.
Close all Gnune instances before apply or undo. See the main README for commands
and Tailscale prerequisites. No LAN-address pairing fallback is offered.

See THIRD_PARTY.md for dependency licensing. Native FFmpeg is built with built-in
software codecs and Chromaprint, without GPL/nonfree libraries or hardware codecs.
Embedded video uses GNOME runtime GStreamer with a bundled gst-libav plugin linked
to private FFmpeg8; optional codecs-extra extensions are not required.
Both codec paths are checked independently during release QA.
