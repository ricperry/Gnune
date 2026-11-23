//! Small live state, capability-filtered podcast metadata and on-demand media.
use super::*;
use crate::{
    podcasts::{EpisodeId, EpisodeOrder, EpisodeProgress},
    state::Library,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    io::{BufRead, BufReader},
    process::{Child, ChildStdout, Command, Stdio},
    sync::{atomic::AtomicUsize, mpsc},
};

#[derive(Clone, Default)]
pub(super) struct Snapshot {
    pins: Vec<PathBuf>,
    history: Vec<PathBuf>,
    added: Vec<PathBuf>,
    progress: BTreeMap<PathBuf, f64>,
    feeds: Vec<String>,
    shows: BTreeMap<String, crate::media::PodcastShow>,
    orders: BTreeMap<String, EpisodeOrder>,
    podcast_progress: BTreeMap<String, BTreeMap<String, EpisodeProgress>>,
    last: Option<EpisodeId>,
}
impl Snapshot {
    pub(super) fn new(db: &Library) -> Self {
        Self {
            pins: db.pins.clone(),
            history: db.history.iter().take(128).cloned().collect(),
            added: db.added.iter().take(128).cloned().collect(),
            progress: db.progress.clone(),
            feeds: db.feeds.clone(),
            shows: db.podcast_shows.clone(),
            orders: db.podcast_orders.clone(),
            podcast_progress: db.podcast_progress.clone(),
            last: db.last_podcast.clone(),
        }
    }
    fn episode_progress(&self, id: &EpisodeId) -> EpisodeProgress {
        self.podcast_progress
            .get(&id.feed_url)
            .and_then(|p| p.get(&id.episode_url))
            .copied()
            .unwrap_or_default()
    }
}
fn revision(value: &impl Serialize) -> String {
    hash(&serde_json::to_string(value).expect("serializable state"))
}
fn progress_value(position: f64, completed: bool) -> Value {
    json!({"position":position,"completed":completed,"revision":revision(&(position,completed))})
}

type ChangeResult = Result<(), String>;
type ChangeReply = Option<mpsc::SyncSender<ChangeResult>>;
pub struct Change {
    action: Action,
    reply: Option<mpsc::SyncSender<Result<(), String>>>,
}
enum Action {
    Legacy(PathBuf, f64),
    Progress {
        path: Option<PathBuf>,
        episode: Option<EpisodeId>,
        position: f64,
        completed: bool,
        base: String,
        history: bool,
        last_base: String,
    },
    Pin {
        album: String,
        pinned: bool,
    },
    Order {
        feed: String,
        order: EpisodeOrder,
    },
}
impl Change {
    pub fn order_feed(&self) -> Option<String> {
        match &self.action {
            Action::Order { feed, .. } => Some(feed.clone()),
            _ => None,
        }
    }
    pub fn reject(self, message: &str) {
        if let Some(reply) = self.reply {
            let _ = reply.send(Err(message.into()));
        }
    }
    pub(super) fn legacy(path: PathBuf, position: f64) -> Self {
        Self {
            action: Action::Legacy(path, position),
            reply: None,
        }
    }
    /// Runs on the GTK thread, checks the current canonical state again and saves
    /// before acknowledging. Incoming progress never seeks a running pipeline.
    pub fn apply(self, db: &mut Library) -> (ChangeResult, ChangeReply) {
        let result = (|| {
            match self.action {
                Action::Legacy(path, position) => {
                    db.phone_progress.insert(path, position);
                }
                Action::Pin { album, pinned } => {
                    let paths: Vec<_> = db
                        .tracks
                        .iter()
                        .filter(|t| hash(&crate::state::album_key(t)) == album)
                        .map(|t| t.path.clone())
                        .collect();
                    db.pins.retain(|p| !paths.contains(p));
                    if pinned && let Some(path) = paths.first() {
                        crate::state::promote(&mut db.pins, path);
                    }
                }
                Action::Order { feed, order } => {
                    db.podcast_orders.insert(feed, order);
                }
                Action::Progress {
                    path,
                    episode,
                    position,
                    completed,
                    base,
                    history,
                    last_base,
                } => {
                    let old = if let Some(id) = &episode {
                        db.podcast_progress
                            .get(&id.feed_url)
                            .and_then(|p| p.get(&id.episode_url))
                            .copied()
                            .unwrap_or_default()
                    } else {
                        EpisodeProgress {
                            position_secs: path
                                .as_ref()
                                .and_then(|p| db.progress.get(p))
                                .copied()
                                .unwrap_or(0.),
                            completed: false,
                        }
                    };
                    if revision(&(old.position_secs, old.completed)) != base {
                        return Err(
                            "Progress changed on another device. Refresh before resuming.".into(),
                        );
                    }
                    if let Some(id) = episode {
                        let last = db.last_podcast.clone();
                        // Only a new play action can make a different show the most recent.
                        let preserve = last.as_ref() != Some(&id)
                            && (!history || revision(&last) != last_base);
                        db.record_podcast_progress(&id, position, completed);
                        if preserve {
                            db.last_podcast = last;
                        }
                    }
                    if let Some(path) = path {
                        db.progress
                            .insert(path.clone(), if completed { 0. } else { position });
                        db.phone_progress.insert(path.clone(), position);
                        if history {
                            crate::state::promote(&mut db.history, &path);
                        }
                    }
                }
            }
            Ok(())
        })();
        (result, self.reply)
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Episode {
    feed: String,
    url: String,
    title: String,
    duration: f64,
}
fn show_id(feed: &str) -> String {
    hash(&format!("podcast-show/{feed}"))
}
fn episode_id(feed: &str, url: &str) -> String {
    hash(&format!("podcast-episode/{feed}/{url}"))
}
fn permitted(grant: &Grant, mode: ShareMode, enabled: bool) -> bool {
    mode == ShareMode::Library && grant.mode == mode && enabled && grant.includes_podcasts
}
fn allowed_feeds(grant: &Grant, snapshot: &Snapshot) -> BTreeMap<String, String> {
    snapshot
        .feeds
        .iter()
        .filter_map(|f| crate::podcasts::normalize_url(f).ok())
        .filter(|_| grant.includes_podcasts)
        .map(|f| (show_id(&f), f))
        .collect()
}
fn albums(paths: &[PathBuf], grant: &Grant, limit: usize) -> Vec<Value> {
    let mut seen = BTreeSet::new();
    paths
        .iter()
        .filter_map(|p| grant.tracks.iter().find(|t| &t.path == p))
        .filter_map(|t| {
            let id = hash(&crate::state::album_key(t));
            seen.insert(id.clone())
                .then(|| json!({"id":id,"track":t.id}))
        })
        .take(limit)
        .collect()
}
fn snapshot(grant: &Grant, state: &Snapshot, enabled: bool) -> Value {
    let feeds = if enabled {
        allowed_feeds(grant, state)
    } else {
        BTreeMap::new()
    };
    let shows: Vec<_> = feeds.iter().map(|(id,feed)| {
        let show=state.shows.get(feed).cloned().unwrap_or_default();
        let art=art_id(id);
        json!({"id":id,"title":if show.title.is_empty(){"Podcast"}else{&show.title},"publisher":show.publisher,"summary":show.summary,"order":state.orders.get(feed).copied().unwrap_or_default(),"art":grant.artwork.contains_key(&art).then_some(art)})
    }).collect();
    let positions: BTreeMap<_, _> = BTreeMap::<String, Value>::new();
    let last=state.last.as_ref().filter(|id|feeds.contains_key(&show_id(&id.feed_url))).map(|id|json!({"show":show_id(&id.feed_url),"episode":episode_id(&id.feed_url,&id.episode_url)}));
    json!({"pins":albums(&state.pins,grant,5),"new":albums(&state.added,grant,7),"history":albums(&state.history,grant,4),"progress":positions,"podcasts":shows,"last_podcast":last,"last_revision":revision(&state.last),"includes_podcasts":enabled})
}

pub(super) fn route(path: &str) -> bool {
    path == "/api/quickplay"
        || path == "/api/state"
        || path == "/api/podcast-session"
        || path.starts_with("/api/podcast-show/")
        || path.starts_with("/api/media-state/")
}
#[derive(Deserialize)]
struct Mutation {
    action: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    position: f64,
    #[serde(default)]
    completed: bool,
    #[serde(default)]
    base: String,
    #[serde(default)]
    pinned: bool,
    #[serde(default)]
    order: EpisodeOrder,
    #[serde(default)]
    history: bool,
    #[serde(default)]
    last_base: String,
}
#[allow(clippy::too_many_arguments)]
pub(super) fn serve(
    mut request: Request,
    path: &str,
    digest: &str,
    grant: &mut Grant,
    (mode, enabled): (ShareMode, bool),
    state: &Snapshot,
    callback: &impl Fn(Change),
    sessions: &mut Sessions,
) {
    let enabled = permitted(grant, mode, enabled);
    if let Some(id) = path.strip_prefix("/api/media-state/") {
        if request.method() != &Method::Get {
            reply(request, 405, "Use GET", "text/plain");
            return;
        }
        if let Some(track) = grant.tracks.iter().find(|t| t.id == id) {
            json(
                request,
                200,
                progress_value(
                    state.progress.get(&track.path).copied().unwrap_or(0.),
                    false,
                ),
            );
            return;
        }
        if let Some(video) = grant.videos.get(id) {
            json(
                request,
                200,
                progress_value(
                    state.progress.get(&video.path).copied().unwrap_or(0.),
                    false,
                ),
            );
            return;
        }
        if let Some(ep) = grant
            .podcast_episodes
            .get(id)
            .filter(|ep| enabled && allowed_feeds(grant, state).contains_key(&show_id(&ep.feed)))
        {
            let saved = state.episode_progress(&EpisodeId {
                feed_url: ep.feed.clone(),
                episode_url: ep.url.clone(),
            });
            json(
                request,
                200,
                progress_value(saved.position_secs, saved.completed),
            );
            return;
        }
        reply(request, 403, "Media not shared", "text/plain");
        return;
    }
    if path == "/api/quickplay" && request.method() == &Method::Get {
        json(request, 200, snapshot(grant, state, enabled));
        return;
    }
    if let Some(id) = path.strip_prefix("/api/podcast-show/") {
        if request.method() != &Method::Get {
            reply(request, 405, "Use GET", "text/plain");
            return;
        }
        if !enabled {
            reply(
                request,
                403,
                "Podcast access is unavailable for this pairing. Choose Pair phone in desktop Devices and pair again with the new Tailscale QR.",
                "text/plain",
            );
            return;
        }
        let Some(feed) = allowed_feeds(grant, state).get(id).cloned() else {
            reply(request, 404, "Podcast is not shared", "text/plain");
            return;
        };
        if let Some(show) = state.shows.get(&feed) {
            include_art(grant, &feed, show);
        }
        let cached = crate::media::cached_podcast(&feed);
        let cached_path = crate::media::podcast_cache_dir()
            .join(format!("{}.json", crate::enrichment::key(&[&feed])));
        let fresh = fs::metadata(cached_path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age < Duration::from_secs(300));
        let parsed = if fresh {
            cached.ok_or_else(|| "Podcast metadata unavailable".into())
        } else {
            crate::media::podcast(&feed, &crate::media::podcast_cache_dir())
                .or_else(|error| cached.ok_or(error))
        };
        match parsed {
            Ok(podcast) => {
                let episodes = crate::podcasts::ordered_episodes(
                    &podcast.episodes,
                    state.orders.get(&feed).copied().unwrap_or_default(),
                );
                let art = art_id(&show_id(&feed));
                let art = grant.artwork.contains_key(&art).then_some(art);
                let rows: Vec<_> = episodes.iter().take(2000).map(|episode| {
                    let id = episode_id(&feed, &episode.url);
                    let identity = EpisodeId { feed_url: feed.clone(), episode_url: episode.url.clone() };
                    let saved = state.episode_progress(&identity);
                    grant.podcast_episodes.insert(id.clone(), Episode {
                        feed: feed.clone(), url: episode.url.clone(),
                        title: episode.title.clone(), duration: 0.,
                    });
                    json!({
                        "id": id, "show": show_id(&feed), "title": episode.title,
                        "summary": episode.summary, "art": art, "published": episode.published,
                        "artist": podcast.show.publisher, "album": podcast.show.title,
                        "duration": 0, "progress": progress_value(saved.position_secs, saved.completed)
                    })
                }).collect();
                json(
                    request,
                    200,
                    json!({"id":id,"order":state.orders.get(&feed).copied().unwrap_or_default(),"episodes":rows}),
                );
            }
            Err(e) => reply(request, 502, e, "text/plain"),
        }
        return;
    }
    if request.method() != &Method::Post {
        reply(request, 405, "Use POST", "text/plain");
        return;
    }
    let change: Mutation = match read_json(&mut request) {
        Ok(v) => v,
        Err(e) => {
            reply(request, 400, e, "text/plain");
            return;
        }
    };
    if path == "/api/podcast-session" {
        if !enabled
            || !grant
                .podcast_episodes
                .get(&change.id)
                .is_some_and(|e| allowed_feeds(grant, state).contains_key(&show_id(&e.feed)))
        {
            reply(
                request,
                403,
                "Open a shared podcast before playing an episode",
                "text/plain",
            );
            return;
        }
        sessions.create(request, digest, &change.id);
        return;
    }
    let action = match change.action.as_str() {
        "pin"
            if grant.mode == ShareMode::Library
                && grant
                    .tracks
                    .iter()
                    .any(|t| hash(&crate::state::album_key(t)) == change.id) =>
        {
            Action::Pin {
                album: change.id,
                pinned: change.pinned,
            }
        }
        "order" if enabled => {
            let Some(feed) = allowed_feeds(grant, state).get(&change.id).cloned() else {
                reply(request, 403, "Podcast not shared", "text/plain");
                return;
            };
            Action::Order {
                feed,
                order: change.order,
            }
        }
        "progress"
            if change.position.is_finite() && (0. ..=31_536_000.).contains(&change.position) =>
        {
            let path = grant
                .tracks
                .iter()
                .find(|t| t.id == change.id)
                .map(|t| t.path.clone())
                .or_else(|| grant.videos.get(&change.id).map(|v| v.path.clone()));
            let episode = grant
                .podcast_episodes
                .get(&change.id)
                .filter(|ep| {
                    enabled && allowed_feeds(grant, state).contains_key(&show_id(&ep.feed))
                })
                .map(|ep| EpisodeId {
                    feed_url: ep.feed.clone(),
                    episode_url: ep.url.clone(),
                });
            if path.is_none() && episode.is_none() {
                reply(request, 403, "Media not shared", "text/plain");
                return;
            }
            Action::Progress {
                path,
                episode,
                position: change.position,
                completed: change.completed,
                base: change.base,
                history: change.history,
                last_base: change.last_base,
            }
        }
        _ => {
            reply(
                request,
                403,
                "Change is not allowed by this pairing",
                "text/plain",
            );
            return;
        }
    };
    let saved_progress = match &action {
        Action::Progress {
            episode,
            position,
            completed,
            ..
        } => Some(progress_value(
            if episode.is_some() || !completed {
                *position
            } else {
                0.
            },
            episode.is_some() && *completed,
        )),
        _ => None,
    };
    let (tx, rx) = mpsc::sync_channel(1);
    callback(Change {
        action,
        reply: Some(tx),
    });
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => json(
            request,
            200,
            json!({"saved":true,"progress":saved_progress}),
        ),
        Ok(Err(e)) => reply(request, 409, e, "text/plain"),
        Err(_) => reply(
            request,
            503,
            "Desktop is busy. Retry when it responds.",
            "text/plain",
        ),
    }
}

struct MediaSession {
    grant: String,
    episode: String,
    expires: Instant,
}
#[derive(Default)]
pub(super) struct Sessions {
    sessions: BTreeMap<String, MediaSession>,
}
impl Sessions {
    fn create(&mut self, request: Request, digest: &str, id: &str) {
        self.sessions
            .retain(|_, s| s.expires > Instant::now() && s.grant != digest);
        if self.sessions.len() >= 64 {
            reply(request, 503, "Too many active players", "text/plain");
            return;
        }
        let secret = token();
        self.sessions.insert(
            hash(&secret),
            MediaSession {
                grant: digest.into(),
                episode: id.into(),
                expires: Instant::now() + Duration::from_secs(3600),
            },
        );
        reply_headers(
            request,
            200,
            json!({"url":format!("/api/podcast-media/{id}")}).to_string(),
            "application/json",
            &[(
                "Set-Cookie",
                format!(
                    "gnune_podcast={secret}; Path=/api/podcast-media/; Secure; HttpOnly; SameSite=Strict; Max-Age=3600"
                ),
            )],
        );
    }
    pub(super) fn serve(
        &self,
        request: Request,
        id: &str,
        grants: &BTreeMap<String, Grant>,
        (mode, enabled): (ShareMode, bool),
        stop: Arc<AtomicBool>,
        streams: Arc<AtomicUsize>,
    ) {
        let secret = header(&request, "Cookie").and_then(|c| {
            c.split(';')
                .find_map(|s| s.trim().strip_prefix("gnune_podcast=").map(str::to_owned))
        });
        let session = secret
            .and_then(|s| self.sessions.get(&hash(&s)))
            .filter(|s| s.expires > Instant::now() && s.episode == id);
        let episode = session
            .and_then(|s| grants.get(&s.grant))
            .filter(|g| permitted(g, mode, enabled))
            .and_then(|g| g.podcast_episodes.get(id));
        let Some(ep) = episode else {
            reply(
                request,
                403,
                "Start this episode from your paired player",
                "text/plain",
            );
            return;
        };
        relay(request, &ep.url, stop, streams);
    }
}
struct Stream {
    reader: BufReader<ChildStdout>,
    child: Child,
    stop: Arc<AtomicBool>,
    streams: Arc<AtomicUsize>,
}
impl std::io::Read for Stream {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        if self.stop.load(Ordering::Acquire) {
            return Ok(0);
        }
        self.reader.read(b)
    }
}
impl Drop for Stream {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.streams.fetch_sub(1, Ordering::AcqRel);
    }
}
fn relay(request: Request, url: &str, stop: Arc<AtomicBool>, streams: Arc<AtomicUsize>) {
    if streams
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
            (n < 4).then_some(n + 1)
        })
        .is_err()
    {
        reply(
            request,
            503,
            "Four media streams are already active",
            "text/plain",
        );
        return;
    }
    let range = header(&request, "Range");
    if range.as_ref().is_some_and(|r| {
        !r.starts_with("bytes=")
            || r.len() > 80
            || r.contains(',')
            || !r[6..].chars().all(|c| c.is_ascii_digit() || c == '-')
    }) {
        streams.fetch_sub(1, Ordering::AcqRel);
        reply(request, 416, "Use one byte range", "text/plain");
        return;
    }
    let url = url.to_owned();
    thread::spawn(move || {
        let mut command = Command::new("curl");
        command.args([
            "--disable",
            "--silent",
            "--show-error",
            "--location",
            "--max-redirs",
            "8",
            "--proto",
            "=http,https",
            "--proto-redir",
            "=http,https",
            "--connect-timeout",
            "8",
            "--max-time",
            "14400",
            "--max-filesize",
            "4294967296",
            "--include",
            "--suppress-connect-headers",
            "--user-agent",
            concat!("Gnune/", env!("CARGO_PKG_VERSION")),
        ]);
        if request.method() == &Method::Head {
            command.arg("--head");
        }
        if let Some(range) = range {
            command.args(["--header", &format!("Range: {range}")]);
        }
        let child = command
            .args(["--url", &url])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(_) => {
                streams.fetch_sub(1, Ordering::AcqRel);
                reply(
                    request,
                    502,
                    "Could not start publisher stream",
                    "text/plain",
                );
                return;
            }
        };
        let stdout = child.stdout.take().unwrap();
        let mut stream = Stream {
            reader: BufReader::new(stdout),
            child,
            stop,
            streams,
        };
        let parsed = (|| -> Result<(u16, Vec<Header>, Option<usize>), ()> {
            for _ in 0..10 {
                let mut line = String::new();
                read_header_line(&mut stream.reader, &mut line)?;
                let status = line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|s| s.parse::<u16>().ok())
                    .ok_or(())?;
                let mut headers = Vec::new();
                let mut length = None;
                let mut size = line.len();
                loop {
                    line.clear();
                    if read_header_line(&mut stream.reader, &mut line)? == 0 {
                        return Err(());
                    }
                    size += line.len();
                    if size > 65536 {
                        return Err(());
                    }
                    if line.trim().is_empty() {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        let name = name.to_ascii_lowercase();
                        let value = value.trim();
                        if name == "content-length" {
                            length = value.parse::<usize>().ok();
                        }
                        if matches!(
                            name.as_str(),
                            "content-type" | "content-range" | "accept-ranges"
                        ) {
                            headers.push(Header::from_bytes(name.as_str(), value).map_err(|_| ())?);
                        }
                        // curl decodes HTTP chunk framing; do not relay hop-by-hop headers.
                        if name == "transfer-encoding" {
                            length = None;
                        }
                        if name == "content-type"
                            && !matches!(status, 301 | 302 | 303 | 307 | 308 | 416)
                            && !media_mime(value)
                        {
                            return Err(());
                        }
                    }
                }
                if (300..400).contains(&status) || status == 100 {
                    continue;
                }
                if matches!(status, 200 | 206)
                    && !headers.iter().any(|h| h.field.equiv("Content-Type"))
                {
                    return Err(());
                }
                return Ok((status, headers, length));
            }
            Err(())
        })();
        match parsed {
            Ok((status, mut headers, length)) if matches!(status, 200 | 206 | 416) => {
                headers.push(Header::from_bytes("Cache-Control", "no-store").unwrap());
                headers.push(Header::from_bytes("X-Content-Type-Options", "nosniff").unwrap());
                let response = Response::new(status.into(), headers, stream, length, None);
                let _ = request.respond(response);
            }
            _ => reply(
                request,
                502,
                "Publisher stream is unavailable or uses an unsupported response",
                "text/plain",
            ),
        }
    });
}

fn media_mime(value: &str) -> bool {
    let mime = value
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    mime.starts_with("audio/")
        || mime.starts_with("video/")
        || mime == "application/octet-stream"
        || mime == "application/ogg"
}
fn read_header_line(reader: &mut BufReader<ChildStdout>, line: &mut String) -> Result<usize, ()> {
    let count = reader.take(65537).read_line(line).map_err(|_| ())?;
    if count > 65536 || (count > 0 && !line.ends_with('\n')) {
        return Err(());
    }
    Ok(count)
}

pub(super) fn refresh_grants(
    grants: &mut BTreeMap<String, Grant>,
    state: &Snapshot,
    mode: ShareMode,
    enabled: bool,
) {
    for grant in grants.values_mut() {
        if permitted(grant, mode, enabled) {
            let next = allowed_feeds(grant, state);
            for id in grant.podcasts.keys().filter(|id| !next.contains_key(*id)) {
                grant.artwork.remove(&art_id(id));
            }
            grant.podcasts = next;
            grant
                .podcast_episodes
                .retain(|_, ep| grant.podcasts.values().any(|f| f == &ep.feed));
        } else {
            grant.podcast_episodes.clear();
        }
    }
}

pub(super) fn is_art(grant: &Grant, id: &str) -> bool {
    grant.podcasts.keys().any(|show| art_id(show) == id)
}
fn art_id(show: &str) -> String {
    hash(&format!("podcast-art/{show}"))
}
fn include_art(grant: &mut Grant, feed: &str, show: &crate::media::PodcastShow) {
    if let Some(image) = &show.image {
        let path = crate::media::podcast_cover_path(image);
        if let Ok(meta) = fs::metadata(&path)
            && meta.is_file()
            && meta.len() <= 8 * 1024 * 1024
        {
            grant.artwork.insert(
                art_id(&show_id(feed)),
                Artwork {
                    path,
                    identity: identity(&meta),
                },
            );
        }
    }
}
pub(super) fn include(grant: &mut Grant, library: &Library) {
    for (feed, show) in &library.podcast_shows {
        if library
            .feeds
            .iter()
            .any(|f| crate::podcasts::normalize_url(f).is_ok_and(|f| f == *feed))
        {
            include_art(grant, feed, show);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    fn episode(feed: &str, url: &str) -> EpisodeId {
        EpisodeId::new(feed, url).unwrap()
    }
    fn progress(id: EpisodeId, position: f64, base: &str, last: &str) -> Change {
        Change {
            action: Action::Progress {
                path: None,
                episode: Some(id),
                position,
                completed: false,
                base: base.into(),
                history: false,
                last_base: last.into(),
            },
            reply: None,
        }
    }
    #[test]
    fn podcast_capability_defaults_and_revocation_do_not_elevate_old_pairings() {
        let mut db = Library {
            feeds: vec!["https://radio.test/feed".into()],
            ..Default::default()
        };
        let state = Snapshot::new(&db);
        let mut old = Grant {
            mode: ShareMode::Library,
            ..Default::default()
        };
        assert!(!permitted(&old, ShareMode::Library, true));
        assert!(
            snapshot(&old, &state, false)["podcasts"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        old.includes_podcasts = true;
        old.podcasts = allowed_feeds(&old, &state);
        old.podcast_episodes.insert(
            "episode".into(),
            Episode {
                feed: db.feeds[0].clone(),
                url: "https://radio.test/one.mp3".into(),
                title: "One".into(),
                duration: 0.,
            },
        );
        let mut grants = BTreeMap::from([("phone".into(), old)]);
        db.feeds.clear();
        refresh_grants(&mut grants, &Snapshot::new(&db), ShareMode::Library, true);
        assert!(grants["phone"].podcast_episodes.is_empty());
        assert!(!permitted(&grants["phone"], ShareMode::Queue, true));
        assert!(!permitted(&grants["phone"], ShareMode::Library, false));
    }
    #[test]
    fn progress_cas_accepts_backward_seek_and_rejects_old_offline_write() {
        let mut db = Library::default();
        let id = episode("https://radio.test/feed", "https://radio.test/a.mp3");
        db.record_podcast_progress(&id, 100., false);
        let base = revision(&(100., false));
        let last = revision(&db.last_podcast);
        assert!(
            progress(id.clone(), 90., &base, &last)
                .apply(&mut db)
                .0
                .is_ok()
        );
        assert_eq!(
            db.podcast_progress[&id.feed_url][&id.episode_url].position_secs,
            90.
        );
        assert!(
            progress(id.clone(), 110., &base, &last)
                .apply(&mut db)
                .0
                .is_err()
        );
        assert_eq!(
            db.podcast_progress[&id.feed_url][&id.episode_url].position_secs,
            90.
        );
    }
    #[test]
    fn late_episode_a_saves_its_resume_without_displacing_newer_episode_b() {
        let mut db = Library::default();
        let a = episode("https://radio.test/feed", "https://radio.test/a.mp3");
        let b = episode("https://radio.test/feed", "https://radio.test/b.mp3");
        db.record_podcast_progress(&a, 10., false);
        let last = revision(&db.last_podcast);
        db.record_podcast_progress(&b, 20., false);
        assert!(
            progress(a.clone(), 15., &revision(&(10., false)), &last)
                .apply(&mut db)
                .0
                .is_ok()
        );
        // A paused phone can poll after it has already seen the newer episode.
        let fresh_last = revision(&db.last_podcast);
        assert!(
            progress(a.clone(), 15., &revision(&(15., false)), &fresh_last)
                .apply(&mut db)
                .0
                .is_ok()
        );
        assert_eq!(db.last_podcast, Some(b));
        assert_eq!(
            db.podcast_progress[&a.feed_url][&a.episode_url].position_secs,
            15.
        );
    }
    #[test]
    fn quickplay_filters_private_album_refs_and_does_not_serialize_all_positions() {
        let permitted = Track {
            id: "one".into(),
            path: "/private/one.mp3".into(),
            album: "Record".into(),
            ..Default::default()
        };
        let private = Track {
            id: "private".into(),
            path: "/private/secret.mp3".into(),
            album: "Secret".into(),
            ..Default::default()
        };
        let db = Library {
            tracks: vec![permitted.clone(), private.clone()],
            pins: vec![private.path.clone(), permitted.path.clone()],
            history: vec![permitted.path.clone()],
            ..Default::default()
        };
        let grant = Grant {
            tracks: vec![permitted],
            ..Default::default()
        };
        let value = snapshot(&grant, &Snapshot::new(&db), false);
        assert_eq!(value["pins"].as_array().unwrap().len(), 1);
        assert!(value["progress"].as_object().unwrap().is_empty());
        assert!(!value.to_string().contains("/private/"));
        assert!(!value.to_string().contains("Secret"));
    }
    fn relay_response(responses: Vec<String>, range: Option<&str>) -> String {
        let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = upstream.local_addr().unwrap();
        let upstream_worker = thread::spawn(move || {
            for response in responses {
                let (mut socket, _) = upstream.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(socket.try_clone().unwrap());
                let mut line = String::new();
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                }
                socket
                    .write_all(response.replace("UPSTREAM", &addr.to_string()).as_bytes())
                    .unwrap();
            }
        });
        let server = Server::http("127.0.0.1:0").unwrap();
        let dest = server.server_addr().to_ip().unwrap();
        let handler = thread::spawn(move || {
            let request = server
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap();
            relay(
                request,
                &format!("http://{addr}/episode"),
                Arc::new(AtomicBool::new(false)),
                Arc::new(AtomicUsize::new(0)),
            );
        });
        let mut curl = Command::new("curl");
        curl.args(["--disable", "--silent", "--max-time", "8", "--include"]);
        if let Some(range) = range {
            curl.args(["--header", range]);
        }
        let result = curl.arg(format!("http://{dest}/test")).output().unwrap();
        handler.join().unwrap();
        upstream_worker.join().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        String::from_utf8(result.stdout).unwrap()
    }
    #[test]
    fn relay_follows_redirect_and_decodes_chunked_media_without_saving_audio() {
        let result=relay_response(vec!["HTTP/1.1 302 Found\r\nLocation: http://UPSTREAM/final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),"HTTP/1.1 200 OK\r\nContent-Type: audio/mpeg\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nabcde\r\n0\r\n\r\n".into()],None);
        assert!(result.ends_with("abcde"));
        assert!(
            result
                .to_lowercase()
                .contains("x-content-type-options: nosniff")
        );
    }
    #[test]
    fn relay_preserves_partial_media_status_and_content_range() {
        let result=relay_response(vec!["HTTP/1.1 206 Partial Content\r\nContent-Type: audio/mpeg\r\nContent-Range: bytes 2-4/5\r\nContent-Length: 3\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\ncde".into()],Some("Range: bytes=2-4"));
        assert!(result.starts_with("HTTP/1.1 206"));
        assert!(result.contains("bytes 2-4/5"));
        assert!(result.ends_with("cde"));
    }
    #[test]
    fn relay_rejects_html_in_place_of_podcast_media() {
        let result=relay_response(vec!["HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 4\r\nConnection: close\r\n\r\noops".into()],None);
        assert!(result.starts_with("HTTP/1.1 502"));
    }
}

#[cfg(test)]
mod artwork_boundary_tests {
    use super::*;
    #[test]
    fn restoring_a_music_only_grant_does_not_copy_podcast_artwork() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let art = Artwork {
            path: file.path().into(),
            identity: identity(&file.as_file().metadata().unwrap()),
        };
        let key = art_id("show");
        let selection = Grant {
            mode: ShareMode::Library,
            includes_podcasts: true,
            podcasts: BTreeMap::from([("show".into(), "https://radio.test/feed".into())]),
            artwork: BTreeMap::from([(key.clone(), art)]),
            ..Default::default()
        };
        let mut store = Store {
            devices: BTreeMap::from([(
                "old".into(),
                Grant {
                    mode: ShareMode::Library,
                    ..Default::default()
                },
            )]),
        };
        refresh_library_grants(&mut store, &selection);
        assert!(!store.devices["old"].artwork.contains_key(&key));
        assert!(!store.devices["old"].includes_podcasts);
    }
}
