//! Persistent mpv process with observed JSON IPC state and a real playlist.

use crate::library::Track;
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, BufWriter, Write},
    net::Shutdown,
    os::{fd::OwnedFd, unix::net::UnixStream},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct PlaybackState {
    pub track_id: Option<String>,
    pub queue_index: Option<usize>,
    pub paused: bool,
    pub position: f64,
    /// Time the backend last reported this position, for display interpolation only.
    pub position_updated: Instant,
    pub duration: f64,
    pub volume: f64,
    pub error: Option<String>,
    pub finished: bool,
    pub loaded: bool,
}

impl Default for PlaybackState {
    fn default() -> Self {
        Self {
            track_id: None,
            queue_index: None,
            paused: true,
            position: 0.0,
            position_updated: Instant::now(),
            duration: 0.0,
            volume: 70.0,
            error: None,
            finished: false,
            loaded: false,
        }
    }
}

#[derive(Default)]
struct Shared {
    state: PlaybackState,
    queue: Vec<Track>,
}

pub struct Player {
    process: Child,
    stream: Mutex<BufWriter<UnixStream>>,
    shared: Arc<Mutex<Shared>>,
    reader: Option<JoinHandle<()>>,
}

impl Player {
    pub fn new() -> Result<Self, String> {
        Self::with_null_audio(std::env::var_os("GNUNE_AUDIO_NULL").is_some_and(|s| s != "0"))
    }

    /// Null output still decodes audio in real time, useful for isolated checks.
    pub fn with_null_audio(null_audio: bool) -> Result<Self, String> {
        let mut command = Command::new("mpv");
        command.args([
            "--no-config",
            "--idle=yes",
            "--no-terminal",
            "--video=no",
            "--audio-display=no",
            "--gapless-audio=yes",
            "--prefetch-playlist=yes",
            "--volume=70",
            "--pause=yes",
        ]);
        if null_audio {
            command.arg("--ao=null");
        }
        Self::spawn(command)
    }

    fn spawn(mut command: Command) -> Result<Self, String> {
        // mpv owns one connected endpoint as stdin. Its IPC client exits on EOF,
        // including when Gnune crashes and the kernel closes all of our descriptors.
        let (mut stream, peer) = UnixStream::pair().map_err(|e| e.to_string())?;
        let mut reader_stream = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
        command
            .arg("--input-ipc-client=fd://0")
            .stdin(Stdio::from(OwnedFd::from(peer)))
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut process = command
            .spawn()
            .map_err(|e| format!("Unable to start mpv: {e}. Install the mpv package."))?;
        // Command retains its configured stdin descriptor after spawn.
        drop(command);
        if let Err(error) = initialize_ipc(&mut stream, &mut reader_stream) {
            let _ = stream.shutdown(Shutdown::Both);
            let exited = process.try_wait().ok().flatten();
            if exited.is_none() {
                let _ = process.kill();
            }
            let status = process.wait().ok();
            return Err(match exited {
                Some(status) => format!("mpv exited during startup: {status}: {error}"),
                None => format!("Unable to initialize mpv IPC: {error} (status {status:?})"),
            });
        }
        let shared = Arc::new(Mutex::new(Shared::default()));
        let reader_shared = shared.clone();
        let reader = thread::Builder::new()
            .name("gnune-mpv-ipc".into())
            .spawn(move || {
                for line in reader_stream.lines() {
                    let Ok(line) = line else { break };
                    if let Ok(value) = serde_json::from_str::<Value>(&line) {
                        let mut shared = reader_shared.lock().unwrap_or_else(|e| e.into_inner());
                        apply_event(&mut shared, &value);
                    }
                }
            });
        let reader = match reader {
            Ok(reader) => reader,
            Err(error) => {
                let _ = stream.shutdown(Shutdown::Both);
                let _ = process.kill();
                let _ = process.wait();
                return Err(format!("Unable to start mpv IPC reader: {error}"));
            }
        };
        let player = Self {
            process,
            stream: Mutex::new(BufWriter::new(stream)),
            shared,
            reader: Some(reader),
        };
        for (id, property) in [
            "time-pos",
            "duration",
            "pause",
            "volume",
            "playlist-pos",
            "idle-active",
        ]
        .iter()
        .enumerate()
        {
            player.command(json!(["observe_property", id + 1, property]))?;
        }
        Ok(player)
    }

    pub fn shutdown(&self) {
        let _ = self.command(json!(["quit"]));
    }

    pub fn snapshot(&self) -> PlaybackState {
        let mut state = self
            .shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .state
            .clone();
        state.paused |= state.finished;
        state
    }

    /// Replace the native playlist once. mpv prefetches following tracks and
    /// advances itself without process restarts or UI timer intervention.
    pub fn set_queue(&self, tracks: &[Track], start: usize) -> Result<(), String> {
        if tracks.is_empty() {
            self.command(json!(["stop"]))?;
            let mut shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            shared.queue.clear();
            shared.state = PlaybackState {
                volume: shared.state.volume,
                ..PlaybackState::default()
            };
            return Ok(());
        }
        if start >= tracks.len() {
            return Err("Queue position is out of range".into());
        }
        let paths = tracks
            .iter()
            .map(|track| {
                track
                    .path
                    .to_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "mpv IPC requires UTF-8 filenames".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?;
        {
            let mut shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            shared.queue = tracks.to_vec();
            shared.state.loaded = false;
            shared.state.error = None;
            shared.state.finished = false;
            shared.state.position = 0.0;
            shared.state.position_updated = Instant::now();
            shared.state.duration = tracks[start].duration;
            shared.state.queue_index = Some(start);
            shared.state.track_id = Some(tracks[start].id.clone());
        }
        let mut commands = vec![
            json!(["set_property", "pause", true]),
            json!(["loadfile", paths[0], "replace"]),
        ];
        commands.extend(
            paths
                .iter()
                .skip(1)
                .map(|path| json!(["loadfile", path, "append"])),
        );
        commands.push(json!(["set_property", "playlist-pos", start]));
        commands.push(json!(["set_property", "pause", false]));
        self.commands(&commands)
    }

    /// Change the native playlist in place so editing future tracks leaves the decoder running.
    pub fn append(&self, t: &Track) -> Result<(), String> {
        let mut shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        let path = t.path.to_str().ok_or("mpv IPC requires UTF-8 filenames")?;
        self.command(json!(["loadfile", path, "append"]))?;
        shared.queue.push(t.clone());
        Ok(())
    }
    pub fn edit(&self, from: usize, to: Option<usize>) -> Result<(), String> {
        let mut shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        if from >= shared.queue.len() || to.is_some_and(|i| i >= shared.queue.len()) {
            return Err("Queue position is out of range".into());
        }
        if let Some(to) = to {
            let before = to + usize::from(from < to);
            self.command(json!(["playlist-move", from, before]))?;
            let entry = shared.queue.remove(from);
            shared.queue.insert(to, entry);
        } else {
            self.command(json!(["playlist-remove", from]))?;
            shared.queue.remove(from);
        }
        Ok(())
    }
    pub fn play_at(&self, index: usize) -> Result<(), String> {
        if index
            >= self
                .shared
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .queue
                .len()
        {
            return Err("Queue position is out of range".into());
        }
        self.commands(&[
            json!(["playlist-play-index", index]),
            json!(["set_property", "pause", false]),
        ])
    }
    pub fn play_pause(&self) -> Result<(), String> {
        self.command(json!(["cycle", "pause"]))
    }
    pub fn set_paused(&self, paused: bool) -> Result<(), String> {
        self.command(json!(["set_property", "pause", paused]))
    }
    pub fn next(&self) -> Result<(), String> {
        self.command(json!(["playlist-next", "weak"]))
    }
    pub fn previous(&self) -> Result<(), String> {
        self.command(json!(["playlist-prev", "weak"]))
    }
    pub fn seek(&self, seconds: f64) -> Result<(), String> {
        if !seconds.is_finite() {
            return Err("Seek time must be finite".into());
        }
        self.command(json!(["seek", seconds.max(0.0), "absolute+exact"]))
    }
    pub fn set_volume(&self, volume: f64) -> Result<(), String> {
        if !volume.is_finite() {
            return Err("Volume must be finite".into());
        }
        self.command(json!(["set_property", "volume", volume.clamp(0.0, 100.0)]))
    }

    fn command(&self, command: Value) -> Result<(), String> {
        self.commands(&[command])
    }
    fn commands(&self, commands: &[Value]) -> Result<(), String> {
        let mut stream = self.stream.lock().unwrap_or_else(|e| e.into_inner());
        for command in commands {
            serde_json::to_writer(&mut *stream, &json!({"command": command}))
                .map_err(|e| e.to_string())?;
            stream.write_all(b"\n").map_err(|e| e.to_string())?;
        }
        stream.flush().map_err(|e| e.to_string())
    }
}

// Read a real reply before exposing a Player; spawning alone does not mean mpv
// accepted our flags or initialized its IPC client. Keep this reader, including
// any read-ahead, for the observer thread after the bounded handshake completes.
fn initialize_ipc(
    stream: &mut UnixStream,
    reader: &mut BufReader<UnixStream>,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(4);
    stream
        .set_write_timeout(Some(Duration::from_secs(4)))
        .map_err(|e| e.to_string())?;
    stream
        .write_all(b"{\"command\":[\"get_property\",\"mpv-version\"],\"request_id\":0}\n")
        .map_err(|e| e.to_string())?;
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or("mpv IPC startup timed out")?;
        reader
            .get_ref()
            .set_read_timeout(Some(remaining))
            .map_err(|e| e.to_string())?;
        let mut line = String::new();
        if reader.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            return Err("mpv closed its IPC connection during startup".into());
        }
        let response: Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
        if response["request_id"] == 0 {
            if response["error"] != "success" {
                return Err(format!(
                    "mpv rejected startup request: {}",
                    response["error"]
                ));
            }
            stream.set_write_timeout(None).map_err(|e| e.to_string())?;
            reader
                .get_ref()
                .set_read_timeout(None)
                .map_err(|e| e.to_string())?;
            return Ok(());
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // IPC EOF is itself mpv's shutdown signal; do not risk a blocking write
        // to an unresponsive child during Drop. Closing just the writer
        // is insufficient because the reader holds another descriptor to our end.
        let _ = self
            .stream
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_ref()
            .shutdown(Shutdown::Both);
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            match self.process.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                _ => {
                    let _ = self.process.kill();
                    break;
                }
            }
        }
        let _ = self.process.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

fn apply_event(shared: &mut Shared, value: &Value) {
    if let Some(error) = value
        .get("error")
        .and_then(Value::as_str)
        .filter(|s| *s != "success")
    {
        shared.state.error = Some(format!("mpv: {error}"));
    }
    match value.get("event").and_then(Value::as_str) {
        Some("property-change") => {
            let data = &value["data"];
            match value["name"].as_str() {
                Some("time-pos") => {
                    if let Some(n) = data.as_f64() {
                        shared.state.position = n;
                        shared.state.position_updated = Instant::now();
                    }
                }
                Some("duration") => {
                    if let Some(n) = data.as_f64() {
                        shared.state.duration = n;
                    }
                }
                Some("pause") => {
                    if let Some(n) = data.as_bool() {
                        shared.state.paused = n;
                    }
                }
                Some("volume") => {
                    if let Some(n) = data.as_f64() {
                        shared.state.volume = n;
                    }
                }
                Some("playlist-pos") => {
                    if let Some(index) = data
                        .as_u64()
                        .map(|n| n as usize)
                        .filter(|i| *i < shared.queue.len())
                    {
                        shared.state.queue_index = Some(index);
                        if shared.state.track_id.as_ref() != Some(&shared.queue[index].id) {
                            shared.state.position = 0.;
                            shared.state.position_updated = Instant::now();
                        }
                        shared.state.track_id = Some(shared.queue[index].id.clone());
                        shared.state.duration = shared.queue[index].duration;
                        shared.state.finished = false;
                    }
                }
                Some("idle-active") if data.as_bool() == Some(true) && !shared.queue.is_empty() => {
                    // mpv may report idle between files without changing its pause
                    // property. Keep the observed pause setting for the next file.
                    shared.state.finished = true;
                }
                _ => {}
            }
        }
        Some("start-file") => {
            shared.state.loaded = false;
        }
        Some("file-loaded") => {
            shared.state.loaded = true;
            shared.state.finished = false;
        }
        Some("end-file") if value["reason"].as_str() == Some("error") => {
            shared.state.error = Some(format!(
                "Could not play this track: {}",
                value["file_error"].as_str().unwrap_or("decoding failed")
            ));
        }
        Some("shutdown") => {
            shared.state.paused = true;
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::scan_paths;

    fn write_wav(path: &std::path::Path, seconds: u32) {
        let bytes = seconds * 8000 * 2;
        let mut file = std::fs::File::create(path).unwrap();
        file.write_all(b"RIFF").unwrap();
        file.write_all(&(36 + bytes).to_le_bytes()).unwrap();
        file.write_all(b"WAVEfmt ").unwrap();
        file.write_all(&16u32.to_le_bytes()).unwrap();
        file.write_all(&1u16.to_le_bytes()).unwrap();
        file.write_all(&1u16.to_le_bytes()).unwrap();
        file.write_all(&8000u32.to_le_bytes()).unwrap();
        file.write_all(&16000u32.to_le_bytes()).unwrap();
        file.write_all(&2u16.to_le_bytes()).unwrap();
        file.write_all(&16u16.to_le_bytes()).unwrap();
        file.write_all(b"data").unwrap();
        file.write_all(&bytes.to_le_bytes()).unwrap();
        file.write_all(&vec![0; bytes as usize]).unwrap();
    }

    #[track_caller]
    fn wait_for(player: &Player, predicate: impl Fn(&PlaybackState) -> bool) -> PlaybackState {
        let deadline = Instant::now() + Duration::from_secs(6);
        loop {
            let state = player.snapshot();
            if predicate(&state) {
                return state;
            }
            assert!(
                Instant::now() < deadline,
                "Timed out waiting for mpv: {state:?}"
            );
            thread::sleep(Duration::from_millis(25));
        }
    }

    #[test]
    fn scan_reads_real_tags_duration_and_embedded_front_cover() {
        use lofty::{
            config::WriteOptions,
            picture::{MimeType, Picture, PictureType},
            prelude::*,
            tag::{Tag, TagType},
        };
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tagged.wav");
        write_wav(&path, 1);
        let mut tag = Tag::new(TagType::Id3v2);
        tag.set_title("A real title".into());
        tag.set_artist("Artist".into());
        tag.set_album("Album".into());
        tag.set_genre("Electronic".into());
        tag.insert_text(
            ItemKey::MusicBrainzArtistId,
            "00000000-0000-4000-8000-000000000001".into(),
        );
        tag.set_track(7);
        tag.set_disk(2);
        tag.insert_text(ItemKey::AlbumArtist, "Album artist".into());
        tag.insert_text(ItemKey::RecordingDate, "2009-06-12".into());
        let image = b"embedded front image".to_vec();
        tag.push_picture(
            Picture::unchecked(image.clone())
                .pic_type(PictureType::CoverFront)
                .mime_type(MimeType::Png)
                .build(),
        );
        tag.save_to_path(&path, WriteOptions::default()).unwrap();
        let scan = scan_paths(&[path], &directory.path().join("covers"));
        assert!(scan.warnings.is_empty(), "{:?}", scan.warnings);
        let track = &scan.tracks[0];
        assert_eq!(
            track.artist_id.as_deref(),
            Some("00000000-0000-4000-8000-000000000001")
        );
        assert_eq!(
            (
                track.title.as_str(),
                track.artist.as_str(),
                track.album.as_str(),
                track.album_artist.as_str(),
                track.genre.as_str()
            ),
            (
                "A real title",
                "Artist",
                "Album",
                "Album artist",
                "Electronic"
            )
        );
        assert_eq!(
            (track.year, track.track_number, track.disc_number),
            (Some(2009), Some(7), Some(2))
        );
        assert!((track.duration - 1.0).abs() < 0.01);
        assert_eq!(std::fs::read(track.cover.as_ref().unwrap()).unwrap(), image);
    }

    #[test]
    fn idle_before_file_loaded_preserves_observed_play_state() {
        let mut shared = Shared::default();
        shared.queue.push(Track::default());
        apply_event(
            &mut shared,
            &json!({"event":"property-change", "name":"pause", "data":false}),
        );
        apply_event(
            &mut shared,
            &json!({"event":"property-change", "name":"idle-active", "data":true}),
        );
        apply_event(&mut shared, &json!({"event":"file-loaded"}));
        assert!(shared.state.loaded && !shared.state.finished && !shared.state.paused);
    }

    #[test]
    fn mpv_exits_when_ipc_disconnects_before_player_drop() {
        let mut player =
            Player::with_null_audio(true).expect("mpv must start for lifecycle verification");
        player
            .stream
            .lock()
            .unwrap()
            .get_ref()
            .shutdown(Shutdown::Both)
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(status) = player.process.try_wait().unwrap() {
                assert!(
                    status.success(),
                    "mpv exited abnormally after IPC EOF: {status}"
                );
                break;
            }
            assert!(
                Instant::now() < deadline,
                "mpv survived IPC disconnect while Player was still alive"
            );
            thread::sleep(Duration::from_millis(10));
        }
        // Drop has not run or sent quit; IPC EOF alone terminated the process.
        assert!(player.reader.take().unwrap().join().is_ok());
        drop(player);
    }

    #[test]
    fn mpv_startup_failure_is_reported_before_exposing_a_player() {
        let result = Player::spawn(Command::new("/usr/bin/false"));
        assert!(result.is_err());
        let error = result.err().unwrap();
        assert!(
            error.contains("startup") || error.contains("initialize"),
            "{error}"
        );
    }

    #[test]
    fn mpv_decodes_native_playlist_and_observes_controls() {
        let directory = tempfile::tempdir().unwrap();
        write_wav(&directory.path().join("one.wav"), 3);
        write_wav(&directory.path().join("two.wav"), 2);
        let tracks = scan_paths(
            &[directory.path().to_owned()],
            &directory.path().join("covers"),
        )
        .tracks;
        let player =
            Player::with_null_audio(true).expect("mpv must be installed to verify playback");
        player.set_queue(&tracks, 0).unwrap();
        wait_for(&player, |s| s.position > 0.1 && !s.paused);
        player.set_paused(true).unwrap();
        wait_for(&player, |s| s.paused);
        player.append(&tracks[0]).unwrap();
        player.edit(2, Some(1)).unwrap();
        player.edit(1, None).unwrap();
        assert_eq!(player.snapshot().track_id, Some(tracks[0].id.clone()));
        player.set_volume(34.0).unwrap();
        wait_for(&player, |s| (s.volume - 34.0).abs() < 0.1);
        player.seek(2.5).unwrap();
        wait_for(&player, |s| s.position > 2.0);
        player.set_paused(false).unwrap();
        wait_for(&player, |s| s.track_id.as_deref() == Some(&tracks[1].id));
        player.previous().unwrap();
        wait_for(&player, |s| s.queue_index == Some(0));
        player.next().unwrap();
        wait_for(&player, |s| s.queue_index == Some(1));
        let state = wait_for(&player, |s| s.finished);
        assert!(state.error.is_none(), "{state:?}");
        let mut broken = tracks[0].clone();
        broken.path = directory.path().join("broken.wav");
        broken.id = "broken".into();
        std::fs::write(&broken.path, b"not audio").unwrap();
        player.set_queue(&[broken], 0).unwrap();
        wait_for(&player, |s| s.error.is_some());
        player.set_queue(&tracks, 1).unwrap();
        let recovered = wait_for(&player, |s| {
            s.track_id.as_deref() == Some(&tracks[1].id) && s.position > 0.1 && !s.paused
        });
        assert!(recovered.error.is_none(), "{recovered:?}");
        player.set_paused(true).unwrap();
        wait_for(&player, |s| s.paused);
        assert!(player.play_at(tracks.len()).is_err());
        player.play_at(0).unwrap();
        wait_for(&player, |s| {
            s.track_id.as_deref() == Some(&tracks[0].id) && !s.paused
        });
    }
}
