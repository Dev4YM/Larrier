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
    #[serde(default)]
    pub accounts: Vec<Account>,
    #[serde(default = "default_providers")]
    pub providers: Vec<Provider>,
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
    #[serde(default)]
    pub custom_agents: Vec<CustomAgent>,
    #[serde(default)]
    pub automations: Vec<crate::automation::Automation>,
}

fn config_version() -> u32 {
    1
}

/// The config format this build writes (see `Config::migrate`).
pub const CONFIG_VERSION: u32 = 2;

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
            providers: default_providers(),
            workspaces: Vec::new(),
            custom_agents: Vec::new(),
            automations: Vec::new(),
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
        self.version = self.version.max(CONFIG_VERSION);
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

    pub fn account(&self, id: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.id == id)
    }

    pub fn provider(&self, id: &str) -> Option<&Provider> {
        self.providers.iter().find(|p| p.id == id)
    }

    pub fn workspace_by_path(&self, path: &str) -> Option<&Workspace> {
        let norm = normalize_path(path);
        self.workspaces.iter().find(|w| normalize_path(&w.path) == norm)
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
    /// Size of the agent's replies in the chat, px.
    #[serde(default = "default_font_size")]
    pub font_size: u32,
    /// Sizes of the other kinds of text, px.
    #[serde(default)]
    pub text_sizes: TextSizes,
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
fn default_font_size() -> u32 {
    14
}

/// Text sizes in px for each kind of text (replies use
/// [`Settings::font_size`]).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextSizes {
    /// The user's messages in the chat.
    #[serde(default = "size_14")]
    pub prompts: u32,
    /// What is typed in the composer.
    #[serde(default = "size_14")]
    pub composer: u32,
    /// Tool steps, commands and cards.
    #[serde(default = "size_13")]
    pub steps: u32,
    /// Code blocks, diffs and tool output.
    #[serde(default = "size_12")]
    pub code: u32,
    /// Chat and repository names in the sidebar.
    #[serde(default = "size_13")]
    pub sidebar: u32,
    /// The integrated terminal.
    #[serde(default = "size_13")]
    pub terminal: u32,
}

fn size_12() -> u32 {
    12
}
fn size_13() -> u32 {
    13
}
fn size_14() -> u32 {
    14
}

impl Default for TextSizes {
    fn default() -> Self {
        TextSizes { prompts: 14, composer: 14, steps: 13, code: 12, sidebar: 13, terminal: 13 }
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
            font_size: default_font_size(),
            text_sizes: TextSizes::default(),
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
    /// Ready for Review / In Progress, then by workspace (Cursor style).
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
    /// The user's own API endpoints and keys, for agents that can take one
    /// at launch.
    #[serde(default)]
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
                v.push(CustomModel { id: id.clone(), name: None, endpoint: None });
            }
        }
        v
    }

    pub fn endpoint(&self, id: &str) -> Option<&AgentEndpoint> {
        self.endpoints.iter().find(|e| e.id == id)
    }

    /// The endpoint a model runs through: the one it was added with, else
    /// an endpoint set to carry every model.
    pub fn endpoint_for(&self, model: Option<&str>) -> Option<&AgentEndpoint> {
        let added = model.and_then(|m| self.custom_models.iter().find(|c| c.id == m)).and_then(|c| c.endpoint.as_deref());
        match added {
            Some(id) => self.endpoint(id).filter(|e| e.enabled),
            None => self.endpoints.iter().find(|e| e.enabled && e.use_for_all),
        }
    }
}

/// A model the user added to an agent: an id the agent accepts but doesn't
/// list, or a model served by one of the agent's [`AgentEndpoint`]s.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CustomModel {
    /// What the agent is given (`--model <id>`).
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    /// Id of the endpoint that serves it; `None` is the agent's own service.
    #[serde(default)]
    pub endpoint: Option<String>,
}

/// The user's own API endpoint for an agent (an OpenAI or Anthropic key, a
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
    /// Uses whatever the agent CLI is logged into by default.
    SystemLogin,
    /// A separate config/home directory with its own login
    /// (e.g. `CLAUDE_CONFIG_DIR`, `CODEX_HOME`).
    ConfigDir { path: String },
    /// API key kept in the OS keychain under `secret_ref`, exported as `env_var`.
    ApiKey { env_var: String, secret_ref: String },
    /// Arbitrary environment variables (values may reference secrets as `secret:<ref>`).
    Env { vars: Vec<(String, String)> },
}

impl AccountAuth {
    pub fn label(&self) -> &'static str {
        match self {
            AccountAuth::SystemLogin => "System login",
            AccountAuth::ConfigDir { .. } => "Separate login",
            AccountAuth::ApiKey { .. } => "API key",
            AccountAuth::Env { .. } => "Environment",
        }
    }
}

/// A model API endpoint for the built-in Chat agent.
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
    pub path: String,
    pub added_at: DateTime<Utc>,
    #[serde(default)]
    pub pinned: bool,
}

impl Workspace {
    pub fn new(path: &str) -> Self {
        let name = std::path::Path::new(path)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string());
        Workspace { id: new_id(), name, path: path.to_string(), added_at: Utc::now(), pinned: false }
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
}
