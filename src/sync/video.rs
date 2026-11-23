//! Explicitly opted-in native video streaming. Cookies authorize one video only;
//! catalog and progress APIs always require the phone's bearer credential.
use super::*;
use std::{
    io::{Seek, SeekFrom},
    sync::atomic::AtomicUsize,
};

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct VideoMetadata {
    pub id: String,
    title: String,
    artist: String,
    album: String,
    duration: f64,
    mime: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    art: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct VideoFile {
    metadata: VideoMetadata,
    pub(super) path: PathBuf,
    identity: FileIdentity,
}

fn video_mime(path: &std::path::Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "mp4" | "m4v" => Some("video/mp4"),
        "webm" => Some("video/webm"),
        "mov" => Some("video/quicktime"),
        "mkv" => Some("video/x-matroska"),
        "avi" => Some("video/x-msvideo"),
        "ogv" => Some("video/ogg"),
        "mpeg" | "mpg" => Some("video/mpeg"),
        _ => None,
    }
}

pub(super) fn include(grant: &mut Grant, library: &crate::state::Library) {
    grant.includes_video = true;
    for path in &library.assets {
        let Some(mime) = video_mime(path) else {
            continue;
        };
        let Ok(file) = fs::File::open(path) else {
            continue;
        };
        let Ok(meta) = file.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let id = hash(&format!("video/{}", path.display()));
        let tags = library.video_tags.get(path);
        let song = tags.map(|tags| tags.song());
        let fallback = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("Untitled video");
        let title = song
            .map(|song| song.title.trim())
            .filter(|title| !title.is_empty())
            .unwrap_or(fallback)
            .to_owned();
        let poster = crate::media::poster_path(path, &crate::state::data_dir().join("posters"));
        let art = fs::metadata(&poster)
            .ok()
            .filter(|meta| meta.is_file() && meta.len() <= 8 * 1024 * 1024)
            .map(|meta| {
                let art = hash(&format!("video-art/{id}"));
                grant.artwork.insert(
                    art.clone(),
                    Artwork {
                        path: poster,
                        identity: identity(&meta),
                    },
                );
                art
            });
        grant.videos.insert(
            id.clone(),
            VideoFile {
                metadata: VideoMetadata {
                    id,
                    title,
                    artist: song.map(|song| song.artist.clone()).unwrap_or_default(),
                    album: song.map(|song| song.album.clone()).unwrap_or_default(),
                    duration: tags.map_or(0., |tags| tags.duration),
                    mime: mime.into(),
                    art,
                },
                path: path.clone(),
                identity: identity(&meta),
            },
        );
    }
    let ids: BTreeMap<_, _> = grant
        .videos
        .values()
        .map(|video| (&video.path, &video.metadata.id))
        .collect();
    grant.video_playlists = library
        .video_playlists
        .iter()
        .map(|(name, paths)| SharedPlaylist {
            id: hash(&format!("video-playlist/{name}")),
            name: name.clone(),
            tracks: paths
                .iter()
                .filter_map(|path| ids.get(path).map(|id| (*id).clone()))
                .collect(),
        })
        .collect();
    grant.scope = hash(
        &serde_json::to_string(&(&grant.scope, &grant.videos, &grant.video_playlists))
            .expect("serializable video catalog"),
    );
}

fn open(video: &VideoFile) -> Result<fs::File, String> {
    let file = fs::File::open(&video.path).map_err(|_| "Video unavailable")?;
    let meta = file.metadata().map_err(|_| "Video unavailable")?;
    if !meta.is_file() || identity(&meta) != video.identity {
        return Err("Video changed or is unavailable".into());
    }
    Ok(file)
}

pub(super) fn manifest(grant: &Grant) -> Vec<VideoMetadata> {
    grant
        .videos
        .values()
        .filter(|video| open(video).is_ok())
        .map(|video| video.metadata.clone())
        .collect()
}
pub(super) fn is_art(grant: &Grant, id: &str) -> bool {
    grant
        .videos
        .values()
        .any(|video| video.metadata.art.as_deref() == Some(id))
}

fn permitted(grant: &Grant, mode: ShareMode, include_video: bool) -> bool {
    mode == ShareMode::Library
        && include_video
        && grant.mode == ShareMode::Library
        && grant.includes_video
}
struct MediaSession {
    grant: String,
    video: String,
    expires: Instant,
}
#[derive(Default)]
pub(super) struct Sessions {
    sessions: BTreeMap<String, MediaSession>,
}
#[derive(Deserialize)]
struct Play {
    id: String,
}
impl Sessions {
    pub(super) fn create(
        &mut self,
        mut request: Request,
        digest: &str,
        grant: &Grant,
        (mode, include_video): (ShareMode, bool),
    ) {
        if !permitted(grant, mode, include_video) {
            reply(
                request,
                403,
                "Video access is unavailable for this pairing. Choose Pair phone in desktop Devices and pair again with the new Tailscale QR.",
                "text/plain",
            );
            return;
        }
        let play: Play = match read_json(&mut request) {
            Ok(play) => play,
            Err(error) => {
                reply(request, 400, error, "text/plain");
                return;
            }
        };
        if !grant.videos.contains_key(&play.id) {
            reply(
                request,
                404,
                "Video is not in this phone's shared selection",
                "text/plain",
            );
            return;
        }
        self.sessions
            .retain(|_, session| session.expires > Instant::now());
        // Bound media cookies; a phone intentionally starting a new video retires
        // its prior stream session rather than growing an unbounded session map.
        self.sessions.retain(|_, session| session.grant != digest);
        if self.sessions.len() >= 64 {
            reply(
                request,
                503,
                "Too many active video players. Try again later.",
                "text/plain",
            );
            return;
        }
        let secret = token();
        self.sessions.insert(
            hash(&secret),
            MediaSession {
                grant: digest.into(),
                video: play.id.clone(),
                expires: Instant::now() + Duration::from_secs(3600),
            },
        );
        let body = serde_json::json!({"url":format!("/api/video/{}", play.id)}).to_string();
        let response = Response::from_string(body).with_header(Header::from_bytes("Content-Type", "application/json").unwrap())
            .with_header(Header::from_bytes("Cache-Control", "no-store").unwrap())
            .with_header(Header::from_bytes("Set-Cookie", format!("gnune_video={secret}; Path=/api/video/; Secure; HttpOnly; SameSite=Strict; Max-Age=3600")).unwrap());
        let _ = request.respond(response);
    }
    pub(super) fn serve(
        &self,
        request: Request,
        id: &str,
        grants: &BTreeMap<String, Grant>,
        (mode, include_video): (ShareMode, bool),
        stop: Arc<AtomicBool>,
        streams: Arc<AtomicUsize>,
    ) {
        let secret = header(&request, "Cookie").and_then(|cookies| {
            cookies.split(';').find_map(|cookie| {
                cookie
                    .trim()
                    .strip_prefix("gnune_video=")
                    .map(str::to_owned)
            })
        });
        let session = secret
            .as_deref()
            .and_then(|secret| self.sessions.get(&hash(secret)))
            .filter(|session| session.expires > Instant::now() && session.video == id);
        let grant = session
            .and_then(|session| grants.get(&session.grant))
            .filter(|grant| permitted(grant, mode, include_video));
        let Some(video) = grant.and_then(|grant| grant.videos.get(id)) else {
            reply(
                request,
                403,
                "Start this video from your paired Gnune player",
                "text/plain",
            );
            return;
        };
        let file = match open(video) {
            Ok(file) => file,
            Err(error) => {
                reply(request, 404, error, "text/plain");
                return;
            }
        };
        stream(request, file, &video.metadata.mime, stop, streams);
    }
}

#[derive(Debug, PartialEq)]
enum Range {
    Full,
    Partial(u64, u64),
    Unsatisfiable,
}
fn range(value: Option<&str>, size: u64) -> Range {
    let Some(value) = value.and_then(|value| value.strip_prefix("bytes=")) else {
        return Range::Full;
    };
    if value.contains(',') {
        return Range::Full;
    }
    let Some((start, end)) = value.split_once('-') else {
        return Range::Full;
    };
    if start.is_empty() {
        let Ok(suffix) = end.parse::<u64>() else {
            return Range::Full;
        };
        if suffix == 0 || size == 0 {
            return Range::Unsatisfiable;
        }
        return Range::Partial(size.saturating_sub(suffix), size - 1);
    }
    let Ok(start) = start.parse::<u64>() else {
        return Range::Full;
    };
    let end = if end.is_empty() {
        size.saturating_sub(1)
    } else {
        match end.parse::<u64>() {
            Ok(end) => end,
            Err(_) => return Range::Full,
        }
    };
    if start >= size || end < start {
        return Range::Unsatisfiable;
    }
    Range::Partial(start, end.min(size - 1))
}
struct Transfer {
    file: std::io::Take<fs::File>,
    stop: Arc<AtomicBool>,
    streams: Arc<AtomicUsize>,
}
impl Read for Transfer {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        if self.stop.load(Ordering::Acquire) {
            return Ok(0);
        }
        self.file.read(bytes)
    }
}
impl Drop for Transfer {
    fn drop(&mut self) {
        self.streams.fetch_sub(1, Ordering::AcqRel);
    }
}
pub(super) fn stream(
    request: Request,
    mut file: fs::File,
    mime: &str,
    stop: Arc<AtomicBool>,
    streams: Arc<AtomicUsize>,
) {
    let Ok(meta) = file.metadata() else {
        reply(request, 404, "Media unavailable", "text/plain");
        return;
    };
    let size = meta.len();
    let range = if request.method() == &Method::Head {
        Range::Full
    } else {
        range(header(&request, "Range").as_deref(), size)
    };
    if range == Range::Unsatisfiable {
        let response = Response::empty(416)
            .with_header(Header::from_bytes("Content-Range", format!("bytes */{size}")).unwrap());
        let _ = request.respond(response);
        return;
    }
    let (status, start, length) = match range {
        Range::Partial(start, end) => (206, start, end - start + 1),
        _ => (200, 0, size),
    };
    if file.seek(SeekFrom::Start(start)).is_err() {
        reply(request, 404, "Media unavailable", "text/plain");
        return;
    }
    if streams
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            (count < 4).then_some(count + 1)
        })
        .is_err()
    {
        reply(
            request,
            503,
            "Too many active transfers. Pause another player and retry.",
            "text/plain",
        );
        return;
    }
    let mut headers = vec![
        Header::from_bytes("Content-Type", mime).unwrap(),
        Header::from_bytes("Accept-Ranges", "bytes").unwrap(),
        Header::from_bytes("Cache-Control", "no-store").unwrap(),
        Header::from_bytes("X-Content-Type-Options", "nosniff").unwrap(),
    ];
    if status == 206 {
        headers.push(
            Header::from_bytes(
                "Content-Range",
                format!("bytes {start}-{}/{size}", start + length - 1),
            )
            .unwrap(),
        );
    }
    let reader = Transfer {
        file: file.take(length),
        stop,
        streams,
    };
    // The native player may keep a range open. A bounded worker leaves the
    // listener free to handle catalog, authorization, seeks and progress.
    let response = Response::new(
        tiny_http::StatusCode(status),
        headers,
        reader,
        usize::try_from(length).ok(),
        None,
    )
    .with_chunked_threshold(usize::MAX);
    let _ = thread::Builder::new()
        .name("gnune-media-transfer".into())
        .spawn(move || {
            let _ = request.respond(response);
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_ranges_cover_closed_open_suffix_and_unsatisfiable_requests() {
        assert_eq!(range(Some("bytes=0-1"), 10), Range::Partial(0, 1));
        assert_eq!(range(Some("bytes=4-"), 10), Range::Partial(4, 9));
        assert_eq!(range(Some("bytes=-4"), 10), Range::Partial(6, 9));
        assert_eq!(range(Some("bytes=8-99"), 10), Range::Partial(8, 9));
        assert_eq!(range(Some("bytes=10-"), 10), Range::Unsatisfiable);
        assert_eq!(range(Some("bytes=3-2"), 10), Range::Unsatisfiable);
        assert_eq!(range(Some("bytes=0-1,4-5"), 10), Range::Full);
        assert_eq!(range(Some("bytes=invalid"), 10), Range::Full);
        assert_eq!(range(Some("bytes=-0"), 10), Range::Unsatisfiable);
    }
    #[test]
    fn video_selection_excludes_photos_and_music() {
        assert_eq!(video_mime(std::path::Path::new("photo.jpg")), None);
        assert_eq!(video_mime(std::path::Path::new("song.mp3")), None);
        assert_eq!(
            video_mime(std::path::Path::new("clip.MP4")),
            Some("video/mp4")
        );
    }
}
