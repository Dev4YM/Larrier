//! Agent Client Protocol client (protocol version 1): newline-delimited
//! JSON-RPC 2.0 over the agent's stdio.
//!
//! Larrier implements the client side: `fs/read_text_file`,
//! `fs/write_text_file`, the `terminal/*` methods and
//! `session/request_permission` (surfaced to the user, or auto-approved in
//! Full auto). Sessions are resumed with `session/load` when the agent
//! supports it; replayed history is ignored because Larrier already has it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use larrier_core::{
    lsf::MediaSource, AgentKind, FileDiff, Message, Part, RunMode, Session, ToolCall, ToolKind, ToolStatus,
};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;

use super::{content_text, normalize_input, plan_entries, s, usage_from};
use crate::events::{AgentEvent, EventSink, PermissionOption, PermissionRequest, TurnCommand};
use crate::request::TurnRequest;

pub const PROTOCOL_VERSION: u64 = 1;

pub async fn run(req: TurnRequest, sink: EventSink, control: mpsc::UnboundedReceiver<TurnCommand>) {
    if req.launch.is_empty() {
        sink.error(format!("No ACP command is configured for {}", req.agent.display_name()));
        sink.done(false, Some("error".into()));
        return;
    }
    let mut cmd = req.launch.command();
    cmd.current_dir(&req.cwd);
    cmd.args(&req.launch.extra_args);
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            sink.error(format!("Failed to start {} over ACP: {e}", req.agent.display_name()));
            sink.done(false, Some("error".into()));
            return;
        }
    };
    let stdin = child.stdin.take().expect("piped");
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");

    // Writer: one JSON message per line.
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Value>();
    tokio::spawn(async move {
        let mut stdin = stdin;
        while let Some(v) = out_rx.recv().await {
            let mut line = serde_json::to_string(&v).unwrap_or_default();
            line.push('\n');
            if stdin.write_all(line.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
                break;
            }
        }
    });
    // Reader: parse each stdout line.
    let (in_tx, in_rx) = mpsc::unbounded_channel::<Value>();
    let log = sink.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(l)) = lines.next_line().await {
            let t = l.trim();
            if t.is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(t) {
                Ok(v) => {
                    if in_tx.send(v).is_err() {
                        break;
                    }
                }
                Err(_) => log.log(t.to_string()),
            }
        }
    });
    let tail = Arc::new(Mutex::new(Vec::<String>::new()));
    let tail2 = tail.clone();
    let log = sink.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(l)) = lines.next_line().await {
            log.log(l.clone());
            let mut t = tail2.lock().unwrap();
            t.push(super::process::strip_ansi(&l));
            if t.len() > 30 {
                t.remove(0);
            }
        }
    });

    let mut acp = Acp::new(req, sink.clone(), out_tx, in_rx, control);
    acp.child = Some(child);
    acp.stderr_tail = tail;
    acp.session_flow().await;
    acp.shutdown().await;
}

#[derive(Debug)]
pub enum RpcError {
    Remote { code: i64, message: String },
    Closed,
    Cancelled,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RpcError::Remote { code, message } => write!(f, "{message} (code {code})"),
            RpcError::Closed => write!(f, "the agent process exited"),
            RpcError::Cancelled => write!(f, "cancelled"),
        }
    }
}

struct PendingPermission {
    rpc_id: Value,
    request_id: String,
}

struct Terminal {
    output: Arc<Mutex<TermOut>>,
    exit: watch::Receiver<Option<(Option<i64>, Option<String>)>>,
    /// Dropping this sender kills the process.
    kill: Option<oneshot::Sender<()>>,
}

#[derive(Default)]
struct TermOut {
    buf: String,
    truncated: bool,
    limit: Option<usize>,
}

impl TermOut {
    fn push(&mut self, s: &str) {
        self.buf.push_str(s);
        if let Some(limit) = self.limit {
            if self.buf.len() > limit {
                let mut cut = self.buf.len() - limit;
                while !self.buf.is_char_boundary(cut) {
                    cut += 1;
                }
                self.buf.drain(..cut);
                self.truncated = true;
            }
        }
    }
}

pub struct Acp {
    req: TurnRequest,
    sink: EventSink,
    out: mpsc::UnboundedSender<Value>,
    incoming: mpsc::UnboundedReceiver<Value>,
    control: mpsc::UnboundedReceiver<TurnCommand>,
    control_closed: bool,
    next_id: u64,
    responses: HashMap<u64, Result<Value, RpcError>>,
    session_id: Option<String>,
    replaying: bool,
    known_tools: HashSet<String>,
    permission: Option<PendingPermission>,
    terminals: HashMap<String, Terminal>,
    term_counter: u64,
    cancelled: bool,
    cancel_deadline: Option<Instant>,
    cost: Option<f64>,
    child: Option<tokio::process::Child>,
    stderr_tail: Arc<Mutex<Vec<String>>>,
}

impl Acp {
    pub fn new(
        req: TurnRequest,
        sink: EventSink,
        out: mpsc::UnboundedSender<Value>,
        incoming: mpsc::UnboundedReceiver<Value>,
        control: mpsc::UnboundedReceiver<TurnCommand>,
    ) -> Self {
        Acp {
            req,
            sink,
            out,
            incoming,
            control,
            control_closed: false,
            next_id: 1,
            responses: HashMap::new(),
            session_id: None,
            replaying: false,
            known_tools: HashSet::new(),
            permission: None,
            terminals: HashMap::new(),
            term_counter: 0,
            cancelled: false,
            cancel_deadline: None,
            cost: None,
            child: None,
            stderr_tail: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn send(&self, v: Value) {
        let _ = self.out.send(v);
    }

    fn respond(&self, id: Value, result: Value) {
        self.send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    }

    fn respond_error(&self, id: Value, code: i64, message: impl Into<String>) {
        self.send(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message.into() } }));
    }

    fn notify(&self, method: &str, params: Value) {
        self.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    /// Send a request and keep serving the agent until its response arrives.
    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            if let Some(r) = self.responses.remove(&id) {
                return r;
            }
            let deadline = self.cancel_deadline;
            tokio::select! {
                msg = self.incoming.recv() => match msg {
                    Some(m) => self.dispatch(m).await,
                    None => return Err(RpcError::Closed),
                },
                cmd = self.control.recv(), if !self.control_closed => {
                    if cmd.is_none() {
                        self.control_closed = true;
                    }
                    self.on_control(cmd);
                }
                _ = tokio::time::sleep_until(deadline.unwrap_or_else(Instant::now)), if deadline.is_some() => {
                    return Err(RpcError::Cancelled);
                }
            }
        }
    }

    async fn dispatch(&mut self, msg: Value) {
        let has_method = msg.get("method").is_some();
        match (msg.get("id").cloned(), has_method) {
            (Some(id), false) => {
                let key = id.as_u64().or_else(|| id.as_str().and_then(|s| s.parse().ok()));
                if let Some(k) = key {
                    let r = match msg.get("error").filter(|e| !e.is_null()) {
                        Some(e) => Err(RpcError::Remote {
                            code: e.get("code").and_then(|c| c.as_i64()).unwrap_or(-1),
                            message: s(e, "message").unwrap_or("error").to_string(),
                        }),
                        None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                    };
                    self.responses.insert(k, r);
                }
            }
            (Some(id), true) => {
                let method = s(&msg, "method").unwrap_or("").to_string();
                let params = msg.get("params").cloned().unwrap_or(Value::Null);
                self.on_request(id, &method, params).await;
            }
            (None, true) => {
                if s(&msg, "method") == Some("session/update") && !self.replaying {
                    let update = msg["params"]["update"].clone();
                    self.on_update(&update);
                }
            }
            (None, false) => {}
        }
    }

    fn on_control(&mut self, cmd: Option<TurnCommand>) {
        match cmd {
            Some(TurnCommand::Cancel) | None => {
                if self.cancelled {
                    return;
                }
                self.cancelled = true;
                if let Some(sid) = &self.session_id {
                    self.notify("session/cancel", json!({ "sessionId": sid }));
                }
                if let Some(p) = self.permission.take() {
                    self.respond(p.rpc_id, json!({ "outcome": { "outcome": "cancelled" } }));
                }
                self.cancel_deadline = Some(Instant::now() + Duration::from_secs(5));
            }
            Some(TurnCommand::Permission { request_id, option_id }) => {
                if self.permission.as_ref().map(|p| p.request_id == request_id).unwrap_or(false) {
                    let p = self.permission.take().unwrap();
                    let outcome = match option_id {
                        Some(o) => json!({ "outcome": { "outcome": "selected", "optionId": o } }),
                        None => json!({ "outcome": { "outcome": "cancelled" } }),
                    };
                    self.respond(p.rpc_id, outcome);
                }
            }
        }
    }

    // ------------------------------------------------------------ requests

    async fn on_request(&mut self, id: Value, method: &str, params: Value) {
        match method {
            "session/request_permission" => self.on_permission(id, params),
            "fs/read_text_file" => {
                let path = self.resolve_path(s(&params, "path").unwrap_or(""));
                match tokio::fs::read_to_string(&path).await {
                    Ok(text) => {
                        let line = params.get("line").and_then(|l| l.as_u64()).unwrap_or(1).max(1) as usize;
                        let limit = params.get("limit").and_then(|l| l.as_u64()).map(|l| l as usize);
                        let content = if line == 1 && limit.is_none() {
                            text
                        } else {
                            let lines: Vec<&str> = text.lines().skip(line - 1).take(limit.unwrap_or(usize::MAX)).collect();
                            lines.join("\n")
                        };
                        self.respond(id, json!({ "content": content }));
                    }
                    Err(e) => self.respond_error(id, -32603, format!("{}: {e}", path.display())),
                }
            }
            "fs/write_text_file" => {
                let path = self.resolve_path(s(&params, "path").unwrap_or(""));
                let content = s(&params, "content").unwrap_or("").to_string();
                if let Some(dir) = path.parent() {
                    let _ = tokio::fs::create_dir_all(dir).await;
                }
                match tokio::fs::write(&path, content).await {
                    Ok(()) => self.respond(id, Value::Null),
                    Err(e) => self.respond_error(id, -32603, format!("{}: {e}", path.display())),
                }
            }
            "terminal/create" => match self.terminal_create(&params) {
                Ok(tid) => self.respond(id, json!({ "terminalId": tid })),
                Err(e) => self.respond_error(id, -32603, e),
            },
            "terminal/output" => {
                let tid = s(&params, "terminalId").unwrap_or("");
                match self.terminals.get(tid) {
                    Some(t) => {
                        let o = t.output.lock().unwrap();
                        let mut r = json!({ "output": o.buf, "truncated": o.truncated });
                        if let Some((code, signal)) = t.exit.borrow().clone() {
                            r["exitStatus"] = json!({ "exitCode": code, "signal": signal });
                        }
                        drop(o);
                        self.respond(id, r);
                    }
                    None => self.respond_error(id, -32602, format!("unknown terminal {tid}")),
                }
            }
            "terminal/wait_for_exit" => {
                let tid = s(&params, "terminalId").unwrap_or("").to_string();
                match self.terminals.get(&tid) {
                    Some(t) => {
                        let mut rx = t.exit.clone();
                        let out = self.out.clone();
                        tokio::spawn(async move {
                            let status = rx.wait_for(|v| v.is_some()).await.ok().and_then(|v| v.clone());
                            let (code, signal) = status.unwrap_or((None, Some("killed".into())));
                            let _ = out.send(json!({ "jsonrpc": "2.0", "id": id, "result": { "exitCode": code, "signal": signal } }));
                        });
                    }
                    None => self.respond_error(id, -32602, format!("unknown terminal {tid}")),
                }
            }
            "terminal/kill" => {
                let tid = s(&params, "terminalId").unwrap_or("");
                if let Some(t) = self.terminals.get_mut(tid) {
                    if let Some(k) = t.kill.take() {
                        let _ = k.send(());
                    }
                }
                self.respond(id, Value::Null);
            }
            "terminal/release" => {
                let tid = s(&params, "terminalId").unwrap_or("").to_string();
                self.terminals.remove(&tid);
                self.respond(id, Value::Null);
            }
            other => self.respond_error(id, -32601, format!("method not supported by Larrier: {other}")),
        }
    }

    fn resolve_path(&self, p: &str) -> PathBuf {
        let path = Path::new(p);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.req.cwd.join(path)
        }
    }

    fn on_permission(&mut self, id: Value, params: Value) {
        let options: Vec<PermissionOption> = params["options"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|o| PermissionOption {
                        id: s(o, "optionId").unwrap_or("").to_string(),
                        label: s(o, "name").unwrap_or("").to_string(),
                        kind: s(o, "kind").unwrap_or("").to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        if self.cancelled {
            self.respond(id, json!({ "outcome": { "outcome": "cancelled" } }));
            return;
        }
        if self.req.run_mode == RunMode::FullAuto {
            let pick = options
                .iter()
                .find(|o| o.kind == "allow_once")
                .or_else(|| options.iter().find(|o| o.kind == "allow_always"))
                .or_else(|| options.iter().find(|o| o.kind.starts_with("allow")));
            if let Some(o) = pick {
                self.respond(id, json!({ "outcome": { "outcome": "selected", "optionId": o.id } }));
                return;
            }
        }
        let tc = &params["toolCall"];
        let tool_call_id = s(tc, "toolCallId").map(|s| s.to_string());
        if let Some(tid) = &tool_call_id {
            if self.known_tools.contains(tid) {
                self.sink.send(AgentEvent::ToolUpdate {
                    id: tid.clone(),
                    status: Some(ToolStatus::Approval),
                    output: None,
                    diffs: Vec::new(),
                    is_error: false,
                    title: None,
                });
            }
        }
        let raw = &tc["rawInput"];
        let detail = s(raw, "command")
            .map(|c| c.to_string())
            .or_else(|| s(raw, "file_path").or_else(|| s(raw, "path")).map(|p| p.to_string()))
            .or_else(|| {
                tc["locations"].as_array().and_then(|l| l.first()).and_then(|l| s(l, "path")).map(|p| p.to_string())
            });
        let title = s(tc, "title").filter(|t| !t.is_empty()).map(|t| format!("Allow: {}", t.trim_matches('`'))).unwrap_or_else(|| "The agent is asking for permission".into());
        let request_id = format!("acp-{}", id);
        self.permission = Some(PendingPermission { rpc_id: id, request_id: request_id.clone() });
        self.sink.send(AgentEvent::Permission { request: PermissionRequest { id: request_id, title, tool_call_id, detail, options } });
    }

    fn terminal_create(&mut self, params: &Value) -> Result<String, String> {
        let command = s(params, "command").ok_or("missing command")?.to_string();
        let args: Vec<String> = params["args"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
        let cwd = s(params, "cwd").map(|c| self.resolve_path(c)).unwrap_or_else(|| self.req.cwd.clone());
        let env: Vec<(String, String)> = params["env"]
            .as_array()
            .map(|a| a.iter().filter_map(|e| Some((s(e, "name")?.to_string(), s(e, "value").unwrap_or("").to_string()))).collect())
            .unwrap_or_default();
        let build = |program: &str, argv: &[String]| {
            let mut c = tokio::process::Command::new(program);
            c.args(argv).current_dir(&cwd).envs(env.iter().cloned());
            c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
            #[cfg(windows)]
            c.creation_flags(0x0800_0000);
            c
        };
        let mut child = match build(&command, &args).spawn() {
            Ok(c) => c,
            Err(_) => {
                // Shell syntax or a builtin: run through the platform shell.
                let line = std::iter::once(command.clone()).chain(args.iter().cloned()).collect::<Vec<_>>().join(" ");
                let shell: (&str, Vec<String>) = if cfg!(windows) {
                    ("cmd", vec!["/D".into(), "/S".into(), "/C".into(), line])
                } else {
                    ("sh", vec!["-c".into(), line])
                };
                build(shell.0, &shell.1).spawn().map_err(|e| format!("could not start `{command}`: {e}"))?
            }
        };
        let output = Arc::new(Mutex::new(TermOut { limit: params["outputByteLimit"].as_u64().map(|l| l as usize), ..Default::default() }));
        for stream in [child.stdout.take().map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>), child.stderr.take().map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>)]
            .into_iter()
            .flatten()
        {
            let out = output.clone();
            tokio::spawn(async move {
                let mut stream = stream;
                let mut buf = [0u8; 8192];
                loop {
                    match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => out.lock().unwrap().push(&String::from_utf8_lossy(&buf[..n])),
                    }
                }
            });
        }
        let (exit_tx, exit_rx) = watch::channel(None);
        let (kill_tx, mut kill_rx) = oneshot::channel::<()>();
        tokio::spawn(async move {
            tokio::select! {
                st = child.wait() => {
                    let code = st.ok().and_then(|s| s.code()).map(|c| c as i64);
                    let _ = exit_tx.send(Some((code, None)));
                }
                _ = &mut kill_rx => {
                    let _ = child.kill().await;
                    let _ = exit_tx.send(Some((None, Some("SIGKILL".into()))));
                }
            }
        });
        self.term_counter += 1;
        let tid = format!("term-{}", self.term_counter);
        self.terminals.insert(tid.clone(), Terminal { output, exit: exit_rx, kill: Some(kill_tx) });
        Ok(tid)
    }

    // ------------------------------------------------------------ updates

    fn on_update(&mut self, u: &Value) {
        let content = &u["content"];
        match s(u, "sessionUpdate").unwrap_or("") {
            "agent_message_chunk" => match s(content, "type").unwrap_or("text") {
                "text" => {
                    if let Some(t) = s(content, "text").filter(|t| !t.is_empty()) {
                        self.sink.text(t);
                    }
                }
                "resource_link" => {
                    let name = s(content, "name").or_else(|| s(content, "uri")).unwrap_or("");
                    self.sink.text(format!("[{name}]"));
                }
                _ => {}
            },
            "agent_thought_chunk" => {
                if let Some(t) = s(content, "text").filter(|t| !t.is_empty()) {
                    self.sink.send(AgentEvent::ReasoningDelta { text: t.to_string() });
                }
            }
            "tool_call" => {
                let mut t = acp_tool(u);
                if let Some(o) = self.terminal_text(u) {
                    t.output = Some(o);
                }
                self.known_tools.insert(t.id.clone());
                let id = t.id.clone();
                self.sink.send(AgentEvent::ToolStart { tool: t });
                self.send_images(&id, u);
            }
            "tool_call_update" => {
                let id = s(u, "toolCallId").unwrap_or("").to_string();
                if !self.known_tools.contains(&id) {
                    // Some agents only ever send updates.
                    self.known_tools.insert(id.clone());
                    self.sink.send(AgentEvent::ToolStart { tool: acp_tool(u) });
                }
                let status = s(u, "status").map(map_status);
                let (output, diffs) = tool_content(u);
                let output = self.terminal_text(u).or(output).or_else(|| u.get("rawOutput").filter(|r| !r.is_null()).map(content_text));
                // Agents refine a call's title as they learn more ("Read
                // src/main.rs" after "Read file").
                let title = s(u, "title").filter(|t| !t.trim().is_empty()).and_then(|_| acp_tool(u).title);
                self.sink.send(AgentEvent::ToolUpdate {
                    id: id.clone(),
                    is_error: status == Some(ToolStatus::Failed),
                    status,
                    output,
                    diffs,
                    title,
                });
                self.send_images(&id, u);
            }
            "plan" => {
                if let Some(entries) = u.get("entries").and_then(plan_entries) {
                    self.sink.send(AgentEvent::Plan { entries });
                }
            }
            "usage_update" => {
                let cost = &u["cost"];
                let usd = cost.get("amount").and_then(|a| a.as_f64()).filter(|_| s(cost, "currency").map(|c| c.eq_ignore_ascii_case("usd")).unwrap_or(true));
                if usd.is_some() {
                    self.cost = usd;
                }
            }
            "notice" => {
                if let Some(t) = s(u, "message").or_else(|| s(u, "text")).or_else(|| s(content, "text")) {
                    self.sink.notice(t.to_string());
                }
            }
            "compaction_update" => {
                if s(u, "status") == Some("started") || s(u, "phase") == Some("start") {
                    self.sink.notice("Compacting context");
                }
            }
            "current_mode_update" | "session_info_update" | "config_option_update" | "available_commands_update" | "user_message_chunk" => {
                self.sink.log(u.to_string());
            }
            _ => self.sink.log(u.to_string()),
        }
    }

    /// A tool's text with each embedded terminal replaced by what that
    /// terminal printed (Larrier runs the agent's terminals itself).
    fn terminal_text(&self, u: &Value) -> Option<String> {
        let items = u["content"].as_array()?;
        if !items.iter().any(|c| s(c, "type") == Some("terminal")) {
            return None;
        }
        let parts: Vec<String> = items
            .iter()
            .filter_map(|c| match s(c, "type") {
                Some("terminal") => {
                    let tid = s(c, "terminalId").unwrap_or("");
                    self.terminals.get(tid).map(|t| t.output.lock().unwrap().buf.clone()).or_else(|| Some(format!("[terminal {tid}]")))
                }
                Some("content") => Some(content_text(&json!([c["content"].clone()]))).filter(|t| !t.is_empty()),
                _ => None,
            })
            .collect();
        Some(parts.join("\n"))
    }

    /// Images inside a tool's content (screenshots, image reads).
    fn send_images(&self, id: &str, u: &Value) {
        let images: Vec<Part> = u["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|c| s(c, "type") == Some("content"))
            .filter_map(|c| {
                let img = &c["content"];
                if s(img, "type") != Some("image") {
                    return None;
                }
                let data = s(img, "data")?.to_string();
                Some(Part::Image { mime: s(img, "mimeType").map(String::from), source: MediaSource::Base64 { data } })
            })
            .collect();
        if !images.is_empty() {
            self.sink.send(AgentEvent::ToolImages { id: id.to_string(), images });
        }
    }

    // ------------------------------------------------------------ flow

    pub async fn session_flow(&mut self) {
        let init = self
            .call(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "clientCapabilities": { "fs": { "readTextFile": true, "writeTextFile": true }, "terminal": true },
                    "clientInfo": { "name": "larrier", "title": "Larrier", "version": larrier_core::APP_VERSION },
                }),
            )
            .await;
        let init = match init {
            Ok(v) => v,
            Err(e) => return self.fail(format!("ACP initialize failed: {e}")),
        };
        let caps = &init["agentCapabilities"];
        let can_load = caps["loadSession"].as_bool().unwrap_or(false);
        let image_ok = caps["promptCapabilities"]["image"].as_bool().unwrap_or(false);
        let cwd = self.req.cwd.to_string_lossy().to_string();

        let mut session_info = Value::Null;
        let mut handoff = self.req.handoff.clone();
        if let Some(rid) = self.req.resume_id.clone().filter(|_| can_load) {
            self.replaying = true;
            let r = self.call("session/load", json!({ "sessionId": rid, "cwd": cwd, "mcpServers": [] })).await;
            self.replaying = false;
            match r {
                Ok(v) => {
                    self.session_id = Some(rid);
                    session_info = v;
                }
                Err(RpcError::Cancelled) => return self.finish_cancelled(),
                Err(e) => self.sink.log(format!("session/load failed ({e}); starting a new session")),
            }
        }
        if self.session_id.is_none() {
            if self.req.resume_id.is_some() {
                // The agent can't reload its old session: hand it the whole history.
                handoff = Some(full_history(&self.req));
            }
            match self.call("session/new", json!({ "cwd": cwd, "mcpServers": [] })).await {
                Ok(v) => {
                    match s(&v, "sessionId") {
                        Some(id) => self.session_id = Some(id.to_string()),
                        None => return self.fail("The agent did not return a session id".into()),
                    }
                    session_info = v;
                }
                Err(RpcError::Remote { code: -32000, message }) => {
                    let methods: Vec<String> = init["authMethods"].as_array().map(|a| a.iter().filter_map(|m| s(m, "name").or_else(|| s(m, "id")).map(|s| s.to_string())).collect()).unwrap_or_default();
                    let login = self.req.agent.spec().login_command.unwrap_or("its login command");
                    return self.fail(format!(
                        "{} needs you to sign in ({message}). Run `{login}` in a terminal{}.",
                        self.req.agent.display_name(),
                        if methods.is_empty() { String::new() } else { format!(" — supported: {}", methods.join(", ")) }
                    ));
                }
                Err(RpcError::Cancelled) => return self.finish_cancelled(),
                Err(e) => return self.fail(format!("ACP session/new failed: {e}")),
            }
        }
        let sid = self.session_id.clone().unwrap();
        self.sink.send(AgentEvent::NativeSession { id: sid.clone(), path: None });

        // Mode.
        if let Some(mode) = self.req.mode.clone() {
            let modes = session_info["modes"]["availableModes"].as_array().cloned().unwrap_or_default();
            let hit = modes.iter().find(|m| s(m, "id") == Some(mode.as_str()) || s(m, "name").map(|n| n.eq_ignore_ascii_case(&mode)).unwrap_or(false));
            if let Some(m) = hit.and_then(|m| s(m, "id")).map(|s| s.to_string()) {
                if session_info["modes"]["currentModeId"].as_str() != Some(m.as_str()) {
                    if let Err(e) = self.call("session/set_mode", json!({ "sessionId": sid, "modeId": m })).await {
                        self.sink.log(format!("set_mode: {e}"));
                    }
                }
            }
        }
        // Model: a config option with category "model", or the legacy models API.
        if let Some(model) = self.req.model.clone() {
            let opts = session_info["configOptions"].as_array().cloned().unwrap_or_default();
            if let Some(opt) = opts.iter().find(|o| s(o, "category") == Some("model")) {
                let choices = opt["options"].as_array().cloned().unwrap_or_default();
                let value = choices
                    .iter()
                    .find(|c| s(c, "value") == Some(model.as_str()) || s(c, "name").map(|n| n.eq_ignore_ascii_case(&model)).unwrap_or(false))
                    .and_then(|c| s(c, "value"))
                    .unwrap_or(model.as_str())
                    .to_string();
                let cid = s(opt, "id").unwrap_or("model").to_string();
                if let Err(e) = self.call("session/set_config_option", json!({ "sessionId": sid, "configId": cid, "value": value })).await {
                    self.sink.log(format!("set_config_option(model): {e}"));
                }
            } else if session_info.get("models").is_some() {
                if let Err(e) = self.call("session/set_model", json!({ "sessionId": sid, "modelId": model })).await {
                    self.sink.log(format!("set_model: {e}"));
                }
            }
        }

        let prompt = self.prompt_blocks(handoff, image_ok);
        match self.call("session/prompt", json!({ "sessionId": sid, "prompt": prompt })).await {
            Ok(v) => {
                let stop = s(&v, "stopReason").unwrap_or("end_turn").to_string();
                if let Some(c) = self.cost {
                    self.sink.send(AgentEvent::Usage { usage: usage_from(0, 0, 0, 0, 0, Some(c)) });
                }
                match stop.as_str() {
                    "cancelled" => self.sink.done(false, Some(stop)),
                    "refusal" => {
                        self.sink.notice("The agent declined to continue");
                        self.sink.done(true, Some(stop));
                    }
                    "max_tokens" | "max_turn_requests" => {
                        self.sink.notice(format!("Stopped: {}", stop.replace('_', " ")));
                        self.sink.done(true, Some(stop));
                    }
                    _ => self.sink.done(true, Some(stop)),
                }
            }
            Err(RpcError::Cancelled) => self.finish_cancelled(),
            Err(e) if self.cancelled => {
                self.sink.log(e.to_string());
                self.finish_cancelled()
            }
            Err(e) => self.fail(format!("{} failed: {e}", self.req.agent.display_name())),
        }
    }

    fn prompt_blocks(&self, handoff: Option<String>, image_ok: bool) -> Vec<Value> {
        let mut req = self.req.clone();
        req.handoff = handoff;
        let mut text = req.full_prompt();
        let mut blocks = Vec::new();
        for a in &self.req.attachments {
            match a {
                Part::Image { mime, source } if image_ok => {
                    let (data, mt) = match source {
                        MediaSource::Base64 { data } => (Some(data.clone()), mime.clone().unwrap_or_else(|| "image/png".into())),
                        MediaSource::Path { path } => (
                            std::fs::read(path).ok().map(|b| base64::engine::general_purpose::STANDARD.encode(b)),
                            mime.clone().unwrap_or_else(|| mime_for(path).into()),
                        ),
                        MediaSource::Url { .. } => (None, String::new()),
                    };
                    if let Some(d) = data {
                        blocks.push(json!({ "type": "image", "data": d, "mimeType": mt }));
                    }
                }
                Part::Image { source: MediaSource::Path { path }, .. } | Part::File { path, .. } => {
                    let name = Path::new(path).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| path.clone());
                    blocks.push(json!({ "type": "resource_link", "uri": file_uri(path), "name": name }));
                }
                _ => {}
            }
        }
        if text.trim().is_empty() {
            text = "(see attachments)".into();
        }
        let mut out = vec![json!({ "type": "text", "text": text })];
        out.extend(blocks);
        out
    }

    fn fail(&mut self, message: String) {
        let tail = self.stderr_tail.lock().unwrap().iter().rev().take(5).rev().cloned().collect::<Vec<_>>().join("\n");
        let msg = if tail.trim().is_empty() { message } else { format!("{message}\n{tail}") };
        self.sink.error(msg);
        self.sink.done(false, Some("error".into()));
    }

    fn finish_cancelled(&mut self) {
        self.sink.done(false, Some("cancelled".into()));
    }

    async fn shutdown(&mut self) {
        self.terminals.clear();
        if let Some(mut child) = self.child.take() {
            // Give the agent a moment to flush its session, then stop it.
            let _ = tokio::time::timeout(Duration::from_millis(300), child.wait()).await;
            let _ = child.start_kill();
        }
    }
}

fn map_status(s: &str) -> ToolStatus {
    match s {
        "pending" => ToolStatus::Pending,
        "in_progress" => ToolStatus::Running,
        "completed" => ToolStatus::Completed,
        "failed" => ToolStatus::Failed,
        _ => ToolStatus::Running,
    }
}

fn kind_of(kind: &str, title: &str) -> ToolKind {
    match kind {
        "read" => ToolKind::Read,
        "edit" => ToolKind::Edit,
        "delete" => ToolKind::Delete,
        "move" => ToolKind::Move,
        "search" => ToolKind::Search,
        "execute" => ToolKind::Shell,
        "think" => ToolKind::Think,
        "fetch" => ToolKind::Fetch,
        _ => ToolKind::classify(title.split_whitespace().next().unwrap_or("")),
    }
}

fn verb(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Read => "Read",
        ToolKind::Edit => "Edit",
        ToolKind::Write => "Write",
        ToolKind::Delete => "Delete",
        ToolKind::Move => "Move",
        ToolKind::Search => "Searched",
        ToolKind::Shell => "Ran",
        ToolKind::Fetch => "Fetched",
        ToolKind::WebSearch => "Searched web",
        ToolKind::Think => "Thought",
        ToolKind::Task => "Subagent",
        ToolKind::Todo => "Updated todos",
        ToolKind::Mcp => "MCP",
        ToolKind::Other => "Used",
    }
}

/// Text output and file diffs from a tool call's `content` array.
fn tool_content(u: &Value) -> (Option<String>, Vec<FileDiff>) {
    let mut texts = Vec::new();
    let mut diffs = Vec::new();
    if let Some(items) = u["content"].as_array() {
        for c in items {
            match s(c, "type").unwrap_or("") {
                "content" => {
                    let t = content_text(&json!([c["content"].clone()]));
                    if !t.is_empty() {
                        texts.push(t);
                    }
                }
                "diff" => {
                    let path = s(c, "path").unwrap_or("").to_string();
                    diffs.push(FileDiff::from_texts(path, s(c, "oldText").map(|s| s.to_string()), s(c, "newText").map(|s| s.to_string())));
                }
                "terminal" => texts.push(format!("[terminal {}]", s(c, "terminalId").unwrap_or(""))),
                _ => {}
            }
        }
    }
    (if texts.is_empty() { None } else { Some(texts.join("\n")) }, diffs)
}

/// Build an LSF tool call from an ACP `tool_call` (or first `tool_call_update`).
fn acp_tool(u: &Value) -> ToolCall {
    let id = s(u, "toolCallId").unwrap_or("tool").to_string();
    let title = s(u, "title").unwrap_or("").trim().to_string();
    let kind_s = s(u, "kind").unwrap_or("other");
    let kind = kind_of(kind_s, &title);
    let input = normalize_input(u.get("rawInput").cloned().unwrap_or(Value::Null));
    let name = if title.is_empty() { kind_s.to_string() } else { title.split_whitespace().next().unwrap_or(kind_s).trim_matches('`').to_string() };
    let mut t = ToolCall::new(id, name, input);
    t.kind = kind;
    t.status = s(u, "status").map(map_status).unwrap_or(ToolStatus::Pending);
    let described = t.describe();
    t.title = Some(if described.split_whitespace().count() > 1 || title.is_empty() {
        described
    } else {
        format!("{} {}", verb(kind), title.trim_matches('`'))
    });
    t.locations = u["locations"].as_array().map(|a| a.iter().filter_map(|l| s(l, "path").map(|p| p.to_string())).collect()).unwrap_or_default();
    let (output, diffs) = tool_content(u);
    t.output = output;
    t.diffs = diffs;
    if t.diffs.is_empty() {
        t.infer_diffs();
    }
    t
}

fn mime_for(path: &str) -> &'static str {
    let l = path.to_ascii_lowercase();
    if l.ends_with(".jpg") || l.ends_with(".jpeg") {
        "image/jpeg"
    } else if l.ends_with(".gif") {
        "image/gif"
    } else if l.ends_with(".webp") {
        "image/webp"
    } else {
        "image/png"
    }
}

pub fn file_uri(path: &str) -> String {
    let p = path.replace('\\', "/");
    if p.len() > 1 && p.as_bytes()[1] == b':' {
        format!("file:///{p}")
    } else if p.starts_with('/') {
        format!("file://{p}")
    } else {
        format!("file:///{p}")
    }
}

/// Full transcript (minus the new prompt) for agents that cannot reload.
fn full_history(req: &TurnRequest) -> String {
    let mut s = Session::new(AgentKind::Custom("handoff".into()), None);
    let mut msgs: Vec<Message> = req.history.clone();
    msgs.pop();
    s.messages = msgs;
    s.handoff_context(0, 60_000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::Launch;
    use larrier_core::Protocol;

    fn req(run_mode: RunMode) -> TurnRequest {
        TurnRequest {
            agent: AgentKind::Gemini,
            protocol: Protocol::Acp,
            session_id: "s".into(),
            prompt: "fix it".into(),
            attachments: Vec::new(),
            history: vec![Message::user_text("fix it")],
            model: Some("gemini-3-pro-preview".into()),
            mode: None,
            modes: Default::default(),
            cwd: std::env::temp_dir(),
            run_mode,
            resume_id: None,
            resume_path: None,
            resume_at: None,
            handoff: None,
            launch: Launch::default(),
            chat: None,
        }
    }

    /// A scripted fake agent on the other end of the channels.
    async fn fake_agent(mut from_client: mpsc::UnboundedReceiver<Value>, to_client: mpsc::UnboundedSender<Value>) -> Vec<Value> {
        let mut seen = Vec::new();
        while let Some(m) = from_client.recv().await {
            seen.push(m.clone());
            let id = m.get("id").cloned();
            match s(&m, "method") {
                Some("initialize") => {
                    let _ = to_client.send(json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true,"promptCapabilities":{"image":true}},"authMethods":[]}}));
                }
                Some("session/new") => {
                    let _ = to_client.send(json!({"jsonrpc":"2.0","id":id,"result":{"sessionId":"acp-1","configOptions":[{"id":"model","name":"Model","category":"model","type":"select","currentValue":"auto","options":[{"value":"gemini-3-pro-preview","name":"Gemini 3 Pro"}]}]}}));
                }
                Some("session/set_config_option") => {
                    let _ = to_client.send(json!({"jsonrpc":"2.0","id":id,"result":{"configOptions":[]}}));
                }
                Some("session/prompt") => {
                    let upd = |u: Value| json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"acp-1","update":u}});
                    let _ = to_client.send(upd(json!({"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"thinking"}})));
                    let _ = to_client.send(upd(json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"On it."}})));
                    let _ = to_client.send(upd(json!({"sessionUpdate":"tool_call","toolCallId":"t1","title":"Edit src/a.rs","kind":"edit","status":"pending","rawInput":{"file_path":"src/a.rs"}})));
                    // Ask the client to read a file and for permission.
                    let _ = to_client.send(json!({"jsonrpc":"2.0","id":100,"method":"fs/read_text_file","params":{"sessionId":"acp-1","path":"definitely-missing-larrier.txt"}}));
                    let _ = to_client.send(json!({"jsonrpc":"2.0","id":101,"method":"session/request_permission","params":{"sessionId":"acp-1","toolCall":{"toolCallId":"t1","title":"Edit src/a.rs"},"options":[{"optionId":"ok","name":"Allow","kind":"allow_once"},{"optionId":"no","name":"Reject","kind":"reject_once"}]}}));
                    // Wait for the permission answer before finishing.
                    while let Some(r) = from_client.recv().await {
                        seen.push(r.clone());
                        if r.get("id") == Some(&json!(101)) {
                            break;
                        }
                    }
                    let _ = to_client.send(upd(json!({"sessionUpdate":"tool_call_update","toolCallId":"t1","status":"completed","content":[{"type":"diff","path":"src/a.rs","oldText":"a\n","newText":"a\nb\n"}]})));
                    let _ = to_client.send(upd(json!({"sessionUpdate":"tool_call","toolCallId":"t2","title":"Read file","kind":"read","status":"in_progress"})));
                    let _ = to_client.send(upd(json!({"sessionUpdate":"tool_call_update","toolCallId":"t2","title":"Read shot.png","status":"completed","content":[{"type":"content","content":{"type":"image","mimeType":"image/png","data":"iVBORw0KGgo="}}]})));
                    let _ = to_client.send(upd(json!({"sessionUpdate":"plan","entries":[{"content":"step","priority":"high","status":"completed"}]})));
                    let _ = to_client.send(upd(json!({"sessionUpdate":"usage_update","used":10,"size":100,"cost":{"amount":0.5,"currency":"USD"}})));
                    let _ = to_client.send(json!({"jsonrpc":"2.0","id":id,"result":{"stopReason":"end_turn"}}));
                    break;
                }
                _ => {}
            }
        }
        seen
    }

    async fn run_with(run_mode: RunMode, answer: Option<&str>) -> (Vec<AgentEvent>, Vec<Value>) {
        let (out_tx, out_rx) = mpsc::unbounded_channel();
        let (in_tx, in_rx) = mpsc::unbounded_channel();
        let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
        let (ctl_tx, ctl_rx) = mpsc::unbounded_channel();
        let sink = EventSink::new(ev_tx);
        let agent = tokio::spawn(fake_agent(out_rx, in_tx));
        let mut acp = Acp::new(req(run_mode), sink, out_tx, in_rx, ctl_rx);
        let answer = answer.map(|s| s.to_string());
        let flow = async {
            acp.session_flow().await;
        };
        let responder = async {
            // Answer the permission request like the UI would.
            loop {
                tokio::time::sleep(Duration::from_millis(5)).await;
                if let Some(a) = &answer {
                    let _ = ctl_tx.send(TurnCommand::Permission { request_id: "acp-101".into(), option_id: Some(a.clone()) });
                }
                if ctl_tx.is_closed() {
                    break;
                }
            }
        };
        tokio::select! {
            _ = flow => {}
            _ = responder => {}
        }
        drop(acp);
        let seen = agent.await.unwrap();
        let mut events = Vec::new();
        while let Ok(e) = ev_rx.try_recv() {
            if !matches!(e, AgentEvent::Log { .. }) {
                events.push(e);
            }
        }
        (events, seen)
    }

    #[tokio::test]
    async fn full_turn_with_permission() {
        let (ev, seen) = run_with(RunMode::AutoEdit, Some("ok")).await;
        assert!(matches!(&ev[0], AgentEvent::NativeSession { id, .. } if id == "acp-1"));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::ReasoningDelta { .. })));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::TextDelta { text } if text == "On it.")));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolStart { tool } if tool.kind == ToolKind::Edit && tool.title.as_deref() == Some("Edit src/a.rs"))));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Permission { request } if request.options.len() == 2)));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolUpdate { diffs, .. } if diffs.first().map(|d| d.added) == Some(1))));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Plan { .. })));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Usage { usage } if usage.cost_usd == Some(0.5))));
        // A refined title and the image a tool returned.
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolUpdate { id, title: Some(t), .. } if id == "t2" && t.contains("shot.png"))), "{ev:?}");
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolImages { id, images } if id == "t2" && images.len() == 1)));
        assert!(matches!(ev.last(), Some(AgentEvent::Done { ok: true, .. })));
        // The model was selected through the config option, the missing file read failed cleanly,
        // and the permission answer carried the chosen option.
        assert!(seen.iter().any(|m| s(m, "method") == Some("session/set_config_option") && m["params"]["value"] == "gemini-3-pro-preview"));
        assert!(seen.iter().any(|m| m.get("id") == Some(&json!(100)) && m.get("error").is_some()));
        assert!(seen.iter().any(|m| m.get("id") == Some(&json!(101)) && m["result"]["outcome"]["optionId"] == "ok"));
    }

    #[tokio::test]
    async fn full_auto_approves_without_asking() {
        let (ev, seen) = run_with(RunMode::FullAuto, None).await;
        assert!(!ev.iter().any(|e| matches!(e, AgentEvent::Permission { .. })));
        assert!(seen.iter().any(|m| m.get("id") == Some(&json!(101)) && m["result"]["outcome"]["optionId"] == "ok"));
        assert!(matches!(ev.last(), Some(AgentEvent::Done { ok: true, .. })));
    }

    #[test]
    fn uris() {
        assert_eq!(file_uri("D:\\a\\b.txt"), "file:///D:/a/b.txt");
        assert_eq!(file_uri("/home/x"), "file:///home/x");
    }
}
