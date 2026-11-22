//! Local, recursive music import. Original media is never modified.

use lofty::{
    config::ParseOptions,
    file::{AudioFile, TaggedFileExt},
    prelude::*,
    probe::Probe,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct Track {
    pub id: String,
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    /// `None` identifies a collection saved before album-artist provenance was kept.
    #[serde(default)]
    pub album_artist_missing: Option<bool>,
    /// The embedded compilation flag, kept separate from inferred grouping.
    #[serde(default)]
    pub compilation: bool,
    /// Derived from the current album evidence, rather than an embedded tag.
    #[serde(default)]
    pub album_folder: bool,
    pub genre: String,
    pub year: Option<u32>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub release_id: Option<String>,
    pub release_group_id: Option<String>,
    pub artist_id: Option<String>,
    pub artist_url: Option<String>,
    /// Duration in seconds.
    pub duration: f64,
    pub cover: Option<PathBuf>,
    pub lyrics: Option<PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub struct ScanResult {
    pub tracks: Vec<Track>,
    pub warnings: Vec<String>,
}

pub fn is_audio_path(path: &Path) -> bool {
    path.extension().and_then(|s| s.to_str()).is_some_and(|s| {
        matches!(
            s.to_ascii_lowercase().as_str(),
            "mp3"
                | "flac"
                | "m4a"
                | "aac"
                | "ogg"
                | "opus"
                | "wav"
                | "aif"
                | "aiff"
                | "ape"
                | "wv"
                | "mpc"
                | "wma"
        )
    })
}

/// Import files and directory trees in deterministic path order, deduplicating
/// overlapping selections. Broken or unsupported tags retain filename metadata.
pub fn scan_paths(paths: &[PathBuf], cache_dir: &Path) -> ScanResult {
    let mut result = ScanResult::default();
    let mut files = BTreeSet::new();
    for path in paths {
        if path.is_file() {
            if is_audio_path(path) {
                match path.canonicalize() {
                    Ok(path) => {
                        files.insert(path);
                    }
                    Err(error) => result.warnings.push(format!("{}: {error}", path.display())),
                }
            }
        } else if path.is_dir() {
            for entry in WalkDir::new(path).follow_links(false).sort_by_file_name() {
                match entry {
                    Ok(entry) if entry.file_type().is_file() && is_audio_path(entry.path()) => {
                        match entry.path().canonicalize() {
                            Ok(path) => {
                                files.insert(path);
                            }
                            Err(error) => result
                                .warnings
                                .push(format!("{}: {error}", entry.path().display())),
                        }
                    }
                    Err(error) => result.warnings.push(error.to_string()),
                    _ => {}
                }
            }
        } else {
            result.warnings.push(format!(
                "{}: file or directory is unavailable",
                path.display()
            ));
        }
    }
    for path in files {
        if path.to_str().is_none() {
            result.warnings.push(format!(
                "Skipped a filename that is not valid UTF-8: {}",
                path.display()
            ));
            continue;
        }

        result
            .tracks
            .push(read_track(&path, cache_dir, &mut result.warnings));
    }
    normalize_album_artists(&mut result.tracks);
    result
}

fn album_credit(tag: &lofty::tag::Tag, artist: &str) -> (String, bool, bool) {
    let compilation = tag
        .get_string(ItemKey::FlagCompilation)
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
    if let Some(credit) = tag
        .get_string(ItemKey::AlbumArtist)
        .filter(|value| !value.trim().is_empty())
    {
        return (credit.to_owned(), false, compilation);
    }
    (
        if compilation {
            "Various Artists"
        } else {
            artist
        }
        .to_owned(),
        true,
        compilation,
    )
}

/// Conventional numbered disc folders belong to their parent release directory.
pub fn album_directory(path: &Path) -> &Path {
    let folder = path.parent().unwrap_or(Path::new(""));
    let numbered_disc = folder
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            let name = name.to_ascii_lowercase();
            name.strip_prefix("disc")
                .or_else(|| name.strip_prefix("cd"))
                .is_some_and(|number| {
                    let number = number.trim_start_matches([' ', '-', '_']);
                    !number.is_empty() && number.chars().all(|c| c.is_ascii_digit())
                })
        });
    if numbered_disc {
        folder.parent().unwrap_or(folder)
    } else {
        folder
    }
}

fn backfill_album_credit(track: &mut Track, offline_evidence: bool) {
    if track.album_artist_missing.is_some() || track.album_artist != track.artist {
        return;
    }
    match Probe::open(&track.path).and_then(|probe| {
        probe
            .options(
                ParseOptions::new()
                    .read_properties(false)
                    .read_cover_art(false),
            )
            .read()
    }) {
        Ok(file) => {
            let (credit, missing, compilation) = file
                .primary_tag()
                .or_else(|| file.first_tag())
                .map(|tag| album_credit(tag, &track.artist))
                .unwrap_or_else(|| (track.artist.clone(), true, false));
            track.album_artist = credit;
            track.album_artist_missing = Some(missing);
            track.compilation = compilation;
        }
        Err(_) if offline_evidence => track.album_artist_missing = Some(true),
        Err(_) => {}
    }
}

/// Infer credits from current release evidence without changing explicit tags.
pub fn normalize_album_artists(tracks: &mut [Track]) {
    let mut albums: BTreeMap<_, Vec<usize>> = BTreeMap::new();
    for (index, track) in tracks.iter().enumerate() {
        albums
            .entry((
                album_directory(&track.path).to_owned(),
                track.album.clone(),
                track.release_id.clone(),
            ))
            .or_default()
            .push(index);
    }
    for ((folder, album, release), indices) in albums {
        let eligible = release.is_none() && album != "Unknown album";
        let performers: BTreeSet<_> = indices.iter().map(|&i| tracks[i].artist.clone()).collect();
        let normalized = |name: &str| {
            name.chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        };
        let named_folder = folder
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|name| normalized(name) == normalized(&album));
        let cover = &tracks[indices[0]].cover;
        let shared_cover = cover.is_some() && indices.iter().all(|&i| tracks[i].cover == *cover);
        // Offline legacy data needs stronger evidence than readable source tags.
        if eligible
            && (performers.len() > 1
                || indices
                    .iter()
                    .any(|&i| tracks[i].album_artist != tracks[i].artist))
        {
            for &index in &indices {
                backfill_album_credit(&mut tracks[index], named_folder && shared_cover);
            }
        }
        let explicit: BTreeSet<_> = indices
            .iter()
            .filter(|&&i| tracks[i].album_artist_missing != Some(true))
            .map(|&i| tracks[i].album_artist.clone())
            .collect();
        let evidence = eligible
            && (named_folder || shared_cover || indices.iter().any(|&i| tracks[i].compilation));
        let credit = (evidence && explicit.len() == 1).then(|| explicit.first().unwrap().clone());
        let infer = evidence && explicit.is_empty() && performers.len() > 1;
        for index in indices {
            let track = &mut tracks[index];
            let missing = track.album_artist_missing == Some(true);
            track.album_folder =
                eligible && missing && credit.is_none() && (infer || track.compilation);
            if missing {
                track.album_artist = credit.clone().unwrap_or_else(|| {
                    if infer || track.compilation {
                        "Various Artists".into()
                    } else {
                        track.artist.clone()
                    }
                });
            }
        }
    }
}

fn read_track(path: &Path, cache: &Path, warnings: &mut Vec<String>) -> Track {
    let mut track = Track {
        id: digest(path.as_os_str().as_encoded_bytes()),
        path: path.to_owned(),
        title: path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        artist: "Unknown artist".into(),
        album: "Unknown album".into(),
        album_artist: String::new(),
        album_artist_missing: Some(true),
        compilation: false,
        album_folder: false,
        genre: "Unknown genre".into(),
        year: None,
        track_number: None,
        disc_number: None,
        release_id: None,
        release_group_id: None,
        artist_id: None,
        artist_url: None,
        duration: 0.0,
        cover: None,
        lyrics: sidecar(path, "lrc"),
    };
    match Probe::open(path).and_then(|probe| probe.read()) {
        Ok(file) => {
            track.duration = file.properties().duration().as_secs_f64();
            if let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) {
                if let Some(value) = tag.title().filter(|v| !v.trim().is_empty()) {
                    track.title = value.into_owned();
                }
                if let Some(value) = tag.artist().filter(|v| !v.trim().is_empty()) {
                    track.artist = value.into_owned();
                }
                if let Some(value) = tag.album().filter(|v| !v.trim().is_empty()) {
                    track.album = value.into_owned();
                }
                if let Some(value) = tag.genre().filter(|v| !v.trim().is_empty()) {
                    track.genre = value.into_owned();
                }
                let (credit, missing, compilation) = album_credit(tag, &track.artist);
                track.album_artist = credit;
                track.album_artist_missing = Some(missing);
                track.compilation = compilation;
                track.year = tag.date().map(|date| u32::from(date.year));
                track.release_id = tag
                    .get_string(ItemKey::MusicBrainzReleaseId)
                    .map(str::to_owned);
                track.release_group_id = tag
                    .get_string(ItemKey::MusicBrainzReleaseGroupId)
                    .map(str::to_owned);
                track.artist_id = tag
                    .get_string(ItemKey::MusicBrainzArtistId)
                    .map(str::to_owned);
                track.artist_url = tag
                    .get_locators(ItemKey::TrackArtistUrl)
                    .next()
                    .or_else(|| tag.get_string(ItemKey::TrackArtistUrl))
                    .map(str::to_owned);
                track.track_number = tag.track();
                track.disc_number = tag.disk();
                let picture = tag
                    .pictures()
                    .iter()
                    .find(|p| p.pic_type() == lofty::picture::PictureType::CoverFront)
                    .or_else(|| tag.pictures().first());
                if let Some(picture) = picture {
                    match cache_cover(picture.data(), cache) {
                        Ok(path) => track.cover = Some(path),
                        Err(error) => {
                            warnings.push(format!("{}: cover cache: {error}", path.display()))
                        }
                    }
                }
            }
        }
        Err(error) => warnings.push(format!(
            "{}: metadata unavailable ({error}); using filename",
            path.display()
        )),
    }
    if track.album_artist.is_empty() {
        track.album_artist = track.artist.clone();
    }
    if track.cover.is_none()
        && let Some(folder_cover) = folder_cover(path)
    {
        match fs::read(folder_cover).and_then(|bytes| cache_cover(&bytes, cache)) {
            Ok(path) => track.cover = Some(path),
            Err(error) => warnings.push(format!("{}: cover cache: {error}", path.display())),
        }
    }
    track
}

fn sidecar(path: &Path, extension: &str) -> Option<PathBuf> {
    let stem = path.file_stem()?;
    let mut siblings: Vec<_> = fs::read_dir(path.parent()?)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    siblings.sort();
    siblings.into_iter().find(|p| {
        p.is_file()
            && p.file_stem() == Some(stem)
            && p.extension()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.eq_ignore_ascii_case(extension))
    })
}

fn folder_cover(path: &Path) -> Option<PathBuf> {
    let mut siblings: Vec<_> = fs::read_dir(path.parent()?)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    siblings.sort();
    for name in ["cover", "folder", "front", "album", "albumart"] {
        if let Some(found) = siblings.iter().find(|p| {
            p.is_file()
                && p.file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.eq_ignore_ascii_case(name))
                && p.extension().and_then(|s| s.to_str()).is_some_and(|s| {
                    matches!(
                        s.to_ascii_lowercase().as_str(),
                        "jpg" | "jpeg" | "png" | "webp"
                    )
                })
        }) {
            return Some(found.clone());
        }
    }
    None
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn save_image(file: &Path, cache: &Path) -> std::io::Result<PathBuf> {
    cache_cover(&fs::read(file)?, cache)
}

fn cache_cover(bytes: &[u8], cache: &Path) -> std::io::Result<PathBuf> {
    fs::create_dir_all(cache)?;
    // Image loaders sniff the contents; a content hash avoids stale covers when
    // tags change and shares artwork across tracks without basename collisions.
    let path = cache.join(format!("{}.cover", digest(bytes)));
    if !path.is_file() {
        let mut temporary = tempfile::NamedTempFile::new_in(cache)?;
        std::io::Write::write_all(&mut temporary, bytes)?;
        temporary.persist(&path).map_err(|error| error.error)?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tagged_wav(path: &Path, artist: &str, album_artist: Option<&str>, compilation: bool) {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&38_u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&8000_u32.to_le_bytes());
        bytes.extend_from_slice(&16000_u32.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&2_u32.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        fs::write(path, bytes).unwrap();
        let mut tag = lofty::tag::Tag::new(lofty::tag::TagType::Id3v2);
        tag.set_artist(artist.into());
        tag.set_album("Soundtrack".into());
        if compilation {
            tag.insert_text(ItemKey::FlagCompilation, "1".into());
        }
        if let Some(credit) = album_artist {
            tag.insert_text(ItemKey::AlbumArtist, credit.into());
        }
        tag.save_to_path(path, lofty::config::WriteOptions::default())
            .unwrap();
    }

    #[test]
    fn compilation_import_and_legacy_backfill_keep_song_credits() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.wav");
        let second = dir.path().join("second.wav");
        tagged_wav(&first, "Performer One", None, true);
        tagged_wav(&second, "Performer Two", None, true);
        let mut scan = scan_paths(&[dir.path().into()], &dir.path().join("cache"));
        assert!(scan.warnings.is_empty(), "{:?}", scan.warnings);
        assert_eq!(scan.tracks.len(), 2);
        assert_eq!(scan.tracks[0].artist, "Performer One");
        assert_eq!(scan.tracks[1].artist, "Performer Two");
        for track in &mut scan.tracks {
            assert_eq!(track.album_artist, "Various Artists");
            assert_eq!(track.album_artist_missing, Some(true));
            // Recreate the metadata that older collections saved.
            track.album_artist = track.artist.clone();
            track.album_artist_missing = None;
        }
        normalize_album_artists(&mut scan.tracks);
        assert!(
            scan.tracks
                .iter()
                .all(|track| track.album_artist == "Various Artists")
        );
        assert!(
            scan.tracks
                .iter()
                .all(|track| track.album_artist_missing == Some(true))
        );
    }

    #[test]
    fn explicit_album_artist_overrides_compilation_flag_and_legacy_inference() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.wav");
        let second = dir.path().join("second.wav");
        tagged_wav(&first, "Performer One", Some("Performer One"), true);
        tagged_wav(&second, "Performer Two", Some("Performer Two"), true);
        let mut scan = scan_paths(&[dir.path().into()], &dir.path().join("cache"));
        for track in &mut scan.tracks {
            assert_eq!(track.album_artist, track.artist);
            assert_eq!(track.album_artist_missing, Some(false));
            track.album_artist_missing = None;
        }
        normalize_album_artists(&mut scan.tracks);
        assert!(
            scan.tracks
                .iter()
                .all(|track| track.album_artist == track.artist)
        );
        assert!(
            scan.tracks
                .iter()
                .all(|track| track.album_artist_missing == Some(false))
        );
    }

    #[test]
    fn loose_same_title_imports_need_album_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.wav");
        let second = dir.path().join("second.wav");
        tagged_wav(&first, "Performer One", None, false);
        tagged_wav(&second, "Performer Two", None, false);
        let mut tracks = scan_paths(&[dir.path().into()], &dir.path().join("cache")).tracks;
        assert_eq!(tracks.len(), 2);
        assert!(
            tracks
                .iter()
                .all(|track| track.album_artist == track.artist && !track.album_folder)
        );
        // A shared sleeve supplies evidence; removing it must undo inference.
        for track in &mut tracks {
            track.cover = Some("sleeve.cover".into());
        }
        normalize_album_artists(&mut tracks);
        assert!(tracks.iter().all(|track| track.album_folder));
        tracks[1].cover = None;
        normalize_album_artists(&mut tracks);
        assert!(
            tracks
                .iter()
                .all(|track| track.album_artist == track.artist && !track.album_folder)
        );
    }

    #[test]
    fn explicit_various_artists_and_missing_compilation_credit_agree_across_imports() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.wav");
        let second = dir.path().join("second.wav");
        tagged_wav(&first, "Performer One", Some("Various Artists"), false);
        tagged_wav(&second, "Performer Two", None, true);
        let cache = dir.path().join("cache");
        let batch = scan_paths(&[first.clone(), second.clone()], &cache).tracks;
        let mut incremental = scan_paths(&[first], &cache).tracks;
        incremental.extend(scan_paths(&[second], &cache).tracks);
        normalize_album_artists(&mut incremental);
        assert_eq!(incremental, batch);
        assert!(
            batch
                .iter()
                .all(|track| track.album_artist == "Various Artists" && !track.album_folder)
        );
        assert_eq!(batch[0].album_artist_missing, Some(false));
        assert_eq!(batch[1].album_artist_missing, Some(true));
        assert!(batch[1].compilation);
    }

    #[test]
    fn recursive_scan_is_sorted_deduplicated_and_retains_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        let album = dir.path().join("nested/album");
        fs::create_dir_all(&album).unwrap();
        fs::write(album.join("b.MP3"), b"invalid audio").unwrap();
        fs::write(album.join("a.flac"), b"invalid audio").unwrap();
        fs::write(album.join("a.LRC"), b"[00:00.00]hello").unwrap();
        fs::write(album.join("Cover.PNG"), b"cover bytes").unwrap();
        let cache = dir.path().join("cache");
        let paths = [dir.path().to_owned(), album.join("a.flac")];
        let result = scan_paths(&paths, &cache);
        assert_eq!(result.tracks.len(), 2);
        assert_eq!(result.tracks[0].title, "a");
        assert!(result.tracks[0].lyrics.is_some());
        assert_eq!(result.tracks[0].cover, result.tracks[1].cover);
        assert_eq!(fs::read_dir(&cache).unwrap().count(), 1);
        let again = scan_paths(&paths, &cache);
        assert_eq!(result.tracks, again.tracks);
        assert_eq!(result.warnings.len(), 2);
    }
}
