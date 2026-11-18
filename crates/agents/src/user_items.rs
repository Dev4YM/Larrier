//! The user's own Claude Code agents, skills and commands (`~/.claude`),
//! where a run in a folder of Lary's for an account finds them. Lary offers
//! them (Run as, @ Subagents, / Skills), so a run must have them: with only
//! the folder's own, `--agent ide-framework` finds nothing and the chat runs
//! as plain Claude Code, without the agent's prompt, tools or MCP servers.
//!
//! They're copied there (Claude Code reads them as it reads its own), kept
//! up to date at each run, and a copy goes once its original is gone. Lary
//! never writes to `~/.claude`, and what the account's folder has of its own
//! stays as it is. The user's settings, hooks, plugins and sign-in stay out,
//! as they always have.

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// What comes along, by the folder Claude Code reads it from.
const KINDS: [&str; 3] = ["agents", "skills", "commands"];
/// In the account's folder: what Lary copied there, and as it was.
const MANIFEST: &str = ".lary-user-items.json";

type Manifest = BTreeMap<String, BTreeMap<String, u64>>;

/// Bring them along for a run with `env` (its environment), when it runs in
/// a folder of Lary's (`CLAUDE_CONFIG_DIR` under its accounts) rather than
/// the user's own.
pub fn bring_for(env: &[(String, String)]) {
    let Some(home) = env.iter().find(|(k, _)| k == "CLAUDE_CONFIG_DIR").map(|(_, v)| PathBuf::from(v)) else { return };
    if !home.starts_with(lary_core::store::data_root().join("accounts")) {
        return;
    }
    let user = crate::harness::Roots::from_env().claude;
    if user != home && user.is_dir() {
        bring(&user, &home);
    }
}

/// Copy the user's agents, skills and commands from `user` into `home`.
pub fn bring(user: &Path, home: &Path) {
    let manifest_path = home.join(MANIFEST);
    let had: Manifest = std::fs::read_to_string(&manifest_path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let mut now: Manifest = BTreeMap::new();
    for kind in KINDS {
        let (src, dst) = (user.join(kind), home.join(kind));
        let before = had.get(kind).cloned().unwrap_or_default();
        let mut copied = BTreeMap::new();
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&src).into_iter().flatten().flatten().map(|e| e.path()).collect();
        entries.sort();
        for path in entries {
            let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string()) else { continue };
            // Hidden ones (a trash folder) and what Claude Code doesn't read
            // (a backup beside an agent) stay behind.
            let wanted = !name.starts_with('.')
                && match kind {
                    "skills" => path.join("SKILL.md").is_file(),
                    _ => path.is_dir() || name.ends_with(".md"),
                };
            if !wanted {
                continue;
            }
            let target = dst.join(&name);
            // The account's own (not a copy of Lary's): left as it is.
            if target.exists() && !before.contains_key(&name) {
                continue;
            }
            let sig = signature(&path);
            if before.get(&name) != Some(&sig) || !target.exists() {
                remove(&target);
                if copy(&path, &target).is_err() {
                    remove(&target);
                    continue;
                }
            }
            copied.insert(name, sig);
        }
        // Copied before, gone from the user's folder since.
        for name in before.keys().filter(|n| !copied.contains_key(*n)) {
            remove(&dst.join(name));
        }
        if !copied.is_empty() {
            now.insert(kind.to_string(), copied);
        }
    }
    if now != had {
        match serde_json::to_string_pretty(&now) {
            Ok(t) if !now.is_empty() => {
                let _ = std::fs::write(&manifest_path, t);
            }
            _ => {
                let _ = std::fs::remove_file(&manifest_path);
            }
        }
    }
}

/// What a file or folder is now (its files' paths, sizes and times): a copy
/// is made again when it changes.
fn signature(path: &Path) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    fn walk(p: &Path, base: &Path, h: &mut std::collections::hash_map::DefaultHasher) {
        let Ok(meta) = std::fs::metadata(p) else { return };
        p.strip_prefix(base).unwrap_or(p).hash(h);
        if meta.is_dir() {
            let mut kids: Vec<PathBuf> = std::fs::read_dir(p).into_iter().flatten().flatten().map(|e| e.path()).collect();
            kids.sort();
            for k in kids {
                walk(&k, base, h);
            }
        } else {
            meta.len().hash(h);
            meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos()).hash(h);
        }
    }
    walk(path, path, &mut h);
    h.finish()
}

fn copy(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for e in std::fs::read_dir(from)?.flatten() {
            copy(&e.path(), &to.join(e.file_name()))?;
        }
        Ok(())
    } else {
        if let Some(dir) = to.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::copy(from, to).map(|_| ())
    }
}

fn remove(path: &Path) {
    if path.is_dir() {
        let _ = std::fs::remove_dir_all(path);
    } else {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, text: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    /// As the user's `~/.claude` is laid out: an agent with its MCP server in
    /// its header and a backup beside it, two skills and a trash folder; the
    /// account's folder has an agent of its own.
    #[test]
    fn brings_the_users_agents_skills_and_commands() {
        let root = std::env::temp_dir().join(format!("lary-user-items-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (user, home) = (root.join("user"), root.join("account"));
        let agent = "---\nname: ide-framework\ndescription: IDE records\nmcpServers:\n  - ide_framework:\n      type: stdio\n      command: node\n---\nYou read IDE records.\n";
        write(&user.join("agents/ide-framework.md"), agent);
        write(&user.join("agents/ide-framework.md.ide-agent-backup"), "old");
        write(&user.join("skills/designer/SKILL.md"), "---\nname: designer\n---\n");
        write(&user.join("skills/designer/references/notes.md"), "notes");
        write(&user.join("skills/.trash/old/SKILL.md"), "gone");
        write(&user.join("commands/ship.md"), "Ship it.");
        write(&home.join("agents/own.md"), "the account's own");

        bring(&user, &home);
        assert_eq!(std::fs::read_to_string(home.join("agents/ide-framework.md")).unwrap(), agent);
        assert!(home.join("skills/designer/references/notes.md").is_file());
        assert!(home.join("commands/ship.md").is_file());
        assert!(!home.join("agents/ide-framework.md.ide-agent-backup").exists(), "Claude Code reads only .md agents");
        assert!(!home.join("skills/.trash").exists(), "hidden folders stay behind");
        assert_eq!(std::fs::read_to_string(home.join("agents/own.md")).unwrap(), "the account's own");

        // Changed in ~/.claude: the copy follows. Gone: the copy goes, the
        // account's own stays.
        std::thread::sleep(std::time::Duration::from_millis(20));
        write(&user.join("agents/ide-framework.md"), "---\nname: ide-framework\n---\nNew prompt.\n");
        std::fs::remove_dir_all(user.join("skills/designer")).unwrap();
        bring(&user, &home);
        assert!(std::fs::read_to_string(home.join("agents/ide-framework.md")).unwrap().contains("New prompt."));
        assert!(!home.join("skills/designer").exists());
        assert!(home.join("agents/own.md").is_file());

        // The user's own agent of the same name as the account's: the
        // account's stays.
        write(&user.join("agents/own.md"), "the user's");
        bring(&user, &home);
        assert_eq!(std::fs::read_to_string(home.join("agents/own.md")).unwrap(), "the account's own");
        let _ = std::fs::remove_dir_all(&root);
    }
}
