//! Live sessions: Larrier watches every agent's session folders and
//! re-imports what changes, so a session running in a terminal, an IDE or
//! another app updates here while it runs.
//!
//! The watcher and the importing run off the UI thread; the UI receives the
//! fresh sessions and swaps them in place (open transcripts, tabs and the
//! sidebar row update without a full refresh).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use larrier_core::Session;
use larrier_formats::live::LiveWatcher;

use crate::state::{store, App};

/// A session counts as live while its agent wrote to it this recently.
pub const LIVE_FOR: Duration = Duration::from_secs(25);
/// After Larrier's own turn ends, its agent may still flush the native file.
const GRACE: Duration = Duration::from_secs(20);

/// Sessions Larrier itself is driving: `None` while running, else when the
/// turn ended.
fn busy() -> &'static Mutex<HashMap<String, Option<Instant>>> {
    static BUSY: OnceLock<Mutex<HashMap<String, Option<Instant>>>> = OnceLock::new();
    BUSY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Last external write per Larrier session id.
fn last_write() -> &'static Mutex<HashMap<String, Instant>> {
    static LAST: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    LAST.get_or_init(|| Mutex::new(HashMap::new()))
}

fn watcher() -> &'static Mutex<Option<LiveWatcher>> {
    static W: OnceLock<Mutex<Option<LiveWatcher>>> = OnceLock::new();
    W.get_or_init(|| Mutex::new(None))
}

/// Fresh sessions, and the ids among them whose conversation grew.
type Fresh = (Vec<Session>, Vec<String>);

static TX: OnceLock<tokio::sync::mpsc::UnboundedSender<Fresh>> = OnceLock::new();

/// Larrier started a turn on this session: live sync leaves it alone.
pub fn mark_busy(id: &str) {
    busy().lock().unwrap().insert(id.to_string(), None);
}

/// Larrier's turn ended; live sync resumes after a short grace period.
pub fn mark_idle(id: &str) {
    busy().lock().unwrap().insert(id.to_string(), Some(Instant::now()));
}

/// Whether live sync must skip this session right now.
pub fn is_busy(id: &str) -> bool {
    is_busy_at(&busy().lock().unwrap(), id, Instant::now())
}

fn is_busy_at(map: &HashMap<String, Option<Instant>>, id: &str, now: Instant) -> bool {
    match map.get(id) {
        Some(None) => true,
        Some(Some(ended)) => now.duration_since(*ended) < GRACE,
        None => false,
    }
}

/// Start live sync (once, from the root component).
pub fn start(app: App) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Fresh>();
    if TX.set(tx).is_err() {
        return;
    }
    spawn_forever(async move {
        while let Some((sessions, grew)) = rx.recv().await {
            apply(app, sessions, &grew);
        }
    });
    // Expire the live markers with a cheap tick that only touches them.
    spawn_forever(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            expire(app);
        }
    });
    restart(app);
}

/// (Re)start or stop the watcher to match the settings.
pub fn restart(app: App) {
    let (on, agents) = {
        let cfg = app.config.read();
        let agents: Vec<_> = larrier_formats::importable_agents().into_iter().filter(|a| cfg.agent(a).import && cfg.agent(a).enabled).collect();
        (cfg.settings.live_sync, agents)
    };
    // Dropping the old watcher stops it.
    *watcher().lock().unwrap() = None;
    if !on || agents.is_empty() {
        return;
    }
    let Some(tx) = TX.get().cloned() else { return };
    // Registering watches on big folders can take a moment: not on the UI thread.
    std::thread::spawn(move || {
        let st = store().clone();
        let on_report = move |r: larrier_formats::SyncReport| {
            for e in r.errors.iter().take(3) {
                tracing::warn!("live sync: {e}");
            }
            // Load off the UI thread; the UI only swaps the result in.
            let sessions: Vec<Session> = r.changed.iter().filter_map(|id| store().load(id).ok()).collect();
            if !sessions.is_empty() {
                let _ = tx.send((sessions, r.grew));
            }
        };
        match larrier_formats::live::spawn(st, agents, Arc::new(is_busy), Box::new(on_report)) {
            Ok(w) => *watcher().lock().unwrap() = Some(w),
            Err(e) => tracing::warn!("live sync could not start: {e}"),
        }
    });
}

/// "Watching 6 agents" for the settings page.
pub fn status(enabled: bool) -> String {
    if !enabled {
        return "Off — sessions update when you import.".into();
    }
    match watcher().lock().unwrap().as_ref() {
        Some(w) if !w.agents().is_empty() => {
            let names: Vec<String> = w.agents().iter().map(|a| a.display_name()).collect();
            let n = names.len();
            format!("Watching {n} agent{}: {}", if n == 1 { "" } else { "s" }, names.join(", "))
        }
        Some(_) => "No agent session folders found to watch.".into(),
        None => "Starting…".into(),
    }
}

/// Swap fresh sessions into the UI. Only those whose agent added to the
/// conversation (`grew`) get the unread dot, and count as being worked on
/// when that was just now; the rest (an agent rewriting its store, a newer
/// importer) update quietly.
fn apply(mut app: App, sessions: Vec<Session>, grew: &[String]) {
    let now = Instant::now();
    tracing::debug!("live sync: showing {} session(s)", sessions.len());
    for mut s in sessions {
        let id = s.id.clone();
        if app.is_running(&id) {
            continue;
        }
        let grew = grew.contains(&id);
        let fresh = grew && s.messages.last().map(|m| chrono::Utc::now() - m.created_at < chrono::Duration::from_std(LIVE_FOR * 4).unwrap_or_default()).unwrap_or(false);
        let visible = app.current_id().as_deref() == Some(id.as_str()) || app.split.peek().as_deref() == Some(id.as_str());
        if grew && !visible && !s.unread {
            s.unread = true;
            crate::state::save_async(s.clone());
        }
        app.apply_external(s);
        if fresh {
            last_write().lock().unwrap().insert(id.clone(), now);
            if !app.live.peek().contains_key(&id) {
                app.live.write().insert(id, now);
            }
        }
    }
}

fn expire(mut app: App) {
    let stale: Vec<String> = {
        let last = last_write().lock().unwrap();
        app.live.peek().keys().filter(|id| last.get(*id).map(|t| t.elapsed() > LIVE_FOR).unwrap_or(true)).cloned().collect()
    };
    if !stale.is_empty() {
        app.live.write().retain(|id, _| !stale.contains(id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_sessions_are_skipped_until_the_grace_period_ends() {
        let mut map = HashMap::new();
        let t0 = Instant::now();
        assert!(!is_busy_at(&map, "a", t0));
        map.insert("a".to_string(), None);
        assert!(is_busy_at(&map, "a", t0 + Duration::from_secs(600)), "running sessions stay busy");
        map.insert("a".to_string(), Some(t0));
        assert!(is_busy_at(&map, "a", t0 + Duration::from_secs(5)));
        assert!(!is_busy_at(&map, "a", t0 + GRACE + Duration::from_secs(1)));
    }
}
