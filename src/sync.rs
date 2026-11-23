//! Opt-in HTTPS phone companion. Only Pair phone starts new sharing consent.
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
mod companion;
mod reachability;
mod settings;
pub use companion::Change;
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
    callback: impl Fn(Change) + Send + 'static,
) -> Result<Option<Session>, SharingError> {
    let migration_guard = settings::migration_guard().map_err(|message| SharingError {
        message,
        retryable: true,
        waiting: true,
    })?;
    let Some(saved) = settings::load()?.filter(|saved| saved.enabled) else {
        return Ok(None);
    };
    let result = (|| {
        let selected = if saved.mode == ShareMode::Library {
            library_selection(library, saved.include_video, saved.include_podcasts)?
        } else {
            (
                saved
                    .queue
                    .clone()
                    .ok_or("Saved queue sharing selection is missing")?,
                saved.skipped,
            )
        };
        start_configured(
            selected,
            callback,
            settings::connection(Some(&saved))?,
            migration_guard,
        )
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
    pub include_podcasts: bool,
    live: Arc<Mutex<Arc<companion::Snapshot>>>,
    _migration_guard: fs::File,
    server: Arc<Server>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
    sharing: Arc<Mutex<ActiveShare>>,
    remembered: settings::Saved,
    route: reachability::RouteCheck,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairingStatus {
    Ready,
    Checking,
    Unavailable,
    Used,
    Expired,
    Limited,
}
struct ActiveShare {
    selection: Arc<Grant>,
    revision: u64,
    pairing: Pairing,
}
impl ActiveShare {
    fn replace(&mut self, selection: Grant, code: String) {
        self.selection = Arc::new(selection);
        self.revision += 1;
        self.pairing = Pairing::new(code);
    }
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
    fn status(&self) -> PairingStatus {
        if self.failures >= 30 {
            PairingStatus::Limited
        } else if self.code.is_none() {
            PairingStatus::Used
        } else if Instant::now() > self.expires {
            PairingStatus::Expired
        } else {
            PairingStatus::Ready
        }
    }
    fn accepts(&self, code: &str) -> bool {
        self.status() == PairingStatus::Ready && self.code.as_deref() == Some(code)
    }
}
impl Session {
    pub fn publish(&self, library: &crate::state::Library) {
        *self.live.lock().unwrap_or_else(|e| e.into_inner()) =
            Arc::new(companion::Snapshot::new(library));
    }
    pub fn renew_pairing(&mut self) {
        let code = token();
        self.url = format!(
            "{}#pair={code}",
            self.url.split('#').next().unwrap_or(&self.url)
        );
        self.sharing
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pairing = Pairing::new(code);
    }
    pub fn pairing_status(&self) -> PairingStatus {
        let status = self.code_status();
        if status == PairingStatus::Ready && self.can_pair_phone() {
            self.route.check(false)
        } else {
            status
        }
    }
    fn code_status(&self) -> PairingStatus {
        self.sharing
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pairing
            .status()
    }
    /// Navigation can replace a consumed/expired code, but never bypass a lockout.
    pub fn prepare_pairing(&mut self) {
        if matches!(
            self.code_status(),
            PairingStatus::Used | PairingStatus::Expired
        ) {
            self.renew_pairing();
        }
        self.route.check(true);
    }
    pub fn can_pair_phone(&self) -> bool {
        self.remembered.connection.is_tailscale()
            && self.mode == ShareMode::Library
            && self.include_video
            && self.include_podcasts
    }
    pub fn pair_phone(&mut self, library: &crate::state::Library) -> Result<(), String> {
        if !self.remembered.connection.is_tailscale() {
            return Err("Set up Tailscale sharing, then reopen Gnune to pair your phone.".into());
        }
        self.replace_selection(library_selection(library, true, true)?)?;
        self.publish(library);
        Ok(())
    }
    fn replace_selection(&mut self, (selection, skipped): (Grant, usize)) -> Result<(), String> {
        let mut remembered = self.remembered.clone();
        remembered.mode = selection.mode;
        remembered.include_video = selection.includes_video;
        remembered.include_podcasts = selection.includes_podcasts;
        remembered.queue = (selection.mode == ShareMode::Queue).then(|| selection.clone());
        remembered.skipped = skipped;
        // Durable consent must succeed before the active listener changes scope.
        settings::save(&remembered)?;
        let code = token();
        self.mode = selection.mode;
        self.track_count = selection.tracks.len();
        self.include_video = selection.includes_video;
        self.include_podcasts = selection.includes_podcasts;
        self.skipped = skipped;
        let mut sharing = self.sharing.lock().unwrap_or_else(|e| e.into_inner());
        sharing.replace(selection, code.clone());
        self.url = format!(
            "{}#pair={code}",
            self.url.split('#').next().unwrap_or(&self.url)
        );
        self.remembered = remembered;
        Ok(())
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
    #[serde(default)]
    includes_podcasts: bool,
    #[serde(default)]
    podcasts: BTreeMap<String, String>,
    #[serde(default)]
    podcast_episodes: BTreeMap<String, companion::Episode>,
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
        if grant.includes_podcasts && selection.includes_podcasts {
            grant.podcasts = selection.podcasts.clone();
        } else {
            grant.podcasts.clear();
            grant.podcast_episodes.clear();
        }
        grant.tracks = selection.tracks.clone();
        grant.files = selection.files.clone();
        grant.playlists = selection.playlists.clone();
        grant.artwork = selection
            .artwork
            .iter()
            .filter(|(id, _)| grant.includes_video || !video::is_art(selection, id))
            .filter(|(id, _)| grant.includes_podcasts || !companion::is_art(selection, id))
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
// Icons are public install metadata, including Safari's root discovery probes.
// Only these routes ignore query strings; API and media routing stays unchanged.
fn mobile_icon(path: &str) -> Option<(&'static [u8], &'static str)> {
    match path.split('?').next().unwrap_or(path) {
        "/favicon.ico" => Some((include_bytes!("mobile/favicon.ico"), "image/x-icon")),
        "/gnune-touch-icon-180-v1.png"
        | "/apple-touch-icon.png"
        | "/apple-touch-icon-precomposed.png" => Some((
            include_bytes!("mobile/gnune-touch-icon-180-v1.png"),
            "image/png",
        )),
        "/icon-192.png" => Some((include_bytes!("mobile/icon-192.png"), "image/png")),
        "/icon-512.png" => Some((include_bytes!("mobile/icon-512.png"), "image/png")),
        "/icon.svg" => Some((include_bytes!("mobile/icon.svg"), "image/svg+xml")),
        _ => None,
    }
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
        ..Grant::default()
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

const TAILSCALE_SETUP: &str = "Close Gnune, then preview setup from its source folder: python3 scripts/gnune-tailscale-setup.py. Follow README Phone collection and playback to apply setup, then reopen Gnune.";

/// Read-only check. This never changes the network or opens a listener.
pub fn pairing_setup() -> Result<(), String> {
    let saved = settings::load()?;
    let connection =
        settings::connection(saved.as_ref()).map_err(|_| TAILSCALE_SETUP.to_owned())?;
    if !connection.is_tailscale() {
        return Err(TAILSCALE_SETUP.into());
    }
    Ok(())
}
/// Only the explicit Pair phone action starts or enlarges sharing consent.
pub fn pair_phone(
    library: &crate::state::Library,
    callback: impl Fn(Change) + Send + 'static,
) -> Result<Session, SharingError> {
    let migration_guard = settings::migration_guard()?;
    let saved = settings::load()?;
    let connection = settings::connection(saved.as_ref())?;
    if !connection.is_tailscale() {
        return Err(TAILSCALE_SETUP.into());
    }
    let session = start_configured(
        library_selection(library, true, true)?,
        callback,
        connection,
        migration_guard,
    )?;
    session.publish(library);
    Ok(session)
}
fn library_selection(
    library: &crate::state::Library,
    include_videos: bool,
    include_podcasts: bool,
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
    if include_podcasts {
        grant.includes_podcasts = true;
        companion::include(&mut grant, library);
        grant.podcasts = library
            .feeds
            .iter()
            .filter_map(|feed| crate::podcasts::normalize_url(feed).ok())
            .map(|feed| (hash(&format!("podcast-show/{feed}")), feed))
            .collect();
    }
    Ok((grant, skipped))
}

fn start_configured(
    (selection, skipped): (Grant, usize),
    callback: impl Fn(Change) + Send + 'static,
    connection: settings::Connection,
    migration_guard: fs::File,
) -> Result<Session, SharingError> {
    let proxied = connection.serve_socket.is_some();
    let pairing_transport = connection.is_tailscale();
    let tls = if proxied {
        None
    } else {
        Some(settings::tls(&connection)?)
    };
    let origin = connection.origin.clone();
    let mode = selection.mode;
    let track_count = selection.tracks.len();
    let include_video = selection.includes_video;
    let include_podcasts = selection.includes_podcasts;
    let live = Arc::new(Mutex::new(Arc::new(companion::Snapshot::default())));
    let worker_live = live.clone();
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
        include_podcasts,
        queue: (mode == ShareMode::Queue).then(|| selection.clone()),
        skipped,
    };
    let listener = if let Some(path) = &connection.serve_socket {
        settings::unix_server(path)
            .map_err(|e| Box::<dyn std::error::Error + Send + Sync>::from(std::io::Error::other(e)))
    } else {
        let (cert, key) = tls.expect("native TLS configuration");
        Server::https(
            connection.bind,
            SslConfig {
                certificate: cert,
                private_key: key,
            },
        )
    };
    let server = match listener {
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
                message: if retryable {
                    format!(
                        "This PC does not currently have the saved sharing address {}. A router DHCP reservation preserves the existing phone icon. For stable Tailscale access, close Gnune and preview scripts/gnune-tailscale-setup.py on the host; a new hostname needs one new phone pairing.",
                        connection.bind.ip()
                    )
                } else {
                    format!(
                        "Phone sharing could not start at {}: {error}",
                        connection.bind
                    )
                },
                retryable,
                waiting: retryable,
            });
        }
    };
    let stop = Arc::new(AtomicBool::new(false));
    let code = token();
    let session_url = format!("{origin}/#pair={code}");
    let sharing = Arc::new(Mutex::new(ActiveShare {
        selection: Arc::new(selection),
        revision: 0,
        pairing: Pairing::new(code),
    }));
    let worker_share = sharing.clone();
    let route = reachability::RouteCheck::new(&origin);
    let route_identity = route.identity.clone();
    let worker_server = server.clone();
    let worker_stop = stop.clone();
    let serve = move || {
        let mut video_sessions = video::Sessions::default();
        let mut podcast_sessions = companion::Sessions::default();
        let streams = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut revision = 0;
        while !worker_stop.load(Ordering::Acquire) {
            let mut request = match worker_server.recv_timeout(Duration::from_millis(200)) {
                Ok(Some(r)) => r,
                Ok(None) => continue,
                Err(_) => break,
            };
            if worker_stop.load(Ordering::Acquire) {
                break;
            }
            let (current_revision, selection) = {
                let sharing = worker_share.lock().unwrap_or_else(|e| e.into_inner());
                (sharing.revision, sharing.selection.clone())
            };
            if current_revision != revision {
                refresh_library_grants(&mut store, &selection);
                // Retry a failed write before serving or issuing a grant for the new scope.
                if let Err(error) = persist(&state, &store) {
                    reply(request, 500, error, "text/plain");
                    continue;
                }
                revision = current_revision;
            }
            let mode = selection.mode;
            let include_video = selection.includes_video;
            let include_podcasts = selection.includes_podcasts;
            companion::refresh_grants(
                &mut store.devices,
                &worker_live.lock().unwrap_or_else(|e| e.into_inner()),
                mode,
                include_podcasts,
            );
            let path = request.url().to_owned();
            let method = request.method().clone();
            if !valid_authority(&request, &origin, proxied) {
                reply(
                    request,
                    421,
                    "Use the configured Gnune HTTPS origin",
                    "text/plain",
                );
                continue;
            }
            if pairing_transport && method == Method::Get && path == reachability::PATH {
                reply(request, 200, route_identity.as_bytes(), "text/plain");
                continue;
            }
            if matches!(method, Method::Get | Method::Head)
                && let Some((body, mime)) = mobile_icon(&path)
            {
                reply(request, 200, body, mime);
                continue;
            }
            if method == Method::Get {
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
            if matches!(method, Method::Get | Method::Head)
                && path.starts_with("/api/podcast-media/")
            {
                podcast_sessions.serve(
                    request,
                    &path[19..],
                    &store.devices,
                    (mode, include_podcasts),
                    worker_stop.clone(),
                    streams.clone(),
                );
                continue;
            }
            if method == Method::Post && path == "/api/pair" {
                if !pairing_transport
                    || mode != ShareMode::Library
                    || !include_video
                    || !include_podcasts
                {
                    reply(
                        request,
                        403,
                        "Choose Pair phone in desktop Devices to share the full collection over Tailscale.",
                        "text/plain",
                    );
                    continue;
                }
                let pair: Pair = match read_json(&mut request) {
                    Ok(p) => p,
                    Err(e) => {
                        reply(request, 400, e, "text/plain");
                        continue;
                    }
                };
                let mut sharing = worker_share.lock().unwrap_or_else(|e| e.into_inner());
                if sharing.revision != current_revision {
                    reply(
                        request,
                        409,
                        "Sharing changed. Scan the current desktop QR again.",
                        "text/plain",
                    );
                    continue;
                }
                let pairing = &mut sharing.pairing;
                if !pairing.accepts(&pair.code) {
                    pairing.failures = pairing.failures.saturating_add(1);
                    drop(sharing);
                    reply(
                        request,
                        403,
                        "Pairing code expired, already used, or incorrect. Choose Pair phone in desktop Devices and scan again.",
                        "text/plain",
                    );
                    continue;
                }
                let secret = token();
                let digest = hash(&secret);
                let mut grant = (*selection).clone();
                grant.name = pair.name.chars().take(60).collect();
                store.devices.insert(digest.clone(), grant);
                match persist(&state, &store) {
                    Ok(()) => {
                        pairing.code = None;
                        drop(sharing);
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
                        drop(sharing);
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
            if companion::route(&path) {
                let snapshot = worker_live
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                companion::serve(
                    request,
                    &path,
                    &digest,
                    store.devices.get_mut(&digest).unwrap(),
                    (mode, include_podcasts),
                    &snapshot,
                    &callback,
                    &mut podcast_sessions,
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
                            callback(Change::legacy(path, position));
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
        include_podcasts,
        live,
        _migration_guard: migration_guard,
        server,
        stop,
        thread: Some(thread),
        sharing,
        remembered,
        route,
    })
}

fn valid_authority(request: &Request, origin: &str, proxied: bool) -> bool {
    let expected = origin.strip_prefix("https://");
    if proxied {
        header(request, "Host").as_deref() == Some("localhost")
            && header(request, "X-Forwarded-Host").as_deref() == expected
            && header(request, "X-Forwarded-Proto").as_deref() == Some("https")
            && header(request, "Tailscale-Funnel-Request").is_none()
    } else {
        header(request, "Host").as_deref() == expected
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_phone_can_pair_before_the_first_import() {
        let (grant, skipped) =
            library_selection(&crate::state::Library::default(), true, true).unwrap();
        assert_eq!(grant.mode, ShareMode::Library);
        assert!(grant.includes_video && grant.includes_podcasts);
        assert_eq!(skipped, 0);
    }
    #[test]
    fn public_install_icons_accept_discovery_aliases_without_exposing_api_routes() {
        let (touch, mime) = mobile_icon("/gnune-touch-icon-180-v1.png").unwrap();
        assert_eq!(mime, "image/png");
        assert_eq!(&touch[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(u32::from_be_bytes(touch[16..20].try_into().unwrap()), 180);
        assert_eq!(u32::from_be_bytes(touch[20..24].try_into().unwrap()), 180);
        for path in [
            "/apple-touch-icon.png",
            "/apple-touch-icon-precomposed.png?probe=1",
        ] {
            assert_eq!(mobile_icon(path), Some((touch, mime)));
        }
        assert_eq!(mobile_icon("/favicon.ico?v=1").unwrap().1, "image/x-icon");
        for path in [
            "/api/manifest",
            "/api/resume?token=example",
            "/audio/track",
            "/",
        ] {
            assert!(mobile_icon(path).is_none());
        }
    }
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
        current.includes_podcasts = true;
        current
            .podcasts
            .insert("new-show".into(), "https://example.com/feed.xml".into());
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
        assert!(!store.devices["library"].includes_podcasts);
        assert!(store.devices["library"].podcasts.is_empty());
        assert!(!store.devices["queue"].includes_podcasts);
        assert!(store.devices["queue"].podcasts.is_empty());
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
    fn new_pairings_require_private_tailscale_https_instead_of_legacy_lan() {
        let mut connection = settings::Connection {
            bind: "127.0.0.1:443".parse().unwrap(),
            origin: "https://pc.example.ts.net".into(),
            cert: PathBuf::new(),
            key: PathBuf::new(),
            serve_socket: Some(PathBuf::from("/tmp/gnune-private/serve.sock")),
        };
        assert!(connection.is_tailscale());
        for origin in [
            "https://192.0.2.1:8443",
            "https://gnune.local",
            "http://pc.example.ts.net",
            "https://pc.example.ts.net.evil.example",
        ] {
            connection.origin = origin.into();
            assert!(!connection.is_tailscale());
        }
        connection.origin = "https://pc.example.ts.net".into();
        connection.serve_socket = None;
        assert!(!connection.is_tailscale());
    }
    #[test]
    fn explicit_repair_replaces_the_selection_and_code_together() {
        let mut sharing = ActiveShare {
            selection: Arc::new(Grant {
                scope: "old-queue".into(),
                ..Grant::default()
            }),
            revision: 0,
            pairing: Pairing::new("old-code".into()),
        };
        sharing.replace(
            Grant {
                scope: "full-collection".into(),
                mode: ShareMode::Library,
                includes_video: true,
                includes_podcasts: true,
                ..Grant::default()
            },
            "new-code".into(),
        );
        assert_eq!(sharing.revision, 1);
        assert_eq!(sharing.selection.scope, "full-collection");
        assert_eq!(sharing.selection.mode, ShareMode::Library);
        assert!(sharing.selection.includes_video && sharing.selection.includes_podcasts);
        assert!(!sharing.pairing.accepts("old-code"));
        assert!(sharing.pairing.accepts("new-code"));
        sharing.replace((*sharing.selection).clone(), "another-code".into());
        assert_eq!(sharing.revision, 2);
        assert!(!sharing.pairing.accepts("new-code"));
        assert!(sharing.pairing.accepts("another-code"));
    }
    #[test]
    fn pairing_tokens_expire_are_single_use_and_can_be_renewed() {
        let mut pairing = Pairing::new("first".into());
        assert_eq!(pairing.status(), PairingStatus::Ready);
        assert!(pairing.accepts("first"));
        assert!(!pairing.accepts("wrong"));
        pairing.code = None;
        assert_eq!(pairing.status(), PairingStatus::Used);
        assert!(!pairing.accepts("first"));
        pairing = Pairing::new("second".into());
        assert!(!pairing.accepts("first"));
        assert!(pairing.accepts("second"));
        pairing.expires = Instant::now() - Duration::from_secs(1);
        assert_eq!(pairing.status(), PairingStatus::Expired);
        assert!(!pairing.accepts("second"));
        pairing = Pairing::new("third".into());
        pairing.failures = 30;
        assert_eq!(pairing.status(), PairingStatus::Limited);
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

#[cfg(test)]
mod proxy_tests {
    use super::*;
    fn request(headers: &[(&str, &str)]) -> Request {
        headers
            .iter()
            .fold(tiny_http::TestRequest::new(), |request, (name, value)| {
                request.with_header(Header::from_bytes(*name, *value).unwrap())
            })
            .into()
    }
    #[test]
    fn private_serve_requires_exact_forwarded_origin_https_and_never_funnel() {
        let origin = "https://fixture.example.ts.net:8443";
        let good = [
            ("Host", "localhost"),
            ("X-Forwarded-Host", "fixture.example.ts.net:8443"),
            ("X-Forwarded-Proto", "https"),
        ];
        assert!(valid_authority(&request(&good), origin, true));
        assert!(!valid_authority(&request(&good), origin, false));
        assert!(!valid_authority(&request(&good[..2]), origin, true));
        let mut wrong = good.to_vec();
        wrong[1].1 = "other.example.ts.net:8443";
        assert!(!valid_authority(&request(&wrong), origin, true));
        wrong = good.to_vec();
        wrong[2].1 = "http";
        assert!(!valid_authority(&request(&wrong), origin, true));
        wrong = good.to_vec();
        wrong.push(("Tailscale-Funnel-Request", "?1"));
        assert!(!valid_authority(&request(&wrong), origin, true));
        assert!(valid_authority(
            &request(&[("Host", "fixture.example.ts.net:8443")]),
            origin,
            false
        ));
    }
}
