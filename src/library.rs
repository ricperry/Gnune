//! Local, recursive music import. Original media is never modified.

use lofty::{
    file::{AudioFile, TaggedFileExt},
    prelude::*,
    probe::Probe,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
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
    result
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
                track.album_artist = tag
                    .get_string(ItemKey::AlbumArtist)
                    .unwrap_or(&track.artist)
                    .to_owned();
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
