//! One poll of one feed: fetch, notify about unseen items, record them as seen.

use std::collections::HashSet;

use anyhow::Result;
use reqwest::blocking::Client;

use crate::feed::{self, FetchOutcome, Item};
use crate::store::{Feed, Store};

/// Individual notifications per feed per poll; anything beyond this is rolled into one summary.
pub const MAX_NOTIFICATIONS: usize = 5;

/// Sends a notification with a title, a body, and a link to open when it is clicked.
pub type Notify<'a> = &'a mut dyn FnMut(&str, &str, Option<&str>) -> Result<()>;

/// Poll one feed and return how many new items it had. On error nothing about the fetch is
/// recorded, so the next poll fetches the same content again and retries what is still unseen.
pub fn poll_feed(store: &Store, client: &Client, feed: &Feed, notify: Notify) -> Result<usize> {
    let outcome = feed::fetch(
        client,
        &feed.url,
        feed.etag.as_deref(),
        feed.last_modified.as_deref(),
    )?;
    let fetched = match outcome {
        FetchOutcome::NotModified => {
            store.record_success(feed.id, feed.etag.as_deref(), feed.last_modified.as_deref())?;
            return Ok(0);
        }
        FetchOutcome::Fetched(fetched) => fetched,
    };
    let parsed = feed::parse(&fetched.body)?;
    let new = notify_new_items(store, feed, &parsed.items, notify)?;
    // Only now store the new validators: had a notification failed, a 304 on the next poll
    // would hide the items that were never announced.
    store.record_success(
        feed.id,
        fetched.etag.as_deref(),
        fetched.last_modified.as_deref(),
    )?;
    Ok(new)
}

/// Notify about the items of `feed` not seen before, marking each seen only after its
/// notification went out. Returns the number of items notified about. A muted feed's new
/// items are marked seen quietly, so unmuting it later does not release a backlog.
pub fn notify_new_items(
    store: &Store,
    feed: &Feed,
    items: &[Item],
    notify: Notify,
) -> Result<usize> {
    let mut keys = HashSet::new();
    let mut unseen = Vec::new();
    for item in items {
        // A feed can list the same entry twice.
        if keys.insert(item.key.as_str()) && !store.is_seen(feed.id, &item.key)? {
            unseen.push(item);
        }
    }

    if feed.muted {
        for item in &unseen {
            store.mark_seen(feed.id, item)?;
        }
        return Ok(0);
    }

    let (individual, rest) = unseen.split_at(unseen.len().min(MAX_NOTIFICATIONS));
    for item in individual {
        let body = item
            .title
            .as_deref()
            .or(item.link.as_deref())
            .unwrap_or("New item");
        notify(&feed.name, body, item.link.as_deref())?;
        store.mark_seen(feed.id, item)?;
    }
    if !rest.is_empty() {
        notify(
            &feed.name,
            &format!("+{} more from {}", rest.len(), feed.name),
            None,
        )?;
        for item in rest {
            store.mark_seen(feed.id, item)?;
        }
    }
    Ok(unseen.len())
}

#[cfg(test)]
mod tests {
    use anyhow::bail;

    use super::*;
    use crate::store::NewFeed;

    fn item(n: usize) -> Item {
        Item {
            key: format!("key-{n}"),
            title: Some(format!("Post {n}")),
            link: Some(format!("https://example.com/{n}")),
        }
    }

    fn items(range: std::ops::Range<usize>) -> Vec<Item> {
        range.map(item).collect()
    }

    /// A store with one feed named "Example" that has already seen `seen`.
    fn store_with_feed(seen: &[Item]) -> (Store, Feed) {
        let mut store = Store::open_in_memory().unwrap();
        let new = NewFeed {
            url: "https://example.com/feed",
            name: "Example",
            etag: None,
            last_modified: None,
        };
        store.add_feed(&new, seen).unwrap();
        let feed = store.list_feeds().unwrap().remove(0);
        (store, feed)
    }

    /// Run `notify_new_items`, collecting the notification bodies. Sends fail from the
    /// `fail_from`-th notification (0-based) onwards.
    fn run(
        store: &Store,
        feed: &Feed,
        items: &[Item],
        fail_from: Option<usize>,
    ) -> (Result<usize>, Vec<String>) {
        let mut sent = Vec::new();
        let result = notify_new_items(store, feed, items, &mut |title, body, _link| {
            assert_eq!(title, "Example");
            if fail_from.is_some_and(|n| sent.len() >= n) {
                bail!("notification failed");
            }
            sent.push(body.to_string());
            Ok(())
        });
        (result, sent)
    }

    #[test]
    fn seen_items_do_not_notify() {
        let (store, feed) = store_with_feed(&items(0..3));
        let (result, sent) = run(&store, &feed, &items(0..3), None);
        assert_eq!(result.unwrap(), 0);
        assert!(sent.is_empty());
    }

    #[test]
    fn new_items_notify_once() {
        let (store, feed) = store_with_feed(&items(0..2));
        let (result, sent) = run(&store, &feed, &items(0..4), None);
        assert_eq!(result.unwrap(), 2);
        assert_eq!(sent, ["Post 2", "Post 3"]);

        let (result, sent) = run(&store, &feed, &items(0..4), None);
        assert_eq!(result.unwrap(), 0);
        assert!(sent.is_empty());
    }

    #[test]
    fn exactly_the_cap_sends_no_summary() {
        let (store, feed) = store_with_feed(&[]);
        let (result, sent) = run(&store, &feed, &items(0..MAX_NOTIFICATIONS), None);
        assert_eq!(result.unwrap(), MAX_NOTIFICATIONS);
        assert_eq!(sent.len(), MAX_NOTIFICATIONS);
        assert!(sent.iter().all(|body| body.starts_with("Post ")));
    }

    #[test]
    fn items_over_the_cap_become_one_summary() {
        let (store, feed) = store_with_feed(&[]);
        let (result, sent) = run(&store, &feed, &items(0..17), None);
        assert_eq!(result.unwrap(), 17);
        assert_eq!(
            sent,
            [
                "Post 0",
                "Post 1",
                "Post 2",
                "Post 3",
                "Post 4",
                "+12 more from Example"
            ]
        );
        // The summarized items are seen too, so they are not announced next time.
        assert_eq!(store.seen_count(feed.id), 17);
    }

    #[test]
    fn a_failed_send_leaves_the_rest_unseen() {
        let (store, feed) = store_with_feed(&[]);
        let (result, sent) = run(&store, &feed, &items(0..4), Some(2));
        assert!(result.is_err());
        assert_eq!(sent, ["Post 0", "Post 1"]);
        assert_eq!(store.seen_count(feed.id), 2);

        // The next poll picks up exactly the items that were not announced.
        let (result, sent) = run(&store, &feed, &items(0..4), None);
        assert_eq!(result.unwrap(), 2);
        assert_eq!(sent, ["Post 2", "Post 3"]);
    }

    #[test]
    fn a_failed_summary_leaves_its_items_unseen() {
        let (store, feed) = store_with_feed(&[]);
        let (result, sent) = run(&store, &feed, &items(0..8), Some(MAX_NOTIFICATIONS));
        assert!(result.is_err());
        assert_eq!(sent.len(), MAX_NOTIFICATIONS);
        assert_eq!(store.seen_count(feed.id), MAX_NOTIFICATIONS as i64);
    }

    #[test]
    fn an_entry_listed_twice_notifies_once() {
        let (store, feed) = store_with_feed(&[]);
        let (result, sent) = run(&store, &feed, &[item(1), item(2), item(1)], None);
        assert_eq!(result.unwrap(), 2);
        assert_eq!(sent, ["Post 1", "Post 2"]);
    }

    #[test]
    fn items_carry_their_link_and_the_summary_has_none() {
        let (store, feed) = store_with_feed(&[]);
        let mut links = Vec::new();
        notify_new_items(&store, &feed, &items(0..7), &mut |_, _, link| {
            links.push(link.map(str::to_string));
            Ok(())
        })
        .unwrap();
        assert_eq!(links.len(), MAX_NOTIFICATIONS + 1);
        assert_eq!(links[0].as_deref(), Some("https://example.com/0"));
        assert_eq!(links[MAX_NOTIFICATIONS], None);
    }

    #[test]
    fn a_muted_feed_marks_items_seen_without_notifying() {
        let (store, feed) = store_with_feed(&[]);
        store.set_muted(feed.id, true).unwrap();
        let muted = store.list_feeds().unwrap().remove(0);
        let (result, sent) = run(&store, &muted, &items(0..3), None);
        assert_eq!(result.unwrap(), 0);
        assert!(sent.is_empty());
        assert_eq!(store.seen_count(feed.id), 3);

        // Unmuting does not release the items that arrived while muted.
        store.set_muted(feed.id, false).unwrap();
        let unmuted = store.list_feeds().unwrap().remove(0);
        let (result, sent) = run(&store, &unmuted, &items(0..4), None);
        assert_eq!(result.unwrap(), 1);
        assert_eq!(sent, ["Post 3"]);
    }

    #[test]
    fn body_falls_back_to_link_then_placeholder() {
        let (store, feed) = store_with_feed(&[]);
        let untitled = Item {
            key: "a".into(),
            title: None,
            link: Some("https://example.com/a".into()),
        };
        let bare = Item {
            key: "b".into(),
            title: None,
            link: None,
        };
        let (_, sent) = run(&store, &feed, &[untitled, bare], None);
        assert_eq!(sent, ["https://example.com/a", "New item"]);
    }
}
