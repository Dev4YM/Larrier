//! Where a Claude Code chat's next turn resumes from.
//!
//! Claude Code finds a session by id only among its own folder's projects
//! (`CLAUDE_CONFIG_DIR`), and a session resumed from a file goes on writing
//! beside that file. A chat's session can be in another folder than the one
//! its next turn runs in: it was imported from your own `~/.claude`, the chat
//! now runs as another account, or the default account changed. Resumed by
//! id it isn't found ("No conversation found"); resumed from where it is, the
//! turn is written there, into a folder the chat doesn't run in, and the next
//! one isn't found.
//!
//! So its newest copy is brought into the folder the turn runs in first, with
//! its subagents and tool results, and the turn resumes from there. What it
//! was copied from stays as it was.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The folder a run with `env` keeps its sessions in.
pub fn home_of(env: &[(String, String)]) -> PathBuf {
    env.iter()
        .find(|(k, _)| k == "CLAUDE_CONFIG_DIR")
        .map(|(_, v)| PathBuf::from(v))
        .unwrap_or_else(|| lary_formats::claude::root(None))
}

/// Every folder Lary knows Claude Code keeps sessions in: the run's, your
/// own, the accounts' own folders and Lary's folders for its accounts.
fn homes(home: &Path) -> Vec<PathBuf> {
    let mut all = vec![home.to_path_buf()];
    all.extend(lary_formats::claude::roots());
    let lary = lary_core::store::data_root().join("accounts");
    all.extend(std::fs::read_dir(lary).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.join("projects").is_dir()));
    all
}

/// The file to resume session `id` from in a run that keeps its sessions in
/// `home` (from `cwd`): its newest copy, brought into `home` first when it's
/// elsewhere. `known` is where the chat last knew it to be. None: it's in
/// none of the folders Lary knows.
pub fn place(id: &str, known: Option<&Path>, home: &Path, cwd: &Path) -> Option<PathBuf> {
    place_among(id, known, home, cwd, &homes(home))
}

fn place_among(id: &str, known: Option<&Path>, home: &Path, cwd: &Path, homes: &[PathBuf]) -> Option<PathBuf> {
    let name = format!("{id}.jsonl");
    let mut copies: Vec<PathBuf> = known.filter(|p| p.is_file()).map(Path::to_path_buf).into_iter().collect();
    for h in homes {
        for d in std::fs::read_dir(h.join("projects")).into_iter().flatten().flatten() {
            let p = d.path().join(&name);
            if p.is_file() {
                copies.push(p);
            }
        }
    }
    let home_c = canon(home);
    let inside = |p: &Path| canon(p).starts_with(&home_c);
    // The newest, the longest of those, then the one already in place.
    let stamp = |p: &Path| {
        let m = std::fs::metadata(p).ok();
        let at = m.as_ref().and_then(|m| m.modified().ok()).unwrap_or(SystemTime::UNIX_EPOCH);
        (at, m.map(|m| m.len()).unwrap_or(0), inside(p))
    };
    let newest = copies.iter().max_by_key(|p| stamp(p))?.clone();
    if inside(&newest) {
        return Some(newest);
    }
    // Over the older copy this folder has, else beside the sessions of the
    // chat's own folder.
    let dest = copies
        .iter()
        .find(|p| inside(p))
        .cloned()
        .unwrap_or_else(|| home.join("projects").join(lary_formats::claude::encode_project(&cwd.to_string_lossy())).join(&name));
    match bring(&newest, &dest) {
        Ok(()) => {
            tracing::info!(id, from = %newest.display(), to = %dest.display(), "claude session brought into the run's folder");
            Some(dest)
        }
        Err(e) => {
            // Resumed from where it is: this turn still runs.
            tracing::warn!(id, from = %newest.display(), to = %dest.display(), "claude session not copied: {e}");
            Some(newest)
        }
    }
}

/// Copy a session file and its folder (subagents, tool results) to `dest`.
fn bring(src: &Path, dest: &Path) -> std::io::Result<()> {
    let dir = dest.parent().ok_or_else(|| std::io::Error::other("no folder"))?;
    std::fs::create_dir_all(dir)?;
    // Whole or not at all: a half-written copy would resume as a cut chat.
    let part = dest.with_extension("jsonl.part");
    std::fs::copy(src, &part)?;
    std::fs::rename(&part, dest)?;
    let side = src.with_extension("");
    if side.is_dir() {
        crate::user_items::copy(&side, &dest.with_extension(""))?;
    }
    Ok(())
}

fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "11111111-2222-3333-4444-555555555555";

    fn write(home: &Path, project: &str, text: &str) -> PathBuf {
        let p = home.join("projects").join(project).join(format!("{ID}.jsonl"));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
        p
    }

    fn later() {
        std::thread::sleep(std::time::Duration::from_millis(30));
    }

    /// Imported from your own folder, run as an account of Lary's: brought
    /// over (with its subagents), resumed from the copy; yours unchanged.
    #[test]
    fn a_session_from_another_folder_is_brought_over() {
        let (user, acct, cwd) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let mine = write(user.path(), "C--work", "a\nb\n");
        let side = mine.with_extension("").join("subagents");
        std::fs::create_dir_all(&side).unwrap();
        std::fs::write(side.join("agent-1.jsonl"), "s\n").unwrap();
        let homes = [acct.path().to_path_buf(), user.path().to_path_buf()];
        let got = place_among(ID, Some(&mine), acct.path(), cwd.path(), &homes).unwrap();
        assert!(got.starts_with(acct.path()), "{}", got.display());
        assert_eq!(std::fs::read_to_string(&got).unwrap(), "a\nb\n");
        assert!(got.with_extension("").join("subagents").join("agent-1.jsonl").is_file());
        assert_eq!(std::fs::read_to_string(&mine).unwrap(), "a\nb\n");
        // Once there and newest, it's resumed in place: nothing copied again.
        later();
        std::fs::write(&got, "a\nb\nc\n").unwrap();
        assert_eq!(place_among(ID, None, acct.path(), cwd.path(), &homes).unwrap(), got);
        assert_eq!(std::fs::read_to_string(&mine).unwrap(), "a\nb\n");
    }

    /// Run as account B, then as A again: B's newer copy comes back over A's.
    #[test]
    fn the_newest_copy_wins() {
        let (a, b, cwd) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let old = write(a.path(), "C--old-place", "1\n");
        later();
        write(b.path(), "C--work", "1\n2\n");
        let homes = [a.path().to_path_buf(), b.path().to_path_buf()];
        let got = place_among(ID, None, a.path(), cwd.path(), &homes).unwrap();
        assert_eq!(got, old, "over the copy A had, where it was");
        assert_eq!(std::fs::read_to_string(&got).unwrap(), "1\n2\n");
    }

    /// Nowhere at all: nothing to resume.
    #[test]
    fn a_session_that_is_nowhere_is_none() {
        let (a, cwd) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        assert!(place_among(ID, None, a.path(), cwd.path(), &[a.path().to_path_buf()]).is_none());
    }

    #[test]
    fn the_runs_folder_comes_from_its_environment() {
        let env = vec![("CLAUDE_CONFIG_DIR".to_string(), "X:/lary/accounts/1".to_string())];
        assert_eq!(home_of(&env), PathBuf::from("X:/lary/accounts/1"));
    }
}
