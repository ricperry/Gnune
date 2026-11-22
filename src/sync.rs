//! Opt-in local HTTPS phone companion. No listener exists until `start` runs.
use gnune::library::Track;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    net::SocketAddr,
    os::unix::fs::MetadataExt,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant, UNIX_EPOCH},
};
use tiny_http::{Header, Method, Request, Response, Server, SslConfig};

pub struct Session {
    pub url: String,
    server: Arc<Server>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
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
    duration: f64,
    bytes: u64,
    version: String,
    mime: String,
}
#[derive(Clone, Serialize, Deserialize)]
struct Grant {
    scope: String,
    name: String,
    tracks: Vec<Track>,
    files: BTreeMap<String, FileIdentity>,
    last_seq: u64,
    positions: BTreeMap<String, f64>,
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
    let mut response = Response::from_data(body.into()).with_status_code(status);
    for (name, value) in [
        ("Content-Type", mime),
        ("Cache-Control", "no-store"),
        ("X-Content-Type-Options", "nosniff"),
        ("Referrer-Policy", "no-referrer"),
        (
            "Content-Security-Policy",
            "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; media-src 'self' blob:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'",
        ),
    ] {
        response.add_header(Header::from_bytes(name, value).expect("static header"));
    }
    let _ = request.respond(response);
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
        duration: track.duration,
        bytes: meta.len(),
        version: format!("{}-{modified}", meta.len()),
        mime: mime.into(),
    }
}

/// The caller must invoke this only after an explicit Share action. PEM material
/// is supplied by the user; this module never changes certificate trust/firewalls.
pub fn start(
    tracks: Vec<Track>,
    callback: impl Fn(PathBuf, f64) + Send + 'static,
) -> Result<Session, String> {
    if tracks.is_empty() {
        return Err("Choose tracks to share first".into());
    }
    let bind: SocketAddr = std::env::var("GNUNE_SYNC_BIND")
        .map_err(|_| "Set GNUNE_SYNC_BIND, e.g. 192.168.1.20:8443")?
        .parse()
        .map_err(|_| "GNUNE_SYNC_BIND must be an IP address and port")?;
    if bind.ip().is_unspecified() {
        return Err(
            "GNUNE_SYNC_BIND must name a specific local interface, not 0.0.0.0 or ::".into(),
        );
    }
    let origin_text = std::env::var("GNUNE_SYNC_ORIGIN")
        .map_err(|_| "Set GNUNE_SYNC_ORIGIN to your stable trusted HTTPS origin")?;
    let origin = url::Url::parse(&origin_text).map_err(|e| e.to_string())?;
    if origin.scheme() != "https"
        || origin.host_str().is_none()
        || origin.path() != "/"
        || origin.query().is_some()
        || origin.fragment().is_some()
        || !origin.username().is_empty()
        || origin.password().is_some()
        || origin.port_or_known_default() != Some(bind.port())
    {
        return Err(
            "GNUNE_SYNC_ORIGIN must be an HTTPS origin with the same port as GNUNE_SYNC_BIND"
                .into(),
        );
    }
    let origin = origin.origin().ascii_serialization();
    let cert = fs::read(
        std::env::var("GNUNE_SYNC_CERT")
            .map_err(|_| "Set GNUNE_SYNC_CERT to the trusted PEM certificate chain")?,
    )
    .map_err(|e| e.to_string())?;
    let key = fs::read(
        std::env::var("GNUNE_SYNC_KEY").map_err(|_| "Set GNUNE_SYNC_KEY to its private PEM key")?,
    )
    .map_err(|e| e.to_string())?;
    let mut files = BTreeMap::new();
    let mut selected = Vec::new();
    for track in &tracks {
        let file = fs::File::open(&track.path).map_err(|e| e.to_string())?;
        let meta = file.metadata().map_err(|e| e.to_string())?;
        if !meta.is_file() {
            return Err("Share only regular audio files".into());
        }
        files.insert(track.id.clone(), identity(&meta));
        selected.push(shared(track, &meta));
    }
    let scope = hash(&serde_json::to_string(&selected).map_err(|e| e.to_string())?);
    let state = state_path()?;
    let mut store: Store = match fs::read(&state) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| format!("Cannot read saved phone pairings: {e}"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Store::default(),
        Err(e) => return Err(e.to_string()),
    };
    // Confirm durable storage works before opening a network listener.
    persist(&state, &store)?;
    let server = Arc::new(
        Server::https(
            bind,
            SslConfig {
                certificate: cert,
                private_key: key,
            },
        )
        .map_err(|e| e.to_string())?,
    );
    let stop = Arc::new(AtomicBool::new(false));
    let pairing = token();
    let session_url = format!("{origin}/#pair={pairing}");
    let worker_server = server.clone();
    let worker_stop = stop.clone();
    let serve = move || {
        let mut unused_pairing = Some(pairing);
        let expires = Instant::now() + Duration::from_secs(600);
        let mut bad_pairs = 0_u32;
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
                    "/sw.js" => Some((
                        include_str!("mobile/sw.js"),
                        "text/javascript; charset=utf-8",
                    )),
                    "/style.css" => {
                        Some((include_str!("mobile/style.css"), "text/css; charset=utf-8"))
                    }
                    "/icon.svg" => Some((include_str!("mobile/icon.svg"), "image/svg+xml")),
                    "/manifest.webmanifest" => Some((
                        r##"{"name":"Gnune on your phone","short_name":"Gnune","id":"/","start_url":"/","scope":"/","display":"standalone","background_color":"#faf9fa","theme_color":"#df007b","icons":[{"src":"/icon-192.png","sizes":"192x192","type":"image/png","purpose":"any"},{"src":"/icon-512.png","sizes":"512x512","type":"image/png","purpose":"any"}]}"##,
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
            if method == Method::Post && path == "/api/pair" {
                let pair: Pair = match read_json(&mut request) {
                    Ok(p) => p,
                    Err(e) => {
                        reply(request, 400, e, "text/plain");
                        continue;
                    }
                };
                if bad_pairs >= 30
                    || Instant::now() > expires
                    || unused_pairing.as_deref() != Some(pair.code.as_str())
                {
                    bad_pairs += 1;
                    reply(
                        request,
                        403,
                        "Pairing code expired, already used, or incorrect. Share again on your desktop.",
                        "text/plain",
                    );
                    continue;
                }
                let secret = token();
                let digest = hash(&secret);
                store.devices.insert(
                    digest.clone(),
                    Grant {
                        scope: scope.clone(),
                        name: pair.name.chars().take(60).collect(),
                        tracks: tracks.clone(),
                        files: files.clone(),
                        last_seq: 0,
                        positions: BTreeMap::new(),
                    },
                );
                match persist(&state, &store) {
                    Ok(()) => {
                        unused_pairing = None;
                        json(
                            request,
                            200,
                            serde_json::json!({"token":secret,"scope":scope}),
                        );
                    }
                    Err(e) => {
                        store.devices.remove(&digest);
                        reply(request, 500, e, "text/plain");
                    }
                }
                continue;
            }
            let digest =
                header(&request, "Authorization").and_then(|h| h.strip_prefix("Bearer ").map(hash));
            let Some(digest) = digest.filter(|d| store.devices.contains_key(d)) else {
                reply(
                    request,
                    401,
                    "Pair this phone using the desktop share link",
                    "text/plain",
                );
                continue;
            };
            if method == Method::Get && path == "/api/manifest" {
                let grant = &store.devices[&digest];
                match grant
                    .tracks
                    .iter()
                    .map(|track| {
                        open_allowed(track, &grant.files).and_then(|file| {
                            file.metadata()
                                .map(|meta| shared(track, &meta))
                                .map_err(|e| e.to_string())
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()
                {
                    Ok(tracks) => json(
                        request,
                        200,
                        serde_json::json!({"scope":grant.scope,"tracks":tracks}),
                    ),
                    Err(e) => reply(request, 404, e, "text/plain"),
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
                            let mut response = Response::from_file(file);
                            response.add_header(
                                Header::from_bytes("Content-Type", metadata.mime.as_str()).unwrap(),
                            );
                            response.add_header(
                                Header::from_bytes("Cache-Control", "no-store").unwrap(),
                            );
                            let _ = request.respond(response);
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
                    Ok(()) => json(request, 200, serde_json::json!({"forgotten":true})),
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
        server,
        stop,
        thread: Some(thread),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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
