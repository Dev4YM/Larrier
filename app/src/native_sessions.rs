//! The harnesses' own sessions on this computer, for bringing one in by
//! hand: File → Import Session makes one Lary's own, and @ in the composer
//! sends one's conversation as context. They're looked for off the UI
//! thread and kept for a minute; the chats Lary ran itself are left out
//! (they're Lary's already).

use std::cmp::Reverse;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use lary_core::store::import_key;
use lary_formats::NativeSessionInfo;

use crate::state::store;
use crate::ui::mentions::MentionChip;

/// What was found, newest first; `None` until it's been looked for.
pub static FOUND: GlobalSignal<Option<Vec<NativeSessionInfo>>> = Signal::global(|| None);

static LOOKED: Mutex<Option<Instant>> = Mutex::new(None);
static LOOKING: AtomicBool = AtomicBool::new(false);

/// Look for them, unless that was done in the last minute.
pub fn refresh() {
    let recent = LOOKED.lock().ok().and_then(|t| *t).is_some_and(|t| t.elapsed() < Duration::from_secs(60));
    if recent || LOOKING.swap(true, Ordering::SeqCst) {
        return;
    }
    spawn_forever(async move {
        let list = tokio::task::spawn_blocking(find).await.unwrap_or_default();
        *FOUND.write() = Some(list);
        if let Ok(mut t) = LOOKED.lock() {
            *t = Some(Instant::now());
        }
        LOOKING.store(false, Ordering::SeqCst);
    });
}

fn find() -> Vec<NativeSessionInfo> {
    let mut seen = HashSet::new();
    let mut list: Vec<NativeSessionInfo> = lary_formats::discover_all()
        .into_iter()
        .filter(|i| seen.insert(import_key(&i.agent, &i.native_id)))
        .filter(|i| match store().import_record(&import_key(&i.agent, &i.native_id)) {
            // Lary ran it: it's Lary's already.
            Some(r) if r.linked => false,
            // Known to hold no messages, and unchanged since.
            Some(r) if r.session_id.is_empty() && r.source_mtime == i.mtime_ms && r.source_len == i.size => false,
            _ => true,
        })
        .collect();
    // By when each was last used, as its row says.
    list.sort_by_key(|i| Reverse(last_used(i).map(|t| t.timestamp_millis()).unwrap_or(i.mtime_ms)));
    list
}

fn last_used(info: &NativeSessionInfo) -> Option<chrono::DateTime<chrono::Utc>> {
    info.updated_at.or_else(|| chrono::DateTime::from_timestamp_millis(info.mtime_ms))
}

/// The Lary session already made from it, when it's one of Lary's own.
pub fn in_lary(info: &NativeSessionInfo, sessions: &[lary_core::SessionSummary]) -> Option<String> {
    let r = store().import_record(&import_key(&info.agent, &info.native_id))?;
    sessions.iter().find(|s| s.id == r.session_id && s.managed).map(|s| s.id.clone())
}

pub fn title(info: &NativeSessionInfo) -> String {
    info.title.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| "Untitled session".into())
}

/// "Claude Code · 2h · lary": its harness, when it was last used, its folder.
pub fn detail(info: &NativeSessionInfo) -> String {
    let when = last_used(info);
    let folder = info.cwd.as_deref().map(|c| c.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(c).to_string());
    [Some(info.agent.display_name().to_string()), when.map(crate::state::rel_time), folder].into_iter().flatten().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")
}

/// Whether it matches what was typed: its title, folder or harness.
pub fn matches(info: &NativeSessionInfo, ql: &str) -> bool {
    ql.is_empty() || title(info).to_lowercase().contains(ql) || info.cwd.as_deref().is_some_and(|c| c.to_lowercase().contains(ql)) || info.agent.display_name().to_lowercase().contains(ql)
}

/// Its conversation as an object for the composer, sent as context (read
/// now, not imported).
pub fn context_chip(info: &NativeSessionInfo) -> anyhow::Result<MentionChip> {
    let s = lary_formats::import_native(info)?;
    let attr = |v: &str| v.replace('"', "'").replace(['\n', '\r'], " ");
    let block = format!("<agent_context title=\"{}\" agent=\"{}\">\n{}\n</agent_context>", attr(&s.title), s.agent.display_name(), s.handoff_context(0, 24_000).trim());
    Ok(MentionChip::harness_session(&s.title, &s.agent, &info.native_id, block))
}
