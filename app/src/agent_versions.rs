//! Keeping agents current: the latest version each one has published
//! (looked up soon after Larrier starts, every few hours, or when asked),
//! and installing or updating one from Settings → Agents the way its makers
//! say to (see `larrier_agents::updates`), its output shown as it runs.

use std::collections::HashMap;
use std::process::Stdio;
use std::time::Duration;

use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use larrier_agents::updates;
use larrier_core::AgentKind;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::state::App;

/// The latest version of each agent (by id), as last looked up.
pub static LATEST: GlobalSignal<HashMap<String, String>> = Signal::global(HashMap::new);
/// The latest versions are being looked up.
pub static CHECKING: GlobalSignal<bool> = Signal::global(|| false);
/// Installs and updates, running or done, by agent id.
pub static JOBS: GlobalSignal<HashMap<String, Job>> = Signal::global(HashMap::new);

#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub installing: bool,
    /// The command, as it reads.
    pub shown: String,
    /// What it printed (the last lines).
    pub log: Vec<String>,
    /// `None` while it runs, then whether it went through.
    pub ok: Option<bool>,
}

const LOG_LINES: usize = 400;

/// Look the latest versions up a little after Larrier starts, then every
/// six hours.
pub fn start() {
    spawn_forever(async move {
        tokio::time::sleep(Duration::from_secs(20)).await;
        loop {
            check().await;
            tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
        }
    });
}

/// Look up now (from Settings).
pub fn check_now() {
    spawn_forever(check());
}

async fn check() {
    if *CHECKING.peek() {
        return;
    }
    *CHECKING.write() = true;
    let kinds: Vec<AgentKind> = AgentKind::BUILTIN.iter().filter(|k| updates::of(k).is_some()).cloned().collect();
    let found = futures::future::join_all(kinds.into_iter().map(|k| async move { (k.id(), updates::latest(&k).await) })).await;
    {
        let mut latest = LATEST.write();
        for (id, v) in found {
            if let Ok(v) = v {
                latest.insert(id, v);
            }
        }
    }
    *CHECKING.write() = false;
}

/// The newer version an installed agent can update to.
pub fn update_to(app: &App, kind: &AgentKind) -> Option<String> {
    let installed = app.detected.read().iter().find(|d| &d.kind == kind && d.installed())?.version.clone()?;
    let latest = LATEST.read().get(&kind.id())?.clone();
    updates::is_newer(&latest, &installed).then_some(latest)
}

/// How many installed agents have a newer version.
pub fn outdated(app: &App) -> usize {
    let kinds: Vec<AgentKind> = app.detected.read().iter().filter(|d| d.installed()).map(|d| d.kind.clone()).collect();
    kinds.iter().filter(|k| update_to(app, k).is_some()).count()
}

/// The command that would install (`install`) or update `kind` here.
pub fn step(app: &App, kind: &AgentKind, install: bool) -> Option<updates::Step> {
    if install {
        return updates::install_step(kind);
    }
    let path = app.detected.peek().iter().find(|d| &d.kind == kind).and_then(|d| d.path.clone())?;
    updates::update_step(kind, &path)
}

/// Install or update `kind`, its output shown as it runs; the agents are
/// found again afterwards and the outcome is told.
pub fn run(app: App, kind: AgentKind, install: bool) {
    let Some(step) = step(&app, &kind, install) else {
        app.toast_err(format!("Larrier can't {} {} itself: see its site", if install { "install" } else { "update" }, kind.display_name()));
        return;
    };
    let id = kind.id();
    if JOBS.peek().get(&id).is_some_and(|j| j.ok.is_none()) {
        return;
    }
    JOBS.write().insert(id.clone(), Job { installing: install, shown: step.shown.clone(), log: Vec::new(), ok: None });
    let push = {
        let id = id.clone();
        move |line: String| {
            if let Some(j) = JOBS.write().get_mut(&id) {
                j.log.push(line);
                let extra = j.log.len().saturating_sub(LOG_LINES);
                j.log.drain(..extra);
            }
        }
    };
    spawn_forever(async move {
        let before = app.detected.peek().iter().find(|d| d.kind == kind).and_then(|d| d.version.clone());
        let mut cmd = tokio::process::Command::new(&step.program);
        cmd.args(&step.args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        if let Some(home) = dirs::home_dir() {
            cmd.current_dir(home);
        }
        // A Git Bash variable some agents' installers misread.
        cmd.env_remove("MSYSTEM");
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000);
        let ok = match cmd.spawn() {
            Err(e) => {
                push(format!("Couldn't start it: {e}"));
                false
            }
            Ok(mut child) => {
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
                for out in [child.stdout.take().map(|o| Box::new(o) as Box<dyn tokio::io::AsyncRead + Unpin + Send>), child.stderr.take().map(|o| Box::new(o) as _)].into_iter().flatten() {
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        let mut lines = BufReader::new(out).lines();
                        while let Ok(Some(l)) = lines.next_line().await {
                            let _ = tx.send(l);
                        }
                    });
                }
                drop(tx);
                let waited = tokio::time::timeout(Duration::from_secs(15 * 60), async {
                    while let Some(l) = rx.recv().await {
                        let l = plain(&l);
                        if !l.trim().is_empty() {
                            push(l);
                        }
                    }
                    child.wait().await
                })
                .await;
                match waited {
                    Ok(Ok(status)) => {
                        if !status.success() {
                            push(format!("Exited with code {}", status.code().unwrap_or(-1)));
                        }
                        status.success()
                    }
                    Ok(Err(e)) => {
                        push(e.to_string());
                        false
                    }
                    Err(_) => {
                        push("Stopped: it ran for 15 minutes".into());
                        false
                    }
                }
            }
        };
        if let Some(j) = JOBS.write().get_mut(&id) {
            j.ok = Some(ok);
        }
        // Found again, with the version it has now.
        let cfg = app.config.peek().clone();
        let found = larrier_agents::detect_all(&cfg).await;
        let mut detected = app.detected;
        detected.set(found);
        let now = app.detected.peek().iter().find(|d| d.kind == kind && d.installed()).and_then(|d| d.version.clone());
        let name = kind.display_name();
        match (ok, now) {
            (true, Some(v)) if before.as_deref() != Some(v.as_str()) => app.toast(format!("{name} is now v{v}")),
            (true, Some(v)) => app.toast(format!("{name} v{v} is the version it has")),
            (true, None) => app.toast(format!("{name}: done, but Larrier doesn't find it yet (a new terminal or sign-in may be needed)")),
            (false, _) => app.toast_err(format!("{name}: the {} didn't go through — its output is in Settings → Agents", if install { "install" } else { "update" })),
        }
    });
}

/// A line without its terminal colors and cursor moves, as it last read
/// (a progress bar redrawn with `\r` shows its last state).
fn plain(line: &str) -> String {
    let line = line.rsplit('\r').find(|s| !s.trim().is_empty()).unwrap_or("");
    let mut out = String::with_capacity(line.len());
    let mut it = line.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\x1b' {
            if !c.is_control() || c == '\t' {
                out.push(c);
            }
            continue;
        }
        match it.peek() {
            // CSI: parameters, then a final letter.
            Some('[') => {
                it.next();
                for d in it.by_ref() {
                    if ('@'..='~').contains(&d) {
                        break;
                    }
                }
            }
            // OSC: up to BEL or ESC \.
            Some(']') => {
                it.next();
                while let Some(d) = it.next() {
                    if d == '\x07' || (d == '\x1b' && it.peek() == Some(&'\\')) {
                        if d == '\x1b' {
                            it.next();
                        }
                        break;
                    }
                }
            }
            _ => {
                it.next();
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn output_reads_plain() {
        assert_eq!(super::plain("\x1b[32m✔\x1b[0m Installed \x1b]0;title\x07v2.1.316"), "✔ Installed v2.1.316");
        assert_eq!(super::plain("10%\r50%\r100% done"), "100% done");
        assert_eq!(super::plain("ok\r"), "ok");
    }
}
