//! Application state (signals) and every action the UI can take.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use dioxus::prelude::*;
use dioxus::core::spawn_forever;
use larrier_agents::{
    engine::TurnRecorder, events::PermissionRequest, git, AgentEvent, Detected, Reply, TurnControl, TurnHandle,
};
use larrier_core::{
    lsf::{new_id, MediaSource},
    store::{import_key, ImportRecord},
    AccountAuth, AgentKind, BusySend, Config, Environment, Message, Part, Role, Session, SessionStatus, SessionSummary, Store,
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
    /// A subagent's transcript (a Task tool call inside a session).
    Subagent { session: String, tool: String },
    Settings(SettingsTab),
    Automations,
    Customize,
}

impl View {
    /// Views that live in the chat tab strip.
    pub fn is_tab(&self) -> bool {
        matches!(self, View::Agent(_) | View::Subagent { .. })
    }

    /// The session a tab belongs to.
    pub fn session_id(&self) -> Option<&str> {
        match self {
            View::Agent(id) => Some(id),
            View::Subagent { session, .. } => Some(session),
            _ => None,
        }
    }
}

/// A subagent chat open next to its session. A window shows one session:
/// its own tab always comes first and is never replaced; its subagents open
/// beside it, VS Code style: a single click opens a preview tab (italic)
/// that the next one replaces, and double-click keeps it. Another session
/// never joins as a tab; it opens side by side instead.
#[derive(Clone, PartialEq, Debug)]
pub struct ChatTab {
    pub view: View,
    pub preview: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettingsTab {
    General,
    Appearance,
    Usage,
    Agents,
    Accounts,
    Models,
    SourceControl,
    Sessions,
    Workspaces,
    Shortcuts,
    About,
}

impl SettingsTab {
    pub const ALL: [SettingsTab; 11] = [
        SettingsTab::General,
        SettingsTab::Appearance,
        SettingsTab::Usage,
        SettingsTab::Agents,
        SettingsTab::Accounts,
        SettingsTab::Models,
        SettingsTab::SourceControl,
        SettingsTab::Sessions,
        SettingsTab::Workspaces,
        SettingsTab::Shortcuts,
        SettingsTab::About,
    ];
    pub fn label(self) -> &'static str {
        match self {
            SettingsTab::General => "General",
            SettingsTab::Appearance => "Appearance",
            SettingsTab::Usage => "Plan & Usage",
            SettingsTab::SourceControl => "Source Control",
            SettingsTab::Agents => "Agents",
            SettingsTab::Accounts => "Accounts",
            SettingsTab::Models => "Models & API Keys",
            SettingsTab::Sessions => "Sessions & Import",
            SettingsTab::Workspaces => "Projects",
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
    /// Background shells, monitors and subagents of the session.
    Tasks,
}

/// A tool tab in the side panel. Like chat tabs, a newly opened tool is a
/// preview (italic) that the next one replaces until it is double-clicked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PanelTab {
    pub tab: RightTab,
    pub preview: bool,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Modal {
    Rename(String),
    ConfirmDelete(String),
    Export(String),
    AddAccount(AgentKind),
    AddWorkspace,
    /// A release's notes; `true` when Larrier was just updated to it.
    WhatsNew(String, bool),
}

#[derive(Clone, PartialEq, Debug)]
pub struct Ui {
    pub sidebar_open: bool,
    pub right_open: bool,
    /// The side panel's active tab (`Home` when none is open).
    pub right_tab: RightTab,
    pub right_tabs: Vec<PanelTab>,
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
    /// Projects expanded / collapsed by the user (overrides the default).
    pub repo_open: BTreeSet<String>,
    pub repo_closed: BTreeSet<String>,
    /// Open top-level menu in the menu bar.
    pub menubar: Option<&'static str>,
    /// Window width in CSS px (for the narrow layout).
    pub win_w: f64,
    /// Image shown full-size.
    pub lightbox: Option<String>,
    /// Attachments produced outside the composer (paste, drop, menus) that
    /// the visible composer picks up.
    pub attach_inbox: Vec<Part>,
    /// What Customize opens on (from `/mcp`): a kind of item, an agent.
    pub customize_focus: Option<(Option<larrier_agents::harness::ItemKind>, Option<AgentKind>)>,
    /// Agents that were interrupted when Larrier last closed.
    pub interrupted: Vec<String>,
    /// Text to insert into a composer: (session, text); `None` is the new
    /// agent composer.
    pub compose_inbox: Vec<(Option<String>, String)>,
    /// Objects for a composer's text, with a picture to attach alongside:
    /// (session, object, picture); `None` is the new agent composer.
    pub chip_inbox: Vec<(Option<String>, crate::ui::mentions::MentionChip, Option<Part>)>,
    /// Messages taken back from the agent, for their chat's composer to
    /// hold again (text, objects, images and files).
    pub restore_inbox: Vec<(String, Queued)>,
    /// Messages sent into a turn that are being taken back (their keys).
    pub unsteering: HashSet<u64>,
    /// Right-click menu of a chat tab.
    pub tab_ctx: Option<TabCtx>,
    /// An agent's terminal shown read-only in the Terminal tab:
    /// (session id, shell tool call id).
    pub agent_term: Option<(String, String)>,
    /// Agent whose models Settings → Models shows.
    pub models_agent: Option<AgentKind>,
}

impl Ui {
    /// Show a side-panel tool: an open tab is shown, otherwise the preview
    /// tab is replaced (or a tab added). `keep` opens it as a regular tab.
    pub fn open_panel(&mut self, tab: RightTab, keep: bool) {
        self.right_open = true;
        if tab == RightTab::Home {
            self.right_tab = RightTab::Home;
            return;
        }
        if let Some(t) = self.right_tabs.iter_mut().find(|t| t.tab == tab) {
            t.preview &= !keep;
        } else if let Some(t) = self.right_tabs.iter_mut().find(|t| t.preview).filter(|_| !keep) {
            t.tab = tab;
        } else {
            self.right_tabs.push(PanelTab { tab, preview: !keep });
        }
        self.right_tab = tab;
    }

    /// Turn a preview tab into a regular one.
    pub fn keep_panel(&mut self, tab: RightTab) {
        if let Some(t) = self.right_tabs.iter_mut().find(|t| t.tab == tab) {
            t.preview = false;
        }
    }

    pub fn close_panel(&mut self, tab: RightTab) {
        let Some(i) = self.right_tabs.iter().position(|t| t.tab == tab) else { return };
        self.right_tabs.remove(i);
        if self.right_tab == tab {
            self.right_tab = self.right_tabs.get(i).or_else(|| i.checked_sub(1).and_then(|j| self.right_tabs.get(j))).map(|t| t.tab).unwrap_or(RightTab::Home);
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct TabCtx {
    pub view: View,
    pub x: f64,
    pub y: f64,
}

/// Editing a past prompt. Only one prompt is edited at a time; switching
/// away from changed text asks to keep editing or discard first.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct EditState {
    pub msg: Option<String>,
    pub original: String,
    pub draft: String,
    /// The objects (@file, /skill, @chat...) the draft's text holds.
    pub mentions: Vec<crate::ui::mentions::MentionChip>,
    /// The prompt's images and files, as they will be resent.
    pub attachments: Vec<Part>,
    pub original_attachments: Vec<Part>,
    /// A switch waiting for "Discard": the prompt to edit next, or `None`
    /// to just stop editing.
    pub confirm: Option<Option<EditTarget>>,
}

/// A prompt to edit: its id, text and attachments.
pub type EditTarget = (String, String, Vec<Part>);

impl EditState {
    pub fn dirty(&self) -> bool {
        self.msg.is_some() && (self.draft.trim() != self.original.trim() || self.attachments != self.original_attachments)
    }
}

/// Where updating Larrier stands.
#[derive(Clone, PartialEq, Debug, Default)]
pub enum UpdateState {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(crate::update::Release),
    Downloading { version: String, got: u64, total: u64 },
    /// Downloaded and checked: restarting puts it in place.
    Ready { version: String, path: PathBuf },
    Failed(String),
}

/// A follow-up waiting for the agent to finish its current turn.
#[derive(Clone, PartialEq, Debug)]
pub struct Queued {
    pub id: u64,
    pub text: String,
    pub attachments: Vec<Part>,
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
    /// A misspelled word right-clicked in a text box.
    pub spell: Option<Misspelling>,
}

/// A misspelled word in a text box: where it is and what it might be.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Misspelling {
    pub word: String,
    pub start: usize,
    pub end: usize,
    pub suggestions: Vec<String>,
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
    /// Option of the agent's first mode group (see `AgentSpec::mode_groups`).
    pub mode: Option<String>,
    /// Option of its second group (permissions / autonomy).
    pub permission: Option<String>,
    /// Reasoning effort; `None` follows the agent's default.
    pub effort: Option<String>,
    pub account_id: Option<String>,
    /// The agent's own named agent the new chat runs as (Claude Code's
    /// `--agent`).
    pub run_as: Option<String>,
    pub workspace: Option<String>,
    pub worktree: bool,
}

#[derive(Clone, Copy)]
pub struct App {
    pub config: Signal<Config>,
    pub sessions: Signal<Vec<SessionSummary>>,
    /// Loaded sessions, one signal each so a streaming agent only re-renders
    /// the views showing that session.
    pub open: Signal<HashMap<String, Signal<Session>>>,
    pub view: Signal<View>,
    pub running: Signal<HashMap<String, TurnControl>>,
    /// What each running agent is waiting for, oldest first.
    pub permissions: Signal<HashMap<String, Vec<PermissionRequest>>>,
    pub detected: Signal<Vec<Detected>>,
    pub ui: Signal<Ui>,
    pub toasts: Signal<Vec<Toast>>,
    pub sync_status: Signal<String>,
    pub draft: Signal<Draft>,
    /// Follow-ups queued per session while its agent is working.
    pub queue: Signal<HashMap<String, Vec<Queued>>>,
    /// Messages sent into a running turn that the agent hasn't taken in yet.
    pub steering: Signal<HashMap<String, Vec<Queued>>>,
    /// Sessions the user stopped: their queue waits instead of sending,
    /// unless the stop was to send a message now.
    pub stopping: Signal<HashMap<String, bool>>,
    /// Chat tabs in the header.
    pub tabs: Signal<Vec<ChatTab>>,
    /// Session shown side by side with the current one.
    pub split: Signal<Option<String>>,
    /// The prompt being edited in place (one at a time).
    pub editing: Signal<EditState>,
    /// Session being dragged (sidebar row or tab), for the drop targets.
    pub drag: Signal<Option<String>>,
    /// Sessions another app (a terminal CLI, an IDE) is writing right now:
    /// id → when Larrier first saw it live. See `crate::live`.
    pub live: Signal<HashMap<String, Instant>>,
    /// Running sessions whose agent finished its reply while background work
    /// (subagents, commands) keeps the turn open: not "generating".
    pub idle: Signal<HashSet<String>>,
    pub update: Signal<UpdateState>,
    /// Each agent's own models and defaults, by agent id (see
    /// `crate::agent_models`).
    pub native_models: Signal<HashMap<String, larrier_agents::models::AgentModels>>,
    /// Agents whose models are being re-read.
    pub models_refreshing: Signal<HashSet<String>>,
}

/// Signals owned by the root scope so background tasks can use them for the
/// app's whole lifetime.
fn root_signal<T: 'static>(v: T) -> Signal<T> {
    Signal::new_in_scope(v, ScopeId::ROOT)
}

pub fn use_app() -> App {
    use_context::<App>()
}

/// Subscribe to one piece of the UI state: the component re-renders only
/// when that piece changes, not on every UI update.
pub fn use_ui<T: PartialEq + 'static>(f: impl Fn(&Ui) -> T + 'static) -> Memo<T> {
    let app = use_app();
    use_memo(move || f(&app.ui.read()))
}

impl App {
    pub fn new() -> App {
        let cfg = store().load_config();
        let draft = Draft {
            agent: cfg.settings.default_agent.clone(),
            model: None,
            mode: None,
            permission: None,
            effort: None,
            account_id: None,
            run_as: None,
            workspace: cfg.workspaces.first().map(|w| w.path.clone()),
            worktree: cfg.settings.use_worktrees,
        };
        let ui = Ui {
            sidebar_open: true,
            right_open: cfg.settings.right_panel_open,
            right_tab: RightTab::Changes,
            right_tabs: vec![PanelTab { tab: RightTab::Changes, preview: false }],
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
            repo_open: cfg.settings.sidebar_opened.iter().cloned().collect(),
            repo_closed: cfg.settings.sidebar_closed.iter().cloned().collect(),
            menubar: None,
            win_w: 1400.0,
            lightbox: None,
            attach_inbox: Vec::new(),
            customize_focus: None,
            interrupted: Vec::new(),
            compose_inbox: Vec::new(),
            chip_inbox: Vec::new(),
            restore_inbox: Vec::new(),
            unsteering: HashSet::new(),
            tab_ctx: None,
            agent_term: None,
            models_agent: None,
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
            queue: root_signal(HashMap::new()),
            steering: root_signal(HashMap::new()),
            stopping: root_signal(HashMap::new()),
            tabs: root_signal(Vec::new()),
            split: root_signal(None),
            editing: root_signal(EditState::default()),
            drag: root_signal(None),
            live: root_signal(HashMap::new()),
            idle: root_signal(HashSet::new()),
            update: root_signal(UpdateState::default()),
            native_models: root_signal(crate::agent_models::load()),
            models_refreshing: root_signal(HashSet::new()),
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

    /// Open or close a sidebar group, remembered across launches.
    pub fn set_group_open(mut self, key: &str, open: bool) {
        {
            let mut ui = self.ui.write();
            if open {
                ui.repo_closed.remove(key);
                ui.repo_open.insert(key.to_string());
            } else {
                ui.repo_open.remove(key);
                ui.repo_closed.insert(key.to_string());
            }
        }
        let (opened, closed) = {
            let ui = self.ui.peek();
            (ui.repo_open.iter().cloned().collect(), ui.repo_closed.iter().cloned().collect())
        };
        self.update_config(|c| {
            c.settings.sidebar_opened = opened;
            c.settings.sidebar_closed = closed;
        });
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

    /// Mid-turn in the agent's own app (a terminal CLI, Claude's or Cursor's
    /// app), not run by Larrier: Larrier can't reach that process, and
    /// running the agent here at the same time would fork the conversation.
    pub fn running_elsewhere(&self, id: &str) -> bool {
        !self.is_running(id) && self.is_active(id)
    }

    /// Whether an agent is working on this session right now — run by
    /// Larrier, or mid-turn in its own app (a terminal CLI, an IDE) as seen
    /// through live sync. Everything that shows "running" uses this.
    pub fn is_active(&self, id: &str) -> bool {
        if self.is_running(id) {
            return true;
        }
        let live = self.live.read().contains_key(id);
        self.sessions.read().iter().find(|s| s.id == id).map(|s| external_active(s, live)).unwrap_or(false)
    }

    /// Ids of every session an agent is working on right now.
    pub fn active_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.running.read().keys().cloned().collect();
        let live = self.live.read();
        for s in self.sessions.read().iter() {
            if !ids.contains(&s.id) && external_active(s, live.contains_key(&s.id)) {
                ids.push(s.id.clone());
            }
        }
        ids
    }

    pub fn current_id(&self) -> Option<String> {
        match &*self.view.read() {
            View::Agent(id) => Some(id.clone()),
            _ => None,
        }
    }

    pub fn current_session(&self) -> Option<Session> {
        let id = self.current_id()?;
        self.peek_session(&id, |s| s.clone())
    }

    /// The signal of a loaded session. Reading it in a component subscribes
    /// that component to this session only.
    pub fn session_sig(&self, id: &str) -> Option<Signal<Session>> {
        self.open.read().get(id).copied()
    }

    /// Read a loaded session, subscribing the calling component to it.
    pub fn with_session<R>(&self, id: &str, f: impl FnOnce(&Session) -> R) -> Option<R> {
        let sig = self.open.read().get(id).copied()?;
        let s = sig.read();
        Some(f(&s))
    }

    /// Read a loaded session without subscribing (event handlers, tasks).
    pub fn peek_session<R>(&self, id: &str, f: impl FnOnce(&Session) -> R) -> Option<R> {
        let sig = self.open.peek().get(id).copied()?;
        let s = sig.peek();
        Some(f(&s))
    }

    /// Change a loaded session in memory (no save).
    pub fn mutate_session<R>(&self, id: &str, f: impl FnOnce(&mut Session) -> R) -> Option<R> {
        let mut sig = self.open.peek().get(id).copied()?;
        let mut s = sig.write();
        Some(f(&mut s))
    }

    pub fn is_loaded(&self, id: &str) -> bool {
        self.open.peek().contains_key(id)
    }

    /// Add or replace a loaded session.
    fn put_session(mut self, s: Session) {
        let existing = self.open.peek().get(&s.id).copied();
        match existing {
            Some(mut sig) => sig.set(s),
            None => {
                let id = s.id.clone();
                let sig = root_signal(s);
                self.open.write().insert(id, sig);
            }
        }
    }

    /// Persist a session in the background and update its sidebar row now.
    pub fn save_session(mut self, s: &Session) {
        let sum = s.summary();
        {
            let mut list = self.sessions.write();
            if let Some(i) = list.iter().position(|x| x.id == sum.id) {
                list.remove(i);
            }
            let at = list.partition_point(|x| x.updated_at > sum.updated_at);
            list.insert(at, sum);
        }
        save_async(s.clone());
    }

    /// A session changed outside Larrier (live sync, already saved): show the
    /// new contents wherever it is open and update its sidebar row.
    pub fn apply_external(mut self, s: Session) {
        let sum = s.summary();
        {
            let mut list = self.sessions.write();
            if let Some(i) = list.iter().position(|x| x.id == sum.id) {
                list.remove(i);
            }
            let at = list.partition_point(|x| x.updated_at > sum.updated_at);
            list.insert(at, sum);
        }
        if self.is_loaded(&s.id) {
            self.put_session(s);
        }
    }

    /// Save a loaded session by id.
    fn save_loaded(self, id: &str) {
        if let Some(snap) = self.peek_session(id, |s| s.clone()) {
            self.save_session(&snap);
        }
    }

    // ------------------------------------------------------------- navigation

    /// Switch views, recording the move for back / forward.
    pub fn navigate(mut self, v: View) {
        if v.is_tab() {
            self.ensure_tab(&v, false);
        }
        if *self.view.peek() == v {
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
        // Sessions deleted since (here or by their agent) drop out of the
        // history instead of blocking the way back.
        let gone: Vec<String> = {
            let ui = self.ui.peek();
            ui.history.iter().filter_map(|v| v.session_id()).filter(|id| !self.is_loaded(id) && !store().exists(id)).map(String::from).collect()
        };
        for id in &gone {
            self.forget_session(id);
        }
        let target = {
            let ui = self.ui.read();
            let t = ui.hist_pos as isize + delta;
            if t < 0 || t as usize >= ui.history.len() {
                drop(ui);
                // Nothing left behind: back lands on a new agent.
                if delta < 0 && !matches!(*self.view.peek(), View::Home) {
                    self.navigate(View::Home);
                }
                return;
            }
            (t as usize, ui.history[t as usize].clone())
        };
        if let Some(id) = target.1.session_id() {
            if !self.ensure_loaded(id) {
                return;
            }
        }
        if target.1.is_tab() {
            self.ensure_tab(&target.1, false);
        }
        self.ui.write().hist_pos = target.0;
        self.view.set(target.1);
    }

    // ------------------------------------------------------------- editing prompts

    /// Edit a prompt (`Some((id, text))`) or stop editing (`None`). When the
    /// open edit has unsaved changes this asks first instead.
    pub fn request_edit(mut self, target: Option<EditTarget>) {
        let st = self.editing.peek().clone();
        let same = match (&st.msg, &target) {
            (Some(a), Some((b, _, _))) => a == b,
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        if st.dirty() {
            self.editing.write().confirm = Some(target);
            document::eval("setTimeout(() => document.querySelector('.user-msg.editing')?.scrollIntoView({block: 'nearest', behavior: 'smooth'}), 20)");
            return;
        }
        self.set_edit(target);
    }

    /// Switch editing without asking.
    pub fn set_edit(mut self, target: Option<EditTarget>) {
        self.editing.set(match target {
            Some((id, text, atts)) => {
                // Its objects come back as objects, not the text the agent read.
                let (draft, mentions) = crate::ui::mentions::to_edit(&text, &self.sessions.peek());
                EditState { msg: Some(id), original: draft.clone(), draft, mentions, attachments: atts.clone(), original_attachments: atts, confirm: None }
            }
            None => EditState::default(),
        });
    }

    // ------------------------------------------------------------- tabs

    /// Make sure a subagent tab shows `v`: an existing tab is reused,
    /// otherwise the session's preview tab is replaced (or one is added).
    pub fn ensure_tab(mut self, v: &View, keep: bool) {
        let View::Subagent { session, .. } = v else { return };
        let mut tabs = self.tabs.write();
        if let Some(t) = tabs.iter_mut().find(|t| &t.view == v) {
            if keep {
                t.preview = false;
            }
            return;
        }
        let tab = ChatTab { view: v.clone(), preview: !keep };
        if !keep {
            if let Some(p) = tabs.iter().position(|t| t.preview && t.view.session_id() == Some(session)) {
                tabs[p] = tab;
                return;
            }
        }
        tabs.push(tab);
    }

    /// The tabs of the window: the session's own tab, then its subagents.
    pub fn window_tabs(&self) -> Vec<ChatTab> {
        let view = self.view.read().clone();
        let Some(session) = view.session_id().map(str::to_string) else { return Vec::new() };
        let mut out = vec![ChatTab { view: View::Agent(session.clone()), preview: false }];
        out.extend(self.tabs.read().iter().filter(|t| t.view.session_id() == Some(session.as_str())).cloned());
        out
    }

    /// Turn a preview tab into a regular one.
    pub fn keep_tab(mut self, v: &View) {
        if let Some(t) = self.tabs.write().iter_mut().find(|t| &t.view == v) {
            t.preview = false;
        }
    }


    /// Open a subagent's transcript in a tab.
    pub fn open_subagent(self, session_id: &str, tool_id: &str) {
        if !self.ensure_loaded(session_id) {
            return;
        }
        self.navigate(View::Subagent { session: session_id.to_string(), tool: tool_id.to_string() });
    }

    /// Close a subagent tab. The session's own tab stays.
    pub fn close_tab(mut self, v: &View) {
        let View::Subagent { session, .. } = v else { return };
        let next = {
            let mut tabs = self.tabs.write();
            let Some(i) = tabs.iter().position(|t| &t.view == v) else { return };
            tabs.remove(i);
            if *self.view.peek() != *v {
                return;
            }
            let same: Vec<&ChatTab> = tabs.iter().filter(|t| t.view.session_id() == Some(session.as_str())).collect();
            let at = tabs[..i].iter().filter(|t| t.view.session_id() == Some(session.as_str())).count();
            same.get(at).or_else(|| at.checked_sub(1).and_then(|j| same.get(j))).map(|t| t.view.clone())
        };
        self.navigate(next.unwrap_or_else(|| View::Agent(session.clone())));
    }

    /// Close the session's subagent tabs other than `v` ("others"), after
    /// it ("right") or all of them ("all").
    pub fn close_tabs(mut self, v: &View, which: &str) {
        let Some(session) = v.session_id().map(str::to_string) else { return };
        {
            let mut tabs = self.tabs.write();
            let mine = |t: &ChatTab| t.view.session_id() == Some(session.as_str());
            let keep_until = match v {
                View::Agent(_) => None,
                _ => tabs.iter().filter(|t| mine(t)).position(|t| &t.view == v),
            };
            let mut seen = 0usize;
            tabs.retain(|t| {
                if !mine(t) {
                    return true;
                }
                let idx = seen;
                seen += 1;
                match which {
                    "right" => keep_until.map(|k| idx <= k).unwrap_or(false),
                    "all" => false,
                    _ => &t.view == v,
                }
            });
        }
        let cur = self.view.peek().clone();
        if matches!(cur, View::Subagent { .. }) && cur.session_id() == Some(session.as_str()) && !self.tabs.peek().iter().any(|t| t.view == cur) {
            self.navigate(View::Agent(session));
        }
    }

    /// A session that no longer exists: out of the sidebar, the tabs and
    /// the back / forward history.
    fn forget_session(mut self, id: &str) {
        self.open.write().remove(id);
        self.forget_tabs(id);
        self.sessions.write().retain(|s| s.id != id);
        let mut ui = self.ui.write();
        let (history, pos) = without_session(std::mem::take(&mut ui.history), ui.hist_pos, id);
        ui.history = history;
        ui.hist_pos = pos;
    }

    /// Drop tabs (and the split) of a session that was deleted or archived.
    fn forget_tabs(mut self, id: &str) {
        self.tabs.write().retain(|t| t.view.session_id() != Some(id));
        if self.split.peek().as_deref() == Some(id) {
            self.split.set(None);
        }
    }

    /// Show `id` side by side with the current session.
    pub fn open_split(mut self, id: &str) {
        if !self.ensure_loaded(id) {
            return;
        }
        if self.current_id().as_deref() == Some(id) {
            return;
        }
        if self.current_id().is_none() {
            return self.open_session(id);
        }
        self.split.set(Some(id.to_string()));
    }

    /// Show `id` side by side, on the left: the open session moves right.
    pub fn open_split_left(mut self, id: &str) {
        if !self.ensure_loaded(id) {
            return;
        }
        let Some(cur) = self.current_id() else { return self.open_session(id) };
        if cur == id {
            return;
        }
        self.split.set(Some(cur));
        self.open_session(id);
    }

    pub fn new_agent(mut self) {
        self.navigate(View::Home);
        self.ui.write().focus_composer += 1;
    }

    pub fn open_settings(self, tab: SettingsTab) {
        self.navigate(View::Settings(tab));
    }

    /// Settings → Models, on one agent's models.
    pub fn open_models(mut self, agent: Option<AgentKind>) {
        if agent.is_some() {
            self.ui.write().models_agent = agent;
        }
        self.navigate(View::Settings(SettingsTab::Models));
    }

    /// Make sure a session is loaded into memory.
    pub fn ensure_loaded(self, id: &str) -> bool {
        if self.is_loaded(id) {
            return true;
        }
        match store().load(id) {
            Ok(s) => {
                self.put_session(s);
                true
            }
            Err(_) if !store().exists(id) => {
                self.forget_session(id);
                self.toast("That session no longer exists");
                false
            }
            Err(e) => {
                self.toast_err(format!("Could not open session: {e}"));
                false
            }
        }
    }

    /// Show a session in the window. Big sessions load off the UI thread so
    /// the window never freezes.
    pub fn open_session(self, id: &str) {
        if self.is_loaded(id) {
            return self.show_session(id);
        }
        let id = id.to_string();
        spawn_forever(async move {
            let id2 = id.clone();
            match tokio::task::spawn_blocking(move || store().load(&id2)).await {
                Ok(Ok(s)) => {
                    if !self.is_loaded(&id) {
                        self.put_session(s);
                    }
                    self.show_session(&id);
                }
                Ok(Err(_)) if !store().exists(&id) => {
                    self.forget_session(&id);
                    self.toast("That session no longer exists");
                }
                Ok(Err(e)) => self.toast_err(format!("Could not open session: {e}")),
                Err(e) => self.toast_err(format!("Could not open session: {e}")),
            }
        });
    }

    fn show_session(mut self, id: &str) {
        self.navigate(View::Agent(id.to_string()));
        if self.split.peek().as_deref() == Some(id) {
            self.split.set(None);
        }
        let unread = self.peek_session(id, |s| s.unread).unwrap_or(false);
        if unread {
            self.update_session(id, |s| s.unread = false);
        }
        self.ui.write().focus_composer += 1;
    }

    /// Mutate a session, persist it and refresh the sidebar.
    pub fn update_session(self, id: &str, f: impl FnOnce(&mut Session)) {
        if !self.ensure_loaded(id) {
            return;
        }
        let Some(snapshot) = self.mutate_session(id, |s| {
            f(s);
            s.clone()
        }) else {
            return;
        };
        self.save_session(&snapshot);
    }

    // ------------------------------------------------------------- running agents

    /// Send a prompt. `session_id == None` starts a new agent from the draft.
    pub fn send(self, session_id: Option<String>, text: String, attachments: Vec<Part>) {
        self.send_with(session_id, text, attachments, None);
    }

    /// [`App::send`], opening the reply with `notice` (what happened to the
    /// agent's own session, say).
    fn send_with(mut self, session_id: Option<String>, text: String, attachments: Vec<Part>, notice: Option<String>) {
        if text.trim().is_empty() && attachments.is_empty() {
            return;
        }
        let id = match session_id {
            Some(id) => {
                if self.is_running(&id) {
                    let how = self.config.read().settings.busy_send;
                    self.send_while_busy(&id, text, attachments, how);
                    return;
                }
                if !self.ensure_loaded(&id) {
                    return;
                }
                if self.running_elsewhere(&id) {
                    let agent = self.peek_session(&id, |s| s.agent.display_name()).unwrap_or_default();
                    self.toast(format!("This chat is running in {agent} right now. Reply there, or send once it finishes."));
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
                // A leading "/skill" must stay first for the agent to run it.
                match text.strip_prefix('/').and_then(|rest| rest.split_once(char::is_whitespace)) {
                    Some((skill, rest)) if !skill.is_empty() => format!("/{skill}\n{quoted}\n{}", rest.trim_start()),
                    _ => format!("{quoted}\n{text}"),
                }
            }
            _ => text,
        };
        let mut msg = Message::user_text(text);
        msg.parts.extend(attachments);
        let Some((agent, model, account)) = self.mutate_session(&id, |s| {
            s.push(msg);
            s.auto_title();
            s.archived = false;
            (s.agent.clone(), s.model.clone(), s.account_id.clone())
        }) else {
            return;
        };
        // A session shown side by side keeps its place; otherwise show it.
        if self.split.peek().as_deref() != Some(id.as_str()) {
            self.navigate(View::Agent(id.clone()));
        }

        let cfg = self.config.read().clone();
        let request = self.peek_session(&id, |s| larrier_agents::build_request(&cfg, s, &agent)).expect("loaded");
        let mut recorder = self.mutate_session(&id, |s| TurnRecorder::begin(s, agent.clone(), model, account)).expect("loaded");
        if let Some(text) = notice {
            self.mutate_session(&id, |s| recorder.apply(s, &AgentEvent::Notice { text }));
        }
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
                let snap = self.mutate_session(&id, |s| {
                    recorder.apply(s, &AgentEvent::Error { message: e.to_string() });
                    recorder.finish(s, false);
                    s.clone()
                });
                if let Some(snap) = snap {
                    self.save_session(&snap);
                }
            }
        }
    }

    fn persist(self, id: &str) {
        self.save_loaded(id);
    }

    fn drive(self, id: String, mut handle: TurnHandle, mut rec: TurnRecorder) {
        let mut app = self;
        // Our agent writes the native session too: keep live sync off it.
        crate::live::mark_busy(&id);
        spawn_forever(async move {
            let mut last_save = Instant::now();
            let mut last_row = Instant::now();
            let mut done = false;
            // The agent used the browser this turn (the tab opened once).
            let mut browsing = false;
            let Some(mut sig) = app.open.peek().get(&id).copied() else {
                app.running.write().remove(&id);
                crate::live::mark_idle(&id);
                return;
            };
            while let Some(first) = handle.events.recv().await {
                let mut batch = vec![first];
                while let Ok(e) = handle.events.try_recv() {
                    batch.push(e);
                }
                let mut snapshot = None;
                // Whether the agent is writing (false) or finished its reply
                // with background work still running (true).
                let mut idle = None;
                {
                    let mut s = sig.write();
                    let s = &mut *s;
                    for e in &batch {
                        match e {
                            AgentEvent::Idle => idle = Some(true),
                            AgentEvent::TextDelta { .. }
                            | AgentEvent::Text { .. }
                            | AgentEvent::ReasoningDelta { .. }
                            | AgentEvent::Reasoning { .. }
                            | AgentEvent::ToolStart { .. }
                            | AgentEvent::Plan { .. }
                            | AgentEvent::Error { .. } => idle = Some(false),
                            _ => {}
                        }
                        match e {
                            AgentEvent::ToolStart { tool } if !browsing && is_browser_tool(tool) => {
                                browsing = true;
                                if app.current_id().as_deref() == Some(id.as_str()) {
                                    app.ui.write().open_panel(RightTab::Browser, false);
                                }
                            }
                            AgentEvent::Permission { request } => {
                                idle = Some(false);
                                app.permissions.write().entry(id.clone()).or_default().push(request.clone());
                                crate::notify::agent_event(&s.title, &format!("{} needs your input", s.title), true);
                            }
                            AgentEvent::Steered { text, .. } => {
                                if let Some(v) = app.steering.write().get_mut(&id) {
                                    if let Some(i) = v.iter().position(|q| &q.text == text) {
                                        v.remove(i);
                                    }
                                }
                            }
                            AgentEvent::Unsteered { key, taken_back } => {
                                app.ui.write().unsteering.remove(key);
                                if *taken_back {
                                    let item = app.steering.write().get_mut(&id).and_then(|v| v.iter().position(|q| q.id == *key).map(|i| v.remove(i)));
                                    if let Some(item) = item {
                                        let mut ui = app.ui.write();
                                        ui.restore_inbox.push((id.clone(), item));
                                        ui.focus_composer += 1;
                                    }
                                } else {
                                    app.toast(format!("{} had already read that message, so it stays in the chat", s.agent.display_name()));
                                }
                            }
                            AgentEvent::PermissionClosed { id: rid } => {
                                if let Some(v) = app.permissions.write().get_mut(&id) {
                                    v.retain(|r| &r.id != rid);
                                }
                            }
                            AgentEvent::NativeSession { id: native, .. } => {
                                // A session imported from this very native
                                // session keeps syncing from it (the agent
                                // writes Larrier's turn there too); any other
                                // native session is this one's own.
                                if s.origin.native_id.as_deref() != Some(native.as_str()) {
                                    store().set_import_record(&import_key(&rec.agent, native), ImportRecord::linked(&id));
                                    store().flush_imports();
                                }
                            }
                            AgentEvent::Log { line } => tracing::debug!("[{}] {line}", rec.agent.id()),
                            AgentEvent::RateLimit { window, used_pct, resets_at } => {
                                crate::ui::usage::observe_limit(&rec.agent, s.account_id.as_deref(), window, *used_pct, *resets_at);
                            }
                            AgentEvent::Done { .. } => done = true,
                            _ => {}
                        }
                        rec.apply(s, e);
                    }
                    // Save every few seconds and refresh the sidebar row a few
                    // times a second — never per token.
                    if done || last_save.elapsed() > Duration::from_secs(3) {
                        save_async(s.clone());
                        last_save = Instant::now();
                    }
                    if last_row.elapsed() > Duration::from_millis(400) {
                        snapshot = Some(s.summary());
                        last_row = Instant::now();
                    }
                }
                if let Some(v) = idle.filter(|v| *v != app.idle.peek().contains(&id)) {
                    if v {
                        app.idle.write().insert(id.clone());
                    } else {
                        app.idle.write().remove(&id);
                    }
                }
                if done {
                    break;
                }
                if let Some(sum) = snapshot {
                    let mut list = app.sessions.write();
                    if let Some(i) = list.iter().position(|x| x.id == sum.id) {
                        list[i] = sum;
                    }
                }
                // Coalesce bursts of events into one render per frame.
                tokio::time::sleep(Duration::from_millis(40)).await;
            }
            let is_current = app.current_id().as_deref() == Some(id.as_str()) || app.split.peek().as_deref() == Some(id.as_str());
            let title = app.mutate_session(&id, |s| {
                if !rec.finished {
                    rec.finish(s, false);
                }
                if is_current {
                    s.unread = false;
                }
                (s.title.clone(), s.status, s.clone())
            });
            let title = title.map(|(t, st, snap)| {
                app.save_session(&snap);
                (t, st)
            });
            app.running.write().remove(&id);
            app.permissions.write().remove(&id);
            if app.idle.peek().contains(&id) {
                app.idle.write().remove(&id);
            }
            crate::live::mark_idle(&id);
            if let Some((t, st)) = &title {
                let ok = *st != SessionStatus::Error;
                let what = if ok { "is ready for review" } else { "failed" };
                crate::notify::agent_event(t, &format!("{t} {what}"), ok);
            }
            // Messages sent into the turn that it never took in go first in
            // the queue.
            let untaken = app.steering.write().remove(&id).unwrap_or_default();
            if !untaken.is_empty() {
                let mut q = app.queue.write();
                let v = q.entry(id.clone()).or_default();
                for (i, m) in untaken.into_iter().enumerate() {
                    v.insert(i, m);
                }
            }
            // After the user's own stop the queue waits, unless the stop was
            // to send the first queued message now.
            let stopped = app.stopping.write().remove(&id);
            if stopped == Some(false) {
                return;
            }
            // Send the next queued follow-up, if any.
            let next = {
                let mut q = app.queue.write();
                let n = q.get_mut(&id).and_then(|v| if v.is_empty() { None } else { Some(v.remove(0)) });
                if q.get(&id).map(|v| v.is_empty()).unwrap_or(false) {
                    q.remove(&id);
                }
                n
            };
            if let Some(n) = next {
                app.send(Some(id.clone()), n.text, n.attachments);
                return;
            }
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

    pub fn stop(mut self, id: &str) {
        if let Some(c) = self.running.read().get(id) {
            self.stopping.write().entry(id.to_string()).or_insert(false);
            c.cancel();
        }
    }

    /// A message for a session whose agent is working: into the turn, into
    /// the queue, or sent now after stopping the agent.
    pub fn send_while_busy(mut self, id: &str, text: String, attachments: Vec<Part>, how: BusySend) {
        let agent = self.peek_session(id, |s| s.agent.clone()).unwrap_or(AgentKind::ClaudeCode);
        let steerable = larrier_agents::can_steer(&agent, &self.config.read());
        let item = Queued { id: rand_id(), text, attachments };
        match how {
            BusySend::Steer | BusySend::Interrupt if steerable => {
                if let Some(c) = self.running.read().get(id) {
                    c.steer(item.text.clone(), item.attachments.clone(), how == BusySend::Interrupt, item.id);
                }
                self.steering.write().entry(id.to_string()).or_default().push(item);
            }
            BusySend::Steer => {
                self.queue.write().entry(id.to_string()).or_default().push(item);
                self.toast(format!("{} can't take messages while it works, so this is queued for when it finishes", agent.display_name()));
            }
            BusySend::Queue => self.queue.write().entry(id.to_string()).or_default().push(item),
            BusySend::Interrupt => {
                self.queue.write().entry(id.to_string()).or_default().insert(0, item);
                self.stopping.write().insert(id.to_string(), true);
                if let Some(c) = self.running.read().get(id) {
                    c.cancel();
                }
            }
        }
    }

    /// Take back a message sent into the running turn that the agent hasn't
    /// read yet; it goes back into the composer (see `Unsteered`).
    pub fn unsteer(mut self, id: &str, key: u64) {
        if let Some(c) = self.running.read().get(id) {
            self.ui.write().unsteering.insert(key);
            c.unsteer(key);
        }
    }

    pub fn queue_move(mut self, id: &str, from: usize, to: usize) {
        if let Some(v) = self.queue.write().get_mut(id) {
            if from < v.len() && from != to {
                let item = v.remove(from);
                v.insert(to.min(v.len()), item);
            }
        }
    }

    pub fn queue_remove(mut self, id: &str, qid: u64) {
        let mut q = self.queue.write();
        if let Some(v) = q.get_mut(id) {
            v.retain(|x| x.id != qid);
            if v.is_empty() {
                q.remove(id);
            }
        }
    }

    pub fn queue_update(mut self, id: &str, qid: u64, text: String, attachments: Vec<Part>) {
        if let Some(item) = self.queue.write().get_mut(id).and_then(|v| v.iter_mut().find(|x| x.id == qid)) {
            item.text = text;
            item.attachments = attachments;
        }
    }

    /// Take a queued message out and send it with `how` (or now, when the
    /// agent has finished meanwhile).
    pub fn queue_send(self, id: &str, qid: u64, how: BusySend) {
        let item = self.queue.peek().get(id).and_then(|v| v.iter().find(|x| x.id == qid).cloned());
        let Some(item) = item else { return };
        self.queue_remove(id, qid);
        if self.is_running(id) {
            self.send_while_busy(id, item.text, item.attachments, how);
        } else {
            self.send(Some(id.to_string()), item.text, item.attachments);
        }
    }

    /// Answer request `request_id` of a running agent.
    pub fn respond_permission(mut self, session_id: &str, request_id: &str, reply: Reply) {
        let left = {
            let mut p = self.permissions.write();
            let v = p.entry(session_id.to_string()).or_default();
            v.retain(|r| r.id != request_id);
            v.len()
        };
        if let Some(ctrl) = self.running.read().get(session_id) {
            ctrl.respond(request_id, reply);
        }
        if left == 0 {
            self.mutate_session(session_id, |s| s.status = SessionStatus::Running);
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
        s.effort = d.effort.clone();
        s.mode = d.mode.clone();
        s.permission = d.permission.clone();
        s.account_id = d.account_id.clone();
        s.run_as = d.run_as.clone().filter(|_| d.agent == AgentKind::ClaudeCode);
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
        self.save_session(&s);
        self.put_session(s);
        self.ensure_tab(&View::Agent(id.clone()), true);
        // Its project opens, so the new chat is seen arriving in it.
        self.set_group_open(&project_key(d.workspace.as_deref()), true);
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

    /// Look for a newer release. `quiet` (the automatic look) keeps failures
    /// (offline, no releases yet) out of sight.
    pub fn check_updates(self, quiet: bool) {
        spawn_forever(async move {
            self.look_for_update(quiet).await;
        });
    }

    /// Look for a newer release and say what was found. The automatic look
    /// (`quiet`) keeps failures out of sight and downloads what it finds
    /// unless the user only wants to be told. Returns whether GitHub could
    /// be asked (the automatic look tries again soon when it couldn't).
    pub async fn look_for_update(mut self, quiet: bool) -> bool {
        let before = self.update.peek().clone();
        if matches!(before, UpdateState::Checking | UpdateState::Downloading { .. }) {
            return true;
        }
        let waiting = matches!(before, UpdateState::Ready { .. });
        if !quiet && !waiting {
            self.update.set(UpdateState::Checking);
        }
        let channel = self.config.peek().settings.update_channel;
        let found = match crate::update::check(channel).await {
            Ok(found) => found,
            Err(_) if quiet => return false,
            Err(e) => {
                self.update.set(UpdateState::Failed(format!("{e:#}")));
                return false;
            }
        };
        // A release already downloaded waits for a restart: only a newer
        // one takes its place.
        if let UpdateState::Ready { version, .. } = &before {
            if !found.as_ref().is_some_and(|r| crate::version::newer(&r.version, version)) {
                return true;
            }
        }
        let fresh = found.is_some();
        self.update.set(match found {
            Some(rel) => UpdateState::Available(rel),
            None => UpdateState::UpToDate,
        });
        // Found on its own: download it now unless the user only wants to
        // be told.
        let mode = self.config.peek().settings.updates;
        if fresh && quiet && matches!(mode, larrier_core::UpdateMode::Download | larrier_core::UpdateMode::Install) {
            self.download_update();
        }
        true
    }

    /// Put a downloaded release in place when Larrier closes (the Install
    /// mode): it waits for this process to end, replaces it, and stays
    /// closed. Called by every way of closing (once).
    pub fn install_on_exit(self) {
        static HANDED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if self.config.peek().settings.updates != larrier_core::UpdateMode::Install {
            return;
        }
        if let UpdateState::Ready { path, .. } = self.update.peek().clone() {
            if HANDED.swap(true, std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            if let Err(e) = crate::update::install_after_exit(&path) {
                tracing::warn!("installing the update on exit: {e:#}");
            }
        }
    }

    /// Download the available release (checked against its checksum).
    pub fn download_update(mut self) {
        let UpdateState::Available(rel) = self.update.peek().clone() else { return };
        self.update.set(UpdateState::Downloading { version: rel.version.clone(), got: 0, total: rel.size });
        spawn_forever(async move {
            let version = rel.version.clone();
            let mut shown = 0u64;
            let r = crate::update::download(&rel, |got, total| {
                // A redraw per percent, not per chunk.
                let pct = if total > 0 { got * 100 / total } else { 0 };
                if pct != shown {
                    shown = pct;
                    self.update.set(UpdateState::Downloading { version: version.clone(), got, total });
                }
            })
            .await;
            self.update.set(match r {
                Ok(path) => UpdateState::Ready { version, path },
                Err(e) => UpdateState::Failed(format!("{e:#}")),
            });
        });
    }

    /// Close and let the downloaded release take this one's place.
    pub fn restart_to_update(self) {
        let UpdateState::Ready { path, .. } = self.update.peek().clone() else { return };
        if !self.running.peek().is_empty() {
            return self.toast("Agents are still working — stop them or wait, then restart to update");
        }
        match crate::update::restart_into(&path) {
            Ok(()) => std::process::exit(0),
            Err(e) => self.toast_err(format!("Could not start the update: {e:#}")),
        }
    }

    /// Clear every unread dot (sessions not open are saved off the UI thread).
    pub fn mark_all_read(mut self) {
        let ids: Vec<String> = self.sessions.peek().iter().filter(|s| s.unread).map(|s| s.id.clone()).collect();
        if ids.is_empty() {
            return self.toast("Nothing unread");
        }
        for s in self.sessions.write().iter_mut() {
            s.unread = false;
        }
        let (open, closed): (Vec<String>, Vec<String>) = ids.iter().cloned().partition(|id| self.is_loaded(id));
        for id in &open {
            self.update_session(id, |s| s.unread = false);
        }
        spawn_forever(async move {
            let _ = tokio::task::spawn_blocking(move || {
                for id in closed {
                    if let Ok(mut s) = store().load(&id) {
                        s.unread = false;
                        let _ = store().save(&s);
                    }
                }
            })
            .await;
        });
        let n = ids.len();
        self.toast(format!("Marked {n} session{} as read", if n == 1 { "" } else { "s" }));
    }

    pub fn toggle_archive(mut self, id: &str) {
        let mut archived = false;
        self.update_session(id, |s| {
            s.archived = !s.archived;
            archived = s.archived;
        });
        if archived {
            let cur = self.current_id().as_deref() == Some(id);
            self.forget_tabs(id);
            if cur {
                self.view.set(View::Home);
            }
        }
    }

    pub fn delete(mut self, id: &str) {
        self.stop(id);
        // Clean up a worktree we created.
        if let Some(s) = self.peek_session(id, |s| s.clone()).or_else(|| store().load(id).ok()) {
            if let (Environment::Worktree { path, .. }, Some(ws)) = (&s.environment, &s.workspace) {
                let _ = git::remove_worktree(&PathBuf::from(&ws.path), &PathBuf::from(path));
            }
        }
        if let Err(e) = store().delete(id) {
            self.toast_err(format!("Delete failed: {e}"));
        }
        let cur = self.view.peek().session_id() == Some(id);
        self.forget_session(id);
        if cur {
            self.navigate(View::Home);
        }
    }

    pub fn fork(self, id: &str, message_id: Option<String>) {
        if !self.ensure_loaded(id) {
            return;
        }
        let forked = self.peek_session(id, |s| s.fork(message_id.as_deref()));
        if let Some(f) = forked {
            let fid = f.id.clone();
            self.save_session(&f);
            self.put_session(f);
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
            s.effort = None;
            s.mode = None;
            s.permission = None;
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

    pub fn set_session_effort(self, id: &str, effort: Option<String>) {
        self.update_session(id, |s| s.effort = effort);
    }

    pub fn set_session_mode(self, id: &str, mode: Option<String>) {
        self.update_session(id, |s| s.mode = mode);
    }

    /// Option of the agent's second mode group (permissions / autonomy).
    pub fn set_session_permission(self, id: &str, permission: Option<String>) {
        self.update_session(id, |s| s.permission = permission);
    }

    pub fn set_session_account(self, id: &str, account: Option<String>) {
        self.update_session(id, |s| s.account_id = account);
    }

    pub fn export(self, id: &str, format: ExportFormat) {
        if !self.ensure_loaded(id) {
            return;
        }
        let Some(s) = self.peek_session(id, |s| s.clone()) else { return };
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
        let Some(s) = self.peek_session(id, |s| s.clone()) else { return };
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
            larrier_formats::importable_agents().into_iter().filter(|a| cfg.agent(a).import && cfg.agent(a).enabled).collect()
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
    /// Edit a past prompt in place: the prompt and everything after it are
    /// replaced by the new one, and the agent's own session follows as far
    /// as the agent allows ([`larrier_formats::Rewind`]). Where it can't, the
    /// reply says so.
    pub fn edit_and_resend(self, session_id: &str, message_id: &str, text: String, attachments: Vec<Part>) {
        if (text.trim().is_empty() && attachments.is_empty()) || !self.ensure_loaded(session_id) {
            return;
        }
        if self.is_running(session_id) {
            return self.toast("Stop the agent before editing an earlier prompt");
        }
        let cfg = self.config.read().clone();
        let found = self.peek_session(session_id, |s| {
            let idx = s.messages.iter().position(|m| m.id == message_id)?;
            let home = account_home(&cfg, &s.agent, s.account_id.as_deref());
            let old_link = s.native_link(&s.agent).map(|l| l.id.clone());
            Some((idx, larrier_formats::rewind(s, idx, home.as_deref()), s.agent.clone(), old_link))
        });
        let Some(Some((idx, rewind, agent, old_link))) = found else { return };
        let name = agent.display_name();
        let notice = match &rewind {
            larrier_formats::Rewind::ResumeAt(_) | larrier_formats::Rewind::Fresh => None,
            larrier_formats::Rewind::Copy(_) => Some(format!("{name} can't rewind its own session: it continues from a copy written up to this prompt.")),
            larrier_formats::Rewind::Handoff if old_link.is_some() => {
                Some(format!("{name} can't rewind its own chat: it continues in a new one, with the conversation before this prompt as context."))
            }
            larrier_formats::Rewind::Handoff => None,
        };
        // Whatever the agent writes to its old session no longer belongs here.
        if let Some(old) = &old_link {
            store().set_import_record(&import_key(&agent, old), ImportRecord::linked(session_id));
        }
        if let larrier_formats::Rewind::Copy(link) = &rewind {
            store().set_import_record(&import_key(&agent, &link.id), ImportRecord::linked(session_id));
        }
        store().flush_imports();
        self.update_session(session_id, move |s| {
            s.messages.truncate(idx);
            match rewind {
                larrier_formats::Rewind::ResumeAt(at) => {
                    if let Some(l) = s.native_link_mut(&agent) {
                        l.resume_at = Some(at);
                        l.synced_messages = idx;
                    }
                }
                larrier_formats::Rewind::Copy(link) => s.set_native_link(link),
                larrier_formats::Rewind::Fresh | larrier_formats::Rewind::Handoff => s.native.retain(|l| l.agent != agent),
            }
            s.recompute_usage();
        });
        let editing = self.editing;
        if editing.peek().msg.as_deref() == Some(message_id) {
            let mut e = editing;
            e.set(EditState::default());
        }
        self.send_with(Some(session_id.to_string()), text, attachments, notice);
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
        let Some(dir) = self.peek_session(id, |s| s.cwd()).flatten() else {
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
        // Objects' invisible marks (see ui::mentions) stay in the text box.
        let text: String = text.chars().filter(|c| !matches!(*c, crate::ui::mentions::OPEN | crate::ui::mentions::CLOSE)).collect();
        match arboard::Clipboard::new().and_then(|mut c| c.set_text(text)) {
            Ok(()) => {}
            Err(e) => self.toast_err(format!("Copy failed: {e}")),
        }
    }

    /// Put an image (as shown: a data URL, a file, or a web address) on the
    /// clipboard as a picture.
    pub fn copy_image(self, src: String) {
        spawn(async move {
            let done = match image_bytes(&src).await {
                Ok((bytes, _)) => tokio::task::spawn_blocking(move || -> Result<(), String> {
                    let img = image::load_from_memory(&bytes).map_err(|e| e.to_string())?.to_rgba8();
                    let (w, h) = img.dimensions();
                    let data = arboard::ImageData { width: w as usize, height: h as usize, bytes: std::borrow::Cow::Owned(img.into_raw()) };
                    arboard::Clipboard::new().and_then(|mut c| c.set_image(data)).map_err(|e| e.to_string())
                })
                .await
                .unwrap_or_else(|e| Err(e.to_string())),
                Err(e) => Err(e),
            };
            match done {
                Ok(()) => self.toast("Image copied".to_string()),
                Err(e) => self.toast_err(format!("Couldn't copy the image: {e}")),
            }
        });
    }

    /// Save an image where the user picks.
    pub fn save_image(self, src: String) {
        spawn(async move {
            let (bytes, ext) = match image_bytes(&src).await {
                Ok(b) => b,
                Err(e) => return self.toast_err(format!("Couldn't save the image: {e}")),
            };
            let pick = rfd::AsyncFileDialog::new().set_file_name(format!("image.{ext}")).add_filter("Image", &[ext.as_str()]).save_file().await;
            if let Some(f) = pick {
                if let Err(e) = std::fs::write(f.path(), &bytes) {
                    self.toast_err(format!("Couldn't save the image: {e}"));
                }
            }
        });
    }

    /// Open an image in the system's viewer (an inline one goes through a
    /// temporary file first).
    pub fn open_image(self, src: String) {
        if !src.starts_with("data:") {
            let _ = open::that(&src);
            return;
        }
        spawn(async move {
            match image_bytes(&src).await {
                Ok((bytes, ext)) => {
                    use sha2::Digest;
                    let name = format!("larrier-image-{}.{ext}", hex_short(&sha2::Sha256::digest(&bytes)));
                    let path = std::env::temp_dir().join(name);
                    match std::fs::write(&path, &bytes) {
                        Ok(()) => {
                            let _ = open::that(&path);
                        }
                        Err(e) => self.toast_err(format!("Couldn't open the image: {e}")),
                    }
                }
                Err(e) => self.toast_err(format!("Couldn't open the image: {e}")),
            }
        });
    }
}

fn hex_short(d: &[u8]) -> String {
    d.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// An image's bytes and a file extension for it, from how it's shown (a
/// data URL, a local path, or a web address).
async fn image_bytes(src: &str) -> Result<(Vec<u8>, String), String> {
    use base64::Engine;
    let ext_of = |mime: &str| match mime {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        _ => "png",
    };
    if let Some(rest) = src.strip_prefix("data:") {
        let (head, data) = rest.split_once(',').ok_or("not an image")?;
        let mime = head.split(';').next().unwrap_or("image/png");
        let bytes = base64::engine::general_purpose::STANDARD.decode(data.trim()).map_err(|e| e.to_string())?;
        return Ok((bytes, ext_of(mime).to_string()));
    }
    if src.starts_with("http://") || src.starts_with("https://") {
        let r = reqwest::get(src).await.map_err(|e| e.to_string())?;
        let mime = r.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("image/png").to_string();
        let bytes = r.bytes().await.map_err(|e| e.to_string())?.to_vec();
        return Ok((bytes, ext_of(&mime).to_string()));
    }
    let path = src.strip_prefix("file:///").unwrap_or(src);
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let ext = std::path::Path::new(path).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_else(|| "png".into());
    Ok((bytes, ext))
}

enum SaveMsg {
    Save(Box<Session>),
    Flush(mpsc::Sender<()>),
}

fn saver() -> &'static Mutex<mpsc::Sender<SaveMsg>> {
    static TX: OnceLock<Mutex<mpsc::Sender<SaveMsg>>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<SaveMsg>();
        std::thread::Builder::new()
            .name("larrier-save".into())
            .spawn(move || {
                while let Ok(first) = rx.recv() {
                    // Coalesce: only the newest snapshot of each session is written.
                    let mut pending: Vec<Session> = Vec::new();
                    let mut acks = Vec::new();
                    let mut take = |m: SaveMsg, pending: &mut Vec<Session>| match m {
                        SaveMsg::Save(s) => {
                            pending.retain(|p| p.id != s.id);
                            pending.push(*s);
                        }
                        SaveMsg::Flush(a) => acks.push(a),
                    };
                    take(first, &mut pending);
                    while let Ok(m) = rx.try_recv() {
                        take(m, &mut pending);
                    }
                    for s in pending {
                        if let Err(e) = store().save(&s) {
                            tracing::error!("saving {}: {e}", s.id);
                        }
                    }
                    for a in acks {
                        let _ = a.send(());
                    }
                }
            })
            .expect("save thread");
        Mutex::new(tx)
    })
}

/// Write a session to disk off the UI thread.
pub fn save_async(s: Session) {
    if let Ok(tx) = saver().lock() {
        let _ = tx.send(SaveMsg::Save(Box::new(s)));
    }
}

/// Wait (briefly) until every queued save is on disk — used when closing.
pub fn flush_saves() {
    let (ack_tx, ack_rx) = mpsc::channel();
    if let Ok(tx) = saver().lock() {
        if tx.send(SaveMsg::Flush(ack_tx)).is_err() {
            return;
        }
    }
    let _ = ack_rx.recv_timeout(Duration::from_secs(5));
}

/// An imported session still mid-turn in its agent's own app. Claude Code
/// and Codex mark turn ends in the transcript (`external_turn`); a turn left
/// open by a crashed CLI stops counting after a while. Agents without turn
/// markers count while their files are being written (`live_recent`).
/// Sidebar key of Home, the project of chats that have none.
pub const HOME_KEY: &str = "~home";

/// Sidebar key of a session's project: its folder, or [`HOME_KEY`] for
/// chats without a project — no folder, the home folder itself, Codex's
/// project-less thread folders (`~/Documents/Codex/<date>/<name>`) and the
/// scratch folders of Claude's "This computer" sessions
/// (`<app data>/Claude/scratch-workspaces/<id>/<id>/scratch-…`). A project
/// made inside one of those is a project of its own.
pub fn project_key(path: Option<&str>) -> String {
    let norm = |p: &str| p.replace('\\', "/").trim_end_matches('/').to_lowercase();
    let Some(p) = path.map(norm).filter(|p| !p.is_empty()) else { return HOME_KEY.into() };
    // Scratch roots and how many folders deep their scratch folders are.
    static HOMES: OnceLock<(Option<String>, Vec<(String, usize)>)> = OnceLock::new();
    let (home, roots) = HOMES.get_or_init(|| {
        let home = dirs::home_dir().map(|h| norm(&h.to_string_lossy()));
        let mut roots: Vec<(String, usize)> = home.iter().map(|h| (format!("{h}/documents/codex/"), 2)).collect();
        roots.extend(dirs::config_dir().map(|c| (format!("{}/claude/scratch-workspaces/", norm(&c.to_string_lossy())), 3)));
        (home, roots)
    });
    let scratch = roots.iter().any(|(r, depth)| p.strip_prefix(r.as_str()).map(|rest| rest.split('/').count() == *depth).unwrap_or(false));
    if home.as_deref() == Some(p.as_str()) || scratch {
        return HOME_KEY.into();
    }
    p
}

/// Back / forward history without the views of session `id`: where `pos`
/// lands (the nearest view before it), and no view twice in a row.
fn without_session(history: Vec<View>, pos: usize, id: &str) -> (Vec<View>, usize) {
    let mut kept: Vec<View> = Vec::with_capacity(history.len());
    let mut at = 0;
    for (i, v) in history.into_iter().enumerate() {
        if v.session_id() == Some(id) || kept.last() == Some(&v) {
            continue;
        }
        kept.push(v);
        if i <= pos {
            at = kept.len() - 1;
        }
    }
    (kept, at)
}

pub fn external_active(s: &SessionSummary, live_recent: bool) -> bool {
    if s.external_turn {
        return chrono::Utc::now() - s.updated_at < chrono::Duration::minutes(45);
    }
    live_recent && !matches!(s.origin, AgentKind::ClaudeCode | AgentKind::Codex | AgentKind::OpenCode | AgentKind::Hermes | AgentKind::Pi)
}

/// Save a pasted / dropped image (a `data:` URL) as an attachment file.
pub fn save_data_url(data_url: &str, name: &str) -> anyhow::Result<Part> {
    use base64::Engine;
    let (meta, b64) = data_url.split_once(',').ok_or_else(|| anyhow::anyhow!("bad data url"))?;
    let mime = meta.trim_start_matches("data:").split(';').next().unwrap_or("application/octet-stream").to_string();
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64.trim())?;
    let ext = match mime.as_str() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        _ => std::path::Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("bin"),
    };
    let dir = store().root().join("attachments");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.{ext}", new_id()));
    std::fs::write(&path, &bytes)?;
    let path = path.to_string_lossy().to_string();
    if mime.starts_with("image/") {
        Ok(Part::Image { mime: Some(mime), source: MediaSource::Path { path } })
    } else {
        Ok(Part::File { path, mime: Some(mime), text: None })
    }
}

/// Attachment for a local file path (images become image parts).
pub fn attachment_for_path(path: &str) -> Part {
    let lower = path.to_lowercase();
    let mime = [("png", "image/png"), ("jpg", "image/jpeg"), ("jpeg", "image/jpeg"), ("gif", "image/gif"), ("webp", "image/webp"), ("bmp", "image/bmp"), ("svg", "image/svg+xml")]
        .iter()
        .find(|(e, _)| lower.ends_with(&format!(".{e}")))
        .map(|(_, m)| m.to_string());
    match mime {
        Some(m) => Part::Image { mime: Some(m), source: MediaSource::Path { path: path.to_string() } },
        None => Part::File { path: path.to_string(), mime: None, text: None },
    }
}

/// `data:` URL for showing an image part (cached; large files skipped).
pub fn image_src(part: &Part) -> Option<String> {
    use base64::Engine;
    use std::sync::Mutex;
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    let Part::Image { mime, source } = part else { return None };
    match source {
        MediaSource::Base64 { data } => Some(format!("data:{};base64,{data}", mime.clone().unwrap_or_else(|| "image/png".into()))),
        MediaSource::Url { url } => Some(url.clone()),
        MediaSource::Path { path } => {
            let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
            if let Some(v) = cache.lock().ok()?.get(path) {
                return Some(v.clone());
            }
            let meta = std::fs::metadata(path).ok()?;
            if meta.len() > 12 * 1024 * 1024 {
                return None;
            }
            let bytes = std::fs::read(path).ok()?;
            let m = mime.clone().unwrap_or_else(|| "image/png".into());
            let url = format!("data:{m};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes));
            cache.lock().ok()?.insert(path.clone(), url.clone());
            Some(url)
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

/// A call to Larrier's browser tools (`mcp__larrier_browser__navigate`,
/// or an ACP agent's "larrier_browser: …").
fn is_browser_tool(t: &larrier_core::ToolCall) -> bool {
    let n = larrier_agents::browser::SERVER_NAME;
    t.name.contains(n) || t.title.as_deref().is_some_and(|x| x.contains(n))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chats_without_a_project_go_home() {
        assert_eq!(project_key(None), HOME_KEY);
        assert_eq!(project_key(Some("")), HOME_KEY);
        assert_eq!(project_key(Some("D:\\Work\\App\\")), "d:/work/app");
        let home = dirs::home_dir().unwrap().to_string_lossy().to_string();
        assert_eq!(project_key(Some(&home)), HOME_KEY);
        assert_eq!(project_key(Some(&format!("{home}/Documents/Codex/2026-10-30/fix-login"))), HOME_KEY);
        assert_ne!(project_key(Some(&format!("{home}/projects/app"))), HOME_KEY);
        let app_data = dirs::config_dir().unwrap().to_string_lossy().to_string();
        assert_eq!(project_key(Some(&format!("{app_data}/Claude/scratch-workspaces/a/b/scratch-2026-10-29-4d46fd"))), HOME_KEY);
        // A project made inside a scratch folder is a project.
        assert_ne!(project_key(Some(&format!("{app_data}/Claude/scratch-workspaces/a/b/scratch-2026-01-01-000000/demo"))), HOME_KEY);
    }

    #[test]
    fn a_deleted_session_leaves_the_history() {
        let a = || View::Agent("a".into());
        let b = || View::Agent("b".into());
        let set = || View::Settings(SettingsTab::General);
        let (h, pos) = without_session(vec![View::Home, a(), set(), b(), a(), set()], 5, "a");
        assert_eq!((h, pos), (vec![View::Home, set(), b(), set()], 3));
        // Back from settings lands before the deleted session.
        let (h, pos) = without_session(vec![View::Home, a(), set()], 2, "a");
        assert_eq!((h, pos), (vec![View::Home, set()], 1));
        // No view twice in a row once it's gone.
        let (h, pos) = without_session(vec![set(), View::Subagent { session: "a".into(), tool: "t".into() }, set()], 2, "a");
        assert_eq!((h, pos), (vec![set()], 0));
    }
}
