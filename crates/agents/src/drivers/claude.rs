//! Claude Code (`claude -p --output-format stream-json --verbose
//! --include-partial-messages`, prompt on stdin), plus the agents that speak
//! the same event shapes: Amp (`amp -x --stream-json`) and Qwen Code
//! (`qwen -p --output-format stream-json`).

use std::collections::{HashMap, HashSet};

use larrier_core::{AgentKind, FileDiff, ToolStatus};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedReceiver;

use super::{content_text, json_line, plan_entries, run_stream, s, tool, u, usage_from, with_file_mentions, LineParser};
use crate::events::{AgentEvent, EventSink, TurnCommand};
use crate::request::TurnRequest;

pub async fn run(req: TurnRequest, sink: EventSink, mut control: UnboundedReceiver<TurnCommand>) {
    let mut cmd = req.launch.command();
    cmd.current_dir(&req.cwd);
    cmd.args(["-p", "--output-format", "stream-json", "--verbose"]);
    if !matches!(req.agent, AgentKind::Custom(_)) {
        cmd.arg("--include-partial-messages");
        // Resuming from the file path is the most robust form (works for
        // sessions Larrier wrote and avoids cross-project id clashes).
        if let Some(target) = req.resume_path.as_ref().or(req.resume_id.as_ref()) {
            cmd.args(["--resume", target]);
        }
        if let Some(m) = &req.model {
            cmd.args(["--model", m]);
        }
        // --permission-mode from the chosen mode (Auto unless picked otherwise).
        cmd.args(&req.modes.args);
        // Claude Code refuses to start nested inside another Claude Code session.
        cmd.env_remove("CLAUDECODE");
    }
    cmd.args(&req.launch.extra_args);
    let prompt = with_file_mentions(&req, req.full_prompt());
    let mut parser = ClaudeStream::default();
    run_stream("Claude Code", cmd, Some(prompt), &sink, &mut control, &mut parser).await;
}

pub async fn run_amp(req: TurnRequest, sink: EventSink, mut control: UnboundedReceiver<TurnCommand>) {
    let mut cmd = req.launch.command();
    cmd.current_dir(&req.cwd);
    if let Some(id) = &req.resume_id {
        cmd.args(["threads", "continue", id]);
    }
    let prompt = with_file_mentions(&req, req.prompt_for_arg(24_000));
    cmd.args(["-x", &prompt, "--stream-json"]);
    cmd.args(&req.modes.args);
    cmd.args(&req.launch.extra_args);
    let mut parser = ClaudeStream::default();
    run_stream("Amp", cmd, None, &sink, &mut control, &mut parser).await;
}

pub async fn run_qwen(req: TurnRequest, sink: EventSink, mut control: UnboundedReceiver<TurnCommand>) {
    let mut cmd = req.launch.command();
    cmd.current_dir(&req.cwd);
    let prompt = with_file_mentions(&req, req.prompt_for_arg(24_000));
    cmd.args(["--output-format", "stream-json"]);
    if let Some(id) = &req.resume_id {
        cmd.args(["--resume", id]);
    }
    if let Some(m) = &req.model {
        cmd.args(["--model", m]);
    }
    cmd.args(&req.modes.args);
    cmd.args(&req.launch.extra_args);
    cmd.args(["-p", &prompt]);
    let mut parser = ClaudeStream::default();
    run_stream("Qwen Code", cmd, None, &sink, &mut control, &mut parser).await;
}

/// Incremental parser for Claude Code stream-json.
#[derive(Default)]
pub struct ClaudeStream {
    pub done: bool,
    failed: bool,
    /// Message ids whose text/thinking already arrived as partial deltas.
    streamed: HashSet<String>,
    current_msg: Option<String>,
    seen_tools: HashSet<String>,
    /// Background task id (shell id / async agent id) → tool call id.
    bg_by_task: HashMap<String, String>,
    /// Tool calls started with `run_in_background`.
    bg_tools: HashSet<String>,
    /// TaskStop / KillShell call id → task id it targets.
    stop_calls: HashMap<String, String>,
    /// BashOutput / TaskOutput call id → task id it reads.
    output_calls: HashMap<String, String>,
    /// Tool ids already seen inside subagents.
    side_tools: HashSet<String>,
}

impl LineParser for ClaudeStream {
    fn handle(&mut self, line: &str, sink: &EventSink) {
        let Some(v) = json_line(line, sink) else { return };
        // Subagent (Task / Agent) traffic carries parent_tool_use_id: record
        // it into the parent tool's nested transcript.
        if let Some(parent) = s(&v, "parent_tool_use_id").filter(|p| !p.is_empty()) {
            let parent = parent.to_string();
            self.side(&v, &parent, sink);
            return;
        }
        match s(&v, "type").unwrap_or("") {
            "system" => self.system(&v, sink),
            "stream_event" => self.stream_event(&v["event"], sink),
            "assistant" => self.assistant(&v, sink),
            "user" => self.user(&v, sink),
            "result" => self.result(&v, sink),
            _ => {}
        }
    }

    fn done(&self) -> bool {
        self.done
    }

    fn failed(&self) -> bool {
        self.failed
    }
}

impl ClaudeStream {
    fn system(&mut self, v: &Value, sink: &EventSink) {
        match s(v, "subtype").unwrap_or("") {
            "init" => {
                if let Some(id) = s(v, "session_id") {
                    sink.send(AgentEvent::NativeSession { id: id.to_string(), path: None });
                }
                if let Some(m) = s(v, "model") {
                    sink.send(AgentEvent::Model { model: m.to_string() });
                }
            }
            "compact_boundary" => sink.notice("Context compacted"),
            "api_retry" => sink.log(format!(
                "API retry {}/{}: {}",
                u(v, "attempt"),
                u(v, "max_retries"),
                s(v, "error").unwrap_or("")
            )),
            "permission_denied" => {
                if let Some(id) = s(v, "tool_use_id") {
                    sink.send(AgentEvent::ToolUpdate {
                        id: id.to_string(),
                        status: Some(ToolStatus::Failed),
                        output: Some(s(v, "message").unwrap_or("Permission denied").to_string()),
                        diffs: Vec::new(),
                        is_error: true,
                        title: None,
                    });
                }
            }
            _ => sink.log(v.to_string()),
        }
    }

    fn stream_event(&mut self, ev: &Value, sink: &EventSink) {
        match s(ev, "type").unwrap_or("") {
            "message_start" => {
                self.current_msg = ev["message"]["id"].as_str().map(|s| s.to_string());
            }
            "content_block_delta" => {
                let d = &ev["delta"];
                match s(d, "type").unwrap_or("") {
                    "text_delta" => {
                        if let Some(t) = d["text"].as_str() {
                            self.mark_streamed();
                            sink.text(t);
                        }
                    }
                    "thinking_delta" => {
                        if let Some(t) = d["thinking"].as_str() {
                            self.mark_streamed();
                            sink.send(AgentEvent::ReasoningDelta { text: t.to_string() });
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn assistant(&mut self, v: &Value, sink: &EventSink) {
        let msg = &v["message"];
        if let Some(err) = s(v, "error") {
            let text = content_text(&msg["content"]);
            sink.error(if text.is_empty() { format!("API error: {err}") } else { text });
            return;
        }
        let mid = s(msg, "id").unwrap_or("").to_string();
        let streamed = self.streamed.contains(&mid);
        let Some(blocks) = msg.get("content").and_then(|c| c.as_array()) else { return };
        for b in blocks {
            match s(b, "type").unwrap_or("") {
                "text" if !streamed => {
                    if let Some(t) = b["text"].as_str() {
                        sink.send(AgentEvent::Text { text: t.to_string() });
                    }
                }
                "thinking" if !streamed => {
                    if let Some(t) = b["thinking"].as_str() {
                        sink.send(AgentEvent::Reasoning { text: t.to_string(), duration_ms: None });
                    }
                }
                "tool_use" | "server_tool_use" => {
                    let id = s(b, "id").unwrap_or("").to_string();
                    if self.seen_tools.insert(id.clone()) {
                        let name = s(b, "name").unwrap_or("tool");
                        let input = b.get("input").cloned().unwrap_or(Value::Null);
                        if name == "TodoWrite" {
                            if let Some(entries) = input.get("todos").and_then(plan_entries) {
                                sink.send(AgentEvent::Plan { entries });
                            }
                        }
                        self.track_background(&id, name, &input);
                        sink.send(AgentEvent::ToolStart { tool: tool(id, name, input) });
                    }
                }
                _ => {}
            }
        }
    }

    fn user(&mut self, v: &Value, sink: &EventSink) {
        let meta = v.get("tool_use_result").cloned();
        let content = &v["message"]["content"];
        // Background task notifications arrive as plain user text.
        if let Some(t) = content.as_str() {
            self.task_notifications(t, sink);
            return;
        }
        let Some(blocks) = content.as_array() else { return };
        for b in blocks {
            match s(b, "type") {
                Some("text") => self.task_notifications(s(b, "text").unwrap_or(""), sink),
                Some("tool_result") => {
                    let id = s(b, "tool_use_id").unwrap_or("").to_string();
                    let is_error = b.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false);
                    let output = content_text(&b["content"]);
                    let diffs = meta.as_ref().map(diffs_from_result).unwrap_or_default();
                    self.tool_result(id, output, is_error, diffs, meta.as_ref(), sink);
                }
                _ => {}
            }
        }
    }

    fn tool_result(&mut self, id: String, output: String, is_error: bool, diffs: Vec<FileDiff>, meta: Option<&Value>, sink: &EventSink) {
        // Subagent facts from the Agent / Task result.
        if let Some(m) = meta.filter(|m| m.is_object()) {
            let dur = m.get("totalDurationMs").and_then(|d| d.as_u64());
            let agent_name = s(m, "agentType").map(|a| a.to_string());
            let is_async = m.get("isAsync").and_then(|a| a.as_bool()).unwrap_or(false) || s(m, "status") == Some("async_launched");
            if dur.is_some() || agent_name.is_some() || is_async {
                sink.send(AgentEvent::ToolMeta { id: id.clone(), duration_ms: dur, background: is_async.then_some(true), agent_name });
            }
            if is_async && !is_error {
                self.bg_tools.insert(id.clone());
                if let Some(a) = s(m, "agentId") {
                    self.bg_by_task.insert(a.to_string(), id.clone());
                }
            }
            if let Some(t) = s(m, "backgroundTaskId") {
                self.bg_by_task.insert(t.to_string(), id.clone());
            }
        }
        if self.bg_tools.contains(&id) && !is_error {
            // Launched in the background: keep it running until a
            // notification / TaskStop / BashOutput says otherwise.
            if let Some(task) = background_task_id(&output) {
                self.bg_by_task.entry(task).or_insert_with(|| id.clone());
            }
            sink.send(AgentEvent::ToolUpdate { id, status: Some(ToolStatus::Running), output: Some(output), diffs, is_error: false, title: None });
            return;
        }
        if let Some(task) = self.stop_calls.remove(&id) {
            if !is_error {
                if let Some(target) = self.bg_by_task.get(&task).cloned() {
                    sink.send(AgentEvent::ToolUpdate { id: target, status: Some(ToolStatus::Cancelled), output: None, diffs: Vec::new(), is_error: false, title: None });
                }
            }
        }
        if let Some(task) = self.output_calls.remove(&id) {
            if let (Some(target), Some(state)) = (self.bg_by_task.get(&task).cloned(), output_status(&output)) {
                sink.send(AgentEvent::ToolUpdate { id: target, status: Some(state), output: Some(output.clone()), diffs: Vec::new(), is_error: false, title: None });
            }
        }
        sink.send(AgentEvent::ToolUpdate {
            id,
            status: Some(if is_error { ToolStatus::Failed } else { ToolStatus::Completed }),
            output: Some(output),
            diffs,
            is_error,
            title: None,
        });
    }

    /// Remember calls that start, read or stop background work.
    fn track_background(&mut self, id: &str, name: &str, input: &Value) {
        if input.get("run_in_background").and_then(|b| b.as_bool()).unwrap_or(false) || name == "Monitor" {
            self.bg_tools.insert(id.to_string());
        }
        let target = ["task_id", "shell_id", "bash_id", "agent_id", "id"].iter().find_map(|k| s(input, k)).map(|t| t.to_string());
        if let Some(t) = target {
            match name {
                "TaskStop" | "KillShell" | "KillBash" => {
                    self.stop_calls.insert(id.to_string(), t);
                }
                "BashOutput" | "TaskOutput" => {
                    self.output_calls.insert(id.to_string(), t);
                }
                _ => {}
            }
        }
    }

    /// `<task-notification>` blocks report background work finishing.
    fn task_notifications(&mut self, text: &str, sink: &EventSink) {
        for n in parse_task_notifications(text) {
            let target = n.tool_use_id.clone().or_else(|| n.task_id.as_ref().and_then(|t| self.bg_by_task.get(t).cloned()));
            let Some(target) = target else { continue };
            let status = match n.status.as_deref().unwrap_or("completed") {
                "failed" | "error" => ToolStatus::Failed,
                "killed" | "stopped" | "cancelled" | "canceled" => ToolStatus::Cancelled,
                "running" => ToolStatus::Running,
                _ => ToolStatus::Completed,
            };
            sink.send(AgentEvent::ToolUpdate { id: target, status: Some(status), output: n.summary.clone(), diffs: Vec::new(), is_error: false, title: None });
        }
    }

    /// An event inside a subagent: wrap it for the parent tool.
    fn side(&mut self, v: &Value, parent: &str, sink: &EventSink) {
        let wrap = |e: AgentEvent| AgentEvent::Subagent { parent_tool_id: parent.to_string(), inner: Box::new(e) };
        match s(v, "type").unwrap_or("") {
            "assistant" => {
                let msg = &v["message"];
                let Some(blocks) = msg.get("content").and_then(|c| c.as_array()) else { return };
                for b in blocks {
                    match s(b, "type").unwrap_or("") {
                        "text" => {
                            if let Some(t) = b["text"].as_str().filter(|t| !t.trim().is_empty()) {
                                sink.send(wrap(AgentEvent::Text { text: t.to_string() }));
                            }
                        }
                        "thinking" => {
                            if let Some(t) = b["thinking"].as_str().filter(|t| !t.trim().is_empty()) {
                                sink.send(wrap(AgentEvent::Reasoning { text: t.to_string(), duration_ms: None }));
                            }
                        }
                        "tool_use" | "server_tool_use" => {
                            let id = s(b, "id").unwrap_or("").to_string();
                            if self.side_tools.insert(id.clone()) {
                                let name = s(b, "name").unwrap_or("tool");
                                let input = b.get("input").cloned().unwrap_or(Value::Null);
                                sink.send(wrap(AgentEvent::ToolStart { tool: tool(id, name, input) }));
                            }
                        }
                        _ => {}
                    }
                }
            }
            "user" => {
                let meta = v.get("tool_use_result").cloned();
                let Some(blocks) = v["message"].get("content").and_then(|c| c.as_array()) else { return };
                for b in blocks {
                    if s(b, "type") != Some("tool_result") {
                        continue;
                    }
                    let id = s(b, "tool_use_id").unwrap_or("").to_string();
                    let is_error = b.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false);
                    let diffs = meta.as_ref().map(diffs_from_result).unwrap_or_default();
                    sink.send(wrap(AgentEvent::ToolUpdate {
                        id,
                        status: Some(if is_error { ToolStatus::Failed } else { ToolStatus::Completed }),
                        output: Some(content_text(&b["content"])),
                        diffs,
                        is_error,
                        title: None,
                    }));
                }
            }
            _ => {}
        }
    }

    fn result(&mut self, v: &Value, sink: &EventSink) {
        let us = &v["usage"];
        if us.is_object() {
            sink.send(AgentEvent::Usage {
                usage: usage_from(
                    u(us, "input_tokens"),
                    u(us, "output_tokens"),
                    u(us, "cache_read_input_tokens"),
                    u(us, "cache_creation_input_tokens"),
                    0,
                    v["total_cost_usd"].as_f64(),
                ),
            });
        }
        if let Some(id) = s(v, "session_id") {
            sink.send(AgentEvent::NativeSession { id: id.to_string(), path: None });
        }
        let is_error = v.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false);
        let subtype = s(v, "subtype").unwrap_or("success").to_string();
        if is_error {
            let errors = v
                .get("errors")
                .and_then(|e| e.as_array())
                .map(|a| a.iter().map(content_text).collect::<Vec<_>>().join("\n"))
                .filter(|s| !s.is_empty());
            let msg = s(v, "result").map(|s| s.to_string()).filter(|s| !s.is_empty()).or(errors).unwrap_or_else(|| subtype.replace('_', " "));
            sink.error(msg);
            self.failed = true;
        }
        self.done = true;
        sink.done(!is_error, Some(subtype));
    }

    fn mark_streamed(&mut self) {
        if let Some(id) = &self.current_msg {
            self.streamed.insert(id.clone());
        }
    }
}

/// One `<task-notification>` block.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct TaskNotification {
    pub task_id: Option<String>,
    pub tool_use_id: Option<String>,
    pub status: Option<String>,
    pub summary: Option<String>,
}

fn tag(block: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let i = block.find(&open)? + open.len();
    let j = block[i..].find(&close)? + i;
    Some(block[i..j].trim().to_string()).filter(|v| !v.is_empty())
}

/// Parse every `<task-notification>` in a message (Claude Code's report of a
/// background shell / agent / monitor finishing).
pub fn parse_task_notifications(text: &str) -> Vec<TaskNotification> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("<task-notification>") {
        let after = &rest[i + "<task-notification>".len()..];
        let end = after.find("</task-notification>").unwrap_or(after.len());
        let block = &after[..end];
        out.push(TaskNotification {
            task_id: tag(block, "task-id"),
            tool_use_id: tag(block, "tool-use-id"),
            status: tag(block, "status").map(|s| s.to_lowercase()),
            summary: tag(block, "summary").or_else(|| tag(block, "result")),
        });
        rest = &after[end..];
    }
    out
}

/// "Command running in background with ID: b8b97ntfm." → `b8b97ntfm`.
pub fn background_task_id(output: &str) -> Option<String> {
    let i = output.find("with ID:")? + "with ID:".len();
    let id: String = output[i..].trim_start().chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect();
    (!id.is_empty()).then_some(id)
}

/// Status reported by BashOutput / TaskOutput (`<status>completed</status>`).
fn output_status(output: &str) -> Option<ToolStatus> {
    let st = tag(output, "status").or_else(|| {
        let i = output.to_ascii_lowercase().find("status:")? + "status:".len();
        Some(output[i..].split_whitespace().next()?.trim_matches(|c: char| !c.is_ascii_alphanumeric()).to_lowercase())
    })?;
    Some(match st.as_str() {
        "running" => ToolStatus::Running,
        "failed" | "error" => ToolStatus::Failed,
        "killed" | "stopped" | "cancelled" => ToolStatus::Cancelled,
        "completed" | "done" | "exited" => ToolStatus::Completed,
        _ => return None,
    })
}

/// Kept for callers that used the old free function.
pub fn tool_result_text(content: &Value) -> String {
    content_text(content)
}

/// Claude's `tool_use_result` carries structured edit info.
fn diffs_from_result(meta: &Value) -> Vec<FileDiff> {
    let Some(path) = meta.get("filePath").and_then(|p| p.as_str()) else { return Vec::new() };
    if let Some(hunks) = meta.get("structuredPatch").and_then(|h| h.as_array()) {
        let mut unified = String::new();
        for h in hunks {
            unified.push_str(&format!(
                "@@ -{},{} +{},{} @@\n",
                u(h, "oldStart"),
                u(h, "oldLines"),
                u(h, "newStart"),
                u(h, "newLines")
            ));
            if let Some(lines) = h["lines"].as_array() {
                for l in lines {
                    unified.push_str(l.as_str().unwrap_or(""));
                    unified.push('\n');
                }
            }
        }
        if !unified.is_empty() {
            let mut d = FileDiff::from_unified(path, unified);
            d.old_text = meta.get("originalFile").and_then(|s| s.as_str()).map(|s| s.to_string());
            return vec![d];
        }
    }
    if meta.get("type").and_then(|t| t.as_str()) == Some("create") {
        let content = meta.get("content").and_then(|c| c.as_str()).map(|s| s.to_string());
        return vec![FileDiff::from_texts(path, None, content)];
    }
    Vec::new()
}

/// Back-compat helper used by older callers.
pub fn todo_entries(input: &Value) -> Option<Vec<larrier_core::PlanEntry>> {
    input.get("todos").and_then(plan_entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::testutil::collect;
    use larrier_core::ToolKind;

    #[test]
    fn parses_partial_stream_without_duplicates() {
        let lines = [
            r#"{"type":"system","subtype":"init","session_id":"s-1","model":"claude-opus-5-5","cwd":"D:\\p","tools":[]}"#,
            r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"msg_1"}},"parent_tool_use_id":null}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}},"parent_tool_use_id":null}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Hel"}},"parent_tool_use_id":null}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"lo"}},"parent_tool_use_id":null}"#,
            r#"{"type":"assistant","parent_tool_use_id":null,"message":{"id":"msg_1","role":"assistant","content":[{"type":"text","text":"Hello"},{"type":"tool_use","id":"toolu_1","name":"Edit","input":{"file_path":"src/a.rs","old_string":"a","new_string":"b\nc"}}]}}"#,
            r#"{"type":"user","parent_tool_use_id":null,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":[{"type":"text","text":"ok"}],"is_error":false}]},"tool_use_result":{"filePath":"src/a.rs","structuredPatch":[{"oldStart":1,"oldLines":1,"newStart":1,"newLines":2,"lines":["-a","+b","+c"]}]}}"#,
            r#"{"type":"assistant","parent_tool_use_id":"toolu_9","message":{"id":"msg_side","content":[{"type":"text","text":"side"}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"result":"Hello","session_id":"s-1","total_cost_usd":0.12,"usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":100,"cache_creation_input_tokens":3}}"#,
        ];
        let mut p = ClaudeStream::default();
        let ev = collect(&mut p, &lines);
        // The subagent line is wrapped, not mixed into the main transcript.
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Subagent { parent_tool_id, inner } if parent_tool_id == "toolu_9" && matches!(&**inner, AgentEvent::Text { text } if text == "side"))));
        let ev: Vec<AgentEvent> = ev.into_iter().filter(|e| !matches!(e, AgentEvent::Subagent { .. })).collect();
        assert!(matches!(&ev[0], AgentEvent::NativeSession { id, .. } if id == "s-1"));
        assert!(matches!(&ev[1], AgentEvent::Model { model } if model == "claude-opus-5-5"));
        assert!(matches!(&ev[2], AgentEvent::ReasoningDelta { text } if text == "hmm"));
        let texts: Vec<_> = ev.iter().filter(|e| matches!(e, AgentEvent::Text { .. } | AgentEvent::TextDelta { .. })).collect();
        assert_eq!(texts.len(), 2, "no duplicate full text after deltas");
        let start = ev.iter().find_map(|e| match e { AgentEvent::ToolStart { tool } => Some(tool), _ => None }).unwrap();
        assert_eq!(start.kind, ToolKind::Edit);
        assert_eq!((start.diffs[0].added, start.diffs[0].removed), (2, 1));
        let upd = ev.iter().find_map(|e| match e { AgentEvent::ToolUpdate { diffs, output, .. } => Some((diffs, output)), _ => None }).unwrap();
        assert_eq!(upd.1.as_deref(), Some("ok"));
        assert_eq!(upd.0[0].added, 2);
        assert!(matches!(ev.iter().find(|e| matches!(e, AgentEvent::Usage { .. })), Some(AgentEvent::Usage { usage }) if usage.cost_usd == Some(0.12) && usage.cache_read_tokens == 100));
        assert!(matches!(ev.last(), Some(AgentEvent::Done { ok: true, .. })));
        assert!(p.done);
    }

    #[test]
    fn error_result_and_permission_denied() {
        let lines = [
            r#"{"type":"assistant","parent_tool_use_id":null,"message":{"id":"m","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"rm -rf x"}}]}}"#,
            r#"{"type":"system","subtype":"permission_denied","tool_name":"Bash","tool_use_id":"t1","message":"denied by policy"}"#,
            r#"{"type":"result","subtype":"error_max_turns","is_error":true,"session_id":"s","errors":["Reached max turns"]}"#,
        ];
        let mut p = ClaudeStream::default();
        let ev = collect(&mut p, &lines);
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolUpdate { is_error: true, .. })));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Error { message } if message.contains("max turns"))));
        assert!(matches!(ev.last(), Some(AgentEvent::Done { ok: false, .. })));
    }

    #[test]
    fn subagent_traffic_and_agent_result_meta() {
        let lines = [
            r#"{"type":"assistant","parent_tool_use_id":null,"message":{"id":"m1","content":[{"type":"tool_use","id":"agent_1","name":"Agent","input":{"description":"Audit","prompt":"look","subagent_type":"Explore"}}]}}"#,
            r#"{"type":"user","parent_tool_use_id":"agent_1","message":{"role":"user","content":"look"}}"#,
            r#"{"type":"assistant","parent_tool_use_id":"agent_1","message":{"id":"s1","content":[{"type":"thinking","thinking":"plan"},{"type":"tool_use","id":"t_read","name":"Read","input":{"file_path":"src/a.rs"}}]}}"#,
            r#"{"type":"user","parent_tool_use_id":"agent_1","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t_read","content":"fn a()"}]}}"#,
            r#"{"type":"assistant","parent_tool_use_id":"agent_1","message":{"id":"s2","content":[{"type":"text","text":"Found it"}]}}"#,
            r#"{"type":"user","parent_tool_use_id":null,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"agent_1","content":[{"type":"text","text":"Found it"}]}]},"tool_use_result":{"agentId":"a1","agentType":"Explore","status":"completed","totalDurationMs":5300,"totalTokens":900}}"#,
        ];
        let mut p = ClaudeStream::default();
        let ev = collect(&mut p, &lines);
        let side: Vec<&AgentEvent> = ev.iter().filter_map(|e| match e { AgentEvent::Subagent { parent_tool_id, inner } if parent_tool_id == "agent_1" => Some(&**inner), _ => None }).collect();
        assert!(matches!(side[0], AgentEvent::Reasoning { text, .. } if text == "plan"));
        assert!(matches!(side[1], AgentEvent::ToolStart { tool } if tool.id == "t_read"));
        assert!(matches!(side[2], AgentEvent::ToolUpdate { id, output: Some(o), .. } if id == "t_read" && o == "fn a()"));
        assert!(matches!(side[3], AgentEvent::Text { text } if text == "Found it"));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolMeta { id, duration_ms: Some(5300), agent_name: Some(n), .. } if id == "agent_1" && n == "Explore")));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolUpdate { id, status: Some(ToolStatus::Completed), .. } if id == "agent_1")));
    }

    #[test]
    fn background_shell_lifecycle() {
        let lines = [
            r#"{"type":"assistant","parent_tool_use_id":null,"message":{"id":"m1","content":[{"type":"tool_use","id":"bg1","name":"Bash","input":{"command":"npm run dev","run_in_background":true}}]}}"#,
            r#"{"type":"user","parent_tool_use_id":null,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"bg1","content":"Command running in background with ID: b8b97ntfm. Output is being written to: /tmp/x.output"}]},"tool_use_result":{"stdout":"","stderr":"","backgroundTaskId":"b8b97ntfm","interrupted":false}}"#,
            r#"{"type":"assistant","parent_tool_use_id":null,"message":{"id":"m2","content":[{"type":"tool_use","id":"bg2","name":"Bash","input":{"command":"cargo build","run_in_background":true}}]}}"#,
            r#"{"type":"user","parent_tool_use_id":null,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"bg2","content":"Command running in background with ID: c2"}]}}"#,
            r#"{"type":"user","parent_tool_use_id":null,"message":{"role":"user","content":"<task-notification>\n<task-id>c2</task-id>\n<status>completed</status>\n<summary>Build finished</summary>\n</task-notification>"}}"#,
            r#"{"type":"assistant","parent_tool_use_id":null,"message":{"id":"m3","content":[{"type":"tool_use","id":"stop1","name":"TaskStop","input":{"task_id":"b8b97ntfm"}}]}}"#,
            r#"{"type":"user","parent_tool_use_id":null,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"stop1","content":"stopped"}]}}"#,
        ];
        let mut p = ClaudeStream::default();
        let ev = collect(&mut p, &lines);
        let ups: Vec<(&str, ToolStatus)> = ev
            .iter()
            .filter_map(|e| match e { AgentEvent::ToolUpdate { id, status: Some(st), .. } => Some((id.as_str(), *st)), _ => None })
            .collect();
        assert_eq!(ups[0], ("bg1", ToolStatus::Running), "background launch stays running");
        assert_eq!(ups[1], ("bg2", ToolStatus::Running));
        assert!(ups.contains(&("bg2", ToolStatus::Completed)), "notification completes bg2");
        assert!(ups.contains(&("bg1", ToolStatus::Cancelled)), "TaskStop cancels bg1");
        assert!(matches!(ev.iter().find(|e| matches!(e, AgentEvent::ToolStart { tool } if tool.id == "bg1")), Some(AgentEvent::ToolStart { tool }) if tool.background));
    }

    #[test]
    fn notification_and_output_parsing() {
        let n = parse_task_notifications("x <task-notification><task-id>a</task-id><tool-use-id>toolu_1</tool-use-id><status>Failed</status><summary>boom</summary></task-notification> y");
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].tool_use_id.as_deref(), Some("toolu_1"));
        assert_eq!(n[0].status.as_deref(), Some("failed"));
        assert_eq!(background_task_id("Command running in background with ID: b8b97ntfm. Output"), Some("b8b97ntfm".into()));
        assert_eq!(output_status("<status>completed</status>\n<stdout>ok</stdout>"), Some(ToolStatus::Completed));
        assert_eq!(output_status("Status: running"), Some(ToolStatus::Running));
    }

}
