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

#[derive(Clone, Debug)]
pub struct Episode {
    pub title: String,
    pub url: String,
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
                        "jpg"
                            | "jpeg"
                            | "png"
                            | "webp"
                            | "gif"
                            | "bmp"
                            | "tif"
                            | "tiff"
                            | "avif"
                            | "heic"
                            | "mp4"
                            | "m4v"
                            | "mkv"
                            | "webm"
                            | "mov"
                            | "avi"
                            | "ogv"
                            | "mpeg"
                            | "mpg"
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

pub fn podcast(feed: &str, dir: &Path) -> Result<Vec<Episode>, String> {
    let url = http_url(feed)?;
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let temporary = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
    fetch(url.as_str(), temporary.path(), "8388608", "30")?;
    let xml = fs::read_to_string(temporary.path()).map_err(|e| e.to_string())?;
    parse_feed(&xml, &url)
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

fn http_url(value: &str) -> Result<Url, String> {
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

fn fetch(url: &str, path: &Path, limit: &str, seconds: &str) -> Result<(), String> {
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
            "Gnune/0.1",
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

fn parse_feed(xml: &str, base: &Url) -> Result<Vec<Episode>, String> {
    let document =
        roxmltree::Document::parse(xml).map_err(|e| format!("Invalid podcast feed: {e}"))?;
    let mut seen = BTreeSet::new();
    let mut episodes = Vec::new();
    for item in document
        .descendants()
        .filter(|n| n.is_element() && matches!(n.tag_name().name(), "item" | "entry"))
    {
        let enclosure = item.children().find(|n| {
            n.is_element()
                && (n.tag_name().name() == "enclosure"
                    || (n.tag_name().name() == "link" && n.attribute("rel") == Some("enclosure")))
        });
        let Some(enclosure) = enclosure else {
            continue;
        };
        let Some(target) = enclosure
            .attribute("url")
            .or_else(|| enclosure.attribute("href"))
        else {
            continue;
        };
        let Ok(url) = base.join(target).and_then(|url| {
            http_url(url.as_str()).map_err(|_| url::ParseError::RelativeUrlWithoutBase)
        }) else {
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
        episodes.push(Episode {
            title: title.trim().to_owned(),
            url: url.into(),
        });
    }
    if episodes.is_empty() {
        return Err("No playable episode enclosures were found in this feed".into());
    }
    Ok(episodes)
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
        let episodes = podcast(&url, dir.path()).unwrap();
        let path = download(&episodes[0], dir.path()).unwrap();
        worker.join().unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"fixture audio");
        assert_eq!(download(&episodes[0], dir.path()).unwrap(), path);
    }
    #[test]
    fn rss_and_atom_enclosures_are_parsed_and_deduplicated() {
        let base = Url::parse("https://example.test/feed.xml").unwrap();
        let episodes = parse_feed(r#"<rss><channel><item><title>A &amp; B</title><enclosure url="/a.mp3"/></item><item><enclosure url="/a.mp3"/></item><item><enclosure url="file:///tmp/private"/></item></channel></rss>"#, &base).unwrap();
        assert_eq!(episodes.len(), 1);
        assert_eq!(episodes[0].title, "A & B");
        assert_eq!(episodes[0].url, "https://example.test/a.mp3");
        let atom = parse_feed(r#"<feed xmlns="http://www.w3.org/2005/Atom"><entry><title>Episode</title><link rel="enclosure" href="https://example.test/b.ogg"/></entry></feed>"#, &base).unwrap();
        assert_eq!(atom[0].url, "https://example.test/b.ogg");
        assert!(parse_feed("broken", &base).is_err());
    }
    #[test]
    fn discovers_only_assets_and_deduplicates_overlapping_roots() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("photo.JPG"), b"photo").unwrap();
        fs::write(dir.path().join("clip.webm"), b"video").unwrap();
        fs::write(dir.path().join("song.mp3"), b"audio").unwrap();
        let found = assets(&[dir.path().into(), dir.path().join("photo.JPG")]);
        assert_eq!(found.len(), 2);
    }
    #[test]
    fn credential_and_local_file_urls_are_rejected() {
        assert!(http_url("file:///tmp/a").is_err());
        assert!(http_url("https://user:pass@example.test/feed").is_err());
    }
}
