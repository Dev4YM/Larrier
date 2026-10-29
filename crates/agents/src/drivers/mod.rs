//! Runtime drivers, one per protocol.

pub mod acp;
pub mod chat;
pub mod claude;
pub mod codex;
pub mod cursor;
pub mod droid;
pub mod gemini;
pub mod hermes;
pub mod opencode;
pub mod pi;
pub mod plain;
pub mod process;

use larrier_core::{PlanEntry, PlanStatus, Protocol, ToolCall, ToolKind, Usage};
use serde_json::Value;
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::events::{EventSink, TurnCommand};
use crate::request::TurnRequest;

pub async fn run(req: TurnRequest, sink: EventSink, control: UnboundedReceiver<TurnCommand>) {
    match req.protocol {
        Protocol::ClaudeStream => claude::run(req, sink, control).await,
        Protocol::AmpStream => claude::run_amp(req, sink, control).await,
        Protocol::QwenStream => claude::run_qwen(req, sink, control).await,
        Protocol::CodexExec => codex::run(req, sink, control).await,
        Protocol::CursorStream => cursor::run(req, sink, control).await,
        Protocol::GeminiStream => gemini::run(req, sink, control).await,
        Protocol::OpenCodeRun => opencode::run(req, sink, control).await,
        Protocol::PiJson => pi::run(req, sink, control).await,
        Protocol::HermesStream => hermes::run(req, sink, control).await,
        Protocol::DroidStream => droid::run(req, sink, control).await,
        Protocol::Acp => acp::run(req, sink, control).await,
        Protocol::PlainText => plain::run(req, sink, control).await,
        Protocol::HttpChat => chat::run(req, sink, control).await,
        Protocol::None => {
            sink.error(format!("{} can't be run — it's an import-only source.", req.agent.display_name()));
            sink.done(false, Some("unsupported".into()));
        }
    }
}

/// A stdout line parser for a JSON-lines agent protocol.
pub trait LineParser {
    fn handle(&mut self, line: &str, sink: &EventSink);
    /// True once the parser has emitted `Done`.
    fn done(&self) -> bool;
    /// True when the stream reported a failure without emitting `Done`.
    fn failed(&self) -> bool {
        false
    }
}

/// Spawn `cmd`, feed `stdin`, parse stdout with `parser`, and guarantee
/// exactly one `Done` event.
pub async fn run_stream<P: LineParser>(
    name: &str,
    cmd: Command,
    stdin: Option<String>,
    sink: &EventSink,
    control: &mut UnboundedReceiver<TurnCommand>,
    parser: &mut P,
) {
    let outcome = process::run_lines(cmd, stdin, sink, control, |line| parser.handle(line, sink)).await;
    if parser.done() {
        return;
    }
    if outcome.cancelled {
        sink.done(false, Some("cancelled".into()));
    } else if outcome.success() && !parser.failed() {
        sink.done(true, None);
    } else {
        if !parser.failed() || !outcome.success() {
            sink.error(outcome.error_message(name));
        }
        sink.done(false, Some("error".into()));
    }
}

/// Parse a JSON line, logging anything that isn't JSON.
pub fn json_line(line: &str, sink: &EventSink) -> Option<Value> {
    let t = line.trim();
    if !t.starts_with('{') {
        sink.log(t.to_string());
        return None;
    }
    match serde_json::from_str(t) {
        Ok(v) => Some(v),
        Err(_) => {
            sink.log(t.to_string());
            None
        }
    }
}

pub fn s<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(|x| x.as_str())
}

pub fn u(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(|x| x.as_u64().or_else(|| x.as_f64().map(|f| f as u64))).unwrap_or(0)
}

/// Add canonical aliases (`file_path`, `pattern`, `command`) so LSF titles and
/// diff inference work across every agent's argument naming.
pub fn normalize_input(input: Value) -> Value {
    let Value::Object(mut map) = input else { return input };
    let alias = |map: &mut serde_json::Map<String, Value>, from: &str, to: &str| {
        if !map.contains_key(to) {
            if let Some(v) = map.get(from).cloned() {
                map.insert(to.to_string(), v);
            }
        }
    };
    for k in ["absolute_path", "filePath", "target_file", "targetFile", "file", "filename"] {
        alias(&mut map, k, "file_path");
    }
    for k in ["globPattern", "glob_pattern", "query", "regex"] {
        alias(&mut map, k, "pattern");
    }
    for k in ["cmd", "commandLine", "command_line", "script"] {
        alias(&mut map, k, "command");
    }
    for k in ["oldText", "old_str", "oldString", "old_text"] {
        alias(&mut map, k, "old_string");
    }
    for k in ["newText", "new_str", "newString", "new_text"] {
        alias(&mut map, k, "new_string");
    }
    for k in ["fileText", "file_text", "contents"] {
        alias(&mut map, k, "content");
    }
    Value::Object(map)
}

/// A tool call with normalized input, inferred diffs and a fresh title.
pub fn tool(id: impl Into<String>, name: &str, input: Value) -> ToolCall {
    let mut t = ToolCall::new(id, name, normalize_input(input));
    t.title = Some(t.describe());
    t.infer_diffs();
    t
}

pub fn tool_of_kind(id: impl Into<String>, name: &str, kind: ToolKind, input: Value) -> ToolCall {
    let mut t = ToolCall::new(id, name, normalize_input(input));
    t.kind = kind;
    t.title = Some(t.describe());
    t.infer_diffs();
    t
}

/// Flatten tool output that may be a string, an array of content blocks or an object.
pub fn content_text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .map(|i| match i.get("type").and_then(|t| t.as_str()) {
                Some("text") | Some("output_text") | Some("input_text") => i.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string(),
                Some("image") => "[image]".to_string(),
                _ => match i {
                    Value::String(s) => s.clone(),
                    other => other.get("text").and_then(|t| t.as_str()).map(|s| s.to_string()).unwrap_or_else(|| other.to_string()),
                },
            })
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(m) => {
            for k in ["output", "text", "content", "stdout", "result", "value", "message"] {
                if let Some(x) = m.get(k) {
                    let t = content_text(x);
                    if !t.is_empty() {
                        return t;
                    }
                }
            }
            v.to_string()
        }
        other => other.to_string(),
    }
}

/// Todo lists in any of the common shapes → plan entries.
pub fn plan_entries(items: &Value) -> Option<Vec<PlanEntry>> {
    let arr = items.as_array()?;
    Some(
        arr.iter()
            .map(|t| {
                let content = t
                    .get("content")
                    .or_else(|| t.get("text"))
                    .or_else(|| t.get("title"))
                    .or_else(|| t.get("description"))
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .to_string();
                let status = if t.get("completed").and_then(|c| c.as_bool()) == Some(true) {
                    PlanStatus::Completed
                } else {
                    match t.get("status").and_then(|s| s.as_str()).unwrap_or("").to_ascii_lowercase().as_str() {
                        "completed" | "done" | "complete" => PlanStatus::Completed,
                        "in_progress" | "in-progress" | "active" | "inprogress" | "running" => PlanStatus::InProgress,
                        "cancelled" | "canceled" => PlanStatus::Cancelled,
                        _ => PlanStatus::Pending,
                    }
                };
                PlanEntry { content, status, priority: t.get("priority").and_then(|p| p.as_str()).map(|s| s.to_string()) }
            })
            .collect(),
    )
}

pub fn usage_from(input: u64, output: u64, cache_read: u64, cache_write: u64, reasoning: u64, cost: Option<f64>) -> Usage {
    Usage { input_tokens: input, output_tokens: output, cache_read_tokens: cache_read, cache_write_tokens: cache_write, reasoning_tokens: reasoning, cost_usd: cost }
}

/// `@path` mentions for attached files, appended to text prompts.
pub fn with_file_mentions(req: &TurnRequest, prompt: String) -> String {
    let mut files = req.file_paths();
    files.extend(req.image_paths());
    if files.is_empty() {
        return prompt;
    }
    let mut p = prompt;
    p.push_str("\n\nAttached files:\n");
    for f in files {
        p.push_str(&format!("- @{f}\n"));
    }
    p
}

/// Run a blocking `git diff --numstat` for one file (best-effort line counts
/// for agents that report edited paths without hunks).
pub fn numstat(cwd: &std::path::Path, path: &str) -> Option<(u32, u32)> {
    let mut c = std::process::Command::new("git");
    c.current_dir(cwd).args(["diff", "--numstat", "HEAD", "--", path]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    let out = c.output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().next()?;
    let mut it = line.split('\t');
    Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
}

#[cfg(test)]
pub(crate) mod testutil {
    use crate::events::{AgentEvent, EventSink};
    use tokio::sync::mpsc;

    /// Feed lines into a parser and collect the events it emits.
    pub fn collect<P: super::LineParser>(parser: &mut P, lines: &[&str]) -> Vec<AgentEvent> {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sink = EventSink::new(tx);
        for l in lines {
            parser.handle(l, &sink);
        }
        drop(sink);
        let mut out = Vec::new();
        while let Ok(e) = rx.try_recv() {
            if !matches!(e, AgentEvent::Log { .. }) {
                out.push(e);
            }
        }
        out
    }
}
