//! Gemini CLI: `gemini -o stream-json [-m] [--approval-mode] [-r id] -p <prompt>`.

use larrier_core::{ToolKind, ToolStatus};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedReceiver;

use super::{content_text, json_line, plan_entries, run_stream, s, tool, u, usage_from, with_file_mentions, LineParser};
use crate::events::{AgentEvent, EventSink, TurnCommand};
use crate::request::TurnRequest;

pub async fn run(req: TurnRequest, sink: EventSink, mut control: UnboundedReceiver<TurnCommand>) {
    let mut cmd = req.launch.command();
    cmd.current_dir(&req.cwd);
    cmd.args(["-o", "stream-json"]);
    if let Some(m) = &req.model {
        cmd.args(["-m", m]);
    }
    cmd.args(&req.modes.args);
    if let Some(id) = &req.resume_id {
        cmd.args(["-r", id]);
    }
    cmd.args(&req.launch.extra_args);
    cmd.args(["-p", &with_file_mentions(&req, req.prompt_for_arg(24_000))]);
    let mut parser = GeminiStream::default();
    run_stream("Gemini CLI", cmd, None, &sink, &mut control, &mut parser).await;
}

#[derive(Default)]
pub struct GeminiStream {
    done: bool,
    failed: bool,
}

impl LineParser for GeminiStream {
    fn handle(&mut self, line: &str, sink: &EventSink) {
        let Some(v) = json_line(line, sink) else { return };
        match s(&v, "type").unwrap_or("") {
            "init" => {
                if let Some(id) = s(&v, "session_id") {
                    sink.send(AgentEvent::NativeSession { id: id.to_string(), path: None });
                }
                if let Some(m) = s(&v, "model") {
                    sink.send(AgentEvent::Model { model: m.to_string() });
                }
            }
            "message" => {
                if s(&v, "role") != Some("assistant") {
                    return;
                }
                let text = content_text(&v["content"]);
                if text.is_empty() {
                    return;
                }
                if v.get("delta").and_then(|d| d.as_bool()).unwrap_or(false) {
                    sink.text(text);
                } else {
                    sink.send(AgentEvent::Text { text });
                }
            }
            "thought" | "thinking" => {
                let text = s(&v, "content").or_else(|| s(&v, "text")).unwrap_or("");
                if !text.is_empty() {
                    sink.send(AgentEvent::ReasoningDelta { text: text.to_string() });
                }
            }
            "tool_use" => {
                let id = s(&v, "tool_id").unwrap_or("tool").to_string();
                let name = s(&v, "tool_name").unwrap_or("tool");
                let params = v.get("parameters").cloned().unwrap_or(Value::Null);
                let t = tool(id, name, params.clone());
                if t.kind == ToolKind::Todo {
                    if let Some(entries) = params.get("todos").and_then(plan_entries) {
                        sink.send(AgentEvent::Plan { entries });
                    }
                }
                sink.send(AgentEvent::ToolStart { tool: t });
            }
            "tool_result" => {
                let id = s(&v, "tool_id").unwrap_or("tool").to_string();
                let ok = s(&v, "status") != Some("error");
                let output = if ok {
                    v.get("output").map(content_text).unwrap_or_default()
                } else {
                    s(&v["error"], "message").map(|m| m.to_string()).unwrap_or_else(|| content_text(&v["error"]))
                };
                sink.send(AgentEvent::ToolUpdate {
                    id,
                    status: Some(if ok { ToolStatus::Completed } else { ToolStatus::Failed }),
                    output: Some(output),
                    diffs: Vec::new(),
                    is_error: !ok,
                    title: None,
                });
            }
            "error" => {
                let msg = s(&v, "message").unwrap_or("error").to_string();
                if s(&v, "severity") == Some("warning") {
                    sink.notice(msg);
                } else {
                    sink.error(msg);
                    self.failed = true;
                }
            }
            "result" => {
                let st = &v["stats"];
                let input = if st.get("input_tokens").is_some() { u(st, "input_tokens") } else { u(st, "input") };
                sink.send(AgentEvent::Usage { usage: usage_from(input, u(st, "output_tokens"), u(st, "cached"), 0, u(st, "thoughts"), None) });
                let ok = s(&v, "status").unwrap_or("success") == "success";
                if !ok && !self.failed {
                    sink.error(s(&v["error"], "message").unwrap_or("Gemini CLI reported an error").to_string());
                }
                self.done = true;
                sink.done(ok, Some(s(&v, "status").unwrap_or("success").to_string()));
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

    #[test]
    fn maps_stream_json() {
        let lines = [
            r#"{"type":"init","timestamp":"2026-10-29T10:00:00Z","session_id":"8f1c","model":"gemini-3-pro-preview"}"#,
            r#"{"type":"message","timestamp":"t","role":"user","content":"hi"}"#,
            r#"{"type":"message","timestamp":"t","role":"assistant","content":"Rea","delta":true}"#,
            r#"{"type":"message","timestamp":"t","role":"assistant","content":"ding","delta":true}"#,
            r#"{"type":"tool_use","timestamp":"t","tool_name":"read_file","tool_id":"rf-1","parameters":{"absolute_path":"D:\\p\\a.rs"}}"#,
            r#"{"type":"tool_result","timestamp":"t","tool_id":"rf-1","status":"success","output":"fn main(){}"}"#,
            r#"{"type":"tool_use","timestamp":"t","tool_name":"run_shell_command","tool_id":"sh-1","parameters":{"command":"cargo build"}}"#,
            r#"{"type":"tool_result","timestamp":"t","tool_id":"sh-1","status":"error","error":{"type":"exit","message":"exit 101"}}"#,
            r#"{"type":"result","timestamp":"t","status":"success","stats":{"total_tokens":30,"input_tokens":20,"output_tokens":10,"cached":5,"duration_ms":9,"tool_calls":2}}"#,
        ];
        let mut p = GeminiStream::default();
        let ev = collect(&mut p, &lines);
        assert!(matches!(&ev[0], AgentEvent::NativeSession { id, .. } if id == "8f1c"));
        assert!(matches!(&ev[2], AgentEvent::TextDelta { text } if text == "Rea"));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolStart { tool } if tool.kind == ToolKind::Read && tool.title.as_deref().unwrap_or("").contains("a.rs"))));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolUpdate { is_error: true, output: Some(o), .. } if o == "exit 101")));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Usage { usage } if usage.input_tokens == 20 && usage.cache_read_tokens == 5)));
        assert!(matches!(ev.last(), Some(AgentEvent::Done { ok: true, .. })));
    }
}
