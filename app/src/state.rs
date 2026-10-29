//! Application state (signals) and every action the UI can take.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use dioxus::prelude::*;
use dioxus::core::spawn_forever;
use larrier_agents::{
    engine::TurnRecorder, events::PermissionRequest, git, AgentEvent, Detected, TurnControl, TurnHandle,
};
use larrier_core::{
    lsf::{new_id, MediaSource},
    store::{import_key, ImportRecord},
    AccountAuth, AgentKind, Config, Environment, Message, Part, Role, Session, SessionStatus, SessionSummary, Store,
    Theme, WorkspaceRef,
};
use larrier_formats::ExportFormat;

static STORE: OnceLock<Store> = OnceLock::new();

pub fn store() -> &'static Store {
    STORE.get_or_init(|| Store::open_default().expect("cannot open Larrier data directory"))
}

#[derive(Clone, PartialEq, Debug)]
pub enum View {
    Home,
    Agent(String),
    Settings(SettingsTab),
    Automations,
    Customize,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettingsTab {
    General,
    Agents,
    Accounts,
    Models,
    Sessions,
    Workspaces,
    Shortcuts,
    About,
}

impl SettingsTab {
    pub const ALL: [SettingsTab; 8] = [
        SettingsTab::General,
        SettingsTab::Agents,
        SettingsTab::Accounts,
        SettingsTab::Models,
        SettingsTab::Sessions,
        SettingsTab::Workspaces,
        SettingsTab::Shortcuts,
        SettingsTab::About,
    ];
    pub fn label(self) -> &'static str {
        match self {
            SettingsTab::General => "General",
            SettingsTab::Agents => "Agents",
            SettingsTab::Accounts => "Accounts",
            SettingsTab::Models => "Models & API Keys",
            SettingsTab::Sessions => "Sessions & Import",
            SettingsTab::Workspaces => "Workspaces",
            SettingsTab::Shortcuts => "Keyboard Shortcuts",
            SettingsTab::About => "About",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RightTab {
    /// Launcher tiles (Changes / Browser / Terminal / File).
    Home,
    Changes,
    Browser,
    Terminal,
    Files,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Modal {
    Rename(String),
    ConfirmDelete(String),
    Export(String),
    AddAccount(AgentKind),
    AddWorkspace,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Ui {
    pub sidebar_open: bool,
    pub right_open: bool,
    pub right_tab: RightTab,
    pub palette_open: bool,
    pub palette_query: String,
    pub sidebar_query: String,
    pub collapsed: BTreeSet<String>,
    pub expanded_sections: BTreeSet<String>,
    pub dark: bool,
    pub system_dark: bool,
    pub modal: Option<Modal>,
    pub browser_url: String,
    pub focus_composer: u64,
    /// File to reveal in the right pane (changes or files tab).
    pub reveal_file: Option<String>,
    pub maximized: bool,
    /// Session context menu at a window position.
    pub ctx: Option<CtxMenu>,
    /// Larrier's own right-click menu for everything else.
    pub gctx: Option<GlobalCtx>,
    /// Selected text quoted into the next prompt.
    pub quote: Option<String>,
    /// Back / forward navigation.
    pub history: Vec<View>,
    pub hist_pos: usize,
    /// Repositories expanded / collapsed by the user (overrides the default).
    pub repo_open: BTreeSet<String>,
    pub repo_closed: BTreeSet<String>,
    /// Open top-level menu in the menu bar.
    pub menubar: Option<&'static str>,
}

/// What was under the pointer when the user right-clicked.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct GlobalCtx {
    pub x: f64,
    pub y: f64,
    pub editable: bool,
    pub selection: String,
    pub link: Option<String>,
    pub code: Option<String>,
    pub path: Option<String>,
    pub image: Option<String>,
}

#[derive(Clone, PartialEq, Debug)]
pub struct CtxMenu {
    pub session_id: String,
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Toast {
    pub id: u64,
    pub text: String,
    pub error: bool,
}

/// Composer selections for the next new agent.
#[derive(Clone, PartialEq, Debug)]
pub struct Draft {
    pub agent: AgentKind,
    pub model: Option<String>,
    pub mode: Option<String>,
    pub account_id: Option<String>,
    pub workspace: Option<String>,
    pub worktree: bool,
}

#[derive(Clone, Copy)]
pub struct App {
    pub config: Signal<Config>,
    pub sessions: Signal<Vec<SessionSummary>>,
    pub open: Signal<HashMap<String, Session>>,
    pub view: Signal<View>,
    pub running: Signal<HashMap<String, TurnControl>>,
    pub permissions: Signal<HashMap<String, PermissionRequest>>,
    pub detected: Signal<Vec<Detected>>,
    pub ui: Signal<Ui>,
    pub toasts: Signal<Vec<Toast>>,
    pub sync_status: Signal<String>,
    pub draft: Signal<Draft>,
}

/// Signals owned by the root scope so background tasks can use them for the
/// app's whole lifetime.
fn root_signal<T: 'static>(v: T) -> Signal<T> {
    Signal::new_in_scope(v, ScopeId::ROOT)
}

pub fn use_app() -> App {
    use_context::<App>()
}

impl App {
    pub fn new() -> App {
        let cfg = store().load_config();
        let draft = Draft {
            agent: cfg.settings.default_agent.clone(),
            model: None,
            mode: None,
            account_id: None,
            workspace: cfg.workspaces.first().map(|w| w.path.clone()),
            worktree: cfg.settings.use_worktrees,
        };
        let ui = Ui {
            sidebar_open: true,
            right_open: cfg.settings.right_panel_open,
            right_tab: RightTab::Changes,
            palette_open: false,
            palette_query: String::new(),
            sidebar_query: String::new(),
            collapsed: BTreeSet::new(),
            expanded_sections: BTreeSet::new(),
            dark: cfg.settings.theme == Theme::Dark,
            system_dark: false,
            modal: None,
            browser_url: "http://localhost:3000".into(),
            focus_composer: 0,
            reveal_file: None,
            maximized: false,
            ctx: None,
            gctx: None,
            quote: None,
            history: vec![View::Home],
            hist_pos: 0,
            repo_open: BTreeSet::new(),
            repo_closed: BTreeSet::new(),
            menubar: None,
        };
        App {
            sessions: root_signal(store().list()),
            config: root_signal(cfg),
            open: root_signal(HashMap::new()),
            view: root_signal(View::Home),
            running: root_signal(HashMap::new()),
            permissions: root_signal(HashMap::new()),
            detected: root_signal(Vec::new()),
            ui: root_signal(ui),
            toasts: root_signal(Vec::new()),
            sync_status: root_signal(String::new()),
            draft: root_signal(draft),
        }
    }

    // ------------------------------------------------------------- basics

    pub fn toast(mut self, text: impl Into<String>) {
        self.push_toast(text.into(), false);
    }

    pub fn toast_err(mut self, text: impl Into<String>) {
        self.push_toast(text.into(), true);
    }

    fn push_toast(&mut self, text: String, error: bool) {
        let id = rand_id();
        self.toasts.write().push(Toast { id, text, error });
        let mut toasts = self.toasts;
        spawn_forever(async move {
            tokio::time::sleep(Duration::from_millis(if error { 6500 } else { 3200 })).await;
            toasts.write().retain(|t| t.id != id);
        });
    }

    pub fn refresh_sessions(mut self) {
        self.sessions.set(store().list());
    }

    pub fn save_config(self) {
        if let Err(e) = store().save_config(&self.config.read()) {
            self.toast_err(format!("Could not save settings: {e}"));
        }
    }

    pub fn update_config(mut self, f: impl FnOnce(&mut Config)) {
        f(&mut self.config.write());
        self.save_config();
    }

    pub fn apply_theme(mut self) {
        let theme = self.config.read().settings.theme;
        let sys = self.ui.read().system_dark;
        self.ui.write().dark = match theme {
            Theme::Dark => true,
            Theme::Light => false,
            Theme::System => sys,
        };
    }

    pub fn is_running(&self, id: &str) -> bool {
        self.running.read().contains_key(id)
    }

    pub fn current_id(&self) -> Option<String> {
        match &*self.view.read() {
            View::Agent(id) => Some(id.clone()),
            _ => None,
        }
    }

    pub fn current_session(&self) -> Option<Session> {
        let id = self.current_id()?;
        self.open.read().get(&id).cloned()
    }

    // ------------------------------------------------------------- navigation

    /// Switch views, recording the move for back / forward.
    pub fn navigate(mut self, v: View) {
        if *self.view.read() == v {
            return;
        }
        {
            let mut ui = self.ui.write();
            let pos = ui.hist_pos;
            ui.history.truncate(pos + 1);
            ui.history.push(v.clone());
            if ui.history.len() > 100 {
                ui.history.remove(0);
            }
            ui.hist_pos = ui.history.len() - 1;
        }
        self.view.set(v);
    }

    pub fn can_go(&self, delta: isize) -> bool {
        let ui = self.ui.read();
        let target = ui.hist_pos as isize + delta;
        target >= 0 && (target as usize) < ui.history.len()
    }

    pub fn go(mut self, delta: isize) {
        let target = {
            let ui = self.ui.read();
            let t = ui.hist_pos as isize + delta;
            if t < 0 || t as usize >= ui.history.len() {
                return;
            }
            (t as usize, ui.history[t as usize].clone())
        };
        if let View::Agent(id) = &target.1 {
            if !self.ensure_loaded(id) {
                return;
            }
        }
        self.ui.write().hist_pos = target.0;
        self.view.set(target.1);
    }

    pub fn new_agent(mut self) {
        self.navigate(View::Home);
        self.ui.write().focus_composer += 1;
    }

    pub fn open_settings(self, tab: SettingsTab) {
        self.navigate(View::Settings(tab));
    }

    /// Make sure a session is loaded into memory.
    pub fn ensure_loaded(mut self, id: &str) -> bool {
        if self.open.read().contains_key(id) {
            return true;
        }
        match store().load(id) {
            Ok(s) => {
                self.open.write().insert(id.to_string(), s);
                true
            }
            Err(e) => {
                self.toast_err(format!("Could not open session: {e}"));
                false
            }
        }
    }

    pub fn open_session(mut self, id: &str) {
        if !self.ensure_loaded(id) {
            return;
        }
        self.navigate(View::Agent(id.to_string()));
        let unread = self.open.read().get(id).map(|s| s.unread).unwrap_or(false);
        if unread {
            self.update_session(id, |s| s.unread = false);
        }
        self.ui.write().focus_composer += 1;
    }

    /// Mutate a session, persist it and refresh the sidebar.
    pub fn update_session(mut self, id: &str, f: impl FnOnce(&mut Session)) {
        if !self.ensure_loaded(id) {
            return;
        }
        let snapshot = {
            let mut open = self.open.write();
            let Some(s) = open.get_mut(id) else { return };
            f(s);
            s.clone()
        };
        if let Err(e) = store().save(&snapshot) {
            self.toast_err(format!("Could not save: {e}"));
        }
        self.refresh_sessions();
    }

    // ------------------------------------------------------------- running agents

    /// Send a prompt. `session_id == None` starts a new agent from the draft.
    pub fn send(mut self, session_id: Option<String>, text: String, attachments: Vec<Part>) {
        if text.trim().is_empty() && attachments.is_empty() {
            return;
        }
        let id = match session_id {
            Some(id) => {
                if self.is_running(&id) {
                    self.toast("Agent is still working — stop it first or wait.");
                    return;
                }
                if !self.ensure_loaded(&id) {
                    return;
                }
                id
            }
            None => match self.create_session_from_draft() {
                Ok(id) => id,
                Err(e) => {
                    self.toast_err(e.to_string());
                    return;
                }
            },
        };

        let quote = self.ui.write().quote.take();
        let text = match quote {
            Some(q) if !q.trim().is_empty() => {
                let quoted: String = q.trim().lines().map(|l| format!("> {l}\n")).collect();
                format!("{quoted}\n{text}")
            }
            _ => text,
        };
        let mut msg = Message::user_text(text);
        msg.parts.extend(attachments);
        let (agent, model, account) = {
            let mut open = self.open.write();
            let s = open.get_mut(&id).expect("loaded");
            s.push(msg);
            s.auto_title();
            s.archived = false;
            (s.agent.clone(), s.model.clone(), s.account_id.clone())
        };
        self.navigate(View::Agent(id.clone()));

        let cfg = self.config.read().clone();
        let request = {
            let open = self.open.read();
            larrier_agents::build_request(&cfg, open.get(&id).unwrap(), &agent)
        };
        let mut recorder = {
            let mut open = self.open.write();
            let s = open.get_mut(&id).unwrap();
            TurnRecorder::begin(s, agent.clone(), model, account)
        };
        match request {
            Ok(req) => {
                if let Some(h) = &req.handoff {
                    tracing::info!("handing {} chars of context to {}", h.len(), agent.display_name());
                }
                let handle = larrier_agents::start_turn(req);
                self.running.write().insert(id.clone(), handle.control.clone());
                self.persist(&id);
                self.drive(id, handle, recorder);
            }
            Err(e) => {
                let mut open = self.open.write();
                let s = open.get_mut(&id).unwrap();
                recorder.apply(s, &AgentEvent::Error { message: e.to_string() });
                recorder.finish(s, false);
                let snap = s.clone();
                drop(open);
                let _ = store().save(&snap);
                self.refresh_sessions();
            }
        }
    }

    fn persist(self, id: &str) {
        let snap = self.open.read().get(id).cloned();
        if let Some(s) = snap {
            let _ = store().save(&s);
        }
        self.refresh_sessions();
    }

    fn drive(self, id: String, mut handle: TurnHandle, mut rec: TurnRecorder) {
        let mut app = self;
        spawn_forever(async move {
            let mut last_save = Instant::now();
            let mut done = false;
            while let Some(first) = handle.events.recv().await {
                let mut batch = vec![first];
                while let Ok(e) = handle.events.try_recv() {
                    batch.push(e);
                }
                {
                    let mut open = app.open.write();
                    let Some(s) = open.get_mut(&id) else { break };
                    for e in &batch {
                        match e {
                            AgentEvent::Permission { request } => {
                                app.permissions.write().insert(id.clone(), request.clone());
                            }
                            AgentEvent::NativeSession { id: native, .. } => {
                                store().set_import_record(&import_key(&rec.agent, native), ImportRecord::linked(&id));
                                store().flush_imports();
                            }
                            AgentEvent::Log { line } => tracing::debug!("[{}] {line}", rec.agent.id()),
                            AgentEvent::Done { .. } => done = true,
                            _ => {}
                        }
                        rec.apply(s, e);
                    }
                    if done || last_save.elapsed() > Duration::from_secs(2) {
                        let _ = store().save(s);
                        last_save = Instant::now();
                    }
                }
                if done {
                    break;
                }
                app.sessions.set(store().list());
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
            let is_current = app.current_id().as_deref() == Some(id.as_str());
            let title = {
                let mut open = app.open.write();
                if let Some(s) = open.get_mut(&id) {
                    if !rec.finished {
                        rec.finish(s, false);
                    }
                    if is_current {
                        s.unread = false;
                    }
                    let _ = store().save(s);
                    Some((s.title.clone(), s.status))
                } else {
                    None
                }
            };
            app.running.write().remove(&id);
            app.permissions.write().remove(&id);
            app.sessions.set(store().list());
            if let Some((title, status)) = title {
                if app.current_id().as_deref() != Some(id.as_str()) {
                    match status {
                        SessionStatus::Error => app.toast_err(format!("“{title}” failed")),
                        _ => app.toast(format!("“{title}” is ready for review")),
                    }
                }
            }
        });
    }

    pub fn stop(self, id: &str) {
        if let Some(c) = self.running.read().get(id) {
            c.cancel();
        }
    }

    pub fn respond_permission(mut self, session_id: &str, option_id: Option<String>) {
        let req = self.permissions.write().remove(session_id);
        if let (Some(req), Some(ctrl)) = (req, self.running.read().get(session_id)) {
            ctrl.respond(req.id, option_id);
        }
        if let Some(s) = self.open.write().get_mut(session_id) {
            s.status = SessionStatus::Running;
        }
    }

    fn create_session_from_draft(mut self) -> anyhow::Result<String> {
        let d = self.draft.read().clone();
        let ws = d.workspace.clone().map(|p| {
            let mut w = WorkspaceRef::from_path(p.clone());
            let path = PathBuf::from(&p);
            if git::is_repo(&path) {
                w.branch = git::current_branch(&path);
                w.remote = git::remote_url(&path);
            }
            w
        });
        let mut s = Session::new(d.agent.clone(), ws.clone());
        s.model = d.model.clone();
        s.mode = d.mode.clone();
        s.account_id = d.account_id.clone();
        if d.worktree {
            if let Some(w) = &ws {
                let repo = PathBuf::from(&w.path);
                if git::is_repo(&repo) {
                    let short = &s.id[s.id.len() - 6..];
                    let branch = format!("larrier/agent-{short}");
                    let base_dir = self
                        .config
                        .read()
                        .settings
                        .worktrees_dir
                        .clone()
                        .map(PathBuf::from)
                        .unwrap_or_else(|| store().worktrees_dir());
                    let dir = base_dir.join(&w.name).join(format!("agent-{short}"));
                    git::create_worktree(&repo, &dir, &branch)?;
                    s.environment = Environment::Worktree {
                        path: dir.to_string_lossy().to_string(),
                        branch,
                        base: w.branch.clone(),
                    };
                }
            }
        }
        if let Some(p) = &d.workspace {
            let exists = self.config.read().workspace_by_path(p).is_some();
            if !exists {
                self.update_config(|c| {
                    c.add_workspace(p);
                });
            }
        }
        let id = s.id.clone();
        store().save(&s)?;
        self.open.write().insert(id.clone(), s);
        Ok(id)
    }

    // ------------------------------------------------------------- session ops

    pub fn rename(self, id: &str, title: String) {
        let t = title.trim().to_string();
        if !t.is_empty() {
            self.update_session(id, |s| s.title = t);
        }
    }

    pub fn toggle_pin(self, id: &str) {
        self.update_session(id, |s| s.pinned = !s.pinned);
    }

    pub fn set_unread(self, id: &str, unread: bool) {
        self.update_session(id, |s| s.unread = unread);
    }

    pub fn toggle_archive(mut self, id: &str) {
        let mut archived = false;
        self.update_session(id, |s| {
            s.archived = !s.archived;
            archived = s.archived;
        });
        if archived && self.current_id().as_deref() == Some(id) {
            self.view.set(View::Home);
        }
    }

    pub fn delete(mut self, id: &str) {
        self.stop(id);
        // Clean up a worktree we created.
        if let Some(s) = self.open.read().get(id).cloned().or_else(|| store().load(id).ok()) {
            if let (Environment::Worktree { path, .. }, Some(ws)) = (&s.environment, &s.workspace) {
                let _ = git::remove_worktree(&PathBuf::from(&ws.path), &PathBuf::from(path));
            }
        }
        if let Err(e) = store().delete(id) {
            self.toast_err(format!("Delete failed: {e}"));
        }
        self.open.write().remove(id);
        if self.current_id().as_deref() == Some(id) {
            self.view.set(View::Home);
        }
        self.refresh_sessions();
    }

    pub fn fork(mut self, id: &str, message_id: Option<String>) {
        if !self.ensure_loaded(id) {
            return;
        }
        let forked = self.open.read().get(id).map(|s| s.fork(message_id.as_deref()));
        if let Some(f) = forked {
            let fid = f.id.clone();
            let _ = store().save(&f);
            self.open.write().insert(fid.clone(), f);
            self.refresh_sessions();
            self.open_session(&fid);
            self.toast("Forked into a new agent");
        }
    }

    /// Hand the session to another agent. Agents that can load native
    /// sessions get a real resumable session file; others receive the
    /// transcript as context on the next turn.
    pub fn port(self, id: &str, target: AgentKind) {
        if !self.ensure_loaded(id) {
            return;
        }
        let cfg = self.config.read().clone();
        let mut note = String::new();
        let target2 = target.clone();
        self.update_session(id, |s| {
            if s.agent == target2 {
                return;
            }
            let from = s.agent.display_name();
            if larrier_formats::supports_native_export(&target2) {
                let home = account_home(&cfg, &target2, s.account_id.as_deref());
                match larrier_formats::export_native(s, &target2, home.as_deref()) {
                    Ok(link) => {
                        store().set_import_record(&import_key(&target2, &link.id), ImportRecord::linked(&s.id));
                        store().flush_imports();
                        s.set_native_link(link);
                        note = format!("Ported from {from} to {} (native session)", target2.display_name());
                    }
                    Err(e) => {
                        tracing::warn!("native export failed: {e}");
                        note = format!("Ported from {from} to {} (context handoff)", target2.display_name());
                    }
                }
            } else {
                note = format!("Ported from {from} to {} (context handoff)", target2.display_name());
            }
            s.agent = target2.clone();
            s.model = None;
            s.mode = None;
            s.account_id = None;
            s.push(Message::new(Role::System, vec![Part::Notice { text: note.clone() }]));
        });
        if !note.is_empty() {
            self.toast(note);
        }
    }

    pub fn set_session_model(self, id: &str, model: Option<String>) {
        self.update_session(id, |s| s.model = model);
    }

    pub fn set_session_mode(self, id: &str, mode: Option<String>) {
        self.update_session(id, |s| s.mode = mode);
    }

    pub fn set_session_account(self, id: &str, account: Option<String>) {
        self.update_session(id, |s| s.account_id = account);
    }

    pub fn export(self, id: &str, format: ExportFormat) {
        if !self.ensure_loaded(id) {
            return;
        }
        let Some(s) = self.open.read().get(id).cloned() else { return };
        let text = match larrier_formats::export_string(&s, format) {
            Ok(t) => t,
            Err(e) => return self.toast_err(e.to_string()),
        };
        let name = format!("{}.{}", sanitize_filename(&s.title), format.extension());
        spawn_forever(async move {
            let file = rfd::AsyncFileDialog::new().set_file_name(&name).save_file().await;
            if let Some(f) = file {
                match std::fs::write(f.path(), text) {
                    Ok(()) => self.toast(format!("Exported to {}", f.path().display())),
                    Err(e) => self.toast_err(format!("Export failed: {e}")),
                }
            }
        });
    }

    /// Open the session in the agent's own CLI in a terminal window.
    pub fn open_in_terminal(self, id: &str) {
        let Some(s) = self.open.read().get(id).cloned() else { return };
        let cwd = s.cwd().unwrap_or_else(|| ".".into());
        let link = s.native_link(&s.agent).cloned();
        let cmd = match (&s.agent, link) {
            (AgentKind::ClaudeCode, Some(l)) => format!("claude --resume {}", l.id),
            (AgentKind::Codex, Some(l)) => format!("codex resume {}", l.id),
            (AgentKind::Cursor, Some(l)) => format!("cursor-agent --resume {}", l.id),
            (AgentKind::Gemini, Some(_)) => "gemini --resume latest".to_string(),
            (AgentKind::OpenCode, Some(l)) => format!("opencode --session {}", l.id),
            (AgentKind::Pi, Some(l)) => format!("pi --session {}", l.path.unwrap_or(l.id)),
            (a, _) => a.spec().binaries.first().map(|b| b.to_string()).unwrap_or_default(),
        };
        if cmd.is_empty() {
            return self.toast("This agent has no CLI to open.");
        }
        match open_terminal(&cwd, &cmd) {
            Ok(()) => self.toast(format!("Opened: {cmd}")),
            Err(e) => self.toast_err(format!("Could not open a terminal: {e}")),
        }
    }

    // ------------------------------------------------------------- import / detect

    pub fn detect_agents(mut self) {
        let cfg = self.config.read().clone();
        spawn_forever(async move {
            let found = larrier_agents::detect_all(&cfg).await;
            self.detected.set(found);
        });
    }

    pub fn sync_imports(mut self, quiet: bool) {
        let agents: Vec<AgentKind> = {
            let cfg = self.config.read();
            larrier_formats::importable_agents().into_iter().filter(|a| cfg.agent(a).import).collect()
        };
        self.sync_status.set("Importing sessions…".into());
        spawn_forever(async move {
            let report = tokio::task::spawn_blocking(move || larrier_formats::sync(store(), &agents)).await;
            match report {
                Ok(r) => {
                    let msg = if r.imported + r.updated == 0 {
                        "Sessions up to date".to_string()
                    } else {
                        format!("Imported {} · updated {}", r.imported, r.updated)
                    };
                    self.sync_status.set(msg.clone());
                    if !quiet || r.imported + r.updated > 0 {
                        self.toast(msg);
                    }
                    for e in r.errors.iter().take(3) {
                        tracing::warn!("import: {e}");
                    }
                }
                Err(e) => self.sync_status.set(format!("Import failed: {e}")),
            }
            self.sessions.set(store().list());
        });
    }

    pub fn import_files(self) {
        spawn_forever(async move {
            let files = rfd::AsyncFileDialog::new()
                .add_filter("Sessions & chat exports", &["jsonl", "json", "md", "zip"])
                .pick_files()
                .await;
            let Some(files) = files else { return };
            let mut count = 0;
            for f in files {
                match larrier_formats::import_file(f.path()) {
                    Ok(list) => {
                        count += list.len();
                        let _ = store().save_many(&list);
                    }
                    Err(e) => self.toast_err(format!("{}: {e}", f.file_name())),
                }
            }
            if count > 0 {
                self.toast(format!("Imported {count} session{}", if count == 1 { "" } else { "s" }));
            }
            self.refresh_sessions();
        });
    }

    pub fn pick_workspace(mut self, for_draft: bool) {
        spawn_forever(async move {
            if let Some(dir) = rfd::AsyncFileDialog::new().pick_folder().await {
                let p = dir.path().to_string_lossy().to_string();
                self.update_config(|c| {
                    c.add_workspace(&p);
                });
                if for_draft {
                    self.draft.write().workspace = Some(p);
                }
            }
        });
    }

    pub fn attach_files(self, mut target: Signal<Vec<Part>>) {
        spawn_forever(async move {
            if let Some(files) = rfd::AsyncFileDialog::new().pick_files().await {
                for f in files {
                    let path = f.path().to_string_lossy().to_string();
                    let lower = path.to_lowercase();
                    let is_img = [".png", ".jpg", ".jpeg", ".gif", ".webp"].iter().any(|e| lower.ends_with(e));
                    let part = if is_img {
                        Part::Image { mime: None, source: MediaSource::Path { path } }
                    } else {
                        Part::File { path, mime: None, text: None }
                    };
                    target.write().push(part);
                }
            }
        });
    }
}

impl App {
    /// Edit a past prompt: continue in a fork that ends just before it, so
    /// the original conversation stays intact.
    pub fn edit_and_resend(mut self, session_id: &str, message_id: &str, text: String) {
        if text.trim().is_empty() || !self.ensure_loaded(session_id) {
            return;
        }
        let forked = {
            let open = self.open.read();
            let Some(s) = open.get(session_id) else { return };
            let Some(idx) = s.messages.iter().position(|m| m.id == message_id) else { return };
            let mut f = s.fork(if idx > 0 { Some(s.messages[idx - 1].id.as_str()) } else { None });
            if idx == 0 {
                f.messages.clear();
            }
            f.title = s.title.clone();
            f
        };
        let fid = forked.id.clone();
        let _ = store().save(&forked);
        self.open.write().insert(fid.clone(), forked);
        self.refresh_sessions();
        self.send(Some(fid), text, Vec::new());
    }

    /// Thumbs up / down on an assistant message (kept in the session extras).
    pub fn set_feedback(self, session_id: &str, message_id: &str, value: Option<&str>) {
        let key = format!("feedback:{message_id}");
        let v = value.map(|s| s.to_string());
        self.update_session(session_id, move |s| match v {
            Some(v) => {
                s.extra.insert(key, serde_json::Value::String(v));
            }
            None => {
                s.extra.remove(&key);
            }
        });
    }

    /// Open the session's folder in an editor (Cursor, then VS Code, else the file manager).
    pub fn open_in_ide(self, id: &str) {
        let Some(dir) = self.open.read().get(id).and_then(|s| s.cwd()) else {
            return self.toast("This agent has no folder");
        };
        for editor in ["cursor", "code"] {
            if let Some(bin) = larrier_agents::detect::which_program(editor) {
                let (prog, prefix) = larrier_agents::request::unwrap_shim(&bin);
                let mut c = std::process::Command::new(prog);
                c.args(prefix).arg(&dir);
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    c.creation_flags(0x0800_0000);
                }
                if c.spawn().is_ok() {
                    return;
                }
            }
        }
        let _ = open::that(&dir);
    }

    /// Copy text to the system clipboard.
    pub fn copy_text(self, text: &str) {
        match arboard::Clipboard::new().and_then(|mut c| c.set_text(text.to_string())) {
            Ok(()) => {}
            Err(e) => self.toast_err(format!("Copy failed: {e}")),
        }
    }
}

/// Display name for the profile row: git's user.name, else the OS user.
pub fn user_name() -> String {
    static NAME: OnceLock<Option<String>> = OnceLock::new();
    let git = NAME.get_or_init(|| {
        let mut c = std::process::Command::new("git");
        c.args(["config", "--global", "user.name"]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000);
        }
        c.output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|s| !s.is_empty())
    });
    if let Some(n) = git {
        return n.clone();
    }
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .map(|u| {
            let mut c = u.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or(u)
        })
        .unwrap_or_else(|_| "You".into())
}

/// Config dir for accounts that use a separate login directory.
pub fn account_home(cfg: &Config, agent: &AgentKind, account_id: Option<&str>) -> Option<PathBuf> {
    let acc = account_id.and_then(|id| cfg.account(id)).or_else(|| cfg.default_account(agent))?;
    match &acc.auth {
        AccountAuth::ConfigDir { path } => Some(PathBuf::from(path)),
        _ => None,
    }
}

pub fn open_terminal(cwd: &str, cmd: &str) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Prefer Windows Terminal, fall back to a classic console.
        let wt = std::process::Command::new("wt.exe").args(["-d", cwd, "cmd", "/K", cmd]).spawn();
        if wt.is_err() {
            std::process::Command::new("cmd")
                .args(["/C", "start", "", "cmd", "/K", cmd])
                .current_dir(cwd)
                .creation_flags(0x0800_0000)
                .spawn()?;
        }
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        let script = format!("tell application \"Terminal\" to do script \"cd '{cwd}' && {cmd}\"");
        std::process::Command::new("osascript").args(["-e", &script]).spawn()?;
        Ok(())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let line = format!("cd '{cwd}' && {cmd}; exec $SHELL");
        for term in ["x-terminal-emulator", "gnome-terminal", "konsole", "xterm"] {
            if std::process::Command::new(term).args(["-e", "sh", "-c", &line]).spawn().is_ok() {
                return Ok(());
            }
        }
        anyhow::bail!("no terminal emulator found")
    }
}

pub fn sanitize_filename(s: &str) -> String {
    let out: String = s.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' { c } else { '_' }).collect();
    let out = out.trim().to_string();
    if out.is_empty() {
        "session".into()
    } else {
        out.chars().take(80).collect()
    }
}

pub fn rand_id() -> u64 {
    let id = new_id();
    let mut h: u64 = 1469598103934665603;
    for b in id.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    h
}

/// "now", "5m", "3h", "2d", "Mar 4".
pub fn rel_time(t: chrono::DateTime<chrono::Utc>) -> String {
    let d = chrono::Utc::now().signed_duration_since(t);
    let s = d.num_seconds();
    if s < 60 {
        "now".into()
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86_400 {
        format!("{}h", s / 3600)
    } else if s < 86_400 * 7 {
        format!("{}d", s / 86_400)
    } else if s < 86_400 * 300 {
        t.format("%b %-d").to_string()
    } else {
        t.format("%b %Y").to_string()
    }
}
