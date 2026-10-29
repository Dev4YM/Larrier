//! Factory Droid: `droid exec -o stream-json --cwd <cwd> [-s id] [-m model]
//! [--auto medium|high] (-f prompt.md | <prompt>)`.
//! Event shapes are taken from the droid 0.156 binary.

use larrier_core::ToolStatus;
use serde_json::Value;
use tokio::sync::mpsc::UnboundedReceiver;

use super::{content_text, json_line, run_stream, s, tool, u, usage_from, with_file_mentions, LineParser};
use crate::events::{AgentEvent, EventSink, TurnCommand};
use crate::request::TurnRequest;

pub async fn run(req: TurnRequest, sink: EventSink, mut control: UnboundedReceiver<TurnCommand>) {
    let mut cmd = req.launch.command();
    cmd.current_dir(&req.cwd);
    cmd.args(["exec", "-o", "stream-json"]);
    cmd.arg("--cwd").arg(&req.cwd);
    if let Some(id) = &req.resume_id {
        cmd.args(["-s", id]);
    }
    if let Some(m) = &req.model {
        cmd.args(["-m", m]);
    }
    // --auto level / --skip-permissions-unsafe and --use-spec from the
    // chosen modes (read-only when no autonomy flag is given).
    cmd.args(&req.modes.args);
    cmd.args(&req.launch.extra_args);
    let prompt = with_file_mentions(&req, req.full_prompt());
    if prompt.chars().count() > 24_000 {
        match req.write_prompt_file(&prompt) {
            Some(p) => {
                cmd.arg("-f").arg(p);
            }
            None => {
                cmd.arg(prompt.chars().take(24_000).collect::<String>());
            }
        }
    } else {
        cmd.arg(prompt);
    }
    let mut parser = DroidStream::default();
    run_stream("Droid", cmd, None, &sink, &mut control, &mut parser).await;
}

#[derive(Default)]
pub struct DroidStream {
    done: bool,
    failed: bool,
}

impl LineParser for DroidStream {
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
            "message" => {
                if s(&v, "role") == Some("assistant") {
                    if let Some(t) = s(&v, "text").filter(|t| !t.trim().is_empty()) {
                        sink.send(AgentEvent::Text { text: t.to_string() });
                    }
                }
            }
            "reasoning" => {
                if let Some(t) = s(&v, "text").filter(|t| !t.trim().is_empty()) {
                    sink.send(AgentEvent::Reasoning { text: t.to_string(), duration_ms: None });
                }
            }
            "tool_call" => {
                let id = s(&v, "id").unwrap_or("tool").to_string();
                let name = s(&v, "toolName").or_else(|| s(&v, "toolId")).unwrap_or("tool");
                sink.send(AgentEvent::ToolStart { tool: tool(id, name, v.get("parameters").cloned().unwrap_or(Value::Null)) });
            }
            "tool_result" => {
                let id = s(&v, "id").unwrap_or("tool").to_string();
                let is_error = v.get("isError").and_then(|b| b.as_bool()).unwrap_or(false);
                let output = if is_error { s(&v["error"], "message").map(|s| s.to_string()).unwrap_or_else(|| content_text(&v["error"])) } else { content_text(&v["value"]) };
                sink.send(AgentEvent::ToolUpdate {
                    id,
                    status: Some(if is_error { ToolStatus::Failed } else { ToolStatus::Completed }),
                    output: Some(output),
                    diffs: Vec::new(),
                    is_error,
                    title: None,
                });
            }
            "completion" => {
                if let Some(id) = s(&v, "session_id") {
                    sink.send(AgentEvent::NativeSession { id: id.to_string(), path: None });
                }
                let us = &v["usage"];
                if us.is_object() {
                    let pick = |a: &str, b: &str| if us.get(a).is_some() { u(us, a) } else { u(us, b) };
                    sink.send(AgentEvent::Usage {
                        usage: usage_from(
                            pick("inputTokens", "input_tokens"),
                            pick("outputTokens", "output_tokens"),
                            pick("cacheReadTokens", "cache_read_input_tokens"),
                            pick("cacheCreationTokens", "cache_creation_input_tokens"),
                            pick("thinkingTokens", "reasoning_tokens"),
                            None,
                        ),
                    });
                }
                self.done = true;
                sink.done(!self.failed, Some("completion".into()));
            }
            "error" => {
                sink.error(s(&v, "message").unwrap_or("Droid error").to_string());
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
            r#"{"type":"system","subtype":"init","cwd":"D:\\p","session_id":"d-1","tools":[],"model":"claude-opus-4-8","reasoning_effort":"high"}"#,
            r#"{"type":"message","role":"user","id":"u1","text":"go","timestamp":1,"session_id":"d-1"}"#,
            r#"{"type":"tool_call","id":"toolu_1","messageId":"m1","toolId":"Execute","toolName":"Execute","parameters":{"command":"npm test"},"timestamp":2,"session_id":"d-1"}"#,
            r#"{"type":"tool_result","id":"toolu_1","messageId":"m1","toolId":"Execute","isError":false,"value":"3 passing","timestamp":3,"session_id":"d-1"}"#,
            r#"{"type":"message","role":"assistant","id":"a1","text":"Tests pass.","timestamp":4,"session_id":"d-1"}"#,
            r#"{"type":"completion","finalText":"Tests pass.","numTurns":2,"durationMs":900,"session_id":"d-1","timestamp":5,"usage":{"inputTokens":40,"outputTokens":8}}"#,
        ];
        let mut p = DroidStream::default();
        let ev = collect(&mut p, &lines);
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolStart { tool } if tool.kind == ToolKind::Shell)));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolUpdate { output: Some(o), .. } if o == "3 passing")));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Text { text } if text == "Tests pass.")));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Usage { usage } if usage.input_tokens == 40)));
        assert!(matches!(ev.last(), Some(AgentEvent::Done { ok: true, .. })));
    }
}
