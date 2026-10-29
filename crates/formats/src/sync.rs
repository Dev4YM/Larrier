//! Incremental import of every agent's native sessions into the store.
//!
//! Full syncs ([`sync`]) and live syncs of single files
//! ([`crate::live::sync_paths`]) share [`apply`], which is serialized so two
//! passes can never import the same native session twice.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use anyhow::Result;
use larrier_core::{
    store::{import_key, ImportRecord},
    AgentKind, Session, Store,
};

use crate::util::ImportSkip;
use crate::{discover, import_native, NativeSessionInfo, SyncReport};

const CHUNK: usize = 48;

enum Plan {
    New,
    Update(String),
}

/// One sync pass at a time, across full and live syncs.
static SYNC_LOCK: Mutex<()> = Mutex::new(());

pub fn sync(store: &Store, agents: &[AgentKind]) -> SyncReport {
    let infos: Vec<NativeSessionInfo> = agents.iter().flat_map(discover).collect();
    apply(store, infos, &|_| false)
}

/// Import new and changed sessions among `infos`. `skip(session_id)` leaves
/// an existing Larrier session alone (for example while Larrier itself is
/// running it, since its agent writes the same native file).
/// Bump when an importer changes what it produces: sessions imported by an
/// older version are imported again on the next sync, keeping what the user
/// changed in Larrier.
pub const IMPORTER_VERSION: u32 = 2;

pub(crate) fn apply(store: &Store, infos: Vec<NativeSessionInfo>, skip: &dyn Fn(&str) -> bool) -> SyncReport {
    let _guard = SYNC_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut report = SyncReport::default();
    let mut todo: Vec<(NativeSessionInfo, Plan, String)> = Vec::new();
    for info in infos {
        let key = import_key(&info.agent, &info.native_id);
        match store.import_record(&key) {
            Some(r) if r.linked || r.deleted => report.skipped += 1,
            Some(r) if !r.session_id.is_empty() && skip(&r.session_id) => report.skipped += 1,
            // Known-empty source that hasn't changed.
            Some(r) if r.session_id.is_empty() && r.source_mtime == info.mtime_ms && r.source_len == info.size && r.importer >= IMPORTER_VERSION => report.skipped += 1,
            Some(r) if !r.session_id.is_empty() && store.exists(&r.session_id) => {
                if r.source_mtime == info.mtime_ms && r.source_len == info.size && r.importer >= IMPORTER_VERSION {
                    report.skipped += 1;
                } else {
                    todo.push((info, Plan::Update(r.session_id), key));
                }
            }
            _ => todo.push((info, Plan::New, key)),
        }
    }

    // Newest first so the sidebar fills with recent work quickly.
    todo.sort_by_key(|t| std::cmp::Reverse(t.0.mtime_ms));
    for chunk in todo.chunks(CHUNK) {
        let results = import_parallel(chunk.iter().map(|(i, _, _)| i).collect());
        let mut batch: Vec<Session> = Vec::new();
        for ((info, plan, key), res) in chunk.iter().zip(results) {
            match res {
                Ok(mut s) => {
                    match plan {
                        Plan::Update(id) => {
                            if let Ok(old) = store.load(id) {
                                merge_user_state(&mut s, &old);
                            }
                            s.id = id.clone();
                            report.updated += 1;
                        }
                        Plan::New => report.imported += 1,
                    }
                    store.set_import_record(key, ImportRecord { session_id: s.id.clone(), source_mtime: info.mtime_ms, source_len: info.size, importer: IMPORTER_VERSION, ..Default::default() });
                    report.changed.push(s.id.clone());
                    batch.push(s);
                }
                Err(e) if e.downcast_ref::<ImportSkip>().is_some() => {
                    // Remember empty sessions so they aren't re-parsed every run.
                    if matches!(plan, Plan::New) {
                        store.set_import_record(key, ImportRecord { session_id: String::new(), source_mtime: info.mtime_ms, source_len: info.size, importer: IMPORTER_VERSION, ..Default::default() });
                    }
                    report.skipped += 1;
                }
                Err(e) => report.errors.push(format!("{} {}: {e:#}", info.agent.id(), info.native_id)),
            }
        }
        if let Err(e) = store.save_many(&batch) {
            report.errors.push(format!("saving: {e:#}"));
        }
        store.flush_imports();
    }
    store.flush_imports();
    report
}

/// Keep what the user changed in Larrier when a native session is re-imported.
fn merge_user_state(new: &mut Session, old: &Session) {
    new.pinned = old.pinned;
    new.unread = old.unread;
    // Thumbs up / down and other Larrier-only notes.
    for (k, v) in old.extra.iter().filter(|(k, _)| k.starts_with("feedback:")) {
        new.extra.insert(k.clone(), v.clone());
    }
    new.archived = old.archived || new.archived;
    new.tags = old.tags.clone();
    new.account_id = old.account_id.clone();
    new.parent = old.parent.clone();
    let old_import_mode = old.extra.get("import_mode").and_then(|m| m.as_str());
    if old.mode.as_deref() != old_import_mode {
        new.mode = old.mode.clone();
    }
    let old_import_permission = old.extra.get("import_permission").and_then(|m| m.as_str());
    if old.permission.as_deref() != old_import_permission {
        new.permission = old.permission.clone();
    }
    let old_import_title = old.extra.get("import_title").and_then(|t| t.as_str());
    if old_import_title.map(|t| t != old.title).unwrap_or(false) {
        new.title = old.title.clone();
    }
    for link in &old.native {
        if new.native_link(&link.agent).is_none() {
            new.native.push(link.clone());
        }
    }
    if old.agent != new.origin.agent {
        // The user continued this session with another agent in Larrier.
        new.agent = old.agent.clone();
        new.model = old.model.clone();
    }
}

fn import_parallel(infos: Vec<&NativeSessionInfo>) -> Vec<Result<Session>> {
    let n = infos.len();
    let workers = std::thread::available_parallelism().map(|p| p.get()).unwrap_or(4).clamp(1, 8).min(n.max(1));
    let next = AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<Result<Session>>>> = (0..n).map(|_| Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                if i >= n {
                    break;
                }
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| import_native(infos[i])))
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("importer panicked")));
                *slots[i].lock().unwrap() = Some(r);
            });
        }
    });
    slots.into_iter().map(|m| m.into_inner().unwrap().unwrap_or_else(|| Err(anyhow::anyhow!("not imported")))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn imported(mode: &str) -> Session {
        let mut s = Session::new(AgentKind::ClaudeCode, None);
        s.mode = Some(mode.into());
        s.extra.insert("import_mode".into(), Value::String(mode.into()));
        s
    }

    #[test]
    fn reimport_keeps_a_mode_picked_in_larrier() {
        // Unchanged in Larrier: the native mode wins.
        let mut new = imported("plan");
        merge_user_state(&mut new, &imported("auto"));
        assert_eq!(new.mode.as_deref(), Some("plan"));

        // Picked in Larrier: kept.
        let mut old = imported("auto");
        old.mode = Some("acceptEdits".into());
        let mut new = imported("plan");
        merge_user_state(&mut new, &old);
        assert_eq!(new.mode.as_deref(), Some("acceptEdits"));
    }
}
