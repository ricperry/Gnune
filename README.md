# Gnune

Gnune (pronounced **“noon”**) is a local-first Rust and GTK4 music player modeled on the Microsoft Zune desktop software. The collection arranges artists, albums and songs in coordinated panes on a dark canvas. Now Playing keeps the sleeve sharp against a soft backdrop sampled from its colors. Full screen expands the whole sleeve to fit the display.

With an existing music library, Gnune opens on Quickplay, with sleeves for pins, newly added music and listening history. A first launch opens the empty collection so you can add a folder. [Quickplay in the Zune tutorial](https://www.youtube.com/watch?v=A1iqtznwpCA&t=31s)

The desktop reference shows the collection at 2:00 and the artwork, diagonal typography and queue that inspired Artist Showcase at 2:40. [Collection in the Zune tutorial](https://www.youtube.com/watch?v=A1iqtznwpCA&t=120s) · [Now Playing in the same recording](https://www.youtube.com/watch?v=A1iqtznwpCA&t=160s)

## Install on Linux

Download the x86_64 Flatpak or AppImage from the [GitHub releases](https://github.com/ricperry/Gnune/releases/latest).

To install the Flatpak for your account, run these commands from the download directory. Flatpak uses the GNOME 50 runtime from Flathub.

```sh
flatpak install --user ./Gnune-0.1.0-x86_64.flatpak
flatpak run org.gnune.Player
```

The Flatpak has its own collection and settings. On first launch, use **Add music folder** to select your library, including a folder on an external drive.

The x86_64 AppImage requires glibc 2.39 or newer. To run it directly, make it executable and launch it.

```sh
chmod +x Gnune-0.1.0-x86_64.AppImage
./Gnune-0.1.0-x86_64.AppImage
```

Each release includes `SHA256SUMS`. Download it alongside your package and run `sha256sum --check --ignore-missing SHA256SUMS` to verify the download. Package build instructions are available for [Flatpak](packaging/flatpak/README.md) and [AppImage](packaging/appimage/README.md).

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

## Offline phone playback

The Devices view shares a snapshot of the current queue with an installable web player. Set a stable HTTPS origin, a matching certificate chain trusted by the phones, its private key, and a LAN bind address before launching Gnune. Substitute values for your network.

```sh
export GNUNE_SYNC_BIND=192.168.1.20:8443
export GNUNE_SYNC_ORIGIN=https://gnune.local:8443
export GNUNE_SYNC_CERT=/path/to/server-chain.pem
export GNUNE_SYNC_KEY=/path/to/server-key.pem
cargo run --locked
```

1. On the desktop, play the album or playlist you want to take with you. Open **Devices → Pair phone with current queue**.
2. Open the configured HTTPS origin on the phone. On iPhone, add it to the Home Screen and open the installed app before downloading. Paste the single-use pairing code from the desktop link. Keep the private key on the desktop.
3. Download the queue while connected. Reopen the player in airplane mode to check the downloaded tracks. Codec support and available browser storage depend on the phone.
4. Reconnect to the desktop while sharing is active and open the companion to sync listening progress. Select the resume entry in **Devices**. **Stop sharing** closes the listener. Existing pairings retain access to their original selection when sharing is enabled again. Use **Unpair phone** in the companion to revoke its credential.

## Local data and verification

The collection, playlists, corrections, downloads, and pairings live under `$XDG_DATA_HOME/gnune`, normally `~/.local/share/gnune`. Media import reads the original files and supports UTF-8 filenames. Online lookup and LAN sharing are separate controls.

```sh
cargo fmt --all --check
cargo check --locked
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

The playback tests run mpv with null audio and require permission to open a private Unix socket. For isolated desktop checks, set `GNUNE_AUDIO_NULL=1` and use a temporary `XDG_DATA_HOME`. Export a separate, mode-0700 `XDG_RUNTIME_DIR` **before** starting a private `dbus-run-session` and compositor so activated services, including AT-SPI, use their own sockets. Physical phone acceptance requires an offline reopen, local playback, and a progress round trip on each device.

## License

Gnune is licensed under the GNU General Public License, version 3 or any later version. [License text](LICENSE)
