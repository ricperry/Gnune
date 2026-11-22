//! Local video tags, soundtrack comparison, and opt-in catalog lookups.
use gnune::library::Track;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeSet, HashSet},
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Default, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(default)]
pub struct Song {
    pub title: String,
    pub artist: String,
    pub album: String,
}

impl Song {
    pub fn credit(&self) -> String {
        [self.artist.as_str(), self.album.as_str()]
            .into_iter()
            .filter(|v| !v.is_empty())
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoTags {
    /// None is an older record that still needs a genre probe.
    pub genre: Option<String>,
    pub saved_genre: Option<String>,
    pub album_artist: Option<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub embedded: Song,
    pub saved: Option<Song>,
    pub duration: f64,
    pub has_audio: bool,
}

impl VideoTags {
    pub fn genre(&self) -> &str {
        self.saved_genre
            .as_deref()
            .or(self.genre.as_deref())
            .unwrap_or("")
    }
    pub fn song(&self) -> &Song {
        self.saved.as_ref().unwrap_or(&self.embedded)
    }
}

pub fn probe(path: &Path) -> Result<VideoTags, String> {
    let output = Command::new("ffprobe")
        .args(["-v", "error", "-protocol_whitelist", "file,pipe", "-of", "json", "-show_entries",
               "format=duration:format_tags=title,artist,album,genre,album_artist,albumartist,track,disc:stream=codec_type,duration:stream_tags=title,artist,album,genre,album_artist,albumartist,track,disc", "-i"])
        .arg(path).output().map_err(|e| format!("Could not run ffprobe: {e}"))?;
    if !output.status.success() {
        return Err("Could not read the video’s metadata.".into());
    }
    let doc: Value = serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
    Ok(tags_from_probe(&doc))
}

fn tags_from_probe(doc: &Value) -> VideoTags {
    let format = &doc["format"];
    let audio = items(&doc["streams"]).find(|stream| stream["codec_type"] == "audio");
    let tag = |name: &str| {
        [format.get("tags"), audio.and_then(|a| a.get("tags"))]
            .into_iter()
            .chain(items(&doc["streams"]).map(|s| s.get("tags")))
            .flatten()
            .filter_map(Value::as_object)
            .flat_map(|tags| tags.iter())
            .filter(|(key, _)| key.eq_ignore_ascii_case(name))
            .filter_map(|(_, v)| v.as_str())
            .map(str::trim)
            .find(|v| !v.is_empty())
            .unwrap_or("")
            .to_owned()
    };
    let duration = audio
        .and_then(|a| seconds(&a["duration"]))
        .or_else(|| seconds(&format["duration"]))
        .unwrap_or(0.);
    VideoTags {
        saved_genre: None,
        genre: Some(tag("genre")),
        album_artist: Some({
            let name = tag("album_artist");
            if name.is_empty() {
                tag("albumartist")
            } else {
                name
            }
        }),
        track_number: tag("track").split('/').next().and_then(|s| s.parse().ok()),
        disc_number: tag("disc").split('/').next().and_then(|s| s.parse().ok()),
        embedded: Song {
            title: tag("title"),
            artist: tag("artist"),
            album: tag("album"),
        },
        saved: None,
        duration,
        has_audio: audio.is_some(),
    }
}

fn seconds(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str()?.parse().ok())
        .filter(|v| v.is_finite() && *v > 0.)
}
fn items(v: &Value) -> impl Iterator<Item = &Value> {
    v.as_array().into_iter().flatten()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Library,
    AcoustId,
    AudD,
}

// Public application identifier registered for Gnune 0.1, not a user credential.
const ACOUSTID_CLIENT_KEY: &str = "Rm2YBCvPPW";

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Keys {
    pub acoustid: String,
    pub audd: String,
}

impl Keys {
    fn path() -> PathBuf {
        gtk::glib::user_config_dir().join("gnune/recognition.json")
    }
    pub fn load() -> Result<Self, String> {
        match fs::read(Self::path()) {
            Ok(b) => {
                serde_json::from_slice(&b).map_err(|e| format!("Reading recognition settings: {e}"))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.to_string()),
        }
    }
    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        let dir = path.parent().unwrap();
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let mut file = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
        serde_json::to_writer(file.as_file_mut(), self).map_err(|e| e.to_string())?;
        file.flush().map_err(|e| e.to_string())?;
        file.persist(path).map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn token(&self, method: Method) -> String {
        let env = match method {
            Method::Library => return String::new(),
            Method::AcoustId => "GNUNE_ACOUSTID_KEY",
            Method::AudD => "GNUNE_AUDD_TOKEN",
        };
        self.resolve_token(method, std::env::var(env).ok().as_deref())
    }

    fn resolve_token(&self, method: Method, environment: Option<&str>) -> String {
        let (stored, fallback) = match method {
            Method::Library => return String::new(),
            Method::AcoustId => (self.acoustid.as_str(), ACOUSTID_CLIENT_KEY),
            Method::AudD => (self.audd.as_str(), ""),
        };
        [environment.unwrap_or_default(), stored, fallback]
            .into_iter()
            .map(str::trim)
            .find(|value| !value.is_empty())
            .unwrap_or_default()
            .to_owned()
    }
}

#[derive(Clone, Debug)]
pub struct Candidate {
    pub song: Song,
    pub confidence: Option<f64>,
}

pub struct Lookup {
    pub source: PathBuf,
    pub method: Method,
    pub tracks: Vec<Track>,
    pub token: String,
    pub start: f64,
}

impl Lookup {
    pub fn run(self, cache: &Path, cancelled: &AtomicBool) -> Result<Vec<Candidate>, String> {
        let tags = probe(&self.source)?;
        if !tags.has_audio {
            return Err("This video has no audio stream to identify.".into());
        }
        if tags.duration <= 0. && self.method != Method::AudD {
            return Err(
                "The recording length is unavailable. Choose an audio passage instead.".into(),
            );
        }
        if cancelled.load(Ordering::Relaxed) {
            return Ok(Vec::new());
        }
        match self.method {
            Method::Library => {
                match_library(&self.source, tags.duration, self.tracks, cache, cancelled)
            }
            Method::AcoustId => lookup_recording(&self.source, tags.duration, &self.token),
            Method::AudD => {
                if tags.duration > 0. && self.start >= tags.duration {
                    return Err("Choose a start position before the end of the video.".into());
                }
                lookup_passage(&self.source, self.start, &self.token)
            }
        }
    }
}

pub fn similar_duration(track: f64, video: f64) -> bool {
    track > 0. && (track - video).abs() <= 15.0_f64.max(video * 0.05)
}

fn fingerprint(path: &Path, format: &str) -> Result<Vec<u8>, String> {
    let out = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-protocol_whitelist",
            "file,pipe",
            "-i",
        ])
        .arg(path)
        .args([
            "-map",
            "0:a:0",
            "-t",
            "120",
            "-vn",
            "-ac",
            "1",
            "-ar",
            "11025",
            "-c:a",
            "pcm_s16le",
            "-f",
            "chromaprint",
            "-fp_format",
            format,
            "pipe:1",
        ])
        .output()
        .map_err(|e| format!("Could not run FFmpeg: {e}"))?;
    if !out.status.success() || out.stdout.is_empty() {
        return Err("Could not fingerprint this audio. Check the audio stream and FFmpeg’s Chromaprint support.".into());
    }
    Ok(out.stdout)
}
fn raw(path: &Path, cache: &Path) -> Result<Vec<u32>, String> {
    let stat = fs::metadata(path).map_err(|e| e.to_string())?;
    let stamp = stat
        .modified()
        .ok()
        .and_then(|d| d.duration_since(std::time::UNIX_EPOCH).ok());
    let key = crate::enrichment::key(&[
        &path.to_string_lossy(),
        &format!("v1-{}-{stamp:?}", stat.len()),
    ]);
    let file = cache.join(key);
    let bytes = match fs::read(&file) {
        Ok(b) if !b.is_empty() && b.len() <= 64 * 1024 && b.len() % 4 == 0 => b,
        _ => {
            let b = fingerprint(path, "raw")?;
            if fs::create_dir_all(cache).is_ok()
                && let Ok(mut t) = tempfile::NamedTempFile::new_in(cache)
                && t.write_all(&b).is_ok()
            {
                let _ = t.persist(&file);
            }
            b
        }
    };
    Ok(bytes
        .chunks_exact(4)
        .map(|n| u32::from_ne_bytes(n.try_into().unwrap()))
        .collect())
}

fn similarity(left: &[u32], right: &[u32]) -> f64 {
    // Each Chromaprint subfingerprint spans about 124 ms. Allow an intro offset
    // of up to 12 seconds. Quiet or constant passages carry too little evidence.
    const MIN: usize = 65;
    let extent = left.len().min(right.len());
    if extent < MIN
        || left.iter().copied().collect::<HashSet<_>>().len() < 12
        || right.iter().copied().collect::<HashSet<_>>().len() < 12
    {
        return 0.;
    }
    let mut best = 0.0_f64;
    for offset in -96_isize..=96 {
        let a = &left[(offset.max(0) as usize).min(left.len())..];
        let b = &right[((-offset).max(0) as usize).min(right.len())..];
        let n = a.len().min(b.len());
        if n < MIN.max(extent * 3 / 4) {
            continue;
        }
        let bits: u32 = a.iter().zip(b).map(|(a, b)| (a ^ b).count_ones()).sum();
        best = best.max(1. - f64::from(bits) / (n * 32) as f64);
    }
    best
}

fn match_library(
    path: &Path,
    duration: f64,
    tracks: Vec<Track>,
    cache: &Path,
    cancel: &AtomicBool,
) -> Result<Vec<Candidate>, String> {
    let needle = raw(path, cache)?;
    let mut hits = Vec::new();
    for t in tracks {
        if cancel.load(Ordering::Relaxed) {
            return Ok(Vec::new());
        }
        if !similar_duration(t.duration, duration) {
            continue;
        }
        let Ok(other) = raw(&t.path, cache) else {
            continue;
        };
        let confidence = similarity(&needle, &other);
        if confidence >= 0.90 {
            hits.push(Candidate {
                song: Song {
                    title: t.title,
                    artist: t.artist,
                    album: t.album,
                },
                confidence: Some(confidence),
            });
        }
    }
    Ok(ranked(hits))
}

fn ranked(mut choices: Vec<Candidate>) -> Vec<Candidate> {
    choices.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.song.cmp(&b.song))
    });
    let mut seen = BTreeSet::new();
    choices.retain(|x| !x.song.title.is_empty() && seen.insert(x.song.clone()));
    choices
}

fn curl() -> Command {
    let mut cmd = Command::new("curl");
    cmd.args([
        "--disable",
        "--silent",
        "--show-error",
        "--proto",
        "=https",
        "--connect-timeout",
        "8",
        "--max-time",
        "30",
        "--max-filesize",
        "5242880",
        "--user-agent",
        "Gnune/0.1",
    ]);
    cmd
}

fn post(mut cmd: Command, input: &[u8]) -> Result<Value, String> {
    let mut job = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not run curl: {e}"))?;
    let write_result = job.stdin.take().unwrap().write_all(input);
    let out = job.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "The recognition service could not be reached (curl {}).",
            out.status
        ));
    }
    write_result.map_err(|e| e.to_string())?;
    serde_json::from_slice(&out.stdout)
        .map_err(|_| "The recognition service returned an unreadable response.".into())
}

fn lookup_recording(path: &Path, duration: f64, key: &str) -> Result<Vec<Candidate>, String> {
    let fp = fingerprint(path, "base64")?;
    let fp = std::str::from_utf8(&fp)
        .map_err(|_| "Invalid fingerprint")?
        .trim();
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("client", key)
        .append_pair("duration", &duration.round().to_string())
        .append_pair("fingerprint", fp)
        .append_pair("meta", "recordings releasegroups compress")
        .finish();
    let mut cmd = curl();
    cmd.args(["--data-binary", "@-", "https://api.acoustid.org/v2/lookup"]);
    parse_acoustid(&post(cmd, body.as_bytes())?, crate::enrichment::recording)
}

fn parse_acoustid(
    doc: &Value,
    resolve: impl Fn(&str) -> Option<Value>,
) -> Result<Vec<Candidate>, String> {
    if doc["status"] != "ok" {
        return Err(format!(
            "AcoustID: {}",
            doc["error"]["message"].as_str().unwrap_or("lookup failed")
        ));
    }
    let mut result = Vec::new();
    let mut missing = Vec::new();
    for hit in items(&doc["results"]) {
        let score = hit["score"].as_f64();
        for recording in items(&hit["recordings"]) {
            let title = recording["title"].as_str().unwrap_or_default();
            if title.trim().is_empty() {
                if let Some(id) = recording["id"].as_str() {
                    missing.push((score, id));
                }
                continue;
            }
            let artist = items(&recording["artists"])
                .filter_map(|v| v["name"].as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let albums = items(&recording["releasegroups"]).filter_map(|v| v["title"].as_str());
            add_album_choices(&mut result, title, &artist, albums, score);
        }
    }
    if result.is_empty() {
        missing.sort_by(|a, b| b.0.unwrap_or_default().total_cmp(&a.0.unwrap_or_default()));
        let mut seen = HashSet::new();
        for (score, id) in missing
            .into_iter()
            .filter(|(_, id)| seen.insert(*id))
            .take(3)
        {
            if let Some(recording) = resolve(id)
                && let Some(title) = recording["title"].as_str().filter(|s| !s.trim().is_empty())
            {
                let artist = crate::enrichment::artist_credit(&recording);
                let albums = items(&recording["releases"]).filter_map(|release| {
                    release["release-group"]["title"]
                        .as_str()
                        .or_else(|| release["title"].as_str())
                });
                add_album_choices(&mut result, title, &artist, albums, score);
            }
        }
        if result.is_empty() && items(&doc["results"]).next().is_some() {
            return Err("The fingerprint matched, but no song details were available. Try another identification method.".into());
        }
    }
    Ok(ranked(result))
}

fn add_album_choices<'a>(
    choices: &mut Vec<Candidate>,
    title: &str,
    artist: &str,
    albums: impl Iterator<Item = &'a str>,
    confidence: Option<f64>,
) {
    let mut albums = albums.filter(|name| !name.trim().is_empty()).peekable();
    let fallback = albums.peek().is_none().then_some("");
    choices.extend(albums.chain(fallback).map(|album| Candidate {
        song: Song {
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
        },
        confidence,
    }));
}

fn lookup_passage(path: &Path, start: f64, key: &str) -> Result<Vec<Candidate>, String> {
    let clip = tempfile::Builder::new()
        .prefix("gnune-clip-")
        .suffix(".wav")
        .tempfile()
        .map_err(|e| e.to_string())?;
    let out = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-v",
            "error",
            "-y",
            "-ss",
            &format!("{start:.3}"),
            "-protocol_whitelist",
            "file,pipe",
            "-i",
        ])
        .arg(path)
        .args([
            "-map",
            "0:a:0",
            "-vn",
            "-t",
            "10",
            "-ac",
            "1",
            "-ar",
            "44100",
            "-c:a",
            "pcm_s16le",
        ])
        .arg(clip.path())
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() || clip.as_file().metadata().map(|m| m.len()).unwrap_or(0) < 1000 {
        return Err("Could not extract an audio passage at this position.".into());
    }
    // Read the credential from stdin so it never appears in a process list.
    let mut cmd = curl();
    cmd.args(["-F", "api_token=<-", "-F"])
        .arg(format!(
            "file=@{};filename=passage.wav;type=audio/wav",
            clip.path().display()
        ))
        .arg("https://api.audd.io/");
    parse_audd(&post(cmd, key.as_bytes())?)
}
fn parse_audd(doc: &Value) -> Result<Vec<Candidate>, String> {
    if doc["status"] != "success" {
        return Err(format!(
            "AudD: {}",
            doc["error"]["error_message"]
                .as_str()
                .unwrap_or("lookup failed")
        ));
    }
    let r = &doc["result"];
    Ok(ranked(vec![Candidate {
        song: Song {
            title: r["title"].as_str().unwrap_or_default().into(),
            artist: r["artist"].as_str().unwrap_or_default().into(),
            album: r["album"].as_str().unwrap_or_default().into(),
        },
        confidence: None,
    }]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn acoustid_uses_registered_client_without_persisting_it() {
        let keys = Keys::default();
        for environment in [None, Some(""), Some(" \t ")] {
            assert_eq!(
                keys.resolve_token(Method::AcoustId, environment),
                ACOUSTID_CLIENT_KEY
            );
        }
        assert_eq!(serde_json::to_value(&keys).unwrap()["acoustid"], "");
        let old_settings: Keys = serde_json::from_str(r#"{"audd":"existing"}"#).unwrap();
        assert_eq!(
            old_settings.resolve_token(Method::AcoustId, None),
            ACOUSTID_CLIENT_KEY
        );
    }

    #[test]
    fn explicit_keys_take_precedence_and_audd_still_requires_a_token() {
        let keys = Keys {
            acoustid: " saved-client ".into(),
            audd: " saved-token ".into(),
        };
        for (method, stored) in [
            (Method::AcoustId, "saved-client"),
            (Method::AudD, "saved-token"),
        ] {
            assert_eq!(keys.resolve_token(method, Some(" env-key ")), "env-key");
            assert_eq!(keys.resolve_token(method, Some("  ")), stored);
            assert_eq!(keys.resolve_token(method, None), stored);
        }
        assert!(Keys::default().resolve_token(Method::AudD, None).is_empty());
        assert!(
            keys.resolve_token(Method::Library, Some("env-key"))
                .is_empty()
        );
    }

    #[test]
    fn keeps_distinct_album_choices_and_skips_unidentified_results() {
        let rec = json!({"title":"Song","artists":[{"name":"Artist"}],
            "releasegroups":[{"title":"Album"},{"title":"Compilation"},{"title":"Album"}]});
        let choices = parse_acoustid(
            &json!({"status":"ok","results":[{"score":0.95,"recordings":[rec]},
            {"score":0.99,"recordings":[{"id":"only-an-id"}]}]}),
            |_| panic!("rich results already present"),
        )
        .unwrap();
        assert_eq!(
            choices
                .iter()
                .map(|v| v.song.album.as_str())
                .collect::<Vec<_>>(),
            ["Album", "Compilation"]
        );
        assert_eq!(choices[0].song.artist, "Artist");
        assert!(
            parse_acoustid(
                &json!({"status":"error","error":{"message":"bad client"}}),
                |_| None
            )
            .is_err()
        );
        assert!(
            parse_audd(&json!({"status":"success","result":null}))
                .unwrap()
                .is_empty()
        );
        assert!(parse_audd(&json!({"status":"error"})).is_err());
        let hit=parse_audd(&json!({"status":"success","result":{"title":"Title","artist":"Band","album":"Record"}})).unwrap();
        assert_eq!(hit[0].song.album, "Record");
    }
    #[test]
    fn resolves_id_only_matches_and_distinguishes_missing_credits_from_no_match() {
        let id = "5b7672f2-5e6a-42e1-ab42-b0d4bb6d8f94";
        let response = json!({"status":"ok","results":[{"score":1.0,"recordings":[{"id":id}]}]});
        let choices = parse_acoustid(&response, |requested| {
            assert_eq!(requested, id);
            Some(json!({"title":"Song","artist-credit":[{"name":"A","joinphrase":" feat. "},{"artist":{"name":"B"}}],
                "releases":[{"release-group":{"title":"Album"},"title":"Album (deluxe)"},{"title":"Single"}]}))
        }).unwrap();
        assert_eq!(choices.len(), 2);
        assert_eq!(choices[0].song.artist, "A feat. B");
        assert_eq!(choices[0].song.album, "Album");
        assert_eq!(choices[1].song.album, "Single");
        assert!(
            parse_acoustid(&response, |_| None)
                .unwrap_err()
                .contains("fingerprint matched")
        );
        assert!(
            parse_acoustid(&json!({"status":"ok","results":[]}), |_| None)
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn local_match_tolerates_a_short_intro_and_rejects_unrelated_or_silent_audio() {
        fn sequence(mut seed: u32) -> Vec<u32> {
            (0..240)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    seed
                })
                .collect()
        }
        let a = sequence(17877);
        let mut aligned = vec![198; 22];
        aligned.extend(a.iter().map(|v| v ^ 0x8));
        assert!(similarity(&a, &aligned) > 0.95);
        assert!(similarity(&a, &sequence(631)) < 0.65);
        assert_eq!(similarity(&[7; 90], &[7; 90]), 0.);
    }
    #[test]
    fn video_probe_reads_grouping_tags_and_number_totals_from_streams() {
        let tags = tags_from_probe(&json!({
            "format": {"duration":"12", "tags": {"TITLE":" Title ", "ARTIST":"Guest"}},
            "streams": [{"codec_type":"video", "tags": {"ALBUM":"Compilation", "GENRE":"Rock", "ALBUM_ARTIST":"Various artists", "TRACK":"3/12", "DISC":"2/2"}}]
        }));
        assert_eq!(tags.song().title, "Title");
        assert_eq!(tags.song().album, "Compilation");
        assert_eq!(tags.genre(), "Rock");
        assert_eq!(tags.album_artist.as_deref(), Some("Various artists"));
        assert_eq!(tags.track_number, Some(3));
        assert_eq!(tags.disc_number, Some(2));
        assert_eq!(tags.duration, 12.);
        assert_eq!(tags_from_probe(&json!({})).genre.as_deref(), Some(""));
    }

    #[test]
    fn legacy_collection_and_saved_overrides_survive_probe_refresh() {
        let mut db: crate::state::Library =
            serde_json::from_str(r#"{"assets":["old.mkv"]}"#).unwrap();
        assert!(db.video_tags.is_empty());
        let path = PathBuf::from("old.mkv");
        let saved = Song {
            title: "Reviewed".into(),
            ..Song::default()
        };
        db.video_tags.entry(path.clone()).or_default().saved = Some(saved.clone());
        let info = VideoTags {
            embedded: Song {
                title: "File tag".into(),
                ..Default::default()
            },
            genre: Some("Rock".into()),
            duration: 20.,
            has_audio: true,
            ..Default::default()
        };
        db.merge_video_tags(path.clone(), info);
        let restored: crate::state::Library =
            serde_json::from_slice(&serde_json::to_vec(&db).unwrap()).unwrap();
        assert_eq!(restored.video_tags[&path].song(), &saved);
        assert_eq!(restored.video_tags[&path].duration, 20.);
        assert_eq!(restored.video_tags[&path].genre(), "Rock");
    }
}
