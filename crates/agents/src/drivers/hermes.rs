//! Hermes Agent: `hermes chat --query-file - --oneshot --format stream-json
//! [-m model] [--resume id] [--in cwd] [--yolo]`, prompt on stdin.

use std::collections::VecDeque;

use larrier_core::ToolStatus;
use serde_json::Value;
use tokio::sync::mpsc::UnboundedReceiver;

use super::{content_text, json_line, run_stream, s, tool, u, usage_from, with_file_mentions, LineParser};
use crate::events::{AgentEvent, EventSink, TurnCommand};
use crate::request::TurnRequest;

pub async fn run(req: TurnRequest, sink: EventSink, mut control: UnboundedReceiver<TurnCommand>) {
    let mut cmd = req.launch.command();
    cmd.current_dir(&req.cwd);
    cmd.args(["chat", "--query-file", "-", "--oneshot", "--format", "stream-json"]);
    cmd.arg("--in").arg(&req.cwd);
    if let Some(id) = &req.resume_id {
        cmd.args(["--resume", id, "--no-restore-cwd"]);
    }
    if let Some(m) = &req.model {
        cmd.args(["-m", m]);
    }
    cmd.args(&req.modes.args);
    if let Some(img) = req.image_paths().first() {
        cmd.args(["--image", img]);
    }
    cmd.args(&req.launch.extra_args);
    cmd.env("PYTHONIOENCODING", "utf-8");
    let prompt = with_file_mentions(&req, req.full_prompt());
    let mut parser = HermesStream::default();
    run_stream("Hermes Agent", cmd, Some(prompt), &sink, &mut control, &mut parser).await;
}

#[derive(Default)]
pub struct HermesStream {
    done: bool,
    failed: bool,
    streamed_text: bool,
    counter: usize,
    /// Hermes tool events carry no ids: match results to calls by name, FIFO.
    pending: VecDeque<(String, String)>,
}

impl LineParser for HermesStream {
    fn handle(&mut self, line: &str, sink: &EventSink) {
        let Some(v) = json_line(line, sink) else { return };
        match s(&v, "type").unwrap_or("") {
            "system" => {
                if let Some(id) = s(&v, "session_id") {
                    sink.send(AgentEvent::NativeSession { id: id.to_string(), path: None });
                }
                if let Some(m) = s(&v, "model") {
                    sink.send(AgentEvent::Model { model: m.to_string() });
                }
            }
            "text" => {
                if let Some(t) = s(&v, "text").filter(|t| !t.is_empty()) {
                    self.streamed_text = true;
                    sink.text(t);
                }
            }
            "reasoning" | "thinking" => {
                if let Some(t) = s(&v, "text").filter(|t| !t.is_empty()) {
                    sink.send(AgentEvent::ReasoningDelta { text: t.to_string() });
                }
            }
            "tool_use" => {
                self.counter += 1;
                let name = s(&v, "name").unwrap_or("tool").to_string();
                let id = s(&v, "id").map(|s| s.to_string()).unwrap_or_else(|| format!("hermes-tool-{}", self.counter));
                self.pending.push_back((name.clone(), id.clone()));
                sink.send(AgentEvent::ToolStart { tool: tool(id, &name, v.get("input").cloned().unwrap_or(Value::Null)) });
            }
            "tool_result" => {
                let name = s(&v, "name").unwrap_or("tool");
                let pos = self.pending.iter().position(|(n, _)| n == name).or(if self.pending.is_empty() { None } else { Some(0) });
                let Some(pos) = pos else { return };
                let (_, id) = self.pending.remove(pos).unwrap();
                // Tools answer with JSON ({"output": …, "exit_code": …}); show
                // the readable part, as imported sessions do.
                let (output, failed) = larrier_formats::hermes::tool_output(&content_text(&v["output"]));
                let is_error = v.get("is_error").and_then(|b| b.as_bool()).unwrap_or(false) || failed;
                sink.send(AgentEvent::ToolUpdate {
                    id,
                    status: Some(if is_error { ToolStatus::Failed } else { ToolStatus::Completed }),
                    output: Some(output),
                    diffs: Vec::new(),
                    is_error,
                    title: None,
                });
            }
            "result" => {
                if let Some(id) = s(&v, "session_id") {
                    sink.send(AgentEvent::NativeSession { id: id.to_string(), path: None });
                }
                if !self.streamed_text {
                    if let Some(t) = s(&v, "text").filter(|t| !t.trim().is_empty()) {
                        sink.send(AgentEvent::Text { text: t.to_string() });
                    }
                }
                let t = &v["tokens"];
                if t.is_object() {
                    sink.send(AgentEvent::Usage { usage: usage_from(u(t, "input"), u(t, "output"), u(t, "cache_read"), u(t, "cache_write"), 0, None) });
                }
                let err = v.get("error").filter(|e| !e.is_null()).map(content_text);
                let ok = err.is_none() && v.get("exit_code").and_then(|c| c.as_i64()).unwrap_or(0) == 0;
                if let Some(e) = err {
                    sink.error(e);
                    self.failed = true;
                }
                self.done = true;
                sink.done(ok, None);
            }
            "error" => {
                sink.error(s(&v, "message").or_else(|| s(&v, "error")).unwrap_or("Hermes error").to_string());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::testutil::collect;
    use larrier_core::ToolKind;

    #[test]
    fn maps_stream_json() {
        let lines = [
            r#"{"type":"system","subtype":"init","model":"anthropic/claude-opus-5-5","session_id":"20261029_101500_ab12cd","timestamp":1}"#,
            r#"{"type":"tool_use","name":"terminal","input":{"command":"ls"},"timestamp":2}"#,
            r#"{"type":"tool_use","name":"read_file","input":{"path":"a.md"},"timestamp":3}"#,
            r#"{"type":"tool_result","name":"read_file","output":"Title A","duration_ms":3,"is_error":false,"timestamp":4}"#,
            r#"{"type":"tool_result","name":"terminal","output":"a.md","duration_ms":5,"is_error":false,"timestamp":5}"#,
            r#"{"type":"text","text":"Here","timestamp":6}"#,
            r#"{"type":"result","session_id":"20261029_101500_ab12cd","exit_code":0,"text":"Here","tokens":{"input":9,"output":3,"total":12,"cache_read":1,"cache_write":0},"duration_ms":40,"timestamp":7}"#,
        ];
        let mut p = HermesStream::default();
        let ev = collect(&mut p, &lines);
        assert!(matches!(&ev[0], AgentEvent::NativeSession { id, .. } if id.starts_with("20261029")));
        assert!(matches!(&ev[2], AgentEvent::ToolStart { tool } if tool.kind == ToolKind::Shell && tool.id == "hermes-tool-1"));
        assert!(matches!(&ev[4], AgentEvent::ToolUpdate { id, output: Some(o), .. } if id == "hermes-tool-2" && o == "Title A"));
        assert!(matches!(&ev[5], AgentEvent::ToolUpdate { id, .. } if id == "hermes-tool-1"));
        assert_eq!(ev.iter().filter(|e| matches!(e, AgentEvent::Text { .. } | AgentEvent::TextDelta { .. })).count(), 1);
        assert!(matches!(ev.last(), Some(AgentEvent::Done { ok: true, .. })));
    }
}
