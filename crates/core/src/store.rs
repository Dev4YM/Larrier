//! On-disk store for sessions and configuration.
//!
//! Layout (under `%APPDATA%\Lary`, `~/Library/Application Support/Lary`
//! or `~/.local/share/lary`, overridable with `LARY_HOME`):
//!
//! ```text
//! config.json            settings, agents, accounts, providers, workspaces
//! index.json             session summaries for fast listing
//! sessions/<id>.lsf.jsonl
//! imports.json           native session fingerprints already imported
//! worktrees/             default location for agent worktrees
//! accounts/<id>/         isolated config dirs for multi-account CLIs
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::lsf::{Session, SessionSummary, LSF_EXT_JSONL};

pub fn data_root() -> PathBuf {
    // `LARRIER_HOME` from before the rename still works.
    for var in ["LARY_HOME", "LARRIER_HOME"] {
        if let Ok(p) = std::env::var(var) {
            if !p.is_empty() {
                return PathBuf::from(p);
            }
        }
    }
    default_root("lary", "Lary")
}

/// `%APPDATA%\<Name>`, `~/Library/Application Support/<Name>` or
/// `~/.local/share/<name>`.
fn default_root(linux: &str, other: &str) -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")));
    base.join(if cfg!(target_os = "linux") { linux } else { other })
}

/// Lary was called Larrier: the first time Lary runs, the folder Larrier
/// kept its data in becomes Lary's. Paths saved inside it (worktrees, the
/// wallpapers and project pictures, the per-account folders) are pointed at
/// the new place, and git worktrees moved with it are repaired. Nothing
/// happens with a folder set by `LARY_HOME` / `LARRIER_HOME`, or once
/// Lary's own folder exists. Returns what happened, for the log.
pub fn migrate_from_larrier() -> Option<String> {
    if ["LARY_HOME", "LARRIER_HOME"].iter().any(|v| std::env::var(v).is_ok_and(|p| !p.is_empty())) {
        return None;
    }
    migrate_dir(&default_root("larrier", "Larrier"), &data_root())
}

/// Move `old` to `new` and point the paths saved inside at the new place
/// (see [`migrate_from_larrier`]).
fn migrate_dir(old: &Path, new: &Path) -> Option<String> {
    if new.exists() || !old.is_dir() {
        return None;
    }
    if let Err(e) = std::fs::rename(old, new) {
        return Some(format!("couldn't move {} to {}: {e}", old.display(), new.display()));
    }
    let (from, to) = (old.display().to_string(), new.display().to_string());
    // As saved in JSON (backslashes doubled on Windows) and with forward
    // slashes.
    let pairs = [
        (from.replace('\\', "\\\\"), to.replace('\\', "\\\\")),
        (from.replace('\\', "/"), to.replace('\\', "/")),
    ];
    let fix = |text: &str| pairs.iter().fold(text.to_string(), |t, (a, b)| t.replace(a.as_str(), b.as_str()));
    for name in ["config.json", "index.json"] {
        let p = new.join(name);
        if let Ok(t) = std::fs::read_to_string(&p) {
            let fixed = fix(&t);
            if fixed != t {
                let _ = std::fs::write(&p, fixed);
            }
        }
    }
    // A session's header line holds where it runs (a worktree's path);
    // its messages are left as they were.
    if let Ok(rd) = std::fs::read_dir(new.join("sessions")) {
        for e in rd.flatten() {
            let Ok(t) = std::fs::read_to_string(e.path()) else { continue };
            let (head, rest) = t.split_once('\n').unwrap_or((t.as_str(), ""));
            let fixed = fix(head);
            if fixed != head {
                let _ = std::fs::write(e.path(), format!("{fixed}\n{rest}"));
            }
        }
    }
    // Worktrees moved with the folder: git finds them again.
    let mut repaired = 0;
    for project in std::fs::read_dir(new.join("worktrees")).into_iter().flatten().flatten() {
        for tree in std::fs::read_dir(project.path()).into_iter().flatten().flatten() {
            if tree.path().join(".git").is_file() {
                let ok = std::process::Command::new("git").arg("-C").arg(tree.path()).args(["worktree", "repair"]).output().is_ok_and(|o| o.status.success());
                repaired += ok as usize;
            }
        }
    }
    Some(format!("moved {from} to {to}; {repaired} worktrees repaired"))
}

/// Fingerprint of an imported native session so re-imports update in place.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct ImportRecord {
    pub session_id: String,
    /// Source modification time (unix millis) when last imported.
    pub source_mtime: i64,
    #[serde(default)]
    pub source_len: u64,
    /// The native session is driven by Lary itself (we ran the agent or
    /// exported into it); sync must never re-import it as a new session.
    #[serde(default)]
    pub linked: bool,
    /// The user deleted the Lary copy: never import this native session
    /// again (its own files are left untouched).
    #[serde(default)]
    pub deleted: bool,
    /// Version of the importer that produced the session; sessions from an
    /// older one are imported again (see `lary_formats::IMPORTER_VERSION`).
    #[serde(default)]
    pub importer: u32,
}

impl ImportRecord {
    pub fn linked(session_id: impl Into<String>) -> Self {
        ImportRecord { session_id: session_id.into(), linked: true, ..Default::default() }
    }
}

/// Key used for import fingerprints: `<agent-id>:<native-id>`.
pub fn import_key(agent: &crate::AgentKind, native_id: &str) -> String {
    format!("{}:{}", agent.id(), native_id)
}

#[derive(Clone)]
pub struct Store {
    root: PathBuf,
    index: Arc<Mutex<Option<BTreeMap<String, SessionSummary>>>>,
    imports: Arc<Mutex<Option<BTreeMap<String, ImportRecord>>>>,
}

impl Store {
    pub fn open_default() -> Result<Store> {
        Store::open(data_root())
    }

    pub fn open(root: impl Into<PathBuf>) -> Result<Store> {
        let root = root.into();
        std::fs::create_dir_all(root.join("sessions")).with_context(|| format!("creating {}", root.display()))?;
        Ok(Store { root, index: Arc::new(Mutex::new(None)), imports: Arc::new(Mutex::new(None)) })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }

    pub fn worktrees_dir(&self) -> PathBuf {
        self.root.join("worktrees")
    }

    pub fn accounts_dir(&self) -> PathBuf {
        self.root.join("accounts")
    }

    pub fn session_path(&self, id: &str) -> PathBuf {
        self.sessions_dir().join(format!("{id}.{LSF_EXT_JSONL}"))
    }

    // -- config -------------------------------------------------------------

    pub fn load_config(&self) -> Config {
        let p = self.root.join("config.json");
        let mut cfg = match std::fs::read_to_string(&p) {
            Ok(t) => serde_json::from_str(&t).unwrap_or_else(|e| {
                tracing::error!("config.json is invalid ({e}); using defaults");
                let _ = std::fs::copy(&p, self.root.join("config.invalid.json"));
                Config::default()
            }),
            Err(_) => Config::default(),
        };
        let before = cfg.version;
        cfg.migrate();
        // Written once in this version's form, so what the migration made
        // (accounts and their ids) stays put; what the older Lary wrote
        // is kept beside it.
        if before < cfg.version && p.exists() {
            let _ = std::fs::copy(&p, self.root.join(format!("config.v{before}.json")));
            if let Err(e) = self.save_config(&cfg) {
                tracing::warn!("couldn't save the updated config: {e}");
            }
        }
        cfg
    }

    pub fn save_config(&self, cfg: &Config) -> Result<()> {
        atomic_write(&self.root.join("config.json"), serde_json::to_string_pretty(cfg)?.as_bytes())
    }

    // -- sessions -----------------------------------------------------------

    fn with_index<R>(&self, f: impl FnOnce(&mut BTreeMap<String, SessionSummary>) -> R) -> R {
        let mut guard = self.index.lock().unwrap();
        if guard.is_none() {
            *guard = Some(self.read_index());
        }
        f(guard.as_mut().unwrap())
    }

    fn read_index(&self) -> BTreeMap<String, SessionSummary> {
        let p = self.root.join("index.json");
        if let Ok(t) = std::fs::read_to_string(&p) {
            if let Ok(list) = serde_json::from_str::<Vec<SessionSummary>>(&t) {
                let mut map: BTreeMap<String, SessionSummary> = list.into_iter().map(|s| (s.id.clone(), s)).collect();
                // Drop entries whose files vanished.
                map.retain(|id, _| self.session_path(id).exists());
                return map;
            }
        }
        self.rebuild_index_map()
    }

    fn rebuild_index_map(&self) -> BTreeMap<String, SessionSummary> {
        let mut map = BTreeMap::new();
        if let Ok(rd) = std::fs::read_dir(self.sessions_dir()) {
            for e in rd.flatten() {
                let p = e.path();
                if p.to_string_lossy().ends_with(LSF_EXT_JSONL) {
                    if let Ok(t) = std::fs::read_to_string(&p) {
                        if let Ok(s) = Session::from_jsonl(&t) {
                            map.insert(s.id.clone(), s.summary());
                        }
                    }
                }
            }
        }
        map
    }

    fn flush_index(&self, map: &BTreeMap<String, SessionSummary>) {
        let list: Vec<&SessionSummary> = map.values().collect();
        if let Ok(t) = serde_json::to_string(&list) {
            if let Err(e) = atomic_write(&self.root.join("index.json"), t.as_bytes()) {
                tracing::error!("writing index: {e}");
            }
        }
    }

    pub fn rebuild_index(&self) {
        let map = self.rebuild_index_map();
        self.flush_index(&map);
        *self.index.lock().unwrap() = Some(map);
    }

    /// All session summaries, newest first.
    pub fn list(&self) -> Vec<SessionSummary> {
        let mut v: Vec<SessionSummary> = self.with_index(|m| m.values().cloned().collect());
        v.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        v
    }

    pub fn exists(&self, id: &str) -> bool {
        self.session_path(id).exists()
    }

    pub fn load(&self, id: &str) -> Result<Session> {
        let p = self.session_path(id);
        let t = std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
        Ok(Session::from_jsonl(&t)?)
    }

    pub fn save(&self, session: &Session) -> Result<()> {
        atomic_write(&self.session_path(&session.id), session.to_jsonl().as_bytes())?;
        let summary = session.summary();
        self.with_index(|m| {
            m.insert(summary.id.clone(), summary);
            self.flush_index(m);
        });
        Ok(())
    }

    /// Save many sessions with a single index flush (bulk import).
    pub fn save_many(&self, sessions: &[Session]) -> Result<()> {
        for s in sessions {
            atomic_write(&self.session_path(&s.id), s.to_jsonl().as_bytes())?;
        }
        self.with_index(|m| {
            for s in sessions {
                m.insert(s.id.clone(), s.summary());
            }
            self.flush_index(m);
        });
        Ok(())
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        let p = self.session_path(id);
        if p.exists() {
            std::fs::remove_file(&p)?;
        }
        self.with_index(|m| {
            m.remove(id);
            self.flush_index(m);
        });
        // Keep a tombstone so sync (and live sync) never bring it back.
        self.with_imports(|m| {
            for r in m.values_mut().filter(|r| r.session_id == id) {
                r.deleted = true;
            }
        });
        self.flush_imports();
        Ok(())
    }

    // -- import fingerprints ------------------------------------------------

    fn with_imports<R>(&self, f: impl FnOnce(&mut BTreeMap<String, ImportRecord>) -> R) -> R {
        let mut guard = self.imports.lock().unwrap();
        if guard.is_none() {
            let p = self.root.join("imports.json");
            let map = std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
            *guard = Some(map);
        }
        f(guard.as_mut().unwrap())
    }

    /// Sessions that a (non-deleted) import record points to.
    pub fn imported_session_ids(&self) -> std::collections::HashSet<String> {
        self.with_imports(|m| m.values().filter(|r| !r.deleted && !r.session_id.is_empty()).map(|r| r.session_id.clone()).collect())
    }

    pub fn import_record(&self, key: &str) -> Option<ImportRecord> {
        self.with_imports(|m| m.get(key).cloned())
    }

    pub fn set_import_record(&self, key: &str, rec: ImportRecord) {
        self.with_imports(|m| {
            m.insert(key.to_string(), rec);
        });
    }

    pub fn flush_imports(&self) {
        let snapshot = self.with_imports(|m| m.clone());
        if let Ok(t) = serde_json::to_string(&snapshot) {
            let _ = atomic_write(&self.root.join("imports.json"), t.as_bytes());
        }
    }
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    // On Windows rename fails if the target is momentarily locked; retry.
    let mut last = None;
    for _ in 0..5 {
        match std::fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
    }
    let _ = std::fs::remove_file(&tmp);
    Err(last.map(anyhow::Error::from).unwrap_or_else(|| anyhow::anyhow!("rename failed")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentKind;
    use crate::lsf::Message;

    #[test]
    fn larriers_folder_becomes_larys_with_its_paths() {
        let base = std::env::temp_dir().join(format!("lary-migrate-{}", crate::lsf::new_id()));
        let (old, new) = (base.join("Larrier"), base.join("Lary"));
        std::fs::create_dir_all(old.join("sessions")).unwrap();
        let o = old.display().to_string();
        let esc = |p: &str| p.replace('\\', "\\\\");
        std::fs::write(old.join("config.json"), format!(r#"{{"home_bg":{{"image":"{}"}}}}"#, esc(&format!("{o}/wallpapers/a.png")))).unwrap();
        let head = format!(r#"{{"kind":"session","environment":{{"type":"worktree","path":"{}"}}}}"#, esc(&format!("{o}/worktrees/web/agent-1")));
        let msg = format!(r#"{{"kind":"message","text":"{}"}}"#, esc(&o));
        std::fs::write(old.join("sessions/s.lsf.jsonl"), format!("{head}\n{msg}\n")).unwrap();

        assert!(migrate_dir(&old, &new).is_some());
        assert!(!old.exists() && new.join("config.json").exists());
        let n = new.display().to_string();
        let config = std::fs::read_to_string(new.join("config.json")).unwrap();
        assert!(config.contains(&esc(&format!("{n}/wallpapers/a.png"))), "{config}");
        let session = std::fs::read_to_string(new.join("sessions/s.lsf.jsonl")).unwrap();
        let (h, rest) = session.split_once('\n').unwrap();
        assert!(h.contains(&esc(&format!("{n}/worktrees"))), "{h}");
        assert!(rest.contains(&esc(&o)), "messages stay as they were");
        // Once Lary's folder is there, nothing moves again.
        std::fs::create_dir_all(&old).unwrap();
        assert!(migrate_dir(&old, &new).is_none());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn save_list_delete() {
        let dir = std::env::temp_dir().join(format!("lary-test-{}", crate::lsf::new_id()));
        let store = Store::open(&dir).unwrap();
        let mut s = Session::new(AgentKind::Codex, None);
        s.push(Message::user_text("hello"));
        store.save(&s).unwrap();
        assert_eq!(store.list().len(), 1);
        let back = store.load(&s.id).unwrap();
        assert_eq!(back.messages.len(), 1);
        // Index survives a fresh handle.
        let store2 = Store::open(&dir).unwrap();
        assert_eq!(store2.list().len(), 1);
        store2.delete(&s.id).unwrap();
        assert!(store2.list().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A config an older Lary wrote is saved once in today's form, with
    /// the old file kept beside it, so the accounts it became keep their ids.
    #[test]
    fn migrated_config_is_saved_once() {
        let dir = std::env::temp_dir().join(format!("lary-test-{}", crate::lsf::new_id()));
        let store = Store::open(&dir).unwrap();
        let old = r#"{"version": 3, "agents": [{"kind": "codex", "endpoints": [{"id": "e1", "name": "Local", "kind": "open_ai_compatible", "base_url": "http://localhost:8000/v1"}]}]}"#;
        std::fs::write(dir.join("config.json"), old).unwrap();
        let first = store.load_config();
        assert_eq!(std::fs::read_to_string(dir.join("config.v3.json")).unwrap(), old);
        let saved = std::fs::read_to_string(dir.join("config.json")).unwrap();
        assert!(saved.contains(&format!("\"version\": {}", crate::config::CONFIG_VERSION)) && !saved.contains("endpoints"), "{saved}");
        let again = store.load_config();
        assert_eq!(first.accounts, again.accounts, "the same accounts, ids included");
        let _ = std::fs::remove_dir_all(dir);
    }
}
