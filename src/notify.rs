//! macOS desktop notifications.

use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow};
use notify_rust::Notification;

/// feedbell is a bare binary with no app bundle, so macOS needs an existing app to attribute
/// its notifications to. Terminal works both from a shell and under launchd; notifications
/// must be allowed for Terminal in System Settings.
const SENDER_BUNDLE_ID: &str = "com.apple.Terminal";

/// Hand a notification to macOS. notify-rust reports no delivery failures, so `Ok` means
/// "handed over", not "shown".
pub fn send(title: &str, body: &str) -> Result<()> {
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
