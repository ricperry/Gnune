//! Blocking local media discovery and podcast import/download helpers.
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use url::Url;

mod poster;
pub use poster::{poster_path, prepare_poster};

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Episode {
    pub title: String,
    pub url: String,
    pub summary: String,
    pub published: Option<i64>,
    pub date: String,
}

pub fn assets(paths: &[PathBuf]) -> Vec<PathBuf> {
    fn visit(path: &Path, output: &mut BTreeSet<PathBuf>) {
        let Ok(metadata) = fs::symlink_metadata(path) else {
            return;
        };
        if metadata.is_symlink() || path.to_str().is_none() {
            return;
        }
        if metadata.is_dir() {
            if let Ok(entries) = fs::read_dir(path) {
                for entry in entries.flatten() {
                    visit(&entry.path(), output);
                }
            }
        } else if metadata.is_file()
            && path
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|extension| {
                    matches!(
                        extension.to_ascii_lowercase().as_str(),
                        "mp4" | "m4v" | "mkv" | "webm" | "mov" | "avi" | "ogv" | "mpeg" | "mpg"
                    )
                })
        {
            output.insert(fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()));
        }
    }
    let mut output = BTreeSet::new();
    for path in paths {
        visit(path, &mut output);
    }
    output.into_iter().collect()
}

pub fn podcast(feed: &str, dir: &Path) -> Result<PodcastFeed, String> {
    let url = http_url(feed)?;
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let temporary = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
    fetch(url.as_str(), temporary.path(), "8388608", "30")?;
    let xml = fs::read_to_string(temporary.path()).map_err(|e| e.to_string())?;
    let podcast = parse_feed(&xml, &url)?;
    let _ = cache_podcast(feed, &podcast, dir);
    Ok(podcast)
}

pub fn download(episode: &Episode, dir: &Path) -> Result<PathBuf, String> {
    let url = http_url(&episode.url)?;
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let extension = Path::new(url.path())
        .extension()
        .and_then(|s| s.to_str())
        .filter(|s| {
            matches!(
                s.to_ascii_lowercase().as_str(),
                "mp3" | "m4a" | "mp4" | "ogg" | "opus" | "wav" | "flac" | "aac" | "webm"
            )
        })
        .unwrap_or("audio")
        .to_ascii_lowercase();
    let destination = dir.join(format!(
        "episode-{}.{}",
        crate::enrichment::key(&[url.as_str()]),
        extension
    ));
    if fs::metadata(&destination).is_ok_and(|m| m.is_file() && m.len() > 0) {
        return Ok(destination);
    }
    let temporary = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
    fetch(url.as_str(), temporary.path(), "2147483648", "600")?;
    if temporary
        .as_file()
        .metadata()
        .map_err(|e| e.to_string())?
        .len()
        == 0
    {
        return Err("The episode download was empty".into());
    }
    temporary.as_file().sync_all().map_err(|e| e.to_string())?;
    match temporary.persist_noclobber(&destination) {
        Ok(_) => Ok(destination),
        Err(error)
            if error.error.kind() == std::io::ErrorKind::AlreadyExists
                && fs::metadata(&destination).is_ok_and(|m| m.is_file() && m.len() > 0) =>
        {
            Ok(destination)
        }
        Err(error) => Err(error.error.to_string()),
    }
}

pub(crate) fn http_url(value: &str) -> Result<Url, String> {
    let url = Url::parse(value).map_err(|e| e.to_string())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("Use an HTTP or HTTPS URL without embedded credentials".into());
    }
    Ok(url)
}

pub(crate) fn fetch(url: &str, path: &Path, limit: &str, seconds: &str) -> Result<(), String> {
    let output = Command::new("curl")
        .args([
            "--disable",
            "--silent",
            "--show-error",
            "--fail",
            "--location",
            "--proto",
            "=http,https",
            "--proto-redir",
            "=http,https",
            "--connect-timeout",
            "8",
            "--max-time",
            seconds,
            "--max-filesize",
            limit,
            "--user-agent",
            concat!(
                "Gnune/",
                env!("CARGO_PKG_VERSION"),
                " (+",
                env!("CARGO_PKG_REPOSITORY"),
                ")"
            ),
            "--output",
        ])
        .arg(path)
        .args(["--url", url])
        .output()
        .map_err(|e| format!("Could not run curl: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Download failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PodcastShow {
    pub title: String,
    pub publisher: String,
    pub summary: String,
    pub image: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PodcastFeed {
    pub show: PodcastShow,
    pub episodes: Vec<Episode>,
}

fn feed_text(node: roxmltree::Node<'_, '_>, names: &[&str]) -> String {
    names
        .iter()
        .find_map(|name| {
            node.children()
                .find(|n| n.is_element() && n.tag_name().name() == *name)
                .map(|n| {
                    n.descendants()
                        .filter(|n| n.is_text())
                        .filter_map(|n| n.text())
                        .collect::<String>()
                })
                .filter(|s| !s.trim().is_empty())
        })
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_feed(xml: &str, base: &Url) -> Result<PodcastFeed, String> {
    let document =
        roxmltree::Document::parse(xml).map_err(|e| format!("Invalid podcast feed: {e}"))?;
    let root = document.root_element();
    let channel = if root.tag_name().name() == "rss"
        || (root.tag_name().name() == "RDF"
            && root.tag_name().namespace() == Some("http://www.w3.org/1999/02/22-rdf-syntax-ns#"))
    {
        root.children()
            .find(|n| n.is_element() && n.tag_name().name() == "channel")
            .ok_or("The RSS feed has no channel")?
    } else if root.tag_name().name() == "feed" {
        root
    } else {
        return Err("This document is not an RSS or Atom feed".into());
    };
    let publisher = feed_text(channel, &["author", "managingEditor", "creator"]);
    let show = PodcastShow {
        title: feed_text(channel, &["title"]).chars().take(500).collect(),
        publisher: publisher.chars().take(500).collect(),
        summary: plain_summary(&feed_text(channel, &["description", "subtitle", "summary"])),
        image: feed_image(channel, base),
    };
    let mut seen = BTreeSet::new();
    let mut episodes = Vec::new();
    for item in document
        .descendants()
        .filter(|n| n.is_element() && matches!(n.tag_name().name(), "item" | "entry"))
    {
        let enclosure = item
            .children()
            .filter(|node| {
                node.is_element()
                    && (node.tag_name().name() == "enclosure"
                        || (node.tag_name().name() == "link"
                            && node.attribute("rel") == Some("enclosure")))
            })
            .filter_map(|node| {
                let target = node.attribute("url").or_else(|| node.attribute("href"))?;
                let url = base
                    .join(target)
                    .ok()
                    .and_then(|url| http_url(url.as_str()).ok())?;
                let kind = node
                    .attribute("type")
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                if kind.starts_with("image/") || kind == "application/pdf" {
                    return None;
                }
                let rank = if kind.starts_with("audio/") {
                    0
                } else if kind.starts_with("video/") || kind == "application/ogg" {
                    1
                } else if kind.is_empty()
                    || kind == "application/octet-stream"
                    || matches!(
                        Path::new(url.path())
                            .extension()
                            .and_then(|value| value.to_str()),
                        Some(
                            "mp3"
                                | "m4a"
                                | "mp4"
                                | "ogg"
                                | "opus"
                                | "wav"
                                | "aac"
                                | "flac"
                                | "webm"
                        )
                    )
                {
                    2
                } else {
                    return None;
                };
                Some((rank, url))
            })
            .min_by_key(|(rank, _)| *rank);
        let Some((_, url)) = enclosure else {
            continue;
        };
        if !seen.insert(url.to_string()) {
            continue;
        }
        let title = item
            .children()
            .find(|n| n.is_element() && n.tag_name().name() == "title")
            .map(|n| {
                n.descendants()
                    .filter_map(|child| child.is_text().then(|| child.text()).flatten())
                    .collect::<String>()
            })
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "Untitled episode".into());
        let date = feed_text(item, &["pubDate", "published", "date", "updated"]);
        episodes.push(Episode {
            title: title.trim().chars().take(1000).collect(),
            url: url.into(),
            summary: plain_summary(&feed_text(
                item,
                &["summary", "description", "encoded", "content"],
            )),
            published: episode_date(&date),
            date,
        });
    }
    episodes.sort_by_key(|episode| std::cmp::Reverse(episode.published));
    episodes.truncate(2000);
    Ok(PodcastFeed { show, episodes })
}

fn episode_date(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc2822(value)
        .or_else(|_| chrono::DateTime::parse_from_rfc3339(value))
        .ok()
        .map(|date| date.timestamp())
}

fn feed_image(channel: roxmltree::Node<'_, '_>, base: &Url) -> Option<String> {
    let images = channel
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() == "image");
    let candidates = images
        .clone()
        .filter_map(|node| node.attribute("href").map(str::to_owned))
        .chain(images.map(|image| feed_text(image, &["url"])))
        .chain([feed_text(channel, &["logo"]), feed_text(channel, &["icon"])]);
    candidates
        .filter(|image| !image.is_empty())
        .find_map(|image| {
            base.join(&image)
                .ok()
                .and_then(|image| http_url(image.as_str()).ok())
                .map(Into::into)
        })
}

pub fn podcast_cache_dir() -> PathBuf {
    gtk::glib::user_cache_dir().join("gnune/podcast-feeds")
}
fn cache_podcast(feed: &str, podcast: &PodcastFeed, dir: &Path) -> Result<(), String> {
    let path = dir.join(format!("{}.json", crate::enrichment::key(&[feed])));
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(path.parent().unwrap()).map_err(|e| e.to_string())?;
    serde_json::to_writer(temporary.as_file_mut(), podcast).map_err(|e| e.to_string())?;
    temporary.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}
pub fn cached_podcast(feed: &str) -> Option<PodcastFeed> {
    cached_podcast_in(feed, &podcast_cache_dir())
}
fn cached_podcast_in(feed: &str, dir: &Path) -> Option<PodcastFeed> {
    use std::io::Read;
    let file =
        fs::File::open(dir.join(format!("{}.json", crate::enrichment::key(&[feed])))).ok()?;
    if file.metadata().ok()?.len() > 16 * 1024 * 1024 {
        return None;
    }
    serde_json::from_reader(std::io::BufReader::new(file.take(16 * 1024 * 1024))).ok()
}
pub fn podcast_cover_path(url: &str) -> PathBuf {
    gtk::glib::user_cache_dir()
        .join("gnune/podcast-covers")
        .join(crate::enrichment::key(&[url]))
}
pub fn podcast_cover(url: &str) -> Result<PathBuf, String> {
    let url = http_url(url)?;
    let path = podcast_cover_path(url.as_str());
    if path.is_file() {
        return Ok(path);
    }
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let temporary =
        tempfile::NamedTempFile::new_in(path.parent().unwrap()).map_err(|e| e.to_string())?;
    fetch(url.as_str(), temporary.path(), "4194304", "15")?;
    let (format, width, height) = gtk::gdk_pixbuf::Pixbuf::file_info(temporary.path())
        .ok_or("The cover is not a supported image")?;
    if !matches!(
        format.name().as_deref(),
        Some("png" | "jpeg" | "webp" | "gif")
    ) || !(1..=4096).contains(&width)
        || !(1..=4096).contains(&height)
    {
        return Err("The cover must be a PNG/JPEG/WebP/GIF image up to 4096 pixels".into());
    }
    temporary.persist(&path).map_err(|e| e.to_string())?;
    Ok(path)
}

pub(crate) fn plain_summary(value: &str) -> String {
    // Descriptions often contain escaped HTML. Never render publisher markup in GTK.
    let value = value.replace("&lt;", "<").replace("&gt;", ">");
    let mut text = String::new();
    let mut tag = false;
    for c in value.chars() {
        match c {
            '<' => {
                tag = true;
                text.push(' ');
            }
            '>' => tag = false,
            _ if !tag => text.push(c),
            _ => {}
        }
    }
    text.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(6000)
        .collect()
}

pub fn is_photo_path(p: &Path) -> bool {
    matches!(
        p.extension()
            .and_then(|v| v.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "png" | "jpg" | "jpeg" | "webp" | "bmp" | "gif" | "tif" | "tiff" | "avif" | "heic"
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn episode_summaries_dates_and_audio_enclosures_are_normalized() {
        let base = Url::parse("https://example.test/feed").unwrap();
        let podcast = parse_feed(r#"<rss><channel><title>Studio</title><item><title>Undated first</title><enclosure url="/undated.mp3"/></item><item><title>Old</title><pubDate>Tue, 01 Jan 2019 09:00:00 GMT</pubDate><enclosure url="/old.mp3"/></item><item><title>Latest</title><pubDate>Wed, 20 Nov 2024 08:00:00 +0000</pubDate><description>&lt;p&gt;Drawing &amp;amp; music&lt;/p&gt;</description><enclosure type="image/png" url="/not-a-podcast.png"/><enclosure type="audio/mpeg" url="/latest.mp3"/></item><item><title>Same instant</title><pubDate>Wed, 20 Nov 2024 03:00:00 -0500</pubDate><enclosure url="/tie.mp3"/></item><item><title>Broken date</title><pubDate>tomorrow</pubDate><enclosure url="/broken-date.mp3"/></item><item><enclosure type="application/pdf" url="/book.pdf"/></item></channel></rss>"#, &base).unwrap();
        assert_eq!(
            podcast
                .episodes
                .iter()
                .map(|episode| episode.title.as_str())
                .collect::<Vec<_>>(),
            [
                "Latest",
                "Same instant",
                "Old",
                "Undated first",
                "Broken date"
            ]
        );
        assert_eq!(podcast.episodes[0].summary, "Drawing & music");
        assert_eq!(podcast.episodes[0].url, "https://example.test/latest.mp3");
        assert_eq!(podcast.episodes[0].published, podcast.episodes[1].published);
        assert_eq!(
            episode_date("Wed, 20 Nov 2024 13:30:00 +0530"),
            podcast.episodes[0].published
        );
    }
    #[test]
    fn atom_uses_publication_date_and_cover_candidates_have_safe_fallbacks() {
        let base = Url::parse("https://example.test/feed").unwrap();
        let atom = parse_feed(r#"<feed xmlns="http://www.w3.org/2005/Atom"><title>Studio</title><logo>/art.png</logo><entry><title>Published first</title><published>2024-01-01T00:00:00Z</published><updated>2025-01-01T00:00:00Z</updated><summary>Short notes</summary><link rel="enclosure" href="/first.ogg"/></entry><entry><title>Latest</title><published>2024-11-20T00:00:00Z</published><link rel="enclosure" type="audio/ogg" href="/latest.ogg"/></entry></feed>"#, &base).unwrap();
        assert_eq!(atom.episodes[0].title, "Latest");
        assert_eq!(atom.episodes[1].summary, "Short notes");
        assert_eq!(
            atom.show.image.as_deref(),
            Some("https://example.test/art.png")
        );
        let rss = parse_feed(r#"<rss xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd"><channel><itunes:image href="file:///private"/><image><url>/fallback.png</url></image></channel></rss>"#, &base).unwrap();
        assert_eq!(
            rss.show.image.as_deref(),
            Some("https://example.test/fallback.png")
        );
    }
    #[test]
    fn newest_entries_are_selected_before_applying_the_episode_limit() {
        let items: String = (0..2002).map(|index| format!("<item><title>Episode{index}</title><pubDate>{}</pubDate><enclosure url='/e{index}.mp3'/></item>", chrono::DateTime::from_timestamp(1_700_000_000 + index * 60, 0).unwrap().to_rfc3339())).collect();
        let parsed = parse_feed(
            &format!("<rss><channel>{items}</channel></rss>"),
            &Url::parse("https://example.test/feed").unwrap(),
        )
        .unwrap();
        assert_eq!(parsed.episodes.len(), 2000);
        assert_eq!(parsed.episodes[0].title, "Episode2001");
    }
    #[test]
    fn episode_metadata_cache_is_separate_from_audio_and_legacy_json_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let feed = "https://example.test/feed";
        let parsed = parse_feed("<rss><channel><title>Studio</title><item><title>Episode</title><description>Notes</description><enclosure url='/audio.mp3'/></item></channel></rss>", &Url::parse(feed).unwrap()).unwrap();
        cache_podcast(feed, &parsed, dir.path()).unwrap();
        let cached = cached_podcast_in(feed, dir.path()).unwrap();
        assert_eq!(cached.episodes[0].summary, "Notes");
        let entries: Vec<_> = fs::read_dir(dir.path()).unwrap().flatten().collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path().extension().unwrap(), "json");
        let episode: Episode =
            serde_json::from_str(r#"{"title":"Old","url":"https://example.test/old.mp3"}"#)
                .unwrap();
        assert!(episode.summary.is_empty());
        assert!(episode.published.is_none());
        let library: crate::state::Library = serde_json::from_str(r#"{"feeds":["https://example.test/feed"],"podcast_shows":{"https://example.test/feed":{"title":"Old show","publisher":"Radio","summary":"Old synopsis"}},"episodes":{"Downloaded":"/tmp/old.mp3"}}"#).unwrap();
        assert!(library.podcast_shows[feed].image.is_none());
        assert_eq!(
            library.episodes["Downloaded"],
            PathBuf::from("/tmp/old.mp3")
        );
    }
    #[test]
    fn downloads_feed_episode_and_reuses_it_offline() {
        use std::{
            io::{BufRead, BufReader, Write},
            net::TcpListener,
        };
        let server = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let url = format!("http://{}/feed", server.local_addr().unwrap());
        let worker = std::thread::spawn(move || {
            for body in [r#"<rss><channel><item><title>Fixture</title><enclosure url="/episode.mp3"/></item></channel></rss>"#.as_bytes(),b"fixture audio"]{
                let(mut stream,_)=server.accept().unwrap();let mut headers=BufReader::new(&stream);let mut line=String::new();while headers.read_line(&mut line).unwrap()>0&&!line.trim_end().is_empty(){line.clear();}
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).unwrap();stream.write_all(body).unwrap();
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let episodes = podcast(&url, dir.path()).unwrap().episodes;
        let path = download(&episodes[0], dir.path()).unwrap();
        worker.join().unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"fixture audio");
        assert_eq!(download(&episodes[0], dir.path()).unwrap(), path);
    }
    #[test]
    fn rss_and_atom_enclosures_are_parsed_and_deduplicated() {
        let base = Url::parse("https://example.test/feed.xml").unwrap();
        let episodes = parse_feed(r#"<rss><channel><item><title>A &amp; B</title><enclosure url="/a.mp3"/></item><item><enclosure url="/a.mp3"/></item><item><enclosure url="file:///tmp/private"/></item></channel></rss>"#, &base).unwrap().episodes;
        assert_eq!(episodes.len(), 1);
        assert_eq!(episodes[0].title, "A & B");
        assert_eq!(episodes[0].url, "https://example.test/a.mp3");
        let atom = parse_feed(r#"<feed xmlns="http://www.w3.org/2005/Atom"><entry><title>Episode</title><link rel="enclosure" href="https://example.test/b.ogg"/></entry></feed>"#, &base).unwrap().episodes;
        assert_eq!(atom[0].url, "https://example.test/b.ogg");
        assert!(parse_feed("broken", &base).is_err());
    }
    #[test]
    fn show_metadata_is_separate_from_episodes_and_empty_feeds_are_valid() {
        let base = Url::parse("https://example.test/rss").unwrap();
        let parsed = parse_feed(r#"<rss xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd"><channel><title>Quiet Studio</title><itunes:author>Creative Radio</itunes:author><description>&lt;p&gt;Art &amp;amp; sound&lt;/p&gt;</description><item><title>Episode title</title><description>Do not use this episode summary</description></item></channel></rss>"#, &base).unwrap();
        assert_eq!(parsed.show.title, "Quiet Studio");
        assert_eq!(parsed.show.publisher, "Creative Radio");
        assert_eq!(parsed.show.summary, "Art & sound");
        assert!(parsed.episodes.is_empty());
        let atom = parse_feed(r#"<feed xmlns="http://www.w3.org/2005/Atom"><title>Sketchbook</title><author><name>Jane</name></author><subtitle>Drawing notes</subtitle></feed>"#, &base).unwrap();
        assert_eq!(atom.show.publisher, "Jane");
        assert_eq!(atom.show.summary, "Drawing notes");
        assert!(parse_feed("<html><title>Not a feed</title></html>", &base).is_err());
    }
    #[test]
    fn rdf_channel_metadata_and_sibling_episodes_are_preserved() {
        let base = Url::parse("https://example.test/rss").unwrap();
        let parsed = parse_feed(r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns="http://purl.org/rss/1.0/"><channel><title>Legacy radio</title></channel><item><title>Archive</title><enclosure url="/archive.mp3"/></item></rdf:RDF>"#, &base).unwrap();
        assert_eq!(parsed.show.title, "Legacy radio");
        assert_eq!(parsed.episodes[0].title, "Archive");
    }
    #[test]
    fn discovers_videos_once_and_leaves_standalone_images_out_of_the_collection() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("photo.JPG"), b"photo").unwrap();
        fs::write(dir.path().join("clip.webm"), b"video").unwrap();
        fs::write(dir.path().join("song.mp3"), b"audio").unwrap();
        let clip = dir.path().join("clip.webm");
        let found = assets(&[
            dir.path().into(),
            clip.clone(),
            dir.path().join("photo.JPG"),
        ]);
        assert_eq!(found, [fs::canonicalize(clip).unwrap()]);
    }
    #[test]
    fn credential_and_local_file_urls_are_rejected() {
        assert!(http_url("file:///tmp/a").is_err());
        assert!(http_url("https://user:pass@example.test/feed").is_err());
    }
}
