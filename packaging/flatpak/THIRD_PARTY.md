# Flatpak media dependencies

Gnune's original code is GNU GPL version 3 or later. This package builds all added
media helpers from the exact unmodified upstream archives pinned in sources.json.
No proprietary codec library, host RPM, or prebuilt third-party executable is copied.

- mpv 0.41.0: GPL version 2 or later; a separately invoked upstream executable. Upstream archive includes Copyright, LICENSE.GPL and LICENSE.LGPL.
  PulseAudio and null output; no scripting engines, display outputs or plugins.
- FFmpeg 8.1.2: LGPL version 2.1 or later. Configured with --disable-gpl,
  --disable-nonfree and --disable-version3. Native software codecs, zlib, bzip2,
  liblzma and Chromaprint; no x264/x265, NVIDIA or AMD proprietary codec library.
- Chromaprint 1.5.1: LGPL version 2.1 or later; bundled KissFFT implementation
  is BSD-3-Clause. Shared library; FFTW is disabled.
- gst-libav 1.26.11: LGPL version 2.1 or later. Built against private FFmpeg8
  to provide software H.264/AAC decoding without optional runtime codecs.
- libass 0.17.3: ISC. Required by mpv, uses runtime FreeType/HarfBuzz/FriBidi.
- libplacebo 7.351.0: LGPL version 2.1 or later. Required by mpv; GPU renderers,
  shader compilers, demos and optional libraries are disabled in this audio helper.

The original source archives and the vendored Gnune/Rust source archive are installed
under /app/share/gnune/sources. They preserve exact copyright and license texts,
including notices for native bundled subcomponents. The Gnune source archive includes
the complete packaging recipe. Runtime/SDK commit identifiers and source hashes are installed under
/app/share/gnune. Build logs remain only in the builder workspace. Libraries remain dynamically linked and replaceable;
Flatpak build permits rebuilding the app from the provided source and recipe.

GTK4, GStreamer (including the MPL-2.0 gtk4paintablesink), curl, OpenSSL and the
core plugins come from the separately installed GNOME 50 runtime. Embedded
software codecs use the bundled gst-libav plugin and private FFmpeg8 libraries,
so optional codecs-extra extensions are not required. These runtime
components are maintained and distributed by their upstream Flatpak projects,
with runtime licensing and source availability independent of this application bundle.
The application never requests proprietary driver extensions.

The locked Rust crate notice inventory is installed alongside this notice. Every
vendored Rust source and its upstream license files is retained in the source archive.
