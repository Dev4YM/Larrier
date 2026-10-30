//! Incremental import of every agent's native sessions into the store.
//!
//! Full syncs ([`sync`]) and live syncs of single files
//! ([`crate::live::sync_paths`]) share [`apply`], which is serialized so two
//! passes can never import the same native session twice.

use std::collections::HashSet;
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
    /// Re-import into this session; `true` when the source changed (not just
    /// the importer).
    Update(String, bool),
}

/// One sync pass at a time, across full and live syncs.
static SYNC_LOCK: Mutex<()> = Mutex::new(());

pub fn sync(store: &Store, agents: &[AgentKind]) -> SyncReport {
    let infos: Vec<NativeSessionInfo> = agents.iter().flat_map(discover).collect();
    let mut report = apply(store, infos, &|_| false);
    report.removed = drop_copies(store);
    report
}

/// Older versions imported a native session twice when two of its agent's
/// stores listed it in the same pass (a Cursor chat in its CLI store and
/// its transcript). No import record points to such a copy while another
/// session holds the native session; copies nobody changed are removed.
/// Forks, ported, renamed, pinned or rated sessions are always kept.
pub fn drop_copies(store: &Store) -> usize {
    let _guard = SYNC_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let held = store.imported_session_ids();
    let mut removed = 0;
    for sum in store.list() {
        if held.contains(&sum.id) || sum.pinned || sum.parent_id.is_some() {
            continue;
        }
        let Ok(s) = store.load(&sum.id) else { continue };
        let Some(native) = s.origin.native_id.clone() else { continue };
        let Some(rec) = store.import_record(&import_key(&s.origin.agent, &native)) else { continue };
        if rec.deleted || rec.session_id.is_empty() || rec.session_id == s.id || !store.exists(&rec.session_id) {
            continue;
        }
        let untouched = s.parent.is_none()
            && s.native.iter().all(|l| l.id == native)
            && s.extra.get("import_title").and_then(|t| t.as_str()) == Some(s.title.as_str())
            && !s.extra.keys().any(|k| k.starts_with("feedback:"));
        if untouched && store.delete(&s.id).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Import new and changed sessions among `infos`. `skip(session_id)` leaves
/// an existing Larrier session alone (for example while Larrier itself is
/// running it, since its agent writes the same native file).
/// Bump when an importer changes what it produces: sessions imported by an
/// older version are imported again on the next sync, keeping what the user
/// changed in Larrier.
pub const IMPORTER_VERSION: u32 = 6;

pub(crate) fn apply(store: &Store, infos: Vec<NativeSessionInfo>, skip: &dyn Fn(&str) -> bool) -> SyncReport {
    let _guard = SYNC_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut report = SyncReport::default();
    let mut todo: Vec<(NativeSessionInfo, Plan, String)> = Vec::new();
    // A native session listed twice (two of its agent's stores) is imported
    // once, from the first listing (agents list their richest store first).
    let mut seen: HashSet<String> = HashSet::new();
    for info in infos {
        let key = import_key(&info.agent, &info.native_id);
        if !seen.insert(key.clone()) {
            continue;
        }
        match store.import_record(&key) {
            Some(r) if r.linked || r.deleted => report.skipped += 1,
            Some(r) if !r.session_id.is_empty() && skip(&r.session_id) => report.skipped += 1,
            // Known-empty source that hasn't changed.
            Some(r) if r.session_id.is_empty() && r.source_mtime == info.mtime_ms && r.source_len == info.size && r.importer >= IMPORTER_VERSION => report.skipped += 1,
            Some(r) if !r.session_id.is_empty() && store.exists(&r.session_id) => {
                let source_changed = r.source_mtime != info.mtime_ms || r.source_len != info.size;
                if !source_changed && r.importer >= IMPORTER_VERSION {
                    report.skipped += 1;
                } else {
                    todo.push((info, Plan::Update(r.session_id, source_changed), key));
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
                    let mut unchanged = false;
                    match plan {
                        Plan::Update(id, source_changed) => {
                            if let Ok(old) = store.load(id) {
                                merge_user_state(&mut s, &old);
                                keep_ids(&mut s, &old);
                                if same_conversation(&s, &old) {
                                    // Touched, not continued (an agent rewriting
                                    // its store, a chat opened in its app): it
                                    // keeps its place and its time.
                                    s.updated_at = old.updated_at;
                                    s.origin.imported_at = old.origin.imported_at;
                                    unchanged = s == old;
                                } else if *source_changed && grew(&s, &old) {
                                    report.grew.push(id.clone());
                                }
                            }
                            s.id = id.clone();
                            report.updated += 1;
                        }
                        Plan::New => report.imported += 1,
                    }
                    store.set_import_record(key, ImportRecord { session_id: s.id.clone(), source_mtime: info.mtime_ms, source_len: info.size, importer: IMPORTER_VERSION, ..Default::default() });
                    if !unchanged {
                        report.changed.push(s.id.clone());
                        batch.push(s);
                    }
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

/// Messages already imported keep their ids (open transcripts, edits and
/// feedback refer to them): the shared run of messages by role.
fn keep_ids(new: &mut Session, old: &Session) {
    for (n, o) in new.messages.iter_mut().zip(&old.messages) {
        if n.role != o.role {
            break;
        }
        n.id = o.id.clone();
    }
}

/// The same messages (by role and content), whatever else changed.
fn same_conversation(a: &Session, b: &Session) -> bool {
    a.messages.len() == b.messages.len() && a.messages.iter().zip(&b.messages).all(|(x, y)| x.role == y.role && x.parts == y.parts)
}

/// The agent added to the conversation: more messages, or a longer last one.
fn grew(new: &Session, old: &Session) -> bool {
    let size = |s: &Session| s.messages.last().map(|m| m.parts.len() + m.text().len()).unwrap_or(0);
    new.messages.len() > old.messages.len() || (new.messages.len() == old.messages.len() && size(new) > size(old))
}

/// Keep what the user changed in Larrier when a native session is re-imported.
fn merge_user_state(new: &mut Session, old: &Session) {
    new.pinned = old.pinned;
    new.unread = old.unread;
    // Thumbs up / down and other Larrier-only notes.
    for (k, v) in old.extra.iter().filter(|(k, _)| k.starts_with("feedback:")) {
        new.extra.insert(k.clone(), v.clone());
    }
    // Archived in the agent follows the agent, unless the user (un)archived
    // it in Larrier.
    let old_import_archived = old.extra.get("import_archived").and_then(|a| a.as_bool()).unwrap_or(false);
    if old.archived != old_import_archived {
        new.archived = old.archived;
    }
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

    /// Two listings of one native session in a pass import it once.
    #[test]
    fn a_session_listed_twice_is_imported_once() {
        let native = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = Store::open(data.path()).unwrap();
        let id = "31111111-2222-3333-4444-555555555555";
        let mut s = Session::new(AgentKind::ClaudeCode, None);
        s.push(larrier_core::Message::user_text("hello"));
        let mut infos = Vec::new();
        for dir in ["a", "b"] {
            let d = native.path().join(dir);
            std::fs::create_dir_all(&d).unwrap();
            let f = d.join(format!("{id}.jsonl"));
            std::fs::write(&f, crate::claude::export_lines(&s, id)).unwrap();
            infos.push(crate::claude::file_info(&f).unwrap());
        }
        let r = apply(&store, infos, &|_| false);
        assert_eq!((r.imported, store.list().len()), (1, 1), "{r:?}");
    }

    /// Copies left by older versions go; the session that holds the native
    /// session, forks and changed copies stay.
    #[test]
    fn untouched_copies_are_removed() {
        let data = tempfile::tempdir().unwrap();
        let store = Store::open(data.path()).unwrap();
        let imported = |title: &str| {
            let mut s = Session::new(AgentKind::Cursor, None);
            s.title = title.into();
            s.extra.insert("import_title".into(), Value::String("Chat".into()));
            s.origin.native_id = Some("chat-1".into());
            s.push(larrier_core::Message::user_text("hi"));
            s
        };
        let held = imported("Chat");
        store.save(&held).unwrap();
        store.set_import_record(&import_key(&AgentKind::Cursor, "chat-1"), ImportRecord { session_id: held.id.clone(), ..Default::default() });
        let copy = imported("Chat");
        let renamed = imported("My chat");
        let mut fork = imported("Chat");
        fork.parent = Some(larrier_core::lsf::ForkRef { session_id: held.id.clone(), message_id: None });
        for s in [&copy, &renamed, &fork] {
            store.save(s).unwrap();
        }
        assert_eq!(drop_copies(&store), 1);
        assert!(!store.exists(&copy.id));
        assert!(store.exists(&held.id) && store.exists(&renamed.id) && store.exists(&fork.id));
    }

    #[test]
    fn archived_follows_the_agent_unless_changed_in_larrier() {
        let archived_in_agent = |on: bool| {
            let mut s = imported("auto");
            s.archived = on;
            if on {
                s.extra.insert("import_archived".into(), Value::Bool(true));
            }
            s
        };
        // Archived in the agent since the last import: archived here too.
        let mut new = archived_in_agent(true);
        merge_user_state(&mut new, &archived_in_agent(false));
        assert!(new.archived);
        // Unarchived in the agent: back in the list.
        let mut new = archived_in_agent(false);
        merge_user_state(&mut new, &archived_in_agent(true));
        assert!(!new.archived);
        // Archived in Larrier: stays archived.
        let mut old = archived_in_agent(false);
        old.archived = true;
        let mut new = archived_in_agent(false);
        merge_user_state(&mut new, &old);
        assert!(new.archived);
        // Brought back in Larrier: stays in the list.
        let mut old = archived_in_agent(true);
        old.archived = false;
        let mut new = archived_in_agent(true);
        merge_user_state(&mut new, &old);
        assert!(!new.archived);
    }
}
