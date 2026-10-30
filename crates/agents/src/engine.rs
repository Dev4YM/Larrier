//! Applies runtime events to an LSF session. Shared by the desktop app and
//! the CLI so both record identical transcripts.

use chrono::Utc;
use larrier_core::lsf::{close_active_tools, find_tool_deep_mut};
use larrier_core::{AgentKind, FileDiff, Message, NativeLink, Part, Role, Session, SessionStatus, ToolCall, ToolStatus};

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
                    resume_at: None,
                };
                session.set_native_link(link);
                false
            }
            AgentEvent::Model { model } => {
                // The reply records the model that answered; the session
                // keeps the user's pick (Claude Code reports `opus` as
                // `claude-opus-5-5`, which would unpick the alias).
                self.model = Some(model.clone());
                if let Some(i) = self.msg_index {
                    if let Some(m) = session.messages.get_mut(i) {
                        m.model = Some(model.clone());
                    }
                }
                false
            }
            AgentEvent::ModelName { name } => {
                self.model = Some(name.clone());
                if let Some(i) = self.msg_index {
                    if let Some(m) = session.messages.get_mut(i) {
                        m.model = Some(name.clone());
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
                place_tool(self.msg(session), tool);
                session.touch();
                true
            }
            AgentEvent::ToolUpdate { id, status, output, diffs, is_error, title } => {
                // Usually in this turn; background tasks can finish turns later.
                if let Some(t) = self.find_tool(session, id) {
                    update_tool(t, *status, output.as_deref(), diffs, *is_error, title.as_deref());
                }
                session.touch();
                true
            }
            AgentEvent::ToolMeta { id, duration_ms, background, agent_name } => {
                if let Some(t) = self.find_tool(session, id) {
                    apply_meta(t, *duration_ms, *background, agent_name.as_deref());
                }
                false
            }
            AgentEvent::ToolImages { id, images } => match self.find_tool(session, id) {
                Some(t) => {
                    t.images.extend(images.iter().cloned());
                    session.touch();
                    true
                }
                None => false,
            },
            AgentEvent::Subagent { parent_tool_id, inner } => {
                let agent = self.agent.clone();
                match self.find_tool(session, parent_tool_id) {
                    Some(t) => {
                        apply_nested(t, inner, &agent, 0);
                        session.touch();
                        true
                    }
                    None => {
                        tracing::debug!("subagent event for unknown tool {parent_tool_id}");
                        false
                    }
                }
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
            AgentEvent::Mode { id } => {
                session.mode = Some(id.clone());
                true
            }
            AgentEvent::PermissionClosed { .. } => {
                session.status = SessionStatus::Running;
                true
            }
            AgentEvent::Steered { text, attachments } => {
                // The prompt goes where the agent took it in; what follows
                // is a new reply.
                self.close_reasoning(session);
                let mut m = Message::user_text(text.clone());
                m.parts.extend(attachments.iter().cloned());
                session.push(m);
                self.msg_index = None;
                session.touch();
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
            AgentEvent::Log { .. } | AgentEvent::RateLimit { .. } | AgentEvent::Idle => false,
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
        // The agent process is gone: nothing it started is still running,
        // including subagents and background shells.
        let end = if ok { ToolStatus::Completed } else { ToolStatus::Cancelled };
        if base <= session.messages.len() {
            stamp_active(&mut session.messages[base..]);
            close_active_tools(&mut session.messages[base..], end);
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

impl TurnRecorder {
    /// A tool call by id: this turn first (incl. subagent transcripts), then
    /// the whole session.
    fn find_tool<'a>(&self, session: &'a mut Session, id: &str) -> Option<&'a mut ToolCall> {
        let start = self.base_len.saturating_sub(1).min(session.messages.len());
        if larrier_core::lsf::find_tool_path(&session.messages[start..], id).is_some() {
            return find_tool_deep_mut(&mut session.messages[start..], id);
        }
        find_tool_deep_mut(&mut session.messages, id)
    }
}

/// Add a started tool to a message, or refresh a re-announced one without
/// losing what was recorded for it (subagent transcript, start time).
fn place_tool(m: &mut Message, tool: &ToolCall) {
    let mut t = tool.clone();
    if let Some(existing) = m.find_tool_mut(&tool.id) {
        if t.subagent.is_empty() {
            t.subagent = std::mem::take(&mut existing.subagent);
        }
        t.started_at = t.started_at.or(existing.started_at);
        t.background |= existing.background;
        if t.agent_name.is_none() {
            t.agent_name = existing.agent_name.take();
        }
        *existing = t;
    } else {
        t.started_at.get_or_insert_with(Utc::now);
        m.parts.push(Part::Tool(t));
    }
}

/// Stamp run times on everything still running (the turn is ending).
fn stamp_active(messages: &mut [Message]) {
    for m in messages.iter_mut() {
        for t in m.tool_calls_mut() {
            stamp_active(&mut t.subagent);
            if t.is_active() {
                t.stamp_duration();
            }
        }
    }
}

fn update_tool(t: &mut ToolCall, status: Option<ToolStatus>, output: Option<&str>, diffs: &[FileDiff], is_error: bool, title: Option<&str>) {
    if let Some(s) = status {
        t.status = s;
    }
    if let Some(o) = output {
        t.output = Some(o.to_string());
    }
    if !diffs.is_empty() {
        t.diffs = diffs.to_vec();
        for d in diffs {
            if !t.locations.contains(&d.path) {
                t.locations.push(d.path.clone());
            }
        }
    }
    if is_error {
        t.is_error = true;
        t.status = ToolStatus::Failed;
    }
    if let Some(ti) = title {
        t.title = Some(ti.to_string());
    }
    if !t.is_active() {
        t.stamp_duration();
    }
}

fn apply_meta(t: &mut ToolCall, duration_ms: Option<u64>, background: Option<bool>, agent_name: Option<&str>) {
    if duration_ms.is_some() {
        t.duration_ms = duration_ms;
    }
    if let Some(b) = background {
        t.background = b;
    }
    if let Some(n) = agent_name.filter(|n| !n.is_empty()) {
        t.agent_name = Some(n.to_string());
    }
}

/// Parts kept per nested transcript while a subagent streams.
const MAX_NESTED_PARTS: usize = 600;

/// Record an event into a subagent's nested transcript.
fn apply_nested(tool: &mut ToolCall, event: &AgentEvent, agent: &AgentKind, depth: usize) {
    if depth > 8 {
        return;
    }
    let msgs = &mut tool.subagent;
    match event {
        AgentEvent::Subagent { parent_tool_id, inner } => {
            if let Some(t) = find_tool_deep_mut(msgs, parent_tool_id) {
                apply_nested(t, inner, agent, depth + 1);
            }
            return;
        }
        AgentEvent::ToolUpdate { id, status, output, diffs, is_error, title } => {
            if let Some(t) = find_tool_deep_mut(msgs, id) {
                update_tool(t, *status, output.as_deref(), diffs, *is_error, title.as_deref());
            }
            return;
        }
        AgentEvent::ToolMeta { id, duration_ms, background, agent_name } => {
            if let Some(t) = find_tool_deep_mut(msgs, id) {
                apply_meta(t, *duration_ms, *background, agent_name.as_deref());
            }
            return;
        }
        AgentEvent::ToolImages { id, images } => {
            if let Some(t) = find_tool_deep_mut(msgs, id) {
                t.images.extend(images.iter().cloned());
            }
            return;
        }
        AgentEvent::NativeSession { .. } | AgentEvent::Permission { .. } | AgentEvent::PermissionClosed { .. } | AgentEvent::Steered { .. } | AgentEvent::Mode { .. } | AgentEvent::Log { .. } | AgentEvent::RateLimit { .. } | AgentEvent::Idle | AgentEvent::Done { .. } => return,
        _ => {}
    }
    let parts: usize = msgs.iter().map(|m| m.parts.len()).sum();
    if parts >= MAX_NESTED_PARTS && !matches!(event, AgentEvent::TextDelta { .. } | AgentEvent::ReasoningDelta { .. } | AgentEvent::Usage { .. } | AgentEvent::Model { .. } | AgentEvent::ModelName { .. }) {
        return;
    }
    if msgs.last().map(|m| m.role != Role::Assistant).unwrap_or(true) {
        let mut m = Message::assistant(agent.clone(), None);
        m.sidechain = true;
        msgs.push(m);
    }
    let m = msgs.last_mut().expect("assistant message");
    match event {
        AgentEvent::Model { model } | AgentEvent::ModelName { name: model } => m.model = Some(model.clone()),
        AgentEvent::TextDelta { text } => m.push_text(text),
        AgentEvent::Text { text } => m.parts.push(Part::Text { text: text.clone() }),
        AgentEvent::ReasoningDelta { text } => m.push_reasoning(text),
        AgentEvent::Reasoning { text, duration_ms } => m.parts.push(Part::Reasoning { text: text.clone(), duration_ms: *duration_ms }),
        AgentEvent::ToolStart { tool } => place_tool(m, tool),
        AgentEvent::Plan { entries } => {
            if let Some(Part::Plan { entries: e }) = m.parts.iter_mut().rev().find(|p| matches!(p, Part::Plan { .. })) {
                *e = entries.clone();
            } else {
                m.parts.push(Part::Plan { entries: entries.clone() });
            }
        }
        AgentEvent::Usage { usage } => {
            let mut u = m.usage.clone().unwrap_or_default();
            u.add(usage);
            m.usage = Some(u);
        }
        AgentEvent::Notice { text } => m.parts.push(Part::Notice { text: text.clone() }),
        AgentEvent::Error { message } => m.parts.push(Part::Error { message: message.clone() }),
        _ => {}
    }
}

/// Count messages the target agent's native session hasn't seen.
pub fn unsynced_from(session: &Session, agent: &AgentKind) -> usize {
    session.native_link(agent).map(|l| l.synced_messages.min(session.messages.len())).unwrap_or(0)
}

/// True when the session has prior conversation the agent must be told about.
pub fn needs_handoff(session: &Session, agent: &AgentKind) -> bool {
    has_unsynced(session, unsynced_from(session, agent))
}

/// True when messages from `from` up to the new prompt (the last message)
/// hold conversation the agent has not seen.
pub fn has_unsynced(session: &Session, from: usize) -> bool {
    session.messages.len().saturating_sub(1) > from && session.messages.iter().take(session.messages.len().saturating_sub(1)).skip(from).any(|m| m.role != Role::System)
}

#[cfg(test)]
mod tests {
    use super::*;
    use larrier_core::lsf::active_tasks;

    fn sub(parent: &str, e: AgentEvent) -> AgentEvent {
        AgentEvent::Subagent { parent_tool_id: parent.into(), inner: Box::new(e) }
    }

    #[test]
    fn records_subagents_and_background_tasks() {
        let mut s = Session::new(AgentKind::ClaudeCode, None);
        s.push(Message::user_text("audit the repo"));
        let mut rec = TurnRecorder::begin(&mut s, AgentKind::ClaudeCode, None, None);
        let task = ToolCall::new("task1", "Agent", serde_json::json!({"description": "Audit", "subagent_type": "Explore", "prompt": "look around"}));
        rec.apply(&mut s, &AgentEvent::ToolStart { tool: task });
        rec.apply(&mut s, &sub("task1", AgentEvent::Text { text: "Looking".into() }));
        let read = ToolCall::new("r1", "Read", serde_json::json!({"file_path": "src/main.rs"}));
        rec.apply(&mut s, &sub("task1", AgentEvent::ToolStart { tool: read }));
        rec.apply(&mut s, &sub("task1", AgentEvent::ToolUpdate { id: "r1".into(), status: Some(ToolStatus::Completed), output: Some("fn main".into()), diffs: vec![], is_error: false, title: None }));
        // Nested subagent inside the subagent.
        rec.apply(&mut s, &sub("task1", AgentEvent::ToolStart { tool: ToolCall::new("task2", "Task", serde_json::json!({"description": "Deep"})) }));
        rec.apply(&mut s, &sub("task2", AgentEvent::Text { text: "deeper".into() }));
        let bg = ToolCall::new("b1", "Bash", serde_json::json!({"command": "npm run dev", "run_in_background": true}));
        rec.apply(&mut s, &AgentEvent::ToolStart { tool: bg });
        rec.apply(&mut s, &AgentEvent::ToolUpdate { id: "b1".into(), status: Some(ToolStatus::Running), output: Some("Command running in background with ID: x1".into()), diffs: vec![], is_error: false, title: None });
        rec.apply(&mut s, &AgentEvent::ToolMeta { id: "task1".into(), duration_ms: Some(4200), background: None, agent_name: None });

        let task = s.messages.last().unwrap().tool_calls().find(|t| t.id == "task1").unwrap().clone();
        assert_eq!(task.duration_ms, Some(4200));
        assert_eq!(task.agent_name.as_deref(), Some("Explore"));
        assert_eq!(task.subagent.len(), 1);
        assert_eq!(task.subagent[0].text(), "Looking");
        assert_eq!(task.subagent_steps(), 2);
        let deep = task.subagent[0].tool_calls().find(|t| t.id == "task2").unwrap();
        assert_eq!(deep.subagent[0].text(), "deeper");
        assert_eq!(active_tasks(&s.messages).len(), 3, "task1, task2 and the background shell");

        rec.apply(&mut s, &AgentEvent::ToolUpdate { id: "task1".into(), status: Some(ToolStatus::Completed), output: Some("done".into()), diffs: vec![], is_error: false, title: None });
        rec.apply(&mut s, &AgentEvent::Done { ok: true, stop_reason: None });
        assert!(active_tasks(&s.messages).is_empty(), "everything closes when the turn ends");
    }

    #[test]
    fn records_tool_images_in_turn_and_subagents() {
        let mut s = Session::new(AgentKind::ClaudeCode, None);
        s.push(Message::user_text("screenshot it"));
        let mut rec = TurnRecorder::begin(&mut s, AgentKind::ClaudeCode, None, None);
        let img = || vec![Part::Image { mime: Some("image/png".into()), source: larrier_core::lsf::MediaSource::Base64 { data: "iVBORw0KGgo=".into() } }];
        rec.apply(&mut s, &AgentEvent::ToolStart { tool: ToolCall::new("shot", "mcp__browser__screenshot", serde_json::json!({})) });
        rec.apply(&mut s, &AgentEvent::ToolStart { tool: ToolCall::new("task1", "Agent", serde_json::json!({"description": "Check UI"})) });
        rec.apply(&mut s, &sub("task1", AgentEvent::ToolStart { tool: ToolCall::new("inner", "mcp__browser__screenshot", serde_json::json!({})) }));
        assert!(rec.apply(&mut s, &AgentEvent::ToolImages { id: "shot".into(), images: img() }));
        rec.apply(&mut s, &sub("task1", AgentEvent::ToolImages { id: "inner".into(), images: img() }));
        let m = s.messages.last().unwrap();
        assert_eq!(m.tool_calls().find(|t| t.id == "shot").unwrap().images.len(), 1);
        let task = m.tool_calls().find(|t| t.id == "task1").unwrap();
        assert_eq!(task.subagent[0].tool_calls().find(|t| t.id == "inner").unwrap().images.len(), 1);
    }
}
