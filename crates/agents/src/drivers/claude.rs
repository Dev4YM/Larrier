//! Claude Code (`claude -p --output-format stream-json --verbose
//! --include-partial-messages`, prompt on stdin), plus the agents that speak
//! the same event shapes: Amp (`amp -x --stream-json`) and Qwen Code
//! (`qwen -p --output-format stream-json`).

use std::collections::HashSet;

use larrier_core::{AgentKind, FileDiff, RunMode, ToolStatus};
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
        cmd.args(["--permission-mode", claude_permission_mode(req.mode.as_deref(), req.run_mode)]);
        // Claude Code refuses to start nested inside another Claude Code session.
        cmd.env_remove("CLAUDECODE");
    }
    cmd.args(&req.launch.extra_args);
    let prompt = with_file_mentions(&req, req.full_prompt());
    let mut parser = ClaudeStream::default();
    run_stream("Claude Code", cmd, Some(prompt), &sink, &mut control, &mut parser).await;
}

/// Since v2.1.302 `-p` runs default to manual approval, which denies any
/// tool that would prompt. Larrier's "Ask" therefore maps to acceptEdits
/// (switch the agent to ACP in Settings for interactive approvals).
pub fn claude_permission_mode(mode: Option<&str>, run_mode: RunMode) -> &'static str {
    match mode {
        Some("plan") => "plan",
        Some("bypass") => "bypassPermissions",
        Some("auto") => "auto",
        Some("accept-edits") => "acceptEdits",
        _ => match run_mode {
            RunMode::FullAuto => "bypassPermissions",
            RunMode::Ask | RunMode::AutoEdit => "acceptEdits",
        },
    }
}

pub async fn run_amp(req: TurnRequest, sink: EventSink, mut control: UnboundedReceiver<TurnCommand>) {
    let mut cmd = req.launch.command();
    cmd.current_dir(&req.cwd);
    if let Some(id) = &req.resume_id {
        cmd.args(["threads", "continue", id]);
    }
    let prompt = with_file_mentions(&req, req.prompt_for_arg(24_000));
    cmd.args(["-x", &prompt, "--stream-json"]);
    if req.run_mode == RunMode::FullAuto {
        cmd.arg("--dangerously-allow-all");
    }
    if let Some(m) = req.mode.as_deref().filter(|m| *m != "smart") {
        cmd.args(["--mode", m]);
    }
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
    let approval = match (req.mode.as_deref(), req.run_mode) {
        (Some("plan"), _) => "plan",
        (Some("yolo"), _) | (_, RunMode::FullAuto) => "yolo",
        (Some("auto-edit"), _) | (_, RunMode::AutoEdit) => "auto-edit",
        _ => "default",
    };
    cmd.args(["--approval-mode", approval]);
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
}

impl LineParser for ClaudeStream {
    fn handle(&mut self, line: &str, sink: &EventSink) {
        let Some(v) = json_line(line, sink) else { return };
        // Subagent (Task) traffic carries parent_tool_use_id; keep the main
        // transcript clean and surface it only through the Task tool result.
        let is_side = v.get("parent_tool_use_id").map(|p| !p.is_null()).unwrap_or(false);
        match s(&v, "type").unwrap_or("") {
            "system" => self.system(&v, sink),
            "stream_event" if !is_side => self.stream_event(&v["event"], sink),
            "assistant" if !is_side => self.assistant(&v, sink),
            "user" if !is_side => self.user(&v, sink),
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
                        sink.send(AgentEvent::ToolStart { tool: tool(id, name, input) });
                    }
                }
                _ => {}
            }
        }
    }

    fn user(&mut self, v: &Value, sink: &EventSink) {
        let meta = v.get("tool_use_result").cloned();
        let Some(blocks) = v["message"].get("content").and_then(|c| c.as_array()) else { return };
        for b in blocks {
            if s(b, "type") != Some("tool_result") {
                continue;
            }
            let id = s(b, "tool_use_id").unwrap_or("").to_string();
            let is_error = b.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false);
            let output = content_text(&b["content"]);
            let diffs = meta.as_ref().map(diffs_from_result).unwrap_or_default();
            sink.send(AgentEvent::ToolUpdate {
                id,
                status: Some(if is_error { ToolStatus::Failed } else { ToolStatus::Completed }),
                output: Some(output),
                diffs,
                is_error,
                title: None,
            });
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
    fn permission_modes() {
        assert_eq!(claude_permission_mode(None, RunMode::Ask), "acceptEdits");
        assert_eq!(claude_permission_mode(None, RunMode::FullAuto), "bypassPermissions");
        assert_eq!(claude_permission_mode(Some("plan"), RunMode::FullAuto), "plan");
    }
}
