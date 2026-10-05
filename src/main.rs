mod feed;
mod notify;
mod poll;
mod store;

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use directories::ProjectDirs;

use feed::FetchOutcome;
use store::{Feed, NewFeed, Store};

/// Desktop notifications for new items in your RSS/Atom feeds.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Subscribe to a feed
    Add {
        url: String,
        /// Name to show in notifications (defaults to the feed's title)
        #[arg(long)]
        name: Option<String>,
    },
    /// Show subscriptions
    List,
    /// Unsubscribe from a feed
    Remove {
        /// The feed's URL or name
        url_or_name: String,
    },
    /// Check every feed once and notify about new items
    Poll,
    /// Send a sample notification to check that permissions work
    TestNotify,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Command::TestNotify = cli.command {
        return test_notify();
    }
    let mut store = Store::open(&db_path()?)?;
    match cli.command {
        Command::TestNotify => unreachable!(),
        Command::Add { url, name } => add(&mut store, &url, name.as_deref()),
        Command::List => list(&store),
        Command::Remove { url_or_name } => remove(&store, &url_or_name),
        Command::Poll => poll(&store),
    }
}

/// `~/Library/Application Support/feedbell/feedbell.db`
fn db_path() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("", "", "feedbell").context("could not find a home directory")?;
    Ok(dirs.data_dir().join("feedbell.db"))
}

fn add(store: &mut Store, url: &str, name: Option<&str>) -> Result<()> {
    let url = feed::normalize_url(url)?;
    if let Some(existing) = store.find_by_url(&url)? {
        println!("Already subscribed to {} ({url}).", existing.name);
        return Ok(());
    }

    let FetchOutcome::Fetched(fetched) = feed::fetch(&feed::http_client()?, &url, None, None)?
    else {
        bail!("{url} answered 304 Not Modified to an unconditional request");
    };
    let parsed = feed::parse(&fetched.body).with_context(|| format!("parsing {url}"))?;

    let name = name
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .or(parsed.title)
        .or_else(|| feed::host_of(&url))
        .unwrap_or_else(|| url.clone());
    store.add_feed(
        &NewFeed {
            url: &url,
            name: &name,
            etag: fetched.etag.as_deref(),
            last_modified: fetched.last_modified.as_deref(),
        },
        &parsed.items,
    )?;
    println!(
        "Subscribed to {name} ({url}). Marked {} existing item(s) as seen.",
        parsed.items.len()
    );
    Ok(())
}

fn list(store: &Store) -> Result<()> {
    let feeds = store.list_feeds()?;
    if feeds.is_empty() {
        println!("No subscriptions yet. Add one with `feedbell add <url>`.");
        return Ok(());
    }

    let rows: Vec<[&str; 4]> = feeds
        .iter()
        .map(|f| {
            [
                f.name.as_str(),
                f.url.as_str(),
                f.last_checked.as_deref().unwrap_or("never"),
                f.last_error.as_deref().unwrap_or("-"),
            ]
        })
        .collect();
    let header = ["NAME", "URL", "LAST CHECKED", "LAST ERROR"];
    let width = |col: usize| {
        rows.iter()
            .chain([&header])
            .map(|row| row[col].chars().count())
            .max()
            .unwrap_or(0)
    };
    let widths = [width(0), width(1), width(2)];
    for row in [&header].into_iter().chain(&rows) {
        println!(
            "{:<w0$}  {:<w1$}  {:<w2$}  {}",
            row[0],
            row[1],
            row[2],
            row[3],
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2],
        );
    }
    Ok(())
}

fn poll(store: &Store) -> Result<()> {
    let feeds = store.list_feeds()?;
    let client = feed::http_client()?;
    let mut new_items = 0;
    let mut failed = 0;
    for feed in &feeds {
        // One broken feed must not stop the others: record the error and move on.
        match poll::poll_feed(store, &client, feed, &mut notify::send) {
            Ok(0) => {}
            Ok(new) => {
                new_items += new;
                println!("{}: {new} new item(s)", feed.name);
            }
            Err(err) => {
                failed += 1;
                let message = format!("{err:#}");
                eprintln!("{}: {message}", feed.name);
                store.record_error(feed.id, &message)?;
            }
        }
    }
    println!(
        "Checked {} feed(s): {new_items} new item(s), {failed} failed.",
        feeds.len()
    );
    if failed > 0 {
        bail!("{failed} feed(s) could not be checked; see `feedbell list`");
    }
    Ok(())
}

fn test_notify() -> Result<()> {
    notify::send(
        "feedbell",
        "Test notification: if you can see this, it works.",
    )?;
    println!(
        "Sent a test notification. If nothing appeared, allow notifications for Terminal in \
         System Settings > Notifications."
    );
    Ok(())
}

fn remove(store: &Store, url_or_name: &str) -> Result<()> {
    let feed = find_feed(store, url_or_name)?;
    store.remove_feed(feed.id)?;
    println!("Removed {} ({}).", feed.name, feed.url);
    Ok(())
}

/// Resolve a `remove` argument: an exact URL match wins, then a name match.
fn find_feed(store: &Store, url_or_name: &str) -> Result<Feed> {
    if let Ok(url) = feed::normalize_url(url_or_name)
        && let Some(feed) = store.find_by_url(&url)?
    {
        return Ok(feed);
    }
    let mut matches = store.find_by_name(url_or_name.trim())?;
    match matches.len() {
        0 => bail!("no subscription matches '{url_or_name}'; see `feedbell list`"),
        1 => Ok(matches.remove(0)),
        _ => {
            let urls: Vec<&str> = matches.iter().map(|f| f.url.as_str()).collect();
            bail!(
                "'{url_or_name}' matches more than one subscription; remove by URL instead:\n  {}",
                urls.join("\n  ")
            )
        }
    }
}
