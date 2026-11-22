# Build Gnune Flatpak

Install GNOME SDK and Platform 50 (`flatpak install flathub org.gnome.Sdk//50
org.gnome.Platform//50`), Rust stable and Python 3. Fetch locked Rust crates with
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

The artifact is `target/pkg-work/flatpak/dist/Gnune-0.1.0-x86_64.flatpak`.
Install with `flatpak install --user ./Gnune-0.1.0-x86_64.flatpak`, then run
`flatpak run org.gnune.Player`. Flatpak may prompt to install GNOME Platform 50.

Permissions: Wayland/fallback X11, PulseAudio, direct rendering, shared memory,
network for metadata, feed downloads and local phone pairing, read-only Music and Videos
folders for local imports, and read-only Pictures for artwork.
The file chooser grants access to individually selected files outside those folders.
Phone pairing requires the computer's firewall/network to allow its chosen port.
No access to arbitrary home files or removable media is granted by default.

See THIRD_PARTY.md for dependency licensing. Native FFmpeg is built with built-in
software codecs and Chromaprint, without GPL/nonfree libraries or hardware codecs.
Embedded video uses GNOME runtime GStreamer with a bundled gst-libav plugin linked
to private FFmpeg8; optional codecs-extra extensions are not required.
Both codec paths are checked independently during release QA.
