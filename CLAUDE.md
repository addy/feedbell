# feedbell

A small macOS command-line tool in Rust. It subscribes to RSS/Atom feeds and shows a macOS
desktop notification whenever a subscribed feed publishes something new.

Personal project: favor simplicity and reliability over features. The code should stay small
enough to read in one sitting.

## Commands

- `feedbell add <url> [--name <name>]`: Normalize the URL, fetch and parse it to confirm it is a
  real feed, then subscribe. If it is already subscribed, print a friendly message and exit 0
  (not an error). On subscribe, mark every item currently in the feed as seen, so adding a feed
  never triggers a burst of notifications.
- `feedbell list`: Show subscriptions with name, URL, last checked time, and last error, if any.
- `feedbell remove <url-or-name>`
- `feedbell poll`: One-shot. Fetch every feed, send a notification for each unseen item, record
  the items as seen, and exit. This is what launchd runs.
- `feedbell install [--interval <seconds>]` / `feedbell uninstall`: Write or remove a LaunchAgent
  so `poll` runs on a schedule. Default interval is 300 seconds.
- `feedbell test-notify`: Send a sample notification to check that permissions work.

## Architecture

- No long-running daemon. `poll` is a one-shot run; launchd handles scheduling through a
  LaunchAgent plist in `~/Library/LaunchAgents` using `StartInterval`.
- The plist references the binary's absolute path (via `std::env::current_exe`). Warn if that
  path is inside a cargo `target/` directory and suggest `cargo install --path .` first.
- launchd stdout and stderr go to `~/Library/Logs/feedbell/`.
- Use `launchctl bootstrap gui/<uid>` and `launchctl bootout gui/<uid>`, not the deprecated
  `load`/`unload` commands.
- State lives in SQLite at `~/Library/Application Support/feedbell/feedbell.db`. Find that path
  with the `directories` crate.

## Crates

- `clap` (derive) for the CLI.
- `reqwest` with the blocking client and rustls, a sensible timeout, and a descriptive
  User-Agent.
- `feed-rs` for parsing RSS 1.0, RSS 2.0, Atom, and JSON Feed.
- `rusqlite` with the `bundled` feature.
- `directories` for the state path.
- `notify-rust` for notifications.
- `anyhow` for errors.

**Ask before adding any crate beyond these** (including dev-dependencies).

## Data model (starting point)

- `feeds`: id, url (unique), name, etag, last_modified, last_checked_at, last_error.
- `seen_items`: feed_id, item_key, title, link, seen_at; unique on (feed_id, item_key).

## Polling behavior

- Send conditional GETs using each feed's stored ETag and Last-Modified values; treat a 304 as
  "nothing new".
- The dedupe key is the entry's id/GUID. If that is missing or empty, fall back to a SHA-256
  hash of the link and title.
- Isolate failures per feed: one broken feed must not stop the others. Record the error on the
  feed, and do not mark anything seen for a feed whose fetch failed.
- Cap notifications at 5 per feed per poll. If there are more, send one summary notification
  such as "+12 more from <feed name>".
- Mark items seen only after their notification has been sent.
- Set a SQLite busy timeout so an overlapping manual `poll` and launchd `poll` do not fail.

## Notifications

- Title is the feed name; body is the item title.
- A notification from an unbundled CLI binary is attributed to another app's bundle ID, which
  notify-rust / mac-notification-sys handles. This must work both from a terminal and when
  launchd runs the binary. Test the launchd path early: it is the most likely thing to break.
- Click-to-open is not part of v1 (see milestone 4).

## Milestones

Commit at the end of each one.

1. Scaffolding, the SQLite store with migrations, and `add`, `list`, `remove`.
2. `poll` with dedupe, conditional GET, notifications, the notification cap, and `test-notify`.
3. `install` and `uninstall` with launchd, plus logging.
4. Later: click-to-open notifications, OPML import and export, per-feed muting.

### Click-to-open (later)

Clicking a notification should open the item's link. Two options under consideration:

- `terminal-notifier` with `-open <url>`.
- Wrapping the binary in a minimal `.app` bundle.

## Quality bar

- `cargo fmt` and `cargo clippy` must both be clean.
- Unit tests for the dedupe key logic and for feed parsing, using local fixture files: RSS 2.0,
  Atom, and a feed with missing GUIDs.
- Tests must not require network access.
- Keep the code straightforward.
