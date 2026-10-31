//! LSF — the Larrier Session Format.
//!
//! One portable representation for every agent conversation and AI chat.
//! Sessions are stored as JSON Lines (`*.lsf.jsonl`): the first line is the
//! session header, every following line is one message. The same data can be
//! written as a single pretty JSON document (`*.lsf.json`) for sharing.
//!
//! The format is lossless for the things every agent has in common (text,
//! reasoning, tool calls with results, file edits, images, plans, usage) and
//! keeps per-agent native ids so a session can be resumed natively by the
//! agent that created it, or ported to any other agent.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::agent::AgentKind;

pub const LSF_VERSION: &str = "1.0";
pub const LSF_EXT_JSONL: &str = "lsf.jsonl";
pub const LSF_EXT_JSON: &str = "lsf.json";

pub fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub status: SessionStatus,
    /// The agent currently driving this session.
    pub agent: AgentKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The agent's mode: an option id of its first mode group, e.g. Claude
    /// Code `plan`, Codex `plan`, Cursor `ask` (see `AgentSpec::mode_groups`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// An option id of the agent's second mode group (Codex / Cursor
    /// permissions, Droid autonomy).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission: Option<String>,
    /// Reasoning effort picked for this session (`low` … `max`); `None`
    /// follows the agent's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    /// One of the agent's own named agents the chat runs as (Claude Code's
    /// `--agent`), rather than the agent's default self.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_as: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceRef>,
    #[serde(default)]
    pub environment: Environment,
    pub origin: Origin,
    /// Native session handles per agent, used to resume natively.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub native: Vec<NativeLink>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<ForkRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub unread: bool,
    /// An imported session whose agent is still mid-turn in its own app
    /// (a terminal CLI, an IDE): the transcript has no end-of-turn marker yet.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub external_turn: bool,
    #[serde(default)]
    pub usage: Usage,
    #[serde(default)]
    pub messages: Vec<Message>,
    /// Anything an importer wants to keep that has no first-class field.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub extra: Map<String, Value>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    #[default]
    Idle,
    Running,
    /// Waiting on the user (permission request, question).
    NeedsInput,
    /// Finished a turn with changes to review.
    Review,
    Done,
    Error,
}

impl SessionStatus {
    pub fn label(self) -> &'static str {
        match self {
            SessionStatus::Idle => "Idle",
            SessionStatus::Running => "In Progress",
            SessionStatus::NeedsInput => "Needs Input",
            SessionStatus::Review => "Ready for Review",
            SessionStatus::Done => "Done",
            SessionStatus::Error => "Error",
        }
    }
    pub fn is_active(self) -> bool {
        matches!(self, SessionStatus::Running | SessionStatus::NeedsInput)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct WorkspaceRef {
    pub path: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
}

impl WorkspaceRef {
    pub fn from_path(path: impl Into<String>) -> Self {
        let path = path.into();
        let name = std::path::Path::new(&path)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone());
        WorkspaceRef { path, name, branch: None, remote: None }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Environment {
    #[default]
    Local,
    /// Isolated git worktree created for this session.
    Worktree {
        path: String,
        branch: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base: Option<String>,
    },
    /// Remote machine over SSH.
    Ssh { host: String, path: String },
}

impl Environment {
    pub fn label(&self) -> &'static str {
        match self {
            Environment::Local => "Local",
            Environment::Worktree { .. } => "Worktree",
            Environment::Ssh { .. } => "SSH",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Origin {
    pub agent: AgentKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_version: Option<String>,
}

impl Origin {
    pub fn larrier(agent: AgentKind) -> Self {
        Origin { agent, native_id: None, path: None, imported_at: None, agent_version: None }
    }
}

/// A handle to the same conversation inside an agent's own storage.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NativeLink {
    pub agent: AgentKind,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// How many LSF messages the native session already contains. Messages
    /// after this index were produced elsewhere and must be handed over.
    #[serde(default)]
    pub synced_messages: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    /// Resume the native session only up to this entry on the next turn
    /// (an earlier prompt was edited: what came after it is dropped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_at: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ForkRef {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_write_tokens: u64,
    #[serde(default)]
    pub reasoning_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// Tokens in the model's context at the latest call (prompt, cached
    /// prompt and cache writes), when the agent reports calls one by one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<u64>,
    /// The model's context window, when the agent reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
}

impl Usage {
    /// Add `other`'s tokens and cost; its context figures replace ours (they
    /// describe the latest call, not a sum).
    pub fn add(&mut self, other: &Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
        self.cache_write_tokens += other.cache_write_tokens;
        self.reasoning_tokens += other.reasoning_tokens;
        if let Some(c) = other.cost_usd {
            self.cost_usd = Some(self.cost_usd.unwrap_or(0.0) + c);
        }
        if other.context_tokens.is_some() {
            self.context_tokens = other.context_tokens;
        }
        if other.context_window.is_some() {
            self.context_window = other.context_window;
        }
    }
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.cache_read_tokens + self.cache_write_tokens
    }
    pub fn is_empty(&self) -> bool {
        self.total_tokens() == 0 && self.cost_usd.is_none()
    }
}

// ---------------------------------------------------------------------------
// Messages and parts
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
    System,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Message {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub role: Role,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default)]
    pub parts: Vec<Part>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Produced by a subagent / side chain rather than the main thread.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sidechain: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_id: Option<String>,
}

impl Message {
    pub fn new(role: Role, parts: Vec<Part>) -> Self {
        Message {
            id: new_id(),
            parent_id: None,
            role,
            created_at: Utc::now(),
            agent: None,
            model: None,
            parts,
            usage: None,
            sidechain: false,
            native_id: None,
        }
    }

    pub fn user_text(text: impl Into<String>) -> Self {
        Message::new(Role::User, vec![Part::Text { text: text.into() }])
    }

    pub fn assistant(agent: AgentKind, model: Option<String>) -> Self {
        let mut m = Message::new(Role::Assistant, Vec::new());
        m.agent = Some(agent);
        m.model = model;
        m
    }

    /// Concatenated visible text of the message.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for p in &self.parts {
            if let Part::Text { text } = p {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(text);
            }
        }
        out
    }

    pub fn tool_calls(&self) -> impl Iterator<Item = &ToolCall> {
        self.parts.iter().filter_map(|p| match p {
            Part::Tool(t) => Some(t),
            _ => None,
        })
    }

    pub fn tool_calls_mut(&mut self) -> impl Iterator<Item = &mut ToolCall> {
        self.parts.iter_mut().filter_map(|p| match p {
            Part::Tool(t) => Some(t),
            _ => None,
        })
    }

    pub fn find_tool_mut(&mut self, id: &str) -> Option<&mut ToolCall> {
        self.tool_calls_mut().find(|t| t.id == id)
    }

    /// Append streamed text, merging with a trailing text part.
    pub fn push_text(&mut self, delta: &str) {
        if let Some(Part::Text { text }) = self.parts.last_mut() {
            text.push_str(delta);
        } else {
            self.parts.push(Part::Text { text: delta.to_string() });
        }
    }

    /// Append streamed reasoning, merging with a trailing reasoning part.
    pub fn push_reasoning(&mut self, delta: &str) {
        if let Some(Part::Reasoning { text, .. }) = self.parts.last_mut() {
            text.push_str(delta);
        } else {
            self.parts.push(Part::Reasoning { text: delta.to_string(), duration_ms: None });
        }
    }

    pub fn is_empty(&self) -> bool {
        self.parts.iter().all(|p| match p {
            Part::Text { text } | Part::Reasoning { text, .. } => text.trim().is_empty(),
            _ => false,
        })
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Part {
    Text {
        text: String,
    },
    Reasoning {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
    },
    Tool(ToolCall),
    Image {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime: Option<String>,
        source: MediaSource,
    },
    /// A file attached as context.
    File {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    Plan {
        entries: Vec<PlanEntry>,
    },
    Error {
        message: String,
    },
    /// System notice rendered inline (ports, interruptions, mode switches).
    Notice {
        text: String,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MediaSource {
    Base64 { data: String },
    Path { path: String },
    Url { url: String },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlanEntry {
    pub content: String,
    #[serde(default)]
    pub status: PlanStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ToolCall {
    pub id: String,
    /// Native tool name, e.g. `Read`, `shell`, `apply_patch`, `edit_file`.
    pub name: String,
    #[serde(default)]
    pub kind: ToolKind,
    /// Human summary such as `Read src/main.rs`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub input: Value,
    #[serde(default)]
    pub status: ToolStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_error: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locations: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diffs: Vec<FileDiff>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// When the call started (set by the runtime, or from the transcript
    /// timestamp on import).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    /// Nested transcript of a subagent (Task / Agent / spawn_agent) call.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subagent: Vec<Message>,
    /// Long-running background command or task (`run_in_background`, async
    /// subagents, monitors). Stays `Running` until the agent reports it done.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub background: bool,
    /// Subagent type, e.g. `Explore`, `general-purpose`, a Codex task name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    /// Images the tool returned (screenshots, image reads, MCP image
    /// content), as `Part::Image` parts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<Part>,
}

impl ToolCall {
    pub fn new(id: impl Into<String>, name: impl Into<String>, input: Value) -> Self {
        let name = name.into();
        let kind = ToolKind::classify(&name);
        let background = input.get("run_in_background").and_then(|v| v.as_bool()).unwrap_or(false)
            || name == "Monitor";
        let agent_name = if kind == ToolKind::Task {
            ["subagent_type", "agent_type", "task_name", "agent"]
                .iter()
                .find_map(|k| input.get(*k).and_then(|v| v.as_str()))
                .filter(|v| !v.trim().is_empty())
                .map(|v| v.to_string())
        } else {
            None
        };
        let mut t = ToolCall {
            id: id.into(),
            name,
            kind,
            title: None,
            input,
            status: ToolStatus::Running,
            output: None,
            is_error: false,
            locations: Vec::new(),
            diffs: Vec::new(),
            duration_ms: None,
            started_at: None,
            subagent: Vec::new(),
            background,
            agent_name,
            images: Vec::new(),
        };
        t.title = Some(t.describe());
        t
    }

    /// A subagent call (Task / Agent / spawn_agent) or one with a nested transcript.
    pub fn is_subagent(&self) -> bool {
        self.kind == ToolKind::Task || !self.subagent.is_empty()
    }

    /// Still working (running, pending or waiting for approval).
    pub fn is_active(&self) -> bool {
        matches!(self.status, ToolStatus::Running | ToolStatus::Pending | ToolStatus::Approval)
    }

    /// Record the run time from `started_at` (when not already known).
    pub fn stamp_duration(&mut self) {
        if let (None, Some(start)) = (self.duration_ms, self.started_at) {
            self.duration_ms = Some((Utc::now() - start).num_milliseconds().max(0) as u64);
        }
    }

    /// Number of tool steps inside the nested transcript (recursive).
    pub fn subagent_steps(&self) -> usize {
        self.subagent.iter().flat_map(|m| m.tool_calls()).map(|t| 1 + t.subagent_steps()).sum()
    }

    /// Best-effort one-line description from the native input.
    pub fn describe(&self) -> String {
        let s = |k: &str| self.input.get(k).and_then(|v| v.as_str()).map(|v| v.to_string());
        let path = s("file_path")
            .or_else(|| s("path"))
            .or_else(|| s("filePath"))
            .or_else(|| s("target_file"))
            .or_else(|| s("notebook_path"))
            .map(|p| short_path(&p));
        match self.kind {
            ToolKind::Read => format!("Read {}", path.unwrap_or_default()),
            ToolKind::Edit => format!("Edit {}", path.unwrap_or_default()),
            ToolKind::Write => format!("Write {}", path.unwrap_or_default()),
            ToolKind::Delete => format!("Delete {}", path.unwrap_or_default()),
            ToolKind::Move => format!("Move {}", path.unwrap_or_default()),
            ToolKind::Shell => {
                let cmd = s("command")
                    .or_else(|| s("cmd"))
                    .or_else(|| {
                        self.input.get("command").and_then(|v| v.as_array()).map(|a| {
                            a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(" ")
                        })
                    })
                    .unwrap_or_default();
                format!("Ran {}", first_line(&cmd))
            }
            ToolKind::Search => {
                let q = s("pattern").or_else(|| s("query")).or_else(|| s("glob")).unwrap_or_default();
                format!("Searched {}", q)
            }
            ToolKind::WebSearch => format!("Searched web for {}", s("query").unwrap_or_default()),
            ToolKind::Fetch => format!("Fetched {}", s("url").unwrap_or_default()),
            ToolKind::Task => format!(
                "Subagent {}",
                s("description").or_else(|| s("task_name")).or_else(|| s("prompt")).or_else(|| s("message")).map(|d| first_line(&d)).unwrap_or_default()
            ),
            ToolKind::Todo => "Updated todos".to_string(),
            ToolKind::Think => "Thought".to_string(),
            ToolKind::Mcp => format!("MCP {}", self.name),
            ToolKind::Other => self.name.clone(),
        }
        .trim()
        .to_string()
    }

    pub fn added_removed(&self) -> (u32, u32) {
        self.diffs.iter().fold((0, 0), |(a, r), d| (a + d.added, r + d.removed))
    }

    /// Fill `diffs` and `locations` from the native input of edit/write tools
    /// (Claude, Codex apply_patch, Cursor, Gemini, OpenCode, Pi shapes).
    pub fn infer_diffs(&mut self) {
        if !self.diffs.is_empty() {
            return;
        }
        let get = |keys: &[&str]| -> Option<String> {
            keys.iter().find_map(|k| self.input.get(*k).and_then(|v| v.as_str()).map(|s| s.to_string()))
        };
        let path = get(&["file_path", "filePath", "path", "target_file", "notebook_path", "file"]);
        // apply_patch style input (a string, or {input|patch: string}).
        let patch = self.input.as_str().map(|s| s.to_string()).or_else(|| get(&["patch", "input", "diff"]));
        if let Some(p) = patch.filter(|p| p.contains("*** Begin Patch") || p.starts_with("--- ") || p.contains("\n@@")) {
            self.diffs = parse_patch(&p, path.as_deref());
        } else if let Some(path) = path.clone() {
            if let Some(edits) = self.input.get("edits").and_then(|e| e.as_array()) {
                let mut d = FileDiff { path: path.clone(), ..Default::default() };
                for e in edits {
                    let old = e.get("old_string").or_else(|| e.get("oldText")).and_then(|v| v.as_str()).unwrap_or("");
                    let new = e.get("new_string").or_else(|| e.get("newText")).and_then(|v| v.as_str()).unwrap_or("");
                    let (a, r) = line_delta(old, new);
                    d.added += a;
                    d.removed += r;
                }
                self.diffs.push(d);
            } else {
                let old = get(&["old_string", "oldString", "oldText", "old_str"]);
                let new = get(&["new_string", "newString", "newText", "new_str", "content", "contents", "code_edit", "file_text", "text"]);
                if self.kind.is_mutation() && (old.is_some() || new.is_some()) {
                    self.diffs.push(FileDiff::from_texts(path, old, new));
                }
            }
        }
        for d in &self.diffs {
            if !self.locations.contains(&d.path) {
                self.locations.push(d.path.clone());
            }
        }
        if self.locations.is_empty() {
            if let Some(p) = path {
                self.locations.push(p);
            }
        }
    }
}

/// Parse a Codex-style `*** Begin Patch` envelope or a plain unified diff
/// into per-file diffs.
pub fn parse_patch(patch: &str, default_path: Option<&str>) -> Vec<FileDiff> {
    let mut out: Vec<FileDiff> = Vec::new();
    let mut cur: Option<FileDiff> = None;
    let mut body = String::new();
    let flush = |cur: &mut Option<FileDiff>, body: &mut String, out: &mut Vec<FileDiff>| {
        if let Some(mut d) = cur.take() {
            d.unified = Some(std::mem::take(body));
            out.push(d);
        }
    };
    for line in patch.lines() {
        let header = ["*** Update File: ", "*** Add File: ", "*** Delete File: ", "+++ b/", "+++ "]
            .iter()
            .find_map(|h| line.strip_prefix(h).map(|p| p.trim().to_string()));
        if let Some(path) = header {
            if line.starts_with("+++") {
                // unified diff: the `---` line preceded it; replace current path
                if let Some(d) = cur.as_mut() {
                    if d.path.is_empty() || d.path == "/dev/null" {
                        d.path = path;
                    }
                    continue;
                }
            }
            flush(&mut cur, &mut body, &mut out);
            cur = Some(FileDiff { path, ..Default::default() });
            continue;
        }
        if let Some(rest) = line.strip_prefix("--- ") {
            flush(&mut cur, &mut body, &mut out);
            let p = rest.trim().trim_start_matches("a/").to_string();
            cur = Some(FileDiff { path: p, ..Default::default() });
            continue;
        }
        if line.starts_with("*** ") {
            continue;
        }
        if cur.is_none() {
            cur = Some(FileDiff { path: default_path.unwrap_or("").to_string(), ..Default::default() });
        }
        if let Some(d) = cur.as_mut() {
            if line.starts_with('+') {
                d.added += 1;
            } else if line.starts_with('-') {
                d.removed += 1;
            }
        }
        body.push_str(line);
        body.push('\n');
    }
    flush(&mut cur, &mut body, &mut out);
    out.retain(|d| !d.path.is_empty() || d.added + d.removed > 0);
    out
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Read,
    Edit,
    Write,
    Delete,
    Move,
    Search,
    Shell,
    Fetch,
    WebSearch,
    Think,
    Task,
    Todo,
    Mcp,
    #[default]
    Other,
}

impl ToolKind {
    /// Map a native tool name to a normalized kind.
    pub fn classify(name: &str) -> ToolKind {
        let n = name.to_ascii_lowercase();
        let n = n.as_str();
        if n.starts_with("mcp__") || n.starts_with("mcp_") {
            return ToolKind::Mcp;
        }
        match n {
            "read" | "read_file" | "readfile" | "view" | "cat" | "read_text_file" | "notebookread" | "ls" | "list" | "list_dir" | "list_directory" | "readmanyfiles" | "read_many_files" => ToolKind::Read,
            "edit" | "multiedit" | "edit_file" | "str_replace" | "str_replace_editor" | "search_replace" | "apply_patch" | "patch" | "replace" | "notebookedit" | "edit_notebook" => ToolKind::Edit,
            "write" | "write_file" | "create_file" | "writefile" => ToolKind::Write,
            "delete" | "delete_file" | "rm" => ToolKind::Delete,
            "move" | "rename" | "mv" => ToolKind::Move,
            "grep" | "glob" | "search" | "find" | "codebase_search" | "grep_search" | "file_search" | "search_file_content" | "find_files" | "semantic_search" | "ripgrep" => ToolKind::Search,
            "bash" | "shell" | "run_terminal_cmd" | "run_shell_command" | "exec" | "exec_command" | "local_shell" | "terminal" | "command" | "powershell" | "bashoutput" | "killshell" | "command_execution" => ToolKind::Shell,
            "webfetch" | "web_fetch" | "fetch" | "fetchurl" | "fetch_url" | "curl" => ToolKind::Fetch,
            "websearch" | "web_search" | "google_web_search" => ToolKind::WebSearch,
            "think" | "thinking" => ToolKind::Think,
            "task" | "agent" | "subagent" | "dispatch_agent" | "spawn_agent" => ToolKind::Task,
            "todowrite" | "todo_write" | "todoread" | "update_plan" | "todo" | "todo_list" | "write_todos" => ToolKind::Todo,
            _ => {
                if n.contains("edit") || n.contains("patch") {
                    ToolKind::Edit
                } else if n.contains("write") || n.contains("create") {
                    ToolKind::Write
                } else if n.contains("read") {
                    ToolKind::Read
                } else if n.contains("search") || n.contains("grep") || n.contains("glob") {
                    ToolKind::Search
                } else if n.contains("shell") || n.contains("bash") || n.contains("terminal") || n.contains("exec") {
                    ToolKind::Shell
                } else {
                    ToolKind::Other
                }
            }
        }
    }

    pub fn is_mutation(self) -> bool {
        matches!(self, ToolKind::Edit | ToolKind::Write | ToolKind::Delete | ToolKind::Move)
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Pending,
    #[default]
    Running,
    Completed,
    Failed,
    /// Waiting for user permission.
    Approval,
    Cancelled,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct FileDiff {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_text: Option<String>,
    /// Unified diff text when the agent reported a patch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unified: Option<String>,
    #[serde(default)]
    pub added: u32,
    #[serde(default)]
    pub removed: u32,
}

impl FileDiff {
    /// Build a diff from before/after text, counting changed lines.
    pub fn from_texts(path: impl Into<String>, old: Option<String>, new: Option<String>) -> Self {
        let (added, removed) = line_delta(old.as_deref().unwrap_or(""), new.as_deref().unwrap_or(""));
        FileDiff { path: path.into(), old_text: old, new_text: new, unified: None, added, removed }
    }

    pub fn from_unified(path: impl Into<String>, unified: String) -> Self {
        let mut added = 0;
        let mut removed = 0;
        for l in unified.lines() {
            if l.starts_with("+++") || l.starts_with("---") {
                continue;
            }
            if l.starts_with('+') {
                added += 1;
            } else if l.starts_with('-') {
                removed += 1;
            }
        }
        FileDiff { path: path.into(), old_text: None, new_text: None, unified: Some(unified), added, removed }
    }
}

/// Count added/removed lines between two texts with a simple LCS on lines
/// (bounded so huge files stay cheap).
pub fn line_delta(old: &str, new: &str) -> (u32, u32) {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    if a.is_empty() {
        return (b.len() as u32, 0);
    }
    if b.is_empty() {
        return (0, a.len() as u32);
    }
    // Trim common prefix/suffix first.
    let mut start = 0;
    while start < a.len() && start < b.len() && a[start] == b[start] {
        start += 1;
    }
    let mut end_a = a.len();
    let mut end_b = b.len();
    while end_a > start && end_b > start && a[end_a - 1] == b[end_b - 1] {
        end_a -= 1;
        end_b -= 1;
    }
    let a = &a[start..end_a];
    let b = &b[start..end_b];
    if a.len() * b.len() > 4_000_000 {
        return (b.len() as u32, a.len() as u32);
    }
    let mut dp = vec![vec![0u32; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            dp[i][j] = if a[i] == b[j] { dp[i + 1][j + 1] + 1 } else { dp[i + 1][j].max(dp[i][j + 1]) };
        }
    }
    let common = dp[0][0];
    (b.len() as u32 - common, a.len() as u32 - common)
}

// ---------------------------------------------------------------------------
// Session behaviour
// ---------------------------------------------------------------------------

impl Session {
    pub fn new(agent: AgentKind, workspace: Option<WorkspaceRef>) -> Self {
        let now = Utc::now();
        Session {
            id: new_id(),
            title: "New Agent".to_string(),
            created_at: now,
            updated_at: now,
            status: SessionStatus::Idle,
            agent: agent.clone(),
            model: None,
            mode: None,
            permission: None,
            effort: None,
            account_id: None,
            run_as: None,
            workspace,
            environment: Environment::Local,
            origin: Origin::larrier(agent),
            native: Vec::new(),
            parent: None,
            tags: Vec::new(),
            pinned: false,
            archived: false,
            unread: false,
            external_turn: false,
            usage: Usage::default(),
            messages: Vec::new(),
            extra: Map::new(),
        }
    }

    /// Working directory the agent runs in (worktree path if isolated).
    pub fn cwd(&self) -> Option<String> {
        match &self.environment {
            Environment::Worktree { path, .. } => Some(path.clone()),
            Environment::Ssh { path, .. } => Some(path.clone()),
            Environment::Local => self.workspace.as_ref().map(|w| w.path.clone()),
        }
    }

    pub fn touch(&mut self) {
        self.updated_at = Utc::now();
    }

    pub fn push(&mut self, mut msg: Message) {
        if msg.parent_id.is_none() {
            msg.parent_id = self.messages.last().map(|m| m.id.clone());
        }
        self.messages.push(msg);
        self.touch();
    }

    pub fn native_link(&self, agent: &AgentKind) -> Option<&NativeLink> {
        self.native.iter().find(|l| &l.agent == agent)
    }

    pub fn native_link_mut(&mut self, agent: &AgentKind) -> Option<&mut NativeLink> {
        self.native.iter_mut().find(|l| &l.agent == agent)
    }

    pub fn set_native_link(&mut self, link: NativeLink) {
        if let Some(existing) = self.native_link_mut(&link.agent.clone()) {
            *existing = link;
        } else {
            self.native.push(link);
        }
    }

    pub fn first_user_text(&self) -> Option<String> {
        self.messages.iter().find(|m| m.role == Role::User).map(|m| m.text()).filter(|t| !t.trim().is_empty())
    }

    pub fn last_assistant_text(&self) -> Option<String> {
        self.messages
            .iter()
            .rev()
            .filter(|m| m.role == Role::Assistant)
            .map(|m| m.text())
            .find(|t| !t.trim().is_empty())
    }

    /// Derive a short title from the first prompt when none was set.
    pub fn auto_title(&mut self) {
        if self.title.is_empty() || self.title == "New Agent" {
            if let Some(t) = self.first_user_text() {
                self.title = title_from_prompt(&t);
            }
        }
    }

    /// Per-file change totals across every tool call (latest diff wins for
    /// stats per file is not attempted; we sum hunks).
    pub fn changed_files(&self) -> Vec<FileChange> {
        fn walk(msgs: &[Message], map: &mut BTreeMap<String, FileChange>) {
            for m in msgs {
                for t in m.tool_calls() {
                    if !t.subagent.is_empty() {
                        walk(&t.subagent, map);
                    }
                    if t.is_error {
                        continue;
                    }
                    for d in &t.diffs {
                        let e = map.entry(d.path.clone()).or_insert_with(|| FileChange { path: d.path.clone(), added: 0, removed: 0, edits: 0 });
                        e.added += d.added;
                        e.removed += d.removed;
                        e.edits += 1;
                    }
                }
            }
        }
        let mut map: BTreeMap<String, FileChange> = BTreeMap::new();
        walk(&self.messages, &mut map);
        map.into_values().collect()
    }

    pub fn diff_stats(&self) -> (u32, u32, usize) {
        let files = self.changed_files();
        let (a, r) = files.iter().fold((0, 0), |(a, r), f| (a + f.added, r + f.removed));
        (a, r, files.len())
    }

    pub fn recompute_usage(&mut self) {
        let mut u = Usage::default();
        for m in &self.messages {
            if let Some(mu) = &m.usage {
                u.add(mu);
            }
        }
        if !u.is_empty() {
            self.usage = u;
        }
    }

    pub fn summary(&self) -> SessionSummary {
        let (added, removed, files) = self.diff_stats();
        let preview = self
            .last_assistant_text()
            .or_else(|| self.first_user_text())
            .map(|t| preview_line(&t, 160))
            .unwrap_or_default();
        SessionSummary {
            id: self.id.clone(),
            title: self.title.clone(),
            agent: self.agent.clone(),
            origin: self.origin.agent.clone(),
            model: self.model.clone(),
            status: self.status,
            workspace: self.workspace.clone(),
            environment: self.environment.label().to_string(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            preview,
            added,
            removed,
            files_changed: files,
            message_count: self.messages.len(),
            active_at: self.messages.iter().map(|m| m.created_at).max(),
            pinned: self.pinned,
            archived: self.archived,
            unread: self.unread,
            external_turn: self.external_turn,
            parent_id: self.parent.as_ref().map(|p| p.session_id.clone()),
            usage: self.usage.clone(),
        }
    }

    /// Fork the conversation up to (and including) `message_id`.
    pub fn fork(&self, message_id: Option<&str>) -> Session {
        let mut s = self.clone();
        s.id = new_id();
        s.title = format!("{} (fork)", self.title);
        s.native.clear();
        s.status = SessionStatus::Idle;
        s.parent = Some(ForkRef { session_id: self.id.clone(), message_id: message_id.map(|m| m.to_string()) });
        if let Some(mid) = message_id {
            if let Some(idx) = s.messages.iter().position(|m| m.id == mid) {
                s.messages.truncate(idx + 1);
            }
        }
        s.created_at = Utc::now();
        s.updated_at = s.created_at;
        s.recompute_usage();
        s
    }

    // -- serialization ------------------------------------------------------

    pub fn to_jsonl(&self) -> String {
        let mut header = serde_json::to_value(self).unwrap_or(Value::Null);
        if let Value::Object(map) = &mut header {
            map.remove("messages");
            let mut ordered = Map::new();
            ordered.insert("lsf".into(), Value::String(LSF_VERSION.into()));
            ordered.insert("kind".into(), Value::String("session".into()));
            for (k, v) in std::mem::take(map) {
                ordered.insert(k, v);
            }
            *map = ordered;
        }
        let mut out = serde_json::to_string(&header).unwrap_or_default();
        out.push('\n');
        for m in &self.messages {
            let mut v = serde_json::to_value(m).unwrap_or(Value::Null);
            if let Value::Object(map) = &mut v {
                let mut ordered = Map::new();
                ordered.insert("kind".into(), Value::String("message".into()));
                for (k, v) in std::mem::take(map) {
                    ordered.insert(k, v);
                }
                *map = ordered;
            }
            out.push_str(&serde_json::to_string(&v).unwrap_or_default());
            out.push('\n');
        }
        out
    }

    pub fn from_jsonl(text: &str) -> Result<Session, LsfError> {
        let mut lines = text.lines().filter(|l| !l.trim().is_empty());
        let first = lines.next().ok_or(LsfError::Empty)?;
        let mut header: Value = serde_json::from_str(first)?;
        if let Value::Object(map) = &mut header {
            map.remove("lsf");
            map.remove("kind");
            map.insert("messages".into(), Value::Array(Vec::new()));
        }
        let mut session: Session = serde_json::from_value(header)?;
        for (i, line) in lines.enumerate() {
            let mut v: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("lsf: skipping malformed line {}: {e}", i + 2);
                    continue;
                }
            };
            let kind = v.get("kind").and_then(|k| k.as_str()).unwrap_or("message").to_string();
            if let Value::Object(map) = &mut v {
                map.remove("kind");
            }
            if kind == "message" {
                match serde_json::from_value::<Message>(v) {
                    Ok(m) => session.messages.push(m),
                    Err(e) => tracing::warn!("lsf: skipping message on line {}: {e}", i + 2),
                }
            }
        }
        Ok(session)
    }

    pub fn to_json_pretty(&self) -> String {
        let doc = serde_json::json!({ "lsf": LSF_VERSION, "session": self });
        serde_json::to_string_pretty(&doc).unwrap_or_default()
    }

    /// Parse either the JSONL or the single-document JSON form.
    pub fn from_any(text: &str) -> Result<Session, LsfError> {
        let trimmed = text.trim_start();
        if trimmed.starts_with('{') {
            if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
                if let Some(s) = v.get("session") {
                    return Ok(serde_json::from_value(s.clone())?);
                }
                if v.get("messages").is_some() && v.get("id").is_some() {
                    return Ok(serde_json::from_value(v)?);
                }
            }
        }
        Session::from_jsonl(text)
    }

    /// Render the conversation as Markdown (export and human handoff).
    pub fn to_markdown(&self) -> String {
        let mut out = format!("# {}\n\n", self.title);
        out.push_str(&format!(
            "_Agent: {} · Created {} · {} messages_\n\n",
            self.agent.display_name(),
            self.created_at.format("%Y-%m-%d %H:%M UTC"),
            self.messages.len()
        ));
        for m in &self.messages {
            let who = match m.role {
                Role::User => "User".to_string(),
                Role::Assistant => m.agent.as_ref().map(|a| a.display_name().to_string()).unwrap_or_else(|| "Assistant".into()),
                Role::System => "System".to_string(),
            };
            out.push_str(&format!("## {}\n\n", who));
            for p in &m.parts {
                match p {
                    Part::Text { text } => {
                        out.push_str(text);
                        out.push_str("\n\n");
                    }
                    Part::Reasoning { text, .. } => {
                        out.push_str("<details><summary>Thinking</summary>\n\n");
                        out.push_str(text);
                        out.push_str("\n\n</details>\n\n");
                    }
                    Part::Tool(t) => {
                        out.push_str(&format!("- **{}**", t.title.clone().unwrap_or_else(|| t.name.clone())));
                        let (a, r) = t.added_removed();
                        if a + r > 0 {
                            out.push_str(&format!(" (+{a} −{r})"));
                        }
                        out.push('\n');
                    }
                    Part::Plan { entries } => {
                        for e in entries {
                            let mark = if e.status == PlanStatus::Completed { "x" } else { " " };
                            out.push_str(&format!("- [{mark}] {}\n", e.content));
                        }
                        out.push('\n');
                    }
                    Part::File { path, .. } => out.push_str(&format!("📎 `{path}`\n\n")),
                    Part::Image { .. } => out.push_str("🖼️ _image_\n\n"),
                    Part::Error { message } => out.push_str(&format!("> ⚠️ {message}\n\n")),
                    Part::Notice { text } => out.push_str(&format!("> {text}\n\n")),
                }
            }
        }
        out
    }

    /// A compact transcript used to hand a conversation to an agent that
    /// cannot load it natively. Keeps the newest content when truncating.
    pub fn handoff_context(&self, from_index: usize, max_chars: usize) -> String {
        let mut blocks: Vec<String> = Vec::new();
        for m in self.messages.iter().skip(from_index) {
            let who = match m.role {
                Role::User => "USER",
                Role::Assistant => "ASSISTANT",
                Role::System => "SYSTEM",
            };
            let mut body = String::new();
            for p in &m.parts {
                match p {
                    Part::Text { text } => {
                        body.push_str(text.trim());
                        body.push('\n');
                    }
                    Part::Tool(t) => {
                        body.push_str(&format!("[tool] {}", t.title.clone().unwrap_or_else(|| t.name.clone())));
                        if let Some(o) = &t.output {
                            let o = truncate_middle(o.trim(), 600);
                            if !o.is_empty() {
                                body.push_str(&format!("\n  → {}", o.replace('\n', "\n    ")));
                            }
                        }
                        body.push('\n');
                    }
                    Part::Plan { entries } => {
                        for e in entries {
                            body.push_str(&format!("[plan:{:?}] {}\n", e.status, e.content));
                        }
                    }
                    Part::File { path, .. } => body.push_str(&format!("[attached] {path}\n")),
                    Part::Error { message } => body.push_str(&format!("[error] {message}\n")),
                    _ => {}
                }
            }
            if !body.trim().is_empty() {
                blocks.push(format!("{who}:\n{}", body.trim_end()));
            }
        }
        let mut out = String::new();
        let mut used = 0;
        let mut kept: Vec<&String> = Vec::new();
        for b in blocks.iter().rev() {
            if used + b.len() > max_chars && !kept.is_empty() {
                break;
            }
            used += b.len();
            kept.push(b);
        }
        kept.reverse();
        if kept.len() < blocks.len() {
            out.push_str(&format!("[… {} earlier messages omitted …]\n\n", blocks.len() - kept.len()));
        }
        for b in kept {
            out.push_str(b);
            out.push_str("\n\n");
        }
        out
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FileChange {
    pub path: String,
    pub added: u32,
    pub removed: u32,
    pub edits: u32,
}

/// Lightweight row used by the sidebar and the session index.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub agent: AgentKind,
    pub origin: AgentKind,
    #[serde(default)]
    pub model: Option<String>,
    pub status: SessionStatus,
    #[serde(default)]
    pub workspace: Option<WorkspaceRef>,
    #[serde(default)]
    pub environment: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub preview: String,
    #[serde(default)]
    pub added: u32,
    #[serde(default)]
    pub removed: u32,
    #[serde(default)]
    pub files_changed: usize,
    #[serde(default)]
    pub message_count: usize,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub unread: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub external_turn: bool,
    #[serde(default)]
    pub parent_id: Option<String>,
    /// Token and cost totals (for the Usage page).
    #[serde(default)]
    pub usage: Usage,
    /// When its latest message began: a prompt, a reply, a message the
    /// agent's own app added (for "recent activity" ordering).
    #[serde(default)]
    pub active_at: Option<DateTime<Utc>>,
}

impl SessionSummary {
    /// Time of the latest activity (entries saved before Larrier recorded
    /// it fall back to when they were last saved).
    pub fn activity(&self) -> DateTime<Utc> {
        self.active_at.unwrap_or(self.updated_at).max(self.created_at)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LsfError {
    #[error("empty session file")]
    Empty,
    #[error("invalid LSF json: {0}")]
    Json(#[from] serde_json::Error),
}

// ---------------------------------------------------------------------------
// Nested transcripts (subagents)
// ---------------------------------------------------------------------------

/// Path to a tool call: `(message index, part index)` per nesting level,
/// descending through `ToolCall::subagent` transcripts.
pub fn find_tool_path(messages: &[Message], id: &str) -> Option<Vec<(usize, usize)>> {
    for (mi, m) in messages.iter().enumerate().rev() {
        for (pi, p) in m.parts.iter().enumerate() {
            if let Part::Tool(t) = p {
                if t.id == id {
                    return Some(vec![(mi, pi)]);
                }
                if !t.subagent.is_empty() {
                    if let Some(mut rest) = find_tool_path(&t.subagent, id) {
                        rest.insert(0, (mi, pi));
                        return Some(rest);
                    }
                }
            }
        }
    }
    None
}

/// Resolve a path from [`find_tool_path`].
pub fn tool_at_path_mut<'a>(messages: &'a mut [Message], path: &[(usize, usize)]) -> Option<&'a mut ToolCall> {
    let (&(mi, pi), rest) = path.split_first()?;
    let mut t = match messages.get_mut(mi)?.parts.get_mut(pi)? {
        Part::Tool(t) => t,
        _ => return None,
    };
    for &(mi, pi) in rest {
        t = match t.subagent.get_mut(mi)?.parts.get_mut(pi)? {
            Part::Tool(t) => t,
            _ => return None,
        };
    }
    Some(t)
}

/// Find a tool call anywhere, including inside subagent transcripts.
pub fn find_tool_deep_mut<'a>(messages: &'a mut [Message], id: &str) -> Option<&'a mut ToolCall> {
    let path = find_tool_path(messages, id)?;
    tool_at_path_mut(messages, &path)
}

/// Close every still-active tool (recursively) with `status`.
pub fn close_active_tools(messages: &mut [Message], status: ToolStatus) {
    for m in messages.iter_mut() {
        for t in m.tool_calls_mut() {
            close_active_tools(&mut t.subagent, status);
            if t.is_active() {
                t.status = status;
            }
        }
    }
}

/// Every background / subagent call that is still active (recursive), in
/// transcript order.
pub fn active_tasks(messages: &[Message]) -> Vec<&ToolCall> {
    fn walk<'a>(msgs: &'a [Message], out: &mut Vec<&'a ToolCall>) {
        for m in msgs {
            for t in m.tool_calls() {
                if (t.background || t.is_subagent()) && t.is_active() {
                    out.push(t);
                }
                walk(&t.subagent, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(messages, &mut out);
    out
}

/// Shrink a nested (subagent) transcript before storing it: long outputs,
/// inputs, reasoning and answers are trimmed and only the last `max_parts`
/// parts are kept.
pub fn slim_transcript(messages: &mut Vec<Message>, max_parts: usize) {
    fn clip(s: &str, max: usize) -> String {
        if s.len() <= max {
            return s.to_string();
        }
        truncate_middle(s, max)
    }
    fn slim_value(v: &mut Value) {
        match v {
            Value::String(s) if s.len() > 1200 => *s = clip(s, 1200),
            Value::Array(a) => a.iter_mut().for_each(slim_value),
            Value::Object(o) => o.values_mut().for_each(slim_value),
            _ => {}
        }
    }
    let mut total: usize = messages.iter().map(|m| m.parts.len()).sum();
    while total > max_parts && !messages.is_empty() {
        let m = &mut messages[0];
        if m.parts.len() <= total - max_parts {
            total -= m.parts.len();
            messages.remove(0);
        } else {
            let drop = total - max_parts;
            m.parts.drain(..drop);
            total -= drop;
        }
    }
    for m in messages.iter_mut() {
        for p in m.parts.iter_mut() {
            match p {
                Part::Text { text } => *text = clip(text, 6000),
                Part::Reasoning { text, .. } => *text = clip(text, 1500),
                Part::Tool(t) => {
                    if let Some(o) = &t.output {
                        t.output = Some(clip(o, 1500));
                    }
                    slim_value(&mut t.input);
                    for d in t.diffs.iter_mut() {
                        d.old_text = None;
                        d.new_text = None;
                        if let Some(u) = &d.unified {
                            d.unified = Some(clip(u, 4000));
                        }
                    }
                    let keep = max_parts / 2;
                    slim_transcript(&mut t.subagent, keep.max(20));
                }
                Part::Image { .. } => *p = Part::Notice { text: "[image]".into() },
                _ => {}
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Text helpers
// ---------------------------------------------------------------------------

pub fn first_line(s: &str) -> String {
    let l = s.lines().next().unwrap_or("").trim();
    truncate_end(l, 120)
}

pub fn truncate_end(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

pub fn truncate_middle(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max / 2).collect();
    let tail: String = s.chars().skip(n - max / 2).collect();
    format!("{head}\n…\n{tail}")
}

/// One-line plain-text preview (markdown markers stripped).
pub fn preview_line(s: &str, max: usize) -> String {
    let mut plain = String::with_capacity(s.len().min(max * 4));
    for line in s.lines() {
        let l = line.trim_start().trim_start_matches(['#', '>']).trim_start();
        let l = l.strip_prefix("- ").or_else(|| l.strip_prefix("* ")).unwrap_or(l);
        if l.starts_with("```") || l.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) {
            continue;
        }
        plain.push_str(l);
        plain.push(' ');
        if plain.len() > max * 4 {
            break;
        }
    }
    let plain = plain.replace("**", "").replace("__", "").replace('`', "");
    let collapsed = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_end(&collapsed, max)
}

pub fn short_path(p: &str) -> String {
    let parts: Vec<&str> = p.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    if parts.len() <= 3 {
        return parts.join("/");
    }
    parts[parts.len() - 3..].join("/")
}

pub fn title_from_prompt(prompt: &str) -> String {
    // Larrier's context tokens (`@agent:<id>`, `@git:diff`) aren't words.
    let word = |w: &&str| !w.starts_with("@agent:") && *w != "@git:diff";
    let line = prompt
        .lines()
        .map(|l| l.trim())
        .find(|l| !l.starts_with('<') && l.split_whitespace().any(|w| word(&w)))
        .unwrap_or("New Agent");
    let words: Vec<&str> = line.split_whitespace().filter(word).take(8).collect();
    let mut t = words.join(" ");
    t = t.trim_end_matches(['.', ',', ':', ';']).to_string();
    let mut c = t.chars();
    let t = match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => "New Agent".into(),
    };
    truncate_end(&t, 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_jsonl() {
        let mut s = Session::new(AgentKind::ClaudeCode, Some(WorkspaceRef::from_path("/tmp/repo")));
        s.push(Message::user_text("Fix the login bug please"));
        let mut a = Message::assistant(AgentKind::ClaudeCode, Some("sonnet".into()));
        a.push_reasoning("thinking");
        let mut t = ToolCall::new("t1", "Edit", serde_json::json!({"file_path": "src/a.rs"}));
        t.diffs.push(FileDiff::from_texts("src/a.rs", Some("a\nb\n".into()), Some("a\nc\nd\n".into())));
        t.status = ToolStatus::Completed;
        a.parts.push(Part::Tool(t));
        a.push_text("Done.");
        s.push(a);
        s.auto_title();
        let text = s.to_jsonl();
        let back = Session::from_any(&text).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.title, "Fix the login bug please");
        assert_eq!(back.diff_stats(), (2, 1, 1));
        let pretty = s.to_json_pretty();
        assert_eq!(Session::from_any(&pretty).unwrap(), s);
    }

    #[test]
    fn infer_patch_diffs() {
        let patch = "*** Begin Patch\n*** Update File: src/a.rs\n@@\n-old\n+new\n+more\n*** Add File: b.txt\n+hello\n*** End Patch";
        let mut t = ToolCall::new("1", "apply_patch", Value::String(patch.into()));
        t.infer_diffs();
        assert_eq!(t.diffs.len(), 2);
        assert_eq!((t.diffs[0].added, t.diffs[0].removed), (2, 1));
        assert_eq!(t.diffs[1].path, "b.txt");
        let mut e = ToolCall::new("2", "Edit", serde_json::json!({"file_path":"x.rs","old_string":"a","new_string":"b\nc"}));
        e.infer_diffs();
        assert_eq!((e.diffs[0].added, e.diffs[0].removed), (2, 1));
    }

    #[test]
    fn nested_tool_lookup_and_closing() {
        let mut task = ToolCall::new("task1", "Agent", serde_json::json!({"description": "Audit", "subagent_type": "Explore", "prompt": "look"}));
        assert!(task.is_subagent());
        assert_eq!(task.agent_name.as_deref(), Some("Explore"));
        assert_eq!(task.title.as_deref(), Some("Subagent Audit"));
        let mut inner = Message::assistant(AgentKind::ClaudeCode, None);
        let mut edit = ToolCall::new("e1", "Edit", serde_json::json!({"file_path": "a.rs", "old_string": "a", "new_string": "b"}));
        edit.infer_diffs();
        inner.parts.push(Part::Tool(edit));
        inner.parts.push(Part::Tool(ToolCall::new("r1", "Read", serde_json::json!({"file_path": "b.rs"}))));
        task.subagent.push(inner);
        let mut bg = ToolCall::new("b1", "Bash", serde_json::json!({"command": "npm run dev", "run_in_background": true}));
        bg.status = ToolStatus::Running;
        assert!(bg.background);
        let mut outer = Message::assistant(AgentKind::ClaudeCode, None);
        outer.parts.push(Part::Tool(task));
        outer.parts.push(Part::Tool(bg));
        let mut msgs = vec![Message::user_text("go"), outer];
        assert_eq!(find_tool_path(&msgs, "r1"), Some(vec![(1, 0), (0, 1)]));
        find_tool_deep_mut(&mut msgs, "r1").unwrap().output = Some("x".into());
        assert_eq!(active_tasks(&msgs).len(), 2);
        close_active_tools(&mut msgs, ToolStatus::Completed);
        assert!(active_tasks(&msgs).is_empty());
        let t = msgs[1].tool_calls().next().unwrap();
        assert_eq!(t.subagent_steps(), 2);
        let mut s = Session::new(AgentKind::ClaudeCode, None);
        s.messages = msgs.clone();
        assert_eq!(s.diff_stats().2, 1, "subagent edits count");
        let text = s.to_jsonl();
        assert_eq!(Session::from_any(&text).unwrap().messages, msgs);
    }

    #[test]
    fn slim_keeps_tail() {
        let mut m = Message::assistant(AgentKind::ClaudeCode, None);
        for i in 0..50 {
            let mut t = ToolCall::new(format!("t{i}"), "Read", serde_json::json!({"file_path": "x"}));
            t.output = Some("y".repeat(5000));
            m.parts.push(Part::Tool(t));
        }
        let mut v = vec![m];
        slim_transcript(&mut v, 10);
        assert_eq!(v[0].parts.len(), 10);
        match &v[0].parts[9] {
            Part::Tool(t) => {
                assert_eq!(t.id, "t49");
                assert!(t.output.as_ref().unwrap().len() < 2000);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn classify_tools() {
        assert_eq!(ToolKind::classify("Bash"), ToolKind::Shell);
        assert_eq!(ToolKind::classify("apply_patch"), ToolKind::Edit);
        assert_eq!(ToolKind::classify("mcp__github__create_pr"), ToolKind::Mcp);
        assert_eq!(ToolKind::classify("Glob"), ToolKind::Search);
    }
}
