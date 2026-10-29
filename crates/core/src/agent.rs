//! The agent catalog: every coding agent and chat source Larrier knows.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::config::RunMode;

/// Identifies an agent (or chat source). Serialized as a stable kebab-case id.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AgentKind {
    ClaudeCode,
    Codex,
    Cursor,
    OpenCode,
    Gemini,
    Pi,
    Hermes,
    Goose,
    Amp,
    Copilot,
    Qwen,
    Kimi,
    Droid,
    Aider,
    /// Larrier's built-in chat over a model provider API.
    Chat,
    /// Imported ChatGPT conversations.
    ChatGpt,
    /// Imported claude.ai conversations.
    ClaudeAi,
    /// A user-defined agent (ACP or stream-json command).
    Custom(String),
}

impl AgentKind {
    pub const BUILTIN: &'static [AgentKind] = &[
        AgentKind::ClaudeCode,
        AgentKind::Codex,
        AgentKind::Cursor,
        AgentKind::OpenCode,
        AgentKind::Gemini,
        AgentKind::Pi,
        AgentKind::Hermes,
        AgentKind::Goose,
        AgentKind::Amp,
        AgentKind::Copilot,
        AgentKind::Qwen,
        AgentKind::Kimi,
        AgentKind::Droid,
        AgentKind::Aider,
        AgentKind::Chat,
        AgentKind::ChatGpt,
        AgentKind::ClaudeAi,
    ];

    pub fn id(&self) -> String {
        match self {
            AgentKind::Custom(id) => format!("custom:{id}"),
            other => other.static_id().to_string(),
        }
    }

    fn static_id(&self) -> &'static str {
        match self {
            AgentKind::ClaudeCode => "claude-code",
            AgentKind::Codex => "codex",
            AgentKind::Cursor => "cursor",
            AgentKind::OpenCode => "opencode",
            AgentKind::Gemini => "gemini",
            AgentKind::Pi => "pi",
            AgentKind::Hermes => "hermes",
            AgentKind::Goose => "goose",
            AgentKind::Amp => "amp",
            AgentKind::Copilot => "copilot",
            AgentKind::Qwen => "qwen",
            AgentKind::Kimi => "kimi",
            AgentKind::Droid => "droid",
            AgentKind::Aider => "aider",
            AgentKind::Chat => "chat",
            AgentKind::ChatGpt => "chatgpt",
            AgentKind::ClaudeAi => "claude-ai",
            AgentKind::Custom(_) => "custom",
        }
    }

    pub fn from_id(id: &str) -> AgentKind {
        if let Some(rest) = id.strip_prefix("custom:") {
            return AgentKind::Custom(rest.to_string());
        }
        AgentKind::BUILTIN
            .iter()
            .find(|k| k.static_id() == id)
            .cloned()
            .unwrap_or_else(|| match id {
                "claude" | "claude_code" | "claudecode" => AgentKind::ClaudeCode,
                "cursor-agent" | "cursor_agent" => AgentKind::Cursor,
                "gemini-cli" => AgentKind::Gemini,
                "copilot-cli" | "github-copilot" => AgentKind::Copilot,
                "qwen-code" => AgentKind::Qwen,
                "factory" | "factory-droid" => AgentKind::Droid,
                other => AgentKind::Custom(other.to_string()),
            })
    }

    pub fn spec(&self) -> &'static AgentSpec {
        catalog().iter().find(|s| &s.kind == self).unwrap_or(&CUSTOM_SPEC)
    }

    pub fn display_name(&self) -> String {
        match self {
            AgentKind::Custom(id) => id.clone(),
            other => other.spec().name.to_string(),
        }
    }
}

impl fmt::Display for AgentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display_name())
    }
}

impl Serialize for AgentKind {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.id())
    }
}

impl<'de> Deserialize<'de> for AgentKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(AgentKind::from_id(&s))
    }
}

/// How Larrier talks to an agent at runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    /// `claude -p --output-format stream-json` style events.
    ClaudeStream,
    /// `codex exec --json` events.
    CodexExec,
    /// `cursor-agent -p --output-format stream-json` events.
    CursorStream,
    /// `gemini -p --output-format stream-json` events.
    GeminiStream,
    /// `opencode run --format json` events.
    OpenCodeRun,
    /// `pi --mode json` events.
    PiJson,
    /// `amp -x --stream-json` (Claude-shaped events).
    AmpStream,
    /// `qwen -p --output-format stream-json` (Claude-shaped events).
    QwenStream,
    /// `hermes chat --format stream-json`.
    HermesStream,
    /// `droid exec -o stream-json`.
    DroidStream,
    /// Agent Client Protocol over stdio (JSON-RPC).
    Acp,
    /// Plain text one-shot command; stdout is the answer.
    PlainText,
    /// Larrier's own HTTP chat client.
    HttpChat,
    /// Import only (no runtime).
    None,
}

#[derive(Debug)]
pub struct AgentSpec {
    pub kind: AgentKind,
    pub name: &'static str,
    pub vendor: &'static str,
    pub tagline: &'static str,
    /// Executable names to look for on PATH, in priority order.
    pub binaries: &'static [&'static str],
    /// Install commands shown in settings (label, command).
    pub install: &'static [(&'static str, &'static str)],
    pub homepage: &'static str,
    /// Accent color for the agent avatar.
    pub color: &'static str,
    /// One or two glyphs for the avatar.
    pub glyph: &'static str,
    pub protocol: Protocol,
    /// Arguments that start the agent in ACP mode, when supported.
    pub acp_command: Option<&'static [&'static str]>,
    pub models: &'static [&'static str],
    /// The agent's own modes and permission levels, as its CLI defines them
    /// (Claude Code: one "Mode" group; Codex: "Mode" + "Permissions"; …).
    /// Group 0 is stored in `Session.mode`, group 1 in `Session.permission`.
    pub mode_groups: &'static [ModeGroup],
    /// Environment variables that carry an API key for this agent.
    pub key_env: &'static [&'static str],
    /// Env var pointing the agent at an alternate config/home dir — used to
    /// isolate multiple logged-in accounts.
    pub home_env: Option<&'static str>,
    pub login_command: Option<&'static str>,
    pub can_run: bool,
    pub can_import: bool,
    /// Larrier can write a native session file this agent resumes.
    pub can_export_native: bool,
    pub session_location: &'static str,
}

/// One choice in a mode group, e.g. Claude Code's "Plan".
#[derive(Debug)]
pub struct ModeOption {
    /// Stored in the session. For ACP agents this is also the ACP mode id.
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    /// CLI arguments this option adds to a run.
    pub args: &'static [&'static str],
    /// Instruction put before the prompt, for modes the CLI has no
    /// non-interactive flag for (Codex plan mode).
    pub prompt_prefix: Option<&'static str>,
    /// Larrier's global autonomy levels (Settings → General) that pick this
    /// option when neither the session nor the agent settings chose one.
    pub autonomy: &'static [RunMode],
    /// Ids older Larrier versions stored for this option.
    pub aliases: &'static [&'static str],
}

/// A set of mutually exclusive options ("Mode", "Permissions", "Autonomy").
#[derive(Debug)]
pub struct ModeGroup {
    pub label: &'static str,
    pub options: &'static [ModeOption],
}

impl ModeGroup {
    /// The option with this id (or a legacy alias of it).
    pub fn find(&self, id: &str) -> Option<&'static ModeOption> {
        self.options.iter().find(|o| o.id == id || o.aliases.contains(&id))
    }

    /// The option used when nothing was picked, for a global autonomy level.
    pub fn default_for(&self, run_mode: RunMode) -> &'static ModeOption {
        self.options.iter().find(|o| o.autonomy.contains(&run_mode)).unwrap_or(&self.options[0])
    }
}

/// What a run gets from the chosen modes.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ModeSelection {
    /// Chosen option id per group, in group order.
    pub ids: Vec<String>,
    /// CLI arguments, permission groups first so a restricting mode
    /// (e.g. Codex plan → read-only) has the last word.
    pub args: Vec<String>,
    pub prompt_prefix: Option<String>,
}

impl AgentSpec {
    pub fn is_chat_source(&self) -> bool {
        matches!(self.kind, AgentKind::ChatGpt | AgentKind::ClaudeAi | AgentKind::Chat)
    }

    /// The option chosen in group `i`: the explicit pick (session or new
    /// agent draft), else the agent's default from settings, else the one
    /// the global autonomy level selects.
    pub fn mode_option(&self, i: usize, pick: Option<&str>, default: Option<&str>, run_mode: RunMode) -> Option<&'static ModeOption> {
        let g = self.mode_groups.get(i)?;
        pick.and_then(|p| g.find(p)).or_else(|| default.and_then(|d| g.find(d))).or_else(|| Some(g.default_for(run_mode)))
    }

    /// Resolve every group into ids, CLI arguments and a prompt prefix.
    pub fn resolve_modes(&self, picks: [Option<&str>; 2], defaults: [Option<&str>; 2], run_mode: RunMode) -> ModeSelection {
        let chosen: Vec<&'static ModeOption> = (0..self.mode_groups.len()).filter_map(|i| self.mode_option(i, picks.get(i).copied().flatten(), defaults.get(i).copied().flatten(), run_mode)).collect();
        let mut sel = ModeSelection { ids: chosen.iter().map(|o| o.id.to_string()).collect(), ..Default::default() };
        for o in chosen.iter().rev() {
            sel.args.extend(o.args.iter().map(|a| a.to_string()));
            if let Some(p) = o.prompt_prefix {
                sel.prompt_prefix = Some(p.to_string());
            }
        }
        sel
    }
}

const ASK: &[RunMode] = &[RunMode::Ask];
const EDIT: &[RunMode] = &[RunMode::AutoEdit];
const FULL: &[RunMode] = &[RunMode::FullAuto];
const ASK_EDIT: &[RunMode] = &[RunMode::Ask, RunMode::AutoEdit];
const NONE: &[RunMode] = &[];

const fn opt(id: &'static str, label: &'static str, description: &'static str, args: &'static [&'static str], autonomy: &'static [RunMode]) -> ModeOption {
    ModeOption { id, label, description, args, prompt_prefix: None, autonomy, aliases: &[] }
}

/// Claude Code `--permission-mode` (code.claude.com/docs/en/permission-modes;
/// `claude --help` 2.1.310: acceptEdits, auto, bypassPermissions, manual,
/// dontAsk, plan). Auto is the CLI's own default since v2.1.283.
static CLAUDE_MODES: &[ModeGroup] = &[ModeGroup {
    label: "Mode",
    options: &[
        ModeOption { aliases: &["agent"], ..opt("auto", "Auto", "Claude handles permission decisions", &["--permission-mode", "auto"], ASK_EDIT) },
        ModeOption { aliases: &["manual"], ..opt("default", "Manual", "Always ask before making changes", &["--permission-mode", "manual"], NONE) },
        ModeOption { aliases: &["accept-edits"], ..opt("acceptEdits", "Accept edits", "Automatically accept all file edits", &["--permission-mode", "acceptEdits"], NONE) },
        opt("plan", "Plan", "Create a plan before making changes", &["--permission-mode", "plan"], NONE),
        ModeOption { aliases: &["bypass"], ..opt("bypassPermissions", "Bypass permissions", "Accepts all permissions", &["--permission-mode", "bypassPermissions"], FULL) },
    ],
}];

/// Codex: `/plan` is interactive only (no `codex exec` flag), so Plan runs
/// read-only with a planning instruction. Permissions follow `/permissions`
/// (developers.openai.com/codex: sandbox_mode × approval_policy, Auto-review =
/// approvals_reviewer "auto_review").
static CODEX_MODES: &[ModeGroup] = &[
    ModeGroup {
        label: "Mode",
        options: &[
            ModeOption { aliases: &["agent"], ..opt("default", "Default", "Codex works on the task directly", &[], ASK_EDIT) },
            ModeOption {
                prompt_prefix: Some("Plan mode: explore and think, then propose a concrete step-by-step plan. Do not modify any files or run commands that change anything until the plan is approved."),
                ..opt("plan", "Plan", "Propose a plan before making changes (read-only)", &["-c", "sandbox_mode=\"read-only\""], NONE)
            },
        ],
    },
    ModeGroup {
        label: "Permissions",
        options: &[
            opt("read-only", "Read only", "Read files; no edits or commands that change anything", &["-c", "sandbox_mode=\"read-only\""], NONE),
            opt("auto", "Default", "Edit and run commands in the workspace; no network", &["-c", "sandbox_mode=\"workspace-write\""], ASK_EDIT),
            opt("auto-review", "Auto-review", "A reviewer approves risky actions automatically", &["-c", "sandbox_mode=\"workspace-write\"", "-c", "approval_policy=\"on-request\"", "-c", "approvals_reviewer=\"auto_review\""], NONE),
            opt("full-access", "Full access", "No sandbox and no approvals; only in an isolated machine", &["-c", "sandbox_mode=\"danger-full-access\"", "-c", "approval_policy=\"never\""], FULL),
        ],
    },
];

/// Cursor Agent 2026.10: `--mode plan|ask` (`--plan`), `--force`/`--yolo`,
/// `--auto-review`.
static CURSOR_MODES: &[ModeGroup] = &[
    ModeGroup {
        label: "Mode",
        options: &[
            opt("agent", "Agent", "Plan, search, edit and run commands", &[], ASK_EDIT),
            opt("plan", "Plan", "Read-only: analyze and propose a plan", &["--mode", "plan"], NONE),
            opt("ask", "Ask", "Read-only questions and explanations", &["--mode", "ask"], NONE),
        ],
    },
    ModeGroup {
        label: "Permissions",
        options: &[
            opt("default", "Default", "Edits files; commands need an allowlist", &[], ASK_EDIT),
            opt("auto-review", "Auto-review", "A classifier runs safe tool calls and stops the rest", &["--auto-review"], NONE),
            ModeOption { aliases: &["yolo"], ..opt("run-everything", "Run everything", "Allow every command unless explicitly denied", &["--force"], FULL) },
        ],
    },
];

/// OpenCode primary agents (`opencode run --agent build|plan`), `--auto`.
static OPENCODE_MODES: &[ModeGroup] = &[
    ModeGroup {
        label: "Agent",
        options: &[
            opt("build", "Build", "The default agent with all tools", &["--agent", "build"], ASK_EDIT),
            opt("plan", "Plan", "Analysis and planning; edits and commands ask first", &["--agent", "plan"], NONE),
        ],
    },
    ModeGroup {
        label: "Permissions",
        options: &[
            opt("ask", "Ask", "Use OpenCode's permission rules; prompts are refused", &[], ASK_EDIT),
            opt("auto", "Auto-approve", "Approve everything not explicitly denied", &["--auto"], FULL),
        ],
    },
];

/// Gemini CLI `--approval-mode default|auto_edit|yolo|plan` (geminicli.com).
static GEMINI_MODES: &[ModeGroup] = &[ModeGroup {
    label: "Approval mode",
    options: &[
        ModeOption { aliases: &["agent"], ..opt("default", "Default", "Ask before every tool call", &["--approval-mode", "default"], ASK) },
        ModeOption { aliases: &["auto-edit"], ..opt("auto_edit", "Auto edit", "Approve edits automatically; ask for the rest", &["--approval-mode", "auto_edit"], EDIT) },
        opt("plan", "Plan", "Read-only planning", &["--approval-mode", "plan"], NONE),
        opt("yolo", "YOLO", "Approve every action", &["--approval-mode", "yolo"], FULL),
    ],
}];

/// Qwen Code (Gemini CLI fork): `--approval-mode plan|default|auto-edit|yolo`.
static QWEN_MODES: &[ModeGroup] = &[ModeGroup {
    label: "Approval mode",
    options: &[
        ModeOption { aliases: &["agent"], ..opt("default", "Default", "Ask before every tool call", &["--approval-mode", "default"], ASK) },
        opt("auto-edit", "Auto edit", "Approve edits automatically; ask for the rest", &["--approval-mode", "auto-edit"], EDIT),
        opt("plan", "Plan", "Read-only planning", &["--approval-mode", "plan"], NONE),
        opt("yolo", "YOLO", "Approve every action", &["--approval-mode", "yolo"], FULL),
    ],
}];

/// Factory Droid 0.156 (`droid exec --help`): `--use-spec`, `--auto
/// low|medium|high`, `--skip-permissions-unsafe`; read-only without flags.
static DROID_MODES: &[ModeGroup] = &[
    ModeGroup {
        label: "Mode",
        options: &[
            ModeOption { aliases: &["agent"], ..opt("normal", "Normal", "Work on the task directly", &[], ASK_EDIT) },
            ModeOption { aliases: &["plan"], ..opt("spec", "Spec", "Write a specification before implementing", &["--use-spec"], NONE) },
        ],
    },
    ModeGroup {
        label: "Autonomy",
        options: &[
            opt("read-only", "Read only", "Reads and inspects; no modifications", &[], ASK),
            opt("low", "Low", "Basic file changes; no installs or system changes", &["--auto", "low"], NONE),
            opt("medium", "Medium", "Installs, builds and local git; no push", &["--auto", "medium"], EDIT),
            opt("high", "High", "Push, deploys and untrusted code", &["--auto", "high"], FULL),
            opt("skip-permissions", "Skip permissions", "Allow everything (unsafe)", &["--skip-permissions-unsafe"], NONE),
        ],
    },
];

/// Hermes: `--yolo` skips approval prompts.
static HERMES_MODES: &[ModeGroup] = &[ModeGroup {
    label: "Permissions",
    options: &[
        opt("ask", "Ask", "Use Hermes's approval rules", &[], ASK_EDIT),
        opt("yolo", "YOLO", "Skip approval prompts", &["--yolo"], FULL),
    ],
}];

/// Amp: `--mode smart|rush`, `--dangerously-allow-all`.
static AMP_MODES: &[ModeGroup] = &[
    ModeGroup {
        label: "Mode",
        options: &[
            opt("smart", "Smart", "The most capable models", &[], ASK_EDIT),
            opt("rush", "Rush", "Faster and cheaper for small tasks", &["--mode", "rush"], NONE),
        ],
    },
    ModeGroup {
        label: "Permissions",
        options: &[
            opt("ask", "Ask", "Use Amp's permission rules", &[], ASK_EDIT),
            opt("allow-all", "Allow all", "Run every tool without asking", &["--dangerously-allow-all"], FULL),
        ],
    },
];

/// Aider `--chat-mode`.
static AIDER_MODES: &[ModeGroup] = &[ModeGroup {
    label: "Chat mode",
    options: &[
        ModeOption { aliases: &["agent"], ..opt("code", "Code", "Edit the code", &[], ASK_EDIT) },
        opt("architect", "Architect", "Plan with one model, edit with another", &["--chat-mode", "architect"], NONE),
        opt("ask", "Ask", "Questions only; no edits", &["--chat-mode", "ask"], NONE),
    ],
}];

static CUSTOM_SPEC: AgentSpec = AgentSpec {
    kind: AgentKind::Custom(String::new()),
    name: "Custom Agent",
    vendor: "You",
    tagline: "Any ACP or stream-json compatible command",
    binaries: &[],
    install: &[],
    homepage: "https://agentclientprotocol.com",
    color: "#8a8a80",
    glyph: "◆",
    protocol: Protocol::Acp,
    acp_command: None,
    models: &[],
    mode_groups: &[],
    key_env: &[],
    home_env: None,
    login_command: None,
    can_run: true,
    can_import: false,
    can_export_native: false,
    session_location: "",
};

pub fn catalog() -> &'static [AgentSpec] {
    &CATALOG
}

/// Agents that can be launched from the composer.
pub fn runnable() -> impl Iterator<Item = &'static AgentSpec> {
    CATALOG.iter().filter(|s| s.can_run)
}

static CATALOG: [AgentSpec; 17] = [
    AgentSpec {
        kind: AgentKind::ClaudeCode,
        name: "Claude Code",
        vendor: "Anthropic",
        tagline: "Anthropic's agentic coding CLI",
        binaries: &["claude"],
        install: &[
            ("Windows", "irm https://claude.ai/install.ps1 | iex"),
            ("macOS / Linux", "curl -fsSL https://claude.ai/install.sh | bash"),
            ("npm", "npm install -g @anthropic-ai/claude-code"),
        ],
        homepage: "https://code.claude.com/docs",
        color: "#d97757",
        glyph: "✳",
        protocol: Protocol::ClaudeStream,
        acp_command: Some(&["npx", "-y", "@agentclientprotocol/claude-agent-acp"]),
        models: &["default", "opus", "sonnet", "haiku", "claude-opus-5-5", "claude-fable-5-1", "claude-sonnet-5", "claude-opus-4-8"],
        mode_groups: CLAUDE_MODES,
        key_env: &["ANTHROPIC_API_KEY"],
        home_env: Some("CLAUDE_CONFIG_DIR"),
        login_command: Some("claude /login"),
        can_run: true,
        can_import: true,
        can_export_native: true,
        session_location: "~/.claude/projects/<encoded-cwd>/<session>.jsonl",
    },
    AgentSpec {
        kind: AgentKind::Codex,
        name: "Codex",
        vendor: "OpenAI",
        tagline: "OpenAI's coding agent CLI",
        binaries: &["codex"],
        install: &[("npm", "npm install -g @openai/codex"), ("Homebrew", "brew install --cask codex")],
        homepage: "https://developers.openai.com/codex/cli",
        color: "#10a37f",
        glyph: "◎",
        protocol: Protocol::CodexExec,
        acp_command: Some(&["npx", "-y", "@agentclientprotocol/codex-acp"]),
        models: &["default", "gpt-6.1-sol", "gpt-6.1", "gpt-5-codex", "gpt-5"],
        mode_groups: CODEX_MODES,
        key_env: &["OPENAI_API_KEY", "CODEX_API_KEY"],
        home_env: Some("CODEX_HOME"),
        login_command: Some("codex login"),
        can_run: true,
        can_import: true,
        can_export_native: true,
        session_location: "~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl",
    },
    AgentSpec {
        kind: AgentKind::Cursor,
        name: "Cursor Agent",
        vendor: "Cursor",
        tagline: "Cursor's agent from the terminal",
        binaries: &["cursor-agent"],
        install: &[
            ("Windows", "irm 'https://cursor.com/install?win32=true' | iex"),
            ("macOS / Linux", "curl https://cursor.com/install -fsS | bash"),
        ],
        homepage: "https://cursor.com/docs/cli",
        color: "#26251e",
        glyph: "⬢",
        protocol: Protocol::CursorStream,
        acp_command: Some(&["cursor-agent", "acp"]),
        models: &["auto", "composer-2", "claude-opus-4-8", "gpt-5", "gemini-3-pro", "grok-4"],
        mode_groups: CURSOR_MODES,
        key_env: &["CURSOR_API_KEY"],
        home_env: Some("CURSOR_CONFIG_DIR"),
        login_command: Some("cursor-agent login"),
        can_run: true,
        can_import: true,
        can_export_native: false,
        session_location: "~/.cursor/projects/<workspace>/agent-transcripts · Cursor state.vscdb",
    },
    AgentSpec {
        kind: AgentKind::OpenCode,
        name: "OpenCode",
        vendor: "SST",
        tagline: "The open source coding agent",
        binaries: &["opencode"],
        install: &[
            ("npm", "npm install -g opencode-ai"),
            ("Scoop", "scoop install opencode"),
            ("Script", "curl -fsSL https://opencode.ai/install | bash"),
        ],
        homepage: "https://opencode.ai",
        color: "#4f46e5",
        glyph: "▣",
        protocol: Protocol::OpenCodeRun,
        acp_command: Some(&["opencode", "acp"]),
        models: &["default", "anthropic/claude-opus-5-5", "openai/gpt-6.1", "opencode/grok-code"],
        mode_groups: OPENCODE_MODES,
        key_env: &[],
        home_env: None,
        login_command: Some("opencode auth login"),
        can_run: true,
        can_import: true,
        can_export_native: false,
        session_location: "~/.local/share/opencode/opencode.db",
    },
    AgentSpec {
        kind: AgentKind::Gemini,
        name: "Gemini CLI",
        vendor: "Google",
        tagline: "Google's open source terminal agent",
        binaries: &["gemini"],
        install: &[("npm", "npm install -g @google/gemini-cli"), ("Homebrew", "brew install gemini-cli")],
        homepage: "https://github.com/google-gemini/gemini-cli",
        color: "#4285f4",
        glyph: "✦",
        protocol: Protocol::GeminiStream,
        acp_command: Some(&["gemini", "--acp"]),
        models: &["default", "gemini-3-pro-preview", "gemini-2.5-pro", "gemini-2.5-flash"],
        mode_groups: GEMINI_MODES,
        key_env: &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        home_env: Some("GEMINI_CLI_HOME"),
        login_command: Some("gemini"),
        can_run: true,
        can_import: true,
        can_export_native: true,
        session_location: "~/.gemini/tmp/<project>/chats/session-*.jsonl",
    },
    AgentSpec {
        kind: AgentKind::Pi,
        name: "Pi",
        vendor: "Earendil",
        tagline: "Minimal, extensible terminal coding agent",
        binaries: &["pi"],
        install: &[
            ("Windows", "irm https://pi.dev/install.ps1 | iex"),
            ("npm", "npm install -g --ignore-scripts @earendil-works/pi-coding-agent"),
        ],
        homepage: "https://github.com/earendil-works/pi",
        color: "#e5484d",
        glyph: "π",
        protocol: Protocol::PiJson,
        acp_command: Some(&["npx", "-y", "pi-acp"]),
        models: &["default", "anthropic/claude-opus-5-5", "openai/gpt-6.1", "google/gemini-3-pro-preview"],
        mode_groups: &[],
        key_env: &["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "GEMINI_API_KEY"],
        home_env: Some("PI_CODING_AGENT_DIR"),
        login_command: Some("pi /login"),
        can_run: true,
        can_import: true,
        can_export_native: true,
        session_location: "~/.pi/agent/sessions/--<cwd>--/*.jsonl",
    },
    AgentSpec {
        kind: AgentKind::Hermes,
        name: "Hermes Agent",
        vendor: "Nous Research",
        tagline: "Self-improving agent with memory and skills",
        binaries: &["hermes"],
        install: &[
            ("Windows", "iex (irm https://hermes-agent.nousresearch.com/install.ps1)"),
            ("macOS / Linux", "curl -fsSL https://hermes-agent.nousresearch.com/install.sh | bash"),
        ],
        homepage: "https://github.com/NousResearch/hermes-agent",
        color: "#c9a227",
        glyph: "☿",
        protocol: Protocol::HermesStream,
        acp_command: Some(&["hermes", "acp"]),
        models: &["default", "nousresearch/hermes-4-405b", "anthropic/claude-opus-5-5", "openai/gpt-6.1"],
        mode_groups: HERMES_MODES,
        key_env: &["OPENROUTER_API_KEY", "NOUS_API_KEY", "ANTHROPIC_API_KEY", "OPENAI_API_KEY"],
        home_env: Some("HERMES_HOME"),
        login_command: Some("hermes setup"),
        can_run: true,
        can_import: true,
        can_export_native: false,
        session_location: "$HERMES_HOME/state.db (%LOCALAPPDATA%\\hermes on Windows)",
    },
    AgentSpec {
        kind: AgentKind::Goose,
        name: "Goose",
        vendor: "Block",
        tagline: "Open source, extensible AI agent",
        binaries: &["goose"],
        install: &[("Script", "curl -fsSL https://github.com/block/goose/releases/download/stable/download_cli.sh | bash")],
        homepage: "https://block.github.io/goose",
        color: "#111111",
        glyph: "G",
        protocol: Protocol::Acp,
        acp_command: Some(&["goose", "acp"]),
        models: &["default"],
        mode_groups: &[],
        key_env: &[],
        home_env: None,
        login_command: Some("goose configure"),
        can_run: true,
        can_import: true,
        can_export_native: false,
        session_location: "~/.local/share/goose/sessions/sessions.db",
    },
    AgentSpec {
        kind: AgentKind::Amp,
        name: "Amp",
        vendor: "Sourcegraph",
        tagline: "Frontier coding agent",
        binaries: &["amp"],
        install: &[("npm", "npm install -g @sourcegraph/amp")],
        homepage: "https://ampcode.com",
        color: "#f34e3f",
        glyph: "A",
        protocol: Protocol::AmpStream,
        acp_command: None,
        models: &["default"],
        mode_groups: AMP_MODES,
        key_env: &["AMP_API_KEY"],
        home_env: None,
        login_command: Some("amp login"),
        can_run: true,
        can_import: false,
        can_export_native: false,
        session_location: "ampcode.com threads (server side)",
    },
    AgentSpec {
        kind: AgentKind::Copilot,
        name: "Copilot CLI",
        vendor: "GitHub",
        tagline: "GitHub Copilot in the terminal",
        binaries: &["copilot"],
        install: &[("npm", "npm install -g @github/copilot")],
        homepage: "https://github.com/github/copilot-cli",
        color: "#8957e5",
        glyph: "◉",
        protocol: Protocol::Acp,
        acp_command: Some(&["copilot", "--acp"]),
        models: &["default", "claude-opus-5-5", "gpt-6.1"],
        mode_groups: &[],
        key_env: &["GH_TOKEN", "GITHUB_TOKEN"],
        home_env: None,
        login_command: Some("copilot /login"),
        can_run: true,
        can_import: true,
        can_export_native: false,
        session_location: "~/.copilot/session-state/<id>/events.jsonl",
    },
    AgentSpec {
        kind: AgentKind::Qwen,
        name: "Qwen Code",
        vendor: "Alibaba",
        tagline: "Agentic coding for Qwen models",
        binaries: &["qwen"],
        install: &[("npm", "npm install -g @qwen-code/qwen-code")],
        homepage: "https://github.com/QwenLM/qwen-code",
        color: "#615ced",
        glyph: "Q",
        protocol: Protocol::QwenStream,
        acp_command: Some(&["qwen", "--acp"]),
        models: &["default", "qwen3-coder-plus"],
        mode_groups: QWEN_MODES,
        key_env: &["OPENAI_API_KEY", "DASHSCOPE_API_KEY"],
        home_env: None,
        login_command: Some("qwen"),
        can_run: true,
        can_import: true,
        can_export_native: false,
        session_location: "~/.qwen/projects/<cwd>/chats/<session>.jsonl",
    },
    AgentSpec {
        kind: AgentKind::Kimi,
        name: "Kimi CLI",
        vendor: "Moonshot AI",
        tagline: "Kimi's terminal agent",
        binaries: &["kimi"],
        install: &[("uv", "uv tool install kimi-cli")],
        homepage: "https://github.com/MoonshotAI/kimi-cli",
        color: "#1f1f1f",
        glyph: "K",
        protocol: Protocol::Acp,
        acp_command: Some(&["kimi", "acp"]),
        models: &["default", "kimi-k2"],
        mode_groups: &[],
        key_env: &["MOONSHOT_API_KEY"],
        home_env: None,
        login_command: Some("kimi"),
        can_run: true,
        can_import: false,
        can_export_native: false,
        session_location: "~/.kimi/sessions/<workdir-hash>/<id>",
    },
    AgentSpec {
        kind: AgentKind::Droid,
        name: "Droid",
        vendor: "Factory",
        tagline: "Factory's software development agent",
        binaries: &["droid"],
        install: &[("Windows", "irm https://app.factory.ai/cli/windows | iex"), ("macOS / Linux", "curl -fsSL https://app.factory.ai/cli | sh")],
        homepage: "https://factory.ai",
        color: "#ee6018",
        glyph: "D",
        protocol: Protocol::DroidStream,
        acp_command: Some(&["droid", "exec", "--output-format", "acp-daemon"]),
        models: &["default", "claude-opus-4-8", "claude-opus-5-5", "gpt-6.1"],
        mode_groups: DROID_MODES,
        key_env: &["FACTORY_API_KEY"],
        home_env: None,
        login_command: Some("droid"),
        can_run: true,
        can_import: true,
        can_export_native: false,
        session_location: "~/.factory/sessions/<cwd>/<id>.jsonl",
    },
    AgentSpec {
        kind: AgentKind::Aider,
        name: "Aider",
        vendor: "Aider",
        tagline: "AI pair programming in your terminal",
        binaries: &["aider"],
        install: &[("pip", "python -m pip install aider-install && aider-install")],
        homepage: "https://aider.chat",
        color: "#14b014",
        glyph: "ai",
        protocol: Protocol::PlainText,
        acp_command: None,
        models: &["default", "sonnet", "opus", "gpt-5", "deepseek"],
        mode_groups: AIDER_MODES,
        key_env: &["ANTHROPIC_API_KEY", "OPENAI_API_KEY"],
        home_env: None,
        login_command: None,
        can_run: true,
        can_import: true,
        can_export_native: false,
        session_location: "<repo>/.aider.chat.history.md",
    },
    AgentSpec {
        kind: AgentKind::Chat,
        name: "Chat",
        vendor: "Larrier",
        tagline: "Chat with any model through your own API keys",
        binaries: &[],
        install: &[],
        homepage: "https://github.com/Dev4YM/Larrier",
        color: "#3a6a9f",
        glyph: "✺",
        protocol: Protocol::HttpChat,
        acp_command: None,
        models: &[],
        mode_groups: &[],
        key_env: &[],
        home_env: None,
        login_command: None,
        can_run: true,
        can_import: false,
        can_export_native: false,
        session_location: "Larrier store",
    },
    AgentSpec {
        kind: AgentKind::ChatGpt,
        name: "ChatGPT",
        vendor: "OpenAI",
        tagline: "Imported ChatGPT conversations (conversations.json export)",
        binaries: &[],
        install: &[],
        homepage: "https://chatgpt.com",
        color: "#0f0f0f",
        glyph: "◍",
        protocol: Protocol::None,
        acp_command: None,
        models: &[],
        mode_groups: &[],
        key_env: &[],
        home_env: None,
        login_command: None,
        can_run: false,
        can_import: true,
        can_export_native: false,
        session_location: "Settings → Data controls → Export",
    },
    AgentSpec {
        kind: AgentKind::ClaudeAi,
        name: "Claude.ai",
        vendor: "Anthropic",
        tagline: "Imported claude.ai conversations (data export)",
        binaries: &[],
        install: &[],
        homepage: "https://claude.ai",
        color: "#c96442",
        glyph: "✳",
        protocol: Protocol::None,
        acp_command: None,
        models: &[],
        mode_groups: &[],
        key_env: &[],
        home_env: None,
        login_command: None,
        can_run: false,
        can_import: true,
        can_export_native: false,
        session_location: "Settings → Privacy → Export data",
    },
];

#[cfg(test)]
mod tests {
    use crate::config::RunMode;

    fn sel(kind: AgentKind, picks: [Option<&str>; 2], run_mode: RunMode) -> super::ModeSelection {
        kind.spec().resolve_modes(picks, [None, None], run_mode)
    }

    #[test]
    fn claude_code_permission_modes() {
        // Auto is Claude Code's own default; Full Auto maps to bypass.
        assert_eq!(sel(AgentKind::ClaudeCode, [None, None], RunMode::AutoEdit).args, ["--permission-mode", "auto"]);
        assert_eq!(sel(AgentKind::ClaudeCode, [None, None], RunMode::FullAuto).args, ["--permission-mode", "bypassPermissions"]);
        assert_eq!(sel(AgentKind::ClaudeCode, [Some("plan"), None], RunMode::FullAuto).args, ["--permission-mode", "plan"]);
        assert_eq!(sel(AgentKind::ClaudeCode, [Some("default"), None], RunMode::AutoEdit).args, ["--permission-mode", "manual"]);
        assert_eq!(sel(AgentKind::ClaudeCode, [Some("acceptEdits"), None], RunMode::Ask).args, ["--permission-mode", "acceptEdits"]);
        // Ids saved by older versions still resolve.
        assert_eq!(sel(AgentKind::ClaudeCode, [Some("bypass"), None], RunMode::Ask).ids, ["bypassPermissions"]);
        assert_eq!(sel(AgentKind::ClaudeCode, [Some("agent"), None], RunMode::Ask).ids, ["auto"]);
        assert_eq!(AgentKind::ClaudeCode.spec().mode_groups.len(), 1);
    }

    #[test]
    fn codex_mode_and_permissions() {
        let s = sel(AgentKind::Codex, [None, None], RunMode::AutoEdit);
        assert_eq!(s.ids, ["default", "auto"]);
        assert_eq!(s.args, ["-c", "sandbox_mode=\"workspace-write\""]);
        let full = sel(AgentKind::Codex, [None, None], RunMode::FullAuto);
        assert_eq!(full.args, ["-c", "sandbox_mode=\"danger-full-access\"", "-c", "approval_policy=\"never\""]);
        // Plan runs read-only even with full access picked: its override comes last.
        let plan = sel(AgentKind::Codex, [Some("plan"), Some("full-access")], RunMode::AutoEdit);
        assert_eq!(plan.args.last().map(String::as_str), Some("sandbox_mode=\"read-only\""));
        assert!(plan.prompt_prefix.is_some());
        let review = sel(AgentKind::Codex, [None, Some("auto-review")], RunMode::Ask);
        assert!(review.args.contains(&"approvals_reviewer=\"auto_review\"".to_string()));
        // The old "read-only" mode id lives on in the permissions group.
        assert_eq!(sel(AgentKind::Codex, [None, Some("read-only")], RunMode::Ask).args, ["-c", "sandbox_mode=\"read-only\""]);
    }

    #[test]
    fn cursor_gemini_opencode_droid() {
        assert_eq!(sel(AgentKind::Cursor, [Some("plan"), None], RunMode::AutoEdit).args, ["--mode", "plan"]);
        assert_eq!(sel(AgentKind::Cursor, [None, None], RunMode::FullAuto).args, ["--force"]);
        assert_eq!(sel(AgentKind::Cursor, [Some("ask"), Some("auto-review")], RunMode::Ask).args, ["--auto-review", "--mode", "ask"]);
        assert_eq!(sel(AgentKind::Gemini, [None, None], RunMode::AutoEdit).args, ["--approval-mode", "auto_edit"]);
        assert_eq!(sel(AgentKind::Gemini, [None, None], RunMode::Ask).args, ["--approval-mode", "default"]);
        assert_eq!(sel(AgentKind::Gemini, [Some("auto-edit"), None], RunMode::Ask).args, ["--approval-mode", "auto_edit"]);
        assert_eq!(sel(AgentKind::OpenCode, [Some("plan"), None], RunMode::FullAuto).args, ["--auto", "--agent", "plan"]);
        assert!(sel(AgentKind::Droid, [None, None], RunMode::Ask).args.is_empty(), "read-only without flags");
        assert_eq!(sel(AgentKind::Droid, [None, None], RunMode::AutoEdit).args, ["--auto", "medium"]);
        assert_eq!(sel(AgentKind::Droid, [Some("spec"), None], RunMode::FullAuto).args, ["--auto", "high", "--use-spec"]);
        assert_eq!(sel(AgentKind::Hermes, [None, None], RunMode::FullAuto).args, ["--yolo"]);
        assert!(sel(AgentKind::Pi, [None, None], RunMode::FullAuto).ids.is_empty());
    }

    #[test]
    fn every_group_has_unique_ids() {
        for spec in super::catalog() {
            for g in spec.mode_groups {
                assert!(!g.options.is_empty(), "{} {}", spec.name, g.label);
                let mut ids: Vec<&str> = g.options.iter().flat_map(|o| std::iter::once(o.id).chain(o.aliases.iter().copied())).collect();
                let n = ids.len();
                ids.sort();
                ids.dedup();
                assert_eq!(ids.len(), n, "{} {}: duplicate ids", spec.name, g.label);
            }
            assert!(spec.mode_groups.len() <= 2, "{}: sessions store two groups", spec.name);
        }
    }

    use super::*;

    #[test]
    fn ids_roundtrip() {
        for k in AgentKind::BUILTIN {
            assert_eq!(&AgentKind::from_id(&k.id()), k);
            assert_eq!(&k.spec().kind, k);
        }
        let c = AgentKind::Custom("my-agent".into());
        assert_eq!(AgentKind::from_id(&c.id()), c);
        assert_eq!(serde_json::to_string(&AgentKind::ClaudeCode).unwrap(), "\"claude-code\"");
    }
}
