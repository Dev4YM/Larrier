//! Persistent configuration: settings, agents, accounts, providers, workspaces.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::agent::{AgentKind, Protocol};
use crate::lsf::new_id;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Config {
    #[serde(default = "config_version")]
    pub version: u32,
    #[serde(default)]
    pub settings: Settings,
    #[serde(default)]
    pub agents: Vec<AgentConfig>,
    /// Every way a harness runs: sign-ins, API keys and endpoints,
    /// environments. Each belongs to one harness ([`Account::agent`]).
    #[serde(default)]
    pub accounts: Vec<Account>,
    /// Chat providers as Larrier wrote them before accounts held every
    /// credential: read once and turned into Larrier Chat accounts
    /// ([`Config::migrate`]), never written.
    #[serde(default, skip_serializing)]
    pub providers: Vec<Provider>,
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
    /// Home, the project of chats without a folder: its name, look,
    /// agents and where its new chats run, as a project with no folders
    /// (see [`Config::home_project`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_project: Option<Workspace>,
    #[serde(default)]
    pub custom_agents: Vec<CustomAgent>,
    #[serde(default)]
    pub automations: Vec<crate::automation::Automation>,
    #[serde(default)]
    pub profile: Profile,
}

/// The person using Larrier, as they describe themselves (Settings ->
/// Profile). Empty fields fall back (the name to git's `user.name`).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Profile {
    #[serde(default)]
    pub name: Option<String>,
    /// What they do ("Founder", "Backend engineer").
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub website: Option<String>,
    #[serde(default)]
    pub about: String,
    /// Their photo, kept in Larrier's data folder.
    #[serde(default)]
    pub photo: Option<String>,
}

fn config_version() -> u32 {
    1
}

/// The config format this build writes (see `Config::migrate`).
pub const CONFIG_VERSION: u32 = 4;

/// Agents whose own headless runners refuse whatever needs asking, while
/// over the Agent Client Protocol the asks reach the user: they connect
/// over ACP unless the user picks Native.
pub fn acp_by_default(kind: &AgentKind) -> bool {
    matches!(kind, AgentKind::OpenCode | AgentKind::Hermes)
}

impl Default for Config {
    fn default() -> Self {
        Config {
            version: CONFIG_VERSION,
            settings: Settings::default(),
            agents: Vec::new(),
            accounts: Vec::new(),
            providers: Vec::new(),
            workspaces: Vec::new(),
            home_project: None,
            custom_agents: Vec::new(),
            automations: Vec::new(),
            profile: Profile::default(),
        }
    }
}

impl Config {
    /// Bring a config written by an older Larrier up to date.
    pub fn migrate(&mut self) {
        if self.version < 2 {
            // 2: OpenCode and Hermes connect over ACP, where they can ask.
            for a in self.agents.iter_mut().filter(|a| acp_by_default(&a.kind)) {
                a.prefer_acp = true;
            }
        }
        if self.version < 3 {
            // 3: text comes in steps instead of a size in px for each kind.
            if let Some(px) = self.settings.legacy_font_size.take() {
                self.settings.text_size = TextSize::nearest(px);
            }
        }
        if self.version < 4 {
            // 4: every credential is an account of one harness.
            self.migrate_credentials();
        }
        self.version = self.version.max(CONFIG_VERSION);
    }

    /// Chat providers, agents' endpoints and key-in-a-variable accounts
    /// become API accounts. Ids and keychain entries stay as they were, so
    /// chats that named a provider or a model's endpoint keep working and no
    /// key has to be entered again.
    fn migrate_credentials(&mut self) {
        let defaults: Vec<String> = default_providers().into_iter().map(|p| p.id).collect();
        let mut chat_default = !self.accounts.iter().any(|a| a.agent == AgentKind::Chat);
        // Chat providers the user set up (the built-in list, untouched,
        // carries nothing to keep).
        for p in std::mem::take(&mut self.providers) {
            if !(p.enabled || p.secret_ref.is_some() || !defaults.contains(&p.id)) {
                continue;
            }
            let mut models = p.models.clone();
            if let Some(d) = p.default_model.as_ref().filter(|d| !models.contains(d)) {
                models.insert(0, d.clone());
            }
            let api = ApiAccount { api: p.kind.into(), base_url: Some(p.base_url.clone()), secret_ref: p.secret_ref.clone(), env_var: None, models };
            let mut acc = Account::new(AgentKind::Chat, p.name.clone(), AccountAuth::Api(api));
            acc.id = p.id.clone();
            acc.is_default = chat_default && p.enabled;
            chat_default &= !acc.is_default;
            self.accounts.push(acc);
        }
        // Each agent's endpoints; one that carried every model becomes the
        // agent's default account, which is what carries every model now.
        for i in 0..self.agents.len() {
            let kind = self.agents[i].kind.clone();
            let endpoints = std::mem::take(&mut self.agents[i].endpoints);
            // The agent ran on its own sign-in, which stays its default
            // unless an endpoint took every model.
            if !endpoints.is_empty() && !self.accounts.iter().any(|a| a.agent == kind) {
                let mut cur = Account::new(kind.clone(), "Current sign-in", AccountAuth::SystemLogin);
                cur.is_default = true;
                self.accounts.push(cur);
            }
            for e in endpoints {
                let api = ApiAccount { api: e.kind.into(), base_url: Some(e.base_url.clone()), secret_ref: e.secret_ref.clone(), env_var: None, models: Vec::new() };
                let mut acc = Account::new(kind.clone(), e.name.clone(), AccountAuth::Api(api));
                acc.id = e.id.clone();
                if e.enabled && e.use_for_all {
                    for a in self.accounts.iter_mut().filter(|a| a.agent == kind) {
                        a.is_default = false;
                    }
                    acc.is_default = true;
                }
                self.accounts.push(acc);
            }
        }
        // A key exported as one variable is an API account that says which.
        for a in self.accounts.iter_mut() {
            if let AccountAuth::ApiKey { env_var, secret_ref } = &a.auth {
                let api = ApiAccount { api: ApiKind::for_variable(env_var), base_url: None, secret_ref: Some(secret_ref.clone()), env_var: Some(env_var.clone()), models: Vec::new() };
                a.auth = AccountAuth::Api(api);
            }
        }
    }

    pub fn agent(&self, kind: &AgentKind) -> AgentConfig {
        self.agents.iter().find(|a| &a.kind == kind).cloned().unwrap_or_else(|| AgentConfig::new(kind.clone()))
    }

    pub fn agent_mut(&mut self, kind: &AgentKind) -> &mut AgentConfig {
        if let Some(i) = self.agents.iter().position(|a| &a.kind == kind) {
            &mut self.agents[i]
        } else {
            self.agents.push(AgentConfig::new(kind.clone()));
            self.agents.last_mut().unwrap()
        }
    }

    pub fn accounts_for(&self, kind: &AgentKind) -> Vec<&Account> {
        self.accounts.iter().filter(|a| &a.agent == kind).collect()
    }

    pub fn default_account(&self, kind: &AgentKind) -> Option<&Account> {
        let list = self.accounts_for(kind);
        list.iter().find(|a| a.is_default).copied().or_else(|| list.first().copied())
    }

    /// The account a chat runs as: the one it picked if it still exists
    /// for this harness, else the harness's default.
    pub fn account_for(&self, kind: &AgentKind, picked: Option<&str>) -> Option<&Account> {
        picked.and_then(|id| self.account(id)).filter(|a| &a.agent == kind).or_else(|| self.default_account(kind))
    }

    /// The API account a model of `kind` runs through: the account it was
    /// added with, else the chat's account when that is one. `None` runs
    /// it on the harness's own sign-in.
    pub fn api_for(&self, kind: &AgentKind, picked: Option<&str>, model: Option<&str>) -> Option<(&Account, &ApiAccount)> {
        let added = model.and_then(|m| self.agent(kind).custom_models.into_iter().find(|c| c.id == m)).and_then(|c| c.account);
        let acc = match added {
            Some(id) => self.account(&id).filter(|a| &a.agent == kind),
            None => self.account_for(kind, picked),
        }?;
        match &acc.auth {
            AccountAuth::Api(api) => Some((acc, api)),
            _ => None,
        }
    }

    pub fn account(&self, id: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.id == id)
    }

    pub fn workspace_by_path(&self, path: &str) -> Option<&Workspace> {
        let norm = normalize_path(path);
        self.workspaces.iter().find(|w| normalize_path(&w.path) == norm)
    }

    /// The project a folder belongs to: the one whose folder holds it most
    /// closely (a project inside another wins for its own folders).
    pub fn project_for(&self, path: &str) -> Option<&Workspace> {
        project_in(&self.workspaces, path)
    }

    /// Home as a project: as the user set it up, else named Home.
    pub fn home_project(&self) -> Workspace {
        self.home_project.clone().unwrap_or_else(|| {
            let mut w = Workspace::new("");
            w.id = HOME_PROJECT.to_string();
            w.name = "Home".to_string();
            w
        })
    }

    pub fn add_workspace(&mut self, path: &str) -> Workspace {
        if let Some(w) = self.workspace_by_path(path) {
            return w.clone();
        }
        let w = Workspace::new(path);
        self.workspaces.push(w.clone());
        w
    }

    pub fn custom_agent(&self, id: &str) -> Option<&CustomAgent> {
        self.custom_agents.iter().find(|c| c.id == id)
    }
}

/// Home's id among projects (see [`Config::home_project`]).
pub const HOME_PROJECT: &str = "home";

/// The project among `projects` that a folder belongs to (see
/// [`Config::project_for`]).
pub fn project_in<'a>(projects: &'a [Workspace], path: &str) -> Option<&'a Workspace> {
    projects.iter().filter_map(|w| w.holds(path).map(|n| (n, w))).max_by_key(|(n, _)| *n).map(|(_, w)| w)
}

pub fn normalize_path(p: &str) -> String {
    let mut s = p.replace('\\', "/");
    while s.ends_with('/') && s.len() > 1 {
        s.pop();
    }
    if cfg!(windows) {
        s = s.to_lowercase();
    }
    s
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Settings {
    #[serde(default)]
    pub theme: Theme,
    #[serde(default = "default_agent")]
    pub default_agent: AgentKind,
    #[serde(default)]
    pub default_mode: Option<String>,
    #[serde(default = "default_sidebar_width")]
    pub sidebar_width: u32,
    #[serde(default = "default_chat_width")]
    pub chat_width: u32,
    #[serde(default = "yes")]
    pub right_panel_open: bool,
    /// The session rail (shown while the side panel is closed) is folded
    /// down to its icons.
    #[serde(default)]
    pub session_rail_collapsed: bool,
    #[serde(default)]
    pub run_mode: RunMode,
    #[serde(default = "yes")]
    pub auto_import: bool,
    /// Watch the agents' session folders and import changes as they happen.
    #[serde(default = "yes")]
    pub live_sync: bool,
    #[serde(default)]
    pub use_worktrees: bool,
    #[serde(default)]
    pub worktrees_dir: Option<String>,
    #[serde(default)]
    pub send_with_ctrl_enter: bool,
    /// How big text is. Every kind of text (replies, prompts, steps, code,
    /// sidebar, terminal) and its line height follow it, in proportion.
    #[serde(default)]
    pub text_size: TextSize,
    /// Reply size in px from before text came in steps (see `Config::migrate`).
    #[serde(default, rename = "font_size", skip_serializing)]
    pub legacy_font_size: Option<u32>,
    #[serde(default = "yes")]
    pub show_reasoning: bool,
    #[serde(default)]
    pub sidebar_group: SidebarGroup,
    #[serde(default)]
    pub sidebar_sort: SidebarSort,
    /// Show archived agents in the sidebar.
    #[serde(default)]
    pub show_archived: bool,
    /// Projects in the order the user dragged them into (sidebar keys;
    /// `~home` is Home). Projects not listed come first, Home last.
    #[serde(default)]
    pub project_order: Vec<String>,
    /// Sidebar groups the user opened or closed (projects, and agents
    /// inside a project as `project|agent`); the rest follow the default.
    #[serde(default)]
    pub sidebar_opened: Vec<String>,
    #[serde(default)]
    pub sidebar_closed: Vec<String>,
    #[serde(default = "yes")]
    pub notify_on_finish: bool,
    /// Look for a newer release on GitHub (installed copies of Larrier).
    /// Off turns `updates` off whatever it says (older configs).
    #[serde(default = "yes")]
    pub auto_update: bool,
    /// What happens when a newer release is found.
    #[serde(default)]
    pub updates: UpdateMode,
    /// Which releases to update to.
    #[serde(default)]
    pub update_channel: UpdateChannel,
    /// The version that last ran, to say what's new after an update.
    #[serde(default)]
    pub last_version: Option<String>,

    // ---- appearance
    /// Accent color (`#rrggbb`); empty = theme default.
    #[serde(default)]
    pub accent: String,
    /// UI font family override.
    #[serde(default)]
    pub ui_font: Option<String>,
    /// Code / monospace font family override.
    #[serde(default)]
    pub mono_font: Option<String>,
    #[serde(default)]
    pub density: Density,
    #[serde(default)]
    pub button_corners: ButtonCorners,
    /// Put the agents sidebar on the right.
    #[serde(default)]
    pub sidebar_right: bool,
    /// Max width of the transcript column in px.
    #[serde(default = "default_transcript_width")]
    pub transcript_width: u32,
    #[serde(default)]
    pub reduce_motion: bool,
    /// Models starred in the model picker, shown in its Favorites tab in the
    /// order they were starred.
    #[serde(default)]
    pub favorite_models: Vec<FavoriteModel>,
    /// Per-theme color overrides: keys like `light.bg`, `dark.sidebar`.
    #[serde(default)]
    pub custom_colors: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub code_theme_light: Option<String>,
    #[serde(default)]
    pub code_theme_dark: Option<String>,
    /// Interface zoom (1.0 = 100%).
    #[serde(default = "default_zoom")]
    pub zoom: f64,

    // ---- system
    #[serde(default = "yes")]
    pub completion_sound: bool,
    #[serde(default = "yes")]
    pub tray_icon: bool,
    #[serde(default = "yes")]
    pub continue_interrupted: bool,
    #[serde(default = "yes")]
    pub show_tips: bool,
    #[serde(default = "yes")]
    pub restore_window: bool,
    /// What Enter does with a message while the agent is working (Ctrl+Enter
    /// does the other of steer and queue).
    #[serde(default)]
    pub busy_send: BusySend,
    /// Agents get Larrier's browser as tools (shown live in the Browser tab).
    #[serde(default = "yes")]
    pub browser_tools: bool,
}

/// What Larrier does about new releases on its own.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UpdateMode {
    /// Never looks.
    Off,
    /// Looks, and shows the update button when there is one.
    Notify,
    /// Also downloads it in the background; restarting installs it.
    #[default]
    Download,
    /// Also installs it when Larrier closes.
    Install,
}

impl UpdateMode {
    pub const ALL: [UpdateMode; 4] = [UpdateMode::Off, UpdateMode::Notify, UpdateMode::Download, UpdateMode::Install];

    pub fn label(self) -> &'static str {
        match self {
            UpdateMode::Off => "Don't check",
            UpdateMode::Notify => "Check and tell me",
            UpdateMode::Download => "Check and download",
            UpdateMode::Install => "Check, download and install",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            UpdateMode::Off => "Larrier never looks for new releases; check by hand below.",
            UpdateMode::Notify => "When a new release is out, the update button appears; downloading is up to you.",
            UpdateMode::Download => "New releases download in the background; Restart puts them in place.",
            UpdateMode::Install => "New releases download in the background and are installed when Larrier closes.",
        }
    }
}

/// A message sent while the agent is working.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BusySend {
    /// Into the running turn, where the agent takes it in.
    #[default]
    Steer,
    /// Waits, and goes when the turn ends.
    Queue,
    /// Stops the turn and goes right away.
    Interrupt,
}

impl BusySend {
    pub const ALL: [BusySend; 3] = [BusySend::Steer, BusySend::Queue, BusySend::Interrupt];

    pub fn label(self) -> &'static str {
        match self {
            BusySend::Steer => "Steer",
            BusySend::Queue => "Queue",
            BusySend::Interrupt => "Stop and send",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            BusySend::Steer => "The agent reads it while it works",
            BusySend::Queue => "Sends when the agent finishes",
            BusySend::Interrupt => "Stops the agent and sends it now",
        }
    }

    /// What the other send key does: steer and queue swap, and the opposite
    /// of stopping the agent is letting it finish. It never stops the agent.
    pub fn alternate(self) -> BusySend {
        match self {
            BusySend::Steer | BusySend::Interrupt => BusySend::Queue,
            BusySend::Queue => BusySend::Steer,
        }
    }
}

/// A model starred in the model picker: which harness runs it, and its id
/// as the picker stores it (Larrier Chat's as `account/model`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct FavoriteModel {
    pub agent: AgentKind,
    pub model: String,
}

impl Settings {
    pub fn is_favorite(&self, agent: &AgentKind, model: &str) -> bool {
        self.favorite_models.iter().any(|f| &f.agent == agent && f.model == model)
    }

    /// Star a model, or unstar it when it already is.
    pub fn toggle_favorite(&mut self, agent: &AgentKind, model: &str) {
        if self.is_favorite(agent, model) {
            self.favorite_models.retain(|f| !(&f.agent == agent && f.model == model));
        } else {
            self.favorite_models.push(FavoriteModel { agent: agent.clone(), model: model.to_string() });
        }
    }
}

/// Corners of buttons, pills, pickers and segmented controls.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ButtonCorners {
    /// Nearly square.
    Solid,
    #[default]
    Balanced,
    /// Fully round (pills and circles).
    Rounded,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Density {
    #[default]
    Comfortable,
    Compact,
}

fn default_transcript_width() -> u32 {
    860
}

fn default_zoom() -> f64 {
    1.0
}

fn yes() -> bool {
    true
}
fn default_agent() -> AgentKind {
    AgentKind::ClaudeCode
}
fn default_sidebar_width() -> u32 {
    300
}
fn default_chat_width() -> u32 {
    560
}
/// How big Larrier's text is. One step sets every kind of text together,
/// so sizes and line heights always fit each other.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TextSize {
    Small,
    #[default]
    Default,
    Large,
    Larger,
}

impl TextSize {
    pub const ALL: [TextSize; 4] = [TextSize::Small, TextSize::Default, TextSize::Large, TextSize::Larger];

    /// Size of the agent's replies, px; every other kind of text is set from it.
    pub fn px(self) -> u32 {
        match self {
            TextSize::Small => 13,
            TextSize::Default => 14,
            TextSize::Large => 15,
            TextSize::Larger => 16,
        }
    }

    /// The step nearest a reply size in px.
    pub fn nearest(px: u32) -> TextSize {
        Self::ALL.into_iter().min_by_key(|t| t.px().abs_diff(px)).unwrap_or_default()
    }

    pub fn label(self) -> &'static str {
        match self {
            TextSize::Small => "Small",
            TextSize::Default => "Default",
            TextSize::Large => "Large",
            TextSize::Larger => "Larger",
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: Theme::default(),
            default_agent: default_agent(),
            default_mode: None,
            sidebar_width: default_sidebar_width(),
            chat_width: default_chat_width(),
            right_panel_open: true,
            session_rail_collapsed: false,
            run_mode: RunMode::default(),
            auto_import: true,
            live_sync: true,
            use_worktrees: false,
            worktrees_dir: None,
            send_with_ctrl_enter: false,
            text_size: TextSize::default(),
            legacy_font_size: None,
            show_reasoning: true,
            sidebar_group: SidebarGroup::default(),
            sidebar_sort: SidebarSort::default(),
            show_archived: false,
            project_order: Vec::new(),
            sidebar_opened: Vec::new(),
            sidebar_closed: Vec::new(),
            notify_on_finish: true,
            auto_update: true,
            updates: UpdateMode::Download,
            update_channel: UpdateChannel::Stable,
            last_version: None,
            accent: String::new(),
            ui_font: None,
            mono_font: None,
            density: Density::default(),
            button_corners: ButtonCorners::default(),
            sidebar_right: false,
            transcript_width: default_transcript_width(),
            reduce_motion: false,
            favorite_models: Vec::new(),
            custom_colors: Default::default(),
            code_theme_light: None,
            code_theme_dark: None,
            zoom: 1.0,
            completion_sound: true,
            tray_icon: true,
            continue_interrupted: true,
            show_tips: true,
            restore_window: true,
            busy_send: BusySend::Steer,
            browser_tools: true,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

/// Which releases Larrier updates to.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UpdateChannel {
    /// Releases only.
    #[default]
    Stable,
    /// Previews (`-beta.N`, `-rc.N`) too, as soon as they're out.
    Preview,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SidebarGroup {
    /// Ready for Review / In Progress, then by workspace.
    #[default]
    Status,
    Workspace,
    Agent,
    /// By project, then by agent inside each project.
    Mixed,
}

/// Order of agents inside each sidebar group.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SidebarSort {
    #[default]
    Recent,
    Created,
    Name,
}

/// How much autonomy agents get when Larrier launches them.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    /// Agent's own defaults; permission prompts are surfaced in Larrier when the protocol supports it.
    Ask,
    /// File edits are auto-approved, commands still ask.
    #[default]
    AutoEdit,
    /// Everything auto-approved (sandbox where available).
    FullAuto,
}

impl RunMode {
    pub fn label(self) -> &'static str {
        match self {
            RunMode::Ask => "Ask every time",
            RunMode::AutoEdit => "Auto-approve edits",
            RunMode::FullAuto => "Full auto",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AgentConfig {
    pub kind: AgentKind,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Explicit executable path; otherwise looked up on PATH.
    #[serde(default)]
    pub binary: Option<String>,
    #[serde(default)]
    pub extra_args: Vec<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub default_mode: Option<String>,
    /// Default option of the agent's second mode group (permissions /
    /// autonomy), when it has one.
    #[serde(default)]
    pub default_permission: Option<String>,
    /// Prefer ACP even when a native stream protocol is available.
    #[serde(default)]
    pub prefer_acp: bool,
    /// Model ids added by hand before custom models existed; read as
    /// [`CustomModel`]s without an endpoint.
    #[serde(default)]
    pub extra_models: Vec<String>,
    /// Import this agent's native sessions automatically.
    #[serde(default = "yes")]
    pub import: bool,
    /// Reasoning effort for new sessions; `None` follows the agent's own.
    #[serde(default)]
    pub default_effort: Option<String>,
    /// Models the agent features that the user hid from Larrier's picker.
    #[serde(default)]
    pub hidden_models: Vec<String>,
    /// Models the agent offers but doesn't feature, shown in the picker.
    #[serde(default)]
    pub shown_models: Vec<String>,
    /// Models the user added for this agent.
    #[serde(default)]
    pub custom_models: Vec<CustomModel>,
    /// API endpoints as Larrier wrote them before they were accounts: read
    /// once and turned into this agent's API accounts, never written.
    #[serde(default, skip_serializing)]
    pub endpoints: Vec<AgentEndpoint>,
}

impl AgentConfig {
    pub fn new(kind: AgentKind) -> Self {
        let acp = acp_by_default(&kind);
        AgentConfig {
            kind,
            enabled: true,
            binary: None,
            extra_args: Vec::new(),
            env: Vec::new(),
            default_model: None,
            default_mode: None,
            default_permission: None,
            prefer_acp: acp,
            extra_models: Vec::new(),
            import: true,
            default_effort: None,
            hidden_models: Vec::new(),
            shown_models: Vec::new(),
            custom_models: Vec::new(),
            endpoints: Vec::new(),
        }
    }

    /// Every model the user added, the older `extra_models` included.
    pub fn added_models(&self) -> Vec<CustomModel> {
        let mut v = self.custom_models.clone();
        for id in &self.extra_models {
            if !v.iter().any(|m| &m.id == id) {
                v.push(CustomModel { id: id.clone(), name: None, account: None });
            }
        }
        v
    }
}

/// A model the user added to an agent: an id the agent accepts but doesn't
/// list, or a model served by one of the agent's API accounts.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CustomModel {
    /// What the agent is given (`--model <id>`).
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    /// The API account that serves it (written as `endpoint` before
    /// endpoints were accounts; the ids are the same); `None` is whatever
    /// the chat runs as.
    #[serde(default, alias = "endpoint")]
    pub account: Option<String>,
}

/// Before API accounts (config version 4): the user's own API endpoint for
/// an agent, read only to become an account. An OpenAI or Anthropic key, a
/// gateway, a local server). Its key lives in the OS keychain.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AgentEndpoint {
    pub id: String,
    pub name: String,
    pub kind: EndpointKind,
    pub base_url: String,
    #[serde(default)]
    pub secret_ref: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Run every model through this endpoint, instead of the agent's own
    /// service (Claude Code on your own Anthropic key).
    #[serde(default)]
    pub use_for_all: bool,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EndpointKind {
    /// Anthropic Messages API (Anthropic, or a gateway that speaks it).
    Anthropic,
    /// OpenAI Responses API.
    OpenAi,
    /// Any OpenAI-compatible `/chat/completions` endpoint.
    OpenAiCompatible,
}

impl EndpointKind {
    pub const ALL: [EndpointKind; 3] = [EndpointKind::Anthropic, EndpointKind::OpenAi, EndpointKind::OpenAiCompatible];

    pub fn label(self) -> &'static str {
        match self {
            EndpointKind::Anthropic => "Anthropic",
            EndpointKind::OpenAi => "OpenAI",
            EndpointKind::OpenAiCompatible => "OpenAI-compatible",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            EndpointKind::Anthropic => "anthropic",
            EndpointKind::OpenAi => "openai",
            EndpointKind::OpenAiCompatible => "openai_compatible",
        }
    }

    pub fn default_url(self) -> &'static str {
        match self {
            EndpointKind::Anthropic => "https://api.anthropic.com",
            EndpointKind::OpenAi => "https://api.openai.com/v1",
            EndpointKind::OpenAiCompatible => "https://openrouter.ai/api/v1",
        }
    }
}

/// An identity used to run an agent: a CLI login (isolated config dir),
/// an API key, or a set of environment variables.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Account {
    pub id: String,
    pub agent: AgentKind,
    pub label: String,
    pub auth: AccountAuth,
    #[serde(default)]
    pub is_default: bool,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub plan: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl Account {
    pub fn new(agent: AgentKind, label: impl Into<String>, auth: AccountAuth) -> Self {
        Account {
            id: new_id(),
            agent,
            label: label.into(),
            auth,
            is_default: false,
            email: None,
            plan: None,
            created_at: Utc::now(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AccountAuth {
    /// Whatever the harness is signed into on this computer (its own
    /// subscription or login).
    SystemLogin,
    /// A sign-in of its own, in a separate folder
    /// (e.g. `CLAUDE_CONFIG_DIR`, `CODEX_HOME`).
    ConfigDir { path: String },
    /// A provider's API, or any endpoint that speaks it, with a key in the
    /// OS keychain.
    Api(ApiAccount),
    /// An API key exported as one variable, as Larrier wrote it before API
    /// accounts: read, then turned into [`AccountAuth::Api`].
    ApiKey { env_var: String, secret_ref: String },
    /// Arbitrary environment variables (values may reference secrets as `secret:<ref>`).
    Env { vars: Vec<(String, String)> },
}

impl AccountAuth {
    pub fn label(&self) -> &'static str {
        match self {
            AccountAuth::SystemLogin => "Current sign-in",
            AccountAuth::ConfigDir { .. } => "Separate sign-in",
            AccountAuth::Api(_) | AccountAuth::ApiKey { .. } => "API",
            AccountAuth::Env { .. } => "Environment",
        }
    }

    /// A subscription or login (as opposed to an API key or variables).
    pub fn is_sign_in(&self) -> bool {
        matches!(self, AccountAuth::SystemLogin | AccountAuth::ConfigDir { .. })
    }

    /// The keychain entry it owns, if any.
    pub fn secret_ref(&self) -> Option<&str> {
        match self {
            AccountAuth::Api(a) => a.secret_ref.as_deref(),
            AccountAuth::ApiKey { secret_ref, .. } => Some(secret_ref),
            _ => None,
        }
    }
}

/// An API account: which API, where it is served, its key and, for
/// Larrier Chat, the models it offers.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ApiAccount {
    pub api: ApiKind,
    /// Where it's served; `None` is the provider's own address.
    #[serde(default)]
    pub base_url: Option<String>,
    /// Its key in the OS keychain (`None` for a local server without one).
    #[serde(default)]
    pub secret_ref: Option<String>,
    /// For a harness that reads its key from the environment: the variable
    /// the key goes in. `None`: Larrier points the harness at the API itself.
    #[serde(default)]
    pub env_var: Option<String>,
    /// The models it offers in Larrier Chat (agents list their own).
    #[serde(default)]
    pub models: Vec<String>,
}

impl ApiAccount {
    /// The address requests go to.
    pub fn url(&self) -> String {
        self.base_url.clone().filter(|u| !u.trim().is_empty()).unwrap_or_else(|| self.api.default_url().to_string())
    }
}

/// The API an account speaks.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ApiKind {
    Anthropic,
    OpenAi,
    Gemini,
    OpenRouter,
    Ollama,
    /// Any OpenAI-compatible `/chat/completions` endpoint (Groq, Together,
    /// DeepSeek, Mistral, LM Studio, vLLM, a gateway…).
    OpenAiCompatible,
    /// The harness maker's own API key (Cursor, Factory, Amp…).
    Service,
}

impl ApiKind {
    pub const ALL: [ApiKind; 7] =
        [ApiKind::Anthropic, ApiKind::OpenAi, ApiKind::Gemini, ApiKind::OpenRouter, ApiKind::Ollama, ApiKind::OpenAiCompatible, ApiKind::Service];

    pub fn label(self) -> &'static str {
        match self {
            ApiKind::Anthropic => "Anthropic",
            ApiKind::OpenAi => "OpenAI",
            ApiKind::Gemini => "Google Gemini",
            ApiKind::OpenRouter => "OpenRouter",
            ApiKind::Ollama => "Ollama",
            ApiKind::OpenAiCompatible => "OpenAI-compatible",
            ApiKind::Service => "Its own API key",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            ApiKind::Anthropic => "anthropic",
            ApiKind::OpenAi => "open_ai",
            ApiKind::Gemini => "gemini",
            ApiKind::OpenRouter => "open_router",
            ApiKind::Ollama => "ollama",
            ApiKind::OpenAiCompatible => "open_ai_compatible",
            ApiKind::Service => "service",
        }
    }

    pub fn from_id(id: &str) -> Option<ApiKind> {
        ApiKind::ALL.into_iter().find(|k| k.id() == id)
    }

    pub fn default_url(self) -> &'static str {
        match self {
            ApiKind::Anthropic => "https://api.anthropic.com",
            ApiKind::OpenAi => "https://api.openai.com/v1",
            ApiKind::Gemini => "https://generativelanguage.googleapis.com/v1beta",
            ApiKind::OpenRouter => "https://openrouter.ai/api/v1",
            ApiKind::Ollama => "http://localhost:11434/v1",
            ApiKind::OpenAiCompatible | ApiKind::Service => "",
        }
    }

    pub fn needs_key(self) -> bool {
        !matches!(self, ApiKind::Ollama)
    }

    /// Models to start a Larrier Chat account with; the user edits the list.
    pub fn suggested_models(self) -> &'static [&'static str] {
        match self {
            ApiKind::Anthropic => &["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"],
            ApiKind::OpenAi => &["gpt-5", "gpt-5-mini", "gpt-4.1", "o3"],
            ApiKind::Gemini => &["gemini-2.5-pro", "gemini-2.5-flash"],
            ApiKind::OpenRouter => &["anthropic/claude-sonnet-4.5", "openai/gpt-5", "x-ai/grok-4", "deepseek/deepseek-chat"],
            ApiKind::Ollama => &["llama3.1", "qwen2.5-coder"],
            ApiKind::OpenAiCompatible | ApiKind::Service => &[],
        }
    }

    /// The API a key variable belongs to (`ANTHROPIC_API_KEY` → Anthropic);
    /// the harness's own service for anything else.
    pub fn for_variable(var: &str) -> ApiKind {
        let v = var.to_ascii_uppercase();
        if v.contains("ANTHROPIC") {
            ApiKind::Anthropic
        } else if v.contains("OPENROUTER") {
            ApiKind::OpenRouter
        } else if v.contains("OPENAI") {
            ApiKind::OpenAi
        } else if v.contains("GEMINI") || v.contains("GOOGLE") {
            ApiKind::Gemini
        } else {
            ApiKind::Service
        }
    }
}

impl From<ProviderKind> for ApiKind {
    fn from(k: ProviderKind) -> Self {
        match k {
            ProviderKind::OpenAi => ApiKind::OpenAi,
            ProviderKind::Anthropic => ApiKind::Anthropic,
            ProviderKind::Gemini => ApiKind::Gemini,
            ProviderKind::OpenRouter => ApiKind::OpenRouter,
            ProviderKind::Ollama => ApiKind::Ollama,
            ProviderKind::OpenAiCompatible => ApiKind::OpenAiCompatible,
        }
    }
}

impl From<EndpointKind> for ApiKind {
    fn from(k: EndpointKind) -> Self {
        match k {
            EndpointKind::Anthropic => ApiKind::Anthropic,
            EndpointKind::OpenAi => ApiKind::OpenAi,
            EndpointKind::OpenAiCompatible => ApiKind::OpenAiCompatible,
        }
    }
}

/// Before API accounts (config version 4): a model API for the built-in
/// Chat agent, read only to become a Larrier Chat account.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: String,
    /// Secret reference in the OS keychain.
    #[serde(default)]
    pub secret_ref: Option<String>,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub default_model: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    OpenAi,
    Anthropic,
    Gemini,
    OpenRouter,
    Ollama,
    /// Any OpenAI-compatible `/chat/completions` endpoint.
    OpenAiCompatible,
}

impl ProviderKind {
    pub fn label(self) -> &'static str {
        match self {
            ProviderKind::OpenAi => "OpenAI",
            ProviderKind::Anthropic => "Anthropic",
            ProviderKind::Gemini => "Google Gemini",
            ProviderKind::OpenRouter => "OpenRouter",
            ProviderKind::Ollama => "Ollama",
            ProviderKind::OpenAiCompatible => "OpenAI-compatible",
        }
    }
    pub fn needs_key(self) -> bool {
        !matches!(self, ProviderKind::Ollama)
    }
}

pub fn default_providers() -> Vec<Provider> {
    let p = |id: &str, name: &str, kind: ProviderKind, url: &str, models: &[&str]| Provider {
        id: id.into(),
        name: name.into(),
        kind,
        base_url: url.into(),
        secret_ref: None,
        models: models.iter().map(|s| s.to_string()).collect(),
        enabled: false,
        default_model: models.first().map(|s| s.to_string()),
    };
    vec![
        p("anthropic", "Anthropic", ProviderKind::Anthropic, "https://api.anthropic.com", &["claude-opus-4-5", "claude-sonnet-4-5", "claude-haiku-4-5"]),
        p("openai", "OpenAI", ProviderKind::OpenAi, "https://api.openai.com/v1", &["gpt-5", "gpt-5-mini", "gpt-4.1", "o3"]),
        p("gemini", "Google Gemini", ProviderKind::Gemini, "https://generativelanguage.googleapis.com/v1beta", &["gemini-2.5-pro", "gemini-2.5-flash"]),
        p("openrouter", "OpenRouter", ProviderKind::OpenRouter, "https://openrouter.ai/api/v1", &["anthropic/claude-sonnet-4.5", "openai/gpt-5", "x-ai/grok-4", "deepseek/deepseek-chat", "nousresearch/hermes-4-405b"]),
        p("ollama", "Ollama", ProviderKind::Ollama, "http://localhost:11434/v1", &["llama3.1", "qwen2.5-coder"]),
    ]
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    /// Its main folder: where its chats start.
    pub path: String,
    pub added_at: DateTime<Utc>,
    /// Listed first in the sidebar.
    #[serde(default)]
    pub pinned: bool,
    /// More folders its agents may read and change (Claude Code's and
    /// Codex's `--add-dir`); chats started in one belong to the project.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folders: Vec<String>,
    /// A Lucide icon's name (a folder when unset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Its color, as `#rrggbb`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// The agents it works with; the first starts its new chats. Empty: any.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<AgentKind>,
    /// Its new chats run in a worktree of their own (`true`) or in the
    /// folder itself; unset follows Settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<bool>,
}

impl Workspace {
    pub fn new(path: &str) -> Self {
        let name = std::path::Path::new(path)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string());
        Workspace {
            id: new_id(),
            name,
            path: path.to_string(),
            added_at: Utc::now(),
            pinned: false,
            folders: Vec::new(),
            icon: None,
            color: None,
            agents: Vec::new(),
            worktree: None,
        }
    }

    /// Its folders, the main one first.
    pub fn all_folders(&self) -> impl Iterator<Item = &String> {
        std::iter::once(&self.path).chain(self.folders.iter())
    }

    /// How closely `path` belongs to it: the length of the folder of its
    /// that holds `path` (the folder itself or one inside it).
    fn holds(&self, path: &str) -> Option<usize> {
        let p = normalize_path(path);
        self.all_folders()
            .map(|f| normalize_path(f))
            .filter(|f| p == *f || (p.starts_with(f.as_str()) && p[f.len()..].starts_with('/')))
            .map(|f| f.len())
            .max()
    }
}

/// A user-defined agent launched from an arbitrary command.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CustomAgent {
    pub id: String,
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    pub protocol: Protocol,
    #[serde(default)]
    pub models: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_belongs_to_the_project_that_holds_it_most_closely() {
        let mut c = Config::default();
        let mut app = Workspace::new("D:/work/app");
        app.folders.push("D:/work/shared".into());
        let inner = Workspace::new("D:/work/app/tools");
        c.workspaces = vec![app.clone(), inner.clone()];
        assert_eq!(c.project_for("D:/work/app").map(|w| &w.id), Some(&app.id));
        assert_eq!(c.project_for("D:/work/app/src").map(|w| &w.id), Some(&app.id));
        assert_eq!(c.project_for("D:/work/shared").map(|w| &w.id), Some(&app.id));
        assert_eq!(c.project_for("D:/work/app/tools/x").map(|w| &w.id), Some(&inner.id));
        assert!(c.project_for("D:/work/apples").is_none());
        assert!(c.project_for("D:/work").is_none());
    }

    /// OpenCode and Hermes move to ACP once; a Native picked later stays.
    #[test]
    fn migrates_to_acp_once() {
        let mut old: Config = serde_json::from_str(r#"{"version": 1, "agents": [{"kind": "opencode", "prefer_acp": false}, {"kind": "claude-code", "prefer_acp": false}]}"#).unwrap();
        old.migrate();
        assert_eq!((old.version, old.agent(&AgentKind::OpenCode).prefer_acp, old.agent(&AgentKind::ClaudeCode).prefer_acp), (CONFIG_VERSION, true, false));
        old.agent_mut(&AgentKind::OpenCode).prefer_acp = false;
        let mut again: Config = serde_json::from_str(&serde_json::to_string(&old).unwrap()).unwrap();
        again.migrate();
        assert!(!again.agent(&AgentKind::OpenCode).prefer_acp, "the user's Native is kept");
        assert!(Config::default().agent(&AgentKind::Hermes).prefer_acp, "new configs start on ACP");
    }

    #[test]
    fn text_sizes_become_one_step() {
        let mut old: Config = serde_json::from_str(r#"{"version": 2, "settings": {"font_size": 15, "text_sizes": {"prompts": 14, "steps": 14}}}"#).unwrap();
        old.migrate();
        assert_eq!(old.settings.text_size, TextSize::Large);
        let saved = serde_json::to_string(&old).unwrap();
        assert!(!saved.contains("font_size") && !saved.contains("text_sizes"), "{saved}");
        let mut again: Config = serde_json::from_str(&saved).unwrap();
        again.migrate();
        assert_eq!(again.settings.text_size, TextSize::Large);
        assert_eq!([11, 13, 14, 15, 16, 22].map(|px| TextSize::nearest(px).px()), [13, 13, 14, 15, 16, 16]);
    }

    fn api(a: &Account) -> &ApiAccount {
        match &a.auth {
            AccountAuth::Api(x) => x,
            other => panic!("not an API account: {other:?}"),
        }
    }

    /// Providers the user set up become Larrier Chat accounts with the same
    /// ids and keys; the untouched built-in list leaves nothing behind.
    #[test]
    fn chat_providers_become_accounts() {
        let mut old: Config = serde_json::from_str(r#"{"version": 3, "providers": [
            {"id": "anthropic", "name": "Anthropic", "kind": "anthropic", "base_url": "https://api.anthropic.com", "secret_ref": "provider:anthropic", "models": ["claude-sonnet-4-5"], "enabled": true, "default_model": "claude-opus-4-5"},
            {"id": "openai", "name": "OpenAI", "kind": "open_ai", "base_url": "https://api.openai.com/v1", "models": ["gpt-5"], "enabled": false},
            {"id": "custom-1", "name": "Groq", "kind": "open_ai_compatible", "base_url": "https://api.groq.com/openai/v1", "models": [], "enabled": false}
        ]}"#).unwrap();
        old.migrate();
        let chat = old.accounts_for(&AgentKind::Chat);
        assert_eq!(chat.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["anthropic", "custom-1"]);
        let a = api(chat[0]);
        assert_eq!((a.api, a.secret_ref.as_deref(), a.models.clone()), (ApiKind::Anthropic, Some("provider:anthropic"), vec!["claude-opus-4-5".to_string(), "claude-sonnet-4-5".to_string()]));
        assert!(chat[0].is_default && !chat[1].is_default);
        assert_eq!(api(chat[1]).url(), "https://api.groq.com/openai/v1");
        let saved = serde_json::to_string(&old).unwrap();
        assert!(!saved.contains("\"providers\""), "{saved}");
        let mut again: Config = serde_json::from_str(&saved).unwrap();
        again.migrate();
        assert_eq!(again.accounts.len(), 2, "migrates once");
    }

    /// An agent's endpoints become its API accounts: one that carried every
    /// model becomes the default; otherwise the agent's own sign-in stays
    /// the default and only the models added with an endpoint use it.
    #[test]
    fn endpoints_become_accounts() {
        let mut old: Config = serde_json::from_str(r#"{"version": 3, "agents": [
            {"kind": "claude-code", "endpoints": [{"id": "e1", "name": "Gateway", "kind": "anthropic", "base_url": "https://gw.example.com", "secret_ref": "endpoint:e1", "use_for_all": true}]},
            {"kind": "codex", "custom_models": [{"id": "llama", "endpoint": "e2"}], "endpoints": [{"id": "e2", "name": "Local", "kind": "open_ai_compatible", "base_url": "http://localhost:8000/v1"}]}
        ], "accounts": [
            {"id": "a1", "agent": "claude-code", "label": "Work", "auth": {"type": "system_login"}, "is_default": true, "created_at": "2026-01-01T00:00:00Z"},
            {"id": "a2", "agent": "pi", "label": "Key", "auth": {"type": "api_key", "env_var": "OPENAI_API_KEY", "secret_ref": "account:a2"}, "created_at": "2026-01-01T00:00:00Z"}
        ]}"#).unwrap();
        old.migrate();
        let claude = AgentKind::ClaudeCode;
        assert_eq!(old.default_account(&claude).map(|a| a.id.as_str()), Some("e1"));
        assert_eq!(old.api_for(&claude, None, Some("sonnet")).map(|(a, _)| a.id.as_str()), Some("e1"));
        assert!(old.api_for(&claude, Some("a1"), None).is_none(), "a sign-in picked for the chat runs on it");
        let codex = AgentKind::Codex;
        assert!(matches!(old.default_account(&codex).map(|a| &a.auth), Some(AccountAuth::SystemLogin)));
        assert!(old.api_for(&codex, None, Some("gpt-5")).is_none());
        assert_eq!(old.api_for(&codex, None, Some("llama")).map(|(a, x)| (a.id.as_str(), x.url())), Some(("e2", "http://localhost:8000/v1".to_string())));
        let pi = old.account("a2").unwrap();
        assert_eq!((api(pi).api, api(pi).env_var.as_deref(), api(pi).secret_ref.as_deref()), (ApiKind::OpenAi, Some("OPENAI_API_KEY"), Some("account:a2")));
        let saved = serde_json::to_string(&old).unwrap();
        assert!(!saved.contains("\"endpoints\"") && !saved.contains("api_key") && saved.contains("\"account\":\"e2\""), "{saved}");
    }

    #[test]
    fn favorites_star_and_unstar() {
        let mut s = Settings::default();
        let claude = AgentKind::ClaudeCode;
        s.toggle_favorite(&claude, "claude-opus-5-5");
        s.toggle_favorite(&AgentKind::Chat, "anthropic/claude-sonnet-5-5");
        assert!(s.is_favorite(&claude, "claude-opus-5-5") && !s.is_favorite(&AgentKind::Codex, "claude-opus-5-5"));
        s.toggle_favorite(&claude, "claude-opus-5-5");
        assert_eq!(s.favorite_models, vec![FavoriteModel { agent: AgentKind::Chat, model: "anthropic/claude-sonnet-5-5".into() }]);
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.favorite_models, s.favorite_models);
    }

    #[test]
    fn variables_name_their_api() {
        assert_eq!(
            ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "OPENROUTER_API_KEY", "GEMINI_API_KEY", "CURSOR_API_KEY"].map(ApiKind::for_variable),
            [ApiKind::Anthropic, ApiKind::OpenAi, ApiKind::OpenRouter, ApiKind::Gemini, ApiKind::Service]
        );
    }
}
