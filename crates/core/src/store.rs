//! On-disk store for sessions and configuration.
//!
//! Layout (under `%APPDATA%\Larrier`, `~/Library/Application Support/Larrier`
//! or `~/.local/share/larrier`, overridable with `LARRIER_HOME`):
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
    if let Ok(p) = std::env::var("LARRIER_HOME") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    let base = dirs::data_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")));
    if cfg!(target_os = "linux") {
        base.join("larrier")
    } else {
        base.join("Larrier")
    }
}

/// Fingerprint of an imported native session so re-imports update in place.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct ImportRecord {
    pub session_id: String,
    /// Source modification time (unix millis) when last imported.
    pub source_mtime: i64,
    #[serde(default)]
    pub source_len: u64,
    /// The native session is driven by Larrier itself (we ran the agent or
    /// exported into it); sync must never re-import it as a new session.
    #[serde(default)]
    pub linked: bool,
    /// The user deleted the Larrier copy: never import this native session
    /// again (its own files are left untouched).
    #[serde(default)]
    pub deleted: bool,
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
        match std::fs::read_to_string(&p) {
            Ok(t) => serde_json::from_str(&t).unwrap_or_else(|e| {
                tracing::error!("config.json is invalid ({e}); using defaults");
                let _ = std::fs::copy(&p, self.root.join("config.invalid.json"));
                Config::default()
            }),
            Err(_) => Config::default(),
        }
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
    fn save_list_delete() {
        let dir = std::env::temp_dir().join(format!("larrier-test-{}", crate::lsf::new_id()));
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
}
