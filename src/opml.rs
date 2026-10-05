//! OPML import and export of the subscription list.

use anyhow::{Context, Result};
use quick_xml::events::Event;
use quick_xml::{Reader, XmlVersion};

use crate::store::Feed;

/// A feed listed in an OPML file.
#[derive(Debug, PartialEq)]
pub struct Outline {
    pub url: String,
    pub name: Option<String>,
}

/// Every `<outline>` with an `xmlUrl`, at any depth. Folders are flattened away.
pub fn parse(opml: &str) -> Result<Vec<Outline>> {
    let mut reader = Reader::from_str(opml);
    let mut outlines = Vec::new();
    loop {
        let element = match reader.read_event().context("this is not valid OPML")? {
            Event::Eof => break,
            Event::Start(element) | Event::Empty(element) => element,
            _ => continue,
        };
        if element.name().as_ref() != "outline" {
            continue;
        }
        let (mut url, mut text, mut title) = (None, None, None);
        for attribute in element.attributes() {
            let attribute = attribute.context("this is not valid OPML")?;
            let slot = match attribute.key.as_ref() {
                "xmlUrl" => &mut url,
                "text" => &mut text,
                "title" => &mut title,
                _ => continue,
            };
            let value = attribute
                .normalized_value(XmlVersion::Implicit1_0)
                .context("this is not valid OPML")?;
            let value = value.trim();
            if !value.is_empty() {
                *slot = Some(value.to_string());
            }
        }
        if let Some(url) = url {
            outlines.push(Outline {
                url,
                name: title.or(text),
            });
        }
    }
    Ok(outlines)
}

pub fn export(feeds: &[Feed]) -> String {
    let mut opml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <opml version=\"2.0\">\n  \
         <head>\n    <title>feedbell subscriptions</title>\n  </head>\n  \
         <body>\n",
    );
    for feed in feeds {
        let name = quick_xml::escape::escape(feed.name.as_str());
        let url = quick_xml::escape::escape(feed.url.as_str());
        opml.push_str(&format!(
            "    <outline type=\"rss\" text=\"{name}\" title=\"{name}\" xmlUrl=\"{url}\"/>\n"
        ));
    }
    opml.push_str("  </body>\n</opml>\n");
    opml
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUBSCRIPTIONS: &str = include_str!("../tests/fixtures/subscriptions.opml");

    fn outline(url: &str, name: Option<&str>) -> Outline {
        Outline {
            url: url.to_string(),
            name: name.map(str::to_string),
        }
    }

    #[test]
    fn parses_nested_outlines_and_skips_folders() {
        assert_eq!(
            parse(SUBSCRIPTIONS).unwrap(),
            [
                outline("https://example.com/feed.xml", Some("Example Blog")),
                outline("https://news.example/rss?a=1&b=2", Some("News & Views")),
                outline("https://text-only.example/atom", Some("Text only")),
                outline("https://nameless.example/feed", None),
            ]
        );
    }

    #[test]
    fn rejects_broken_xml() {
        assert!(parse("<opml><body><outline xmlUrl=\"x\"</body>").is_err());
    }

    #[test]
    fn a_file_without_feeds_is_empty() {
        assert!(parse("<opml><body/></opml>").unwrap().is_empty());
    }

    #[test]
    fn export_round_trips() {
        let feed = |name: &str, url: &str| Feed {
            id: 0,
            url: url.to_string(),
            name: name.to_string(),
            muted: false,
            etag: None,
            last_modified: None,
            last_checked: None,
            last_error: None,
        };
        let feeds = [
            feed("Tom & \"Jerry\" <b>", "https://example.com/feed?a=1&b=2"),
            feed("Plain", "https://plain.example/"),
        ];
        assert_eq!(
            parse(&export(&feeds)).unwrap(),
            [
                outline(
                    "https://example.com/feed?a=1&b=2",
                    Some("Tom & \"Jerry\" <b>")
                ),
                outline("https://plain.example/", Some("Plain")),
            ]
        );
    }
}
