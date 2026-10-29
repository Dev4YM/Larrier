//! Applies runtime events to an LSF session. Shared by the desktop app and
//! the CLI so both record identical transcripts.

use chrono::Utc;
use larrier_core::{AgentKind, Message, NativeLink, Part, Role, Session, SessionStatus, ToolStatus};

use crate::events::AgentEvent;

/// Tracks the assistant message being built during one turn.
pub struct TurnRecorder {
    pub agent: AgentKind,
    pub model: Option<String>,
    pub account_id: Option<String>,
    /// Index of the assistant message in `session.messages`.
    msg_index: Option<usize>,
    reasoning_started: Option<std::time::Instant>,
    /// Number of LSF messages that existed before this turn (user msg incl.).
    pub base_len: usize,
    pub finished: bool,
    pub ok: bool,
}

impl TurnRecorder {
    /// Call after pushing the user's message.
    pub fn begin(session: &mut Session, agent: AgentKind, model: Option<String>, account_id: Option<String>) -> Self {
        session.status = SessionStatus::Running;
        session.agent = agent.clone();
        if model.is_some() {
            session.model = model.clone();
        }
        session.touch();
        TurnRecorder {
            agent,
            model,
            account_id,
            msg_index: None,
            reasoning_started: None,
            base_len: session.messages.len(),
            finished: false,
            ok: false,
        }
    }

    fn msg<'a>(&mut self, session: &'a mut Session) -> &'a mut Message {
        let idx = match self.msg_index {
            Some(i) if i < session.messages.len() => i,
            _ => {
                let m = Message::assistant(self.agent.clone(), self.model.clone());
                session.push(m);
                let i = session.messages.len() - 1;
                self.msg_index = Some(i);
                i
            }
        };
        &mut session.messages[idx]
    }

    fn close_reasoning(&mut self, session: &mut Session) {
        if let Some(start) = self.reasoning_started.take() {
            let elapsed = start.elapsed().as_millis() as u64;
            let m = self.msg(session);
            for p in m.parts.iter_mut().rev() {
                if let Part::Reasoning { duration_ms, .. } = p {
                    if duration_ms.is_none() {
                        *duration_ms = Some(elapsed);
                    }
                    break;
                }
            }
        }
    }

    /// Apply one event. Returns true when the session changed visibly.
    pub fn apply(&mut self, session: &mut Session, event: &AgentEvent) -> bool {
        match event {
            AgentEvent::NativeSession { id, path } => {
                let synced = session.messages.len();
                let link = NativeLink {
                    agent: self.agent.clone(),
                    id: id.clone(),
                    path: path.clone(),
                    synced_messages: synced,
                    account_id: self.account_id.clone(),
                };
                session.set_native_link(link);
                false
            }
            AgentEvent::Model { model } => {
                self.model = Some(model.clone());
                session.model = Some(model.clone());
                if let Some(i) = self.msg_index {
                    if let Some(m) = session.messages.get_mut(i) {
                        m.model = Some(model.clone());
                    }
                }
                false
            }
            AgentEvent::TextDelta { text } => {
                self.close_reasoning(session);
                self.msg(session).push_text(text);
                session.touch();
                true
            }
            AgentEvent::Text { text } => {
                self.close_reasoning(session);
                let m = self.msg(session);
                m.parts.push(Part::Text { text: text.clone() });
                session.touch();
                true
            }
            AgentEvent::ReasoningDelta { text } => {
                if self.reasoning_started.is_none() {
                    self.reasoning_started = Some(std::time::Instant::now());
                }
                self.msg(session).push_reasoning(text);
                true
            }
            AgentEvent::Reasoning { text, duration_ms } => {
                self.close_reasoning(session);
                self.msg(session).parts.push(Part::Reasoning { text: text.clone(), duration_ms: *duration_ms });
                true
            }
            AgentEvent::ToolStart { tool } => {
                self.close_reasoning(session);
                let m = self.msg(session);
                if let Some(existing) = m.find_tool_mut(&tool.id) {
                    *existing = tool.clone();
                } else {
                    m.parts.push(Part::Tool(tool.clone()));
                }
                session.touch();
                true
            }
            AgentEvent::ToolUpdate { id, status, output, diffs, is_error, title } => {
                // Tool might live in an earlier assistant message of this turn.
                let base = self.base_len;
                for m in session.messages.iter_mut().skip(base.saturating_sub(1)).rev() {
                    if let Some(t) = m.find_tool_mut(id) {
                        if let Some(s) = status {
                            t.status = *s;
                        }
                        if let Some(o) = output {
                            t.output = Some(o.clone());
                        }
                        if !diffs.is_empty() {
                            t.diffs = diffs.clone();
                        }
                        if *is_error {
                            t.is_error = true;
                            t.status = ToolStatus::Failed;
                        }
                        if let Some(ti) = title {
                            t.title = Some(ti.clone());
                        }
                        break;
                    }
                }
                session.touch();
                true
            }
            AgentEvent::Plan { entries } => {
                let m = self.msg(session);
                if let Some(Part::Plan { entries: e }) = m.parts.iter_mut().rev().find(|p| matches!(p, Part::Plan { .. })) {
                    *e = entries.clone();
                } else {
                    m.parts.push(Part::Plan { entries: entries.clone() });
                }
                true
            }
            AgentEvent::Permission { .. } => {
                session.status = SessionStatus::NeedsInput;
                true
            }
            AgentEvent::Usage { usage } => {
                let m = self.msg(session);
                let mut u = m.usage.clone().unwrap_or_default();
                u.add(usage);
                m.usage = Some(u);
                session.recompute_usage();
                false
            }
            AgentEvent::Notice { text } => {
                self.msg(session).parts.push(Part::Notice { text: text.clone() });
                true
            }
            AgentEvent::Error { message } => {
                self.close_reasoning(session);
                let m = self.msg(session);
                // Agents often report the same failure twice (inline + final result).
                let dup = m.parts.iter().rev().take(2).any(|p| match p {
                    Part::Error { message: e } => e.trim() == message.trim(),
                    Part::Text { text } => text.trim() == message.trim(),
                    _ => false,
                });
                if !dup {
                    m.parts.push(Part::Error { message: message.clone() });
                }
                true
            }
            AgentEvent::Log { .. } => false,
            AgentEvent::Done { ok, .. } => {
                self.close_reasoning(session);
                self.finish(session, *ok);
                true
            }
        }
    }

    /// Mark the turn finished; any still-running tool is closed out.
    pub fn finish(&mut self, session: &mut Session, ok: bool) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.ok = ok;
        let base = self.base_len;
        for m in session.messages.iter_mut().skip(base) {
            for t in m.tool_calls_mut() {
                if matches!(t.status, ToolStatus::Running | ToolStatus::Pending | ToolStatus::Approval) {
                    t.status = if ok { ToolStatus::Completed } else { ToolStatus::Cancelled };
                }
            }
        }
        // Drop an empty assistant shell.
        if let Some(i) = self.msg_index {
            if session.messages.get(i).map(|m| m.parts.is_empty()).unwrap_or(false) {
                session.messages.remove(i);
                self.msg_index = None;
            }
        }
        let changed = session.messages.iter().skip(base).any(|m| m.tool_calls().any(|t| t.kind.is_mutation() && !t.is_error));
        session.status = if !ok {
            SessionStatus::Error
        } else if changed {
            SessionStatus::Review
        } else {
            SessionStatus::Done
        };
        // Native session now contains everything up to here.
        let len = session.messages.len();
        let agent = self.agent.clone();
        if let Some(link) = session.native_link_mut(&agent) {
            link.synced_messages = len;
        }
        session.unread = true;
        session.updated_at = Utc::now();
        session.recompute_usage();
    }
}

/// Count messages the target agent's native session hasn't seen.
pub fn unsynced_from(session: &Session, agent: &AgentKind) -> usize {
    session.native_link(agent).map(|l| l.synced_messages.min(session.messages.len())).unwrap_or(0)
}

/// True when the session has prior conversation the agent must be told about.
pub fn needs_handoff(session: &Session, agent: &AgentKind) -> bool {
    let synced = unsynced_from(session, agent);
    // The last message is the new user prompt; anything before it that the
    // agent has not seen requires a handoff.
    session.messages.len().saturating_sub(1) > synced && session.messages.iter().take(session.messages.len().saturating_sub(1)).skip(synced).any(|m| m.role != Role::System)
}
