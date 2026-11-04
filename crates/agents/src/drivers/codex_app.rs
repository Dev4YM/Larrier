//! OpenAI Codex over `codex app-server` (JSON-RPC on stdio): the same
//! threads as `codex exec`, plus what exec can't do. Approvals, questions
//! and plans are answered live, messages are steered into the running
//! turn, and a stop interrupts the turn cleanly.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use larrier_core::lsf::{MediaSource, Part};
use larrier_core::{ToolStatus, Usage};
use larrier_formats::codex::{error_message, unwrap_shell};
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedReceiver;

use super::codex::CodexStream;
use super::process::{run_interactive, Interactive};
use super::{plan_entries, s, tool_of_kind, u, usage_from};
use crate::events::{AgentEvent, AskOption, AskQuestion, EventSink, PermissionOption, PermissionRequest, Reply, RequestKind, TurnCommand};
use crate::request::TurnRequest;

pub async fn run(req: TurnRequest, sink: EventSink, mut control: UnboundedReceiver<TurnCommand>) {
    let mut cmd = req.launch.command();
    cmd.current_dir(&req.cwd);
    let mut live = CodexLive::new(&req);
    // Config overrides (sandbox, effort, providers) apply to the server;
    // the user's other extra arguments are `codex exec`'s own. Codex's own
    // plan mode changes nothing by itself, so the sandbox comes from the
    // permissions picked (exec's Plan forces read-only), ready for when
    // the plan is built.
    let modes: Vec<String> = if live.plan_mode {
        let spec = req.agent.spec();
        req.modes.ids.iter().enumerate().skip(1).filter_map(|(i, id)| spec.mode_groups.get(i)?.find(id)).flat_map(|o| o.args.iter().map(|a| a.to_string())).collect()
    } else {
        req.modes.args.clone()
    };
    cmd.args(config_args(&modes));
    cmd.args(config_args(&req.launch.extra_args));
    if let Some(b) = &req.browser {
        cmd.args(b.codex_args());
    }
    cmd.arg("app-server");
    let outcome = run_interactive(cmd, Vec::new(), &sink, &mut control, &mut live).await;
    if !live.initialized && !outcome.cancelled {
        // A Codex without the app server: run the turn the old way.
        sink.log(format!("codex app-server unavailable ({}); using codex exec", outcome.error_message("Codex")));
        return super::codex::run(req, sink, control).await;
    }
    if !live.done {
        live.close_open(&sink, ToolStatus::Cancelled);
        if outcome.cancelled || live.stop_at.is_some() {
            sink.done(false, Some("cancelled".into()));
        } else {
            sink.error(outcome.error_message("Codex"));
            sink.done(false, Some("error".into()));
        }
    }
}

/// The `codex` options that apply to the app server too (config overrides,
/// features, profile).
fn config_args(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-c" | "--config" | "--enable" | "--disable" | "-p" | "--profile" => {
                if let Some(v) = it.next() {
                    out.push(a.clone());
                    out.push(v.clone());
                }
            }
            _ if ["--config=", "--enable=", "--disable=", "--profile="].iter().any(|p| a.starts_with(p)) => out.push(a.clone()),
            _ => {}
        }
    }
    out
}

/// A request of ours the server hasn't answered yet.
enum Pending {
    Init,
    Thread,
    Turn,
    Steer(String, Vec<Part>, u64),
    Other,
}

/// What a request from Codex is waiting for.
enum AskKind {
    Command,
    FileChange,
    Permissions,
    Questions,
    /// An MCP server's form (`fields` are its property names and schemas)
    /// or link.
    Form,
    Link,
    Plan,
}

struct Ask {
    rpc: Value,
    kind: AskKind,
    item: Option<String>,
    /// Question ids, form fields, the rule to remember, or the access asked for.
    extra: Value,
}

pub struct CodexLive {
    inner: CodexStream,
    out: Vec<String>,
    next_id: u64,
    pending: HashMap<u64, Pending>,
    cwd: String,
    model: Option<String>,
    resume: Option<String>,
    /// Codex's own plan mode (questions, then a plan to approve).
    pub plan_mode: bool,
    /// A collaboration mode was set on the thread: every turn names one.
    collab: bool,
    first: Vec<Value>,
    thread: Option<String>,
    turn: Option<String>,
    thread_model: Option<String>,
    thread_effort: Option<String>,
    asks: HashMap<String, Ask>,
    /// Tool items started and not yet completed.
    open: HashSet<String>,
    file_items: HashMap<String, Value>,
    texts: HashMap<String, String>,
    summaries: HashMap<String, Vec<String>>,
    outputs: HashMap<String, (String, Option<Instant>)>,
    /// Thread totals before this process's turns (usage is cumulative).
    base: Option<Usage>,
    total: Option<Usage>,
    /// Messages sent during a turn that Codex hasn't taken in.
    waiting: Vec<(String, Vec<Part>, u64)>,
    /// Messages to send once the interrupt they asked for lands.
    restart: Vec<(String, Vec<Part>, u64)>,
    /// The plan this turn proposed (plan mode).
    plan: Option<String>,
    interrupting: bool,
    cancel_wanted: bool,
    pub stop_at: Option<Instant>,
    finished: bool,
    pub done: bool,
    pub initialized: bool,
    last_error: Option<String>,
    failed_servers: HashSet<String>,
}

/// How often streaming command output is redrawn.
const OUTPUT_EVERY: Duration = Duration::from_millis(150);

impl CodexLive {
    pub fn new(req: &TurnRequest) -> Self {
        let plan_mode = req.modes.ids.first().map(String::as_str) == Some("plan");
        // Codex plans natively; the read-only instruction is for exec.
        let mut prompt = if plan_mode { req.prompt.clone() } else { req.full_prompt() };
        if plan_mode {
            if let Some(h) = req.handoff.as_ref().filter(|h| !h.trim().is_empty()) {
                prompt = format!("<conversation_history>\n{h}\n</conversation_history>\n\n{prompt}");
            }
        }
        let mut live = CodexLive {
            inner: CodexStream::new(req.cwd.clone()),
            out: Vec::new(),
            next_id: 0,
            pending: HashMap::new(),
            cwd: req.cwd.to_string_lossy().to_string(),
            model: req.model.clone(),
            resume: req.resume_id.clone(),
            plan_mode,
            collab: false,
            first: Vec::new(),
            thread: None,
            turn: None,
            thread_model: None,
            thread_effort: None,
            asks: HashMap::new(),
            open: HashSet::new(),
            file_items: HashMap::new(),
            texts: HashMap::new(),
            summaries: HashMap::new(),
            outputs: HashMap::new(),
            base: None,
            total: None,
            waiting: Vec::new(),
            restart: Vec::new(),
            plan: None,
            interrupting: false,
            cancel_wanted: false,
            stop_at: None,
            finished: false,
            done: false,
            initialized: false,
            last_error: None,
            failed_servers: HashSet::new(),
        };
        live.first = input(&prompt, &req.attachments);
        let version = env!("CARGO_PKG_VERSION");
        live.send("initialize", json!({"clientInfo": {"name": "larrier", "title": "Larrier", "version": version}, "capabilities": {"experimentalApi": true}}), Pending::Init);
        live
    }

    fn send(&mut self, method: &str, params: Value, pending: Pending) {
        let id = self.next_id;
        self.next_id += 1;
        self.pending.insert(id, pending);
        self.out.push(json!({"id": id, "method": method, "params": params}).to_string());
    }

    fn respond(&mut self, rpc: Value, result: Value) {
        self.out.push(json!({"id": rpc, "result": result}).to_string());
    }

    fn start_turn(&mut self, input: Vec<Value>) {
        let Some(thread) = self.thread.clone() else { return };
        let mut params = json!({"threadId": thread, "input": input});
        if let Some(m) = &self.model {
            params["model"] = json!(m);
        }
        // Plan mode is a collaboration mode, and it stays on the thread
        // until another is named.
        self.collab |= self.plan_mode;
        if self.collab {
            if let Some(model) = self.model.clone().or_else(|| self.thread_model.clone()) {
                let mode = if self.plan_mode { "plan" } else { "default" };
                params["collaborationMode"] = json!({"mode": mode, "settings": {"model": model, "reasoning_effort": self.thread_effort, "developer_instructions": null}});
            }
        }
        self.plan = None;
        self.send("turn/start", params, Pending::Turn);
    }

    fn interrupt(&mut self) {
        if let (Some(thread), Some(turn)) = (self.thread.clone(), self.turn.clone()) {
            if !self.interrupting {
                self.interrupting = true;
                self.send("turn/interrupt", json!({"threadId": thread, "turnId": turn}), Pending::Other);
            }
        }
    }

    fn steer(&mut self, text: String, parts: Vec<Part>, key: u64) {
        match (self.thread.clone(), self.turn.clone()) {
            (Some(thread), Some(turn)) if !self.interrupting => {
                let input = input(&text, &parts);
                self.send("turn/steer", json!({"threadId": thread, "input": input, "expectedTurnId": turn}), Pending::Steer(text, parts, key));
            }
            _ => self.waiting.push((text, parts, key)),
        }
    }

    /// Mark tools still running as ended (an interrupted turn leaves them open).
    pub fn close_open(&mut self, sink: &EventSink, status: ToolStatus) {
        for id in std::mem::take(&mut self.open) {
            sink.send(AgentEvent::ToolUpdate { id, status: Some(status), output: None, diffs: Vec::new(), is_error: false, title: None });
        }
    }

    fn on_response(&mut self, id: u64, v: &Value, sink: &EventSink) {
        let Some(pending) = self.pending.remove(&id) else { return };
        let err = v.get("error").filter(|e| !e.is_null()).map(|e| error_message(s(e, "message").unwrap_or("Codex refused the request")));
        let r = &v["result"];
        match pending {
            Pending::Init => {
                if let Some(e) = err {
                    return self.fail(e, sink);
                }
                self.initialized = true;
                self.out.push(json!({"method": "initialized"}).to_string());
                let mut params = json!({"cwd": self.cwd});
                if let Some(m) = &self.model {
                    params["model"] = json!(m);
                }
                match self.resume.clone() {
                    Some(t) => {
                        params["threadId"] = json!(t);
                        self.send("thread/resume", params, Pending::Thread);
                    }
                    None => self.send("thread/start", params, Pending::Thread),
                }
            }
            Pending::Thread => {
                if let Some(e) = err {
                    return self.fail(e, sink);
                }
                let th = &r["thread"];
                self.thread = s(th, "id").map(String::from);
                self.thread_model = s(r, "model").map(String::from);
                self.thread_effort = s(r, "reasoningEffort").map(String::from);
                if let Some(id) = &self.thread {
                    let path = s(th, "path").map(|p| p.trim_start_matches(r"\\?\").to_string());
                    sink.send(AgentEvent::NativeSession { id: id.clone(), path });
                }
                let first = std::mem::take(&mut self.first);
                self.start_turn(first);
            }
            Pending::Turn => {
                if let Some(e) = err {
                    return self.fail(e, sink);
                }
                self.turn = s(&r["turn"], "id").map(String::from);
                self.interrupting = false;
                if self.cancel_wanted {
                    self.interrupt();
                }
                // Messages sent before the turn began join it now.
                for (text, parts, key) in std::mem::take(&mut self.waiting) {
                    self.steer(text, parts, key);
                }
            }
            Pending::Steer(text, parts, key) => match err {
                None => sink.send(AgentEvent::Steered { text, attachments: parts }),
                // The turn had ended: it starts the next one, or goes back
                // to the queue.
                Some(_) => self.waiting.push((text, parts, key)),
            },
            Pending::Other => {}
        }
    }

    fn fail(&mut self, msg: String, sink: &EventSink) {
        if self.last_error.as_deref() != Some(msg.as_str()) {
            sink.error(msg);
        }
        self.done = true;
        self.finished = true;
        sink.done(false, Some("error".into()));
    }

    // ------------------------------------------------------------ requests

    fn on_request(&mut self, rpc: Value, method: &str, p: &Value, sink: &EventSink) {
        let rid = format!("codex-{}", rpc);
        let item = s(p, "itemId").map(String::from);
        let mut req = PermissionRequest { id: rid.clone(), tool_call_id: item.clone(), ..Default::default() };
        let opt = |id: &str, label: &str, kind: &str| PermissionOption { id: id.into(), label: label.into(), kind: kind.into() };
        let (kind, extra) = match method {
            "item/commandExecution/requestApproval" => {
                let command = s(p, "command").map(unwrap_shell).unwrap_or_default();
                req.title = if command.is_empty() { "Run a command".into() } else { command };
                let net = p["networkApprovalContext"].as_object().map(|n| format!("Network access to {}", n.get("host").and_then(|h| h.as_str()).unwrap_or("a host")));
                let cwd = s(p, "cwd").filter(|c| !same_path(c, &self.cwd)).map(|c| format!("In {c}"));
                req.detail = [s(p, "reason").map(String::from), net, cwd].into_iter().flatten().reduce(|a, b| format!("{a}\n{b}"));
                req.options.push(opt("accept", "Allow", "allow_once"));
                let rule = p["proposedExecpolicyAmendment"].clone();
                if let Some(prefix) = rule.as_array().filter(|a| !a.is_empty()) {
                    // The rule's command as the model wrote it, not its shell.
                    let words = larrier_formats::codex::unwrap_argv(prefix);
                    let short: String = words.chars().take(48).collect();
                    let more = if short.len() < words.len() { "…" } else { "" };
                    req.options.push(opt("rule", &format!("Always allow `{short}{more}`"), "allow_always"));
                }
                req.options.push(opt("session", "Allow for this session", "allow_always"));
                req.options.push(opt("decline", "Deny", "reject_once"));
                (AskKind::Command, rule)
            }
            "item/fileChange/requestApproval" => {
                let changes = item.as_ref().and_then(|i| self.file_items.get(i)).map(|it| it["changes"].clone()).unwrap_or(Value::Null);
                let list: Vec<(String, String)> = changes
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|c| (s(c, "path").unwrap_or("").to_string(), s(&c["kind"], "type").or_else(|| s(c, "kind")).unwrap_or("update").to_string()))
                    .collect();
                req.title = match list.as_slice() {
                    [(path, k)] => format!("{} {}", match k.as_str() { "add" => "Write", "delete" => "Delete", _ => "Edit" }, short_path(path, &self.cwd)),
                    [] => "Change files".into(),
                    l => format!("Edit {} files", l.len()),
                };
                let root = s(p, "grantRoot").map(|r| format!("And allow writing in {r} from now on"));
                req.detail = [s(p, "reason").map(String::from), root].into_iter().flatten().reduce(|a, b| format!("{a}\n{b}"));
                req.options = vec![opt("accept", "Allow", "allow_once"), opt("session", "Allow edits for this session", "allow_always"), opt("decline", "Deny", "reject_once")];
                (AskKind::FileChange, Value::Null)
            }
            "item/permissions/requestApproval" => {
                let perms = p["permissions"].clone();
                let mut lines: Vec<String> = Vec::new();
                if perms["network"]["enabled"].as_bool() == Some(true) {
                    lines.push("Network access".into());
                }
                for (key, verb) in [("write", "Write to"), ("read", "Read")] {
                    for path in perms["fileSystem"][key].as_array().into_iter().flatten().filter_map(|x| x.as_str()) {
                        lines.push(format!("{verb} {path}"));
                    }
                }
                for e in perms["fileSystem"]["entries"].as_array().into_iter().flatten() {
                    let path = s(&e["path"], "path").or_else(|| s(&e["path"], "pattern")).unwrap_or("");
                    lines.push(format!("{} {path}", match s(e, "access") { Some("write") => "Write to", Some("deny") => "No access to", _ => "Read" }));
                }
                req.title = "Codex asks for more access".into();
                req.detail = [s(p, "reason").map(String::from), (!lines.is_empty()).then(|| lines.join("\n"))].into_iter().flatten().reduce(|a, b| format!("{a}\n{b}"));
                req.options = vec![opt("turn", "Allow for this turn", "allow_once"), opt("session", "Allow for this session", "allow_always"), opt("decline", "Deny", "reject_once")];
                (AskKind::Permissions, perms)
            }
            "item/tool/requestUserInput" => {
                req.kind = RequestKind::Question;
                req.title = "Codex has a question".into();
                let mut ids = Vec::new();
                for q in p["questions"].as_array().into_iter().flatten() {
                    ids.push(json!(s(q, "id").unwrap_or("")));
                    req.questions.push(AskQuestion {
                        id: s(q, "id").map(String::from),
                        header: s(q, "header").map(String::from),
                        question: s(q, "question").unwrap_or("").to_string(),
                        multi: false,
                        options: q["options"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|o| Some(AskOption { label: s(o, "label")?.to_string(), description: s(o, "description").map(String::from) }))
                            .collect(),
                    });
                }
                (AskKind::Questions, Value::Array(ids))
            }
            "mcpServer/elicitation/request" => {
                let server = s(p, "serverName").unwrap_or("An MCP server");
                let message = s(p, "message").unwrap_or("").to_string();
                let fields = p["requestedSchema"]["properties"].as_object().map(|o| o.len()).unwrap_or(0);
                if s(p, "mode") != Some("url") && fields == 0 {
                    // Nothing to fill in: a yes or no (Codex asks this way
                    // before an MCP tool runs).
                    req.title = if message.is_empty() { format!("{server} asks to go ahead") } else { message.clone() };
                    req.options = vec![opt("accept", "Allow", "allow_once"), opt("decline", "Deny", "reject_once")];
                    (AskKind::Link, Value::Null)
                } else if s(p, "mode") == Some("url") {
                    req.title = format!("{server} asks you to open a link");
                    req.detail = Some(format!("{message}\n{}", s(p, "url").unwrap_or("")).trim().to_string());
                    req.options = vec![opt("accept", "Done, continue", "allow_once"), opt("decline", "Decline", "reject_once")];
                    (AskKind::Link, Value::Null)
                } else {
                    req.kind = RequestKind::Question;
                    req.title = if message.is_empty() { format!("{server} needs some details") } else { format!("{server}: {message}") };
                    let schema = &p["requestedSchema"];
                    let mut fields = Vec::new();
                    for (key, prop) in schema["properties"].as_object().into_iter().flatten() {
                        let (options, multi) = field_options(prop);
                        req.questions.push(AskQuestion {
                            id: Some(key.clone()),
                            header: s(prop, "title").map(String::from),
                            question: s(prop, "description").or_else(|| s(prop, "title")).unwrap_or(key).to_string(),
                            multi,
                            options: options.iter().map(|(label, _)| AskOption { label: label.clone(), description: None }).collect(),
                        });
                        fields.push(json!({"key": key, "schema": prop}));
                    }
                    (AskKind::Form, Value::Array(fields))
                }
            }
            other => {
                // Nothing Larrier can answer: say so rather than leave Codex waiting.
                self.out.push(json!({"id": rpc, "error": {"code": -32601, "message": format!("Larrier doesn't handle {other}")}}).to_string());
                return;
            }
        };
        if let Some(id) = item.as_ref().filter(|i| self.open.contains(*i)) {
            if matches!(kind, AskKind::Command | AskKind::FileChange) {
                sink.send(AgentEvent::ToolUpdate { id: id.clone(), status: Some(ToolStatus::Approval), output: None, diffs: Vec::new(), is_error: false, title: None });
            }
        }
        self.asks.insert(rid, Ask { rpc, kind, item, extra });
        sink.send(AgentEvent::Permission { request: req });
    }

    fn answer(&mut self, request_id: &str, reply: Reply, sink: &EventSink) {
        let Some(ask) = self.asks.remove(request_id) else { return };
        let opt = reply.option_id.clone();
        let opt = opt.as_deref();
        let mut allowed = !matches!(opt, None | Some("decline"));
        let result = match ask.kind {
            AskKind::Command => {
                let d = match opt {
                    Some("accept") => json!("accept"),
                    Some("session") => json!("acceptForSession"),
                    Some("rule") => json!({"acceptWithExecpolicyAmendment": {"execpolicy_amendment": ask.extra}}),
                    _ => json!("decline"),
                };
                json!({"decision": d})
            }
            AskKind::FileChange => json!({"decision": match opt { Some("accept") => "accept", Some("session") => "acceptForSession", _ => "decline" }}),
            AskKind::Permissions => match opt {
                Some(scope @ ("turn" | "session")) => json!({"permissions": ask.extra, "scope": scope}),
                _ => json!({"permissions": {}, "scope": "turn"}),
            },
            AskKind::Questions => {
                let ids: Vec<String> = ask.extra.as_array().into_iter().flatten().filter_map(|i| i.as_str().map(String::from)).collect();
                let answers: serde_json::Map<String, Value> = ids.iter().zip(&reply.answers).filter(|(_, a)| !a.is_empty()).map(|(id, a)| (id.clone(), json!({"answers": a}))).collect();
                allowed = !answers.is_empty();
                json!({"answers": answers})
            }
            AskKind::Form => {
                let mut content = serde_json::Map::new();
                for (f, a) in ask.extra.as_array().into_iter().flatten().zip(&reply.answers) {
                    if let (Some(key), Some(v)) = (s(f, "key"), field_value(&f["schema"], a)) {
                        content.insert(key.to_string(), v);
                    }
                }
                allowed = !content.is_empty();
                if allowed {
                    json!({"action": "accept", "content": content, "_meta": null})
                } else {
                    json!({"action": "decline", "content": null, "_meta": null})
                }
            }
            AskKind::Link => json!({"action": if allowed { "accept" } else { "decline" }, "content": if allowed { json!({}) } else { Value::Null }, "_meta": null}),
            AskKind::Plan => return self.answer_plan(opt, reply.message, sink),
        };
        if let Some(id) = ask.item.filter(|i| self.open.contains(i)) {
            if allowed && matches!(ask.kind, AskKind::Command | AskKind::FileChange) {
                sink.send(AgentEvent::ToolUpdate { id, status: Some(ToolStatus::Running), output: None, diffs: Vec::new(), is_error: false, title: None });
            }
        }
        self.respond(ask.rpc, result);
        // Words that came with a refusal reach Codex as a message.
        if !allowed {
            if let Some(m) = reply.message.filter(|m| !m.trim().is_empty()) {
                self.steer(m, Vec::new(), 0);
            }
        }
    }

    /// The plan is approved (Codex builds it, out of plan mode) or sent back
    /// with notes; dismissed, the turn simply ends.
    fn answer_plan(&mut self, opt: Option<&str>, message: Option<String>, sink: &EventSink) {
        let note = message.filter(|m| !m.trim().is_empty());
        let next = match (opt, note) {
            (Some("approve"), _) => {
                self.plan_mode = false;
                sink.send(AgentEvent::Mode { id: "default".into() });
                Some("Implement the plan.".to_string())
            }
            (Some("keep"), Some(n)) => Some(n),
            _ => None,
        };
        match next {
            Some(text) => {
                sink.send(AgentEvent::Steered { text: text.clone(), attachments: Vec::new() });
                self.start_turn(input(&text, &[]));
            }
            None => self.finish("completed", &Value::Null, sink),
        }
    }

    // ------------------------------------------------------- notifications

    fn on_notification(&mut self, method: &str, p: &Value, sink: &EventSink) {
        match method {
            "item/started" | "item/completed" => self.on_item(method == "item/completed", &p["item"], sink),
            "item/agentMessage/delta" => {
                let Some(id) = s(p, "itemId") else { return };
                let acc = self.texts.entry(id.to_string()).or_default();
                acc.push_str(s(p, "delta").unwrap_or(""));
                // A Codex app directive (::git-commit{…}) still being written
                // is held back; finished ones are dropped.
                let cut = acc.rfind('\n').map(|i| i + 1).unwrap_or(0);
                let shown = if acc[cut..].trim_start().starts_with("::") { &acc[..cut] } else { &acc[..] };
                let (text, _) = larrier_formats::codex::app_directives(shown);
                self.inner.stream_text(id, &text, false, sink);
            }
            "item/reasoning/summaryTextDelta" => {
                let Some(id) = s(p, "itemId") else { return };
                let i = u(p, "summaryIndex") as usize;
                let parts = self.summaries.entry(id.to_string()).or_default();
                if parts.len() <= i {
                    parts.resize(i + 1, String::new());
                }
                parts[i].push_str(s(p, "delta").unwrap_or(""));
                let text = parts.iter().filter(|x| !x.is_empty()).cloned().collect::<Vec<_>>().join("\n\n");
                self.inner.stream_text(id, &text, true, sink);
            }
            "item/commandExecution/outputDelta" => {
                let Some(id) = s(p, "itemId") else { return };
                let (acc, at) = self.outputs.entry(id.to_string()).or_default();
                acc.push_str(s(p, "delta").unwrap_or(""));
                if at.map(|t| t.elapsed() >= OUTPUT_EVERY).unwrap_or(true) {
                    *at = Some(Instant::now());
                    let output = Some(acc.clone());
                    sink.send(AgentEvent::ToolUpdate { id: id.to_string(), status: None, output, diffs: Vec::new(), is_error: false, title: None });
                }
            }
            "turn/plan/updated" => {
                let steps: Vec<Value> = p["plan"].as_array().into_iter().flatten().map(|x| json!({"content": s(x, "step").unwrap_or(""), "status": s(x, "status").unwrap_or("pending")})).collect();
                if let Some(entries) = plan_entries(&Value::Array(steps)) {
                    sink.send(AgentEvent::Plan { entries });
                }
            }
            "thread/tokenUsage/updated" => {
                let tu = &p["tokenUsage"];
                let (total, last) = (usage_of(&tu["total"]), usage_of(&tu["last"]));
                if self.base.is_none() {
                    self.base = Some(minus(&total, &last));
                }
                self.total = Some(total);
                let context = u(&tu["last"], "inputTokens");
                sink.send(AgentEvent::Usage { usage: Usage { context_tokens: Some(context), ..Default::default() } });
            }
            "account/rateLimits/updated" => {
                let rl = &p["rateLimits"];
                for w in [&rl["primary"], &rl["secondary"]] {
                    let Some(used) = w["usedPercent"].as_f64() else { continue };
                    let Some(mins) = w["windowDurationMins"].as_u64() else { continue };
                    sink.send(AgentEvent::RateLimit { window: window_name(mins), used_pct: used, resets_at: w["resetsAt"].as_i64() });
                }
            }
            "error" => {
                let msg = error_message(s(&p["error"], "message").unwrap_or("Codex error"));
                if p["willRetry"].as_bool() == Some(true) {
                    sink.log(format!("retrying: {msg}"));
                } else if self.last_error.as_deref() != Some(msg.as_str()) {
                    sink.error(msg.clone());
                    self.last_error = Some(msg);
                }
            }
            "mcpServer/startupStatus/updated" => {
                if s(p, "status") == Some("failed") {
                    let name = s(p, "name").unwrap_or("an MCP server").to_string();
                    if self.failed_servers.insert(name.clone()) {
                        let why = s(p, "error").or_else(|| s(p, "failureReason")).map(|e| format!(": {e}")).unwrap_or_default();
                        sink.notice(format!("Codex couldn't start the MCP server {name}{why}"));
                    }
                }
            }
            // Answered elsewhere (an auto-reviewer, a timeout): the card goes.
            "serverRequest/resolved" => {
                let rid = format!("codex-{}", p["requestId"]);
                if self.asks.remove(&rid).is_some() {
                    sink.send(AgentEvent::PermissionClosed { id: rid });
                }
            }
            "turn/completed" => {
                let turn = &p["turn"];
                if self.turn.is_some() && s(turn, "id") != self.turn.as_deref() {
                    return;
                }
                let status = s(turn, "status").unwrap_or("completed").to_string();
                self.finish(&status, &turn["error"], sink);
            }
            _ => {}
        }
    }

    fn on_item(&mut self, completed: bool, item: &Value, sink: &EventSink) {
        let Some(id) = s(item, "id").map(String::from) else { return };
        match s(item, "type").unwrap_or("") {
            "agentMessage" | "reasoning" if !completed => return,
            "plan" => {
                if completed {
                    self.plan = s(item, "text").map(String::from).filter(|t| !t.trim().is_empty());
                }
                return;
            }
            "fileChange" => {
                self.file_items.insert(id.clone(), item.clone());
            }
            "contextCompaction" => {
                if completed {
                    sink.notice("Codex compacted the conversation to fit its context.".to_string());
                }
                return;
            }
            "imageView" => {
                if !completed {
                    let path = s(item, "path").unwrap_or("").to_string();
                    let mut t = tool_of_kind(id.clone(), "view_image", larrier_core::ToolKind::Read, json!({"file_path": path}));
                    t.status = ToolStatus::Completed;
                    sink.send(AgentEvent::ToolStart { tool: t });
                }
                return;
            }
            _ => {}
        }
        let Some(x) = exec_item(item) else { return };
        let tool = !matches!(s(&x, "type"), Some("agent_message" | "reasoning"));
        if tool {
            if completed {
                self.open.remove(&id);
                self.outputs.remove(&id);
            } else {
                self.open.insert(id.clone());
            }
        }
        self.inner.item(if completed { "item.completed" } else { "item.started" }, &x, sink);
    }

    /// The turn ended: what's still waiting starts the next one, a plan asks
    /// to be approved, or the conversation is over.
    fn finish(&mut self, status: &str, error: &Value, sink: &EventSink) {
        for (id, ask) in std::mem::take(&mut self.asks) {
            if !matches!(ask.kind, AskKind::Plan) {
                sink.send(AgentEvent::PermissionClosed { id });
            }
        }
        if status != "completed" {
            self.close_open(sink, if status == "interrupted" { ToolStatus::Cancelled } else { ToolStatus::Failed });
        }
        if let Some(t) = self.total.clone() {
            let usage = minus(&t, &self.base.clone().unwrap_or_default());
            sink.send(AgentEvent::Usage { usage });
            self.base = Some(t);
        }
        self.turn = None;
        self.interrupting = false;
        let stopped = self.stop_at.is_some();
        let next = if stopped {
            Vec::new()
        } else if !self.restart.is_empty() {
            std::mem::take(&mut self.restart)
        } else if status == "completed" {
            std::mem::take(&mut self.waiting)
        } else {
            Vec::new()
        };
        if !next.is_empty() {
            let mut all = Vec::new();
            for (text, parts, _) in next {
                all.extend(input(&text, &parts));
                sink.send(AgentEvent::Steered { text, attachments: parts });
            }
            return self.start_turn(all);
        }
        if status == "completed" && self.plan_mode && !stopped {
            if let Some(plan) = self.plan.take() {
                let rid = format!("codex-plan-{}", self.next_id);
                self.asks.insert(rid.clone(), Ask { rpc: Value::Null, kind: AskKind::Plan, item: None, extra: Value::Null });
                let opt = |id: &str, label: &str, kind: &str| PermissionOption { id: id.into(), label: label.into(), kind: kind.into() };
                let request = PermissionRequest {
                    id: rid,
                    title: "Codex's plan is ready".into(),
                    kind: RequestKind::Plan,
                    plan: Some(plan),
                    options: vec![opt("approve", "Implement the plan", "allow_once"), opt("keep", "Keep planning", "reject_once")],
                    ..Default::default()
                };
                sink.send(AgentEvent::Permission { request });
                return;
            }
        }
        self.done = true;
        self.finished = true;
        match status {
            "completed" => sink.done(true, Some("end_turn".into())),
            "interrupted" => sink.done(false, Some("cancelled".into())),
            _ => {
                let msg = error_message(s(error, "message").unwrap_or("Codex turn failed"));
                if self.last_error.as_deref() != Some(msg.as_str()) {
                    sink.error(msg);
                }
                sink.done(false, Some("error".into()));
            }
        }
    }
}

impl Interactive for CodexLive {
    fn handle(&mut self, line: &str, sink: &EventSink) {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            sink.log(line.to_string());
            return;
        };
        let method = s(&v, "method").map(String::from);
        match (method, v.get("id").filter(|i| !i.is_null())) {
            (Some(m), Some(id)) => self.on_request(id.clone(), &m, &v["params"], sink),
            (Some(m), None) => self.on_notification(&m, &v["params"], sink),
            (None, Some(id)) => {
                if let Some(id) = id.as_u64() {
                    self.on_response(id, &v, sink);
                }
            }
            (None, None) => {}
        }
    }

    fn command(&mut self, cmd: TurnCommand, sink: &EventSink) {
        match cmd {
            TurnCommand::Cancel | TurnCommand::Interrupt => {
                if self.finished || self.stop_at.is_some() {
                    return;
                }
                self.stop_at = Some(Instant::now() + Duration::from_secs(8));
                // A plan waiting for approval: nothing runs, just end.
                if self.turn.is_none() && self.asks.values().any(|a| matches!(a.kind, AskKind::Plan)) {
                    self.asks.clear();
                    self.done = true;
                    self.finished = true;
                    sink.done(false, Some("cancelled".into()));
                    return;
                }
                if self.turn.is_some() {
                    self.interrupt();
                } else {
                    self.cancel_wanted = true;
                }
            }
            TurnCommand::Permission { request_id, reply } => self.answer(&request_id, reply, sink),
            TurnCommand::Steer { text, attachments, now, key } => {
                if self.finished {
                    return;
                }
                if now && self.turn.is_some() {
                    self.restart.push((text, attachments, key));
                    self.interrupt();
                } else {
                    self.steer(text, attachments, key);
                }
            }
            TurnCommand::Unsteer { key } => {
                let held = self.waiting.len() + self.restart.len();
                self.waiting.retain(|(_, _, k)| *k != key);
                self.restart.retain(|(_, _, k)| *k != key);
                let taken_back = self.waiting.len() + self.restart.len() < held;
                sink.send(AgentEvent::Unsteered { key, taken_back });
            }
            // Codex has no way to stop one task alone.
            TurnCommand::StopTask { .. } => {}
        }
    }

    fn take_output(&mut self) -> Vec<String> {
        std::mem::take(&mut self.out)
    }

    fn finished(&self) -> bool {
        self.finished
    }

    fn stop_deadline(&self) -> Option<Instant> {
        self.stop_at
    }
}

/// A turn's input: the text, then attached images (other files are named
/// in the text).
fn input(text: &str, parts: &[Part]) -> Vec<Value> {
    let mut text = text.to_string();
    let files: Vec<&str> = parts.iter().filter_map(|p| if let Part::File { path, text: None, .. } = p { Some(path.as_str()) } else { None }).collect();
    if !files.is_empty() {
        text.push_str("\n\nAttached files (read them as needed):\n");
        for f in files {
            text.push_str(&format!("- {f}\n"));
        }
    }
    let mut out = vec![json!({"type": "text", "text": text})];
    for p in parts {
        if let Part::Image { source: MediaSource::Path { path }, .. } = p {
            out.push(json!({"type": "localImage", "path": path}));
        }
    }
    out
}

/// An app-server item in the shape of `codex exec --json`'s, for the
/// shared parser.
fn exec_item(it: &Value) -> Option<Value> {
    let id = it["id"].clone();
    let status = snake(s(it, "status").unwrap_or("inProgress"));
    Some(match s(it, "type")? {
        "agentMessage" => json!({"type": "agent_message", "id": id, "text": it["text"]}),
        "reasoning" => {
            let parts: Vec<&str> = it["summary"].as_array().into_iter().flatten().filter_map(|x| x.as_str()).filter(|x| !x.is_empty()).collect();
            json!({"type": "reasoning", "id": id, "text": parts.join("\n\n")})
        }
        "commandExecution" => json!({"type": "command_execution", "id": id, "command": it["command"], "aggregated_output": it["aggregatedOutput"], "exit_code": it["exitCode"], "status": status}),
        "fileChange" => {
            let changes: Vec<Value> = it["changes"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|c| json!({"path": c["path"], "kind": s(&c["kind"], "type").unwrap_or("update"), "diff": c["diff"]}))
                .collect();
            json!({"type": "file_change", "id": id, "changes": changes, "status": status})
        }
        "mcpToolCall" => json!({"type": "mcp_tool_call", "id": id, "server": it["server"], "tool": it["tool"], "arguments": it["arguments"], "result": it["result"], "error": it["error"], "status": status}),
        "webSearch" => json!({"type": "web_search", "id": id, "query": it["query"], "results": it["results"]}),
        "collabAgentToolCall" => {
            let states: serde_json::Map<String, Value> = it["agentsStates"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(k, v)| (k.clone(), json!({"status": snake(s(v, "status").unwrap_or("running")), "message": v["message"]})))
                .collect();
            json!({"type": "collab_tool_call", "id": id, "tool": snake(s(it, "tool").unwrap_or("spawnAgent")), "prompt": it["prompt"], "receiver_thread_ids": it["receiverThreadIds"], "agents_states": states, "status": status, "model": it["model"]})
        }
        _ => return None,
    })
}

/// `inProgress` → `in_progress`.
fn snake(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_uppercase() {
            out.push('_');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn usage_of(v: &Value) -> Usage {
    let input = u(v, "inputTokens");
    let cached = u(v, "cachedInputTokens");
    usage_from(input.saturating_sub(cached), u(v, "outputTokens"), cached, u(v, "cacheWriteInputTokens"), u(v, "reasoningOutputTokens"), None)
}

fn minus(a: &Usage, b: &Usage) -> Usage {
    usage_from(
        a.input_tokens.saturating_sub(b.input_tokens),
        a.output_tokens.saturating_sub(b.output_tokens),
        a.cache_read_tokens.saturating_sub(b.cache_read_tokens),
        a.cache_write_tokens.saturating_sub(b.cache_write_tokens),
        a.reasoning_tokens.saturating_sub(b.reasoning_tokens),
        None,
    )
}

/// Plan-limit windows named as the account's own are ("5-hour", "Weekly").
fn window_name(minutes: u64) -> String {
    match minutes {
        300 => "5-hour".into(),
        10080 => "Weekly".into(),
        1440 => "Daily".into(),
        m if m % 1440 == 0 => format!("{}-day", m / 1440),
        m if m % 60 == 0 => format!("{}-hour", m / 60),
        m => format!("{m}-min"),
    }
}

fn same_path(a: &str, b: &str) -> bool {
    let n = |p: &str| p.trim_start_matches(r"\\?\").trim_end_matches(['\\', '/']).replace('\\', "/").to_lowercase();
    n(a) == n(b)
}

fn short_path(path: &str, cwd: &str) -> String {
    let (p, c) = (path.replace('\\', "/"), cwd.replace('\\', "/"));
    p.strip_prefix(&format!("{}/", c.trim_end_matches('/'))).map(String::from).unwrap_or(p)
}

/// A form field's choices as (label, value), and whether several may be picked.
fn field_options(prop: &Value) -> (Vec<(String, Value)>, bool) {
    let titled = |list: &Value| -> Vec<(String, Value)> { list.as_array().into_iter().flatten().filter_map(|o| Some((s(o, "title")?.to_string(), o["const"].clone()))).collect() };
    let plain = |list: &Value, names: &Value| -> Vec<(String, Value)> {
        list.as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .filter_map(|(i, v)| Some((names[i].as_str().or(v.as_str())?.to_string(), v.clone())))
            .collect()
    };
    match s(prop, "type") {
        Some("boolean") => (vec![("Yes".into(), json!(true)), ("No".into(), json!(false))], false),
        Some("array") => {
            let items = &prop["items"];
            let list = if items["anyOf"].is_array() { titled(&items["anyOf"]) } else { plain(&items["enum"], &Value::Null) };
            (list, true)
        }
        _ if prop["oneOf"].is_array() => (titled(&prop["oneOf"]), false),
        _ if prop["enum"].is_array() => (plain(&prop["enum"], &prop["enumNames"]), false),
        _ => (Vec::new(), false),
    }
}

/// A form answer as the value its field takes.
fn field_value(prop: &Value, answer: &[String]) -> Option<Value> {
    let first = answer.first().map(|a| a.trim()).filter(|a| !a.is_empty())?;
    let (options, multi) = field_options(prop);
    let pick = |a: &str| options.iter().find(|(l, _)| l == a).map(|(_, v)| v.clone()).unwrap_or_else(|| json!(a));
    if multi {
        return Some(Value::Array(answer.iter().map(|a| pick(a)).collect()));
    }
    Some(match s(prop, "type") {
        Some("boolean") => json!(matches!(first.to_ascii_lowercase().as_str(), "yes" | "true")),
        Some("integer") => json!(first.parse::<i64>().ok()?),
        Some("number") => json!(first.parse::<f64>().ok()?),
        _ => pick(first),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::Launch;
    use larrier_core::{AgentKind, Protocol, RunMode};

    fn req(plan: bool) -> TurnRequest {
        let mut modes = larrier_core::ModeSelection::default();
        if plan {
            modes.ids = vec!["plan".into()];
        }
        TurnRequest {
            agent: AgentKind::Codex,
            protocol: Protocol::CodexExec,
            session_id: "s".into(),
            prompt: "go".into(),
            attachments: Vec::new(),
            history: Vec::new(),
            model: None,
            mode: None,
            modes,
            cwd: std::path::PathBuf::from(r"C:\work\demo"),
            run_mode: RunMode::Ask,
            resume_id: None,
            resume_path: None,
            resume_at: None,
            handoff: None,
            launch: Launch::default(),
            chat: None,
            notice: None,
            run_as: None,
            browser: None,
        }
    }

    /// Replays a recorded app-server session: each event Larrier shows, and
    /// everything it wrote back (requests and replies).
    fn live(fixture: &str, plan: bool, mut answer: impl FnMut(&PermissionRequest) -> Option<TurnCommand>) -> (Vec<AgentEvent>, Vec<Value>) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = EventSink::new(tx);
        let mut p = CodexLive::new(&req(plan));
        let mut written: Vec<Value> = Vec::new();
        let mut out = Vec::new();
        for l in fixture.lines().filter(|l| !l.trim().is_empty()) {
            written.extend(p.take_output().iter().map(|l| serde_json::from_str::<Value>(l).unwrap()));
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
        written.extend(p.take_output().iter().map(|l| serde_json::from_str::<Value>(l).unwrap()));
        (out.into_iter().filter(|e| !matches!(e, AgentEvent::Log { .. })).collect(), written)
    }

    fn reply(id: &str, option: &str) -> Option<TurnCommand> {
        Some(TurnCommand::Permission { request_id: id.into(), reply: Reply::option(Some(option.into())) })
    }

    /// Real run (Codex 0.165): apply_patch adding accept.txt, approved once.
    #[test]
    fn replay_file_approval() {
        let (ev, written) = live(include_str!("../../tests/fixtures/codex_app_approval.jsonl"), false, |r| reply(&r.id, "accept"));
        let methods: Vec<&str> = written.iter().filter_map(|w| w["method"].as_str()).collect();
        assert_eq!(methods, ["initialize", "initialized", "thread/start", "turn/start"]);
        assert_eq!(written[0]["params"]["capabilities"]["experimentalApi"], true);
        assert!(written[3]["params"].get("collaborationMode").is_none(), "no mode named outside plan mode");
        assert!(matches!(&ev[0], AgentEvent::NativeSession { path: Some(p), .. } if p.ends_with(".jsonl")));
        let req = ev.iter().find_map(|e| match e { AgentEvent::Permission { request } => Some(request), _ => None }).unwrap();
        assert_eq!(req.title, "Write accept.txt");
        assert_eq!(req.options.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(), ["accept", "session", "decline"]);
        let answer = written.iter().find(|w| w.get("result").is_some()).unwrap();
        assert_eq!((answer["id"].as_u64(), answer["result"]["decision"].as_str()), (Some(0), Some("accept")));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolUpdate { status: Some(ToolStatus::Approval), .. })));
        let diff = ev.iter().find_map(|e| match e { AgentEvent::ToolUpdate { diffs, .. } if !diffs.is_empty() => Some(diffs[0].clone()), _ => None }).unwrap();
        assert_eq!((diff.added, diff.new_text.as_deref()), (1, Some("yes\n")));
        let text: String = ev.iter().filter_map(|e| match e { AgentEvent::Text { text } | AgentEvent::TextDelta { text } => Some(text.as_str()), _ => None }).collect();
        assert!(text.contains("Created accept.txt containing yes."), "{text}");
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Usage { usage } if usage.input_tokens + usage.cache_read_tokens > 0)));
        assert!(matches!(ev.last(), Some(AgentEvent::Done { ok: true, .. })));
    }

    /// Real run: plan mode, request_user_input answered from the card.
    #[test]
    fn replay_question() {
        let (ev, written) = live(include_str!("../../tests/fixtures/codex_app_question.jsonl"), true, |r| {
            Some(TurnCommand::Permission { request_id: r.id.clone(), reply: Reply { answers: vec![vec!["Blue".into()]], ..Default::default() } })
        });
        let turn = written.iter().find(|w| w["method"] == "turn/start").unwrap();
        assert_eq!(turn["params"]["collaborationMode"]["mode"], "plan");
        assert_eq!(turn["params"]["collaborationMode"]["settings"]["model"], "gpt-6.1-sol", "the thread's model");
        let req = ev.iter().find_map(|e| match e { AgentEvent::Permission { request } => Some(request), _ => None }).unwrap();
        assert_eq!((req.kind, req.questions.len()), (RequestKind::Question, 1));
        assert_eq!((req.questions[0].question.as_str(), req.questions[0].options.len()), ("Which color do you prefer?", 2));
        let answer = written.iter().find(|w| w.get("result").is_some()).unwrap();
        assert_eq!(answer["result"]["answers"]["preferred_color"]["answers"], json!(["Blue"]));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::RateLimit { window, .. } if window == "5-hour")));
        // No plan was proposed, so the turn simply ends.
        assert!(matches!(ev.last(), Some(AgentEvent::Done { ok: true, .. })));
    }

    /// Real run: a command approved, its output streaming, then stopped.
    #[test]
    fn replay_output_and_interrupt() {
        let mut stopped = false;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = EventSink::new(tx);
        let mut p = CodexLive::new(&req(false));
        let mut written: Vec<Value> = Vec::new();
        let mut ev = Vec::new();
        for l in include_str!("../../tests/fixtures/codex_app_interrupt.jsonl").lines().filter(|l| !l.trim().is_empty()) {
            p.handle(l, &sink);
            while let Ok(e) = rx.try_recv() {
                if let AgentEvent::Permission { request } = &e {
                    p.command(TurnCommand::Permission { request_id: request.id.clone(), reply: Reply::option(Some("accept".into())) }, &sink);
                }
                // Stop once output has streamed.
                if !stopped && matches!(&e, AgentEvent::ToolUpdate { output: Some(o), status: None, .. } if o.contains("tick")) {
                    stopped = true;
                    p.command(TurnCommand::Cancel, &sink);
                }
                ev.push(e);
            }
            written.extend(p.take_output().iter().map(|l| serde_json::from_str::<Value>(l).unwrap()));
        }
        assert!(stopped, "output streamed before the end");
        let req = ev.iter().find_map(|e| match e { AgentEvent::Permission { request } => Some(request), _ => None }).unwrap();
        assert!(req.title.starts_with("for ($i=1"), "the command, not its shell: {}", req.title);
        let rule = req.options.iter().find(|o| o.id == "rule").unwrap();
        assert!(rule.label.starts_with("Always allow `for ($i=1") && rule.label.ends_with("…`"), "{}", rule.label);
        let int = written.iter().find(|w| w["method"] == "turn/interrupt").expect("interrupt sent");
        assert_eq!(int["params"]["turnId"], "01a18d17-89c6-77f3-8e3c-38bd1cf7a632");
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolUpdate { status: Some(ToolStatus::Cancelled), .. })), "the open command is closed");
        assert!(matches!(ev.last(), Some(AgentEvent::Done { ok: false, stop_reason: Some(r) }) if r == "cancelled"));
    }

    /// A message sent mid-turn is steered in; one that misses the turn
    /// starts the next.
    #[test]
    fn steer_and_follow_on_turn() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = EventSink::new(tx);
        let mut p = CodexLive::new(&req(false));
        p.take_output();
        for l in [
            r#"{"id":0,"result":{}}"#,
            r#"{"id":1,"result":{"thread":{"id":"t1"},"model":"m"}}"#,
            r#"{"id":2,"result":{"turn":{"id":"u1"}}}"#,
        ] {
            p.handle(l, &sink);
        }
        p.take_output();
        p.command(TurnCommand::Steer { text: "also this".into(), attachments: Vec::new(), now: false, key: 1 }, &sink);
        let w: Value = serde_json::from_str(&p.take_output()[0]).unwrap();
        assert_eq!((w["method"].as_str(), w["params"]["expectedTurnId"].as_str()), (Some("turn/steer"), Some("u1")));
        let sid = w["id"].as_u64().unwrap();
        p.handle(&json!({"id": sid, "result": {"turnId": "u1"}}).to_string(), &sink);
        p.command(TurnCommand::Steer { text: "late".into(), attachments: Vec::new(), now: false, key: 2 }, &sink);
        let late: Value = serde_json::from_str(&p.take_output()[0]).unwrap();
        p.handle(&json!({"id": late["id"], "error": {"message": "no active turn"}}).to_string(), &sink);
        p.handle(r#"{"method":"turn/completed","params":{"turn":{"id":"u1","status":"completed"}}}"#, &sink);
        let next: Value = serde_json::from_str(&p.take_output()[0]).unwrap();
        assert_eq!((next["method"].as_str(), next["params"]["input"][0]["text"].as_str()), (Some("turn/start"), Some("late")));
        let mut steered = Vec::new();
        while let Ok(e) = rx.try_recv() {
            if let AgentEvent::Steered { text, .. } = e {
                steered.push(text);
            }
        }
        assert_eq!(steered, ["also this", "late"]);
        assert!(!p.finished());
        // A message Larrier still holds (no turn yet) can be taken back; one
        // Codex has can't.
        p.take_output();
        let n = p.waiting.len();
        p.turn = None;
        p.command(TurnCommand::Steer { text: "held".into(), attachments: Vec::new(), now: false, key: 3 }, &sink);
        assert_eq!(p.waiting.len(), n + 1);
        p.command(TurnCommand::Unsteer { key: 3 }, &sink);
        p.command(TurnCommand::Unsteer { key: 1 }, &sink);
        let mut answers = Vec::new();
        while let Ok(e) = rx.try_recv() {
            if let AgentEvent::Unsteered { key, taken_back } = e {
                answers.push((key, taken_back));
            }
        }
        assert_eq!(answers, [(3, true), (1, false)]);
        assert_eq!(p.waiting.len(), n);
    }

    #[test]
    fn items_in_exec_shape() {
        let cmd = exec_item(&json!({"type": "commandExecution", "id": "c1", "command": "ls", "aggregatedOutput": "a", "exitCode": 0, "status": "inProgress"})).unwrap();
        assert_eq!((cmd["type"].as_str(), cmd["status"].as_str()), (Some("command_execution"), Some("in_progress")));
        let fc = exec_item(&json!({"type": "fileChange", "id": "f1", "changes": [{"path": "a.txt", "kind": {"type": "add"}, "diff": "yes\n"}], "status": "declined"})).unwrap();
        assert_eq!(fc["changes"][0]["kind"], "add");
        let collab = exec_item(&json!({"type": "collabAgentToolCall", "id": "x", "tool": "spawnAgent", "agentsStates": {"t1": {"status": "notFound"}}, "status": "completed"})).unwrap();
        assert_eq!((collab["tool"].as_str(), collab["agents_states"]["t1"]["status"].as_str()), (Some("spawn_agent"), Some("not_found")));
        assert!(exec_item(&json!({"type": "userMessage", "id": "u"})).is_none());
    }

    #[test]
    fn server_options_only() {
        let args: Vec<String> = ["-c", "model_reasoning_effort=\"high\"", "--full-auto", "--enable", "x", "--profile=work", "-m", "gpt"].iter().map(|s| s.to_string()).collect();
        assert_eq!(config_args(&args), ["-c", "model_reasoning_effort=\"high\"", "--enable", "x", "--profile=work"]);
    }

    #[test]
    fn form_answers_take_their_types() {
        let b = json!({"type": "boolean"});
        assert_eq!(field_value(&b, &["Yes".into()]), Some(json!(true)));
        let n = json!({"type": "integer"});
        assert_eq!(field_value(&n, &["42".into()]), Some(json!(42)));
        let e = json!({"type": "string", "oneOf": [{"const": "eu", "title": "Europe"}, {"const": "us", "title": "United States"}]});
        assert_eq!(field_value(&e, &["Europe".into()]), Some(json!("eu")));
        assert_eq!(field_value(&json!({"type": "string"}), &[]), None);
    }
}
