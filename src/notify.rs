//! macOS desktop notifications.
//!
//! With `terminal-notifier` installed, clicking a notification opens the item's link.
//! Without it, or if it fails, notify-rust shows the same notification with no click action.

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow, bail};
use notify_rust::Notification;

/// feedbell is a bare binary with no app bundle, so macOS needs an existing app to attribute
/// notify-rust's notifications to. Terminal works both from a shell and under launchd;
/// notifications must be allowed for Terminal in System Settings.
const SENDER_BUNDLE_ID: &str = "com.apple.Terminal";

/// Where Homebrew puts `terminal-notifier`. launchd's PATH has neither directory.
const TERMINAL_NOTIFIER_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin"];

/// Send a notification. `link` is opened when it is clicked, if `terminal-notifier` is there.
pub fn send(title: &str, body: &str, link: Option<&str>) -> Result<()> {
    if let Some(program) = terminal_notifier() {
        match send_clickable(&program, title, body, link) {
            Ok(()) => return Ok(()),
            Err(err) => eprintln!("{err:#}; sending a plain notification instead"),
        }
    }
    send_plain(title, body)
}

/// The `terminal-notifier` binary, if installed.
pub fn terminal_notifier() -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    TERMINAL_NOTIFIER_DIRS
        .iter()
        .map(PathBuf::from)
        .chain(std::env::split_paths(&path))
        .map(|dir| dir.join("terminal-notifier"))
        .find(|candidate| candidate.is_file())
}

fn send_clickable(program: &PathBuf, title: &str, body: &str, link: Option<&str>) -> Result<()> {
    let output = Command::new(program)
        .args(terminal_notifier_args(title, body, link))
        .output()
        .with_context(|| format!("running {}", program.display()))?;
    if !output.status.success() {
        bail!(
            "terminal-notifier failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn terminal_notifier_args(title: &str, body: &str, link: Option<&str>) -> Vec<String> {
    // terminal-notifier rejects values starting with characters like `[` or `-`. It strips
    // one leading backslash from every value, so always adding one is safe.
    let mut args = vec![
        "-title".to_string(),
        format!("\\{title}"),
        "-message".to_string(),
        format!("\\{body}"),
    ];
    // The link comes from the feed, so only let a click open a web page.
    if let Some(link) = link
        && (link.starts_with("https://") || link.starts_with("http://"))
    {
        args.push("-open".to_string());
        args.push(link.to_string());
    }
    args
}

/// Hand a notification to macOS through notify-rust, which reports no delivery failures: `Ok`
/// means "handed over", not "shown".
fn send_plain(title: &str, body: &str) -> Result<()> {
    // Set the sender ourselves. Left alone, notify-rust looks one up by running an AppleScript
    // and hides any failure to set it.
    static SENDER: OnceLock<Result<(), String>> = OnceLock::new();
    SENDER
        .get_or_init(|| notify_rust::set_application(SENDER_BUNDLE_ID).map_err(|e| e.to_string()))
        .clone()
        .map_err(|e| anyhow!("could not send notifications as {SENDER_BUNDLE_ID}: {e}"))?;

    Notification::new()
        .summary(title)
        .body(body)
        .show()
        .context("sending the notification")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_escape_title_and_body_and_open_the_link() {
        assert_eq!(
            terminal_notifier_args("[Blog]", "-5 degrees", Some("https://example.com/a?b=c")),
            [
                "-title",
                "\\[Blog]",
                "-message",
                "\\-5 degrees",
                "-open",
                "https://example.com/a?b=c"
            ]
        );
    }

    #[test]
    fn args_without_a_link_have_no_open() {
        assert_eq!(
            terminal_notifier_args("Blog", "Post", None),
            ["-title", "\\Blog", "-message", "\\Post"]
        );
    }

    #[test]
    fn only_web_links_are_opened() {
        for link in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "x-app://do",
            "",
        ] {
            let args = terminal_notifier_args("Blog", "Post", Some(link));
            assert!(!args.contains(&"-open".to_string()), "link: {link}");
        }
        let args = terminal_notifier_args("Blog", "Post", Some("http://example.com/"));
        assert!(args.contains(&"-open".to_string()));
    }
}
