//! Cursor CLI: `cursor-agent -p --output-format stream-json
//! --stream-partial-output --trust --workspace <cwd> … <prompt>`.
//! (Never the bare `agent` binary — on some machines that's another CLI.)

use std::collections::HashSet;

use larrier_core::{FileDiff, ToolKind, ToolStatus};
use serde_json::{Map, Value};
use tokio::sync::mpsc::UnboundedReceiver;

use super::{content_text, json_line, plan_entries, run_stream, s, tool, tool_of_kind, u, usage_from, with_file_mentions, LineParser};
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
    // Model ids have no spaces; a display name ("Grok 4.7 256K Low", saved
    // by older versions from Cursor's own report) would be refused.
    if let Some(m) = req.model.as_ref().filter(|m| !m.trim().contains(char::is_whitespace)) {
        cmd.args(["--model", m]);
    }
    // --mode plan|ask and --force / --auto-review from the chosen modes.
    // Print mode can already write files; --force also runs shell commands.
    cmd.args(&req.modes.args);
    cmd.args(&req.launch.extra_args);
    cmd.arg(with_file_mentions(&req, req.prompt_for_arg(24_000)));
    let mut parser = CursorStream { workspace: Some(req.cwd.to_string_lossy().to_string()), ..Default::default() };
    run_stream("Cursor Agent", cmd, None, &sink, &mut control, &mut parser).await;
}

#[derive(Default)]
pub struct CursorStream {
    /// The `--workspace` path: the CLI stores the chat under it.
    workspace: Option<String>,
    done: bool,
    failed: bool,
    /// Text deltas arrived since the last flush.
    deltas: bool,
    thinking: bool,
    started: HashSet<String>,
    /// Text streamed since the last flush.
    segment: String,
    /// Between `connection: reconnecting` and `reconnected`.
    reconnecting: bool,
    /// After a `retry`, the model call restarts from its checkpoint and
    /// streams `segment` again: what is left to skip.
    replay: Option<String>,
}

impl CursorStream {
    fn delta(&mut self, text: &str, sink: &EventSink) {
        let mut text = text;
        if self.reconnecting {
            // A snapshot of the interrupted message: only what's new.
            match text.strip_prefix(self.segment.as_str()) {
                Some(rest) => text = rest,
                None if self.segment.starts_with(text) || self.segment.ends_with(text) => return,
                None => {}
            }
        } else if let Some(rep) = self.replay.take() {
            let common = rep.bytes().zip(text.bytes()).take_while(|(a, b)| a == b).count();
            let common = (0..=common).rev().find(|i| text.is_char_boundary(*i) && rep.is_char_boundary(*i)).unwrap_or(0);
            if common < rep.len() && common == text.len() {
                self.replay = Some(rep[common..].to_string());
                return;
            }
            text = &text[common..];
        }
        if !text.is_empty() {
            self.deltas = true;
            self.segment.push_str(text);
            sink.text(text.to_string());
        }
    }
}

impl LineParser for CursorStream {
    fn handle(&mut self, line: &str, sink: &EventSink) {
        let Some(v) = json_line(line, sink) else { return };
        match s(&v, "type").unwrap_or("") {
            "system" => {
                if s(&v, "subtype") == Some("init") {
                    if let Some(id) = s(&v, "session_id") {
                        let path = self.workspace.as_deref().map(|w| larrier_formats::cursor::chat_db(w, id).to_string_lossy().to_string());
                        sink.send(AgentEvent::NativeSession { id: id.to_string(), path });
                    }
                    if let Some(m) = s(&v, "model") {
                        // Cursor names its model ("Composer 2"), it doesn't
                        // give the id `--model` takes.
                        sink.send(AgentEvent::ModelName { name: m.to_string() });
                    }
                }
            }
            "assistant" => {
                let text = content_text(&v["message"]["content"]);
                let ts = v.get("timestamp_ms").is_some();
                let call = v.get("model_call_id").is_some();
                match (ts, call) {
                    // A real streaming delta.
                    (true, false) => self.delta(&text, sink),
                    // Flush before a tool call / final flush: duplicates of
                    // what was streamed — only use them when nothing was.
                    _ => {
                        if !self.deltas && !text.trim().is_empty() {
                            sink.send(AgentEvent::Text { text });
                        }
                        self.deltas = false;
                        self.segment.clear();
                        self.replay = None;
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
            // Transient network trouble; Cursor retries from a checkpoint.
            "connection" => match s(&v, "subtype") {
                Some("reconnecting") => {
                    self.reconnecting = true;
                    sink.log(format!("Cursor reconnecting (attempt {})", v["attempt"].as_u64().unwrap_or(1)));
                }
                Some("reconnected") => {
                    self.reconnecting = false;
                    sink.log("Cursor reconnected".to_string());
                }
                _ => {}
            },
            "retry" => {
                if s(&v, "subtype") == Some("resuming") && !self.segment.is_empty() {
                    self.replay = Some(self.segment.clone());
                }
                sink.log(format!("Cursor retry: {}", s(&v, "subtype").unwrap_or("?")));
            }
            "result" => {
                if let Some(id) = s(&v, "session_id") {
                    sink.send(AgentEvent::NativeSession { id: id.to_string(), path: None });
                }
                let us = &v["usage"];
                if us.is_object() {
                    sink.send(AgentEvent::Usage { usage: usage_from(u(us, "inputTokens"), u(us, "outputTokens"), u(us, "cacheReadTokens"), u(us, "cacheWriteTokens"), 0, None) });
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
        // Most tools are `<kind>ToolCall: {args, result}`; others arrive as
        // `function: {name, arguments: "<json>"}`.
        let (raw, args) = if key == "function" {
            let args = match &body["arguments"] {
                Value::String(a) => serde_json::from_str(a).unwrap_or_else(|_| Value::String(a.clone())),
                Value::Null => Value::Object(Map::new()),
                other => other.clone(),
            };
            (s(body, "name").unwrap_or("tool"), args)
        } else {
            (key.trim_end_matches("ToolCall"), body.get("args").cloned().unwrap_or(Value::Object(Map::new())))
        };
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
            // Loads a deferred tool's schema before calling it.
            "getMcpTools" => ("get_mcp_tools", ToolKind::Other),
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
                let mut t = if kind == ToolKind::classify(name) { tool(id.clone(), name, input) } else { tool_of_kind(id.clone(), name, kind, input) };
                if name == "get_mcp_tools" {
                    t.title = Some(format!("Loaded {}", s(&args, "toolName").unwrap_or("tools")));
                }
                sink.send(AgentEvent::ToolStart { tool: t });
            }
        }
        if s(v, "subtype") != Some("completed") {
            return;
        }
        // The docs don't place a `function` tool's result; accept it inside
        // `function` or beside it.
        let result = body.get("result").or_else(|| v["tool_call"].get("result")).cloned().unwrap_or(Value::Null);
        let (status, output, is_error) = match result.as_object().and_then(|m| m.iter().next()) {
            Some((k, _)) if k == "success" && name == "get_mcp_tools" => (ToolStatus::Completed, format!("Loaded {}", s(&args, "toolName").unwrap_or("tools")), false),
            Some((k, r)) if k == "success" => (ToolStatus::Completed, success_text(kind, r), false),
            Some((k, r)) => {
                let msg = s(r, "error").or_else(|| s(r, "message")).or_else(|| s(r, "reason")).filter(|m| !m.trim().is_empty()).map(|s| s.to_string());
                // Print mode without --force rejects shell commands with an empty reason.
                let msg = msg.unwrap_or_else(|| if k == "rejected" { "Rejected".to_string() } else { k.clone() });
                (if k == "rejected" { ToolStatus::Cancelled } else { ToolStatus::Failed }, msg, k != "rejected")
            }
            // `function` tools report a plain result.
            None if !result.is_null() => (ToolStatus::Completed, content_text(&result), false),
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
            } else if let Some(n) = r["linesCreated"].as_u64() {
                // writeToolCall: a new file of `linesCreated` lines.
                diffs.push(FileDiff { path, added: n as u32, ..Default::default() });
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
        ToolKind::Read | ToolKind::Search if r.get("files").is_some() || r.get("workspaceResults").is_some() || r.get("directoryTreeRoot").is_some() => search_text(r),
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

/// Glob / grep / ls results as text: `files`, `workspaceResults` (grep
/// matches, file lists or counts per workspace) or a directory tree.
fn search_text(r: &Value) -> String {
    let mut lines: Vec<String> = Vec::new();
    let strs = |v: &Value| v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect::<Vec<_>>()).unwrap_or_default();
    lines.extend(strs(&r["files"]));
    for ws in r["workspaceResults"].as_object().into_iter().flat_map(|m| m.values()) {
        let c = ws.get("content").unwrap_or(ws);
        for f in c["matches"].as_array().into_iter().flatten() {
            let file = s(f, "file").unwrap_or("");
            for m in f["matches"].as_array().into_iter().flatten() {
                let n = m["lineNumber"].as_u64().map(|n| n.to_string()).unwrap_or_default();
                let sep = if m["isContextLine"].as_bool() == Some(true) { '-' } else { ':' };
                lines.push(format!("{file}{sep}{n}{sep}{}", s(m, "content").unwrap_or("")));
            }
        }
        lines.extend(strs(&ws["files"]["files"]));
        lines.extend(strs(&ws["files"]));
        for c in ws["count"]["counts"].as_array().into_iter().flatten() {
            lines.push(format!("{}:{}", s(c, "file").unwrap_or(""), c["count"].as_u64().unwrap_or(0)));
        }
    }
    // `ls`: a directory tree.
    fn walk(node: &Value, prefix: &str, out: &mut Vec<String>) {
        for d in node["childrenDirs"].as_array().into_iter().flatten() {
            let name = s(d, "absPath").and_then(|p| p.rsplit(['/', '\\']).next()).unwrap_or("");
            out.push(format!("{prefix}{name}/"));
            walk(d, &format!("{prefix}  "), out);
        }
        for f in node["childrenFiles"].as_array().into_iter().flatten() {
            out.push(format!("{prefix}{}", s(f, "name").unwrap_or("")));
        }
    }
    walk(&r["directoryTreeRoot"], "", &mut lines);
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::testutil::collect;

    fn fixture(text: &str) -> Vec<AgentEvent> {
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        collect(&mut CursorStream::default(), &lines)
    }

    fn streamed(ev: &[AgentEvent]) -> String {
        ev.iter().filter_map(|e| match e { AgentEvent::TextDelta { text } | AgentEvent::Text { text } => Some(text.as_str()), _ => None }).collect()
    }

    fn output<'a>(ev: &'a [AgentEvent], name: &str) -> Vec<(&'a ToolStatus, &'a str)> {
        let ids: Vec<&str> = ev.iter().filter_map(|e| match e { AgentEvent::ToolStart { tool } if tool.name == name => Some(tool.id.as_str()), _ => None }).collect();
        ev.iter()
            .filter_map(|e| match e {
                AgentEvent::ToolUpdate { id, status: Some(st), output: Some(o), .. } if ids.contains(&id.as_str()) => Some((st, o.as_str())),
                _ => None,
            })
            .collect()
    }

    /// Real print-mode run (no --force): read, two edits, a rejected shell
    /// command, a glob, and a reconnect that replays the last message.
    #[test]
    fn real_tools_and_reconnect_run() {
        let ev = fixture(include_str!("../../tests/fixtures/cursor_tools_reconnect.jsonl"));
        assert_eq!(streamed(&ev), "I'll follow those five steps in this directory, starting by reading `notes.txt`.done", "the replayed answer shows once");
        assert!(output(&ev, "read")[0].1.contains("beta line"));
        let edits: Vec<(u32, u32)> = ev.iter().filter_map(|e| match e { AgentEvent::ToolUpdate { diffs, .. } if !diffs.is_empty() => Some((diffs[0].added, diffs[0].removed)), _ => None }).collect();
        assert_eq!(edits, [(1, 1), (1, 0)]);
        assert!(output(&ev, "shell").iter().all(|(st, o)| **st == ToolStatus::Cancelled && *o == "Rejected"));
        assert!(output(&ev, "glob")[0].1.contains("notes.txt"));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Usage { usage } if usage.input_tokens == 271 && usage.cache_read_tokens == 30592)));
        assert!(matches!(ev.last(), Some(AgentEvent::Done { ok: true, .. })));
    }

    /// Real run: a deferred tool loaded with getMcpTools, grep, a to-do list
    /// marked done, and a reconnect whose only copy of the answer arrives
    /// mid-reconnect.
    #[test]
    fn real_todos_and_grep_run() {
        let ev = fixture(include_str!("../../tests/fixtures/cursor_todos_grep.jsonl"));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolStart { tool } if tool.title.as_deref() == Some("Loaded TodoWrite"))));
        assert_eq!(output(&ev, "get_mcp_tools")[0].1, "Loaded TodoWrite");
        assert_eq!(output(&ev, "grep")[0].1, ".\\notes.txt:3:gamma line");
        let plans: Vec<_> = ev.iter().filter_map(|e| match e { AgentEvent::Plan { entries } => Some(entries), _ => None }).collect();
        assert!(plans.last().unwrap().iter().all(|p| p.status == larrier_core::PlanStatus::Completed), "TODO_STATUS_COMPLETED");
        assert!(streamed(&ev).ends_with("then mark the todos done.Done."), "{}", streamed(&ev));
        assert!(matches!(ev.last(), Some(AgentEvent::Done { ok: true, .. })));
    }

    /// Documented shapes not seen in the runs above: `function` tools and
    /// `writeToolCall` results.
    #[test]
    fn function_tools_and_writes() {
        let lines = [
            r#"{"type":"tool_call","subtype":"started","call_id":"f1","tool_call":{"function":{"name":"read_file","arguments":"{\"target_file\":\"src/a.rs\"}"}}}"#,
            r#"{"type":"tool_call","subtype":"completed","call_id":"f1","tool_call":{"function":{"name":"read_file","arguments":"{\"target_file\":\"src/a.rs\"}"},"result":"fn main() {}"}}"#,
            r#"{"type":"tool_call","subtype":"started","call_id":"w1","tool_call":{"writeToolCall":{"args":{"path":"b.txt","fileText":"x\ny\n"}}}}"#,
            r#"{"type":"tool_call","subtype":"completed","call_id":"w1","tool_call":{"writeToolCall":{"args":{"path":"b.txt","fileText":"x\ny\n"},"result":{"success":{"path":"b.txt","linesCreated":2,"fileSize":4}}}}}"#,
        ];
        let ev = collect(&mut CursorStream::default(), &lines);
        assert!(matches!(&ev[0], AgentEvent::ToolStart { tool } if tool.kind == ToolKind::Read && tool.title.as_deref() == Some("Read src/a.rs")));
        assert!(matches!(&ev[1], AgentEvent::ToolUpdate { output: Some(o), .. } if o == "fn main() {}"), "{:?}", &ev[1]);
        assert!(matches!(&ev[2], AgentEvent::ToolStart { tool } if tool.kind == ToolKind::Write));
        assert!(matches!(&ev[3], AgentEvent::ToolUpdate { diffs, .. } if diffs[0].added == 2));
    }

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
        // "Composer 2" is a display name: shown, never passed back as --model.
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::ModelName { name } if name == "Composer 2")));
        assert!(!ev.iter().any(|e| matches!(e, AgentEvent::Model { .. })));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolUpdate { output: Some(o), .. } if o.contains("a.txt"))));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolUpdate { diffs, .. } if diffs.first().map(|d| d.added) == Some(3))));
        assert!(matches!(ev.last(), Some(AgentEvent::Done { ok: true, .. })));
    }
}
