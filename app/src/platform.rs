//! What differs between Windows, macOS and Linux in how the window looks
//! and how shortcuts read.
//!
//! - Windows: no system frame; Larrier draws its menu bar and the window's
//!   buttons.
//! - macOS: the system's traffic lights over Larrier's own top bar, and the
//!   app's commands in the system menu bar.
//! - Linux: the system's frame (works with every window manager), with
//!   Larrier's menu bar under it.

/// Larrier draws the window's caption buttons itself (no system frame).
pub const DRAWS_CAPTION: bool = cfg!(windows);

/// The app's menus are in the system menu bar, not the window.
pub const SYSTEM_MENU_BAR: bool = cfg!(target_os = "macos");

/// A shortcut as this platform writes it: `Ctrl+K` reads `⌘K` on macOS
/// (and `Alt` `⌥`, `Shift` `⇧`).
pub fn keys(s: &str) -> String {
    if !cfg!(target_os = "macos") {
        return s.to_string();
    }
    mac_keys(s)
}

fn mac_keys(s: &str) -> String {
    s.replace("Ctrl+", "⌘").replace("Alt+", "⌥").replace("Shift+", "⇧").replace("Ctrl", "⌘").replace("Alt", "⌥").replace("Shift", "⇧")
}

/// One key of a shortcut table (`["Ctrl", "K"]`) as this platform names it.
pub fn key(k: &str) -> &str {
    if !cfg!(target_os = "macos") {
        return k;
    }
    match k {
        "Ctrl" => "⌘",
        "Alt" => "⌥",
        "Shift" => "⇧",
        other => other,
    }
}

/// macOS and Linux: an app opened from the Dock, Finder or a desktop menu
/// gets a bare PATH, without Homebrew, npm's or `~/.local/bin`, where the
/// agents and the tools they run (node, git, gh) live. Larrier takes the
/// PATH the user's login shell sets up, as a terminal has it, ahead of its
/// own. Called once at startup, before any thread starts.
pub fn adopt_login_path() {
    #[cfg(unix)]
    {
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};
        const MARK: &str = "__LARRIER_PATH__";
        let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/sh".into());
        let Ok(mut child) = Command::new(&shell)
            .args(["-l", "-i", "-c", &format!("printf '{MARK}%s{MARK}' \"$PATH\"")])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            return;
        };
        // A shell that waits on something (a prompt, a slow profile) is
        // given up on.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(30)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
            }
        }
        let mut out = String::new();
        if let Some(mut s) = child.stdout.take() {
            let _ = std::io::Read::read_to_string(&mut s, &mut out);
        }
        let login = out.split(MARK).nth(1).unwrap_or("");
        let current = std::env::var("PATH").unwrap_or_default();
        let mut seen = std::collections::HashSet::new();
        let joined: Vec<&str> = login.split(':').chain(current.split(':')).filter(|d| !d.is_empty() && seen.insert(*d)).collect();
        if !joined.is_empty() {
            std::env::set_var("PATH", joined.join(":"));
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn shortcuts_read_the_mac_way() {
        assert_eq!(super::mac_keys("Ctrl+K"), "⌘K");
        assert_eq!(super::mac_keys("Ctrl+Shift+P"), "⌘⇧P");
        assert_eq!(super::mac_keys("Settings (Ctrl+,)"), "Settings (⌘,)");
        assert_eq!(super::mac_keys("Ctrl+Enter does the other"), "⌘Enter does the other");
    }
}
