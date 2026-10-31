//! Claude Code (`claude -p --output-format stream-json --verbose
//! --include-partial-messages`, prompt on stdin), plus the agents that speak
//! the same event shapes: Amp (`amp -x --stream-json`) and Qwen Code
//! (`qwen -p --output-format stream-json`).

use std::collections::{HashMap, HashSet};

use larrier_core::{lsf::MediaSource, AgentKind, FileDiff, Part, ToolCall, ToolKind, ToolStatus, Usage};
use larrier_formats::claude::{fix_tool_kind, loaded_tools, server_tool_result};
use larrier_formats::util::image_parts;
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedReceiver;

use super::{content_text, json_line, plan_entries, run_stream, s, tool, u, usage_from, with_file_mentions, LineParser};
use crate::events::{AgentEvent, AskOption, AskQuestion, EventSink, PermissionOption, PermissionRequest, Reply, RequestKind, TurnCommand};
use crate::request::TurnRequest;

pub async fn run(req: TurnRequest, sink: EventSink, control: UnboundedReceiver<TurnCommand>) {
    if req.agent == AgentKind::ClaudeCode {
        return run_live(req, sink, control).await;
    }
    run_print(req, sink, control).await
}

/// Claude Code over its stdio control protocol (what the Agent SDK uses):
/// the prompt goes in as a stream-json line and stdin stays open, so the
/// approvals, questions and plans Claude asks for reach the user live,
/// messages can be added to the running turn, and a stop is a clean
/// interrupt.
async fn run_live(req: TurnRequest, sink: EventSink, mut control: UnboundedReceiver<TurnCommand>) {
    let mut cmd = req.launch.command();
    cmd.current_dir(&req.cwd);
    cmd.args(["-p", "--output-format", "stream-json", "--verbose", "--include-partial-messages"]);
    cmd.args(["--input-format", "stream-json", "--permission-prompt-tool", "stdio", "--replay-user-messages"]);
    if let Some(target) = req.resume_path.as_ref().or(req.resume_id.as_ref()) {
        cmd.args(["--resume", target]);
        if let Some(at) = &req.resume_at {
            cmd.args(["--resume-session-at", at, "--fork-session"]);
        }
    }
    if let Some(m) = &req.model {
        cmd.args(["--model", m]);
    }
    // The chat runs as one of the user's agents (its prompt, tools and
    // MCP servers) rather than as Claude Code itself.
    if let Some(a) = &req.run_as {
        cmd.args(["--agent", a]);
    }
    browser_args(&mut cmd, &req);
    cmd.args(&req.modes.args);
    cmd.env_remove("CLAUDECODE");
    cmd.env("CLAUDE_CODE_FORWARD_SUBAGENT_TEXT", "1");
    cmd.args(&req.launch.extra_args);
    let mut live = ClaudeLive::default();
    let id = uuid::Uuid::new_v4().to_string();
    let first = live.user_line(&req.full_prompt(), &req.attachments, false, Some(&id));
    live.prompt = Some(id);
    let outcome = super::process::run_interactive(cmd, vec![first], &sink, &mut control, &mut live).await;
    if live.inner.done {
        return;
    }
    // The reply was done; only background work kept Claude running.
    if live.waiting {
        sink.done(true, Some("end_turn".into()));
        return;
    }
    if outcome.cancelled {
        sink.done(false, Some("cancelled".into()));
    } else if outcome.success() && !live.inner.failed() {
        sink.done(true, None);
    } else {
        sink.error(outcome.error_message("Claude Code"));
        sink.done(false, Some("error".into()));
    }
}

async fn run_print(req: TurnRequest, sink: EventSink, mut control: UnboundedReceiver<TurnCommand>) {
    let mut cmd = req.launch.command();
    cmd.current_dir(&req.cwd);
    cmd.args(["-p", "--output-format", "stream-json", "--verbose"]);
    if !matches!(req.agent, AgentKind::Custom(_)) {
        cmd.arg("--include-partial-messages");
        // Resuming from the file path is the most robust form (works for
        // sessions Larrier wrote and avoids cross-project id clashes).
        if let Some(target) = req.resume_path.as_ref().or(req.resume_id.as_ref()) {
            cmd.args(["--resume", target]);
            // An earlier prompt was edited: continue from just before it, in
            // a new session file (the original stays as it was).
            if let Some(at) = &req.resume_at {
                cmd.args(["--resume-session-at", at, "--fork-session"]);
            }
        }
        if let Some(m) = &req.model {
            cmd.args(["--model", m]);
        }
        if let Some(a) = &req.run_as {
            cmd.args(["--agent", a]);
        }
        browser_args(&mut cmd, &req);
        // --permission-mode from the chosen mode (Auto unless picked otherwise).
        cmd.args(&req.modes.args);
        // Claude Code refuses to start nested inside another Claude Code session.
        cmd.env_remove("CLAUDECODE");
        // Subagents' own text and thinking, not just their tool calls
        // (`--forward-subagent-text`; older CLIs ignore the variable).
        cmd.env("CLAUDE_CODE_FORWARD_SUBAGENT_TEXT", "1");
    }
    cmd.args(&req.launch.extra_args);
    let prompt = with_file_mentions(&req, req.full_prompt());
    let mut parser = ClaudeStream::default();
    run_stream("Claude Code", cmd, Some(prompt), &sink, &mut control, &mut parser).await;
}

pub async fn run_amp(req: TurnRequest, sink: EventSink, mut control: UnboundedReceiver<TurnCommand>) {
    let mut cmd = req.launch.command();
    cmd.current_dir(&req.cwd);
    if let Some(id) = &req.resume_id {
        cmd.args(["threads", "continue", id]);
    }
    let prompt = with_file_mentions(&req, req.prompt_for_arg(24_000));
    cmd.args(["-x", &prompt, "--stream-json"]);
    cmd.args(&req.modes.args);
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
    cmd.args(&req.modes.args);
    cmd.args(&req.launch.extra_args);
    cmd.args(["-p", &prompt]);
    let mut parser = ClaudeStream::default();
    run_stream("Qwen Code", cmd, None, &sink, &mut control, &mut parser).await;
}

/// Larrier's browser as an MCP server for this run, its tools allowed.
fn browser_args(cmd: &mut tokio::process::Command, req: &TurnRequest) {
    if let Some(b) = &req.browser {
        cmd.arg("--mcp-config").arg(b.mcp_json().to_string());
        cmd.arg("--allowedTools").arg(format!("mcp__{}", crate::browser::SERVER_NAME));
    }
}

/// A request Claude is waiting on.
struct Ask {
    kind: RequestKind,
    input: Value,
    suggestions: Value,
    tool_use_id: Option<String>,
    questions: Vec<String>,
}

/// One Claude Code process over the control protocol: the stream parser,
/// plus the requests, replies and messages that go back over stdin.
#[derive(Default)]
pub struct ClaudeLive {
    pub inner: ClaudeStream,
    out: Vec<String>,
    asks: HashMap<String, Ask>,
    /// Messages sent during the turn, by uuid, until Claude takes them in.
    steers: HashMap<String, (String, Vec<Part>, u64)>,
    /// `cancel_async_message` requests in flight: request id → steer key.
    unsteers: HashMap<String, u64>,
    host_requests: u64,
    stop_at: Option<std::time::Instant>,
    finished: bool,
    /// Cost reported so far: `total_cost_usd` adds up over a process's turns.
    cost_so_far: f64,
    /// The permission mode Claude reports it runs in.
    mode: Option<String>,
    /// The prompt this run started with, until Claude takes it in: a
    /// resumed session may first finish something queued before it (the
    /// notice about a background command the last run left behind).
    prompt: Option<String>,
    /// Background tasks running, as Claude counts them.
    background: usize,
    /// The reply is done and background tasks still run: the turn goes on
    /// until they end (Claude reports back on them), or the user stops it.
    waiting: bool,
    /// Background work ended while waiting and Claude hasn't reported back:
    /// past this, the turn ends anyway.
    linger: Option<std::time::Instant>,
}

/// Images Claude takes inline (the API's limit is 5 MB each).
const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024;

impl ClaudeLive {
    /// A prompt line: text, attached images as image blocks, other files as
    /// `@path` mentions.
    fn user_line(&mut self, text: &str, attachments: &[Part], now: bool, uuid: Option<&str>) -> String {
        let mut blocks: Vec<Value> = Vec::new();
        let mut mentions: Vec<String> = Vec::new();
        for a in attachments {
            match a {
                Part::Image { mime, source: MediaSource::Path { path } } => match image_block(path, mime.as_deref()) {
                    Some(b) => blocks.push(b),
                    None => mentions.push(path.clone()),
                },
                Part::Image { mime, source: MediaSource::Base64 { data } } => {
                    blocks.push(json!({"type": "image", "source": {"type": "base64", "media_type": mime.clone().unwrap_or_else(|| "image/png".into()), "data": data}}))
                }
                Part::File { path, text: None, .. } => mentions.push(path.clone()),
                _ => {}
            }
        }
        let mut text = text.to_string();
        if !mentions.is_empty() {
            text.push_str("\n\nAttached files:\n");
            for m in mentions {
                text.push_str(&format!("- @{m}\n"));
            }
        }
        blocks.insert(0, json!({"type": "text", "text": text}));
        let mut line = json!({"type": "user", "session_id": "", "message": {"role": "user", "content": blocks}, "parent_tool_use_id": null});
        if let Some(u) = uuid {
            line["uuid"] = json!(u);
        }
        if now {
            line["priority"] = json!("now");
        }
        line.to_string()
    }

    fn reply(&mut self, request_id: &str, response: Value) {
        self.out.push(json!({"type": "control_response", "response": {"subtype": "success", "request_id": request_id, "response": response}}).to_string());
    }

    fn on_request(&mut self, v: &Value, sink: &EventSink) {
        let Some(rid) = s(v, "request_id").map(String::from) else { return };
        let r = &v["request"];
        match s(r, "subtype").unwrap_or("") {
            "can_use_tool" => self.can_use_tool(rid, r, sink),
            // Only sent for hooks a host registers; Larrier registers none.
            "hook_callback" => self.reply(&rid, json!({"continue": true})),
            other => self.out.push(json!({"type": "control_response", "response": {"subtype": "error", "request_id": rid, "error": format!("Larrier doesn't handle {other}")}}).to_string()),
        }
    }

    fn can_use_tool(&mut self, rid: String, r: &Value, sink: &EventSink) {
        let name = s(r, "tool_name").unwrap_or("tool").to_string();
        let input = r.get("input").cloned().unwrap_or(json!({}));
        let tool_use_id = s(r, "tool_use_id").map(String::from);
        let suggestions = r.get("permission_suggestions").cloned().unwrap_or(Value::Null);
        let mut req = PermissionRequest { id: rid.clone(), tool_call_id: tool_use_id.clone(), ..Default::default() };
        let mut questions = Vec::new();
        match name.as_str() {
            "AskUserQuestion" => {
                req.kind = RequestKind::Question;
                req.title = "Claude has a question".into();
                for q in input["questions"].as_array().into_iter().flatten() {
                    let text = s(q, "question").unwrap_or("").to_string();
                    questions.push(text.clone());
                    req.questions.push(AskQuestion {
                        id: None,
                        header: s(q, "header").map(String::from),
                        question: text,
                        multi: q["multiSelect"].as_bool().unwrap_or(false),
                        options: q["options"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|o| Some(AskOption { label: s(o, "label")?.to_string(), description: s(o, "description").map(String::from) }))
                            .collect(),
                    });
                }
            }
            "ExitPlanMode" => {
                req.kind = RequestKind::Plan;
                req.title = "Claude's plan is ready".into();
                req.plan = s(&input, "plan").map(String::from);
                req.options = vec![
                    PermissionOption { id: "approve_edits".into(), label: "Approve and auto-accept edits".into(), kind: "allow_always".into() },
                    PermissionOption { id: "approve".into(), label: "Approve, ask before edits".into(), kind: "allow_once".into() },
                    PermissionOption { id: "keep".into(), label: "Keep planning".into(), kind: "reject_once".into() },
                ];
            }
            _ => {
                let call = claude_tool(tool_use_id.clone().unwrap_or_default(), &name, input.clone());
                // What it is about to do, not a log line ("Ran …").
                req.title = match call.kind {
                    ToolKind::Shell => s(&input, "command").map(String::from),
                    ToolKind::Write => call.locations.first().map(|p| format!("Write {p}")),
                    ToolKind::Edit => call.locations.first().map(|p| format!("Edit {p}")),
                    _ => None,
                }
                .or_else(|| call.title.clone())
                .unwrap_or_else(|| name.clone());
                let reason = s(r, "decision_reason").map(super::process::strip_ansi);
                req.detail = [s(r, "description").map(String::from), reason, s(r, "blocked_path").map(|p| format!("Path: {p}"))]
                    .into_iter()
                    .flatten()
                    .filter(|d| !d.trim().is_empty() && Some(d.as_str()) != call.title.as_deref())
                    .reduce(|a, b| format!("{a}\n{b}"));
                req.options.push(PermissionOption { id: "allow_once".into(), label: "Allow".into(), kind: "allow_once".into() });
                let always = suggestions.as_array().is_some_and(|a| !a.is_empty()) && r["suppress_always_allow_rule"].as_bool() != Some(true);
                if always {
                    req.options.push(PermissionOption { id: "allow_always".into(), label: always_label(&suggestions), kind: "allow_always".into() });
                }
                req.options.push(PermissionOption { id: "reject".into(), label: "Deny".into(), kind: "reject_once".into() });
                if let Some(id) = &tool_use_id {
                    sink.send(AgentEvent::ToolUpdate { id: id.clone(), status: Some(ToolStatus::Approval), output: None, diffs: Vec::new(), is_error: false, title: None });
                }
            }
        }
        self.asks.insert(rid, Ask { kind: req.kind, input, suggestions, tool_use_id, questions });
        sink.send(AgentEvent::Permission { request: req });
    }

    fn answer(&mut self, request_id: &str, reply: Reply, sink: &EventSink) {
        let Some(ask) = self.asks.remove(request_id) else { return };
        let deny = |message: String| json!({"behavior": "deny", "message": message});
        let response = match ask.kind {
            RequestKind::Question if reply.answers.iter().all(|a| a.is_empty()) => deny(reply.message.unwrap_or_else(|| "The user dismissed the question.".into())),
            RequestKind::Question => {
                let mut input = ask.input.clone();
                let answers: serde_json::Map<String, Value> = ask.questions.iter().zip(&reply.answers).map(|(q, a)| (q.clone(), json!(a.join(", ")))).collect();
                input["answers"] = Value::Object(answers);
                json!({"behavior": "allow", "updatedInput": input})
            }
            RequestKind::Plan => match reply.option_id.as_deref() {
                Some("approve_edits") => {
                    json!({"behavior": "allow", "updatedInput": ask.input, "updatedPermissions": [{"type": "setMode", "mode": "acceptEdits", "destination": "session"}]})
                }
                Some("approve") => json!({"behavior": "allow", "updatedInput": ask.input}),
                _ => deny(reply.message.filter(|m| !m.trim().is_empty()).unwrap_or_else(|| "Keep planning: the user isn't ready for you to start yet.".into())),
            },
            RequestKind::Tool => match reply.option_id.as_deref() {
                Some("allow_once") => json!({"behavior": "allow", "updatedInput": ask.input}),
                Some("allow_always") => json!({"behavior": "allow", "updatedInput": ask.input, "updatedPermissions": ask.suggestions}),
                _ => deny(reply.message.filter(|m| !m.trim().is_empty()).unwrap_or_else(|| "The user denied this.".into())),
            },
        };
        let allowed = response["behavior"] == "allow";
        if let Some(id) = &ask.tool_use_id {
            if allowed && ask.kind == RequestKind::Tool {
                sink.send(AgentEvent::ToolUpdate { id: id.clone(), status: Some(ToolStatus::Running), output: None, diffs: Vec::new(), is_error: false, title: None });
            }
            if !allowed {
                self.inner.user_denied.insert(id.clone());
            }
        }
        self.reply(request_id, response);
    }

    fn on_result(&mut self, v: &Value, sink: &EventSink) {
        let total = v["total_cost_usd"].as_f64();
        let cost = total.map(|t| (t - self.cost_so_far).max(0.0));
        if let Some(t) = total {
            self.cost_so_far = t;
        }
        let stopped = self.stop_at.is_some();
        // Not our turn: Claude finished something queued before our prompt,
        // which it hasn't taken in yet. Its turn comes next.
        if self.prompt.is_some() && !stopped && v["is_error"].as_bool() != Some(true) {
            sink.log(format!("claude: a turn ended before the prompt was taken in ({} turns)", v["num_turns"]));
            return;
        }
        if stopped {
            for (id, ask) in self.asks.drain() {
                if let Some(t) = ask.tool_use_id {
                    self.inner.user_denied.insert(t);
                }
                sink.send(AgentEvent::PermissionClosed { id });
            }
        }
        let (ok, subtype) = self.inner.turn_end(v, sink, cost, stopped);
        // A message sent during the turn that Claude has not taken in yet
        // starts another turn in this process.
        if !stopped && !self.steers.is_empty() {
            return;
        }
        // Background commands still run: keep Claude going so they finish
        // (closing it would kill them) and it hears when they do.
        if !stopped && ok && self.background > 0 {
            self.waiting = true;
            self.linger = None;
            sink.send(AgentEvent::Idle);
            return;
        }
        self.inner.done = true;
        self.finished = true;
        sink.done(ok, Some(subtype));
    }
}

impl super::process::Interactive for ClaudeLive {
    fn handle(&mut self, line: &str, sink: &EventSink) {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            self.inner.handle(line, sink);
            return;
        };
        match s(&v, "type").unwrap_or("") {
            "control_request" => self.on_request(&v, sink),
            "control_cancel_request" => {
                if let Some(id) = s(&v, "request_id") {
                    if self.asks.remove(id).is_some() {
                        sink.send(AgentEvent::PermissionClosed { id: id.to_string() });
                    }
                }
            }
            // Claude's answer to taking a message back.
            "control_response" => {
                let r = &v["response"];
                if let Some(key) = s(r, "request_id").and_then(|id| self.unsteers.remove(id)) {
                    let taken_back = s(r, "subtype") == Some("success") && r["response"]["cancelled"].as_bool() == Some(true);
                    if taken_back {
                        self.steers.retain(|_, (_, _, k)| *k != key);
                    }
                    sink.send(AgentEvent::Unsteered { key, taken_back });
                }
            }
            "keep_alive" => {}
            // A prompt line taken in: messages sent during the turn are
            // recorded where Claude read them.
            "user" if v["isReplay"].as_bool() == Some(true) => {
                let uuid = s(&v, "uuid");
                if uuid.is_some() && uuid == self.prompt.as_deref() {
                    self.prompt = None;
                } else if let Some((text, parts, _)) = uuid.and_then(|u| self.steers.remove(u)) {
                    sink.send(AgentEvent::Steered { text, attachments: parts });
                }
            }
            "system" if s(&v, "subtype") == Some("background_tasks_changed") => {
                self.background = v["tasks"].as_array().map(|a| a.len()).unwrap_or(0);
                // Claude reports back on finished work in a turn of its own.
                if self.waiting && self.background == 0 {
                    self.linger = Some(std::time::Instant::now() + std::time::Duration::from_secs(60));
                }
                self.inner.handle(line, sink);
            }
            // The mode Claude runs in; it changes when a plan is approved.
            "system" if s(&v, "permissionMode").is_some() => {
                let mode = s(&v, "permissionMode").unwrap_or("default").to_string();
                if s(&v, "subtype") == Some("status") && self.mode.as_deref().is_some_and(|m| m != mode) {
                    sink.send(AgentEvent::Mode { id: mode.clone() });
                }
                self.mode = Some(mode);
                self.inner.handle(line, sink);
            }
            "result" => self.on_result(&v, sink),
            _ => self.inner.handle(line, sink),
        }
    }

    fn command(&mut self, cmd: TurnCommand, sink: &EventSink) {
        match cmd {
            TurnCommand::Cancel => {
                // Only background work was left: stopping ends it with Claude.
                if self.waiting && !self.finished {
                    self.inner.done = true;
                    self.finished = true;
                    sink.done(false, Some("cancelled".into()));
                    return;
                }
                if self.stop_at.is_none() && !self.finished {
                    self.host_requests += 1;
                    self.out.push(json!({"type": "control_request", "request_id": format!("larrier_{}", self.host_requests), "request": {"subtype": "interrupt"}}).to_string());
                    self.stop_at = Some(std::time::Instant::now() + std::time::Duration::from_secs(8));
                }
            }
            TurnCommand::Permission { request_id, reply } => self.answer(&request_id, reply, sink),
            TurnCommand::Steer { text, attachments, now, key } => {
                if self.finished {
                    return;
                }
                let id = uuid::Uuid::new_v4().to_string();
                let line = self.user_line(&text, &attachments, now, Some(&id));
                self.steers.insert(id, (text, attachments, key));
                self.out.push(line);
            }
            TurnCommand::Unsteer { key } => {
                let uuid = self.steers.iter().find(|(_, (_, _, k))| *k == key).map(|(u, _)| u.clone());
                match uuid {
                    // Claude holds it until its next step: ask for it back.
                    Some(uuid) if !self.finished => {
                        self.host_requests += 1;
                        let rid = format!("larrier_{}", self.host_requests);
                        self.unsteers.insert(rid.clone(), key);
                        self.out.push(json!({"type": "control_request", "request_id": rid, "request": {"subtype": "cancel_async_message", "message_uuid": uuid}}).to_string());
                    }
                    _ => sink.send(AgentEvent::Unsteered { key, taken_back: false }),
                }
            }
        }
    }

    fn take_output(&mut self) -> Vec<String> {
        std::mem::take(&mut self.out)
    }

    fn finished(&self) -> bool {
        self.finished
    }

    fn stop_deadline(&self) -> Option<std::time::Instant> {
        self.stop_at.or(self.linger)
    }
}

/// An inline image block for an attached image file.
fn image_block(path: &str, mime: Option<&str>) -> Option<Value> {
    use base64::Engine;
    let lower = path.to_ascii_lowercase();
    let media = mime.map(String::from).or_else(|| {
        [("png", "image/png"), ("jpg", "image/jpeg"), ("jpeg", "image/jpeg"), ("gif", "image/gif"), ("webp", "image/webp")]
            .iter()
            .find(|(e, _)| lower.ends_with(&format!(".{e}")))
            .map(|(_, m)| m.to_string())
    })?;
    if !matches!(media.as_str(), "image/png" | "image/jpeg" | "image/gif" | "image/webp") || std::fs::metadata(path).ok()?.len() > MAX_IMAGE_BYTES {
        return None;
    }
    let data = base64::engine::general_purpose::STANDARD.encode(std::fs::read(path).ok()?);
    Some(json!({"type": "image", "source": {"type": "base64", "media_type": media, "data": data}}))
}

/// The "always allow" choice, named by the rule Claude Code would add.
fn always_label(suggestions: &Value) -> String {
    let rules: Vec<String> = suggestions
        .as_array()
        .into_iter()
        .flatten()
        .filter(|u| s(u, "type") == Some("addRules"))
        .flat_map(|u| u["rules"].as_array().cloned().unwrap_or_default())
        .filter_map(|r| s(&r, "ruleContent").map(|c| format!("{}({c})", s(&r, "toolName").unwrap_or(""))).or_else(|| s(&r, "toolName").map(String::from)))
        .collect();
    let mode = suggestions.as_array().into_iter().flatten().find(|u| s(u, "type") == Some("setMode")).and_then(|u| s(u, "mode"));
    match (rules.first(), mode) {
        (Some(r), _) => format!("Always allow {r}"),
        (None, Some("acceptEdits")) => "Allow all edits this session".into(),
        _ => "Always allow".into(),
    }
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
    /// Background task id (shell id / async agent id) → tool call id.
    bg_by_task: HashMap<String, String>,
    /// Tool calls started with `run_in_background`.
    bg_tools: HashSet<String>,
    /// TaskStop / KillShell call id → task id it targets.
    stop_calls: HashMap<String, String>,
    /// BashOutput / TaskOutput call id → task id it reads.
    output_calls: HashMap<String, String>,
    /// Tool ids already seen inside subagents.
    side_tools: HashSet<String>,
    /// The model each subagent (by parent tool id) was last seen on.
    side_models: HashMap<String, String>,
    /// Subagent message ids whose usage was recorded.
    side_usage: HashSet<String>,
    /// Tool calls already marked denied by a `permission_denied` event.
    pub(crate) denied: HashSet<String>,
    /// Tool calls the user denied (or stopped while they waited): not
    /// "denied without asking".
    pub(crate) user_denied: HashSet<String>,
    /// An API retry streak was announced (until the next response starts).
    retrying: bool,
    /// A safeguard refusal was announced (its `fallback` block and the
    /// result's `refusal` stop reason need no second notice).
    refusal_noted: bool,
    switch_noted: bool,
    /// The output-token cut-off was announced.
    cutoff_noted: bool,
    /// Tool calls the model is still writing, by content block index.
    writing: HashMap<u64, Writing>,
    /// Tool calls shown while being written: their full input still has
    /// to replace the partial one.
    partial_tools: HashSet<String>,
}

/// A tool call whose input is still streaming in.
struct Writing {
    id: String,
    name: String,
    json: String,
    shown_len: usize,
    shown_at: Option<std::time::Instant>,
}

/// How often a tool call being written is redrawn.
const WRITING_EVERY: std::time::Duration = std::time::Duration::from_millis(120);

impl LineParser for ClaudeStream {
    fn handle(&mut self, line: &str, sink: &EventSink) {
        let Some(v) = json_line(line, sink) else { return };
        // Subagent (Task / Agent) traffic carries parent_tool_use_id: record
        // it into the parent tool's nested transcript.
        if let Some(parent) = s(&v, "parent_tool_use_id").filter(|p| !p.is_empty()) {
            let parent = parent.to_string();
            self.side(&v, &parent, sink);
            return;
        }
        match s(&v, "type").unwrap_or("") {
            "system" => self.system(&v, sink),
            "stream_event" => self.stream_event(&v["event"], sink),
            "assistant" => self.assistant(&v, sink),
            "user" => self.user(&v, sink),
            "result" => self.result(&v, sink),
            "rate_limit_event" => rate_limit(&v["rate_limit_info"], sink),
            // Heartbeats, tool-use summaries, prompt suggestions, auth
            // progress and /clear resets carry nothing a transcript keeps.
            "tool_progress" | "tool_use_summary" | "prompt_suggestion" | "auth_status" | "conversation_reset" => {}
            _ => sink.log(line),
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
            "api_retry" => {
                // One notice per retry streak; every attempt goes to the log.
                if !self.retrying {
                    self.retrying = true;
                    let status = v["error_status"].as_u64().map(|c| format!(" ({c})")).unwrap_or_default();
                    let what = match s(v, "error").unwrap_or("unknown") {
                        "overloaded" => "is overloaded".to_string(),
                        "rate_limit" => "rate limit hit".to_string(),
                        "server_error" => "server error".to_string(),
                        "unknown" => "request failed".to_string(),
                        other => other.replace('_', " "),
                    };
                    sink.notice(format!("Claude API {what}{status}, retrying…"));
                }
                sink.log(format!("API retry {}/{}: {}", u(v, "attempt"), u(v, "max_retries"), s(v, "error").unwrap_or("")));
            }
            "permission_denied" => {
                if let Some(id) = s(v, "tool_use_id") {
                    self.denied.insert(id.to_string());
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
            "informational" | "local_command_output" => {
                if let Some(c) = s(v, "content").map(str::trim).filter(|c| !c.is_empty()) {
                    sink.notice(c);
                }
            }
            "model_refusal_fallback" => {
                self.refusal_noted = true;
                self.switch_noted = true;
                let to = s(v, "fallback_model").unwrap_or("the fallback model");
                sink.notice(s(v, "content").map(str::trim).filter(|c| !c.is_empty()).map(String::from).unwrap_or_else(|| format!("Safeguards flagged this message; retrying on {to}")));
            }
            "model_refusal_no_fallback" => {
                self.refusal_noted = true;
                sink.notice(s(v, "content").map(str::trim).filter(|c| !c.is_empty()).unwrap_or("Safeguards stopped this response"));
            }
            "task_notification" => {
                // `worker_restart`: neither finished nor failed.
                if s(v, "reason") != Some("worker_restart") {
                    let target = s(v, "tool_use_id").map(String::from).or_else(|| s(v, "task_id").and_then(|t| self.bg_by_task.get(t).cloned()));
                    if let (Some(id), Some(status)) = (target, task_status(s(v, "status").unwrap_or("completed"))) {
                        let output = s(v, "summary").filter(|x| !x.is_empty()).map(String::from);
                        sink.send(AgentEvent::ToolUpdate { id, status: Some(status), output, diffs: Vec::new(), is_error: false, title: None });
                    }
                }
            }
            "task_updated" => {
                let target = s(v, "task_id").and_then(|t| self.bg_by_task.get(t).cloned());
                if let (Some(id), Some(status)) = (target, s(&v["patch"], "status").and_then(task_status).filter(|st| *st != ToolStatus::Running)) {
                    sink.send(AgentEvent::ToolUpdate { id, status: Some(status), output: None, diffs: Vec::new(), is_error: false, title: None });
                }
            }
            "hook_response" => {
                if s(v, "outcome") == Some("error") {
                    let why = [s(v, "stderr"), s(v, "output"), s(v, "stdout")].into_iter().flatten().flat_map(str::lines).map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
                    let event = s(v, "hook_event").or_else(|| s(v, "hook_name")).unwrap_or("A");
                    sink.notice(format!("{event} hook failed{}{why}", if why.is_empty() { "" } else { ": " }));
                }
            }
            "plugin_install" => {
                if s(v, "status") == Some("failed") {
                    sink.notice(format!("Plugin install failed: {} {}", s(v, "name").unwrap_or(""), s(v, "error").unwrap_or("")).trim_end().to_string());
                }
            }
            // Progress and bookkeeping with no transcript effect: hook start /
            // progress, the compacting status, thinking-token estimates, task
            // start / progress / live-set snapshots (completion arrives as
            // task_notification), session state, checkpoints, command lists,
            // shutdown and control-request progress.
            "hook_started" | "hook_progress" | "status" | "thinking_tokens" | "task_started" | "task_progress" | "background_tasks_changed" | "session_state_changed" | "files_persisted" | "commands_changed"
            | "worker_shutting_down" | "control_request_progress" => {}
            _ => sink.log(v.to_string()),
        }
    }

    fn stream_event(&mut self, ev: &Value, sink: &EventSink) {
        match s(ev, "type").unwrap_or("") {
            "message_start" => {
                self.current_msg = ev["message"]["id"].as_str().map(|s| s.to_string());
                self.retrying = false;
                self.writing.clear();
            }
            // A tool call starts: show it right away, then as it is written.
            "content_block_start" => {
                let b = &ev["content_block"];
                if let (Some("tool_use"), Some(id), Some(name), Some(i)) = (s(b, "type"), s(b, "id"), s(b, "name"), ev["index"].as_u64()) {
                    if self.seen_tools.insert(id.to_string()) {
                        self.partial_tools.insert(id.to_string());
                        self.writing.insert(i, Writing { id: id.to_string(), name: name.to_string(), json: String::new(), shown_len: 0, shown_at: None });
                        sink.send(AgentEvent::ToolStart { tool: claude_tool(id.to_string(), name, json!({})) });
                    }
                }
            }
            "content_block_stop" => {
                if let Some(i) = ev["index"].as_u64() {
                    self.writing.remove(&i);
                }
            }
            "message_delta" if s(&ev["delta"], "stop_reason") == Some("max_tokens") && !self.cutoff_noted => {
                self.cutoff_noted = true;
                sink.notice("Response hit the output token limit");
            }
            // The reply is done; background work may still keep the turn.
            "message_delta" if s(&ev["delta"], "stop_reason") == Some("end_turn") => sink.send(AgentEvent::Idle),
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
                    // A tool call's input as it is written (a command, a
                    // file's new content): redrawn a few times a second.
                    "input_json_delta" => {
                        let (Some(i), Some(part)) = (ev["index"].as_u64(), d["partial_json"].as_str()) else { return };
                        let Some(w) = self.writing.get_mut(&i) else { return };
                        w.json.push_str(part);
                        let due = w.shown_at.map_or(true, |t| t.elapsed() >= WRITING_EVERY);
                        if due && w.json.len() > w.shown_len {
                            if let Some(input) = complete_json(&w.json) {
                                w.shown_len = w.json.len();
                                w.shown_at = Some(std::time::Instant::now());
                                sink.send(AgentEvent::ToolStart { tool: claude_tool(w.id.clone(), &w.name, input) });
                            }
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
        // Each API call's usage tells how full the context is now (token
        // totals come with the result).
        if let Some(us) = msg.get("usage").filter(|u| u.is_object()) {
            let context = u(us, "input_tokens") + u(us, "cache_read_input_tokens") + u(us, "cache_creation_input_tokens");
            if context > 0 {
                sink.send(AgentEvent::Usage { usage: Usage { context_tokens: Some(context), ..Default::default() } });
            }
        }
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
                    if let Some(t) = b["thinking"].as_str().filter(|t| !t.is_empty()) {
                        sink.send(AgentEvent::Reasoning { text: t.to_string(), duration_ms: v["thinking_duration_ms"].as_u64() });
                    }
                }
                "tool_use" | "server_tool_use" => {
                    let id = s(b, "id").unwrap_or("").to_string();
                    // New, or shown while it was written: the full input now.
                    if self.seen_tools.insert(id.clone()) || self.partial_tools.remove(&id) {
                        let name = s(b, "name").unwrap_or("tool");
                        let input = b.get("input").cloned().unwrap_or(Value::Null);
                        if name == "TodoWrite" {
                            if let Some(entries) = input.get("todos").and_then(plan_entries) {
                                sink.send(AgentEvent::Plan { entries });
                            }
                        }
                        self.track_background(&id, name, &input);
                        sink.send(AgentEvent::ToolStart { tool: claude_tool(id, name, input) });
                    }
                }
                // Model switch (`--fallback-model`, or a safeguard refusal
                // already announced by its system event).
                "fallback" => {
                    if !std::mem::take(&mut self.switch_noted) {
                        let from = s(&b["from"], "model").unwrap_or("the requested model");
                        let to = s(&b["to"], "model").unwrap_or("a fallback model");
                        sink.notice(format!("Switched from {from} to {to}"));
                    }
                }
                // `redacted_thinking` is encrypted: nothing to show.
                _ => {
                    if let Some((id, output, is_error)) = server_tool_result(b) {
                        let status = if is_error { ToolStatus::Failed } else { ToolStatus::Completed };
                        sink.send(AgentEvent::ToolUpdate { id, status: Some(status), output: Some(output), diffs: Vec::new(), is_error, title: None });
                    }
                }
            }
        }
        // Without partial messages the end of the reply comes with it.
        if s(msg, "stop_reason") == Some("end_turn") {
            sink.send(AgentEvent::Idle);
        }
    }

    fn user(&mut self, v: &Value, sink: &EventSink) {
        let meta = v.get("tool_use_result").cloned();
        let content = &v["message"]["content"];
        // Background task notifications arrive as plain user text.
        if let Some(t) = content.as_str() {
            self.task_notifications(t, sink);
            return;
        }
        let Some(blocks) = content.as_array() else { return };
        for b in blocks {
            match s(b, "type") {
                Some("text") => self.task_notifications(s(b, "text").unwrap_or(""), sink),
                Some("tool_result") => {
                    let id = s(b, "tool_use_id").unwrap_or("").to_string();
                    let is_error = b.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false);
                    let (output, images) = result_content(&b["content"]);
                    let diffs = meta.as_ref().map(diffs_from_result).unwrap_or_default();
                    if !images.is_empty() {
                        sink.send(AgentEvent::ToolImages { id: id.clone(), images });
                    }
                    self.tool_result(id, output, is_error, diffs, meta.as_ref(), sink);
                }
                _ => {}
            }
        }
    }

    fn tool_result(&mut self, id: String, output: String, is_error: bool, diffs: Vec<FileDiff>, meta: Option<&Value>, sink: &EventSink) {
        // Subagent facts from the Agent / Task result.
        if let Some(m) = meta.filter(|m| m.is_object()) {
            let dur = m.get("totalDurationMs").and_then(|d| d.as_u64());
            let agent_name = s(m, "agentType").map(|a| a.to_string());
            let is_async = m.get("isAsync").and_then(|a| a.as_bool()).unwrap_or(false) || s(m, "status") == Some("async_launched");
            if dur.is_some() || agent_name.is_some() || is_async {
                sink.send(AgentEvent::ToolMeta { id: id.clone(), duration_ms: dur, background: is_async.then_some(true), agent_name });
            }
            if is_async && !is_error {
                self.bg_tools.insert(id.clone());
                if let Some(a) = s(m, "agentId") {
                    self.bg_by_task.insert(a.to_string(), id.clone());
                }
            }
            if let Some(t) = s(m, "backgroundTaskId") {
                self.bg_by_task.insert(t.to_string(), id.clone());
            }
        }
        if self.bg_tools.contains(&id) && !is_error {
            // Launched in the background: keep it running until a
            // notification / TaskStop / BashOutput says otherwise.
            if let Some(task) = background_task_id(&output) {
                self.bg_by_task.entry(task).or_insert_with(|| id.clone());
            }
            sink.send(AgentEvent::ToolUpdate { id, status: Some(ToolStatus::Running), output: Some(output), diffs, is_error: false, title: None });
            return;
        }
        if let Some(task) = self.stop_calls.remove(&id) {
            if !is_error {
                if let Some(target) = self.bg_by_task.get(&task).cloned() {
                    sink.send(AgentEvent::ToolUpdate { id: target, status: Some(ToolStatus::Cancelled), output: None, diffs: Vec::new(), is_error: false, title: None });
                }
            }
        }
        if let Some(task) = self.output_calls.remove(&id) {
            if let (Some(target), Some(state)) = (self.bg_by_task.get(&task).cloned(), output_status(&output)) {
                sink.send(AgentEvent::ToolUpdate { id: target, status: Some(state), output: Some(output.clone()), diffs: Vec::new(), is_error: false, title: None });
            }
        }
        sink.send(AgentEvent::ToolUpdate {
            id,
            status: Some(if is_error { ToolStatus::Failed } else { ToolStatus::Completed }),
            output: Some(output),
            diffs,
            is_error,
            title: None,
        });
    }

    /// Remember calls that start, read or stop background work.
    fn track_background(&mut self, id: &str, name: &str, input: &Value) {
        if input.get("run_in_background").and_then(|b| b.as_bool()).unwrap_or(false) || name == "Monitor" {
            self.bg_tools.insert(id.to_string());
        }
        let target = ["task_id", "shell_id", "bash_id", "agent_id", "id"].iter().find_map(|k| s(input, k)).map(|t| t.to_string());
        if let Some(t) = target {
            match name {
                "TaskStop" | "KillShell" | "KillBash" => {
                    self.stop_calls.insert(id.to_string(), t);
                }
                "BashOutput" | "TaskOutput" => {
                    self.output_calls.insert(id.to_string(), t);
                }
                _ => {}
            }
        }
    }

    /// `<task-notification>` blocks report background work finishing.
    fn task_notifications(&mut self, text: &str, sink: &EventSink) {
        for n in parse_task_notifications(text) {
            let target = n.tool_use_id.clone().or_else(|| n.task_id.as_ref().and_then(|t| self.bg_by_task.get(t).cloned()));
            let Some(target) = target else { continue };
            let Some(status) = task_status(n.status.as_deref().unwrap_or("completed")) else { continue };
            sink.send(AgentEvent::ToolUpdate { id: target, status: Some(status), output: n.summary.clone(), diffs: Vec::new(), is_error: false, title: None });
        }
    }

    /// An event inside a subagent: wrap it for the parent tool.
    fn side(&mut self, v: &Value, parent: &str, sink: &EventSink) {
        let wrap = |e: AgentEvent| AgentEvent::Subagent { parent_tool_id: parent.to_string(), inner: Box::new(e) };
        match s(v, "type").unwrap_or("") {
            "assistant" => {
                let msg = &v["message"];
                // What the subagent runs on, and how full its context is.
                if let Some(m) = s(msg, "model").filter(|m| self.side_models.get(parent).map(|x| x != m).unwrap_or(true)) {
                    self.side_models.insert(parent.to_string(), m.to_string());
                    sink.send(wrap(AgentEvent::Model { model: m.to_string() }));
                }
                if let (Some(mid), Some(us)) = (s(msg, "id"), msg.get("usage").filter(|u| u.is_object())) {
                    let context = u(us, "input_tokens") + u(us, "cache_read_input_tokens") + u(us, "cache_creation_input_tokens");
                    if context > 0 && self.side_usage.insert(mid.to_string()) {
                        sink.send(wrap(AgentEvent::Usage { usage: Usage { context_tokens: Some(context), ..Default::default() } }));
                    }
                }
                let Some(blocks) = msg.get("content").and_then(|c| c.as_array()) else { return };
                for b in blocks {
                    match s(b, "type").unwrap_or("") {
                        "text" => {
                            if let Some(t) = b["text"].as_str().filter(|t| !t.trim().is_empty()) {
                                sink.send(wrap(AgentEvent::Text { text: t.to_string() }));
                            }
                        }
                        "thinking" => {
                            if let Some(t) = b["thinking"].as_str().filter(|t| !t.trim().is_empty()) {
                                sink.send(wrap(AgentEvent::Reasoning { text: t.to_string(), duration_ms: None }));
                            }
                        }
                        "tool_use" | "server_tool_use" => {
                            let id = s(b, "id").unwrap_or("").to_string();
                            if self.side_tools.insert(id.clone()) {
                                let name = s(b, "name").unwrap_or("tool");
                                let input = b.get("input").cloned().unwrap_or(Value::Null);
                                sink.send(wrap(AgentEvent::ToolStart { tool: claude_tool(id, name, input) }));
                            }
                        }
                        _ => {
                            if let Some((id, output, is_error)) = server_tool_result(b) {
                                let status = if is_error { ToolStatus::Failed } else { ToolStatus::Completed };
                                sink.send(wrap(AgentEvent::ToolUpdate { id, status: Some(status), output: Some(output), diffs: Vec::new(), is_error, title: None }));
                            }
                        }
                    }
                }
            }
            "user" => {
                let meta = v.get("tool_use_result").cloned();
                let Some(blocks) = v["message"].get("content").and_then(|c| c.as_array()) else { return };
                for b in blocks {
                    if s(b, "type") != Some("tool_result") {
                        continue;
                    }
                    let id = s(b, "tool_use_id").unwrap_or("").to_string();
                    let is_error = b.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false);
                    let diffs = meta.as_ref().map(diffs_from_result).unwrap_or_default();
                    let (output, images) = result_content(&b["content"]);
                    if !images.is_empty() {
                        sink.send(wrap(AgentEvent::ToolImages { id: id.clone(), images }));
                    }
                    sink.send(wrap(AgentEvent::ToolUpdate {
                        id,
                        status: Some(if is_error { ToolStatus::Failed } else { ToolStatus::Completed }),
                        output: Some(output),
                        diffs,
                        is_error,
                        title: None,
                    }));
                }
            }
            _ => {}
        }
    }

    fn result(&mut self, v: &Value, sink: &EventSink) {
        let (ok, subtype) = self.turn_end(v, sink, v["total_cost_usd"].as_f64(), false);
        self.done = true;
        sink.done(ok, Some(subtype));
    }

    /// What a `result` reports, short of ending the turn: `cost` is this
    /// turn's own, and `stopped` marks the user's stop (its error result is
    /// no failure). Returns whether it succeeded, and its subtype.
    pub(crate) fn turn_end(&mut self, v: &Value, sink: &EventSink, cost: Option<f64>, stopped: bool) -> (bool, String) {
        let us = &v["usage"];
        if us.is_object() {
            sink.send(AgentEvent::Usage {
                usage: usage_from(
                    u(us, "input_tokens"),
                    u(us, "output_tokens"),
                    u(us, "cache_read_input_tokens"),
                    u(us, "cache_creation_input_tokens"),
                    // Part of output_tokens; shown as a breakdown.
                    u(&us["output_tokens_details"], "thinking_tokens"),
                    cost,
                ),
            });
        }
        if let Some(id) = s(v, "session_id") {
            sink.send(AgentEvent::NativeSession { id: id.to_string(), path: None });
        }
        // The authoritative list of denied calls (`permission_denied`
        // events are best-effort). A headless run can't ask, so say why.
        let denials = v["permission_denials"].as_array().cloned().unwrap_or_default();
        let mut denied_tools: Vec<&str> = Vec::new();
        for d in &denials {
            if let Some(id) = s(d, "tool_use_id").filter(|id| self.denied.insert(id.to_string())) {
                sink.send(AgentEvent::ToolUpdate { id: id.to_string(), status: Some(ToolStatus::Failed), output: None, diffs: Vec::new(), is_error: true, title: None });
            }
            let by_user = s(d, "tool_use_id").is_some_and(|id| self.user_denied.contains(id));
            if let Some(name) = s(d, "tool_name").filter(|n| !by_user && !denied_tools.contains(n)) {
                denied_tools.push(name);
            }
        }
        if !denied_tools.is_empty() {
            sink.notice(format!("Claude Code denied {} without asking: this run's permission mode doesn't allow it", denied_tools.join(", ")));
        }
        match s(v, "stop_reason") {
            Some("max_tokens") if !self.cutoff_noted => sink.notice("Response hit the output token limit"),
            Some("refusal") if !self.refusal_noted => sink.notice("Claude's safeguards stopped this response"),
            _ => {}
        }
        let is_error = v.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false);
        let subtype = s(v, "subtype").unwrap_or("success").to_string();
        if stopped {
            return (false, "cancelled".to_string());
        }
        if is_error {
            let errors = v
                .get("errors")
                .and_then(|e| e.as_array())
                .map(|a| a.iter().map(content_text).collect::<Vec<_>>().join("\n"))
                .filter(|s| !s.is_empty());
            let label = match subtype.as_str() {
                "error_max_turns" => "Stopped at the turn limit (--max-turns)".to_string(),
                "error_max_budget_usd" => "Stopped at the spending limit (--max-budget-usd)".to_string(),
                "error_max_structured_output_retries" => "Gave up producing valid structured output".to_string(),
                "error_during_execution" => "Claude Code failed while running the turn".to_string(),
                other => other.replace('_', " "),
            };
            let msg = s(v, "result").map(|s| s.to_string()).filter(|s| !s.is_empty()).or(errors).unwrap_or(label);
            sink.error(msg);
            self.failed = true;
        }
        (!is_error, subtype)
    }

    fn mark_streamed(&mut self) {
        if let Some(id) = &self.current_msg {
            self.streamed.insert(id.clone());
        }
    }
}

/// One `<task-notification>` block.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct TaskNotification {
    pub task_id: Option<String>,
    pub tool_use_id: Option<String>,
    pub status: Option<String>,
    pub summary: Option<String>,
}

fn tag(block: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let i = block.find(&open)? + open.len();
    let j = block[i..].find(&close)? + i;
    Some(block[i..j].trim().to_string()).filter(|v| !v.is_empty())
}

/// Parse every `<task-notification>` in a message (Claude Code's report of a
/// background shell / agent / monitor finishing).
pub fn parse_task_notifications(text: &str) -> Vec<TaskNotification> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("<task-notification>") {
        let after = &rest[i + "<task-notification>".len()..];
        let end = after.find("</task-notification>").unwrap_or(after.len());
        let block = &after[..end];
        out.push(TaskNotification {
            task_id: tag(block, "task-id"),
            tool_use_id: tag(block, "tool-use-id"),
            status: tag(block, "status").map(|s| s.to_lowercase()),
            summary: tag(block, "summary").or_else(|| tag(block, "result")),
        });
        rest = &after[end..];
    }
    out
}

/// A Claude Code tool call, with the kinds the generic classifier gets wrong
/// fixed (TaskCreate is not a file write, ToolSearch is not a code search).
fn claude_tool(id: String, name: &str, input: Value) -> ToolCall {
    let mut t = tool(id, name, input);
    fix_tool_kind(&mut t);
    t
}

/// Tool result text and images. Image blocks (MCP / browser screenshots)
/// travel as images, not "[image]" lines; `tool_reference` blocks
/// (ToolSearch) read as "Loaded X" instead of raw JSON.
fn result_content(content: &Value) -> (String, Vec<Part>) {
    let images = image_parts(content);
    let loaded = loaded_tools(content);
    let text = match content.as_array() {
        Some(items) if !images.is_empty() || loaded.is_some() => {
            let rest: Vec<Value> = items.iter().filter(|b| !matches!(s(b, "type"), Some("image") | Some("tool_reference"))).cloned().collect();
            content_text(&Value::Array(rest))
        }
        _ => content_text(content),
    };
    let text = match loaded {
        Some(l) if text.trim().is_empty() => l,
        Some(l) => format!("{text}\n{l}"),
        None => text,
    };
    (text, images)
}

/// Background task status words (notifications, `task_updated` patches).
fn task_status(status: &str) -> Option<ToolStatus> {
    Some(match status.to_ascii_lowercase().as_str() {
        "failed" | "error" => ToolStatus::Failed,
        "killed" | "stopped" | "cancelled" | "canceled" => ToolStatus::Cancelled,
        "running" => ToolStatus::Running,
        "pending" => return None,
        _ => ToolStatus::Completed,
    })
}

/// A JSON object cut off mid-stream, closed so it parses: an open string
/// is ended, open arrays and objects are closed, and a key or comma left
/// hanging is dropped. None while nothing usable has arrived yet.
pub fn complete_json(partial: &str) -> Option<Value> {
    fn close(s: &str) -> Option<Value> {
        let mut stack = Vec::new();
        let (mut in_str, mut esc) = (false, false);
        for c in s.chars() {
            if in_str {
                match (esc, c) {
                    (true, _) => esc = false,
                    (false, '\\') => esc = true,
                    (false, '"') => in_str = false,
                    _ => {}
                }
                continue;
            }
            match c {
                '"' => in_str = true,
                '{' => stack.push('}'),
                '[' => stack.push(']'),
                '}' | ']' => {
                    stack.pop();
                }
                _ => {}
            }
        }
        let mut out = s.to_string();
        if in_str {
            if esc {
                out.pop();
            }
            out.push('"');
        }
        out.extend(stack.iter().rev());
        serde_json::from_str(&out).ok()
    }
    if let Some(v) = close(partial) {
        return Some(v);
    }
    // A key or value cut short: go back to the last complete member.
    let (mut in_str, mut esc, mut last_comma) = (false, false, None);
    for (i, c) in partial.char_indices() {
        if in_str {
            match (esc, c) {
                (true, _) => esc = false,
                (false, '\\') => esc = true,
                (false, '"') => in_str = false,
                _ => {}
            }
        } else if c == '"' {
            in_str = true;
        } else if c == ',' {
            last_comma = Some(i);
        }
    }
    close(&partial[..last_comma?])
}

/// `rate_limit_event`: the window's usage for the plan-limit chip, and a
/// notice when the limit stops the turn.
fn rate_limit(info: &Value, sink: &EventSink) {
    let window = match s(info, "rateLimitType") {
        Some("five_hour") => " (5-hour limit)",
        Some("seven_day") | Some("seven_day_opus") | Some("seven_day_sonnet") => " (weekly limit)",
        _ => "",
    };
    // Same names as the plan limits read from the account.
    let name = match s(info, "rateLimitType") {
        Some("five_hour") => Some("5-hour"),
        Some("seven_day") => Some("Weekly"),
        Some("seven_day_opus") => Some("Weekly · Opus"),
        Some("seven_day_sonnet") => Some("Weekly · Sonnet"),
        _ => None,
    };
    let used = info["utilization"].as_f64().map(|u| u * 100.0).or_else(|| (s(info, "status") == Some("rejected")).then_some(100.0));
    if let (Some(name), Some(used_pct)) = (name, used) {
        sink.send(AgentEvent::RateLimit { window: name.to_string(), used_pct, resets_at: info["resetsAt"].as_i64() });
    }
    match s(info, "status") {
        Some("rejected") => {
            let resets = info["resetsAt"]
                .as_i64()
                .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
                .map(|t| format!(" · resets {}", t.with_timezone(&chrono::Local).format("%a %H:%M")))
                .unwrap_or_default();
            let credits = if s(info, "errorCode") == Some("credits_required") { " · buy usage credits to continue" } else { "" };
            sink.notice(format!("Usage limit reached{window}{resets}{credits}"));
        }
        Some(st) => sink.log(format!("rate limit {st}{window}: {:.0}% used", info["utilization"].as_f64().unwrap_or(0.0) * 100.0)),
        None => {}
    }
}

/// "Command running in background with ID: b8b97ntfm." → `b8b97ntfm`.
pub fn background_task_id(output: &str) -> Option<String> {
    let i = output.find("with ID:")? + "with ID:".len();
    let id: String = output[i..].trim_start().chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect();
    (!id.is_empty()).then_some(id)
}

/// Status reported by BashOutput / TaskOutput (`<status>completed</status>`).
fn output_status(output: &str) -> Option<ToolStatus> {
    let st = tag(output, "status").or_else(|| {
        let i = output.to_ascii_lowercase().find("status:")? + "status:".len();
        Some(output[i..].split_whitespace().next()?.trim_matches(|c: char| !c.is_ascii_alphanumeric()).to_lowercase())
    })?;
    Some(match st.as_str() {
        "running" => ToolStatus::Running,
        "failed" | "error" => ToolStatus::Failed,
        "killed" | "stopped" | "cancelled" => ToolStatus::Cancelled,
        "completed" | "done" | "exited" => ToolStatus::Completed,
        _ => return None,
    })
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
    use crate::drivers::process::Interactive;

    /// Feed a recorded control-protocol run to `ClaudeLive`, answering each
    /// request with `answer`; returns the events and the lines written back.
    fn live(fixture: &str, mut answer: impl FnMut(&PermissionRequest) -> Option<TurnCommand>) -> (Vec<AgentEvent>, Vec<Value>) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = EventSink::new(tx);
        let mut p = ClaudeLive::default();
        let mut out = Vec::new();
        for l in fixture.lines().filter(|l| !l.trim().is_empty()) {
            p.handle(l, &sink);
            while let Ok(e) = rx.try_recv() {
                if let AgentEvent::Permission { request } = &e {
                    if let Some(c) = answer(request) {
                        p.command(c, &sink);
                    }
                }
                out.push(e);
            }
        }
        while let Ok(e) = rx.try_recv() {
            out.push(e);
        }
        let written = p.take_output().iter().map(|l| serde_json::from_str(l).unwrap()).collect();
        (out.into_iter().filter(|e| !matches!(e, AgentEvent::Log { .. })).collect(), written)
    }

    fn reply(id: &str, option: &str) -> Option<TurnCommand> {
        Some(TurnCommand::Permission { request_id: id.into(), reply: Reply::option(Some(option.into())) })
    }

    /// Feed lines to a live run started with prompt `p1`; its events, and
    /// whether it closed (finished).
    fn feed(p: &mut ClaudeLive, lines: &[&str]) -> Vec<AgentEvent> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = EventSink::new(tx);
        for l in lines {
            p.handle(l, &sink);
        }
        let mut out = Vec::new();
        while let Ok(e) = rx.try_recv() {
            out.push(e);
        }
        out.into_iter().filter(|e| !matches!(e, AgentEvent::Log { .. })).collect()
    }

    /// Real sequence (Claude Code 2.1.310, resumed after a run that left a
    /// background command): Claude first ends a turn of its own about that
    /// command, then takes in the prompt. The first end isn't ours.
    #[test]
    fn a_turn_before_the_prompt_is_not_ours() {
        let mut p = ClaudeLive { prompt: Some("p1".into()), ..Default::default() };
        let ev = feed(&mut p, &[
            r#"{"type":"system","subtype":"task_notification","task_id":"b1","tool_use_id":"toolu_old","status":"stopped","summary":"Background shell command didn't finish before the previous session ended"}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"num_turns":0,"result":"","session_id":"s1","total_cost_usd":0}"#,
        ]);
        assert!(!ev.iter().any(|e| matches!(e, AgentEvent::Done { .. })), "{ev:?}");
        assert!(!p.finished());
        let ev = feed(&mut p, &[
            r#"{"type":"user","isReplay":true,"uuid":"p1","message":{"role":"user","content":[{"type":"text","text":"see this"}]}}"#,
            r#"{"type":"assistant","message":{"id":"m1","role":"assistant","content":[{"type":"text","text":"SECOND"}],"stop_reason":"end_turn"}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"num_turns":1,"result":"SECOND","session_id":"s1","total_cost_usd":0.01}"#,
        ]);
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Text { text } if text == "SECOND")));
        assert!(matches!(ev.last(), Some(AgentEvent::Done { ok: true, .. })));
        assert!(p.finished());
        // An error before the prompt is taken in is still the run's.
        let mut q = ClaudeLive { prompt: Some("p1".into()), ..Default::default() };
        let ev = feed(&mut q, &[r#"{"type":"result","subtype":"error_during_execution","is_error":true,"num_turns":0,"session_id":"s1"}"#]);
        assert!(matches!(ev.last(), Some(AgentEvent::Done { ok: false, .. })), "{ev:?}");
    }

    /// Real sequence: a background command outlives the reply. Claude keeps
    /// running until it ends, then reports back in a turn of its own.
    #[test]
    fn waits_for_background_work() {
        let mut p = ClaudeLive::default();
        let ev = feed(&mut p, &[
            r#"{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"b9","task_type":"local_bash","description":"ping"}]}"#,
            r#"{"type":"assistant","message":{"id":"m1","role":"assistant","content":[{"type":"text","text":"STARTED"}],"stop_reason":"end_turn"}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"num_turns":2,"result":"STARTED","session_id":"s1","total_cost_usd":0.01}"#,
        ]);
        assert!(!ev.iter().any(|e| matches!(e, AgentEvent::Done { .. })), "{ev:?}");
        assert!(matches!(ev.last(), Some(AgentEvent::Idle)));
        assert!(!p.finished() && p.waiting);
        let ev = feed(&mut p, &[
            r#"{"type":"system","subtype":"task_notification","task_id":"b9","status":"completed"}"#,
            r#"{"type":"system","subtype":"background_tasks_changed","tasks":[]}"#,
            r#"{"type":"assistant","message":{"id":"m2","role":"assistant","content":[{"type":"text","text":"FINISHED"}],"stop_reason":"end_turn"}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"num_turns":1,"result":"FINISHED","session_id":"s1","total_cost_usd":0.02}"#,
        ]);
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Text { text } if text == "FINISHED")));
        assert!(matches!(ev.last(), Some(AgentEvent::Done { ok: true, .. })));
        assert!(p.finished());
        // Stop while only background work is left ends the turn at once.
        let mut q = ClaudeLive::default();
        feed(&mut q, &[
            r#"{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"b9"}]}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"num_turns":1,"session_id":"s1"}"#,
        ]);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        q.command(TurnCommand::Cancel, &EventSink::new(tx));
        assert!(matches!(rx.try_recv(), Ok(AgentEvent::Done { ok: false, .. })));
        assert!(q.finished());
    }

    /// Real run (Claude Code 2.1.310): `echo hi > a.txt` in Manual mode,
    /// approved once in Larrier.
    #[test]
    fn live_approval() {
        let (ev, written) = live(include_str!("../../tests/fixtures/claude_live_approval.jsonl"), |r| reply(&r.id, "allow_once"));
        let req = ev.iter().find_map(|e| match e { AgentEvent::Permission { request } => Some(request), _ => None }).unwrap();
        assert_eq!(req.kind, RequestKind::Tool);
        assert!(req.title.contains("echo hi > a.txt"), "{}", req.title);
        let ids: Vec<&str> = req.options.iter().map(|o| o.id.as_str()).collect();
        assert_eq!(ids, ["allow_once", "allow_always", "reject"]);
        assert_eq!(req.options[1].label, "Always allow Bash(echo hi *)");
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolUpdate { status: Some(ToolStatus::Approval), .. })));
        let r = &written[0]["response"];
        assert_eq!((r["request_id"].as_str(), r["response"]["behavior"].as_str()), (Some(req.id.as_str()), Some("allow")));
        assert_eq!(r["response"]["updatedInput"]["command"], "echo hi > a.txt");
        assert!(matches!(ev.last(), Some(AgentEvent::Done { ok: true, .. })));
        assert!(!ev.iter().any(|e| matches!(e, AgentEvent::Notice { text } if text.contains("without asking"))));
    }

    /// Real run: AskUserQuestion answered from the question card.
    #[test]
    fn live_question() {
        let (ev, written) = live(include_str!("../../tests/fixtures/claude_live_question.jsonl"), |r| {
            Some(TurnCommand::Permission { request_id: r.id.clone(), reply: Reply { answers: vec![vec!["Blue".into()]], ..Default::default() } })
        });
        let req = ev.iter().find_map(|e| match e { AgentEvent::Permission { request } => Some(request), _ => None }).unwrap();
        assert_eq!((req.kind, req.questions.len()), (RequestKind::Question, 1));
        let q = &req.questions[0];
        assert_eq!((q.question.as_str(), q.header.as_deref(), q.options.len(), q.multi), ("Which color do you prefer?", Some("Color"), 2, false));
        let resp = &written[0]["response"]["response"];
        assert_eq!(resp["behavior"], "allow");
        assert_eq!(resp["updatedInput"]["answers"]["Which color do you prefer?"], "Blue");
        assert_eq!(resp["updatedInput"]["questions"][0]["question"], "Which color do you prefer?", "the input goes back whole");
    }

    /// Real run: plan mode, the plan approved with edits auto-accepted;
    /// Claude reports the mode it moved to.
    #[test]
    fn live_plan() {
        let (ev, written) = live(include_str!("../../tests/fixtures/claude_live_plan.jsonl"), |r| reply(&r.id, "approve_edits"));
        let req = ev.iter().find_map(|e| match e { AgentEvent::Permission { request } => Some(request), _ => None }).unwrap();
        assert_eq!(req.kind, RequestKind::Plan);
        assert!(req.plan.as_deref().unwrap_or("").starts_with("# Plan: create plan.txt"));
        let resp = &written[0]["response"]["response"];
        assert_eq!(resp["updatedPermissions"][0]["mode"], "acceptEdits");
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Mode { id } if id == "acceptEdits")), "later turns start in the new mode");
    }

    /// A message sent into the turn can be taken back until Claude reads it
    /// (`cancel_async_message`); once read, Claude says so.
    #[test]
    fn live_take_back() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = EventSink::new(tx);
        let mut p = ClaudeLive::default();
        for (key, text) in [(7, "never mind"), (8, "too late")] {
            p.command(TurnCommand::Steer { text: text.into(), attachments: Vec::new(), now: false, key }, &sink);
        }
        let sent: Vec<Value> = p.take_output().iter().map(|l| serde_json::from_str(l).unwrap()).collect();
        p.command(TurnCommand::Unsteer { key: 7 }, &sink);
        let ask: Value = serde_json::from_str(&p.take_output()[0]).unwrap();
        assert_eq!((ask["request"]["subtype"].as_str(), ask["request"]["message_uuid"].as_str()), (Some("cancel_async_message"), sent[0]["uuid"].as_str()));
        let rid = ask["request_id"].as_str().unwrap();
        p.handle(&json!({"type": "control_response", "response": {"subtype": "success", "request_id": rid, "response": {"cancelled": true}}}).to_string(), &sink);
        // The other was already read: Claude answers it wasn't cancelled.
        p.command(TurnCommand::Unsteer { key: 8 }, &sink);
        let ask: Value = serde_json::from_str(&p.take_output()[0]).unwrap();
        p.handle(&json!({"type": "control_response", "response": {"subtype": "success", "request_id": ask["request_id"], "response": {"cancelled": false}}}).to_string(), &sink);
        // One Larrier never sent: nothing to ask.
        p.command(TurnCommand::Unsteer { key: 9 }, &sink);
        assert!(p.take_output().is_empty());
        let ev: Vec<AgentEvent> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        let answers: Vec<(u64, bool)> = ev.iter().filter_map(|e| match e { AgentEvent::Unsteered { key, taken_back } => Some((*key, *taken_back)), _ => None }).collect();
        assert_eq!(answers, [(7, true), (8, false), (9, false)]);
        // The turn may end now: only the message Claude kept is waited for.
        p.handle(&json!({"type": "user", "isReplay": true, "uuid": sent[1]["uuid"], "message": {"role": "user", "content": "too late"}}).to_string(), &sink);
        p.handle(r#"{"type":"result","subtype":"success","is_error":false,"result":"done","session_id":"s1","usage":{"input_tokens":1,"output_tokens":1}}"#, &sink);
        assert!(p.finished());
    }

    #[test]
    fn live_denial_steer_and_stop() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = EventSink::new(tx);
        let mut p = ClaudeLive::default();
        p.handle(r#"{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","display_name":"Bash","input":{"command":"rm -rf build"},"tool_use_id":"t1"}}"#, &sink);
        p.command(TurnCommand::Permission { request_id: "r1".into(), reply: Reply { option_id: Some("reject".into()), message: Some("Use git clean instead".into()), ..Default::default() } }, &sink);
        let r: Value = serde_json::from_str(&p.take_output()[0]).unwrap();
        assert_eq!(r["response"]["response"], json!({"behavior": "deny", "message": "Use git clean instead"}));
        // A message for the running turn is recorded once Claude takes it in.
        p.command(TurnCommand::Steer { text: "also update the docs".into(), attachments: Vec::new(), now: false, key: 1 }, &sink);
        let line: Value = serde_json::from_str(&p.take_output()[0]).unwrap();
        let uuid = line["uuid"].as_str().unwrap().to_string();
        assert!(line.get("priority").is_none());
        // Its turn isn't over while the message waits.
        p.handle(r#"{"type":"result","subtype":"success","is_error":false,"result":"done","session_id":"s1","total_cost_usd":0.5,"usage":{"input_tokens":1,"output_tokens":1}}"#, &sink);
        assert!(!p.finished());
        p.handle(&json!({"type": "user", "isReplay": true, "uuid": uuid, "message": {"role": "user", "content": "also update the docs"}}).to_string(), &sink);
        p.handle(r#"{"type":"result","subtype":"success","is_error":false,"result":"done","session_id":"s1","total_cost_usd":0.8,"usage":{"input_tokens":1,"output_tokens":1}}"#, &sink);
        assert!(p.finished());
        let ev: Vec<AgentEvent> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Steered { text, .. } if text == "also update the docs")));
        let costs: Vec<f64> = ev.iter().filter_map(|e| match e { AgentEvent::Usage { usage } => usage.cost_usd, _ => None }).collect();
        assert_eq!(costs.len(), 2);
        assert!((costs[1] - 0.3).abs() < 1e-9, "each turn's own cost: {costs:?}");
        assert_eq!(ev.iter().filter(|e| matches!(e, AgentEvent::Done { .. })).count(), 1);

        // Stop: an interrupt, and the error result that follows is no failure.
        let mut p = ClaudeLive::default();
        p.handle(r#"{"type":"control_request","request_id":"r2","request":{"subtype":"can_use_tool","tool_name":"Write","input":{"file_path":"a.txt","content":"x"},"tool_use_id":"t2"}}"#, &sink);
        p.command(TurnCommand::Cancel, &sink);
        let i: Value = serde_json::from_str(&p.take_output()[0]).unwrap();
        assert_eq!(i["request"]["subtype"], "interrupt");
        assert!(p.stop_deadline().is_some());
        p.handle(r#"{"type":"result","subtype":"error_during_execution","is_error":true,"session_id":"s1","permission_denials":[{"tool_name":"Write","tool_use_id":"t2"}]}"#, &sink);
        let ev: Vec<AgentEvent> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::PermissionClosed { id } if id == "r2")));
        assert!(!ev.iter().any(|e| matches!(e, AgentEvent::Error { .. } | AgentEvent::Notice { .. })), "{ev:?}");
        assert!(matches!(ev.last(), Some(AgentEvent::Done { ok: false, stop_reason: Some(r) }) if r == "cancelled"));
    }

    #[test]
    fn images_go_inline() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("a.png");
        std::fs::write(&png, [0x89, b'P', b'N', b'G']).unwrap();
        let mut p = ClaudeLive::default();
        let atts = vec![
            Part::Image { mime: None, source: MediaSource::Path { path: png.to_string_lossy().to_string() } },
            Part::File { path: "notes.md".into(), mime: None, text: None },
        ];
        let line: Value = serde_json::from_str(&p.user_line("look", &atts, false, None)).unwrap();
        let blocks = line["message"]["content"].as_array().unwrap();
        assert_eq!(blocks[0]["text"], "look\n\nAttached files:\n- @notes.md\n");
        assert_eq!((blocks[1]["type"].as_str(), blocks[1]["source"]["media_type"].as_str()), (Some("image"), Some("image/png")));
    }

    #[test]
    fn closes_partial_json() {
        assert_eq!(complete_json(r#"{"file_path":"a.rs","content":"one\ntwo"#), Some(json!({"file_path": "a.rs", "content": "one\ntwo"})));
        assert_eq!(complete_json(r#"{"file_path":"a.rs","cont"#), Some(json!({"file_path": "a.rs"})));
        assert_eq!(complete_json(r#"{"todos":[{"content":"x","st"#), Some(json!({"todos": [{"content": "x"}]})));
        assert_eq!(complete_json(r#"{"command":"echo \"#), Some(json!({"command": "echo "})));
        assert_eq!(complete_json(""), None);
    }

    #[test]
    fn shows_a_tool_while_it_is_written() {
        let lines = [
            r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"m1"}},"parent_tool_use_id":null}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"w1","name":"Write","input":{}}},"parent_tool_use_id":null}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"file_path\":\"/r/a.rs\",\"content\":\"fn a() {}\\nfn b"}},"parent_tool_use_id":null}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_stop","index":0},"parent_tool_use_id":null}"#,
            r#"{"type":"assistant","message":{"id":"m1","content":[{"type":"tool_use","id":"w1","name":"Write","input":{"file_path":"/r/a.rs","content":"fn a() {}\nfn b() {}\nfn c() {}\n"}}]},"parent_tool_use_id":null}"#,
        ];
        let mut p = ClaudeStream::default();
        let ev = collect(&mut p, &lines);
        let starts: Vec<&ToolCall> = ev.iter().filter_map(|e| match e { AgentEvent::ToolStart { tool } if tool.id == "w1" => Some(tool), _ => None }).collect();
        assert_eq!(starts.len(), 3, "placeholder, partial content, full input");
        assert!(starts[0].diffs.is_empty());
        assert_eq!(starts[1].diffs.first().map(|d| d.added), Some(2), "two lines written so far");
        assert_eq!(starts[2].diffs.first().map(|d| d.added), Some(3));
    }
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
        // The subagent line is wrapped, not mixed into the main transcript.
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Subagent { parent_tool_id, inner } if parent_tool_id == "toolu_9" && matches!(&**inner, AgentEvent::Text { text } if text == "side"))));
        let ev: Vec<AgentEvent> = ev.into_iter().filter(|e| !matches!(e, AgentEvent::Subagent { .. })).collect();
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
    fn subagent_traffic_and_agent_result_meta() {
        let lines = [
            r#"{"type":"assistant","parent_tool_use_id":null,"message":{"id":"m1","content":[{"type":"tool_use","id":"agent_1","name":"Agent","input":{"description":"Audit","prompt":"look","subagent_type":"Explore"}}]}}"#,
            r#"{"type":"user","parent_tool_use_id":"agent_1","message":{"role":"user","content":"look"}}"#,
            r#"{"type":"assistant","parent_tool_use_id":"agent_1","message":{"id":"s1","model":"claude-haiku-4-5","usage":{"input_tokens":200,"cache_read_input_tokens":1000},"content":[{"type":"thinking","thinking":"plan"},{"type":"tool_use","id":"t_read","name":"Read","input":{"file_path":"src/a.rs"}}]}}"#,
            r#"{"type":"assistant","parent_tool_use_id":"agent_1","message":{"id":"s1","model":"claude-haiku-4-5","usage":{"input_tokens":200,"cache_read_input_tokens":1000},"content":[{"type":"text","text":"Reading"}]}}"#,
            r#"{"type":"user","parent_tool_use_id":"agent_1","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t_read","content":"fn a()"}]}}"#,
            r#"{"type":"assistant","parent_tool_use_id":"agent_1","message":{"id":"s2","content":[{"type":"text","text":"Found it"}]}}"#,
            r#"{"type":"user","parent_tool_use_id":null,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"agent_1","content":[{"type":"text","text":"Found it"}]}]},"tool_use_result":{"agentId":"a1","agentType":"Explore","status":"completed","totalDurationMs":5300,"totalTokens":900}}"#,
        ];
        let mut p = ClaudeStream::default();
        let ev = collect(&mut p, &lines);
        let side: Vec<&AgentEvent> = ev
            .iter()
            .filter_map(|e| match e {
                AgentEvent::Subagent { parent_tool_id, inner } if parent_tool_id == "agent_1" && !matches!(**inner, AgentEvent::Model { .. } | AgentEvent::Usage { .. }) => Some(&**inner),
                _ => None,
            })
            .collect();
        assert!(matches!(side[0], AgentEvent::Reasoning { text, .. } if text == "plan"));
        assert!(matches!(side[1], AgentEvent::ToolStart { tool } if tool.id == "t_read"));
        assert!(matches!(side[2], AgentEvent::Text { text } if text == "Reading"));
        assert!(matches!(side[3], AgentEvent::ToolUpdate { id, output: Some(o), .. } if id == "t_read" && o == "fn a()"));
        assert!(matches!(side[4], AgentEvent::Text { text } if text == "Found it"));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolMeta { id, duration_ms: Some(5300), agent_name: Some(n), .. } if id == "agent_1" && n == "Explore")));
        // The subagent's model and context, once each.
        let count = |want: fn(&AgentEvent) -> bool| ev.iter().filter(|e| matches!(e, AgentEvent::Subagent { inner, .. } if want(inner))).count();
        assert_eq!(count(|e| matches!(e, AgentEvent::Model { model } if model == "claude-haiku-4-5")), 1);
        assert_eq!(count(|e| matches!(e, AgentEvent::Usage { usage } if usage.context_tokens == Some(1200))), 1);
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolUpdate { id, status: Some(ToolStatus::Completed), .. } if id == "agent_1")));
    }

    /// The reply ends while a background subagent still works: the turn
    /// stays open, but the agent is no longer writing.
    #[test]
    fn end_of_reply_is_idle_not_done() {
        let lines = [
            r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"m9"}},"parent_tool_use_id":null}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Waiting on the audit."}},"parent_tool_use_id":null}"#,
            r#"{"type":"stream_event","event":{"type":"message_delta","delta":{"stop_reason":"end_turn"}},"parent_tool_use_id":null}"#,
            r#"{"type":"stream_event","event":{"type":"message_delta","delta":{"stop_reason":"end_turn"}},"parent_tool_use_id":"agent_1"}"#,
        ];
        let mut p = ClaudeStream::default();
        let ev = collect(&mut p, &lines);
        assert_eq!(ev.iter().filter(|e| matches!(e, AgentEvent::Idle)).count(), 1, "a subagent's end is not the agent's");
        assert!(!ev.iter().any(|e| matches!(e, AgentEvent::Done { .. })));
        assert!(!p.done);
    }

    #[test]
    fn background_shell_lifecycle() {
        let lines = [
            r#"{"type":"assistant","parent_tool_use_id":null,"message":{"id":"m1","content":[{"type":"tool_use","id":"bg1","name":"Bash","input":{"command":"npm run dev","run_in_background":true}}]}}"#,
            r#"{"type":"user","parent_tool_use_id":null,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"bg1","content":"Command running in background with ID: b8b97ntfm. Output is being written to: /tmp/x.output"}]},"tool_use_result":{"stdout":"","stderr":"","backgroundTaskId":"b8b97ntfm","interrupted":false}}"#,
            r#"{"type":"assistant","parent_tool_use_id":null,"message":{"id":"m2","content":[{"type":"tool_use","id":"bg2","name":"Bash","input":{"command":"cargo build","run_in_background":true}}]}}"#,
            r#"{"type":"user","parent_tool_use_id":null,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"bg2","content":"Command running in background with ID: c2"}]}}"#,
            r#"{"type":"user","parent_tool_use_id":null,"message":{"role":"user","content":"<task-notification>\n<task-id>c2</task-id>\n<status>completed</status>\n<summary>Build finished</summary>\n</task-notification>"}}"#,
            r#"{"type":"assistant","parent_tool_use_id":null,"message":{"id":"m3","content":[{"type":"tool_use","id":"stop1","name":"TaskStop","input":{"task_id":"b8b97ntfm"}}]}}"#,
            r#"{"type":"user","parent_tool_use_id":null,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"stop1","content":"stopped"}]}}"#,
        ];
        let mut p = ClaudeStream::default();
        let ev = collect(&mut p, &lines);
        let ups: Vec<(&str, ToolStatus)> = ev
            .iter()
            .filter_map(|e| match e { AgentEvent::ToolUpdate { id, status: Some(st), .. } => Some((id.as_str(), *st)), _ => None })
            .collect();
        assert_eq!(ups[0], ("bg1", ToolStatus::Running), "background launch stays running");
        assert_eq!(ups[1], ("bg2", ToolStatus::Running));
        assert!(ups.contains(&("bg2", ToolStatus::Completed)), "notification completes bg2");
        assert!(ups.contains(&("bg1", ToolStatus::Cancelled)), "TaskStop cancels bg1");
        assert!(matches!(ev.iter().find(|e| matches!(e, AgentEvent::ToolStart { tool } if tool.id == "bg1")), Some(AgentEvent::ToolStart { tool }) if tool.background));
    }

    #[test]
    fn notification_and_output_parsing() {
        let n = parse_task_notifications("x <task-notification><task-id>a</task-id><tool-use-id>toolu_1</tool-use-id><status>Failed</status><summary>boom</summary></task-notification> y");
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].tool_use_id.as_deref(), Some("toolu_1"));
        assert_eq!(n[0].status.as_deref(), Some("failed"));
        assert_eq!(background_task_id("Command running in background with ID: b8b97ntfm. Output"), Some("b8b97ntfm".into()));
        assert_eq!(output_status("<status>completed</status>\n<stdout>ok</stdout>"), Some(ToolStatus::Completed));
        assert_eq!(output_status("Status: running"), Some(ToolStatus::Running));
    }

    fn fixture(text: &str) -> Vec<AgentEvent> {
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        collect(&mut ClaudeStream::default(), &lines)
    }

    fn notices(ev: &[AgentEvent]) -> Vec<&str> {
        ev.iter().filter_map(|e| match e { AgentEvent::Notice { text } => Some(text.as_str()), _ => None }).collect()
    }

    fn final_status(ev: &[AgentEvent], id: &str) -> Option<ToolStatus> {
        ev.iter().rev().find_map(|e| match e { AgentEvent::ToolUpdate { id: i, status, .. } if i == id => *status, _ => None })
    }

    /// Captured from Claude Code 2.1.310, `--permission-mode auto`, with
    /// `--include-partial-messages`: a Bash call and a file write run
    /// without prompting.
    #[test]
    fn fixture_auto_mode_runs_tools_headless() {
        let ev = fixture(include_str!("../../tests/fixtures/claude_auto_tools.jsonl"));
        // Each call starts as soon as the model begins it, then again with
        // its full input: one tool per id.
        let mut tools: Vec<&ToolCall> = ev.iter().filter_map(|e| match e { AgentEvent::ToolStart { tool } => Some(tool), _ => None }).collect();
        tools.dedup_by(|a, b| a.id == b.id);
        assert_eq!(tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Bash", "Write"]);
        for t in &tools {
            assert_eq!(final_status(&ev, &t.id), Some(ToolStatus::Completed), "{} completes", t.name);
        }
        let write_diffs = ev.iter().find_map(|e| match e { AgentEvent::ToolUpdate { id, diffs, .. } if *id == tools[1].id && !diffs.is_empty() => Some(diffs), _ => None }).expect("write diff");
        assert_eq!(write_diffs[0].path, "/tmp/w/created.txt");
        let streamed: String = ev.iter().filter_map(|e| match e { AgentEvent::TextDelta { text } => Some(text.as_str()), _ => None }).collect();
        assert_eq!(streamed, "OK", "text streams token by token");
        assert!(!ev.iter().any(|e| matches!(e, AgentEvent::Text { .. } | AgentEvent::Error { .. })), "no duplicate full text, no errors");
        assert!(notices(&ev).is_empty(), "a rate-limit warning is not a notice: {:?}", notices(&ev));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::NativeSession { id, .. } if id == "sess-fixture")));
        assert!(matches!(ev.last(), Some(AgentEvent::Done { ok: true, .. })));
    }

    /// Captured in the default (Manual) mode: the write is denied headlessly.
    #[test]
    fn fixture_permission_denial() {
        let ev = fixture(include_str!("../../tests/fixtures/claude_permission_denied.jsonl"));
        let write = ev.iter().find_map(|e| match e { AgentEvent::ToolStart { tool } => Some(tool), _ => None }).unwrap();
        assert_eq!(final_status(&ev, &write.id), Some(ToolStatus::Failed));
        assert_eq!(notices(&ev), ["Claude Code denied Write without asking: this run's permission mode doesn't allow it"]);
        assert!(matches!(ev.last(), Some(AgentEvent::Done { ok: true, .. })), "the turn itself still succeeds");
    }

    /// Captured with an empty config dir and with an unknown `--model`:
    /// `is_error` results with subtype `success`, exit code 1.
    #[test]
    fn fixture_auth_and_model_errors() {
        for (text, want) in [
            (include_str!("../../tests/fixtures/claude_not_logged_in.jsonl"), "Not logged in"),
            (include_str!("../../tests/fixtures/claude_bad_model.jsonl"), "issue with the selected model"),
        ] {
            let mut p = ClaudeStream::default();
            let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
            let ev = collect(&mut p, &lines);
            let errors: Vec<&str> = ev.iter().filter_map(|e| match e { AgentEvent::Error { message } => Some(message.as_str()), _ => None }).collect();
            assert!(!errors.is_empty() && errors.iter().all(|m| m.contains(want)), "{errors:?}");
            assert!(p.failed && p.done);
            assert!(matches!(ev.last(), Some(AgentEvent::Done { ok: false, .. })));
        }
    }

    /// Event shapes from the stream-json / Agent SDK reference that a short
    /// local run doesn't produce.
    #[test]
    fn documented_events() {
        let png = "iVBORw0KGgo=";
        let shot = format!(r#"{{"type":"user","parent_tool_use_id":null,"message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"t_shot","content":[{{"type":"text","text":"captured"}},{{"type":"image","source":{{"type":"base64","media_type":"image/png","data":"{png}"}}}}]}}]}}}}"#);
        let lines = [
            r#"{"type":"system","subtype":"api_retry","attempt":1,"max_retries":10,"retry_delay_ms":500,"error_status":529,"error":"overloaded"}"#,
            r#"{"type":"system","subtype":"api_retry","attempt":2,"max_retries":10,"retry_delay_ms":900,"error_status":529,"error":"overloaded"}"#,
            r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"m1"}},"parent_tool_use_id":null}"#,
            r#"{"type":"system","subtype":"model_refusal_fallback","original_model":"claude-fable-5","fallback_model":"claude-opus-5"}"#,
            r#"{"type":"assistant","parent_tool_use_id":null,"message":{"id":"m1","content":[{"type":"fallback","from":{"model":"claude-fable-5"},"to":{"model":"claude-opus-5"}},{"type":"tool_use","id":"t_task","name":"TaskCreate","input":{"subject":"Write tests"}},{"type":"tool_use","id":"t_find","name":"ToolSearch","input":{"query":"select:Monitor"}},{"type":"tool_use","id":"t_shot","name":"mcp__browser__screenshot","input":{}},{"type":"tool_use","id":"t_bg","name":"Bash","input":{"command":"npm test","run_in_background":true}},{"type":"tool_use","id":"t_agent","name":"Agent","input":{"description":"scan","prompt":"scan","run_in_background":true}}]}}"#,
            r#"{"type":"user","parent_tool_use_id":null,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t_find","content":[{"type":"tool_reference","tool_name":"Monitor"}]}]}}"#,
            &shot,
            r#"{"type":"user","parent_tool_use_id":null,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t_bg","content":"Command running in background with ID: bx1"}]},"tool_use_result":{"backgroundTaskId":"bx1"}}"#,
            r#"{"type":"user","parent_tool_use_id":null,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t_agent","content":"Async agent launched"}]},"tool_use_result":{"status":"async_launched","agentId":"ag1","isAsync":true}}"#,
            r#"{"type":"system","subtype":"task_notification","task_id":"bx1","status":"failed","output_file":"/tmp/bx1","summary":"exit 1"}"#,
            r#"{"type":"system","subtype":"task_updated","task_id":"ag1","patch":{"status":"killed"}}"#,
            r#"{"type":"system","subtype":"hook_response","hook_id":"h","hook_name":"PostToolUse:Bash","hook_event":"PostToolUse","output":"","stdout":"","stderr":"lint failed\nmore","exit_code":2,"outcome":"error"}"#,
            r#"{"type":"system","subtype":"informational","content":"Backgrounding after the current tool finishes…","level":"warning"}"#,
            r#"{"type":"system","subtype":"thinking_tokens","estimated_tokens":10,"estimated_tokens_delta":10}"#,
            r#"{"type":"tool_progress","tool_use_id":"t_bg","tool_name":"Bash","parent_tool_use_id":null,"elapsed_time_seconds":30,"heartbeat":true}"#,
            r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"m2"}},"parent_tool_use_id":null}"#,
            r#"{"type":"assistant","parent_tool_use_id":null,"message":{"id":"m2","content":[{"type":"fallback","from":{"model":"claude-opus-5"},"to":{"model":"claude-sonnet-5"}},{"type":"text","text":"partial answer"}]}}"#,
            r#"{"type":"stream_event","event":{"type":"message_delta","delta":{"stop_reason":"max_tokens"}},"parent_tool_use_id":null}"#,
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed_warning","utilization":0.82,"resetsAt":1793900000,"rateLimitType":"seven_day"}}"#,
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":1793278800,"rateLimitType":"five_hour"}}"#,
            r#"{"type":"some_future_event","x":1}"#,
            r#"{"type":"result","subtype":"error_max_turns","is_error":true,"stop_reason":"max_tokens","errors":[],"usage":{"input_tokens":5,"output_tokens":40,"output_tokens_details":{"thinking_tokens":12}},"permission_denials":[]}"#,
        ];
        let mut p = ClaudeStream::default();
        let ev = collect(&mut p, &lines);
        let n = notices(&ev);
        assert_eq!(n[0], "Claude API is overloaded (529), retrying…", "one notice per retry streak");
        assert_eq!(n[1], "Safeguards flagged this message; retrying on claude-opus-5");
        assert_eq!(n[2], "PostToolUse hook failed: lint failed");
        assert_eq!(n[3], "Backgrounding after the current tool finishes…");
        assert_eq!(n[4], "Switched from claude-opus-5 to claude-sonnet-5", "an unannounced switch is noted");
        assert_eq!(n[5], "Response hit the output token limit");
        assert!(n[6].starts_with("Usage limit reached (5-hour limit) · resets "), "{}", n[6]);
        assert_eq!(n.len(), 7, "refusal switch and cut-off are not repeated: {n:?}");
        let start = |id: &str| ev.iter().find_map(|e| match e { AgentEvent::ToolStart { tool } if tool.id == id => Some(tool), _ => None }).unwrap();
        assert_eq!(start("t_task").kind, ToolKind::Other, "TaskCreate is not a file write");
        assert_eq!(start("t_find").kind, ToolKind::Other);
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolUpdate { id, output: Some(o), .. } if id == "t_find" && o == "Loaded Monitor")));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolImages { id, images } if id == "t_shot" && images.len() == 1)));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolUpdate { id, output: Some(o), .. } if id == "t_shot" && o == "captured")), "no [image] placeholder");
        assert_eq!(final_status(&ev, "t_bg"), Some(ToolStatus::Failed), "system task_notification");
        assert_eq!(final_status(&ev, "t_agent"), Some(ToolStatus::Cancelled), "task_updated patch");
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Usage { usage } if usage.reasoning_tokens == 12)));
        // Window usage for the plan-limit chip: a warning reports its share,
        // a rejection means the window is used up.
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::RateLimit { window, used_pct, .. } if window == "Weekly" && (*used_pct - 82.0).abs() < 0.01)));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::RateLimit { window, used_pct, resets_at: Some(1793278800) } if window == "5-hour" && *used_pct == 100.0)));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Error { message } if message == "Stopped at the turn limit (--max-turns)")));
        assert!(matches!(ev.last(), Some(AgentEvent::Done { ok: false, stop_reason: Some(r) }) if r == "error_max_turns"));
    }
}
