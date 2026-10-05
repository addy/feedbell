//! Scheduling through launchd: a LaunchAgent that runs `feedbell poll` on an interval.

use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output};
use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use directories::BaseDirs;

const LABEL: &str = "local.feedbell.poll";

pub fn install(interval: u32) -> Result<()> {
    let exe = std::env::current_exe().context("finding the feedbell binary")?;
    if in_cargo_target(&exe) {
        eprintln!(
            "Warning: {} is inside a cargo target/ directory, so the schedule breaks when the \
             build is cleaned or moved. Run `cargo install --path .` and then \
             `feedbell install` from the installed binary.",
            exe.display()
        );
    }

    let home = home_dir()?;
    let log = home.join("Library/Logs/feedbell/poll.log");
    let plist = plist_path(&home);
    for dir in [log.parent(), plist.parent()].into_iter().flatten() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    std::fs::write(&plist, plist_xml(&exe, interval, &log))
        .with_context(|| format!("writing {}", plist.display()))?;

    // Replace a previous install; launchd refuses to bootstrap a label that is loaded.
    let domain = gui_domain()?;
    bootout(&domain)?;
    let output = launchctl(&["bootstrap", &domain, &plist.to_string_lossy()])?;
    if !output.status.success() {
        bail!(
            "launchctl bootstrap failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    println!("Installed: feedbell poll runs every {interval} seconds, starting now.");
    println!("  LaunchAgent: {}", plist.display());
    println!("  Log:         {}", log.display());
    Ok(())
}

pub fn uninstall() -> Result<()> {
    let plist = plist_path(&home_dir()?);
    let was_loaded = bootout(&gui_domain()?)?;
    let had_plist = plist.exists();
    if had_plist {
        std::fs::remove_file(&plist).with_context(|| format!("removing {}", plist.display()))?;
    }
    if was_loaded || had_plist {
        println!("Uninstalled. Subscriptions and logs are left in place.");
    } else {
        println!("Nothing to uninstall: feedbell is not scheduled.");
    }
    Ok(())
}

/// Unload the agent if it is loaded, and wait until launchd has let go of it. Returns whether
/// it was loaded.
fn bootout(domain: &str) -> Result<bool> {
    let target = format!("{domain}/{LABEL}");
    let is_loaded = || Ok::<_, anyhow::Error>(launchctl(&["print", &target])?.status.success());
    if !is_loaded()? {
        return Ok(false);
    }
    let output = launchctl(&["bootout", &target])?;
    // bootout returns before the job is fully gone, and bootstrapping too early fails.
    for _ in 0..50 {
        if !is_loaded()? {
            return Ok(true);
        }
        sleep(Duration::from_millis(100));
    }
    bail!(
        "launchctl bootout did not unload {target}: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    )
}

fn launchctl(args: &[&str]) -> Result<Output> {
    Command::new("/bin/launchctl")
        .args(args)
        .output()
        .context("running launchctl")
}

/// The launchd domain of the logged-in user: `gui/<uid>`.
fn gui_domain() -> Result<String> {
    let output = Command::new("/usr/bin/id")
        .arg("-u")
        .output()
        .context("running id -u")?;
    let uid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || uid.is_empty() {
        bail!("could not determine the user id");
    }
    Ok(format!("gui/{uid}"))
}

fn home_dir() -> Result<PathBuf> {
    let dirs = BaseDirs::new().context("could not find a home directory")?;
    Ok(dirs.home_dir().to_path_buf())
}

fn plist_path(home: &Path) -> PathBuf {
    home.join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

fn in_cargo_target(exe: &Path) -> bool {
    exe.components()
        .any(|c| matches!(c, Component::Normal(name) if name == "target"))
}

fn plist_xml(exe: &Path, interval: u32, log: &Path) -> String {
    let exe = xml_escape(&exe.to_string_lossy());
    let log = xml_escape(&log.to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>poll</string>
    </array>
    <key>StartInterval</key>
    <integer>{interval}</integer>
    <key>RunAtLoad</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
</dict>
</plist>
"#
    )
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_has_the_binary_interval_and_log() {
        let xml = plist_xml(
            Path::new("/Users/me/.cargo/bin/feedbell"),
            300,
            Path::new("/Users/me/Library/Logs/feedbell/poll.log"),
        );
        assert!(xml.contains("<string>local.feedbell.poll</string>"));
        assert!(xml.contains("<string>/Users/me/.cargo/bin/feedbell</string>"));
        assert!(xml.contains("<string>poll</string>"));
        assert!(xml.contains("<key>StartInterval</key>\n    <integer>300</integer>"));
        assert_eq!(
            xml.matches("<string>/Users/me/Library/Logs/feedbell/poll.log</string>")
                .count(),
            2
        );
    }

    #[test]
    fn plist_escapes_paths() {
        let xml = plist_xml(Path::new("/a&b/<c>/feedbell"), 60, Path::new("/logs/x"));
        assert!(xml.contains("<string>/a&amp;b/&lt;c&gt;/feedbell</string>"));
    }

    #[test]
    fn detects_cargo_target_directories() {
        assert!(in_cargo_target(Path::new(
            "/Users/me/feedbell/target/debug/feedbell"
        )));
        assert!(in_cargo_target(Path::new(
            "/Users/me/feedbell/target/release/feedbell"
        )));
        assert!(!in_cargo_target(Path::new("/Users/me/.cargo/bin/feedbell")));
        assert!(!in_cargo_target(Path::new("/opt/targets/bin/feedbell")));
    }
}
