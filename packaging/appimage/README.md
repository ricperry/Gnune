# AppImage packaging

Using Python 3.11 or newer, this builds the x86_64 AppImage using the same GNOME 50 SDK and source-built
FFmpeg/Chromaprint/mpv prefix as `packaging/flatpak/build.py`. Build that recipe's
native prefix and Rust binary first. No system installation or publishing occurs.
The packaging work directory must be new: an existing AppDir is never overwritten.

```sh
python3 packaging/appimage/build.py \
  --root "$PWD" \
  --sdk-build "$PWD/target/pkg-work/flatpak/sdk" \
  --binary "$PWD/target/pkg-work/flatpak/sdk/files/bin/gnune" \
  --media-prefix "$PWD/target/pkg-work/flatpak/sdk/files" \
  --work /tmp/gnune-appimage-build \
  --output "$PWD/dist/Gnune-0.3.0-x86_64.AppImage" \
  --appimagetool /path/to/appimagetool-1.9.1-x86_64.AppImage \
  --runtime-file /path/to/appimagetool-1.9.1-runtime-x86_64
```

Use a private, writable `XDG_RUNTIME_DIR` when building in an isolated execution
environment. The SDK and Platform commits are recorded in appimage-build-info.json;
reinstall those recorded commits to rebuild the identical dependency inputs. The
packager records source and binary hashes; bit-for-bit reproducibility across
different build timestamps and compilers is not claimed.

The AppImage tool is pinned to SHA256
`ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0`, from
https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage.
The supplied static type2 runtime, commit `caf24f9`, is pinned to
`d30a3ba1388ef57be73faabf606e0d326682f48c87f666106a3c8d6b35b58b4f`.
Extract the runtime directly from the checksum-verified tool without FUSE:

```sh
tool=/path/to/appimagetool-1.9.1-x86_64.AppImage
head -c "$("$tool" --appimage-offset)" "$tool" > /path/to/appimagetool-1.9.1-runtime-x86_64
```

Its source is at
https://github.com/AppImage/type2-runtime/tree/caf24f9; its embedded
`--appimage-help` contains the runtime and vendored-library license links.

The payload includes GTK4, GStreamer and a bounded playback codec/plugin set,
`gtk4paintablesink`, mpv, FFmpeg/ffprobe, curl, GTK schemas, icons, MIME data,
GdkPixbuf loaders, Glycin JPEG/PNG/SVG loaders, bubblewrap and TLS GIO modules.
It retains the SDK manifest (exact dependency source commits and patch paths),
SDK license notices and Gnune/Rust notices. The release's source archives provide
Gnune, Rust crates and the native media dependency sources and build recipes.
Runtime source/build metadata are provided by the matching SDK-source companion
release asset and the recorded SDK manifest. Upstream build recipes are maintained at: https://gitlab.gnome.org/GNOME/gnome-build-meta and
https://gitlab.com/freedesktop-sdk/freedesktop-sdk.

The host supplies glibc, its ELF loader, graphics/Vulkan/DRM libraries and drivers,
font configuration/fonts, a Wayland or X11 session, system CA trust and an audio
server. No proprietary drivers are bundled. The measured glibc floor is written
into appimage-build-info.json; this is not an AppImage for older Linux releases.
Use the Flatpak for systems whose glibc is older. Normal AppImage mounting needs
FUSE; `APPIMAGE_EXTRACT_AND_RUN=1 ./Gnune-0.3.0-x86_64.AppImage` runs without FUSE.

The launcher uses only its bundled plugin directory and a Gnune-specific registry,
relocates loader definitions into a private temporary directory, and honors custom
CA trust configuration. The launcher uses `CURL_CA_BUNDLE`, then `SSL_CERT_FILE`,
or a readable host CA bundle for both curl and GIO/GStreamer HTTPS playback. It
passes the selected PEM file through `GNUNE_TLS_CA_FILE`; before streaming,
Gnune loads it as GIO’s default trust database, including for HTTP streams that
redirect to HTTPS. An unreadable or invalid bundle blocks that stream and reports
a trust configuration error; local playback remains available. Certificate and
hostname verification remain enabled. Native and Flatpak launches retain
their default GIO trust database. Extracting the AppImage permits replacing LGPL shared
libraries. Close Gnune before replacing its extracted shared libraries.

For isolated playback QA, generate synthetic fixtures using the host FFmpeg's
libx264/libvpx/libmp3lame encoders and run the AppImage under private Sway/DBus:

```sh
packaging/appimage/generate-fixtures.sh /tmp/gnune-fixtures
GNUNE_QA_ROOT=/tmp/gnune-qa \
GNUNE_QA_MEDIA=/tmp/gnune-fixtures \
GNUNE_QA_APPIMAGE="$PWD/dist/Gnune-0.3.0-x86_64.AppImage" \
  packaging/appimage/check-private-session.sh
```

This uses null audio, software rendering, temporary XDG homes, a private Wayland
socket and no personal desktop/browser. It checks WAV, embedded PNG/FLAC artwork,
embedded JPEG/MP3 artwork, H264/AAC and VP8 playback plus pause/seek and a smaller
window. Inspect its screenshots and saved progress values. Accessibility is
disabled only in this test session; this does not establish GNOME/Mutter, hardware
video acceleration, physical audio, human usability or screen-reader acceptance.
