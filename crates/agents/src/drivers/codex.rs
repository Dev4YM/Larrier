//! OpenAI Codex: `codex exec --json … -` (prompt on stdin) and
//! `codex exec resume <id> --json … -` for follow-ups.

use std::collections::HashMap;
use std::path::PathBuf;

use larrier_core::{FileDiff, ToolKind, ToolStatus};
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedReceiver;

use super::{content_text, json_line, numstat, plan_entries, run_stream, s, tool, tool_of_kind, u, usage_from, LineParser};
use crate::events::{AgentEvent, EventSink, TurnCommand};
use crate::request::TurnRequest;

pub async fn run(req: TurnRequest, sink: EventSink, mut control: UnboundedReceiver<TurnCommand>) {
    let mut cmd = req.launch.command();
    cmd.current_dir(&req.cwd);
    cmd.arg("exec");
    match &req.resume_id {
        Some(id) => {
            cmd.args(["resume", id]);
        }
        None => {
            cmd.arg("-C").arg(&req.cwd);
        }
    }
    // Sandbox / approval / plan from the chosen modes, as config overrides
    // (they work for new runs and `exec resume` alike).
    cmd.args(&req.modes.args);
    cmd.args(["--json", "--skip-git-repo-check"]);
    if let Some(m) = &req.model {
        cmd.args(["-m", m]);
    }
    for img in req.image_paths() {
        cmd.args(["-i", &img]);
    }
    cmd.args(&req.launch.extra_args);
    cmd.arg("-");
    let mut prompt = req.full_prompt();
    let files = req.file_paths();
    if !files.is_empty() {
        prompt.push_str("\n\nAttached files (read them as needed):\n");
        for f in files {
            prompt.push_str(&format!("- {f}\n"));
        }
    }
    let mut parser = CodexStream::new(req.cwd.clone());
    run_stream("Codex", cmd, Some(prompt), &sink, &mut control, &mut parser).await;
}

/// Parser for `codex exec --json` events.
pub struct CodexStream {
    cwd: PathBuf,
    done: bool,
    failed: bool,
    /// Characters already emitted per streaming item (agent_message / reasoning).
    emitted: HashMap<String, usize>,
    started_tools: HashMap<String, ToolKind>,
    /// Subagent thread id → the spawn_agent tool call that started it.
    agent_threads: HashMap<String, String>,
    /// Subagent threads whose final message was already recorded.
    agent_done: HashMap<String, ToolStatus>,
}

impl CodexStream {
    pub fn new(cwd: PathBuf) -> Self {
        CodexStream {
            cwd,
            done: false,
            failed: false,
            emitted: HashMap::new(),
            started_tools: HashMap::new(),
            agent_threads: HashMap::new(),
            agent_done: HashMap::new(),
        }
    }

    /// Emit the not-yet-seen tail of a growing text item.
    fn stream_text(&mut self, id: &str, text: &str, reasoning: bool, sink: &EventSink) {
        let seen = self.emitted.get(id).copied();
        let start = seen.unwrap_or(0);
        if text.len() <= start || !text.is_char_boundary(start) {
            return;
        }
        let tail = &text[start..];
        self.emitted.insert(id.to_string(), text.len());
        let ev = match (reasoning, seen.is_none()) {
            (false, true) => AgentEvent::Text { text: tail.to_string() },
            (false, false) => AgentEvent::TextDelta { text: tail.to_string() },
            (true, true) => AgentEvent::Reasoning { text: tail.to_string(), duration_ms: None },
            (true, false) => AgentEvent::ReasoningDelta { text: tail.to_string() },
        };
        sink.send(ev);
    }

    fn item(&mut self, phase: &str, item: &Value, sink: &EventSink) {
        let id = s(item, "id").unwrap_or("item").to_string();
        let completed = phase == "item.completed";
        match s(item, "type").unwrap_or("") {
            "agent_message" => {
                if let Some(t) = s(item, "text") {
                    self.stream_text(&id, t, false, sink);
                }
            }
            "reasoning" => {
                if let Some(t) = s(item, "text") {
                    self.stream_text(&id, t, true, sink);
                }
            }
            "command_execution" => {
                let command = s(item, "command").unwrap_or("").to_string();
                self.start(&id, || tool(id.clone(), "shell", json!({ "command": command })), sink);
                let status = s(item, "status").unwrap_or(if completed { "completed" } else { "in_progress" });
                if status != "in_progress" {
                    let mut out = s(item, "aggregated_output").unwrap_or("").to_string();
                    let exit = item.get("exit_code").and_then(|c| c.as_i64());
                    if let Some(code) = exit.filter(|c| *c != 0) {
                        out.push_str(&format!("\n[exit code {code}]"));
                    }
                    let st = match status {
                        "declined" => ToolStatus::Cancelled,
                        "failed" => ToolStatus::Failed,
                        _ if exit.unwrap_or(0) != 0 => ToolStatus::Failed,
                        _ => ToolStatus::Completed,
                    };
                    sink.send(AgentEvent::ToolUpdate { id, status: Some(st), output: Some(out), diffs: Vec::new(), is_error: false, title: None });
                }
            }
            "file_change" => {
                let changes = item.get("changes").cloned().unwrap_or(Value::Null);
                let list: Vec<(String, String)> = changes
                    .as_array()
                    .map(|a| a.iter().map(|c| (s(c, "path").unwrap_or("").to_string(), s(c, "kind").unwrap_or("update").to_string())).collect())
                    .unwrap_or_default();
                let kind = match list.as_slice() {
                    [(_, k)] if k == "add" => ToolKind::Write,
                    [(_, k)] if k == "delete" => ToolKind::Delete,
                    _ => ToolKind::Edit,
                };
                let first = list.first().map(|(p, _)| p.clone()).unwrap_or_default();
                let mut t = tool_of_kind(id.clone(), "apply_patch", kind, json!({ "file_path": first, "changes": changes }));
                if list.len() > 1 {
                    t.title = Some(format!("Edit {} files", list.len()));
                }
                t.locations = list.iter().map(|(p, _)| p.clone()).collect();
                if !self.started_tools.contains_key(&id) {
                    self.started_tools.insert(id.clone(), kind);
                    sink.send(AgentEvent::ToolStart { tool: t });
                }
                let status = s(item, "status").unwrap_or(if completed { "completed" } else { "in_progress" });
                if status != "in_progress" {
                    let diffs = list.iter().map(|(p, k)| self.diff_for(p, k)).collect();
                    let failed = status == "failed";
                    sink.send(AgentEvent::ToolUpdate {
                        id,
                        status: Some(if failed { ToolStatus::Failed } else { ToolStatus::Completed }),
                        output: None,
                        diffs,
                        is_error: failed,
                        title: None,
                    });
                }
            }
            "mcp_tool_call" => {
                let name = format!("mcp__{}__{}", s(item, "server").unwrap_or("mcp"), s(item, "tool").unwrap_or("tool"));
                let args = item.get("arguments").cloned().unwrap_or(Value::Null);
                self.start(&id, || tool(id.clone(), &name, args.clone()), sink);
                let status = s(item, "status").unwrap_or("");
                if completed || status == "completed" || status == "failed" {
                    let err = item.get("error").filter(|e| !e.is_null());
                    let output = match err {
                        Some(e) => s(e, "message").unwrap_or("error").to_string(),
                        None => content_text(&item["result"]["content"]),
                    };
                    sink.send(AgentEvent::ToolUpdate {
                        id,
                        status: Some(if err.is_some() || status == "failed" { ToolStatus::Failed } else { ToolStatus::Completed }),
                        output: Some(output),
                        diffs: Vec::new(),
                        is_error: err.is_some(),
                        title: None,
                    });
                }
            }
            "web_search" => {
                let q = s(item, "query").unwrap_or("").to_string();
                self.start(&id, || tool(id.clone(), "web_search", json!({ "query": q })), sink);
                if completed {
                    let out = item.get("results").map(content_text).unwrap_or_default();
                    sink.send(AgentEvent::ToolUpdate { id, status: Some(ToolStatus::Completed), output: Some(out), diffs: Vec::new(), is_error: false, title: None });
                }
            }
            "todo_list" => {
                if let Some(entries) = item.get("items").and_then(plan_entries) {
                    sink.send(AgentEvent::Plan { entries });
                }
            }
            "collab_tool_call" => self.collab(&id, item, completed, sink),
            // Error *items* are Codex's non-fatal warnings (unsupported service
            // tier, missing model metadata…); fatal problems arrive as a
            // top-level `error` / `turn.failed` event instead.
            "error" => {
                if completed || phase == "item.started" {
                    sink.notice(s(item, "message").unwrap_or("warning").to_string());
                }
            }
            _ => {}
        }
    }

    /// Multi-agent calls (`spawn_agent`, `send_input`, `wait`, `close_agent`).
    /// A spawned subagent keeps running after its spawn call returns, so the
    /// spawn tool stays Running until a later call reports the agent's state.
    fn collab(&mut self, id: &str, item: &Value, completed: bool, sink: &EventSink) {
        let t_name = s(item, "tool").unwrap_or("spawn_agent").to_string();
        let prompt = s(item, "prompt").unwrap_or("").to_string();
        let receivers: Vec<String> = item
            .get("receiver_thread_ids")
            .and_then(|r| r.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();
        let spawn = t_name == "spawn_agent";
        let n = receivers.len().max(1);
        let plural = if n == 1 { "subagent" } else { "subagents" };
        let title = match t_name.as_str() {
            "spawn_agent" => None,
            "send_input" => Some(format!("Messaged {plural}")),
            "wait" => Some(format!("Waited for {n} {plural}")),
            "close_agent" => Some(format!("Closed {n} {plural}")),
            other => Some(other.replace('_', " ")),
        };
        let first = prompt.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
        let description: String = first.chars().take(80).collect();
        let mut input = json!({ "description": description, "prompt": prompt, "tool": t_name, "receiver_thread_ids": receivers });
        for k in ["agent_type", "role", "model"] {
            if let Some(v) = s(item, k) {
                input[k] = json!(v);
            }
        }
        let kind = if spawn { ToolKind::Task } else { ToolKind::Other };
        self.start(
            id,
            || {
                let mut t = tool_of_kind(id.to_string(), &t_name, kind, input.clone());
                if spawn {
                    t.agent_name = s(item, "agent_type").or_else(|| s(item, "role")).map(|a| a.to_string());
                    t.title = Some(t.describe());
                } else {
                    t.title = title.clone();
                }
                t
            },
            sink,
        );
        if spawn {
            for r in &receivers {
                self.agent_threads.entry(r.clone()).or_insert_with(|| id.to_string());
            }
        }
        // Agent states reported by this call update the spawn tools.
        let mut summary = Vec::new();
        if let Some(states) = item.get("agents_states").and_then(|a| a.as_object()) {
            for (thread, st) in states {
                let state = s(st, "status").unwrap_or("running");
                let message = s(st, "message").filter(|m| !m.trim().is_empty());
                summary.push(match message {
                    Some(m) => format!("{state}: {}", m.lines().next().unwrap_or("")),
                    None => state.to_string(),
                });
                let Some(spawn_id) = self.agent_threads.get(thread).cloned() else { continue };
                let status = collab_status(state);
                if status == ToolStatus::Running || self.agent_done.get(thread) == Some(&status) {
                    continue;
                }
                self.agent_done.insert(thread.clone(), status);
                if let Some(m) = message {
                    sink.send(AgentEvent::Subagent { parent_tool_id: spawn_id.clone(), inner: Box::new(AgentEvent::Text { text: m.to_string() }) });
                }
                sink.send(AgentEvent::ToolUpdate {
                    id: spawn_id,
                    status: Some(status),
                    output: message.map(|m| m.to_string()),
                    diffs: Vec::new(),
                    is_error: status == ToolStatus::Failed,
                    title: None,
                });
            }
        }
        if !completed {
            return;
        }
        let failed = s(item, "status") == Some("failed");
        // A successful spawn is only the launch; the agent is still working.
        let status = if failed {
            ToolStatus::Failed
        } else if spawn {
            if !receivers.is_empty() && receivers.iter().all(|r| self.agent_done.contains_key(r)) {
                return; // already finished (reported above)
            }
            ToolStatus::Running
        } else {
            ToolStatus::Completed
        };
        let output = (!spawn && !summary.is_empty()).then(|| summary.join("\n"));
        sink.send(AgentEvent::ToolUpdate { id: id.to_string(), status: Some(status), output, diffs: Vec::new(), is_error: failed, title: None });
    }

    fn start(&mut self, id: &str, make: impl FnOnce() -> larrier_core::ToolCall, sink: &EventSink) {
        if !self.started_tools.contains_key(id) {
            let t = make();
            self.started_tools.insert(id.to_string(), t.kind);
            sink.send(AgentEvent::ToolStart { tool: t });
        }
    }

    fn diff_for(&self, path: &str, kind: &str) -> FileDiff {
        let mut d = FileDiff { path: path.to_string(), ..Default::default() };
        let full = if std::path::Path::new(path).is_absolute() { PathBuf::from(path) } else { self.cwd.join(path) };
        match kind {
            "add" => {
                if let Ok(t) = std::fs::read_to_string(&full) {
                    d.added = t.lines().count() as u32;
                }
            }
            "delete" => {}
            _ => {
                if let Some((a, r)) = numstat(&self.cwd, path) {
                    d.added = a;
                    d.removed = r;
                }
            }
        }
        d
    }
}

impl LineParser for CodexStream {
    fn handle(&mut self, line: &str, sink: &EventSink) {
        let Some(v) = json_line(line, sink) else { return };
        match s(&v, "type").unwrap_or("") {
            "thread.started" => {
                if let Some(id) = s(&v, "thread_id") {
                    sink.send(AgentEvent::NativeSession { id: id.to_string(), path: None });
                }
            }
            t @ ("item.started" | "item.updated" | "item.completed") => {
                let item = v["item"].clone();
                self.item(t, &item, sink);
            }
            "turn.completed" => {
                let us = &v["usage"];
                let input = u(us, "input_tokens");
                let cached = u(us, "cached_input_tokens");
                sink.send(AgentEvent::Usage {
                    usage: usage_from(
                        input.saturating_sub(cached),
                        u(us, "output_tokens"),
                        cached,
                        u(us, "cache_write_input_tokens"),
                        u(us, "reasoning_output_tokens"),
                        None,
                    ),
                });
                self.done = true;
                sink.done(true, Some("end_turn".into()));
            }
            "turn.failed" => {
                let raw = s(&v["error"], "message").unwrap_or("Codex turn failed");
                let msg = serde_json::from_str::<Value>(raw)
                    .ok()
                    .and_then(|j| j["error"]["message"].as_str().or(j["message"].as_str()).map(|m| m.to_string()))
                    .unwrap_or_else(|| raw.to_string());
                sink.error(msg);
                self.failed = true;
                self.done = true;
                sink.done(false, Some("error".into()));
            }
            "error" => {
                let raw = s(&v, "message").unwrap_or("error");
                // API failures arrive as a JSON string; surface the human message.
                let pretty = serde_json::from_str::<Value>(raw)
                    .ok()
                    .and_then(|j| j["error"]["message"].as_str().or(j["message"].as_str()).map(|m| m.to_string()));
                let msg = pretty.as_deref().unwrap_or(raw);
                // Transient reconnect notices are informational.
                if msg.to_ascii_lowercase().contains("reconnect") {
                    sink.log(msg.to_string());
                } else {
                    sink.error(msg.to_string());
                    self.failed = true;
                }
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

/// Codex `CollabAgentStatus` → tool status.
fn collab_status(state: &str) -> ToolStatus {
    match state {
        "completed" | "shutdown" => ToolStatus::Completed,
        "errored" | "failed" | "not_found" => ToolStatus::Failed,
        "interrupted" | "cancelled" => ToolStatus::Cancelled,
        _ => ToolStatus::Running,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::testutil::collect;

    #[test]
    fn maps_exec_events() {
        let lines = [
            r#"{"type":"thread.started","thread_id":"0199a213-81c0-7800-8aa1-bbab2a035a53"}"#,
            r#"{"type":"turn.started"}"#,
            r#"{"type":"item.completed","item":{"id":"item_0","type":"reasoning","text":"**Scanning** the repo"}}"#,
            r#"{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"bash -lc ls","aggregated_output":"","exit_code":null,"status":"in_progress"}}"#,
            r#"{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"bash -lc ls","aggregated_output":"Cargo.toml\nsrc\n","exit_code":0,"status":"completed"}}"#,
            r#"{"type":"item.completed","item":{"id":"item_2","type":"file_change","changes":[{"path":"zz_new_file_larrier_test.txt","kind":"add"},{"path":"src/b.rs","kind":"update"}],"status":"completed"}}"#,
            r#"{"type":"item.started","item":{"id":"item_3","type":"todo_list","items":[{"text":"a","completed":true},{"text":"b","completed":false}]}}"#,
            r#"{"type":"item.completed","item":{"id":"item_4","type":"agent_message","text":"Done."}}"#,
            r#"{"type":"turn.completed","usage":{"input_tokens":1000,"cached_input_tokens":800,"output_tokens":50,"reasoning_output_tokens":20}}"#,
        ];
        let mut p = CodexStream::new(std::env::temp_dir());
        let ev = collect(&mut p, &lines);
        assert!(matches!(&ev[0], AgentEvent::NativeSession { id, .. } if id.starts_with("0199a213")));
        assert!(matches!(&ev[1], AgentEvent::Reasoning { .. }));
        assert!(matches!(&ev[2], AgentEvent::ToolStart { tool } if tool.kind == ToolKind::Shell && tool.title.as_deref() == Some("Ran bash -lc ls")));
        assert!(matches!(&ev[3], AgentEvent::ToolUpdate { status: Some(ToolStatus::Completed), output: Some(o), .. } if o.contains("Cargo.toml")));
        assert!(matches!(&ev[4], AgentEvent::ToolStart { tool } if tool.kind == ToolKind::Edit && tool.locations.len() == 2));
        assert!(matches!(&ev[5], AgentEvent::ToolUpdate { diffs, .. } if diffs.len() == 2));
        assert!(matches!(&ev[6], AgentEvent::Plan { entries } if entries.len() == 2));
        assert!(matches!(&ev[7], AgentEvent::Text { text } if text == "Done."));
        assert!(matches!(&ev[8], AgentEvent::Usage { usage } if usage.input_tokens == 200 && usage.cache_read_tokens == 800 && usage.reasoning_tokens == 20));
        assert!(matches!(ev.last(), Some(AgentEvent::Done { ok: true, .. })));
    }

    #[test]
    fn streaming_updates_and_failure() {
        let lines = [
            r#"{"type":"item.started","item":{"id":"m","type":"agent_message","text":"Hel"}}"#,
            r#"{"type":"item.updated","item":{"id":"m","type":"agent_message","text":"Hello"}}"#,
            r#"{"type":"item.completed","item":{"id":"m","type":"agent_message","text":"Hello"}}"#,
            r#"{"type":"error","message":"Reconnecting... 1/5"}"#,
            r#"{"type":"turn.failed","error":{"message":"usage limit reached"}}"#,
        ];
        let mut p = CodexStream::new(std::env::temp_dir());
        let ev = collect(&mut p, &lines);
        assert!(matches!(&ev[0], AgentEvent::Text { text } if text == "Hel"));
        assert!(matches!(&ev[1], AgentEvent::TextDelta { text } if text == "lo"));
        assert!(matches!(&ev[2], AgentEvent::Error { message } if message.contains("usage limit")));
        assert!(matches!(ev.last(), Some(AgentEvent::Done { ok: false, .. })));
    }

    #[test]
    fn maps_collab_subagents() {
        let lines = [
            r#"{"type":"item.started","item":{"id":"item_1","type":"collab_tool_call","tool":"spawn_agent","sender_thread_id":"t0","receiver_thread_ids":[],"prompt":"Audit the parser\nthoroughly","agents_states":{},"status":"in_progress"}}"#,
            r#"{"type":"item.completed","item":{"id":"item_1","type":"collab_tool_call","tool":"spawn_agent","sender_thread_id":"t0","receiver_thread_ids":["t1"],"prompt":"Audit the parser\nthoroughly","agents_states":{"t1":{"status":"pending_init","message":null}},"status":"completed"}}"#,
            r#"{"type":"item.started","item":{"id":"item_2","type":"collab_tool_call","tool":"wait","sender_thread_id":"t0","receiver_thread_ids":["t1"],"prompt":null,"agents_states":{},"status":"in_progress"}}"#,
            r#"{"type":"item.completed","item":{"id":"item_2","type":"collab_tool_call","tool":"wait","sender_thread_id":"t0","receiver_thread_ids":["t1"],"prompt":null,"agents_states":{"t1":{"status":"completed","message":"Parser looks fine."}},"status":"completed"}}"#,
        ];
        let mut p = CodexStream::new(std::env::temp_dir());
        let ev = collect(&mut p, &lines);
        assert!(matches!(&ev[0], AgentEvent::ToolStart { tool } if tool.kind == ToolKind::Task && tool.is_subagent() && tool.title.as_deref() == Some("Subagent Audit the parser")));
        assert!(matches!(&ev[1], AgentEvent::ToolUpdate { id, status: Some(ToolStatus::Running), .. } if id == "item_1"), "spawned agent keeps running");
        assert!(matches!(&ev[2], AgentEvent::ToolStart { tool } if tool.kind == ToolKind::Other && tool.title.as_deref() == Some("Waited for 1 subagent")));
        assert!(matches!(&ev[3], AgentEvent::Subagent { parent_tool_id, inner } if parent_tool_id == "item_1" && matches!(&**inner, AgentEvent::Text { text } if text == "Parser looks fine.")));
        assert!(matches!(&ev[4], AgentEvent::ToolUpdate { id, status: Some(ToolStatus::Completed), output: Some(o), .. } if id == "item_1" && o == "Parser looks fine."));
        assert!(matches!(&ev[5], AgentEvent::ToolUpdate { id, status: Some(ToolStatus::Completed), .. } if id == "item_2"));
    }
}
