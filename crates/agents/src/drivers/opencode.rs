//! OpenCode: `opencode run --format json --thinking --dir <cwd> [-s ses_…]
//! [-m provider/model] [--agent build|plan] [--auto] <message>`.

use larrier_core::{FileDiff, RunMode, ToolKind, ToolStatus};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedReceiver;

use super::{content_text, json_line, plan_entries, run_stream, s, tool, u, usage_from, LineParser};
use crate::events::{AgentEvent, EventSink, TurnCommand};
use crate::request::TurnRequest;

pub async fn run(req: TurnRequest, sink: EventSink, mut control: UnboundedReceiver<TurnCommand>) {
    let mut cmd = req.launch.command();
    cmd.current_dir(&req.cwd);
    cmd.args(["run", "--format", "json", "--thinking"]);
    cmd.arg("--dir").arg(&req.cwd);
    if let Some(id) = &req.resume_id {
        cmd.args(["-s", id]);
    }
    if let Some(m) = &req.model {
        cmd.args(["-m", m]);
    }
    if let Some(agent) = req.mode.as_deref().filter(|m| *m == "build" || *m == "plan") {
        cmd.args(["--agent", agent]);
    }
    // In json mode OpenCode rejects permission prompts unless --auto is set;
    // tools its config already allows still run.
    if req.run_mode == RunMode::FullAuto {
        cmd.arg("--auto");
    }
    for f in req.file_paths().into_iter().chain(req.image_paths()) {
        cmd.args(["-f", &f]);
    }
    cmd.args(&req.launch.extra_args);
    let mut prompt = req.prompt_for_arg(24_000);
    if prompt.starts_with('-') {
        prompt.insert_str(0, "Task: ");
    }
    cmd.arg(prompt);
    let mut parser = OpenCodeStream::default();
    run_stream("OpenCode", cmd, None, &sink, &mut control, &mut parser).await;
}

#[derive(Default)]
pub struct OpenCodeStream {
    session: Option<String>,
    failed: bool,
}

impl LineParser for OpenCodeStream {
    fn handle(&mut self, line: &str, sink: &EventSink) {
        let Some(v) = json_line(line, sink) else { return };
        if let Some(id) = s(&v, "sessionID") {
            if self.session.as_deref() != Some(id) {
                self.session = Some(id.to_string());
                sink.send(AgentEvent::NativeSession { id: id.to_string(), path: None });
            }
        }
        let part = &v["part"];
        match s(&v, "type").unwrap_or("") {
            "text" => {
                if let Some(t) = s(part, "text").filter(|t| !t.trim().is_empty()) {
                    sink.send(AgentEvent::Text { text: t.to_string() });
                }
            }
            "reasoning" => {
                if let Some(t) = s(part, "text").filter(|t| !t.trim().is_empty()) {
                    let dur = part["time"]["end"].as_u64().zip(part["time"]["start"].as_u64()).map(|(e, b)| e.saturating_sub(b));
                    sink.send(AgentEvent::Reasoning { text: t.to_string(), duration_ms: dur });
                }
            }
            "tool_use" => {
                let id = s(part, "callID").or_else(|| s(part, "id")).unwrap_or("tool").to_string();
                let name = s(part, "tool").unwrap_or("tool");
                let state = &part["state"];
                let input = state.get("input").cloned().unwrap_or(Value::Null);
                let t = tool(id.clone(), name, input.clone());
                if t.kind == ToolKind::Todo {
                    if let Some(entries) = input.get("todos").and_then(plan_entries) {
                        sink.send(AgentEvent::Plan { entries });
                    }
                }
                let path = t.locations.first().cloned().unwrap_or_default();
                sink.send(AgentEvent::ToolStart { tool: t });
                let status = s(state, "status").unwrap_or("completed");
                if status == "completed" || status == "error" {
                    let is_error = status == "error";
                    let output = if is_error { s(state, "error").unwrap_or("error").to_string() } else { content_text(&state["output"]) };
                    let diffs = state["metadata"]
                        .get("diff")
                        .and_then(|d| d.as_str())
                        .map(|d| vec![FileDiff::from_unified(path, d.to_string())])
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
            }
            "step_finish" => {
                let t = &part["tokens"];
                if t.is_object() {
                    sink.send(AgentEvent::Usage {
                        usage: usage_from(u(t, "input"), u(t, "output"), u(&t["cache"], "read"), u(&t["cache"], "write"), u(t, "reasoning"), part["cost"].as_f64()),
                    });
                }
            }
            "error" => {
                let e = &v["error"];
                let msg = s(&e["data"], "message").or_else(|| s(e, "message")).or_else(|| s(e, "name")).unwrap_or("OpenCode error");
                sink.error(msg.to_string());
                self.failed = true;
            }
            _ => {}
        }
    }

    fn done(&self) -> bool {
        false
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
    fn maps_run_json() {
        let lines = [
            r#"{"type":"step_start","timestamp":1,"sessionID":"ses_abc","part":{"type":"step-start"}}"#,
            r#"{"type":"reasoning","timestamp":2,"sessionID":"ses_abc","part":{"type":"reasoning","text":"plan it","time":{"start":10,"end":2010}}}"#,
            r#"{"type":"tool_use","timestamp":3,"sessionID":"ses_abc","part":{"type":"tool","tool":"edit","callID":"call_1","state":{"status":"completed","input":{"filePath":"src/x.ts","oldString":"a","newString":"b"},"output":"ok","title":"src/x.ts","metadata":{"diff":"--- a/src/x.ts\n+++ b/src/x.ts\n@@ -1 +1 @@\n-a\n+b\n"}}}}"#,
            r#"{"type":"text","timestamp":4,"sessionID":"ses_abc","part":{"type":"text","text":"Updated x.ts","time":{"start":1,"end":2}}}"#,
            r#"{"type":"step_finish","timestamp":5,"sessionID":"ses_abc","part":{"type":"step-finish","reason":"stop","tokens":{"input":100,"output":20,"reasoning":5,"cache":{"read":50,"write":0}},"cost":0.01}}"#,
        ];
        let mut p = OpenCodeStream::default();
        let ev = collect(&mut p, &lines);
        assert!(matches!(&ev[0], AgentEvent::NativeSession { id, .. } if id == "ses_abc"));
        assert!(matches!(&ev[1], AgentEvent::Reasoning { duration_ms: Some(2000), .. }));
        assert!(matches!(&ev[2], AgentEvent::ToolStart { tool } if tool.kind == ToolKind::Edit));
        assert!(matches!(&ev[3], AgentEvent::ToolUpdate { diffs, .. } if diffs[0].added == 1 && diffs[0].removed == 1));
        assert!(matches!(&ev[4], AgentEvent::Text { text } if text == "Updated x.ts"));
        assert!(matches!(&ev[5], AgentEvent::Usage { usage } if usage.cost_usd == Some(0.01)));
    }
}
