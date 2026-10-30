//! Live sync: watch every agent's native session store and re-import the
//! sessions that change while they change.
//!
//! * [`watch_specs`] lists what to watch per agent (session folders, or the
//!   database file for agents that keep sessions in SQLite).
//! * [`sync_paths`] re-imports just the sessions behind a set of changed
//!   files; database-backed agents are rescanned incrementally instead.
//! * [`spawn`] runs a file watcher with per-file debouncing on its own
//!   thread and reports what changed.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use anyhow::Result;
use larrier_core::{AgentKind, Store};
use notify::{RecursiveMode, Watcher};

use crate::{claude, codex, cursor, discover, droid, gemini, hermes, opencode, pi, NativeSessionInfo, SyncReport};

/// Quiet time after the last write before a session file is re-imported.
const FILE_QUIET: Duration = Duration::from_millis(500);
/// A file written continuously is still re-imported this often.
const FILE_MAX_WAIT: Duration = Duration::from_secs(2);
/// Database stores change with every UI interaction; rescan them less often.
const DB_QUIET: Duration = Duration::from_millis(1500);
const DB_MAX_WAIT: Duration = Duration::from_secs(6);

/// One place to watch for an agent.
#[derive(Clone, Debug, PartialEq)]
pub struct WatchSpec {
    pub agent: AgentKind,
    /// Folder to watch.
    pub path: PathBuf,
    /// Watch the whole tree; otherwise only the files directly in `path`
    /// whose name starts with `prefix`.
    pub recursive: bool,
    pub prefix: Option<String>,
    /// Session files map one to one onto sessions. When false (database
    /// stores), a change rescans the agent's sessions incrementally.
    pub per_file: bool,
}

impl WatchSpec {
    fn tree(agent: AgentKind, path: PathBuf, per_file: bool) -> WatchSpec {
        WatchSpec { agent, path, recursive: true, prefix: None, per_file }
    }

    /// A single file (and its `-wal` / `-journal` siblings).
    fn file(agent: AgentKind, file: &Path) -> Option<WatchSpec> {
        Some(WatchSpec {
            agent,
            path: file.parent()?.to_path_buf(),
            recursive: false,
            prefix: Some(file.file_name()?.to_string_lossy().to_string()),
            per_file: false,
        })
    }

    pub fn matches(&self, p: &Path) -> bool {
        if self.recursive {
            return p.starts_with(&self.path);
        }
        p.parent() == Some(self.path.as_path())
            && p.file_name().map(|n| n.to_string_lossy().starts_with(self.prefix.as_deref().unwrap_or(""))).unwrap_or(false)
    }

    /// Debounce key for a raw change: the session file it belongs to, or one
    /// key per database store.
    fn key_for(&self, p: &Path) -> Option<PathBuf> {
        if !self.per_file {
            return Some(self.path.join(self.prefix.as_deref().unwrap_or("_")));
        }
        match self.agent {
            AgentKind::ClaudeCode => claude::session_file_for(p),
            _ => Some(p.to_path_buf()),
        }
    }

    fn exists(&self) -> bool {
        self.path.is_dir()
    }
}

/// What to watch for these agents (only places that exist right now).
pub fn watch_specs(agents: &[AgentKind]) -> Vec<WatchSpec> {
    let mut out = Vec::new();
    for a in agents {
        match a {
            AgentKind::ClaudeCode => out.push(WatchSpec::tree(a.clone(), claude::root(None).join("projects"), true)),
            AgentKind::Codex => {
                let root = codex::root(None);
                out.push(WatchSpec::tree(a.clone(), root.join("sessions"), true));
                out.push(WatchSpec::tree(a.clone(), root.join("archived_sessions"), true));
            }
            AgentKind::Gemini => out.push(WatchSpec::tree(a.clone(), gemini::root(None).join("tmp"), true)),
            AgentKind::Pi => out.push(WatchSpec::tree(a.clone(), pi::root(None).join("sessions"), true)),
            AgentKind::Droid => out.push(WatchSpec::tree(a.clone(), droid::root(), true)),
            AgentKind::Cursor => {
                out.push(WatchSpec::tree(a.clone(), cursor::config_dir().join("projects"), false));
                if let Some(db) = cursor::ide_db() {
                    out.extend(WatchSpec::file(a.clone(), &db));
                }
            }
            AgentKind::OpenCode => {
                out.extend(WatchSpec::file(a.clone(), &opencode::db_path()));
                out.push(WatchSpec::tree(a.clone(), opencode::data_dir().join("storage").join("session"), false));
            }
            AgentKind::Hermes => out.extend(WatchSpec::file(a.clone(), &hermes::home_dir().join("state.db"))),
            _ => {}
        }
    }
    out.retain(WatchSpec::exists);
    out
}

/// Session info for one changed file of a per-file agent.
fn info_for(agent: &AgentKind, path: &Path, cache: &mut InfoCache) -> Option<NativeSessionInfo> {
    match agent {
        AgentKind::ClaudeCode => claude::file_info(&claude::session_file_for(path)?),
        AgentKind::Codex => {
            let names = cache.codex_names.get_or_insert_with(|| codex::thread_names(&codex::root(None)));
            codex::file_info(path, names)
        }
        AgentKind::Gemini => {
            let projects = cache.gemini_projects.get_or_insert_with(|| gemini::project_map(&gemini::root(None)));
            gemini::file_info(path, projects)
        }
        AgentKind::Pi => pi::file_info(path),
        AgentKind::Droid => droid::file_info(path),
        _ => None,
    }
}

/// Per-batch lookups shared by several changed files.
#[derive(Default)]
struct InfoCache {
    codex_names: Option<HashMap<String, String>>,
    gemini_projects: Option<Vec<(String, String)>>,
}

/// Re-import the sessions behind these changed files. `skip(session_id)`
/// leaves a Larrier session alone (e.g. while Larrier itself runs it).
pub fn sync_paths(store: &Store, agents: &[AgentKind], paths: &[PathBuf], skip: &dyn Fn(&str) -> bool) -> SyncReport {
    sync_paths_in(store, &watch_specs(agents), paths, skip)
}

/// [`sync_paths`] against explicit watch specs.
pub fn sync_paths_in(store: &Store, specs: &[WatchSpec], paths: &[PathBuf], skip: &dyn Fn(&str) -> bool) -> SyncReport {
    let mut infos: Vec<NativeSessionInfo> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut rescan: Vec<AgentKind> = Vec::new();
    let mut cache = InfoCache::default();
    for p in paths {
        let Some(spec) = specs.iter().find(|s| s.matches(p)) else { continue };
        if !spec.per_file {
            if !rescan.contains(&spec.agent) {
                rescan.push(spec.agent.clone());
            }
            continue;
        }
        if let Some(info) = info_for(&spec.agent, p, &mut cache) {
            if seen.insert(info.path.clone()) {
                infos.push(info);
            }
        }
    }
    for a in &rescan {
        infos.extend(discover(a));
    }
    if infos.is_empty() {
        return SyncReport::default();
    }
    crate::sync::apply(store, infos, skip)
}

/// Collects raw change events and releases each key once its writes have
/// settled (`quiet`), or after `max_wait` for a file written continuously.
#[derive(Debug)]
pub struct Debouncer {
    quiet: Duration,
    max_wait: Duration,
    pending: HashMap<PathBuf, (Instant, Instant)>,
}

impl Debouncer {
    pub fn new(quiet: Duration, max_wait: Duration) -> Debouncer {
        Debouncer { quiet, max_wait, pending: HashMap::new() }
    }

    pub fn push(&mut self, key: PathBuf, now: Instant) {
        let e = self.pending.entry(key).or_insert((now, now));
        e.1 = now;
    }

    /// Keys that are ready now (removed from the pending set).
    pub fn take_ready(&mut self, now: Instant) -> Vec<PathBuf> {
        let ready: Vec<PathBuf> = self
            .pending
            .iter()
            .filter(|(_, (first, last))| now.duration_since(*last) >= self.quiet || now.duration_since(*first) >= self.max_wait)
            .map(|(k, _)| k.clone())
            .collect();
        for k in &ready {
            self.pending.remove(k);
        }
        ready
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

/// A running live sync. Dropping it stops watching.
pub struct LiveWatcher {
    _watcher: notify::RecommendedWatcher,
    stop: Arc<AtomicBool>,
    agents: Vec<AgentKind>,
    folders: usize,
}

impl LiveWatcher {
    /// Agents with at least one watched folder.
    pub fn agents(&self) -> &[AgentKind] {
        &self.agents
    }

    pub fn folders(&self) -> usize {
        self.folders
    }
}

impl Drop for LiveWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Start live sync for `agents`. `on_report` runs on the watcher thread
/// whenever sessions were imported or updated.
pub fn spawn(
    store: Store,
    agents: Vec<AgentKind>,
    skip: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    on_report: Box<dyn Fn(SyncReport) + Send>,
) -> Result<LiveWatcher> {
    let specs = watch_specs(&agents);
    let (tx, rx) = mpsc::channel::<PathBuf>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            if matches!(ev.kind, notify::EventKind::Access(_)) {
                return;
            }
            for p in ev.paths {
                let _ = tx.send(p);
            }
        }
    })?;
    let mut watching: Vec<(PathBuf, bool)> = Vec::new();
    for s in &specs {
        let key = (s.path.clone(), s.recursive);
        if watching.contains(&key) {
            continue;
        }
        let mode = if s.recursive { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive };
        match watcher.watch(&s.path, mode) {
            Ok(()) => watching.push(key),
            Err(e) => tracing::warn!("live sync: cannot watch {}: {e}", s.path.display()),
        }
    }
    let mut live_agents: Vec<AgentKind> = Vec::new();
    for s in specs.iter().filter(|s| watching.iter().any(|(p, _)| *p == s.path)) {
        if !live_agents.contains(&s.agent) {
            live_agents.push(s.agent.clone());
        }
    }

    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    std::thread::Builder::new().name("larrier-live-sync".into()).spawn(move || {
        let mut files = Debouncer::new(FILE_QUIET, FILE_MAX_WAIT);
        let mut dbs = Debouncer::new(DB_QUIET, DB_MAX_WAIT);
        let route = |p: PathBuf, files: &mut Debouncer, dbs: &mut Debouncer, now: Instant| {
            if let Some(spec) = specs.iter().find(|s| s.matches(&p)) {
                if let Some(key) = spec.key_for(&p) {
                    if spec.per_file { files.push(key, now) } else { dbs.push(key, now) }
                }
            }
        };
        loop {
            if stop2.load(Ordering::Relaxed) {
                break;
            }
            match rx.recv_timeout(Duration::from_millis(150)) {
                Ok(p) => {
                    let now = Instant::now();
                    route(p, &mut files, &mut dbs, now);
                    while let Ok(p) = rx.try_recv() {
                        route(p, &mut files, &mut dbs, now);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if files.is_empty() && dbs.is_empty() {
                continue;
            }
            let now = Instant::now();
            let mut ready = files.take_ready(now);
            ready.extend(dbs.take_ready(now));
            if ready.is_empty() {
                continue;
            }
            let started = Instant::now();
            let report = sync_paths_in(&store, &specs, &ready, &*skip);
            tracing::debug!("live sync: {} changed path(s) → {} updated, {} grew in {:?}", ready.len(), report.changed.len(), report.grew.len(), started.elapsed());
            if !report.changed.is_empty() || !report.errors.is_empty() {
                on_report(report);
            }
        }
    })?;
    Ok(LiveWatcher { _watcher: watcher, stop, agents: live_agents, folders: watching.len() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use larrier_core::{Message, Session};

    fn claude_spec(root: &Path) -> WatchSpec {
        WatchSpec::tree(AgentKind::ClaudeCode, root.to_path_buf(), true)
    }

    fn write_claude_session(dir: &Path, id: &str, prompts: &[&str]) -> PathBuf {
        let mut s = Session::new(AgentKind::ClaudeCode, None);
        for p in prompts {
            s.push(Message::user_text(*p));
            let mut a = Message::assistant(AgentKind::ClaudeCode, None);
            a.push_text(&format!("Answer to {p}"));
            s.push(a);
        }
        let file = dir.join(format!("{id}.jsonl"));
        std::fs::write(&file, claude::export_lines(&s, id)).unwrap();
        file
    }

    #[test]
    fn debouncer_waits_for_quiet_and_caps_the_wait() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut d = Debouncer::new(ms(500), ms(2000));
        let a = PathBuf::from("a.jsonl");
        d.push(a.clone(), t0);
        assert!(d.take_ready(t0 + ms(300)).is_empty());
        d.push(a.clone(), t0 + ms(400));
        assert!(d.take_ready(t0 + ms(800)).is_empty(), "the second write restarts the quiet period");
        assert_eq!(d.take_ready(t0 + ms(950)), vec![a.clone()]);
        assert!(d.is_empty());

        // A file written every 200 ms is still released after `max_wait`.
        let mut d = Debouncer::new(ms(500), ms(2000));
        let mut t = t0;
        let mut released = None;
        for i in 0..20 {
            d.push(a.clone(), t);
            if !d.take_ready(t).is_empty() {
                released = Some(i);
                break;
            }
            t += ms(200);
        }
        assert_eq!(released, Some(10));
    }

    #[test]
    fn specs_match_files_and_database_siblings() {
        let tree = claude_spec(Path::new("/x/projects"));
        assert!(tree.matches(Path::new("/x/projects/p/a.jsonl")));
        assert!(!tree.matches(Path::new("/x/other/a.jsonl")));
        let db = WatchSpec::file(AgentKind::Hermes, Path::new("/h/state.db")).unwrap();
        assert!(db.matches(Path::new("/h/state.db-wal")));
        assert!(!db.matches(Path::new("/h/other.db")));
        assert_eq!(db.key_for(Path::new("/h/state.db-wal")), db.key_for(Path::new("/h/state.db")));
    }

    #[test]
    fn subagent_files_map_to_their_session() {
        let p = Path::new("/x/projects/proj/abc/subagents/agent-1.jsonl");
        assert_eq!(claude::session_file_for(p), Some(PathBuf::from("/x/projects/proj/abc.jsonl")));
        assert_eq!(claude::session_file_for(Path::new("/x/projects/proj/abc/tool-results/1.txt")), None);
    }

    #[test]
    fn sync_paths_imports_updates_skips_and_respects_deletes() {
        let native = tempfile::tempdir().unwrap();
        let proj = native.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = Store::open(data.path()).unwrap();
        let specs = vec![claude_spec(native.path())];
        let never = |_: &str| false;

        // New session file: imported.
        let file = write_claude_session(&proj, "11111111-2222-3333-4444-555555555555", &["first"]);
        let r = sync_paths_in(&store, &specs, std::slice::from_ref(&file), &never);
        assert_eq!((r.imported, r.updated), (1, 0), "{r:?}");
        let id = r.changed[0].clone();
        assert_eq!(store.load(&id).unwrap().messages.len(), 2);

        // Unchanged file: nothing to do.
        let r = sync_paths_in(&store, &specs, std::slice::from_ref(&file), &never);
        assert!(r.changed.is_empty());

        // The agent keeps writing: the same Larrier session is updated in place.
        std::thread::sleep(Duration::from_millis(20));
        write_claude_session(&proj, "11111111-2222-3333-4444-555555555555", &["first", "second"]);
        let r = sync_paths_in(&store, &specs, std::slice::from_ref(&file), &never);
        assert_eq!((r.imported, r.updated, r.changed.clone()), (0, 1, vec![id.clone()]));
        assert_eq!(store.load(&id).unwrap().messages.len(), 4);

        // While Larrier itself runs the session it is left alone.
        std::thread::sleep(Duration::from_millis(20));
        write_claude_session(&proj, "11111111-2222-3333-4444-555555555555", &["first", "second", "third"]);
        let busy = |sid: &str| sid == id;
        let r = sync_paths_in(&store, &specs, std::slice::from_ref(&file), &busy);
        assert!(r.changed.is_empty() && r.skipped == 1);

        // A subagent transcript changing re-imports its parent session.
        let sub = proj.join("11111111-2222-3333-4444-555555555555").join("subagents");
        std::fs::create_dir_all(&sub).unwrap();
        let agent_file = sub.join("agent-a1.jsonl");
        std::fs::write(&agent_file, "{}\n").unwrap();
        let r = sync_paths_in(&store, &specs, &[agent_file], &never);
        assert_eq!(r.changed, vec![id.clone()]);

        // Deleted in Larrier: never comes back, even when the file changes.
        store.delete(&id).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        write_claude_session(&proj, "11111111-2222-3333-4444-555555555555", &["first", "second", "third", "fourth"]);
        let r = sync_paths_in(&store, &specs, std::slice::from_ref(&file), &never);
        assert!(r.changed.is_empty(), "{r:?}");
        assert!(!store.exists(&id));

        // Paths outside the watched folders are ignored.
        let r = sync_paths_in(&store, &specs, &[data.path().join("x.jsonl")], &never);
        assert_eq!(r, SyncReport::default());
    }

    /// An agent rewriting its store without adding to a conversation must
    /// not make it look new: no unread dot, no "running", same place.
    #[test]
    fn touched_sessions_are_not_news() {
        let native = tempfile::tempdir().unwrap();
        let proj = native.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = Store::open(data.path()).unwrap();
        let specs = vec![claude_spec(native.path())];
        let never = |_: &str| false;
        let sid = "21111111-2222-3333-4444-555555555555";

        let file = write_claude_session(&proj, sid, &["first"]);
        let r = sync_paths_in(&store, &specs, std::slice::from_ref(&file), &never);
        let id = r.changed[0].clone();
        assert!(r.grew.is_empty(), "a first import is not news");
        let before = store.load(&id).unwrap();

        // Same conversation written again (and stamped later).
        std::thread::sleep(Duration::from_millis(20));
        let text = std::fs::read_to_string(&file).unwrap();
        std::fs::write(&file, format!("{text}{}
", serde_json::json!({"type": "system", "subtype": "noop", "sessionId": sid}))).unwrap();
        let r = sync_paths_in(&store, &specs, std::slice::from_ref(&file), &never);
        assert_eq!(r.updated, 1, "{r:?}");
        assert!(r.grew.is_empty(), "{r:?}");
        let after = store.load(&id).unwrap();
        assert_eq!(after.updated_at, before.updated_at);

        // A new turn is news, and the earlier messages keep their ids.
        std::thread::sleep(Duration::from_millis(20));
        write_claude_session(&proj, sid, &["first", "second"]);
        let r = sync_paths_in(&store, &specs, std::slice::from_ref(&file), &never);
        assert_eq!(r.grew, vec![id.clone()]);
        let grown = store.load(&id).unwrap();
        assert_eq!(grown.messages.len(), 4);
        assert_eq!(grown.messages[0].id, before.messages[0].id);
        assert_eq!(grown.messages[1].id, before.messages[1].id);
    }
}
