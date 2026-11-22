use gnune::library::Track;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
};
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Library {
    pub tracks: Vec<Track>,
    pub pins: Vec<PathBuf>,
    pub history: Vec<PathBuf>,
    pub added: Vec<PathBuf>,
    pub portraits: BTreeMap<String, PathBuf>,
    pub covers: BTreeMap<String, PathBuf>,
    pub playlists: BTreeMap<String, Vec<PathBuf>>,
    pub video_playlists: BTreeMap<String, Vec<PathBuf>>,
    pub rules: BTreeMap<String, String>,
    pub enrichment: bool,
    pub recent_queue: Vec<PathBuf>,
    pub assets: Vec<PathBuf>,
    pub video_tags: BTreeMap<PathBuf, crate::recognition::VideoTags>,
    pub feeds: Vec<String>,
    pub phone_progress: BTreeMap<PathBuf, f64>,
    pub episodes: BTreeMap<String, PathBuf>,
    pub progress: BTreeMap<PathBuf, f64>,
    pub volume: Option<f64>,
    pub current: Option<PathBuf>,
}
impl Library {
    pub fn add_video_to_playlist(&mut self, name: String, path: PathBuf) {
        let paths = self.video_playlists.entry(name).or_default();
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    pub fn merge_video_tags(&mut self, path: PathBuf, mut info: crate::recognition::VideoTags) {
        if let Some(existing) = self.video_tags.get_mut(&path) {
            info.saved = existing.saved.take();
            info.saved_genre = existing.saved_genre.take();
        }
        self.video_tags.insert(path, info);
    }
}
pub fn data_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
        })
        .join("gnune")
}
enum WriteRequest {
    Playback,
    Flush(mpsc::SyncSender<()>),
}

/// Playback updates are sequential and coalesced. Explicit edits and clean exit
/// wait for earlier playback writes before committing their newer state.
pub struct LibraryWriter {
    root: PathBuf,
    pending: Arc<Mutex<Option<Library>>>,
    requests: mpsc::Sender<WriteRequest>,
}
impl LibraryWriter {
    pub fn new(root: PathBuf, report: impl Fn(io::Error) + Send + 'static) -> Self {
        let pending = Arc::new(Mutex::new(None));
        let worker_pending = Arc::clone(&pending);
        let worker_root = root.clone();
        let (requests, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for request in rx {
                match request {
                    WriteRequest::Playback => {
                        let next = worker_pending.lock().unwrap().take();
                        if let Some(db) = next
                            && let Err(error) = save(&worker_root, &db)
                        {
                            report(error);
                        }
                    }
                    WriteRequest::Flush(done) => {
                        let _ = done.send(());
                    }
                }
            }
        });
        Self {
            root,
            pending,
            requests,
        }
    }

    pub fn playback_changed(&self, db: &Library) -> io::Result<()> {
        if self.pending.lock().unwrap().replace(db.clone()).is_none() {
            self.requests
                .send(WriteRequest::Playback)
                .map_err(io::Error::other)?;
        }
        Ok(())
    }

    pub fn save(&self, db: &Library) -> io::Result<()> {
        let (done, flushed) = mpsc::sync_channel(1);
        self.requests
            .send(WriteRequest::Flush(done))
            .map_err(io::Error::other)?;
        flushed.recv().map_err(io::Error::other)?;
        save(&self.root, db)
    }
}
fn save(root: &Path, db: &Library) -> io::Result<()> {
    fs::create_dir_all(root)?;
    let mut f = tempfile::NamedTempFile::new_in(root)?;
    {
        let mut out = io::BufWriter::new(f.as_file_mut());
        serde_json::to_writer(&mut out, db)?;
        out.flush()?;
    }
    f.persist(root.join("collection.json"))?;
    Ok(())
}
pub fn load() -> Result<Library, String> {
    let p = data_dir().join("collection.json");
    match fs::read(&p) {
        Ok(b) => serde_json::from_slice(&b).map_err(|e| format!("Reading {}: {e}", p.display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Library::default()),
        Err(e) => Err(e.to_string()),
    }
}
pub fn album_key(t: &Track) -> String {
    if let Some(id) = &t.release_id {
        return format!("release/{id}");
    }
    if t.album == "Unknown album" {
        return format!(
            "folder/{}",
            t.path.parent().unwrap_or(Path::new("")).display()
        );
    }
    serde_json::to_string(&(&t.album_artist, &t.album)).unwrap()
}
pub fn promote(items: &mut Vec<PathBuf>, path: &Path) {
    items.retain(|x| x != path);
    items.insert(0, path.into());
}
pub fn matches(t: &Track, q: &str) -> bool {
    if q.trim().is_empty() {
        return true;
    }
    let key = format!(
        "{} {} {} {} {}",
        t.artist, t.album_artist, t.album, t.title, t.genre
    )
    .to_lowercase();
    q.split_whitespace()
        .all(|v| key.contains(&v.to_lowercase()))
}
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub at: Option<f64>,
    pub text: String,
}
pub fn lines(text: &str) -> Vec<Line> {
    let mut out = Vec::new();
    let mut plain = Vec::new();
    let mut offset = 0.0;
    for raw in text.lines() {
        let mut s = raw.trim();
        if let Some(v) = s.strip_prefix("[offset:").and_then(|v| v.strip_suffix(']')) {
            offset = v.trim().parse::<f64>().unwrap_or(0.0) / 1000.;
            continue;
        }
        let mut times = Vec::new();
        while let Some((tag, tail)) = s.strip_prefix('[').and_then(|s| s.split_once(']')) {
            if let Some((m, sec)) = tag.split_once(':')
                && let (Ok(m), Ok(sec)) = (m.parse::<u32>(), sec.parse::<f64>())
                && (0.0..60.0).contains(&sec)
            {
                times.push(m as f64 * 60. + sec);
            }
            s = tail;
        }
        if times.is_empty() {
            if !s.is_empty() {
                plain.push(Line {
                    at: None,
                    text: s.to_string(),
                })
            }
        } else {
            for at in times {
                out.push(Line {
                    at: Some(at),
                    text: s.trim().to_string(),
                });
            }
        }
    }
    if out.is_empty() {
        return plain;
    }
    for l in &mut out {
        l.at = l.at.map(|n| (n - offset).max(0.));
    }
    out.sort_by(|a, b| a.at.unwrap().total_cmp(&b.at.unwrap()));
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_save_follows_pending_playback_updates() {
        let dir = tempfile::tempdir().unwrap();
        let writer = LibraryWriter::new(dir.path().into(), |e| panic!("{e}"));
        let mut db = Library::default();
        for i in 1..12 {
            db.current = Some(PathBuf::from(i.to_string()));
            writer.playback_changed(&db).unwrap();
        }
        db.pins.push(PathBuf::from("latest edit"));
        writer.save(&db).unwrap();
        let saved: Library =
            serde_json::from_slice(&fs::read(dir.path().join("collection.json")).unwrap()).unwrap();
        assert_eq!(saved.pins, db.pins);
        assert_eq!(saved.current, db.current);
    }

    #[test]
    fn repeated_lyric_and_offset() {
        let v = lines("[ar:Who]\n[offset:200]\n[00:01.20][00:03.20]hello");
        assert_eq!(v.len(), 2);
        assert_eq!(v[1].at, Some(3.));
        assert_eq!(v[0].text, "hello");
    }
    #[test]
    fn video_playlists_roundtrip_without_changing_music_playlists_or_legacy_tags() {
        let mut db: Library = serde_json::from_str(r#"{"playlists":{"Same name":["song.mp3"]},"video_tags":{"old.mkv":{"embedded":{"title":"Old"}}}}"#).unwrap();
        assert!(db.video_playlists.is_empty());
        assert!(db.video_tags[Path::new("old.mkv")].genre.is_none());
        db.add_video_to_playlist("Same name".into(), "second.mkv".into());
        db.add_video_to_playlist("Same name".into(), "first.mkv".into());
        db.add_video_to_playlist("Same name".into(), "second.mkv".into());
        let restored: Library = serde_json::from_slice(&serde_json::to_vec(&db).unwrap()).unwrap();
        assert_eq!(restored.playlists["Same name"], [PathBuf::from("song.mp3")]);
        assert_eq!(
            restored.video_playlists["Same name"],
            [PathBuf::from("second.mkv"), PathBuf::from("first.mkv")]
        );
    }

    #[test]
    fn video_genre_override_survives_rescan_and_can_clear_a_native_genre() {
        let mut db = Library::default();
        let path = PathBuf::from("clip.mp4");
        db.video_tags.entry(path.clone()).or_default().saved_genre = Some("Reviewed".into());
        db.merge_video_tags(path.clone(), recognition_tags("Rock"));
        assert_eq!(db.video_tags[&path].genre(), "Reviewed");
        db.video_tags.get_mut(&path).unwrap().saved_genre = Some(String::new());
        db.merge_video_tags(path.clone(), recognition_tags("Pop"));
        assert_eq!(db.video_tags[&path].genre(), "");
        fn recognition_tags(genre: &str) -> crate::recognition::VideoTags {
            crate::recognition::VideoTags {
                genre: Some(genre.into()),
                ..Default::default()
            }
        }
    }

    #[test]
    fn plain_lyrics() {
        assert_eq!(lines("one\ntwo")[1].at, None);
    }
}
