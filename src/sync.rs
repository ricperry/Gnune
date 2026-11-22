//! Opt-in local HTTPS phone companion. No listener exists until `start` runs.
use gnune::library::Track;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    os::unix::fs::MetadataExt,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant, UNIX_EPOCH},
};
use tiny_http::{Header, Method, Request, Response, Server, SslConfig};
mod settings;
mod video;

#[derive(Debug)]
pub struct SharingError {
    pub message: String,
    pub retryable: bool,
    pub waiting: bool,
}
impl From<String> for SharingError {
    fn from(message: String) -> Self {
        Self {
            message,
            retryable: false,
            waiting: false,
        }
    }
}
impl From<&str> for SharingError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}
impl std::fmt::Display for SharingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(f)
    }
}
pub fn stop_remembered() -> Result<(), String> {
    settings::disable()
}

pub fn resume(
    library: &crate::state::Library,
    callback: impl Fn(PathBuf, f64) + Send + 'static,
) -> Result<Option<Session>, SharingError> {
    let Some(saved) = settings::load()?.filter(|saved| saved.enabled) else {
        return Ok(None);
    };
    let result = (|| {
        let selected = if saved.mode == ShareMode::Library {
            library_selection(library, saved.include_video)?
        } else {
            (
                saved
                    .queue
                    .clone()
                    .ok_or("Saved queue sharing selection is missing")?,
                saved.skipped,
            )
        };
        start_configured(selected, callback, settings::connection(Some(&saved))?)
    })();
    result.map(Some).map_err(|mut e| {
        e.waiting = true;
        e
    })
}

pub struct Session {
    pub url: String,
    pub mode: ShareMode,
    pub track_count: usize,
    pub skipped: usize,
    pub include_video: bool,
    server: Arc<Server>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
    pairing: Arc<Mutex<Pairing>>,
}
struct Pairing {
    code: Option<String>,
    expires: Instant,
    failures: u32,
}
impl Pairing {
    fn new(code: String) -> Self {
        Self {
            code: Some(code),
            expires: Instant::now() + Duration::from_secs(600),
            failures: 0,
        }
    }
    fn accepts(&self, code: &str) -> bool {
        self.failures < 30 && Instant::now() <= self.expires && self.code.as_deref() == Some(code)
    }
}
impl Session {
    pub fn renew_pairing(&mut self) {
        let code = token();
        self.url = format!(
            "{}#pair={code}",
            self.url.split('#').next().unwrap_or(&self.url)
        );
        *self.pairing.lock().unwrap_or_else(|e| e.into_inner()) = Pairing::new(code);
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.server.unblock();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct SharedTrack {
    id: String,
    title: String,
    artist: String,
    album: String,
    album_id: String,
    album_artist: String,
    genre: String,
    year: Option<u32>,
    disc_number: Option<u32>,
    track_number: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    art: Option<String>,
    duration: f64,
    bytes: u64,
    version: String,
    mime: String,
}
#[derive(Clone, Copy, Default, Serialize, Deserialize, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum ShareMode {
    #[default]
    Queue,
    Library,
}
#[derive(Clone, Serialize, Deserialize)]
struct SharedPlaylist {
    id: String,
    name: String,
    tracks: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Artwork {
    path: PathBuf,
    identity: FileIdentity,
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct Grant {
    scope: String,
    name: String,
    tracks: Vec<Track>,
    files: BTreeMap<String, FileIdentity>,
    last_seq: u64,
    positions: BTreeMap<String, f64>,
    #[serde(default)]
    mode: ShareMode,
    #[serde(default)]
    playlists: Vec<SharedPlaylist>,
    #[serde(default)]
    artwork: BTreeMap<String, Artwork>,
    #[serde(default)]
    includes_video: bool,
    #[serde(default)]
    videos: BTreeMap<String, video::VideoFile>,
    #[serde(default)]
    video_playlists: Vec<SharedPlaylist>,
}
fn refresh_library_grants(store: &mut Store, selection: &Grant) {
    if selection.mode != ShareMode::Library {
        return;
    }
    for grant in store
        .devices
        .values_mut()
        .filter(|grant| grant.mode == ShareMode::Library)
    {
        grant.tracks = selection.tracks.clone();
        grant.files = selection.files.clone();
        grant.playlists = selection.playlists.clone();
        grant.artwork = selection
            .artwork
            .iter()
            .filter(|(id, _)| grant.includes_video || !video::is_art(selection, id))
            .map(|(id, art)| (id.clone(), art.clone()))
            .collect();
        // The original video cap survives a music-only active session. A later
        // explicit video opt-in may restore that capability, never add it.
        if grant.includes_video && selection.includes_video {
            grant.videos = selection.videos.clone();
            grant.video_playlists = selection.video_playlists.clone();
        } else {
            grant.videos.clear();
            grant.video_playlists.clear();
        }
    }
}
#[derive(Clone, Serialize, Deserialize, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    bytes: u64,
    modified_ns: u128,
}
fn identity(meta: &fs::Metadata) -> FileIdentity {
    FileIdentity {
        device: meta.dev(),
        inode: meta.ino(),
        bytes: meta.len(),
        modified_ns: meta
            .modified()
            .ok()
            .and_then(|v| v.duration_since(UNIX_EPOCH).ok())
            .map(|v| v.as_nanos())
            .unwrap_or(0),
    }
}
fn open_allowed(
    track: &Track,
    allowed: &BTreeMap<String, FileIdentity>,
) -> Result<fs::File, String> {
    let file = fs::File::open(&track.path).map_err(|e| e.to_string())?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    // Validate the opened descriptor, so swapping a path to a symlink between
    // validation and open cannot expose another file. No bytes are read first.
    if !meta.is_file() || allowed.get(&track.id) != Some(&identity(&meta)) {
        return Err("Shared file changed or was replaced. Share this queue again.".into());
    }
    Ok(file)
}
#[derive(Default, Serialize, Deserialize)]
struct Store {
    devices: BTreeMap<String, Grant>,
}
#[derive(Deserialize)]
struct Pair {
    code: String,
    name: String,
}
#[derive(Deserialize)]
struct Update {
    seq: u64,
    id: String,
    position: f64,
    completed: bool,
}
#[derive(Deserialize)]
struct Progress {
    updates: Vec<Update>,
}

fn token() -> String {
    let mut bytes = [0_u8; 24];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
fn header(request: &Request, name: &'static str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str().to_owned())
}
fn reply(request: Request, status: u16, body: impl Into<Vec<u8>>, mime: &str) {
    reply_headers(request, status, body, mime, &[]);
}
fn reply_headers(
    request: Request,
    status: u16,
    body: impl Into<Vec<u8>>,
    mime: &str,
    extra: &[(&str, String)],
) {
    let mut response = Response::from_data(body.into()).with_status_code(status);
    for (name, value) in [
        ("Content-Type", mime),
        ("Cache-Control", "no-store"),
        ("X-Content-Type-Options", "nosniff"),
        ("Referrer-Policy", "no-referrer"),
        (
            "Content-Security-Policy",
            "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data: blob:; media-src 'self' blob:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'",
        ),
    ] {
        response.add_header(Header::from_bytes(name, value).expect("static header"));
    }
    for (name, value) in extra {
        response
            .add_header(Header::from_bytes(*name, value.as_str()).expect("valid response header"));
    }
    let _ = request.respond(response);
}
fn resume_cookie(secret: &str) -> String {
    format!(
        "gnune_resume={secret}; Path=/api/resume; Secure; HttpOnly; SameSite=Strict; Max-Age={}",
        if secret.is_empty() { 0 } else { 31_536_000 }
    )
}
fn cookie(request: &Request, name: &str) -> Option<String> {
    header(request, "Cookie").and_then(|cookies| {
        cookies.split(';').find_map(|part| {
            part.trim()
                .split_once('=')
                .filter(|(key, _)| *key == name)
                .map(|(_, value)| value.to_owned())
        })
    })
}
fn json(request: Request, status: u16, value: serde_json::Value) {
    reply(
        request,
        status,
        value.to_string(),
        "application/json; charset=utf-8",
    );
}
fn read_json<T: for<'de> Deserialize<'de>>(request: &mut Request) -> Result<T, String> {
    if !header(request, "Content-Type").is_some_and(|v| v.starts_with("application/json")) {
        return Err("Expected application/json".into());
    }
    let mut bytes = Vec::new();
    request
        .as_reader()
        .take(65_537)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 65_536 {
        return Err("Request is too large".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}
fn state_path() -> Result<PathBuf, String> {
    let root = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/share")))
        .ok_or("Set XDG_DATA_HOME to store phone pairings")?;
    Ok(root.join("gnune/mobile-devices.json"))
}
fn persist(path: &PathBuf, store: &Store) -> Result<(), String> {
    use std::io::Write;
    fs::create_dir_all(path.parent().ok_or("Invalid state directory")?)
        .map_err(|e| e.to_string())?;
    let mut file =
        tempfile::NamedTempFile::new_in(path.parent().unwrap()).map_err(|e| e.to_string())?;
    file.write_all(&serde_json::to_vec(store).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    file.persist(path).map_err(|e| e.error.to_string())?;
    fs::File::open(path.parent().unwrap())
        .and_then(|dir| dir.sync_all())
        .map_err(|e| e.to_string())?;
    Ok(())
}
fn shared(track: &Track, meta: &fs::Metadata) -> SharedTrack {
    let modified = meta
        .modified()
        .ok()
        .and_then(|v| v.duration_since(UNIX_EPOCH).ok())
        .map(|v| v.as_nanos())
        .unwrap_or(0);
    let extension = track
        .path
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mime = match extension.as_str() {
        "mp3" => "audio/mpeg",
        "m4a" | "mp4" => "audio/mp4",
        "aac" => "audio/aac",
        "flac" => "audio/flac",
        "ogg" | "opus" => "audio/ogg",
        "wav" => "audio/wav",
        "aif" | "aiff" => "audio/aiff",
        _ => "application/octet-stream",
    };
    SharedTrack {
        id: track.id.clone(),
        title: track.title.clone(),
        artist: track.artist.clone(),
        album: track.album.clone(),
        album_id: hash(&crate::state::album_key(track)),
        album_artist: track.album_artist.clone(),
        genre: track.genre.clone(),
        year: track.year,
        disc_number: track.disc_number,
        track_number: track.track_number,
        art: None,
        duration: track.duration,
        bytes: meta.len(),
        version: format!("{}-{modified}", meta.len()),
        mime: mime.into(),
    }
}

/// Pin file identities, album grouping and ordered playlist membership to this
/// grant. Starting another share never enlarges an existing phone's grant.
fn selection(
    tracks: Vec<Track>,
    mode: ShareMode,
    playlists: &BTreeMap<String, Vec<PathBuf>>,
    covers: &BTreeMap<String, PathBuf>,
) -> Result<(Grant, usize), String> {
    let total = tracks.len();
    let mut grant = Grant {
        scope: String::new(),
        name: String::new(),
        tracks: Vec::new(),
        files: BTreeMap::new(),
        last_seq: 0,
        positions: BTreeMap::new(),
        mode,
        playlists: Vec::new(),
        artwork: BTreeMap::new(),
        includes_video: false,
        videos: BTreeMap::new(),
        video_playlists: Vec::new(),
    };
    let mut selected = Vec::new();
    for track in tracks {
        let metadata = fs::File::open(&track.path).and_then(|file| file.metadata());
        let meta = match metadata {
            Ok(meta) if meta.is_file() => meta,
            _ if mode == ShareMode::Library => continue,
            _ => {
                return Err(
                    "A queued file is unavailable. Choose a playable queue and share again.".into(),
                );
            }
        };
        let album = crate::state::album_key(&track);
        let album_id = hash(&album);
        if let Some(path) = covers.get(&album).or(track.cover.as_ref())
            && let Ok(meta) = fs::metadata(path)
            && meta.is_file()
            && meta.len() <= 8 * 1024 * 1024
        {
            grant.artwork.entry(album_id).or_insert_with(|| Artwork {
                path: path.clone(),
                identity: identity(&meta),
            });
        }
        grant.files.insert(track.id.clone(), identity(&meta));
        selected.push(shared(&track, &meta));
        grant.tracks.push(track);
    }
    if grant.tracks.is_empty() && mode == ShareMode::Queue {
        return Err("There are no available music files to share.".into());
    }
    if mode == ShareMode::Library {
        let ids: BTreeMap<_, _> = grant
            .tracks
            .iter()
            .map(|track| (&track.path, &track.id))
            .collect();
        grant.playlists = playlists
            .iter()
            .map(|(name, paths)| SharedPlaylist {
                id: hash(&format!("playlist/{name}")),
                name: name.clone(),
                tracks: paths
                    .iter()
                    .filter_map(|path| ids.get(path).map(|id| (*id).clone()))
                    .collect(),
            })
            .collect();
    }
    grant.scope = hash(
        &serde_json::to_string(&(mode, selected, &grant.playlists)).map_err(|e| e.to_string())?,
    );
    let skipped = total - grant.tracks.len();
    Ok((grant, skipped))
}

fn manifest(grant: &Grant, include_video: bool) -> serde_json::Value {
    let tracks: Vec<_> = grant
        .tracks
        .iter()
        .filter_map(|track| {
            let file = open_allowed(track, &grant.files).ok()?;
            let mut track = shared(track, &file.metadata().ok()?);
            if grant.artwork.contains_key(&track.album_id) {
                track.art = Some(track.album_id.clone());
            }
            Some(track)
        })
        .collect();
    let available: std::collections::BTreeSet<_> =
        tracks.iter().map(|track| track.id.as_str()).collect();
    let playlists: Vec<_> = grant
        .playlists
        .iter()
        .map(|playlist| SharedPlaylist {
            id: playlist.id.clone(),
            name: playlist.name.clone(),
            tracks: playlist
                .tracks
                .iter()
                .filter(|id| available.contains(id.as_str()))
                .cloned()
                .collect(),
        })
        .collect();
    let videos = if include_video {
        video::manifest(grant)
    } else {
        Vec::new()
    };
    let available: std::collections::BTreeSet<_> =
        videos.iter().map(|video| video.id.as_str()).collect();
    let video_playlists: Vec<_> = grant
        .video_playlists
        .iter()
        .map(|playlist| SharedPlaylist {
            id: playlist.id.clone(),
            name: playlist.name.clone(),
            tracks: playlist
                .tracks
                .iter()
                .filter(|id| available.contains(id.as_str()))
                .cloned()
                .collect(),
        })
        .collect();
    serde_json::json!({"scope":grant.scope,"mode":grant.mode,"tracks":tracks,"playlists":playlists,"includes_video":grant.includes_video && include_video,"videos":videos,"video_playlists":video_playlists})
}

fn artwork(grant: &Grant, id: &str) -> Result<Vec<u8>, String> {
    let art = grant
        .artwork
        .get(id)
        .ok_or("Artwork is not in this phone's shared selection")?;
    let mut file = fs::File::open(&art.path).map_err(|_| "Artwork unavailable")?;
    let meta = file.metadata().map_err(|_| "Artwork unavailable")?;
    if !meta.is_file() || identity(&meta) != art.identity || meta.len() > 8 * 1024 * 1024 {
        return Err("Artwork changed or is unavailable".into());
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Artwork unavailable")?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err("Artwork is too large".into());
    }
    let stream = gtk::gio::MemoryInputStream::from_bytes(&gtk::glib::Bytes::from_owned(bytes));
    let image = gtk::gdk_pixbuf::Pixbuf::from_stream_at_scale(
        &stream,
        512,
        512,
        true,
        None::<&gtk::gio::Cancellable>,
    )
    .map_err(|_| "Artwork format unavailable")?;
    image
        .save_to_bufferv("png", &[])
        .map_err(|_| "Artwork unavailable".into())
}

/// The caller must invoke this only after an explicit Share action. PEM material
/// is supplied by the user; this module never changes certificate trust/firewalls.
pub fn start(
    tracks: Vec<Track>,
    callback: impl Fn(PathBuf, f64) + Send + 'static,
) -> Result<Session, SharingError> {
    start_selection(
        selection(tracks, ShareMode::Queue, &BTreeMap::new(), &BTreeMap::new())?,
        callback,
    )
}

pub fn start_library(
    library: &crate::state::Library,
    include_videos: bool,
    callback: impl Fn(PathBuf, f64) + Send + 'static,
) -> Result<Session, SharingError> {
    start_selection(library_selection(library, include_videos)?, callback)
}
fn library_selection(
    library: &crate::state::Library,
    include_videos: bool,
) -> Result<(Grant, usize), String> {
    let (mut grant, skipped) = selection(
        library
            .tracks
            .iter()
            .filter(|track| gnune::library::is_audio_path(&track.path))
            .cloned()
            .collect(),
        ShareMode::Library,
        &library.playlists,
        &library.covers,
    )?;
    if include_videos {
        video::include(&mut grant, library);
    }
    if grant.tracks.is_empty() && grant.videos.is_empty() {
        return Err("There are no available files to share.".into());
    }
    Ok((grant, skipped))
}

fn start_selection(
    (selection, skipped): (Grant, usize),
    callback: impl Fn(PathBuf, f64) + Send + 'static,
) -> Result<Session, SharingError> {
    let saved = settings::load()?;
    let connection = settings::connection(saved.as_ref())?;
    start_configured((selection, skipped), callback, connection)
}
fn start_configured(
    (selection, skipped): (Grant, usize),
    callback: impl Fn(PathBuf, f64) + Send + 'static,
    connection: settings::Connection,
) -> Result<Session, SharingError> {
    let (cert, key) = settings::tls(&connection)?;
    let origin = connection.origin.clone();
    let mode = selection.mode;
    let track_count = selection.tracks.len();
    let include_video = selection.includes_video;
    let state = state_path()?;
    let mut store: Store = match fs::read(&state) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| format!("Cannot read saved phone pairings: {e}"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Store::default(),
        Err(e) => return Err(e.to_string().into()),
    };
    refresh_library_grants(&mut store, &selection);
    // Confirm durable storage works before opening a network listener.
    persist(&state, &store)?;
    let remembered = settings::Saved {
        enabled: true,
        connection: connection.clone(),
        mode,
        include_video,
        queue: (mode == ShareMode::Queue).then(|| selection.clone()),
        skipped,
    };
    let server = match Server::https(
        connection.bind,
        SslConfig {
            certificate: cert,
            private_key: key,
        },
    ) {
        Ok(server) => {
            settings::save(&remembered)?;
            Arc::new(server)
        }
        Err(error) => {
            let retryable = error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::AddrNotAvailable);
            if retryable {
                settings::save(&remembered)?;
            }
            return Err(SharingError {
                message: format!(
                    "Phone sharing could not start at {}: {error}",
                    connection.bind
                ),
                retryable,
                waiting: retryable,
            });
        }
    };
    let stop = Arc::new(AtomicBool::new(false));
    let code = token();
    let session_url = format!("{origin}/#pair={code}");
    let pairing = Arc::new(Mutex::new(Pairing::new(code)));
    let worker_pairing = pairing.clone();
    let worker_server = server.clone();
    let worker_stop = stop.clone();
    let serve = move || {
        let mut video_sessions = video::Sessions::default();
        let streams = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        while !worker_stop.load(Ordering::Acquire) {
            let mut request = match worker_server.recv_timeout(Duration::from_millis(200)) {
                Ok(Some(r)) => r,
                Ok(None) => continue,
                Err(_) => break,
            };
            if worker_stop.load(Ordering::Acquire) {
                break;
            }
            let path = request.url().to_owned();
            let method = request.method().clone();
            if header(&request, "Host").as_deref() != origin.strip_prefix("https://") {
                reply(
                    request,
                    421,
                    "Use the configured Gnune HTTPS origin",
                    "text/plain",
                );
                continue;
            }
            if method == Method::Get {
                if path == "/icon-192.png" {
                    reply(
                        request,
                        200,
                        include_bytes!("mobile/icon-192.png").as_slice(),
                        "image/png",
                    );
                    continue;
                }
                if path == "/icon-512.png" {
                    reply(
                        request,
                        200,
                        include_bytes!("mobile/icon-512.png").as_slice(),
                        "image/png",
                    );
                    continue;
                }
                let asset = match path.as_str() {
                    "/" => Some((
                        include_str!("mobile/index.html"),
                        "text/html; charset=utf-8",
                    )),
                    "/app.js" => Some((
                        include_str!("mobile/app.js"),
                        "text/javascript; charset=utf-8",
                    )),
                    "/catalog.js" => Some((
                        include_str!("mobile/catalog.js"),
                        "text/javascript; charset=utf-8",
                    )),
                    "/sw.js" => Some((
                        include_str!("mobile/sw.js"),
                        "text/javascript; charset=utf-8",
                    )),
                    "/style.css" => {
                        Some((include_str!("mobile/style.css"), "text/css; charset=utf-8"))
                    }
                    "/icon.svg" => Some((include_str!("mobile/icon.svg"), "image/svg+xml")),
                    "/manifest.webmanifest" => Some((
                        r##"{"name":"Gnune on your phone","short_name":"Gnune","id":"/","start_url":"/","scope":"/","display":"standalone","background_color":"#111014","theme_color":"#111014","icons":[{"src":"/icon-192.png","sizes":"192x192","type":"image/png","purpose":"any"},{"src":"/icon-512.png","sizes":"512x512","type":"image/png","purpose":"any"}]}"##,
                        "application/manifest+json",
                    )),
                    _ => None,
                };
                if let Some((body, mime)) = asset {
                    reply(request, 200, body, mime);
                    continue;
                }
            }
            if method == Method::Post
                && header(&request, "Origin").as_deref() != Some(origin.as_str())
            {
                reply(
                    request,
                    403,
                    "Open Gnune from its configured HTTPS origin",
                    "text/plain",
                );
                continue;
            }
            if matches!(method, Method::Get | Method::Head) && path.starts_with("/api/video/") {
                video_sessions.serve(
                    request,
                    &path[11..],
                    &store.devices,
                    (mode, include_video),
                    worker_stop.clone(),
                    streams.clone(),
                );
                continue;
            }
            if method == Method::Post && path == "/api/pair" {
                let pair: Pair = match read_json(&mut request) {
                    Ok(p) => p,
                    Err(e) => {
                        reply(request, 400, e, "text/plain");
                        continue;
                    }
                };
                let mut pairing = worker_pairing.lock().unwrap_or_else(|e| e.into_inner());
                if !pairing.accepts(&pair.code) {
                    pairing.failures = pairing.failures.saturating_add(1);
                    drop(pairing);
                    reply(
                        request,
                        403,
                        "Pairing code expired, already used, or incorrect. Choose New pairing QR in desktop Devices and scan again.",
                        "text/plain",
                    );
                    continue;
                }
                let secret = token();
                let digest = hash(&secret);
                let mut grant = selection.clone();
                grant.name = pair.name.chars().take(60).collect();
                store.devices.insert(digest.clone(), grant);
                match persist(&state, &store) {
                    Ok(()) => {
                        pairing.code = None;
                        drop(pairing);
                        reply_headers(
                            request,
                            200,
                            serde_json::json!({"token":secret,"scope":selection.scope,"mode":selection.mode}).to_string(),
                            "application/json; charset=utf-8",
                            &[("Set-Cookie", resume_cookie(&secret))],
                        );
                    }
                    Err(e) => {
                        store.devices.remove(&digest);
                        drop(pairing);
                        reply(request, 500, e, "text/plain");
                    }
                }
                continue;
            }
            let secret = header(&request, "Authorization")
                .and_then(|h| h.strip_prefix("Bearer ").map(str::to_owned))
                .or_else(|| {
                    (method == Method::Post && path == "/api/resume")
                        .then(|| cookie(&request, "gnune_resume"))
                        .flatten()
                });
            let digest = secret.as_deref().map(hash);
            let Some(digest) = digest.filter(|d| store.devices.contains_key(d)) else {
                reply(
                    request,
                    401,
                    "Pair this phone using the desktop share link",
                    "text/plain",
                );
                continue;
            };
            if store.devices[&digest].mode == ShareMode::Library
                && mode != ShareMode::Library
                && !(method == Method::Post && path == "/api/forget")
            {
                reply(
                    request,
                    403,
                    "Full music library sharing is off. Enable it in desktop Devices to reconnect this pairing.",
                    "text/plain",
                );
                continue;
            }
            if method == Method::Post && path == "/api/video-session" {
                video_sessions.create(
                    request,
                    &digest,
                    &store.devices[&digest],
                    (mode, include_video),
                );
                continue;
            }
            if method == Method::Post && path == "/api/resume" {
                let grant = &store.devices[&digest];
                let secret = secret.unwrap();
                reply_headers(
                    request,
                    200,
                    serde_json::json!({"token":secret,"scope":grant.scope,"mode":grant.mode,"last_seq":grant.last_seq})
                        .to_string(),
                    "application/json; charset=utf-8",
                    &[("Set-Cookie", resume_cookie(&secret))],
                );
                continue;
            }
            if method == Method::Get && path == "/api/manifest" {
                let grant = &store.devices[&digest];
                json(request, 200, manifest(grant, include_video));
            } else if method == Method::Get && path.starts_with("/api/art/") {
                if !include_video && video::is_art(&store.devices[&digest], &path[9..]) {
                    reply(request, 403, "Video sharing is off", "text/plain");
                    continue;
                }
                match artwork(&store.devices[&digest], &path[9..]) {
                    Ok(bytes) => reply(request, 200, bytes, "image/png"),
                    Err(error) => reply(request, 404, error, "text/plain"),
                }
            } else if method == Method::Get && path.starts_with("/api/media/") {
                let id = &path[11..];
                let grant = &store.devices[&digest];
                let track = grant.tracks.iter().find(|t| t.id == id);
                match track {
                    Some(track) => match open_allowed(track, &grant.files) {
                        Ok(file) => {
                            let metadata = match file.metadata() {
                                Ok(meta) => shared(track, &meta),
                                Err(e) => {
                                    reply(request, 404, e.to_string(), "text/plain");
                                    continue;
                                }
                            };
                            video::stream(
                                request,
                                file,
                                &metadata.mime,
                                worker_stop.clone(),
                                streams.clone(),
                            );
                        }
                        Err(e) => reply(request, 404, e.to_string(), "text/plain"),
                    },
                    None => reply(
                        request,
                        404,
                        "Track is not in this phone’s shared selection",
                        "text/plain",
                    ),
                }
            } else if method == Method::Post && path == "/api/forget" {
                let original = store.devices.remove(&digest).unwrap();
                match persist(&state, &store) {
                    Ok(()) => reply_headers(
                        request,
                        200,
                        "{\"forgotten\":true}",
                        "application/json",
                        &[("Set-Cookie", resume_cookie(""))],
                    ),
                    Err(error) => {
                        store.devices.insert(digest, original);
                        reply(request, 500, error, "text/plain");
                    }
                }
            } else if method == Method::Post && path == "/api/progress" {
                let progress: Progress = match read_json(&mut request) {
                    Ok(p) => p,
                    Err(e) => {
                        reply(request, 400, e, "text/plain");
                        continue;
                    }
                };
                let original = store.devices[&digest].clone();
                if progress.updates.len() > 100
                    || progress.updates.iter().any(|u| {
                        !u.position.is_finite()
                            || u.position < 0.0
                            || u.position > 31_536_000.0
                            || !original.tracks.iter().any(|t| t.id == u.id)
                    })
                {
                    reply(
                        request,
                        400,
                        "Invalid progress batch or unauthorized track",
                        "text/plain",
                    );
                    continue;
                }
                let grant = store.devices.get_mut(&digest).unwrap();
                let mut callbacks = Vec::new();
                let mut ack = Vec::new();
                let mut updates = progress.updates;
                updates.sort_by_key(|u| u.seq);
                for update in updates {
                    ack.push(update.seq);
                    if update.seq <= grant.last_seq {
                        continue;
                    }
                    let track = grant.tracks.iter().find(|t| t.id == update.id).unwrap();
                    let position = if update.completed {
                        0.0
                    } else if track.duration > 0.0 {
                        update.position.min(track.duration)
                    } else {
                        update.position
                    };
                    grant.positions.insert(update.id, position);
                    grant.last_seq = update.seq;
                    callbacks.push((track.path.clone(), position));
                }
                match persist(&state, &store) {
                    Ok(()) => {
                        for (path, position) in callbacks {
                            callback(path, position);
                        }
                        json(request, 200, serde_json::json!({"ack":ack}));
                    }
                    Err(e) => {
                        store.devices.insert(digest, original);
                        reply(request, 500, e, "text/plain");
                    }
                }
            } else {
                reply(request, 404, "Not found", "text/plain");
            }
        }
    };
    let thread = thread::Builder::new()
        .name("gnune-phone-sync".into())
        .spawn(serve)
        .map_err(|e| e.to_string())?;
    Ok(Session {
        url: session_url,
        mode,
        track_count,
        skipped,
        include_video,
        server,
        stop,
        thread: Some(thread),
        pairing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture_track(dir: &std::path::Path, id: &str, number: u32) -> Track {
        let path = dir.join(format!("{id}.mp3"));
        fs::write(&path, b"fixture audio bytes").unwrap();
        Track {
            id: id.into(),
            path,
            title: id.into(),
            album: "Compilation".into(),
            album_artist: "Various Artists".into(),
            artist: format!("Singer {id}"),
            track_number: Some(number),
            ..Track::default()
        }
    }
    #[test]
    fn library_selection_is_path_free_ordered_and_skips_unavailable_songs() {
        let dir = tempfile::tempdir().unwrap();
        let a = fixture_track(dir.path(), "a", 2);
        let b = fixture_track(dir.path(), "b", 1);
        let missing = Track {
            id: "missing".into(),
            path: dir.path().join("missing.mp3"),
            ..Track::default()
        };
        let playlists = BTreeMap::from([(
            "My playlist".into(),
            vec![
                a.path.clone(),
                b.path.clone(),
                a.path.clone(),
                missing.path.clone(),
            ],
        )]);
        let (grant, skipped) = selection(
            vec![a, b, missing],
            ShareMode::Library,
            &playlists,
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(skipped, 1);
        let manifest = manifest(&grant, false);
        assert_eq!(
            manifest["playlists"][0]["tracks"],
            serde_json::json!(["a", "b", "a"])
        );
        assert_eq!(
            manifest["tracks"][0]["album_id"],
            manifest["tracks"][1]["album_id"]
        );
        assert!(
            !manifest
                .to_string()
                .contains(&dir.path().to_string_lossy().to_string())
        );
        assert_eq!(manifest["videos"], serde_json::json!([]));
        assert_eq!(manifest["mode"], "library");
    }
    #[test]
    fn current_library_refresh_never_elevates_queue_or_video_caps_and_keeps_cache_scope() {
        let dir = tempfile::tempdir().unwrap();
        let a = fixture_track(dir.path(), "a", 1);
        let b = fixture_track(dir.path(), "b", 2);
        let (queue, _) = selection(
            vec![a.clone()],
            ShareMode::Queue,
            &BTreeMap::new(),
            &BTreeMap::new(),
        )
        .unwrap();
        let (library, _) = selection(
            vec![a.clone()],
            ShareMode::Library,
            &BTreeMap::new(),
            &BTreeMap::new(),
        )
        .unwrap();
        let (mut current, _) = selection(
            vec![a, b],
            ShareMode::Library,
            &BTreeMap::new(),
            &BTreeMap::new(),
        )
        .unwrap();
        current.includes_video = true;
        let mut store = Store {
            devices: BTreeMap::from([
                ("queue".into(), queue.clone()),
                ("library".into(), library.clone()),
            ]),
        };
        refresh_library_grants(&mut store, &current);
        assert_eq!(store.devices["queue"].tracks.len(), 1);
        assert_eq!(store.devices["queue"].scope, queue.scope);
        assert_eq!(store.devices["library"].tracks.len(), 2);
        assert_eq!(store.devices["library"].scope, library.scope);
        assert!(!store.devices["library"].includes_video);
        store.devices.get_mut("library").unwrap().includes_video = true;
        current.includes_video = false;
        refresh_library_grants(&mut store, &current);
        assert!(store.devices["library"].includes_video);
        assert!(store.devices["library"].videos.is_empty());
    }
    #[test]
    fn old_saved_grants_default_to_their_original_queue_without_new_permissions() {
        let grant: Grant = serde_json::from_value(serde_json::json!({
            "scope":"original","name":"Phone","tracks":[],"files":{},"last_seq":4,"positions":{}
        }))
        .unwrap();
        assert_eq!(grant.mode, ShareMode::Queue);
        assert!(!grant.includes_video);
        assert!(grant.playlists.is_empty() && grant.videos.is_empty() && grant.artwork.is_empty());
        assert_eq!(grant.last_seq, 4);
    }
    #[test]
    fn resume_cookie_has_a_narrow_secure_path_and_can_be_cleared() {
        let secret = token();
        let cookie = resume_cookie(&secret);
        assert!(cookie.contains("Path=/api/resume; Secure; HttpOnly; SameSite=Strict"));
        assert!(!cookie.contains("Domain="));
        assert!(resume_cookie("").ends_with("Max-Age=0"));
    }
    #[test]
    fn pairing_tokens_expire_are_single_use_and_can_be_renewed() {
        let mut pairing = Pairing::new("first".into());
        assert!(pairing.accepts("first"));
        assert!(!pairing.accepts("wrong"));
        pairing.code = None;
        assert!(!pairing.accepts("first"));
        pairing = Pairing::new("second".into());
        assert!(!pairing.accepts("first"));
        assert!(pairing.accepts("second"));
        pairing.expires = Instant::now() - Duration::from_secs(1);
        assert!(!pairing.accepts("second"));
        pairing = Pairing::new("third".into());
        pairing.failures = 30;
        assert!(!pairing.accepts("third"));
        pairing = Pairing::new("fourth".into());
        assert!(pairing.accepts("fourth"));
    }
    #[test]
    fn secrets_are_random_and_disk_contains_only_digests() {
        let a = token();
        let b = token();
        assert_ne!(a, b);
        assert_eq!(a.len(), 48);
        assert_eq!(hash(&a).len(), 64);
    }
    #[test]
    fn state_roundtrips_without_plaintext_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let secret = token();
        let mut store = Store::default();
        store.devices.insert(
            hash(&secret),
            Grant {
                scope: "selection".into(),
                name: "Phone".into(),
                tracks: vec![],
                files: BTreeMap::new(),
                last_seq: 0,
                positions: BTreeMap::new(),
                ..Grant::default()
            },
        );
        persist(&path, &store).unwrap();
        let raw = fs::read_to_string(path).unwrap();
        assert!(!raw.contains(&secret));
        assert_eq!(
            serde_json::from_str::<Store>(&raw).unwrap().devices.len(),
            1
        );
    }
}
