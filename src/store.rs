//! SQLite-backed state: subscribed feeds and the items already seen for each.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use rusqlite::{Connection, params};

use crate::feed::Item;

/// Schema migrations, applied in order. `PRAGMA user_version` records how many have run, so
/// only ever append to this list.
const MIGRATIONS: &[&str] = &["
    CREATE TABLE feeds (
        id              INTEGER PRIMARY KEY,
        url             TEXT NOT NULL UNIQUE,
        name            TEXT NOT NULL,
        etag            TEXT,
        last_modified   TEXT,
        last_checked_at INTEGER, -- unix seconds
        last_error      TEXT
    );
    CREATE TABLE seen_items (
        feed_id  INTEGER NOT NULL REFERENCES feeds(id) ON DELETE CASCADE,
        item_key TEXT NOT NULL,
        title    TEXT,
        link     TEXT,
        seen_at  INTEGER NOT NULL, -- unix seconds
        UNIQUE (feed_id, item_key)
    );
"];

/// A subscription.
#[derive(Debug)]
pub struct Feed {
    pub id: i64,
    pub url: String,
    pub name: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    /// Local time, already formatted for display.
    pub last_checked: Option<String>,
    pub last_error: Option<String>,
}

/// What `add` stores for a new subscription.
pub struct NewFeed<'a> {
    pub url: &'a str,
    pub name: &'a str,
    pub etag: Option<&'a str>,
    pub last_modified: Option<&'a str>,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        Self::init(conn)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        // A manual `poll` and a launchd `poll` can overlap; wait rather than fail.
        conn.busy_timeout(Duration::from_secs(10))?;
        conn.pragma_update(None, "foreign_keys", true)?;
        migrate(&mut conn)?;
        Ok(Self { conn })
    }

    pub fn find_by_url(&self, url: &str) -> Result<Option<Feed>> {
        Ok(self.query_feeds("WHERE url = ?1", url)?.pop())
    }

    /// Feeds whose name matches, ignoring ASCII case. Names are not unique.
    pub fn find_by_name(&self, name: &str) -> Result<Vec<Feed>> {
        self.query_feeds("WHERE name = ?1 COLLATE NOCASE ORDER BY id", name)
    }

    pub fn list_feeds(&self) -> Result<Vec<Feed>> {
        let mut stmt = self
            .conn
            .prepare(&format!("{SELECT_FEED} ORDER BY name COLLATE NOCASE, id"))?;
        let feeds = stmt.query_map([], row_to_feed)?.collect::<Result<_, _>>()?;
        Ok(feeds)
    }

    /// Subscribe to a feed and mark `items` as seen, atomically, so a new subscription never
    /// produces a burst of notifications.
    pub fn add_feed(&mut self, feed: &NewFeed, items: &[Item]) -> Result<i64> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO feeds (url, name, etag, last_modified, last_checked_at)
             VALUES (?1, ?2, ?3, ?4, unixepoch())",
            params![feed.url, feed.name, feed.etag, feed.last_modified],
        )?;
        let feed_id = tx.last_insert_rowid();
        for item in items {
            insert_seen(&tx, feed_id, item)?;
        }
        tx.commit()?;
        Ok(feed_id)
    }

    /// Remove a feed and, through the foreign key, its seen items.
    pub fn remove_feed(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM feeds WHERE id = ?1", [id])?;
        Ok(())
    }

    /// The current local time, formatted like the times in `list`.
    pub fn now_local(&self) -> Result<String> {
        let now = self
            .conn
            .query_row("SELECT datetime('now', 'localtime')", [], |row| row.get(0))?;
        Ok(now)
    }

    pub fn is_seen(&self, feed_id: i64, item_key: &str) -> Result<bool> {
        let seen = self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM seen_items WHERE feed_id = ?1 AND item_key = ?2)",
            params![feed_id, item_key],
            |row| row.get(0),
        )?;
        Ok(seen)
    }

    pub fn mark_seen(&self, feed_id: i64, item: &Item) -> Result<()> {
        insert_seen(&self.conn, feed_id, item)
    }

    /// Record a poll that worked: stamp the time, clear any error, and store the validators
    /// for the next conditional GET.
    pub fn record_success(
        &self,
        feed_id: i64,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE feeds SET etag = ?2, last_modified = ?3, last_checked_at = unixepoch(),
                 last_error = NULL
             WHERE id = ?1",
            params![feed_id, etag, last_modified],
        )?;
        Ok(())
    }

    /// Record a poll that failed. The validators are left alone.
    pub fn record_error(&self, feed_id: i64, error: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE feeds SET last_checked_at = unixepoch(), last_error = ?2 WHERE id = ?1",
            params![feed_id, error],
        )?;
        Ok(())
    }

    fn query_feeds(&self, clause: &str, param: &str) -> Result<Vec<Feed>> {
        let mut stmt = self.conn.prepare(&format!("{SELECT_FEED} {clause}"))?;
        let feeds = stmt
            .query_map([param], row_to_feed)?
            .collect::<Result<_, _>>()?;
        Ok(feeds)
    }

    #[cfg(test)]
    pub fn seen_count(&self, feed_id: i64) -> i64 {
        self.conn
            .query_row(
                "SELECT count(*) FROM seen_items WHERE feed_id = ?1",
                [feed_id],
                |row| row.get(0),
            )
            .unwrap()
    }
}

const SELECT_FEED: &str = "SELECT id, url, name, etag, last_modified,
    datetime(last_checked_at, 'unixepoch', 'localtime'), last_error FROM feeds";

/// Ignores an item that is already seen, as an overlapping poll may have recorded it first.
fn insert_seen(conn: &Connection, feed_id: i64, item: &Item) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO seen_items (feed_id, item_key, title, link, seen_at)
         VALUES (?1, ?2, ?3, ?4, unixepoch())",
        params![feed_id, item.key, item.title, item.link],
    )?;
    Ok(())
}

fn row_to_feed(row: &rusqlite::Row) -> rusqlite::Result<Feed> {
    Ok(Feed {
        id: row.get(0)?,
        url: row.get(1)?,
        name: row.get(2)?,
        etag: row.get(3)?,
        last_modified: row.get(4)?,
        last_checked: row.get(5)?,
        last_error: row.get(6)?,
    })
}

fn migrate(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    let version: u32 = tx.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
        tx.execute_batch(sql)
            .with_context(|| format!("applying migration {}", i + 1))?;
    }
    tx.pragma_update(None, "user_version", MIGRATIONS.len() as u32)?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_feed<'a>(url: &'a str, name: &'a str) -> NewFeed<'a> {
        NewFeed {
            url,
            name,
            etag: Some("\"abc\""),
            last_modified: None,
        }
    }

    fn item(key: &str) -> Item {
        Item {
            key: key.to_string(),
            title: Some(format!("title {key}")),
            link: None,
        }
    }

    #[test]
    fn migrations_are_idempotent() {
        let mut store = Store::open_in_memory().unwrap();
        migrate(&mut store.conn).unwrap();
        let version: u32 = store
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version as usize, MIGRATIONS.len());
    }

    #[test]
    fn add_marks_items_seen_and_lists_feed() {
        let mut store = Store::open_in_memory().unwrap();
        let id = store
            .add_feed(
                &new_feed("https://example.com/feed", "Example"),
                // A feed that repeats a key must not break the subscribe.
                &[item("a"), item("b"), item("a")],
            )
            .unwrap();
        assert_eq!(store.seen_count(id), 2);

        let feeds = store.list_feeds().unwrap();
        assert_eq!(feeds.len(), 1);
        assert_eq!(feeds[0].name, "Example");
        assert!(feeds[0].last_checked.is_some());
        assert!(feeds[0].last_error.is_none());
    }

    #[test]
    fn duplicate_url_is_rejected() {
        let mut store = Store::open_in_memory().unwrap();
        let feed = new_feed("https://example.com/feed", "Example");
        store.add_feed(&feed, &[]).unwrap();
        assert!(store.add_feed(&feed, &[]).is_err());
        assert_eq!(store.list_feeds().unwrap().len(), 1);
    }

    #[test]
    fn find_by_url_and_name() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .add_feed(&new_feed("https://a.example/feed", "Blog"), &[])
            .unwrap();
        store
            .add_feed(&new_feed("https://b.example/feed", "blog"), &[])
            .unwrap();

        let found = store.find_by_url("https://a.example/feed").unwrap();
        assert_eq!(found.unwrap().name, "Blog");
        assert!(store.find_by_url("https://c.example/").unwrap().is_none());
        assert_eq!(store.find_by_name("BLOG").unwrap().len(), 2);
        assert!(store.find_by_name("other").unwrap().is_empty());
    }

    #[test]
    fn mark_seen_is_per_feed_and_idempotent() {
        let mut store = Store::open_in_memory().unwrap();
        let a = store
            .add_feed(&new_feed("https://a.example/feed", "A"), &[])
            .unwrap();
        let b = store
            .add_feed(&new_feed("https://b.example/feed", "B"), &[])
            .unwrap();

        assert!(!store.is_seen(a, "x").unwrap());
        store.mark_seen(a, &item("x")).unwrap();
        store.mark_seen(a, &item("x")).unwrap();
        assert!(store.is_seen(a, "x").unwrap());
        assert!(!store.is_seen(b, "x").unwrap());
        assert_eq!(store.seen_count(a), 1);
    }

    #[test]
    fn record_error_then_success() {
        let mut store = Store::open_in_memory().unwrap();
        let id = store
            .add_feed(&new_feed("https://example.com/feed", "Example"), &[])
            .unwrap();

        store.record_error(id, "connection refused").unwrap();
        let feed = store.list_feeds().unwrap().remove(0);
        assert_eq!(feed.last_error.as_deref(), Some("connection refused"));
        // A failed poll keeps the validators from the last good one.
        assert_eq!(feed.etag.as_deref(), Some("\"abc\""));

        store
            .record_success(id, Some("\"def\""), Some("yesterday"))
            .unwrap();
        let feed = store.list_feeds().unwrap().remove(0);
        assert!(feed.last_error.is_none());
        assert_eq!(feed.etag.as_deref(), Some("\"def\""));
        assert_eq!(feed.last_modified.as_deref(), Some("yesterday"));
    }

    #[test]
    fn remove_deletes_feed_and_its_seen_items() {
        let mut store = Store::open_in_memory().unwrap();
        let id = store
            .add_feed(
                &new_feed("https://example.com/feed", "Example"),
                &[item("a")],
            )
            .unwrap();
        store.remove_feed(id).unwrap();
        assert!(store.list_feeds().unwrap().is_empty());
        assert_eq!(store.seen_count(id), 0);
    }
}
