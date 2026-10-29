//! Pi coding agent: `pi --mode json [--session <path|id>] [--model m] <prompt>`.

use std::collections::HashSet;

use larrier_core::{FileDiff, ToolStatus};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedReceiver;

use super::{content_text, json_line, run_stream, s, tool, u, usage_from, LineParser};
use crate::events::{AgentEvent, EventSink, TurnCommand};
use crate::request::TurnRequest;

pub async fn run(req: TurnRequest, sink: EventSink, mut control: UnboundedReceiver<TurnCommand>) {
    let mut cmd = req.launch.command();
    cmd.current_dir(&req.cwd);
    cmd.args(["--mode", "json"]);
    if let Some(target) = req.resume_path.as_ref().or(req.resume_id.as_ref()) {
        cmd.args(["--session", target]);
    }
    if let Some(m) = &req.model {
        cmd.args(["--model", m]);
    }
    cmd.args(&req.launch.extra_args);
    for f in req.file_paths().into_iter().chain(req.image_paths()) {
        cmd.arg(format!("@{f}"));
    }
    cmd.arg(req.prompt_for_arg(24_000));
    let mut parser = PiStream::default();
    run_stream("Pi", cmd, None, &sink, &mut control, &mut parser).await;
}

#[derive(Default)]
pub struct PiStream {
    done: bool,
    failed: bool,
    /// Whether the current assistant message streamed deltas.
    streamed_text: bool,
    streamed_thinking: bool,
    tool_paths: std::collections::HashMap<String, String>,
    started: HashSet<String>,
}

impl LineParser for PiStream {
    fn handle(&mut self, line: &str, sink: &EventSink) {
        let Some(v) = json_line(line, sink) else { return };
        match s(&v, "type").unwrap_or("") {
            "session" => {
                if let Some(id) = s(&v, "id") {
                    sink.send(AgentEvent::NativeSession { id: id.to_string(), path: None });
                }
            }
            "message_start" => {
                self.streamed_text = false;
                self.streamed_thinking = false;
            }
            "message_update" => {
                let e = &v["assistantMessageEvent"];
                let delta = s(e, "delta").unwrap_or("");
                match s(e, "type").unwrap_or("") {
                    "text_delta" if !delta.is_empty() => {
                        self.streamed_text = true;
                        sink.text(delta);
                    }
                    "thinking_delta" if !delta.is_empty() => {
                        self.streamed_thinking = true;
                        sink.send(AgentEvent::ReasoningDelta { text: delta.to_string() });
                    }
                    "error" => {
                        sink.error(s(e, "error").or_else(|| s(e, "message")).unwrap_or("model error").to_string());
                        self.failed = true;
                    }
                    _ => {}
                }
            }
            "message_end" => self.message_end(&v["message"], sink),
            "tool_execution_start" => {
                let id = s(&v, "toolCallId").unwrap_or("tool").to_string();
                let name = s(&v, "toolName").unwrap_or("tool");
                let t = tool(id.clone(), name, v.get("args").cloned().unwrap_or(Value::Null));
                if let Some(p) = t.locations.first() {
                    self.tool_paths.insert(id.clone(), p.clone());
                }
                if self.started.insert(id) {
                    sink.send(AgentEvent::ToolStart { tool: t });
                }
            }
            "tool_execution_end" => {
                let id = s(&v, "toolCallId").unwrap_or("tool").to_string();
                let is_error = v.get("isError").and_then(|b| b.as_bool()).unwrap_or(false);
                let result = &v["result"];
                let output = content_text(result.get("content").unwrap_or(result));
                let diffs = result["details"]
                    .get("diff")
                    .and_then(|d| d.as_str())
                    .map(|d| vec![FileDiff::from_unified(self.tool_paths.get(&id).cloned().unwrap_or_default(), d.to_string())])
                    .unwrap_or_default();
                sink.send(AgentEvent::ToolUpdate {
                    id,
                    status: Some(if is_error { ToolStatus::Failed } else { ToolStatus::Completed }),
                    output: Some(output),
                    diffs,
                    is_error,
                    title: None,
                });
            }
            "compaction_start" => sink.notice("Compacting context"),
            "auto_retry_start" => sink.log(v.to_string()),
            "agent_settled" | "agent_end" => {
                if !self.done {
                    self.done = true;
                    sink.done(!self.failed, Some(s(&v, "type").unwrap_or("").to_string()));
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

impl PiStream {
    fn message_end(&mut self, m: &Value, sink: &EventSink) {
        if s(m, "role") != Some("assistant") {
            return;
        }
        if let Some(blocks) = m["content"].as_array() {
            for b in blocks {
                match s(b, "type").unwrap_or("") {
                    "text" if !self.streamed_text => {
                        if let Some(t) = s(b, "text").filter(|t| !t.is_empty()) {
                            sink.send(AgentEvent::Text { text: t.to_string() });
                        }
                    }
                    "thinking" if !self.streamed_thinking => {
                        if let Some(t) = s(b, "thinking").or_else(|| s(b, "text")).filter(|t| !t.is_empty()) {
                            sink.send(AgentEvent::Reasoning { text: t.to_string(), duration_ms: None });
                        }
                    }
                    _ => {}
                }
            }
        }
        if let Some(model) = s(m, "model") {
            sink.send(AgentEvent::Model { model: model.to_string() });
        }
        let us = &m["usage"];
        if us.is_object() {
            let cost = us["cost"].get("total").and_then(|c| c.as_f64()).or_else(|| us["cost"].as_f64());
            sink.send(AgentEvent::Usage { usage: usage_from(u(us, "input"), u(us, "output"), u(us, "cacheRead"), u(us, "cacheWrite"), 0, cost) });
        }
        if s(m, "stopReason") == Some("error") {
            sink.error(s(m, "errorMessage").unwrap_or("Pi reported an error").to_string());
            self.failed = true;
        }
        self.streamed_text = false;
        self.streamed_thinking = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::testutil::collect;
    use larrier_core::ToolKind;

    #[test]
    fn maps_json_mode() {
        let lines = [
            r#"{"type":"session","version":3,"id":"019a-pi","timestamp":"2026-10-29T10:00:00Z","cwd":"D:\\p"}"#,
            r#"{"type":"agent_start"}"#,
            r#"{"type":"message_start","message":{"role":"assistant"}}"#,
            r#"{"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","contentIndex":0,"delta":"ok"}}"#,
            r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":1,"delta":"Editing"}}"#,
            r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"thinking","thinking":"ok"},{"type":"text","text":"Editing"}],"model":"claude-opus-5-5","usage":{"input":10,"output":4,"cacheRead":2,"cacheWrite":0,"totalTokens":16,"cost":{"total":0.002}},"stopReason":"toolUse"}}"#,
            r#"{"type":"tool_execution_start","toolCallId":"tc1","toolName":"edit","args":{"path":"a.py","oldText":"x","newText":"y"}}"#,
            r#"{"type":"tool_execution_end","toolCallId":"tc1","toolName":"edit","result":{"content":[{"type":"text","text":"Edited a.py"}],"details":{"diff":"-x\n+y\n"}},"isError":false}"#,
            r#"{"type":"agent_end"}"#,
            r#"{"type":"agent_settled"}"#,
        ];
        let mut p = PiStream::default();
        let ev = collect(&mut p, &lines);
        assert!(matches!(&ev[0], AgentEvent::NativeSession { id, .. } if id == "019a-pi"));
        assert_eq!(ev.iter().filter(|e| matches!(e, AgentEvent::Text { .. } | AgentEvent::TextDelta { .. })).count(), 1);
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolStart { tool } if tool.kind == ToolKind::Edit)));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolUpdate { diffs, .. } if diffs[0].path == "a.py")));
        assert_eq!(ev.iter().filter(|e| matches!(e, AgentEvent::Done { .. })).count(), 1);
    }
}
