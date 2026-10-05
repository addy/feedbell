//! Fetching and parsing feeds, plus URL normalization and the dedupe key.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::StatusCode;
use reqwest::Url;
use reqwest::blocking::Client;
use reqwest::header::{ETAG, HeaderMap, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};
use sha2::{Digest, Sha256};

const USER_AGENT: &str = concat!(
    "feedbell/",
    env!("CARGO_PKG_VERSION"),
    " (macOS feed notifier)"
);
const TIMEOUT: Duration = Duration::from_secs(20);

/// One entry of a feed, reduced to what feedbell stores and shows.
#[derive(Debug, PartialEq)]
pub struct Item {
    /// Dedupe key; see [`item_key`].
    pub key: String,
    pub title: Option<String>,
    pub link: Option<String>,
}

#[derive(Debug)]
pub struct ParsedFeed {
    pub title: Option<String>,
    pub items: Vec<Item>,
}

/// A successful fetch: the body and the validators to store for conditional GETs.
pub struct Fetched {
    pub body: Vec<u8>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

/// Canonical form of a feed URL, so the same feed is not subscribed twice. Assumes `https://`
/// when no scheme is given, lowercases the scheme and host, and drops any fragment.
pub fn normalize_url(input: &str) -> Result<String> {
    let input = input.trim();
    if input.is_empty() {
        bail!("the URL is empty");
    }
    let with_scheme = if input.contains("://") {
        input.to_string()
    } else {
        format!("https://{input}")
    };
    let mut url =
        Url::parse(&with_scheme).with_context(|| format!("'{input}' is not a valid URL"))?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!(
            "only http and https feeds are supported, not {}",
            url.scheme()
        );
    }
    if url.host_str().is_none_or(str::is_empty) {
        bail!("'{input}' has no host");
    }
    url.set_fragment(None);
    Ok(url.into())
}

/// The host of a normalized URL, used as a feed name when the feed has no title.
pub fn host_of(url: &str) -> Option<String> {
    Url::parse(url).ok()?.host_str().map(str::to_string)
}

pub fn http_client() -> Result<Client> {
    Client::builder()
        .user_agent(USER_AGENT)
        .timeout(TIMEOUT)
        .build()
        .context("building the HTTP client")
}

pub enum FetchOutcome {
    /// The server answered 304: nothing changed since the stored validators.
    NotModified,
    Fetched(Fetched),
}

/// GET a feed, conditionally when validators from an earlier fetch are given.
pub fn fetch(
    client: &Client,
    url: &str,
    etag: Option<&str>,
    last_modified: Option<&str>,
) -> Result<FetchOutcome> {
    let mut request = client.get(url);
    if let Some(etag) = etag {
        request = request.header(IF_NONE_MATCH, etag);
    }
    if let Some(last_modified) = last_modified {
        request = request.header(IF_MODIFIED_SINCE, last_modified);
    }
    let response = request.send().with_context(|| format!("fetching {url}"))?;
    if response.status() == StatusCode::NOT_MODIFIED {
        return Ok(FetchOutcome::NotModified);
    }
    let response = response.error_for_status()?;
    let etag = header(response.headers(), ETAG);
    let last_modified = header(response.headers(), LAST_MODIFIED);
    let body = response
        .bytes()
        .with_context(|| format!("reading the response from {url}"))?
        .to_vec();
    Ok(FetchOutcome::Fetched(Fetched {
        body,
        etag,
        last_modified,
    }))
}

fn header(headers: &HeaderMap, name: reqwest::header::HeaderName) -> Option<String> {
    Some(headers.get(name)?.to_str().ok()?.to_string())
}

pub fn parse(body: &[u8]) -> Result<ParsedFeed> {
    // feed-rs invents an id for entries that have none, and for an entry without a link that
    // id is random, so it would look new on every poll. Leave missing ids empty instead and
    // let `item_key` derive a stable one.
    let parser = feed_rs::parser::Builder::new()
        .id_generator(|_, _, _| String::new())
        .build();
    let feed = parser
        .parse(body)
        .context("this does not look like an RSS, Atom, or JSON feed")?;

    let items = feed
        .entries
        .into_iter()
        .map(|entry| {
            let title = entry.title.and_then(|t| non_empty(&t.content));
            let link = entry.links.first().and_then(|l| non_empty(&l.href));
            Item {
                key: item_key(&entry.id, link.as_deref(), title.as_deref()),
                title,
                link,
            }
        })
        .collect();
    Ok(ParsedFeed {
        title: feed.title.and_then(|t| non_empty(&t.content)),
        items,
    })
}

/// The dedupe key for an entry: its id/GUID, or when that is missing or blank, a SHA-256 hash
/// of its link and title.
pub fn item_key(id: &str, link: Option<&str>, title: Option<&str>) -> String {
    let id = id.trim();
    if !id.is_empty() {
        return id.to_string();
    }
    let mut hasher = Sha256::new();
    hasher.update(link.unwrap_or_default());
    // Separator, so ("ab", "c") and ("a", "bc") hash differently.
    hasher.update([0]);
    hasher.update(title.unwrap_or_default());
    let hex: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("sha256:{hex}")
}

fn non_empty(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RSS2: &[u8] = include_bytes!("../tests/fixtures/rss2.xml");
    const ATOM: &[u8] = include_bytes!("../tests/fixtures/atom.xml");
    const MISSING_GUIDS: &[u8] = include_bytes!("../tests/fixtures/missing_guids.xml");

    #[test]
    fn parses_rss2() {
        let feed = parse(RSS2).unwrap();
        assert_eq!(feed.title.as_deref(), Some("Example RSS Blog"));
        assert_eq!(
            feed.items,
            vec![
                Item {
                    key: "post-2".into(),
                    title: Some("Second post".into()),
                    link: Some("https://example.com/posts/second".into()),
                },
                Item {
                    key: "https://example.com/posts/first".into(),
                    title: Some("First post".into()),
                    link: Some("https://example.com/posts/first".into()),
                },
            ]
        );
    }

    #[test]
    fn parses_atom() {
        let feed = parse(ATOM).unwrap();
        assert_eq!(feed.title.as_deref(), Some("Example Atom Feed"));
        assert_eq!(
            feed.items,
            vec![
                Item {
                    key: "urn:uuid:entry-2".into(),
                    title: Some("Atom entry two".into()),
                    link: Some("https://example.org/entries/2".into()),
                },
                Item {
                    key: "urn:uuid:entry-1".into(),
                    title: Some("Atom entry one".into()),
                    link: Some("https://example.org/entries/1".into()),
                },
            ]
        );
    }

    #[test]
    fn missing_guids_fall_back_to_a_hash() {
        let items = parse(MISSING_GUIDS).unwrap().items;
        assert_eq!(items.len(), 4);

        // No <guid>, an empty <guid>, and no <guid> or <link>: all hashed.
        assert_eq!(
            items[0].key,
            item_key(
                "",
                Some("https://noguid.example/a"),
                Some("Has link and title")
            )
        );
        assert_eq!(
            items[1].key,
            item_key(
                "",
                Some("https://noguid.example/b"),
                Some("Empty guid element")
            )
        );
        assert_eq!(items[2].link, None);
        assert_eq!(items[2].key, item_key("", None, Some("Title only")));
        for item in &items[..3] {
            assert!(item.key.starts_with("sha256:"), "{}", item.key);
        }

        // A real GUID in the same feed is still used as is.
        assert_eq!(items[3].key, "real-guid-d");
    }

    #[test]
    fn keys_are_stable_across_parses() {
        let first = parse(MISSING_GUIDS).unwrap().items;
        let second = parse(MISSING_GUIDS).unwrap().items;
        assert_eq!(first, second);
    }

    #[test]
    fn rejects_non_feeds() {
        assert!(parse(b"<html><body>Not a feed</body></html>").is_err());
        assert!(parse(b"").is_err());
    }

    #[test]
    fn item_key_prefers_the_id() {
        assert_eq!(item_key("guid-1", Some("https://x/"), Some("T")), "guid-1");
        assert_eq!(item_key("  guid-1\n", None, None), "guid-1");
    }

    #[test]
    fn item_key_hashes_link_and_title_when_id_is_blank() {
        let key = item_key("", Some("https://x/a"), Some("Title"));
        assert_eq!(key.len(), "sha256:".len() + 64);
        assert_eq!(key, item_key("   ", Some("https://x/a"), Some("Title")));
        assert_ne!(key, item_key("", Some("https://x/b"), Some("Title")));
        assert_ne!(key, item_key("", Some("https://x/a"), Some("Other")));
        // The link/title boundary matters.
        assert_ne!(
            item_key("", Some("ab"), Some("c")),
            item_key("", Some("a"), Some("bc"))
        );
    }

    #[test]
    fn item_key_matches_known_sha256() {
        // sha256 of a single NUL byte: empty link, separator, empty title.
        assert_eq!(
            item_key("", None, None),
            "sha256:6e340b9cffb37a989ca544e6bb780a2c78901d3fb33738768511a30617afa01d"
        );
    }

    #[test]
    fn normalize_url_canonicalizes() {
        let cases = [
            ("example.com/feed", "https://example.com/feed"),
            (
                "  https://Example.COM/Feed.xml  ",
                "https://example.com/Feed.xml",
            ),
            ("HTTP://example.com", "http://example.com/"),
            (
                "https://example.com/feed#latest",
                "https://example.com/feed",
            ),
            (
                "https://example.com/feed?format=rss",
                "https://example.com/feed?format=rss",
            ),
            ("localhost:8080/atom.xml", "https://localhost:8080/atom.xml"),
        ];
        for (input, expected) in cases {
            assert_eq!(normalize_url(input).unwrap(), expected, "input: {input}");
        }
    }

    #[test]
    fn normalize_url_rejects_bad_input() {
        for input in ["", "   ", "ftp://example.com/feed", "My Blog", "https://"] {
            assert!(normalize_url(input).is_err(), "input: {input:?}");
        }
    }

    #[test]
    fn host_of_normalized_url() {
        assert_eq!(
            host_of("https://example.com/feed").as_deref(),
            Some("example.com")
        );
    }
}
