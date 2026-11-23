# Gnune 0.3.0

- Discover podcasts through the cached Podcast Index recommendations, full-directory
  search or OPML lists. Subscribe to shows, browse their summaries and episodes,
  stream or download audio, and choose oldest-first or newest-first playback.
  Quickplay resumes the last episode; stale feeds refresh automatically.
- Share the full music collection, playlists, videos and subscribed podcasts with
  a phone over Tailscale HTTPS. Phone Quickplay includes pins, history and podcast
  resume; episode order, completion and listening position sync both ways.
  Existing phone grants keep their original scope until paired again.
- Set up Tailscale Serve with the host helper’s preview/apply/undo workflow. The
  app checks its managed HTTPS endpoint before displaying the pairing QR
  and retries when the connection returns. The backend uses a private Unix socket.
- Keep window buttons visible on a transparent overlay, rewind playback by ten
  seconds from the transport, and cycle Smart DJ artist suggestions in Quickplay.
  Refresh the phone Home Screen icon from the current Gnune artwork.

The x86_64 Flatpak uses GNOME Platform 50. The x86_64 AppImage requires glibc 2.39
or newer. Flatpak keeps its library, pairing data and configuration separate from
host installations. For Flatpak phone sharing, run the setup helper on the host
with `--flatpak` to target its separate settings and private Serve socket. There
is no LAN-address pairing fallback. See the README for installation, host setup
and local data locations. Source archives and dependency notices accompany the
packages.

Phone streaming requires Tailscale on both devices, tailnet access to the managed
HTTPS service, and the desktop app to remain open. Setup requires tailnet HTTPS
to be enabled and runs separately from Gnune; scanning the QR does not configure
Tailscale. Changing the browser origin requires pairing there again; offline audio
and pending progress remain with the original browser installation. Gnune does
not import or sync the iPhone Music library. Played music can be saved locally;
there is no bulk playlist download. Videos stream in their original codec and
remain online-only. Offline storage and codec support depend on the phone browser.
Podcast discovery and uncached feeds need an internet connection.

Simulated browser tests and private desktop sessions do not establish physical-phone,
GNOME/Mutter, physical-audio or accessibility acceptance.
