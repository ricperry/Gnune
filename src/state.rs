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
    pub podcast_shows: BTreeMap<String, crate::media::PodcastShow>,
    pub podcast_orders: BTreeMap<String, crate::podcasts::EpisodeOrder>,
    pub podcast_progress: BTreeMap<String, BTreeMap<String, crate::podcasts::EpisodeProgress>>,
    pub last_podcast: Option<crate::podcasts::EpisodeId>,
    pub podcast_downloads: BTreeMap<PathBuf, crate::podcasts::EpisodeId>,
    pub phone_progress: BTreeMap<PathBuf, f64>,
    pub episodes: BTreeMap<String, PathBuf>,
    pub progress: BTreeMap<PathBuf, f64>,
    pub volume: Option<f64>,
    pub current: Option<PathBuf>,
}
impl Library {
    pub fn record_podcast_progress(
        &mut self,
        id: &crate::podcasts::EpisodeId,
        position: f64,
        completed: bool,
    ) {
        if !position.is_finite() {
            return;
        }
        self.podcast_progress
            .entry(id.feed_url.clone())
            .or_default()
            .insert(
                id.episode_url.clone(),
                crate::podcasts::EpisodeProgress {
                    position_secs: position.max(0.),
                    completed,
                },
            );
        self.last_podcast = Some(id.clone());
    }

    /// Remove membership only; saved episodes, progress and cached show metadata remain usable.
    pub fn unsubscribe_podcast(&mut self, feed: &str) -> bool {
        let Ok(feed) = crate::podcasts::normalize_url(feed) else {
            return false;
        };
        let before = self.feeds.len();
        self.feeds.retain(|candidate| {
            !crate::podcasts::normalize_url(candidate).is_ok_and(|candidate| candidate == feed)
        });
        self.feeds.len() != before
    }

    pub fn normalize_album_groups(&mut self) {
        if self.covers.is_empty() {
            gnune::library::normalize_album_artists(&mut self.tracks);
            return;
        }
        let previous: Vec<_> = self.tracks.iter().map(album_key).collect();
        gnune::library::normalize_album_artists(&mut self.tracks);
        let mut ordered: Vec<_> = self.tracks.iter().zip(previous).collect();
        ordered.sort_by(|(a, _), (b, _)| a.path.cmp(&b.path));
        for (track, old_key) in ordered {
            let key = album_key(track);
            let legacy_key = serde_json::to_string(&(&track.artist, &track.album)).unwrap();
            if let Some(cover) = self
                .covers
                .get(&old_key)
                .or_else(|| {
                    (track.album_artist_missing == Some(true))
                        .then(|| self.covers.get(&legacy_key))
                        .flatten()
                })
                .cloned()
            {
                // Preserve the old entry too: another release may still use it.
                self.covers.entry(key).or_insert(cover);
            }
        }
    }
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
        Ok(b) => {
            let mut db: Library =
                serde_json::from_slice(&b).map_err(|e| format!("Reading {}: {e}", p.display()))?;
            db.normalize_album_groups();
            Ok(db)
        }
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
    if t.album_folder {
        return format!(
            "compilation/{}",
            serde_json::to_string(&(gnune::library::album_directory(&t.path), &t.album)).unwrap()
        );
    }
    serde_json::to_string(&(&t.album_artist, &t.album)).unwrap()
}
pub fn sort_album_tracks(tracks: &mut [Track]) {
    tracks.sort_by_cached_key(|track| {
        (
            track.disc_number.unwrap_or(1),
            track.track_number.unwrap_or(0),
            track.title.to_lowercase(),
            track.path.clone(),
        )
    });
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
    fn podcast_listening_state_roundtrips_and_legacy_libraries_default_to_newest() {
        let mut library: Library =
            serde_json::from_str(r#"{"feeds":["https://example.test/feed"]}"#).unwrap();
        assert!(library.last_podcast.is_none());
        assert!(library.podcast_progress.is_empty());
        let id = crate::podcasts::EpisodeId::new(
            "https://example.test:443/feed#alias",
            "https://example.test/audio.mp3",
        )
        .unwrap();
        library.podcast_orders.insert(
            id.feed_url.clone(),
            crate::podcasts::EpisodeOrder::OldestFirst,
        );
        library.record_podcast_progress(&id, 348., false);
        library
            .podcast_downloads
            .insert(PathBuf::from("/tmp/saved.mp3"), id.clone());
        let saved = serde_json::to_string(&library).unwrap();
        assert!(saved.contains("oldest_first"));
        let restored: Library = serde_json::from_str(&saved).unwrap();
        assert_eq!(restored.last_podcast, Some(id.clone()));
        assert_eq!(
            restored.podcast_progress[&id.feed_url][&id.episode_url].position_secs,
            348.
        );
        assert_eq!(
            restored.podcast_downloads[&PathBuf::from("/tmp/saved.mp3")],
            id
        );
        assert_eq!(
            crate::podcasts::EpisodeOrder::default(),
            crate::podcasts::EpisodeOrder::NewestFirst
        );
    }

    #[test]
    fn unsubscribing_removes_canonical_aliases_and_preserves_downloads_and_show_cache() {
        let dir = tempfile::tempdir().unwrap();
        let downloaded = dir.path().join("saved.wav");
        fs::write(&downloaded, b"offline audio").unwrap();
        let feed = "https://example.test/feed";
        let mut library = Library {
            feeds: vec![
                "https://example.test:443/feed#old".into(),
                feed.into(),
                "https://other.test/feed".into(),
            ],
            ..Default::default()
        };
        library
            .episodes
            .insert("Saved episode".into(), downloaded.clone());
        library.podcast_shows.insert(
            feed.into(),
            crate::media::PodcastShow {
                title: "Cached show".into(),
                ..Default::default()
            },
        );
        assert!(library.unsubscribe_podcast(feed));
        assert_eq!(library.feeds, ["https://other.test/feed"]);
        assert_eq!(library.podcast_shows[feed].title, "Cached show");
        assert_eq!(
            fs::read(&library.episodes["Saved episode"]).unwrap(),
            b"offline audio"
        );
        let reloaded: Library =
            serde_json::from_str(&serde_json::to_string(&library).unwrap()).unwrap();
        assert!(!crate::podcasts::contains_feed(&reloaded.feeds, feed));
        assert_eq!(reloaded.episodes["Saved episode"], downloaded);
        assert!(!library.unsubscribe_podcast(feed));
        assert!(!library.unsubscribe_podcast("not a URL"));
    }

    fn legacy_compilation(folder: &str) -> Vec<Track> {
        ["Performer One", "Performer Two"]
            .into_iter()
            .enumerate()
            .map(|(index, artist)| Track {
                path: PathBuf::from(folder).join(format!("{index}.m4a")),
                title: format!("Song {index}"),
                artist: artist.into(),
                album_artist: artist.into(),
                album: "Soundtrack".into(),
                cover: Some("cached-sleeve.cover".into()),
                year: Some(2000 + index as u32),
                track_number: Some(1 + index as u32),
                ..Track::default()
            })
            .collect()
    }

    #[test]
    fn legacy_compilations_merge_offline_without_losing_saved_data() {
        let dir = tempfile::tempdir().unwrap();
        let tracks = legacy_compilation(dir.path().join("Soundtrack").to_str().unwrap());
        let old_key = album_key(&tracks[1]);
        let paths: Vec<_> = tracks.iter().map(|track| track.path.clone()).collect();
        let mut db = Library {
            tracks,
            pins: vec![paths[1].clone()],
            history: vec![paths[0].clone()],
            playlists: BTreeMap::from([("Road trip".into(), paths.clone())]),
            covers: BTreeMap::from([(old_key.clone(), "chosen-cover.png".into())]),
            ..Library::default()
        };
        // Old JSON has no provenance field and remains readable.
        let mut json = serde_json::to_value(&db).unwrap();
        for track in json["tracks"].as_array_mut().unwrap() {
            track
                .as_object_mut()
                .unwrap()
                .remove("album_artist_missing");
        }
        db = serde_json::from_value(json).unwrap();
        let original = db.tracks.clone();
        db.normalize_album_groups();
        let key = album_key(&db.tracks[0]);
        assert_eq!(key, album_key(&db.tracks[1]));
        assert!(key.starts_with("compilation/"));
        assert_eq!(db.covers[&key], PathBuf::from("chosen-cover.png"));
        assert!(db.covers.contains_key(&old_key));
        assert_eq!(db.pins, [paths[1].clone()]);
        assert_eq!(db.history, [paths[0].clone()]);
        assert_eq!(db.playlists["Road trip"], paths);
        for (track, before) in db.tracks.iter().zip(&original) {
            assert_eq!(track.album_artist, "Various Artists");
            assert_eq!(track.artist, before.artist);
            assert_eq!(track.path, before.path);
            assert_eq!(track.title, before.title);
            assert_eq!(track.year, before.year);
            assert_eq!(track.track_number, before.track_number);
        }
        let normalized = serde_json::to_value(&db).unwrap();
        db.normalize_album_groups();
        assert_eq!(serde_json::to_value(&db).unwrap(), normalized);
    }

    #[test]
    fn compilation_identity_keeps_distinct_folders_and_release_ids_separate() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = Library::default();
        for edition in ["Original", "Deluxe"] {
            db.tracks.extend(legacy_compilation(
                dir.path()
                    .join(edition)
                    .join("Soundtrack")
                    .to_str()
                    .unwrap(),
            ));
        }
        db.normalize_album_groups();
        assert_eq!(album_key(&db.tracks[0]), album_key(&db.tracks[1]));
        assert_eq!(album_key(&db.tracks[2]), album_key(&db.tracks[3]));
        assert_ne!(album_key(&db.tracks[0]), album_key(&db.tracks[2]));
        db.tracks[0].release_id = Some("release-one".into());
        db.tracks[1].release_id = Some("release-two".into());
        assert_ne!(album_key(&db.tracks[0]), album_key(&db.tracks[1]));
        db.tracks[2].release_id = Some("release-one".into());
        assert_eq!(album_key(&db.tracks[0]), album_key(&db.tracks[2]));
    }

    #[test]
    fn mixed_explicit_compilation_credits_and_cover_migration_are_deterministic() {
        let mut tracks = legacy_compilation("/music/Soundtrack");
        tracks[0].album_artist = "Various Artists".into();
        tracks[0].album_artist_missing = Some(false);
        tracks[1].album_artist_missing = Some(true);
        tracks[1].compilation = true;
        let mut db = Library {
            tracks,
            ..Library::default()
        };
        db.normalize_album_groups();
        assert_eq!(album_key(&db.tracks[0]), album_key(&db.tracks[1]));
        assert_eq!(db.tracks[0].album_artist_missing, Some(false));

        let dir = tempfile::tempdir().unwrap();
        let tracks = legacy_compilation(dir.path().join("Soundtrack").to_str().unwrap());
        let covers = BTreeMap::from([
            (album_key(&tracks[0]), "first-cover.png".into()),
            (album_key(&tracks[1]), "second-cover.png".into()),
        ]);
        let mut forward = Library {
            tracks: tracks.clone(),
            covers: covers.clone(),
            ..Library::default()
        };
        let mut reversed = Library {
            tracks: tracks.into_iter().rev().collect(),
            covers,
            ..Library::default()
        };
        forward.normalize_album_groups();
        reversed.normalize_album_groups();
        assert_eq!(forward.covers, reversed.covers);
        assert_eq!(
            forward.covers[&album_key(&forward.tracks[0])],
            PathBuf::from("first-cover.png")
        );
    }

    #[test]
    fn album_order_uses_disc_then_track_independent_of_saved_artist_order() {
        let mut tracks: Vec<_> = [(2, 1), (1, 11), (1, 5), (1, 2)]
            .into_iter()
            .map(|(disc, number)| Track {
                disc_number: Some(disc),
                track_number: Some(number),
                ..Track::default()
            })
            .collect();
        sort_album_tracks(&mut tracks);
        assert_eq!(
            tracks
                .iter()
                .map(|track| (track.disc_number.unwrap(), track.track_number.unwrap()))
                .collect::<Vec<_>>(),
            [(1, 2), (1, 5), (1, 11), (2, 1)]
        );
    }

    #[test]
    fn numbered_compilation_discs_share_the_release_directory_and_key() {
        let mut tracks = legacy_compilation("/music/Soundtrack");
        for (index, track) in tracks.iter_mut().enumerate() {
            track.path = PathBuf::from(format!(
                "/music/Soundtrack/{}/song.m4a",
                if index == 0 { "CD1" } else { "Disc 2" }
            ));
            track.artist = "Band".into();
            track.album_artist = "Band".into();
            track.album_artist_missing = Some(true);
            track.compilation = true;
        }
        let mut db = Library {
            tracks,
            ..Library::default()
        };
        db.normalize_album_groups();
        assert_eq!(album_key(&db.tracks[0]), album_key(&db.tracks[1]));
        assert_eq!(
            gnune::library::album_directory(&db.tracks[0].path),
            Path::new("/music/Soundtrack")
        );
        assert_eq!(
            gnune::library::album_directory(Path::new("/music/Soundtrack/Discography/song.m4a")),
            Path::new("/music/Soundtrack/Discography")
        );
        assert!(
            db.tracks
                .iter()
                .all(|track| track.album_artist == "Various Artists")
        );
    }

    #[test]
    fn one_explicit_album_credit_reconciles_missing_credits_but_conflicts_stay_split() {
        let mut tracks = legacy_compilation("/music/Soundtrack");
        tracks[0].album_artist = "House Band".into();
        tracks[0].album_artist_missing = Some(false);
        tracks[1].album_artist_missing = Some(true);
        let original = tracks.clone();
        let mut batch = Library {
            tracks,
            ..Library::default()
        };
        batch.normalize_album_groups();
        assert_eq!(album_key(&batch.tracks[0]), album_key(&batch.tracks[1]));
        assert_eq!(batch.tracks[1].album_artist, "House Band");
        let mut incremental = Library {
            tracks: vec![original[1].clone()],
            ..Library::default()
        };
        incremental.normalize_album_groups();
        incremental.tracks.insert(0, original[0].clone());
        incremental.normalize_album_groups();
        assert_eq!(incremental.tracks, batch.tracks);
        let mut conflict = original[0].clone();
        conflict.path = "/music/Soundtrack/third.m4a".into();
        conflict.album_artist = "Other Band".into();
        incremental.tracks.push(conflict);
        incremental.normalize_album_groups();
        assert_eq!(incremental.tracks[1].album_artist, "Performer Two");
        let keys: std::collections::BTreeSet<_> =
            incremental.tracks.iter().map(album_key).collect();
        assert_eq!(keys.len(), 3);
    }

    #[test]
    fn ordinary_multi_disc_albums_keep_the_existing_identity() {
        let mut tracks = legacy_compilation("/music/Soundtrack");
        for (index, track) in tracks.iter_mut().enumerate() {
            track.artist = "Band".into();
            track.album_artist = "Band".into();
            track.path = PathBuf::from(format!("/music/Soundtrack/Disc {index}/song.flac"));
        }
        tracks[1].album_artist_missing = Some(true); // A newly imported second disc.
        let expected = album_key(&tracks[0]);
        gnune::library::normalize_album_artists(&mut tracks);
        assert_eq!(album_key(&tracks[0]), expected);
        assert_eq!(album_key(&tracks[1]), expected);
        assert_eq!(tracks[0].album_artist, "Band");
    }

    #[test]
    fn incremental_imports_resolve_compilation_members_and_keep_custom_covers() {
        let dir = tempfile::tempdir().unwrap();
        let mut tracks = legacy_compilation(dir.path().join("Soundtrack").to_str().unwrap());
        tracks[0].album_artist_missing = Some(true);
        tracks[1].album_artist_missing = Some(true);
        let mut db = Library {
            covers: BTreeMap::from([(album_key(&tracks[0]), "chosen-cover.png".into())]),
            tracks: vec![tracks.remove(0)],
            ..Library::default()
        };
        db.normalize_album_groups();
        assert_eq!(db.tracks[0].album_artist, "Performer One");
        db.tracks.extend(tracks);
        db.normalize_album_groups();
        let key = album_key(&db.tracks[0]);
        assert_eq!(key, album_key(&db.tracks[1]));
        assert_eq!(db.covers[&key], PathBuf::from("chosen-cover.png"));
    }

    #[test]
    fn offline_loose_tracks_are_not_guessed_to_be_a_compilation() {
        let dir = tempfile::tempdir().unwrap();
        let mut tracks = legacy_compilation(dir.path().join("Loose tracks").to_str().unwrap());
        let keys: Vec<_> = tracks.iter().map(album_key).collect();
        gnune::library::normalize_album_artists(&mut tracks);
        assert_eq!(tracks.iter().map(album_key).collect::<Vec<_>>(), keys);
        assert!(
            tracks
                .iter()
                .all(|track| track.album_artist_missing.is_none())
        );
    }
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
