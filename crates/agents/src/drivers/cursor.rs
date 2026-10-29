//! Cursor CLI: `cursor-agent -p --output-format stream-json
//! --stream-partial-output --trust --workspace <cwd> … <prompt>`.
//! (Never the bare `agent` binary — on some machines that's another CLI.)

use std::collections::HashSet;

use larrier_core::{FileDiff, ToolKind, ToolStatus};
use serde_json::{Map, Value};
use tokio::sync::mpsc::UnboundedReceiver;

use super::{content_text, json_line, plan_entries, run_stream, s, tool, tool_of_kind, with_file_mentions, LineParser};
use crate::events::{AgentEvent, EventSink, TurnCommand};
use crate::request::TurnRequest;

pub async fn run(req: TurnRequest, sink: EventSink, mut control: UnboundedReceiver<TurnCommand>) {
    let mut cmd = req.launch.command();
    cmd.current_dir(&req.cwd);
    cmd.args(["-p", "--output-format", "stream-json", "--stream-partial-output", "--trust"]);
    cmd.arg("--workspace").arg(&req.cwd);
    if let Some(id) = &req.resume_id {
        cmd.args(["--resume", id]);
    }
    if let Some(m) = &req.model {
        cmd.args(["--model", m]);
    }
    // --mode plan|ask and --force / --auto-review from the chosen modes.
    // Print mode can already write files; --force also runs shell commands.
    cmd.args(&req.modes.args);
    cmd.args(&req.launch.extra_args);
    cmd.arg(with_file_mentions(&req, req.prompt_for_arg(24_000)));
    let mut parser = CursorStream::default();
    run_stream("Cursor Agent", cmd, None, &sink, &mut control, &mut parser).await;
}

#[derive(Default)]
pub struct CursorStream {
    done: bool,
    failed: bool,
    /// Text deltas arrived since the last flush.
    deltas: bool,
    thinking: bool,
    started: HashSet<String>,
}

impl LineParser for CursorStream {
    fn handle(&mut self, line: &str, sink: &EventSink) {
        let Some(v) = json_line(line, sink) else { return };
        match s(&v, "type").unwrap_or("") {
            "system" => {
                if s(&v, "subtype") == Some("init") {
                    if let Some(id) = s(&v, "session_id") {
                        sink.send(AgentEvent::NativeSession { id: id.to_string(), path: None });
                    }
                    if let Some(m) = s(&v, "model") {
                        sink.send(AgentEvent::Model { model: m.to_string() });
                    }
                }
            }
            "assistant" => {
                let text = content_text(&v["message"]["content"]);
                let ts = v.get("timestamp_ms").is_some();
                let call = v.get("model_call_id").is_some();
                match (ts, call) {
                    // A real streaming delta.
                    (true, false) => {
                        if !text.is_empty() {
                            self.deltas = true;
                            sink.text(text);
                        }
                    }
                    // Flush before a tool call / final flush: duplicates of
                    // what was streamed — only use them when nothing was.
                    _ => {
                        if !self.deltas && !text.trim().is_empty() {
                            sink.send(AgentEvent::Text { text });
                        }
                        self.deltas = false;
                    }
                }
            }
            "thinking" => {
                let text = s(&v, "text").map(|t| t.to_string()).unwrap_or_else(|| content_text(&v["message"]["content"]));
                match s(&v, "subtype").unwrap_or("delta") {
                    "completed" | "done" => self.thinking = false,
                    _ => {
                        if !text.is_empty() {
                            self.thinking = true;
                            sink.send(AgentEvent::ReasoningDelta { text });
                        }
                    }
                }
            }
            "tool_call" => self.tool_call(&v, sink),
            "result" => {
                if let Some(id) = s(&v, "session_id") {
                    sink.send(AgentEvent::NativeSession { id: id.to_string(), path: None });
                }
                let is_error = v.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false) || s(&v, "subtype").map(|s| s != "success").unwrap_or(false);
                if is_error {
                    sink.error(s(&v, "result").unwrap_or("Cursor Agent failed").to_string());
                    self.failed = true;
                }
                self.done = true;
                sink.done(!is_error, s(&v, "subtype").map(|s| s.to_string()));
            }
            "error" => {
                sink.error(s(&v, "message").or_else(|| s(&v, "error")).unwrap_or("error").to_string());
                self.failed = true;
            }
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

impl CursorStream {
    fn tool_call(&mut self, v: &Value, sink: &EventSink) {
        let id = s(v, "call_id").unwrap_or("call").to_string();
        let Some((key, body)) = v["tool_call"].as_object().and_then(|m| m.iter().next()) else { return };
        let raw = key.trim_end_matches("ToolCall");
        let args = body.get("args").cloned().unwrap_or(Value::Object(Map::new()));
        let (name, kind) = match raw {
            "shell" | "terminal" | "runTerminalCmd" => ("shell", ToolKind::Shell),
            "read" | "readFile" => ("read", ToolKind::Read),
            "ls" | "listDir" => ("ls", ToolKind::Read),
            "glob" | "fileSearch" => ("glob", ToolKind::Search),
            "grep" | "semanticSearch" | "codebaseSearch" => ("grep", ToolKind::Search),
            "edit" | "searchReplace" | "multiEdit" => ("edit", ToolKind::Edit),
            "write" | "createFile" => ("write", ToolKind::Write),
            "delete" | "deleteFile" => ("delete", ToolKind::Delete),
            "updateTodos" | "todoWrite" => ("todo_write", ToolKind::Todo),
            "webSearch" => ("web_search", ToolKind::WebSearch),
            "webFetch" | "fetch" => ("web_fetch", ToolKind::Fetch),
            "mcp" => ("mcp", ToolKind::Mcp),
            "task" | "subagent" => ("task", ToolKind::Task),
            other => (other, ToolKind::classify(other)),
        };
        if kind == ToolKind::Todo {
            if let Some(entries) = args.get("todos").and_then(plan_entries) {
                sink.send(AgentEvent::Plan { entries });
            }
        }
        if self.started.insert(id.clone()) {
            let mut input = args.clone();
            if kind == ToolKind::Mcp {
                let tool_name = s(&args, "toolName").or_else(|| s(&args, "name")).unwrap_or("tool").to_string();
                let server = s(&args, "providerIdentifier").or_else(|| s(&args, "server")).unwrap_or("mcp").to_string();
                let t = tool_of_kind(id.clone(), &format!("mcp__{server}__{tool_name}"), ToolKind::Mcp, input);
                sink.send(AgentEvent::ToolStart { tool: t });
            } else {
                if let Value::Object(m) = &mut input {
                    if let Some(p) = m.get("path").cloned() {
                        m.entry("file_path").or_insert(p);
                    }
                }
                let t = if kind == ToolKind::classify(name) { tool(id.clone(), name, input) } else { tool_of_kind(id.clone(), name, kind, input) };
                sink.send(AgentEvent::ToolStart { tool: t });
            }
        }
        if s(v, "subtype") != Some("completed") {
            return;
        }
        let result = body.get("result").cloned().unwrap_or(Value::Null);
        let (status, output, is_error) = match result.as_object().and_then(|m| m.iter().next()) {
            Some((k, r)) if k == "success" => (ToolStatus::Completed, success_text(kind, r), false),
            Some((k, r)) => {
                let msg = s(r, "error").or_else(|| s(r, "message")).or_else(|| s(r, "reason")).map(|s| s.to_string()).unwrap_or_else(|| k.clone());
                (if k == "rejected" { ToolStatus::Cancelled } else { ToolStatus::Failed }, msg, k != "rejected")
            }
            None => (ToolStatus::Completed, String::new(), false),
        };
        let mut diffs = Vec::new();
        if let Some(r) = result.get("success") {
            let path = s(&args, "path").or_else(|| s(r, "path")).unwrap_or("").to_string();
            if let Some(d) = s(r, "diffString").or_else(|| s(r, "diff")) {
                diffs.push(FileDiff::from_unified(path, d.to_string()));
            } else if r.get("linesAdded").is_some() || r.get("linesRemoved").is_some() {
                diffs.push(FileDiff {
                    path,
                    added: r["linesAdded"].as_u64().unwrap_or(0) as u32,
                    removed: r["linesRemoved"].as_u64().unwrap_or(0) as u32,
                    ..Default::default()
                });
            }
        }
        sink.send(AgentEvent::ToolUpdate { id, status: Some(status), output: Some(output), diffs, is_error, title: None });
    }
}

fn success_text(kind: ToolKind, r: &Value) -> String {
    match kind {
        ToolKind::Shell => {
            let mut out = s(r, "stdout").unwrap_or("").to_string();
            let err = s(r, "stderr").unwrap_or("");
            if !err.is_empty() {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(err);
            }
            if let Some(code) = r.get("exitCode").and_then(|c| c.as_i64()).filter(|c| *c != 0) {
                out.push_str(&format!("\n[exit code {code}]"));
            }
            out
        }
        ToolKind::Read => s(r, "content").map(|s| s.to_string()).unwrap_or_else(|| content_text(r)),
        ToolKind::Edit | ToolKind::Write | ToolKind::Delete => String::new(),
        _ => {
            let t = content_text(r);
            if t == "{}" {
                String::new()
            } else {
                t
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::testutil::collect;

    #[test]
    fn dedupes_partial_output_and_maps_tools() {
        let lines = [
            r#"{"type":"system","subtype":"init","apiKeySource":"login","cwd":"D:\\p","session_id":"chat-1","model":"Composer 2","permissionMode":"default"}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"hi"}]},"session_id":"chat-1"}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Let me "}]},"session_id":"chat-1","timestamp_ms":1}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"look."}]},"session_id":"chat-1","timestamp_ms":2}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Let me look."}]},"session_id":"chat-1","timestamp_ms":3,"model_call_id":"mc1"}"#,
            r#"{"type":"tool_call","subtype":"started","call_id":"c1","tool_call":{"shellToolCall":{"args":{"command":"ls","workingDirectory":"D:\\p"}}},"session_id":"chat-1"}"#,
            r#"{"type":"tool_call","subtype":"completed","call_id":"c1","tool_call":{"shellToolCall":{"args":{"command":"ls"},"result":{"success":{"exitCode":0,"stdout":"a.txt\n","stderr":""}}}},"session_id":"chat-1"}"#,
            r#"{"type":"tool_call","subtype":"started","call_id":"c2","tool_call":{"editToolCall":{"args":{"path":"src/a.rs"}}}}"#,
            r#"{"type":"tool_call","subtype":"completed","call_id":"c2","tool_call":{"editToolCall":{"args":{"path":"src/a.rs"},"result":{"success":{"linesAdded":3,"linesRemoved":1}}}}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"All set."}]},"session_id":"chat-1"}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":10,"result":"Let me look.All set.","session_id":"chat-1"}"#,
        ];
        let mut p = CursorStream::default();
        let ev = collect(&mut p, &lines);
        let deltas: Vec<_> = ev.iter().filter_map(|e| match e { AgentEvent::TextDelta { text } => Some(text.clone()), _ => None }).collect();
        assert_eq!(deltas, vec!["Let me ".to_string(), "look.".into()]);
        // Final assistant message had no deltas before it → kept as Text.
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Text { text } if text == "All set.")));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolStart { tool } if tool.kind == ToolKind::Shell)));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolUpdate { output: Some(o), .. } if o.contains("a.txt"))));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolUpdate { diffs, .. } if diffs.first().map(|d| d.added) == Some(3))));
        assert!(matches!(ev.last(), Some(AgentEvent::Done { ok: true, .. })));
    }
}
