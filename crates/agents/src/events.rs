//! Normalized runtime events every driver emits, and the controls a running
//! turn accepts.

use larrier_core::{FileDiff, Part, PlanEntry, ToolCall, ToolStatus, Usage};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AgentEvent {
    /// The agent's own session id (used to resume natively next turn).
    NativeSession { id: String, path: Option<String> },
    /// Model actually used, when the agent reports it as an id it accepts
    /// back (`--model`): it becomes the session's model.
    Model { model: String },
    /// The model by its display name only (Cursor's "Composer 2"): shown on
    /// the reply, never passed back to the agent.
    ModelName { name: String },
    /// The agent changed its own mode during the turn (Claude Code leaves
    /// plan mode once its plan is approved): later turns start in it.
    Mode { id: String },
    /// Streamed assistant text.
    TextDelta { text: String },
    /// A complete assistant text block (non-streaming agents). Starts a new
    /// text part rather than appending to the previous one.
    Text { text: String },
    ReasoningDelta { text: String },
    Reasoning { text: String, duration_ms: Option<u64> },
    ToolStart { tool: ToolCall },
    ToolUpdate {
        id: String,
        status: Option<ToolStatus>,
        output: Option<String>,
        #[serde(default)]
        diffs: Vec<FileDiff>,
        #[serde(default)]
        is_error: bool,
        title: Option<String>,
    },
    /// Images a tool returned (screenshots from MCP / browser tools).
    ToolImages { id: String, images: Vec<Part> },
    Plan { entries: Vec<PlanEntry> },
    /// Extra facts about a tool call: run time, background-ness, subagent type.
    ToolMeta {
        id: String,
        #[serde(default)]
        duration_ms: Option<u64>,
        #[serde(default)]
        background: Option<bool>,
        #[serde(default)]
        agent_name: Option<String>,
    },
    /// Something that happened inside the subagent spawned by tool
    /// `parent_tool_id` (recorded into that tool's nested transcript).
    Subagent { parent_tool_id: String, inner: Box<AgentEvent> },
    Permission { request: PermissionRequest },
    /// The agent withdrew a request it made (the turn was interrupted or
    /// it no longer needs the answer).
    PermissionClosed { id: String },
    /// The agent took in a message sent while it was working: recorded as
    /// a prompt at this point of the turn.
    Steered {
        text: String,
        #[serde(default)]
        attachments: Vec<Part>,
    },
    /// The answer to `TurnCommand::Unsteer`: the message was taken back, or
    /// the agent had already read it.
    Unsteered { key: u64, taken_back: bool },
    Usage { usage: Usage },
    Notice { text: String },
    Error { message: String },
    /// A plan rate-limit window as the agent reports it mid-turn (Claude
    /// Code's `rate_limit_event`): percent used and when it resets (unix
    /// seconds). Not part of the transcript.
    RateLimit { window: String, used_pct: f64, resets_at: Option<i64> },
    /// The agent finished its reply but the turn goes on: background work
    /// (subagents, commands) still runs and may bring it back. Not part of
    /// the transcript; content events after it mean it is replying again.
    Idle,
    /// Diagnostic output from the agent process (not shown inline).
    Log { line: String },
    /// Turn finished. `ok == false` when the agent failed or was cancelled.
    Done { ok: bool, stop_reason: Option<String> },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    pub options: Vec<PermissionOption>,
    #[serde(default)]
    pub kind: RequestKind,
    /// The questions of a `Question` request.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<AskQuestion>,
    /// The plan of a `Plan` request (Markdown).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
}

/// What an agent is waiting for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestKind {
    /// Approval to run a tool.
    #[default]
    Tool,
    /// Answers to questions.
    Question,
    /// Approval of a plan before the agent starts changing things.
    Plan,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AskQuestion {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub header: Option<String>,
    pub question: String,
    #[serde(default)]
    pub multi: bool,
    #[serde(default)]
    pub options: Vec<AskOption>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AskOption {
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PermissionOption {
    pub id: String,
    pub label: String,
    /// `allow_once`, `allow_always`, `reject_once`, `reject_always`
    pub kind: String,
}

/// Commands sent into a running turn.
#[derive(Clone, Debug)]
pub enum TurnCommand {
    Cancel,
    Permission { request_id: String, reply: Reply },
    /// A message for the running turn: folded in where the agent can take
    /// it (`now`: the agent stops what it is doing and answers it first).
    /// `key` names it for `Unsteer`.
    Steer { text: String, attachments: Vec<Part>, now: bool, key: u64 },
    /// Take back a `Steer` message the agent hasn't read yet; answered with
    /// `AgentEvent::Unsteered`.
    Unsteer { key: u64 },
}

/// The user's answer to a request.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reply {
    /// The option picked; `None` dismisses the request.
    pub option_id: Option<String>,
    /// Answers to a `Question` request, one list per question.
    pub answers: Vec<Vec<String>>,
    /// Words for the agent with the choice (why not, what to change).
    pub message: Option<String>,
}

impl Reply {
    pub fn option(id: Option<String>) -> Reply {
        Reply { option_id: id, ..Default::default() }
    }
}

/// Handle to a running turn: a stream of events plus a control channel.
pub struct TurnHandle {
    pub events: mpsc::UnboundedReceiver<AgentEvent>,
    pub control: TurnControl,
}

#[derive(Clone)]
pub struct TurnControl {
    tx: mpsc::UnboundedSender<TurnCommand>,
}

impl TurnControl {
    pub fn new(tx: mpsc::UnboundedSender<TurnCommand>) -> Self {
        TurnControl { tx }
    }
    pub fn cancel(&self) {
        let _ = self.tx.send(TurnCommand::Cancel);
    }
    pub fn respond(&self, request_id: impl Into<String>, reply: Reply) {
        let _ = self.tx.send(TurnCommand::Permission { request_id: request_id.into(), reply });
    }
    pub fn steer(&self, text: String, attachments: Vec<Part>, now: bool, key: u64) {
        let _ = self.tx.send(TurnCommand::Steer { text, attachments, now, key });
    }

    pub fn unsteer(&self, key: u64) {
        let _ = self.tx.send(TurnCommand::Unsteer { key });
    }
}

/// Sender side used by drivers.
#[derive(Clone)]
pub struct EventSink {
    tx: mpsc::UnboundedSender<AgentEvent>,
}

impl EventSink {
    pub fn new(tx: mpsc::UnboundedSender<AgentEvent>) -> Self {
        EventSink { tx }
    }
    pub fn send(&self, e: AgentEvent) {
        let _ = self.tx.send(e);
    }
    pub fn text(&self, t: impl Into<String>) {
        self.send(AgentEvent::TextDelta { text: t.into() });
    }
    pub fn error(&self, m: impl Into<String>) {
        self.send(AgentEvent::Error { message: m.into() });
    }
    pub fn notice(&self, m: impl Into<String>) {
        self.send(AgentEvent::Notice { text: m.into() });
    }
    pub fn log(&self, m: impl Into<String>) {
        self.send(AgentEvent::Log { line: m.into() });
    }
    pub fn done(&self, ok: bool, reason: Option<String>) {
        self.send(AgentEvent::Done { ok, stop_reason: reason });
    }
}

/// Create the paired channels for a turn.
pub fn turn_channels() -> (EventSink, mpsc::UnboundedReceiver<TurnCommand>, TurnHandle) {
    let (etx, erx) = mpsc::unbounded_channel();
    let (ctx, crx) = mpsc::unbounded_channel();
    (EventSink::new(etx), crx, TurnHandle { events: erx, control: TurnControl::new(ctx) })
}
