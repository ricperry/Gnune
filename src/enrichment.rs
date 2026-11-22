//! Blocking, opt-in metadata enrichment. Call from one worker, never the UI thread.
use gnune::library::Track;
use serde_json::Value;
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};
use url::Url;

#[derive(Clone, Default, Debug)]
pub struct Details {
    pub art: Option<PathBuf>,
    pub lyrics: Option<String>,
    pub website: Option<String>,
    pub bandcamp: Option<String>,
}

static MB_LAST: Mutex<Option<Instant>> = Mutex::new(None);
const AGENT: &str = "Gnune/0.1 (local desktop music-library enrichment)";

fn locations(t: &Track, dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let artist = t
        .artist_id
        .as_deref()
        .filter(|id| uuid(id))
        .unwrap_or(&t.artist);
    let release = t
        .release_id
        .as_deref()
        .filter(|id| uuid(id))
        .unwrap_or(&t.album);
    (
        dir.join(key(&[artist])),
        dir.join(key(&[&t.album_artist, release])),
        dir.join(key(&[
            artist,
            release,
            &t.title,
            &format!("{:.3}", t.duration),
        ])),
    )
}
/// Resolve the album sleeve without reading its pixels or looking up song details.
pub fn cached_cover(t: &Track, dir: &Path) -> Option<PathBuf> {
    let (_, album_dir, _) = locations(t, dir);
    let art = album_dir.join("cover");
    art.is_file().then_some(art)
}
/// Consult local tags and cached provider results without a network request.
pub fn cached(t: &Track, dir: &Path) -> Details {
    let (artist_dir, album_dir, song_dir) = locations(t, dir);
    let art = album_dir.join("cover");
    Details {
        art: fs::File::open(&art).ok().and_then(|file| {
            let mut header = Vec::with_capacity(8);
            file.take(8).read_to_end(&mut header).ok()?;
            image(&header).then_some(art)
        }),
        lyrics: read_text(&song_dir.join("lyrics.txt")),
        website: t
            .artist_url
            .clone()
            .filter(|s| web_url(s))
            .or_else(|| read_text(&artist_dir.join("website.txt")).filter(|s| web_url(s))),
        bandcamp: read_text(&artist_dir.join("bandcamp.txt")).filter(|s| bandcamp_url(s)),
    }
}

pub fn lookup(t: &Track, dir: &Path, need_art: bool, need_lyrics: bool) -> Details {
    let (artist, album, title, duration) = (
        t.artist.as_str(),
        t.album.as_str(),
        t.title.as_str(),
        t.duration,
    );
    let mut result = cached(t, dir);
    if artist.trim().is_empty() {
        return result;
    }
    let (artist_dir, album_dir, song_dir) = locations(t, dir);
    if need_art
        && result.art.is_none()
        && !album.trim().is_empty()
        && let Some((kind, id)) = t
            .release_id
            .as_ref()
            .filter(|id| uuid(id))
            .map(|id| ("release", id.clone()))
            .or_else(|| {
                t.release_group_id
                    .as_ref()
                    .filter(|id| uuid(id))
                    .map(|id| ("release-group", id.clone()))
            })
            .or_else(|| cover_identity(&t.album_artist, album))
        && let Some(bytes) = download(&format!(
            "https://coverartarchive.org/{kind}/{id}/front-500"
        ))
        .filter(|b| image(b))
    {
        let path = album_dir.join("cover");
        save_new(&path, &bytes);
        result.art = fs::read(&path).ok().filter(|b| image(b)).map(|_| path);
    }
    if need_lyrics
        && result.lyrics.is_none()
        && !title.trim().is_empty()
        && duration.is_finite()
        && duration > 0.0
    {
        let seconds = duration.to_string();
        if let Some(value) = json_request(
            "https://lrclib.net/api/get",
            &[
                ("artist_name", artist),
                ("album_name", album),
                ("track_name", title),
                ("duration", &seconds),
            ],
        ) {
            result.lyrics = lyrics(&value, artist, album, title, duration);
            if let Some(lyrics) = &result.lyrics {
                let path = song_dir.join("lyrics.txt");
                save_new(&path, lyrics.as_bytes());
                result.lyrics = read_text(&path).or_else(|| Some(lyrics.clone()));
            }
        }
    }
    if result.website.is_none() || result.bandcamp.is_none() {
        if let Some(id) = t
            .artist_id
            .clone()
            .filter(|id| uuid(id))
            .or_else(|| artist_id(artist))
            && let Some(value) = mb_request(
                &format!("https://musicbrainz.org/ws/2/artist/{id}"),
                &[("inc", "url-rels")],
            )
            && let Some(relations) = value["relations"].as_array()
        {
            for relation in relations {
                let target = text(&relation["url"], "resource");
                if result.website.is_none()
                    && text(relation, "type") == "official homepage"
                    && web_url(target)
                {
                    result.website = Some(target.to_owned());
                }
                if result.bandcamp.is_none() && bandcamp_url(target) {
                    result.bandcamp = Some(target.to_owned());
                }
            }
        }
        if let Some(site) = &result.website {
            save_new(&artist_dir.join("website.txt"), site.as_bytes());
        }
        if let Some(site) = &result.bandcamp {
            save_new(&artist_dir.join("bandcamp.txt"), site.as_bytes());
        }
        let current = cached(t, dir);
        result.website = current.website.or(result.website);
        result.bandcamp = current.bandcamp.or(result.bandcamp);
    }
    result
}

fn lyrics(value: &Value, artist: &str, album: &str, title: &str, duration: f64) -> Option<String> {
    if !same(text(value, "artistName"), artist)
        || !same(text(value, "trackName"), title)
        || (!album.is_empty() && !same(text(value, "albumName"), album))
        || !value["duration"]
            .as_f64()
            .is_some_and(|d| (d - duration).abs() <= 2.0)
    {
        return None;
    }
    ["syncedLyrics", "plainLyrics"]
        .into_iter()
        .filter_map(|field| value[field].as_str())
        .find(|s| !s.trim().is_empty())
        .map(str::to_owned)
}

fn cover_identity(artist: &str, album: &str) -> Option<(&'static str, String)> {
    let query = format!("release:{} AND artist:{}", phrase(album), phrase(artist));
    if let Some(response) = mb_request(
        "https://musicbrainz.org/ws/2/release/",
        &[("query", &query), ("limit", "100")],
    ) && let Some(id) = unique(&response, "releases", |r| {
        same(text(r, "title"), album) && credited(r, artist)
    }) {
        return Some(("release", id));
    }
    // Multiple editions are acceptable only when one exact release group is unambiguous.
    let query = format!(
        "releasegroup:{} AND artist:{}",
        phrase(album),
        phrase(artist)
    );
    let response = mb_request(
        "https://musicbrainz.org/ws/2/release-group/",
        &[("query", &query), ("limit", "100")],
    )?;
    unique(&response, "release-groups", |r| {
        same(text(r, "title"), album) && credited(r, artist)
    })
    .map(|id| ("release-group", id))
}

fn credited(value: &Value, artist: &str) -> bool {
    let name = artist_credit(value);
    !name.is_empty() && same(&name, artist)
}

pub(crate) fn artist_credit(value: &Value) -> String {
    let mut name = String::new();
    for credit in value["artist-credit"].as_array().into_iter().flatten() {
        if let Some(part) = credit["name"]
            .as_str()
            .or_else(|| credit["artist"]["name"].as_str())
        {
            name.push_str(part);
            name.push_str(text(credit, "joinphrase"));
        }
    }
    name
}

pub(crate) fn recording(id: &str) -> Option<Value> {
    if !uuid(id) {
        return None;
    }
    mb_request(
        &format!("https://musicbrainz.org/ws/2/recording/{id}"),
        &[("inc", "artist-credits releases release-groups")],
    )
}

fn artist_id(artist: &str) -> Option<String> {
    let query = format!("artist:{}", phrase(artist));
    let response = mb_request(
        "https://musicbrainz.org/ws/2/artist/",
        &[("query", &query), ("limit", "100")],
    )?;
    unique(&response, "artists", |item| {
        same(text(item, "name"), artist)
    })
}

fn unique(response: &Value, field: &str, predicate: impl Fn(&Value) -> bool) -> Option<String> {
    let items = response[field].as_array()?;
    if response["count"].as_u64()? > items.len() as u64 {
        return None;
    }
    let mut matches = items.iter().filter(|item| predicate(item));
    let id = text(matches.next()?, "id");
    if matches.next().is_some() || !uuid(id) {
        return None;
    }
    Some(id.to_owned())
}

fn mb_request(endpoint: &str, query: &[(&str, &str)]) -> Option<Value> {
    let mut last = MB_LAST.lock().ok()?;
    if let Some(previous) = *last {
        thread::sleep(Duration::from_millis(1100).saturating_sub(previous.elapsed()));
    }
    *last = Some(Instant::now());
    drop(last);
    json_request(endpoint, query)
}

fn json_request(endpoint: &str, query: &[(&str, &str)]) -> Option<Value> {
    let mut url = Url::parse(endpoint).ok()?;
    url.query_pairs_mut().extend_pairs(query.iter().copied());
    if url.host_str() == Some("musicbrainz.org") {
        url.query_pairs_mut().append_pair("fmt", "json");
    }
    serde_json::from_slice(&download(url.as_str())?).ok()
}

fn download(endpoint: &str) -> Option<Vec<u8>> {
    let output = Command::new("curl")
        .args([
            "--disable",
            "--silent",
            "--show-error",
            "--fail",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--retry",
            "1",
            "--retry-delay",
            "2",
            "--retry-max-time",
            "10",
            "--connect-timeout",
            "5",
            "--max-time",
            "20",
            "--max-filesize",
            "8388608",
            "--user-agent",
            AGENT,
            "--url",
            endpoint,
        ])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then_some(output.stdout)
        .filter(|bytes| !bytes.is_empty())
}

fn save_new(path: &Path, bytes: &[u8]) {
    let Some(parent) = path.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let Ok(mut temporary) = tempfile::NamedTempFile::new_in(parent) else {
        return;
    };
    if temporary.write_all(bytes).is_ok() {
        let _ = temporary.persist_noclobber(path);
    }
}
fn read_text(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .filter(|s| !s.trim().is_empty())
}
fn text<'a>(value: &'a Value, field: &str) -> &'a str {
    value[field].as_str().unwrap_or("")
}
fn same(left: &str, right: &str) -> bool {
    fn normalize(value: &str) -> String {
        value
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    }
    normalize(left) == normalize(right)
}
fn phrase(value: &str) -> String {
    let mut escaped = String::from("\"");
    for character in value.chars() {
        if "+-!(){}[]^\"~*?:\\/&|".contains(character) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped.push('"');
    escaped
}
fn web_url(value: &str) -> bool {
    Url::parse(value).is_ok_and(|u| {
        matches!(u.scheme(), "http" | "https")
            && u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
    })
}
fn bandcamp_url(value: &str) -> bool {
    web_url(value)
        && Url::parse(value).is_ok_and(|u| {
            u.host_str()
                .is_some_and(|h| h == "bandcamp.com" || h.ends_with(".bandcamp.com"))
        })
}
fn uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
fn image(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xff, 0xd8, 0xff]) || bytes.starts_with(b"\x89PNG\r\n\x1a\n")
}
pub(crate) fn key(parts: &[&str]) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for part in parts {
        for byte in (part.len() as u64)
            .to_le_bytes()
            .into_iter()
            .chain(part.bytes())
        {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn rejects_ambiguous_and_truncated_matches() {
        let id = "12345678-1234-1234-1234-123456789abc";
        assert_eq!(
            unique(&json!({"count":1,"artists":[{"id":id}]}), "artists", |_| {
                true
            }),
            Some(id.into())
        );
        assert!(
            unique(&json!({"count":2,"artists":[{"id":id}]}), "artists", |_| {
                true
            })
            .is_none()
        );
        assert!(
            unique(
                &json!({"count":2,"artists":[{"id":id},{"id":id}]}),
                "artists",
                |_| true
            )
            .is_none()
        );
    }
    #[test]
    fn synced_lyrics_win_and_identity_is_checked() {
        let value = json!({"artistName":"Artist","albumName":"Album","trackName":"Song","duration":120,"syncedLyrics":"[00:01] Hi","plainLyrics":"Hi"});
        assert_eq!(
            lyrics(&value, "Artist", "Album", "Song", 120.0).as_deref(),
            Some("[00:01] Hi")
        );
        assert!(lyrics(&value, "Other", "Album", "Song", 120.0).is_none());
        assert!(lyrics(&value, "Artist", "Album", "Song", 125.0).is_none());
    }
    #[test]
    fn cache_preserves_user_correction_and_never_needs_network() {
        let dir = tempfile::tempdir().unwrap();
        let mut t = Track {
            artist: "A".into(),
            album: "B".into(),
            title: "C".into(),
            duration: 1.0,
            ..Default::default()
        };
        let (_, _, song) = locations(&t, dir.path());
        save_new(&song.join("lyrics.txt"), b"corrected");
        save_new(&song.join("lyrics.txt"), b"remote");
        assert_eq!(cached(&t, dir.path()).lyrics.as_deref(), Some("corrected"));
        t.title = "different".into();
        assert!(cached(&t, dir.path()).lyrics.is_none());
    }
    #[test]
    fn escaping_and_link_hosts_are_safe() {
        assert_eq!(phrase("a\"b:c\\d"), "\"a\\\"b\\:c\\\\d\"");
        assert!(bandcamp_url("https://artist.bandcamp.com/album/a"));
        assert!(!bandcamp_url("https://bandcamp.com.attacker.example"));
        assert!(!web_url("file:///tmp/file"));
        assert_ne!(key(&["ab", "c"]), key(&["a", "bc"]));
    }
}
