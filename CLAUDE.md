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
- `feedbell mute <url-or-name>` / `feedbell unmute <url-or-name>`: A muted feed is still
  polled, but its new items are marked seen without notifying, so unmuting never releases a
  backlog.
- `feedbell import <file>` / `feedbell export [file]`: OPML. Import subscribes to each feed
  the way `add` does (fetch, validate, mark current items seen), flattens folders, and carries
  on past feeds that fail. Export writes to stdout when no file is given.

## Architecture

- No long-running daemon. `poll` is a one-shot run; launchd handles scheduling through a
  LaunchAgent plist in `~/Library/LaunchAgents` using `StartInterval`.
- The plist references the binary's absolute path (via `std::env::current_exe`). Warn if that
  path is inside a cargo `target/` directory and suggest `cargo install --path .` first.
- launchd stdout and stderr both go to `~/Library/Logs/feedbell/poll.log`. Lines are
  timestamped. A scheduled poll with nothing new and no failures logs nothing, so the log
  only grows with news; `list` shows when each feed was last checked. There is no rotation.
- The LaunchAgent label is `local.feedbell.poll`. It sets `RunAtLoad`, so a poll runs at
  install and at login rather than one interval later. The minimum interval is 60 seconds.
- `install` over an existing install replaces it. `uninstall` leaves the database and logs.
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
- `sha2` for the dedupe-key fallback hash.
- `quick-xml` for reading OPML (already in the build as a dependency of `feed-rs`).

**Ask before adding any crate beyond these** (including dev-dependencies).

## Data model (starting point)

- `feeds`: id, url (unique), name, etag, last_modified, last_checked_at, last_error, muted.
- `seen_items`: feed_id, item_key, title, link, seen_at; unique on (feed_id, item_key).

## Polling behavior

- Send conditional GETs using each feed's stored ETag and Last-Modified values; treat a 304 as
  "nothing new".
- The dedupe key is the entry's id/GUID. If that is missing or empty, fall back to a SHA-256
  hash of the link and title. feed-rs generates its own ids for entries without one (random
  when there is no link), so its id generator is turned off in `feed::parse`.
- Isolate failures per feed: one broken feed must not stop the others. Record the error on the
  feed, and do not mark anything seen for a feed whose fetch failed.
- Cap notifications at 5 per feed per poll. If there are more, send one summary notification
  such as "+12 more from <feed name>".
- Mark items seen only after their notification has been sent. Store a feed's new ETag and
  Last-Modified only after all of its notifications went out, so a failure is retried rather
  than hidden behind a 304.
- Set a SQLite busy timeout so an overlapping manual `poll` and launchd `poll` do not fail.

## Notifications

- Title is the feed name; body is the item title.
- feedbell is an unbundled CLI binary, so its notifications are attributed to another app's
  bundle ID. We set this explicitly to `com.apple.Terminal` (`src/notify.rs`). Verified to
  show banners both from a shell and under launchd. Notifications must be allowed for
  Terminal in System Settings > Notifications, whichever terminal app is actually in use.
  `com.apple.Finder` and Ghostty's bundle ID did not get a delivery confirmation from macOS.
- notify-rust sends when the notification handle is dropped and discards errors, so there is
  no delivery-failure signal. "Sent" means "handed to macOS". The one failure we can detect,
  an unusable sender bundle ID, is checked explicitly.
- Do not let notify-rust pick the sender itself: it runs an AppleScript lookup to do so.
- Click-to-open uses `terminal-notifier -open <url>` when it is installed (`brew install
  terminal-notifier`). feedbell looks in `/opt/homebrew/bin` and `/usr/local/bin` first,
  because launchd's PATH has neither. If it is missing or fails, the notify-rust path above
  is the fallback, with no click action.
- terminal-notifier needs its own permission in System Settings > Notifications. Unlike
  notify-rust it exits non-zero on failure. `terminal-notifier -diagnose` explains why, and
  `terminal-notifier -list ALL` shows what was delivered.
- terminal-notifier rejects values that start with `[`, `-`, and similar, and strips one
  leading backslash from every value, so title and message are always passed with one.
- Only `http://` and `https://` links are passed to `-open`; the link comes from the feed.

## Milestones

Commit at the end of each one.

1. Scaffolding, the SQLite store with migrations, and `add`, `list`, `remove`.
2. `poll` with dedupe, conditional GET, notifications, the notification cap, and `test-notify`.
3. `install` and `uninstall` with launchd, plus logging.
4. Click-to-open notifications, OPML import and export, per-feed muting.

All four are done. For click-to-open, the alternative considered and rejected was wrapping the
binary in a minimal `.app` bundle: a click would relaunch the app, which needs Objective-C
delegate code to receive it.

## Quality bar

- `cargo fmt` and `cargo clippy` must both be clean.
- Unit tests for the dedupe key logic and for feed parsing, using local fixture files: RSS 2.0,
  Atom, and a feed with missing GUIDs.
- Tests must not require network access.
- Keep the code straightforward.
