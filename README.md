# Gnune

Gnune (pronounced **“noon”**) is a local-first Rust and GTK4 music player modeled on the Microsoft Zune desktop software. The collection arranges artists, albums and songs in coordinated panes on a dark canvas. Now Playing keeps the sleeve sharp against a soft backdrop sampled from its colors. Full screen expands the whole sleeve to fit the display.

With an existing music library, Gnune opens on Quickplay, with pinned albums, new music, listening history, a podcast resume tile, and shelves for subscribed shows and videos. A first launch opens the empty collection so you can add a folder. [Quickplay in the Zune tutorial](https://www.youtube.com/watch?v=A1iqtznwpCA&t=31s)

The desktop reference shows the collection at 2:00 and the artwork, diagonal typography and queue that inspired Artist Showcase at 2:40. [Collection in the Zune tutorial](https://www.youtube.com/watch?v=A1iqtznwpCA&t=120s) · [Now Playing in the same recording](https://www.youtube.com/watch?v=A1iqtznwpCA&t=160s)

## Install on Linux

Download the x86_64 Flatpak or AppImage from the [GitHub releases](https://github.com/ricperry/Gnune/releases/latest).

To install the Flatpak for your account, run these commands from the download directory. Flatpak uses the GNOME 50 runtime from Flathub.

```sh
flatpak install --user ./Gnune-0.3.0-x86_64.flatpak
flatpak run org.gnune.Player
```

The Flatpak has its own collection and settings. On first launch, use **Add music folder** to select your library, including a folder on an external drive. For Tailscale phone pairing, run the host setup helper with `--flatpak` as described below.

The x86_64 AppImage requires glibc 2.39 or newer. To run it directly, make it executable and launch it.

```sh
chmod +x Gnune-0.3.0-x86_64.AppImage
./Gnune-0.3.0-x86_64.AppImage
```

Each release includes `SHA256SUMS`. Download it alongside your package and run `sha256sum --check --ignore-missing SHA256SUMS` to verify the download. See the [release notes](RELEASE_NOTES.md) for changes and limits. Package build instructions are available for [Flatpak](packaging/flatpak/README.md) and [AppImage](packaging/appimage/README.md).

## Build from source

Build with stable Rust and GTK 4.14 or newer. On Fedora, install the native build dependencies and run Gnune from the checkout.

```sh
sudo dnf install gcc pkgconf-pkg-config gtk4-devel gstreamer1-devel openssl-devel
cargo run --locked
cargo run --locked -- "/path/to/an album"
```

`gstreamer1-devel` supplies the `gstreamer-1.0.pc` file Cargo needs to build. Video and podcast playback use GStreamer, its codec plugins, and the `gstreamer1-plugin-gtk4` video sink. For music, Gnune starts a headless mpv child. Its private playback connection closes when Gnune exits or crashes, which also ends mpv. FFmpeg and ffprobe read video tags, generate posters and extract audio. Fingerprint matching uses FFmpeg’s Chromaprint muxer. Online lookups and feed downloads use curl.

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
| Window controls | Click the controls in the upper-right corner, or press F10 and use Tab, Shift+Tab and Enter |
| Return from window controls | Escape returns focus to the previous control |
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

Pin albums to Quickplay. Save the queue as a playlist, drag songs onto a playlist in the Songs view, or make a playlist from an artist, genre, title, or album search. **Smart DJ** builds a queue from shared artists and genres. Its Quickplay artist group changes when you enter or return to Quickplay. **More artists** cycles another group when more than four artists are available, without repeating artists until the pool has been visited. The group stays put while the page refreshes or resizes. **Mixview** lets you explore related local releases while playback continues. Artist Showcase fills the window with your chosen artist photograph or the current cover, overlays the track typography and places the playing queue along the right edge on wider windows.

## Podcasts and videos

**Collection → Videos** opens in **Artists**. Select an artist or genre to narrow the album and song panes, then select an album to see its videos in disc and track order when those tags are available. Select **All** to widen the selection again. Narrow windows put the artist or genre picker above the results. **Songs** shows individual video posters in title order. A video without an album keeps its own poster and title in the album pane.

Use **+** beside a video to add it to a video playlist. Under **Playlists**, choose a member to play from that point in saved order. Videos continue playing when you browse elsewhere. **Song info** lets you correct the genre locally and expand **File location** to see or copy the source path.

Gnune looks for artwork beside the video (such as `clip-poster.jpg` or `clip.png`), then checks for embedded artwork, then samples the footage for a usable frame. It caches the poster in its data directory.

Choose a video in the grid, open a video file, or pass its path to Gnune. Playback stays in the main window with the shared transport, volume and fullscreen controls. Space pauses or plays, the arrow keys seek, `[` and `]` adjust speed, and `j` cycles available subtitles. Matching subtitle files such as `clip.srt` and `clip.en.srt` load with the video. Switch tabs to keep watching in the live thumbnail beside the progress bar. Click the thumbnail to return to the full video at the same position. The full video view uses the artist portrait in the footer when available, with cover art or a video poster as a fallback. **Stop** ends video playback.

In **Collection → Podcasts**, choose **Add a podcast**. The browser opens with roughly 25,000 shows recommended by publishers through Podcast Index. Type to filter the list locally, or select **Search full directory** to send the query to p0dcasters and search hundreds of thousands of active shows. Both sources are free to browse without an account. [Podcast Index recommendations](https://public.podcastindex.org/recommendations.json) · [p0dcasters directory](https://p0dcasters.com/about)

Choose **Read feed** to see the publisher’s summary and playable episode count. **Subscribe** saves the show in the Podcasts tab and removes it from discovery. Available covers load only for visible results.

Subscribed shows form a searchable, title-sorted grid with cover art. Click a tile to open its synopsis and episode list. **Full synopsis** expands longer descriptions. Episode rows show up to three lines of summary. **Play latest** or an episode’s **Play** opens Now Playing with the show cover, episode title, and listening order. Audio streams through GStreamer. You can browse elsewhere while it plays and click the footer artwork to return. The footer has a **Rewind 10 seconds** button for music, videos, and podcasts. **Download** remains available for offline listening. Existing files remain under **Downloaded episodes** in the collection.

Right-click a subscribed tile, or focus it and press **Menu** or **Shift+F10**, for **Open podcast**, **Play latest**, **Refresh episodes** and **Unsubscribe**. **Unsubscribe** also appears on the show page. It keeps downloaded files and ongoing playback, and can be reversed with **Subscribe**.

Choose **Oldest first** for a serial show or **Newest first** for current episodes. Gnune remembers the order for each show and uses it to advance at the end of an episode. **Quickplay → latest podcast** resumes your last episode at its saved position or continues with an unfinished episode if you’ve finished it. Episode lists default to newest first. Equal, missing or invalid dates keep feed order, with undated entries after dated episodes. Opening a show refreshes it after 15 minutes. **Play latest** waits for a stale feed to refresh and uses the last known episode if that read fails. A newer play choice supersedes a pending older one. **Refresh episodes** requests a fresh list manually.

Episode metadata and cover art are cached separately under `$XDG_CACHE_HOME/gnune/podcast-feeds/` and `podcast-covers/`. These caches never contain streamed audio. Cached tiles appear immediately, and visible older subscriptions missing cover metadata are backfilled without resubscribing. Feed reads are limited to four at a time, 8 MiB and 30 seconds, retaining up to 2,000 episodes after sorting. Cover retrieval is limited to two at a time, 4 MiB, 15 seconds and 4096 pixels. Missing art uses the audio placeholder.

**Scan feed list** accepts an OPML URL or local path. **Open OPML file** opens a file picker. You can also select the BBC catalog or the bundled starter picks, preview an individual RSS or Atom URL, or subscribe directly from the Podcasts tab.

Gnune caches the Podcast Index list for 24 hours under `$XDG_CACHE_HOME/gnune/` (normally `~/.cache/gnune/`). If a refresh fails, the browser uses the last valid list or the bundled picks on first use. Custom OPML lists support up to 2 MiB and 2,000 distinct feed URLs.

Collection has three categories, Music, Videos and Podcasts. Music and video playlists live within their respective categories. Folder imports pick up playable media and use nearby images for artwork.

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

Open **Devices** with the phone icon at the bottom left of the player. **Pair phone** shares your whole music library, saved playlists, collection videos and subscribed podcasts over **Tailscale**. Keep Tailscale connected on this PC and your phone. Playback happens on your phone. The Bluetooth phones list is under its own disclosure.

**Pair phone** refreshes the collection and creates a single-use QR. Choose **Refresh QR** for a replacement, or **Pair another phone** after a successful scan. Existing phones stay connected. Existing pairings keep their original capabilities: pair an older queue-only or music-only phone again to grant the full collection. Unavailable files are omitted; collection photos and other assets are excluded.

Sharing resumes when Gnune reopens, without starting playback. **Stop sharing** cancels waiting and keeps it off across restarts until you choose **Pair phone** again. Full-library music refreshes from the loaded collection on restart. The phone has its own playback controls; it does not remote-control desktop playback. Phone **Quickplay** shares album pins, new music, history and the latest podcast. Episode order, completion and listening position sync in both directions. Gnune does not import the iPhone Music library or files from your phone.

Use the host helper below once to set up the private Tailscale HTTPS address. Devices checks that address against the running Gnune instance before showing the QR. If the connection drops, leave Devices open to see it recover. The desktop app must be running to stream your collection. Previously paired phones retain their existing access.

1. Choose **Pair phone** in desktop **Devices**. With Tailscale connected on both devices, scan the QR with your phone camera, open its link in Safari or your browser, then tap **Pair phone**. QR generation is local. **Pairing link → Copy pairing link** remains available as a fallback. Each code works once for ten minutes. Opening Devices replaces a used or expired code; an unused valid code stays stable. Too many unsuccessful attempts require explicitly choosing **Pair phone** again.
2. Browse **artists**, **albums**, **songs** and **playlists**, drill into an artist or album, search and use **back**. **videos**, **video playlists** and **podcasts** are included in the full collection pairing. A visible player and **up next** retain your playback context. Play and shuffle use the current album, playlist or song list; playlist repeats and ordering are preserved.
3. Tap a song to play. Only the song that becomes current transfers; browsing, adding to up next and selecting a playlist do not fetch audio. There is no next-song prefetch or bulk playlist download. Played songs are saved in this browser when storage allows; the **on phone** filter finds them. Previously downloaded queues still work. **Remove saved audio** clears local audio while keeping pairing and listening progress.
4. Wait for offline-player readiness and test saved songs by reopening in airplane mode. Reconnect Tailscale to return listening progress. Codec support and available browser storage depend on the phone. Native video streams only on deliberate playback, supports byte-range seeking and does not download into offline storage. Your phone must support the original codec; H.264/AAC MP4 is widely supported.

Pairing stays in this browser across sessions. Foregrounding the player or reconnecting retries credential recovery, catalog refresh and progress sync without starting playback. Home Screen installation is optional. After pairing in Safari, use **Share → Add to Home Screen**; Android offers **Install app**. On iOS/iPadOS 17.2 and later, a newly created Home Screen app can inherit Safari’s login cookie and recover the same scoped pairing online. IndexedDB audio and progress are separate between contexts. Existing Home Screen apps without a credential or copied cookie must pair once inside that context. The host-only, HttpOnly, Secure recovery cookie uses only `/api/resume`; the server stores only the credential digest. **Unpair phone** revokes that grant in all contexts and clears saved media in the current one. [WebKit cookie handoff behavior](https://webkit.org/blog/14787/webkit-features-in-safari-17-2/#login-cookies)

To pair an existing Home Screen app again, choose **Pair phone** in desktop **Devices** and copy its fresh Tailscale pairing link. In the phone app, open **connection & storage → Change pairing**, paste the link and tap **Pair phone**. **Keep current pairing** cancels, including after an empty or invalid entry. The old pairing remains active until the new link succeeds; saved audio and pending progress retain their original scope. You do not need to unpair, clear storage or create another Home Screen icon.

If your iPhone Home Screen icon shows a plain G, open Gnune in Safari while connected to the desktop. Close all Gnune tabs and the Home Screen app so the update can activate, then reopen Gnune in Safari and check the preview under **Share → Add to Home Screen**. Keep the existing app until you've verified the new icon and pairing. Saved songs and pending progress belong to the installation that stored them.

The 180×180 Home Screen icon comes from the desktop artwork in `assets/gnuneIconDraft2.png`. After changing that artwork, regenerate the icon with `python3 scripts/generate-mobile-touch-icon.py` (requires Pillow).

Open while connected to check for companion updates, then close all Gnune tabs and reopen to activate a waiting update. The offline shell updates together without removing pairings, played songs or pending progress. Artwork comes only from your local collection, through an authenticated, size-limited endpoint, with a bounded phone artwork cache.

If the phone cannot open a QR link, confirm Tailscale is connected on both devices, the desktop MagicDNS hostname resolves on the phone, and the tailnet policy permits access to its HTTPS service. QR scanning does not configure Tailscale or its access policy. Offline-shell setup is separate from online pairing. A revoked pairing is identified separately from an unavailable desktop; saved songs remain usable locally.

For stable access at home and away, the host helper below uses your existing **Tailscale MagicDNS hostname** and **Tailscale Serve** managed HTTPS. It avoids DHCP addresses and certificate-file renewal. First enable HTTPS certificates for the existing tailnet in Tailscale's DNS settings. Keep Tailscale connected on the desktop and iPhone. Run the helper on the host as your regular user; **close all Gnune instances before apply or undo**. Open the updated Gnune build when setup finishes. On a new installation, sharing starts when you choose **Pair phone**.

```sh
python3 scripts/gnune-tailscale-setup.py
python3 scripts/gnune-tailscale-setup.py --privilege pkexec --apply
# Review or restore the previous endpoint:
python3 scripts/gnune-tailscale-setup.py --undo
python3 scripts/gnune-tailscale-setup.py --privilege pkexec --undo --apply
```

Add `--flatpak` to each command for the Flatpak installation, or use `--settings /absolute/path/to/sharing.json` to select a configuration. Run the helper on the host, including for Flatpak. Flatpak uses its own settings and private socket under `~/.var/app/org.gnune.Player/config/gnune`; host Cargo/AppImage settings and phone pairings are not imported automatically. Preview is read-only. Apply elevates only the exact Serve add/remove command when necessary (`--privilege sudo` is also supported). It refuses a conflicting mount on the chosen port, preserves unrelated Serve configuration, probes trusted HTTPS before atomically updating the endpoint, and retains original consent, queue and TLS paths for undo. A private mode-0700 directory and mode-0600 Unix socket connect Serve to Gnune; no HTTP TCP backend, subnet route, Funnel, firewall rule or certificate file is created. Other local users cannot substitute a TCP backend while Gnune is closed. The service is accessible only within your tailnet; existing tailnet access rules still apply. [Tailscale Serve](https://tailscale.com/docs/features/tailscale-serve)

Changing from an IP URL to a hostname changes the browser origin: pair once at the new QR and install its Home Screen app if desired. The old icon's offline audio remains at its old origin; return pending progress there first when it is reachable. No browser storage or phone grant is moved or deleted. Gnune uses the saved Tailscale setup even if an old launcher exports a LAN address. During undo, the helper checks the existing endpoint and removes only a mount it created. On a first-time setup, undo also checks that the initial sharing settings are still intact.

## Local data and verification

The collection, playlists, corrections, downloads, and pairings live under `$XDG_DATA_HOME/gnune`, normally `~/.local/share/gnune`. Media import reads the original files and supports UTF-8 filenames. Online lookup and phone sharing are separate controls.

```sh
cargo fmt --all --check
cargo check --locked
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

The playback tests run mpv with null audio and require permission to open a private Unix socket. For isolated desktop checks, set `GNUNE_AUDIO_NULL=1` and use a temporary `XDG_DATA_HOME`. Export a separate, mode-0700 `XDG_RUNTIME_DIR` **before** starting a private `dbus-run-session` and compositor so activated services, including AT-SPI, use their own sockets. Physical phone acceptance requires an offline reopen, local playback, and a progress round trip on each device.

Run `python3 tests/tailscale-setup.py` for migration preview/apply/undo, race and capability safeguards with mocked Tailscale commands and temporary Unix sockets. These tests do not change live Tailscale configuration.

Run `node tests/mobile-pairing.cjs` and `node tests/mobile-catalog.cjs` for companion pairing, resume, fetch-on-play, navigation, grouping, playlist ordering and offline-start regressions. These use simulated browser storage and requests; they do not establish Safari or physical-phone acceptance.

## License

Gnune is licensed under the GNU General Public License, version 3 or any later version. [License text](LICENSE)
