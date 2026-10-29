//! Automation scheduler: starts agent runs when an automation's schedule
//! comes due or its repository's HEAD moves.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use chrono::{Local, Utc};
use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use larrier_core::automation::{Automation, Trigger};

use crate::state::{App, View};

const TICK: Duration = Duration::from_secs(30);

/// Start the background scheduler. Call once, from the root component.
pub fn start(app: App) {
    spawn_forever(async move {
        // Let startup (imports, agent detection) settle first.
        tokio::time::sleep(Duration::from_secs(5)).await;
        loop {
            tick(app).await;
            tokio::time::sleep(TICK).await;
        }
    });
}

/// Run an automation immediately, regardless of its trigger.
pub fn run_now(app: App, id: &str) {
    let Some(a) = app.config.read().automations.iter().find(|a| a.id == id).cloned() else { return };
    if is_busy(&app, &a) {
        app.toast(format!("“{}” is already running", a.name));
        return;
    }
    if launch(app, &a).is_some() {
        app.toast(format!("Started “{}”", a.name));
    }
}

async fn tick(app: App) {
    sync_statuses(app);
    let now = Local::now();
    let autos: Vec<Automation> = app.config.read().automations.clone();

    // HEAD of every workspace watched by an on-commit automation.
    let watched: HashSet<String> = autos
        .iter()
        .filter(|a| a.enabled && matches!(a.trigger, Trigger::OnGitChange { .. }))
        .filter_map(|a| a.workspace.clone())
        .collect();
    let heads: HashMap<String, (Option<String>, Option<String>)> = if watched.is_empty() {
        HashMap::new()
    } else {
        tokio::task::spawn_blocking(move || watched.into_iter().map(|ws| (ws.clone(), (git_head(&ws), git_branch(&ws)))).collect())
            .await
            .unwrap_or_default()
    };

    for a in autos {
        if !a.enabled || is_busy(&app, &a) {
            continue;
        }
        let due = match &a.trigger {
            Trigger::OnGitChange { branch } => {
                let Some(ws) = &a.workspace else { continue };
                let Some((Some(head), cur_branch)) = heads.get(ws).cloned() else { continue };
                if let Some(b) = branch.as_ref().filter(|b| !b.trim().is_empty()) {
                    if cur_branch.as_deref() != Some(b.trim()) {
                        continue;
                    }
                }
                match &a.last_head {
                    // First sighting: remember where HEAD is, don't run.
                    None => {
                        set_head(app, &a.id, head);
                        false
                    }
                    Some(prev) if *prev != head => {
                        set_head(app, &a.id, head);
                        true
                    }
                    Some(_) => false,
                }
            }
            _ => a.is_due(now),
        };
        if due {
            launch(app, &a);
        }
    }
}

fn set_head(app: App, id: &str, head: String) {
    app.update_config(|c| {
        if let Some(x) = c.automations.iter_mut().find(|x| x.id == id) {
            x.last_head = Some(head);
        }
    });
}

/// The automation's previous run is still working.
fn is_busy(app: &App, a: &Automation) -> bool {
    a.last_session_id.as_deref().map(|s| app.is_running(s)).unwrap_or(false)
}

/// Mirror finished runs' outcomes into the automation's last status.
fn sync_statuses(app: App) {
    let updates: Vec<(String, String)> = {
        let cfg = app.config.read();
        let sessions = app.sessions.read();
        cfg.automations
            .iter()
            .filter(|a| a.last_status.as_deref() == Some("running"))
            .filter_map(|a| {
                let sid = a.last_session_id.as_deref()?;
                if app.is_running(sid) {
                    return None;
                }
                let st = sessions.iter().find(|s| s.id == sid).map(|s| s.status.label().to_string()).unwrap_or_else(|| "Finished".into());
                Some((a.id.clone(), st))
            })
            .collect()
    };
    if updates.is_empty() {
        return;
    }
    app.update_config(|c| {
        for (id, st) in &updates {
            if let Some(x) = c.automations.iter_mut().find(|x| &x.id == id) {
                x.last_status = Some(st.clone());
            }
        }
    });
}

/// Start one run in a new agent session without taking over the window.
/// Returns the new session id.
fn launch(mut app: App, a: &Automation) -> Option<String> {
    // Everything `send` touches that belongs to the user.
    let prev_view = app.view.read().clone();
    let prev_draft = app.draft.read().clone();
    let (prev_history, prev_pos, prev_quote) = {
        let mut ui = app.ui.write();
        (ui.history.clone(), ui.hist_pos, ui.quote.take())
    };

    {
        let mut d = app.draft.write();
        d.agent = a.agent.clone();
        d.model = a.model.clone().filter(|m| !m.trim().is_empty());
        d.mode = None;
        d.permission = None;
        d.account_id = None;
        d.workspace = a.workspace.clone();
        d.worktree = a.worktree;
    }
    app.send(None, a.prompt.clone(), Vec::new());

    let started = match app.view.read().clone() {
        View::Agent(id) if prev_view != View::Agent(id.clone()) => Some(id),
        _ => None,
    };

    app.draft.set(prev_draft);
    {
        let mut ui = app.ui.write();
        ui.history = prev_history;
        ui.hist_pos = prev_pos;
        ui.quote = prev_quote;
    }
    app.view.set(prev_view);

    if let Some(id) = &started {
        app.rename(id, format!("⟳ {}", a.name));
    }
    let now = Utc::now();
    let sid = started.clone();
    let aid = a.id.clone();
    app.update_config(move |c| {
        if let Some(x) = c.automations.iter_mut().find(|x| x.id == aid) {
            x.last_run = Some(now);
            x.runs += 1;
            match sid {
                Some(s) => {
                    x.last_session_id = Some(s);
                    x.last_status = Some("running".into());
                }
                None => x.last_status = Some("Could not start".into()),
            }
        }
    });
    started
}

fn git(ws: &str, args: &[&str]) -> Option<String> {
    let mut c = std::process::Command::new("git");
    c.arg("-C").arg(ws).args(args).env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    let out = c.output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|s| !s.is_empty())
}

fn git_head(ws: &str) -> Option<String> {
    git(ws, &["rev-parse", "HEAD"])
}

fn git_branch(ws: &str) -> Option<String> {
    git(ws, &["rev-parse", "--abbrev-ref", "HEAD"])
}
