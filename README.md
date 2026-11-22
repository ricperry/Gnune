# Gnune

Gnune (pronounced **“noon”**) is a local-first Rust and GTK4 music player modeled on the Microsoft Zune desktop software. The collection arranges artists, albums and songs in coordinated panes on a dark canvas. Now Playing keeps the sleeve sharp against a soft backdrop sampled from its colors. Full screen expands the whole sleeve to fit the display.

With an existing music library, Gnune opens on Quickplay, with sleeves for pins, newly added music and listening history. A first launch opens the empty collection so you can add a folder. [Quickplay in the Zune tutorial](https://www.youtube.com/watch?v=A1iqtznwpCA&t=31s)

The desktop reference shows the collection at 2:00 and the artwork, diagonal typography and queue that inspired Artist Showcase at 2:40. [Collection in the Zune tutorial](https://www.youtube.com/watch?v=A1iqtznwpCA&t=120s) · [Now Playing in the same recording](https://www.youtube.com/watch?v=A1iqtznwpCA&t=160s)

## Install on Linux

Download the x86_64 Flatpak or AppImage from the [GitHub releases](https://github.com/ricperry/Gnune/releases/latest).

To install the Flatpak for your account, run these commands from the download directory. Flatpak uses the GNOME 50 runtime from Flathub.

```sh
flatpak install --user ./Gnune-0.2.0-x86_64.flatpak
flatpak run org.gnune.Player
```

The Flatpak has its own collection and settings. On first launch, use **Add music folder** to select your library, including a folder on an external drive.

The x86_64 AppImage requires glibc 2.39 or newer. To run it directly, make it executable and launch it.

```sh
chmod +x Gnune-0.2.0-x86_64.AppImage
./Gnune-0.2.0-x86_64.AppImage
```

Each release includes `SHA256SUMS`. Download it alongside your package and run `sha256sum --check --ignore-missing SHA256SUMS` to verify the download. See the [release notes](RELEASE_NOTES.md) for changes and limits. Package build instructions are available for [Flatpak](packaging/flatpak/README.md) and [AppImage](packaging/appimage/README.md).

## Build from source

Build with stable Rust and GTK 4.14 or newer. On Fedora, install the native build dependencies and run Gnune from the checkout.

```sh
sudo dnf install gcc pkgconf-pkg-config gtk4-devel gstreamer1-devel openssl-devel
cargo run --locked
cargo run --locked -- "/path/to/an album"
```

`gstreamer1-devel` supplies the `gstreamer-1.0.pc` file Cargo needs to build. Video and downloaded podcast playback use GStreamer, its codec plugins, and the `gstreamer1-plugin-gtk4` video sink. For music, Gnune starts a headless mpv child. Its private playback connection closes when Gnune exits or crashes, which also ends mpv. FFmpeg and ffprobe read video tags, generate posters and extract audio. Fingerprint matching uses FFmpeg’s Chromaprint muxer. Online lookups and feed downloads use curl.

If GTK reports **Unable to connect to the accessibility bus** with **Connection refused** on Fedora, `systemctl --user restart at-spi-dbus-bus.service` restarts the user’s AT-SPI service. Relaunch Gnune so GTK can discover the current bus address.

Use **Add music folder** on an empty collection or the **+** by Settings to import a folder. Choose **Settings → Open file** to play a track. Gnune reads embedded tags, including disc and track numbers. It looks for embedded artwork and for images named `cover`, `folder`, `front`, `album`, or `albumart` beside the tracks. Select an artist to narrow the album and song panes. Select a sleeve to see the album’s tracks, then **Album details → Choose cover** to assign your own artwork. **All artists** and the album pane’s **All** clear the respective filters.

Album tiles appear automatically as you scroll in Collection and artist pages. Right-click a music album, or focus it and press **Menu** or **Shift+F10**, for playback, details, Smart DJ and Quickplay pin actions.

Compilation flags, album-named folders and shared sleeves keep uncredited compilations together as **Various Artists**. Explicit album credits and individual song credits are preserved. Numbered CD/Disc folders share their release. Existing collections update on opening without reimporting, while separate releases remain distinct.

The desktop and phone icons use the updated Gnune mark in `assets/`. The desktop binary embeds the PNG artwork. To give GNOME its launcher entry and icon for a local Cargo installation, run the following from the project root with `~/.cargo/bin` on your desktop session’s PATH.

```sh
cargo install --locked --path .
install -Dm644 data/org.gnune.Player.desktop ~/.local/share/applications/org.gnune.Player.desktop
install -Dm644 src/mobile/icon-512.png ~/.local/share/icons/hicolor/512x512/apps/org.gnune.Player.png
```

## Listening

| Action | Location or shortcut |
|---|---|
| Quickplay, Collection, Now Playing | Ctrl+1, Ctrl+2, Ctrl+3 |
| Back | Left arrow at the start of the navigation, or Alt+Left |
| Window controls | Approach the upper-right corner, or press F10 and use Tab, Shift+Tab and Enter |
| Dismiss keyboard window controls | Escape returns focus to the previous control |
| Move or resize the window | Drag unused space to move, or drag an edge or corner to resize |
| Open a file | Ctrl+O |
| Open the editable queue | Ctrl+J |
| Open lyrics | Ctrl+L |
| Full-screen artwork | Double-click the listening view, press F11 or Alt+Enter, or use the playback menu |
| Leave full screen | Escape |
| Shuffle, repeat, mini player, art wall, artist showcase | Playback menu at the right of the transport |

Mini player fits the transport controls into a small window. Choose **Return to full player** in its playback menu to restore the previous size and maximized or full-screen state. Opening a page from the mini transport returns to the full player. F11 or **Full screen** opens the full interface in full screen. Playback continues across these view changes.

Put a timestamped `.lrc` beside a track with the same filename stem. Select a line to seek, scroll to read ahead, and choose **Follow current line** to return. In **Library and settings**, enable **Look up missing art, lyrics and artist links** to use MusicBrainz, the Cover Art Archive and LRCLIB. Matching information is sent for the playing track. Gnune uses embedded MusicBrainz identifiers when available. Successful results are cached for offline listening. An ambiguous match can leave an item without art or links.

Quickplay, Collection, Device and Now Playing share a dark palette sampled from the playing artwork. When the artwork changes, GTK fades the colors and soft backdrop together over 1.5 seconds using the display’s frame clock. Collection panes slide with text at its final size. Listening views add a gentle zoom. Opening Now Playing from the transport thumbnail expands the playing sleeve into place. Gnune follows the desktop reduced-motion setting.

Pin albums to Quickplay. Save the queue as a playlist, drag songs onto a playlist in the Songs view, or make a playlist from an artist, genre, title, or album search. **Smart DJ** builds a queue from shared artists and genres. **Mixview** lets you explore related local releases while playback continues. Artist Showcase fills the window with your chosen artist photograph or the current cover, overlays the track typography and places the playing queue along the right edge on wider windows.

## Podcasts, videos and photos

**Collection → Videos** opens in **Artists**. Select an artist or genre to narrow the album and song panes, then select an album to see its videos in disc and track order when those tags are available. Select **All** to widen the selection again. Narrow windows put the artist or genre picker above the results. **Songs** shows individual video posters in title order. A video without an album keeps its own poster and title in the album pane.

Use **+** beside a video to add it to a video playlist. Under **Playlists**, choose a member to play from that point in saved order. Videos continue playing when you browse elsewhere. **Song info** lets you correct the genre locally and expand **File location** to see or copy the source path.

Gnune looks for artwork beside the video (such as `clip-poster.jpg` or `clip.png`), then checks for embedded artwork, then samples the footage for a usable frame. It caches the poster in its data directory.

Choose a video in the grid, open a video file, or pass its path to Gnune. Playback stays in the main window with the shared transport, volume and fullscreen controls. Space pauses or plays, the arrow keys seek, `[` and `]` adjust speed, and `j` cycles available subtitles. Matching subtitle files such as `clip.srt` and `clip.en.srt` load with the video. Switch tabs to keep watching in the live thumbnail beside the progress bar. Click the thumbnail to return to the full video at the same position. The full video view uses the artist portrait in the footer when available, with cover art or a video poster as a fallback. **Stop** ends video playback.

In **Collection → Podcasts**, subscribe with an RSS or Atom feed, refresh the episode list, and download episodes for offline playback. Pictures groups photos by folder or modification date. Thumbnails load in the background as you browse. Select a photo to open the group’s slideshow or view it full screen.

### Identify a video’s soundtrack

Gnune reads embedded title, artist, album, genre, album artist and disc/track tags from new imports and backfills previously indexed videos while retaining saved corrections. Open **Song info** on a video tile or during playback. Choose a method and select **Identify music**.

| Method | What it uses |
|---|---|
| My music collection | Offline Chromaprint comparison with tagged tracks of similar duration. Recent fingerprints are cached locally. |
| AcoustID · whole recording | Sends a fingerprint and the full audio duration. Best for videos containing a near-complete studio recording. If AcoustID supplies only recording IDs, Gnune checks MusicBrainz for credits. You can review alternate titles and albums. [AcoustID lookup](https://acoustid.org/webservice) · [MusicBrainz recording lookup](https://musicbrainz.org/doc/MusicBrainz_API) |
| AudD · audio passage | Sends up to ten seconds of audio from the selected position. During playback the field starts at the playhead. Choose a passage containing the song for clips or videos with dialogue. [AudD recognition API](https://docs.audd.io/) |

Review a match with **Use selected match**, adjust any fields, then choose **Save to collection**. You can also enter title, artist, album and genre manually. Edits live in Gnune’s library. Search the Videos view by filename, title, artist, or album.

AcoustID is ready to use with Gnune’s registered application key, and **My music collection** remains the default method. Under **Recognition service keys**, enter your AudD token or an optional AcoustID override, then use **Remember keys on this computer** to save your entries under `$XDG_CONFIG_HOME/gnune`. Nonblank `GNUNE_ACOUSTID_KEY` and `GNUNE_AUDD_TOKEN` environment variables take precedence over those entries. AcoustID falls back to Gnune’s built-in key when neither override is set. Online identification uses the first audio stream and runs only when you select an online method and submit a lookup.

## Phone collection and playback

Devices shows connected-phone status from Fedora's BlueZ service. Bluetooth is separate from Gnune's opt-in Wi-Fi/HTTPS companion; connect phones in GNOME Settings → Bluetooth.

Choose the access you want in **Devices**:

- **Pair phone with current queue** grants only the selected music queue. An existing queue pairing never gains the rest of your collection.
- **Share full music library over Wi-Fi** separately enables music browsing and your saved music playlists, even without a desktop playback queue. Unavailable files are omitted. Existing full-library pairings see the current music collection when this mode starts again; their cache namespace and listening progress stay stable. Stop and restart sharing after importing or editing the library to refresh its snapshot.
- **Include collection videos** is a separate, initially unchecked option for full-library sharing. A phone must pair with a video-enabled QR to gain that capability. Music-only phones never silently gain videos. Video playlists preserve their desktop order; photos and other assets are excluded.

A library pairing can connect only while full-library sharing is enabled. Video additionally requires both its original grant and the active desktop session to permit video. Choose the scope once: Gnune remembers that consent and resumes sharing each time the desktop app opens, without starting desktop playback. Queue sharing retains the exact selected files; full-library sharing refreshes from the loaded collection on restart. The app waits and retries if its configured Wi-Fi address is temporarily unavailable. **Devices → Stop sharing** cancels waiting and keeps sharing off across restarts until you enable it again. **Retry saved sharing** retries after fixing TLS or connection settings. Existing queue pairings retain only their original selection, even under a full-library listener. Gnune does not sync the iPhone Music library or import files from your phone.

Set a stable HTTPS origin, a certificate chain trusted by the phone, its private key and a LAN bind address before launching Gnune. Substitute values for your network; keep the key on the desktop.

```sh
export GNUNE_SYNC_BIND=192.168.1.20:8443
export GNUNE_SYNC_ORIGIN=https://gnune.local:8443
export GNUNE_SYNC_CERT=/path/to/server-chain.pem
export GNUNE_SYNC_KEY=/path/to/server-key.pem
cargo run --locked
```

The first enabled session saves its connection settings and scope under `$XDG_CONFIG_HOME/gnune/sharing.json`, so normal desktop launches can reconnect without exporting variables again. Environment variables override the saved fields. For initial setup, Gnune also accepts literal `GNUNE_SYNC_*` assignments in `$XDG_CONFIG_HOME/gnune/phone-sharing/env.sh`; it never executes that file or expands shell expressions. PEM private-key contents and phone credentials are not copied into the sharing settings. Certificate parsing and the key match are checked before remembering consent to wait for an unavailable address; certificate trust and hostname acceptance remain the phone’s responsibility. Keep the configured address/origin stable. Gnune does not start at login or keep a listener running after the desktop app exits.

1. Choose a sharing mode in desktop **Devices**. On the same Wi-Fi, scan its QR with your phone camera, open the link in Safari or your browser, then tap **Pair phone**. No copying or typing is needed. QR generation is local. **Copy pairing link** and the phone’s manual disclosure remain available. Each code works once for ten minutes; **New pairing QR** renews it without revoking paired phones.
2. Browse **artists**, **albums**, **songs** and **playlists**, drill into an artist or album, search and use **back**. **videos** and **video playlists** appear for a video-enabled library pairing. A visible player and **up next** retain your playback context. Play and shuffle use the current album, playlist or song list; playlist repeats and ordering are preserved.
3. Tap a song to play. Only the song that becomes current transfers; browsing, adding to up next and selecting a playlist do not fetch audio. There is no next-song prefetch or bulk playlist download. Played songs are saved in this browser when storage allows; the **on phone** filter finds them. Previously downloaded queues still work. **Remove saved audio** clears local audio while keeping pairing and listening progress.
4. Wait for offline-player readiness and test saved songs by reopening in airplane mode. Reconnect to desktop Wi-Fi to return music listening progress; select a resume entry in desktop **Devices**. Codec support and available browser storage depend on the phone. Native video streams only on deliberate playback, supports byte-range seeking and does not download into offline storage. Your phone must support the original codec; H.264/AAC MP4 is widely supported.

Pairing stays in this browser across sessions. Foregrounding the player or reconnecting Wi-Fi retries credential recovery, catalog refresh and progress sync without starting playback. Home Screen installation is optional. After pairing in Safari, use **Share → Add to Home Screen**; Android offers **Install app**. On iOS/iPadOS 17.2 and later, a newly created Home Screen app can inherit Safari’s login cookie and recover the same scoped pairing online. IndexedDB audio and progress are separate between contexts. Existing Home Screen apps without a credential or copied cookie must pair once inside that context. The host-only, HttpOnly, Secure recovery cookie uses only `/api/resume`; the server stores only the credential digest. **Unpair phone** revokes that grant in all contexts and clears saved media in the current one. [WebKit cookie handoff behavior](https://webkit.org/blog/14787/webkit-features-in-safari-17-2/#login-cookies)

To switch an already installed Home Screen app from a queue pairing to your full collection, stop desktop sharing, choose **Share full music library over Wi-Fi** (and the separate video option if wanted), then copy its new pairing link. In the existing phone app, open **connection & storage → Change pairing**, paste the link and tap **Pair phone**. **Keep current pairing** cancels, including after an empty or invalid entry. The old pairing remains active until the new link succeeds; saved audio and pending progress are retained under their original scope. You do not need to unpair, clear storage or create another Home Screen icon.

Open while connected to check for companion updates, then close all Gnune tabs and reopen to activate a waiting update. The offline shell updates together without removing pairings, played songs or pending progress. Artwork comes only from your local collection, through an authenticated, size-limited endpoint, with a bounded phone artwork cache.

If the phone cannot open a QR link, confirm the hostname resolves there, the certificate is trusted and the LAN bind address and port are reachable through Fedora’s firewall. QR scanning does not configure DNS, certificates or firewall rules. Offline-shell setup is separate from online pairing. A revoked pairing is identified separately from an unavailable desktop; saved songs remain usable locally.

For an existing Tailscale setup, `python3 scripts/gnune-tailscale-route.py --host LAN_IP --phone PHONE_TAILSCALE_IP` previews advertising only this computer's LAN address as a `/32` route. Run the helper on the host from the source checkout. Review its printed tailnet policy and route-approval instructions before repeating with `--apply`; `--remove --apply` withdraws that exact route. It preserves other advertised routes and exit-node settings, and does not configure certificates, Gnune, forwarding or firewall rules. Existing broad tailnet grants can still permit access. Keeping the same HTTPS origin preserves an existing phone pairing.

## Local data and verification

The collection, playlists, corrections, downloads, and pairings live under `$XDG_DATA_HOME/gnune`, normally `~/.local/share/gnune`. Media import reads the original files and supports UTF-8 filenames. Online lookup and LAN sharing are separate controls.

```sh
cargo fmt --all --check
cargo check --locked
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

The playback tests run mpv with null audio and require permission to open a private Unix socket. For isolated desktop checks, set `GNUNE_AUDIO_NULL=1` and use a temporary `XDG_DATA_HOME`. Export a separate, mode-0700 `XDG_RUNTIME_DIR` **before** starting a private `dbus-run-session` and compositor so activated services, including AT-SPI, use their own sockets. Physical phone acceptance requires an offline reopen, local playback, and a progress round trip on each device.

Run `node tests/mobile-pairing.cjs` and `node tests/mobile-catalog.cjs` for companion pairing, resume, fetch-on-play, navigation, grouping, playlist ordering and offline-start regressions. These use simulated browser storage and requests; they do not establish Safari or physical-phone acceptance.

## License

Gnune is licensed under the GNU General Public License, version 3 or any later version. [License text](LICENSE)
