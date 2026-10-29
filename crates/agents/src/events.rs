//! Normalized runtime events every driver emits, and the controls a running
//! turn accepts.

use larrier_core::{FileDiff, PlanEntry, ToolCall, ToolStatus, Usage};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AgentEvent {
    /// The agent's own session id (used to resume natively next turn).
    NativeSession { id: String, path: Option<String> },
    /// Model actually used, when the agent reports it.
    Model { model: String },
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
    Plan { entries: Vec<PlanEntry> },
    Permission { request: PermissionRequest },
    Usage { usage: Usage },
    Notice { text: String },
    Error { message: String },
    /// Diagnostic output from the agent process (not shown inline).
    Log { line: String },
    /// Turn finished. `ok == false` when the agent failed or was cancelled.
    Done { ok: bool, stop_reason: Option<String> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    pub options: Vec<PermissionOption>,
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
    Permission { request_id: String, option_id: Option<String> },
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
    pub fn respond(&self, request_id: impl Into<String>, option_id: Option<String>) {
        let _ = self.tx.send(TurnCommand::Permission { request_id: request_id.into(), option_id });
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
