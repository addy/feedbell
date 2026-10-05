# feedbell

Get a macOS notification when an RSS or Atom feed publishes something new.

It is a small command-line tool. There is no app and nothing running in the background: macOS runs it for a moment every 5 minutes.

## What you need

| Need | How to get it |
|---|---|
| A Mac | feedbell is macOS only. |
| Rust | Install from [rustup.rs](https://rustup.rs). Built and tested with Rust 1.94. |
| Xcode Command Line Tools | `xcode-select --install` |
| `terminal-notifier` (optional) | `brew install terminal-notifier` |

`terminal-notifier` is what makes **clicking a notification open the article**. Without it you still get notifications; clicking them does nothing.

## Set up

**1. Install**

```sh
git clone https://github.com/addy/feedbell
cd feedbell
cargo install --path .
```

**2. Check that notifications work**

```sh
feedbell test-notify
```

A banner should appear. The first time, macOS may ask for permission: allow it. If you see no banner, go to [No notification?](#no-notification)

**3. Add a feed**

```sh
feedbell add https://blog.rust-lang.org/feed.xml
```

**4. Turn on the schedule**

```sh
feedbell install
```

Done. feedbell now checks your feeds every 5 minutes.

## Commands

| Command | What it does |
|---|---|
| `feedbell add <url>` | Subscribe. Add `--name "My name"` to choose the name shown in notifications. |
| `feedbell list` | Show your feeds, when each was last checked, and any error. |
| `feedbell remove <url-or-name>` | Unsubscribe. |
| `feedbell mute <url-or-name>` | Stop notifications from a feed, but stay subscribed. |
| `feedbell unmute <url-or-name>` | Turn its notifications back on. |
| `feedbell import <file>` | Subscribe to every feed in an OPML file. |
| `feedbell export [file]` | Save your feeds as OPML. Prints to the screen if you give no file. |
| `feedbell poll` | Check all feeds once, right now. |
| `feedbell install` | Check automatically every 5 minutes. Use `--interval <seconds>` to change that (minimum 60). |
| `feedbell uninstall` | Stop checking automatically. |
| `feedbell test-notify` | Send a test notification. |

## Good to know

- **Adding a feed is quiet.** Everything already in the feed counts as seen. You are only notified about items published afterwards.
- **No floods.** One check sends at most 5 notifications per feed, then a single "+12 more" summary.
- **Add feeds whenever.** Before or after `feedbell install`; new feeds are picked up on the next check.
- **Muting is quiet too.** Items that arrive while a feed is muted are skipped for good, so unmuting does not release a backlog.
- **One broken feed does not block the rest.** Its error shows up in `feedbell list`.

## No notification?

| Check | Fix |
|---|---|
| You installed `terminal-notifier` | System Settings > Notifications > **terminal-notifier** > Allow Notifications. Run `terminal-notifier -diagnose` to see what is wrong. |
| You did not install it | System Settings > Notifications > **Terminal** > Allow Notifications. This applies even if you use a different terminal app. |
| A Focus or Do Not Disturb mode is on | Turn it off, or allow the app above in that Focus. |
| A feed is failing | Run `feedbell list` and read the LAST ERROR column. |
| The schedule is not running | Run `feedbell install` again, then `feedbell poll` to check by hand. |

## Where things live

| What | Where |
|---|---|
| Your feeds and seen items | `~/Library/Application Support/feedbell/feedbell.db` |
| Log of scheduled checks | `~/Library/Logs/feedbell/poll.log` |
| The schedule | `~/Library/LaunchAgents/local.feedbell.poll.plist` |

The log only gets a line when there is something new or something failed.

## Remove everything

```sh
feedbell uninstall
cargo uninstall feedbell
rm -r ~/Library/Application\ Support/feedbell ~/Library/Logs/feedbell
```

## Development

```sh
cargo test
cargo fmt
cargo clippy
```

Tests use local fixture files and need no network. Design decisions are recorded in [CLAUDE.md](CLAUDE.md).
