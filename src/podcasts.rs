//! Public podcast catalog and OPML discovery. Listing shows never subscribes to them or fetches their feeds.
use crate::media::{self, PodcastShow};
use std::{
    collections::HashSet,
    fs,
    io::{BufReader, Read},
    path::Path,
    rc::Rc,
    time::Duration,
};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EpisodeId {
    pub feed_url: String,
    pub episode_url: String,
}
impl EpisodeId {
    pub fn new(feed: &str, episode: &str) -> Result<Self, String> {
        Ok(Self {
            feed_url: normalize_url(feed)?,
            episode_url: normalize_url(episode)?,
        })
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpisodeOrder {
    OldestFirst,
    #[default]
    #[serde(other)]
    NewestFirst,
}
#[derive(Clone, Copy, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct EpisodeProgress {
    pub position_secs: f64,
    pub completed: bool,
}
pub fn progress_update(
    previous: Option<EpisodeProgress>,
    position: f64,
    completed: bool,
    paused: bool,
    explicit: bool,
) -> Option<EpisodeProgress> {
    if !position.is_finite() || (!explicit && paused && !completed) {
        return None;
    }
    let position = position.max(0.);
    if !explicit
        && previous.is_some_and(|saved| {
            (saved.position_secs - position).abs() < 0.05 && saved.completed == completed
        })
    {
        return None;
    }
    Some(EpisodeProgress {
        position_secs: position,
        completed,
    })
}

pub fn ordered_episodes(episodes: &[media::Episode], order: EpisodeOrder) -> Vec<media::Episode> {
    let mut episodes = episodes.to_vec();
    episodes.sort_by(|a, b| match (a.published, b.published) {
        (Some(a), Some(b)) => {
            if order == EpisodeOrder::OldestFirst {
                a.cmp(&b)
            } else {
                b.cmp(&a)
            }
        }
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    episodes
}
pub fn resume_episode(
    episodes: &[media::Episode],
    last: &str,
    progress: &std::collections::BTreeMap<String, EpisodeProgress>,
) -> Option<(usize, f64)> {
    let saved_position = |index: usize| {
        progress.get(&episodes[index].url).map_or(0., |saved| {
            if saved.position_secs.is_finite() {
                saved.position_secs.max(0.)
            } else {
                0.
            }
        })
    };
    let position = episodes.iter().position(|episode| episode.url == last);
    if let Some(index) = position {
        let saved = progress.get(last).copied().unwrap_or_default();
        if !saved.completed {
            return Some((
                index,
                if saved.position_secs.is_finite() {
                    saved.position_secs.max(0.)
                } else {
                    0.
                },
            ));
        }
        if let Some(next) = (index + 1..episodes.len()).find(|index| {
            !progress
                .get(&episodes[*index].url)
                .is_some_and(|saved| saved.completed)
        }) {
            return Some((next, saved_position(next)));
        }
    }
    episodes
        .iter()
        .position(|episode| {
            !progress
                .get(&episode.url)
                .is_some_and(|saved| saved.completed)
        })
        .map(|index| (index, saved_position(index)))
}

pub const RECOMMENDATIONS: &str = "https://public.podcastindex.org/recommendations.json";
pub const BBC: &str = "https://www.bbc.co.uk/radio/opml/bbc_podcast_opml.xml";
pub const STARTER: &str = "Bundled starter list";
const LIST_LIMIT: u64 = 2 * 1024 * 1024;
const MAX_SHOWS: usize = 2000;

#[derive(Clone, Debug)]
pub struct Listing {
    pub url: String,
    pub show: PodcastShow,
}
#[derive(Debug)]
pub struct Catalog {
    pub shows: Vec<Listing>,
    pub skipped: usize,
    pub truncated: bool,
    pub note: String,
}
#[derive(Default)]
pub struct Browse {
    pub opened: bool,
    pub source: String,
    pub query: String,
    pub shows: Vec<Rc<Listing>>,
    pub message: String,
    pub scanning: bool,
    pub revision: u64,
    pub view: Option<Rc<crate::collection_media::PodcastBrowser>>,
    pub feed_errors: std::collections::HashMap<String, String>,
    pub read: HashSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResultKey {
    pub revision: u64,
    pub query: String,
}

pub fn normalize_url(value: &str) -> Result<String, String> {
    let mut url = media::http_url(value.trim())?;
    url.set_fragment(None);
    if url.host_str() == Some("podcasts.files.bbci.co.uk") && url.scheme() == "http" {
        let _ = url.set_scheme("https");
    }
    Ok(url.into())
}

/// Revoke queued/in-flight intentions so a late read cannot resubscribe or autoplay.
pub fn cancel_feed_requests(
    feed: &str,
    subscriptions: &mut HashSet<String>,
    queue: &mut std::collections::VecDeque<(String, bool, bool)>,
    latest: &mut Option<String>,
) {
    let Ok(feed) = normalize_url(feed) else {
        return;
    };
    let same = |candidate: &str| normalize_url(candidate).is_ok_and(|candidate| candidate == feed);
    subscriptions.retain(|candidate| !same(candidate));
    queue.retain(|(candidate, _, _)| !same(candidate));
    if latest.as_deref().is_some_and(same) {
        latest.take();
    }
}

pub fn contains_feed(feeds: &[String], value: &str) -> bool {
    let Ok(value) = normalize_url(value) else {
        return false;
    };
    feeds
        .iter()
        .any(|feed| normalize_url(feed).is_ok_and(|feed| feed == value))
}

pub fn matches(listing: &Listing, query: &str) -> bool {
    if query.trim().is_empty() {
        return true;
    }
    let text = format!(
        "{} {} {} {}",
        listing.show.title, listing.show.publisher, listing.show.summary, listing.url
    )
    .to_lowercase();
    query
        .split_whitespace()
        .all(|word| text.contains(&word.to_lowercase()))
}

pub fn search_rank(listing: &Listing, query: &str) -> u8 {
    let query = query.trim().to_lowercase();
    let title = listing.show.title.to_lowercase();
    if title == query {
        0
    } else if title.starts_with(&query) {
        1
    } else if title.contains(&query) {
        2
    } else if query.split_whitespace().all(|word| title.contains(word)) {
        3
    } else {
        4
    }
}

pub fn scan(source: &str) -> Result<Catalog, String> {
    if source == RECOMMENDATIONS {
        return recommendations(
            &gtk::glib::user_cache_dir().join("gnune"),
            Duration::from_secs(86400),
            |path| media::fetch(RECOMMENDATIONS, path, "67108864", "45"),
        );
    }
    if source == STARTER {
        return parse_opml(include_str!("podcasts/starter.opml"), None);
    }
    let base = if source.starts_with("https://") || source.starts_with("http://") {
        Some(media::http_url(source)?)
    } else {
        None
    };
    let temporary;
    let path = if let Some(url) = &base {
        temporary = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
        media::fetch(url.as_str(), temporary.path(), "2097152", "20")?;
        temporary.path()
    } else {
        Path::new(source)
    };
    if !fs::metadata(path)
        .map_err(|e| format!("Could not open feed list: {e}"))?
        .is_file()
    {
        return Err("Choose an OPML file".into());
    }
    let file = fs::File::open(path).map_err(|e| format!("Could not open feed list: {e}"))?;
    let mut bytes = Vec::new();
    file.take(LIST_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > LIST_LIMIT {
        return Err("Feed lists must be smaller than 2 MiB".into());
    }
    let xml = String::from_utf8(bytes).map_err(|_| "The feed list must use UTF-8")?;
    parse_opml(&xml, base.as_ref())
}

const RECOMMENDATION_LIMIT: u64 = 64 * 1024 * 1024;

#[derive(serde::Deserialize)]
struct Recommendation {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    image: Option<String>,
}

fn parse_recommendations(path: &Path) -> Result<Catalog, String> {
    let file = fs::File::open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() > RECOMMENDATION_LIMIT {
        return Err("The recommendations list exceeds 64 MiB".into());
    }
    let reader = BufReader::new(file.take(RECOMMENDATION_LIMIT + 1));
    let entries: Vec<Recommendation> = serde_json::from_reader(reader)
        .map_err(|e| format!("Invalid Podcast Index recommendations: {e}"))?;
    let mut catalog = Catalog {
        shows: Vec::new(),
        skipped: 0,
        truncated: false,
        note: String::new(),
    };
    let mut seen = HashSet::new();
    for entry in entries {
        let Ok(url) = normalize_url(&entry.url) else {
            catalog.skipped += 1;
            continue;
        };
        if !seen.insert(url.clone()) {
            catalog.skipped += 1;
            continue;
        }
        if catalog.shows.len() == 100_000 {
            catalog.truncated = true;
            break;
        }
        catalog.shows.push(Listing {
            url: url.clone(),
            show: PodcastShow {
                title: if entry.title.trim().is_empty() {
                    url
                } else {
                    entry.title.chars().take(500).collect()
                },
                publisher: String::new(),
                summary: String::new(),
                image: entry
                    .image
                    .and_then(|image| media::http_url(&image).ok())
                    .map(Into::into),
            },
        });
    }
    if catalog.shows.is_empty() {
        return Err("Podcast Index returned no usable recommendations".into());
    }
    Ok(catalog)
}

fn recommendations(
    cache_dir: &Path,
    max_age: Duration,
    fetch: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<Catalog, String> {
    let cache = cache_dir.join("podcast-recommendations.json");
    let cached = parse_recommendations(&cache).ok();
    let fresh = fs::metadata(&cache)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < max_age);
    if fresh && let Some(mut catalog) = cached {
        catalog.note = "Using the daily cached Podcast Index recommendations.".into();
        return Ok(catalog);
    }
    let refresh = || -> Result<Catalog, String> {
        fs::create_dir_all(cache_dir).map_err(|e| e.to_string())?;
        let temporary = tempfile::NamedTempFile::new_in(cache_dir).map_err(|e| e.to_string())?;
        fetch(temporary.path())?;
        let catalog = parse_recommendations(temporary.path())?;
        temporary.as_file().sync_all().map_err(|e| e.to_string())?;
        temporary.persist(&cache).map_err(|e| e.error.to_string())?;
        Ok(catalog)
    };
    match refresh() {
        Ok(catalog) => Ok(catalog),
        Err(error) => {
            let mut catalog = if let Some(mut catalog) = cached {
                catalog.note = format!(
                    "Could not refresh recommendations ({error}). Browsing the last saved list."
                );
                catalog
            } else {
                let mut catalog = parse_opml(include_str!("podcasts/starter.opml"), None)?;
                catalog.note = format!(
                    "Could not load recommendations ({error}). Showing bundled starter picks; try again or search the full directory."
                );
                catalog
            };
            catalog.note = catalog.note.chars().take(1200).collect();
            Ok(catalog)
        }
    }
}

/// Search the public directory when the listener submits a query.
pub fn search_directory(query: &str) -> Result<Catalog, String> {
    let mut url = url::Url::parse("https://p0dcasters.com/api/search").unwrap();
    url.query_pairs_mut()
        .extend_pairs([("q", query), ("limit", "60")]);
    let temporary = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
    media::fetch(url.as_str(), temporary.path(), "2097152", "20")?;
    parse_directory(&fs::read_to_string(temporary.path()).map_err(|e| e.to_string())?)
}

fn parse_directory(json: &str) -> Result<Catalog, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("Invalid directory response: {e}"))?;
    let entries = value["shows"]
        .as_array()
        .ok_or("The directory returned no results list")?;
    let mut seen = HashSet::new();
    let mut catalog = Catalog {
        shows: Vec::new(),
        skipped: 0,
        truncated: false,
        note: String::new(),
    };
    for entry in entries.iter().take(60) {
        let Some(url) = entry["feedUrl"]
            .as_str()
            .and_then(|s| normalize_url(s).ok())
        else {
            catalog.skipped += 1;
            continue;
        };
        if !seen.insert(url.clone()) {
            catalog.skipped += 1;
            continue;
        }
        catalog.shows.push(Listing {
            url: url.clone(),
            show: PodcastShow {
                title: entry["title"]
                    .as_str()
                    .unwrap_or(&url)
                    .chars()
                    .take(500)
                    .collect(),
                publisher: entry["author"]
                    .as_str()
                    .unwrap_or_default()
                    .chars()
                    .take(500)
                    .collect(),
                summary: media::plain_summary(entry["description"].as_str().unwrap_or_default()),
                image: entry["image"]
                    .as_str()
                    .and_then(|value| media::http_url(value).ok())
                    .map(Into::into),
            },
        });
    }
    Ok(catalog)
}

fn parse_opml(xml: &str, base: Option<&url::Url>) -> Result<Catalog, String> {
    let document =
        roxmltree::Document::parse(xml).map_err(|e| format!("Invalid OPML feed list: {e}"))?;
    if document.root_element().tag_name().name() != "opml" {
        return Err(
            "This is not an OPML feed list. Use Preview RSS / Atom for a single feed.".into(),
        );
    }
    let mut catalog = Catalog {
        shows: Vec::new(),
        skipped: 0,
        truncated: false,
        note: String::new(),
    };
    let mut seen = HashSet::new();
    for node in document
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "outline")
    {
        let Some(value) = node
            .attribute("xmlUrl")
            .or_else(|| node.attribute("xmlurl"))
        else {
            continue;
        };
        let value = base
            .and_then(|base| base.join(value).ok())
            .map(|url| url.to_string())
            .unwrap_or_else(|| value.to_owned());
        let Ok(url) = normalize_url(&value) else {
            catalog.skipped += 1;
            continue;
        };
        if !seen.insert(url.clone()) {
            catalog.skipped += 1;
            continue;
        }
        if catalog.shows.len() == MAX_SHOWS {
            catalog.truncated = true;
            break;
        }
        let title = node
            .attribute("title")
            .or_else(|| node.attribute("text"))
            .unwrap_or(&url)
            .trim()
            .to_owned();
        let publisher = node
            .ancestors()
            .skip(1)
            .find_map(|n| n.attribute("text"))
            .unwrap_or_default()
            .to_owned();
        catalog.shows.push(Listing {
            url,
            show: PodcastShow {
                title,
                publisher,
                summary: media::plain_summary(node.attribute("description").unwrap_or_default()),
                image: None,
            },
        });
    }
    catalog
        .shows
        .sort_by_key(|show| show.show.title.to_lowercase());
    Ok(catalog)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn episode_order_and_completed_resume_share_stable_url_identity() {
        let episode = |url: &str, published| media::Episode {
            url: url.into(),
            title: url.into(),
            published,
            ..Default::default()
        };
        let original = vec![
            episode("new", Some(30)),
            episode("old", Some(10)),
            episode("middle", Some(20)),
            episode("undated", None),
        ];
        let oldest = ordered_episodes(&original, EpisodeOrder::OldestFirst);
        assert_eq!(
            oldest
                .iter()
                .map(|episode| episode.url.as_str())
                .collect::<Vec<_>>(),
            ["old", "middle", "new", "undated"]
        );
        let newest = ordered_episodes(&original, EpisodeOrder::NewestFirst);
        assert_eq!(
            newest
                .iter()
                .map(|episode| episode.url.as_str())
                .collect::<Vec<_>>(),
            ["new", "middle", "old", "undated"]
        );
        let mut progress = std::collections::BTreeMap::from([(
            "old".into(),
            EpisodeProgress {
                position_secs: 42.,
                completed: false,
            },
        )]);
        assert_eq!(resume_episode(&oldest, "old", &progress), Some((0, 42.)));
        assert_eq!(resume_episode(&newest, "old", &progress), Some((2, 42.)));
        progress.insert(
            "middle".into(),
            EpisodeProgress {
                position_secs: 17.,
                completed: false,
            },
        );
        progress.get_mut("old").unwrap().completed = true;
        assert_eq!(resume_episode(&oldest, "old", &progress), Some((1, 17.)));
        for episode in &oldest {
            progress.insert(
                episode.url.clone(),
                EpisodeProgress {
                    completed: true,
                    ..Default::default()
                },
            );
        }
        assert_eq!(resume_episode(&oldest, "old", &progress), None);
    }
    #[test]
    fn paused_or_unchanged_desktop_snapshots_do_not_overwrite_phone_progress() {
        let previous = EpisodeProgress {
            position_secs: 20.,
            completed: false,
        };
        assert!(progress_update(Some(previous), 20.2, false, true, false).is_none());
        assert!(progress_update(Some(previous), 20., false, false, false).is_none());
        assert_eq!(
            progress_update(Some(previous), 10., false, true, true)
                .unwrap()
                .position_secs,
            10.
        );
        let finished = progress_update(Some(previous), 120., true, true, false).unwrap();
        assert!(finished.completed);
        assert!(progress_update(Some(finished), 120., true, true, false).is_none());
        assert!(progress_update(None, f64::NAN, false, false, true).is_none());
    }

    #[test]
    fn unsubscribe_revokes_pending_subscribe_reads_and_latest_playback() {
        let feed = "https://example.test/feed";
        let mut pending = HashSet::from([
            "https://example.test:443/feed#alias".into(),
            "https://other.test/feed".into(),
        ]);
        let mut queue = std::collections::VecDeque::from([
            (feed.into(), true, false),
            (format!("{feed}#preview"), false, true),
            ("https://other.test/feed".into(), true, false),
        ]);
        let mut latest = Some(format!("{feed}#playing"));
        cancel_feed_requests(feed, &mut pending, &mut queue, &mut latest);
        // A late successful read now has neither membership nor autoplay intent.
        assert!(!pending.remove(feed));
        assert!(
            pending
                .iter()
                .all(|url| normalize_url(url).unwrap() != feed)
        );
        assert_eq!(queue, [("https://other.test/feed".into(), true, false)]);
        assert!(latest.is_none());
        pending.insert(feed.into());
        assert!(
            pending.remove(feed),
            "a new explicit Subscribe can still succeed"
        );
    }

    #[test]
    fn removing_one_show_keeps_a_newer_play_choice_and_other_subscriptions() {
        let mut pending = HashSet::from(["https://other.test/feed".into()]);
        let mut queue = std::collections::VecDeque::new();
        let mut latest = Some("https://other.test/feed".into());
        cancel_feed_requests(
            "https://example.test/feed",
            &mut pending,
            &mut queue,
            &mut latest,
        );
        assert_eq!(latest.as_deref(), Some("https://other.test/feed"));
        assert!(pending.contains("https://other.test/feed"));
        cancel_feed_requests("invalid", &mut pending, &mut queue, &mut latest);
        assert!(pending.contains("https://other.test/feed"));
    }

    const RECOMMENDATION_FIXTURE: &str = r#"[{"title":"First ranked show","url":"https://example.test/first","ignored":{"image":"large","sources":[1,2]}},{"title":"Second ranked show","url":"https://example.test/second"},{"title":"Duplicate","url":"https://example.test/first#duplicate"},{"url":"file:///tmp/private"}]"#;

    #[test]
    fn recommendations_cache_is_daily_atomic_and_survives_failed_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let first = recommendations(dir.path(), Duration::ZERO, |path| {
            fs::write(path, RECOMMENDATION_FIXTURE).map_err(|e| e.to_string())
        })
        .unwrap();
        assert_eq!(first.shows.len(), 2);
        assert_eq!(first.skipped, 2);
        assert_eq!(first.shows[0].show.title, "First ranked show");
        let cached = recommendations(dir.path(), Duration::from_secs(86400), |_| {
            panic!("fresh cache must not make a request")
        })
        .unwrap();
        assert_eq!(cached.shows.len(), 2);
        let failed =
            recommendations(dir.path(), Duration::ZERO, |_| Err("offline".into())).unwrap();
        assert_eq!(failed.shows.len(), 2);
        assert!(failed.note.contains("last saved list"));
        let invalid = recommendations(dir.path(), Duration::ZERO, |path| {
            fs::write(path, "broken").map_err(|e| e.to_string())
        })
        .unwrap();
        assert_eq!(invalid.shows.len(), 2);
        assert_eq!(
            fs::read_to_string(dir.path().join("podcast-recommendations.json")).unwrap(),
            RECOMMENDATION_FIXTURE
        );
    }
    #[test]
    fn missing_recommendation_cache_falls_back_to_bundled_shows() {
        let dir = tempfile::tempdir().unwrap();
        let catalog =
            recommendations(dir.path(), Duration::ZERO, |_| Err("offline".into())).unwrap();
        assert_eq!(catalog.shows.len(), scan(STARTER).unwrap().shows.len());
        assert!(catalog.note.contains("bundled starter picks"));
        assert!(!dir.path().join("podcast-recommendations.json").exists());
    }
    #[test]
    fn nested_opml_deduplicates_and_rejects_unsafe_feeds() {
        let catalog = parse_opml(r#"<opml><body><outline text="Science"><outline title="Space" xmlUrl="https://example.test/feed#x" description="&lt;p&gt;Moon &amp;amp; Mars&lt;/p&gt;"/><outline xmlUrl="https://example.test/feed"/><outline xmlUrl="file:///tmp/private"/><outline xmlUrl="https://a:b@example.test/feed"/></outline></body></opml>"#, None).unwrap();
        assert_eq!(catalog.shows.len(), 1);
        assert_eq!(catalog.skipped, 3);
        assert_eq!(catalog.shows[0].show.publisher, "Science");
        assert_eq!(catalog.shows[0].show.summary, "Moon & Mars");
        assert!(matches(&catalog.shows[0], "SPACE mars"));
        assert!(!matches(&catalog.shows[0], "unrelated"));
    }
    #[test]
    fn directory_results_require_safe_feed_urls_and_preserve_search_metadata() {
        let catalog = parse_directory(r#"{"shows":[{"title":"Drawing days","author":"Open Studio","description":"&lt;p&gt;Drawing &amp; tools&lt;/p&gt;","feedUrl":"https://example.test/art"},{"feedUrl":"https://example.test/art#duplicate"},{"feedUrl":"file:///tmp/private"},{"title":"Missing URL"}]}"#).unwrap();
        assert_eq!(catalog.shows.len(), 1);
        assert_eq!(catalog.skipped, 3);
        assert_eq!(catalog.shows[0].show.publisher, "Open Studio");
        assert_eq!(catalog.shows[0].show.summary, "Drawing & tools");
        assert!(matches(&catalog.shows[0], "drawing studio"));
        assert!(parse_directory("{}").is_err());
    }
    #[test]
    fn bbc_insecure_addresses_match_secure_starter_and_existing_subscriptions() {
        assert!(contains_feed(
            &["http://podcasts.files.bbci.co.uk/b006qykl.rss".into()],
            "https://podcasts.files.bbci.co.uk/b006qykl.rss"
        ));
        assert_eq!(
            normalize_url("http://example.test/feed").unwrap(),
            "http://example.test/feed"
        );
    }
    #[test]
    fn exact_show_titles_sort_before_description_matches() {
        let mut catalog = scan(STARTER).unwrap();
        let exact = catalog
            .shows
            .iter()
            .find(|s| s.show.title == "In Our Time")
            .unwrap()
            .clone();
        let other = &mut catalog.shows[0];
        other.show.title = "A different show".into();
        other.show.summary = "Conversations in our time".into();
        assert!(matches(other, "In Our Time"));
        assert!(search_rank(&exact, "In Our Time") < search_rank(other, "In Our Time"));
    }
    #[test]
    fn empty_lists_and_invalid_documents_are_distinct() {
        assert!(
            parse_opml("<opml><body/></opml>", None)
                .unwrap()
                .shows
                .is_empty()
        );
        assert!(parse_opml("<rss><channel/></rss>", None).is_err());
        assert!(parse_opml("invalid", None).is_err());
    }
    #[test]
    fn list_cap_and_relative_urls() {
        let xml = format!(
            "<opml><body>{}</body></opml>",
            (0..2002)
                .map(|i| format!("<outline xmlUrl='/feed/{i}'/>"))
                .collect::<String>()
        );
        let catalog = parse_opml(
            &xml,
            Some(&url::Url::parse("https://example.test/list").unwrap()),
        )
        .unwrap();
        assert_eq!(catalog.shows.len(), 2000);
        assert!(catalog.truncated);
        assert!(
            catalog.shows[0]
                .url
                .starts_with("https://example.test/feed/")
        );
    }
    #[test]
    fn local_list_scan_and_subscription_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("list.opml");
        fs::write(
            &path,
            "<opml><body><outline xmlUrl='https://example.test/feed'/></body></opml>",
        )
        .unwrap();
        assert_eq!(scan(path.to_str().unwrap()).unwrap().shows.len(), 1);
        assert!(contains_feed(
            &["https://example.test:443/feed#fragment".into()],
            "https://example.test/feed"
        ));
        assert!(!scan(STARTER).unwrap().shows.is_empty());
    }
}
