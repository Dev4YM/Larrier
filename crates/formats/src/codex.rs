//! OpenAI Codex CLI: `~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl`
//! (optionally `.jsonl.zst`). Model content comes from `response_item`
//! records; real user prompts come from `user_message` / `UserMessage`
//! events, because `response_item` user messages also carry injected context.
//!
//! Multi-agent runs: a `spawn_agent` call starts a child thread with its own
//! rollout (`session_meta.source.subagent.thread_spawn.parent_thread_id`);
//! the parent's `SubAgentActivity` items map the spawn `call_id` to the
//! child's `agent_thread_id`, which names the child rollout file.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::{DateTime, Utc};
use larrier_core::lsf::{close_active_tools, find_tool_path, slim_transcript, tool_at_path_mut, MediaSource};
use larrier_core::{AgentKind, FileDiff, Message, NativeLink, Part, Role, Session, ToolCall, ToolKind, ToolStatus, Usage};
use serde_json::{json, Value};

use crate::util::{self, content_text, env_dir, finish, head_lines, json_lines, json_or_string, parse_ts, s, string, trim_output, Conv, Meta};
use crate::NativeSessionInfo;

const AGENT: AgentKind = AgentKind::Codex;

pub fn root(home_override: Option<&Path>) -> PathBuf {
    home_override.map(PathBuf::from).or_else(|| env_dir("CODEX_HOME")).unwrap_or_else(|| util::home().join(".codex"))
}

pub(crate) fn thread_names(root: &Path) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Ok(t) = std::fs::read_to_string(root.join("session_index.jsonl")) {
        for v in json_lines(&t) {
            if let (Some(id), Some(name)) = (s(&v, "id"), s(&v, "thread_name")) {
                if !name.trim().is_empty() {
                    map.insert(id.to_string(), name.to_string());
                }
            }
        }
    }
    map
}

pub fn discover() -> Vec<NativeSessionInfo> {
    let root = root(None);
    let names = thread_names(&root);
    let mut out = Vec::new();
    for sub in ["sessions", "archived_sessions"] {
        let dir = root.join(sub);
        if !dir.exists() {
            continue;
        }
        for e in walkdir::WalkDir::new(&dir).into_iter().flatten() {
            if let Some(info) = file_info(e.path(), &names) {
                out.push(info);
            }
        }
    }
    // A rollout may exist both live and archived; keep the newest.
    out.sort_by_key(|i| std::cmp::Reverse(i.mtime_ms));
    let mut seen = HashSet::new();
    out.retain(|i| seen.insert(i.native_id.clone()));
    out
}

/// Session info for one rollout file; `None` for sub-agent threads (they
/// live in their parent's history) and anything that isn't a rollout.
pub fn file_info(p: &Path, names: &HashMap<String, String>) -> Option<NativeSessionInfo> {
    let name = p.file_name()?.to_string_lossy().to_string();
    if !name.starts_with("rollout-") || !(name.ends_with(".jsonl") || name.ends_with(".jsonl.zst")) {
        return None;
    }
    let first = head_lines(p, 1).into_iter().next()?;
    let v = serde_json::from_str::<Value>(&first).ok()?;
    if s(&v, "type") != Some("session_meta") {
        return None;
    }
    let pl = &v["payload"];
    if pl["source"].get("subagent").is_some() || pl.get("parent_thread_id").map(|x| !x.is_null()).unwrap_or(false) {
        return None;
    }
    let id = s(pl, "id").or_else(|| s(pl, "session_id")).unwrap_or("").to_string();
    if id.is_empty() {
        return None;
    }
    let mut info = util::info(AGENT, id.clone(), p);
    info.cwd = string(pl, "cwd");
    info.title = names.get(&id).cloned();
    Some(info)
}

pub fn import(info: &NativeSessionInfo) -> Result<Session> {
    let text = util::read_text(&info.path)?;
    let mut s = parse(&text, Some(&info.path))?;
    if let Some(t) = info.title.clone() {
        s.title = t.clone();
        s.extra.insert("import_title".into(), Value::String(t));
    }
    Ok(s)
}

fn is_injected(text: &str) -> bool {
    let t = text.trim_start();
    [
        "<environment_context",
        "<user_instructions",
        "<codex_internal",
        "<permissions",
        "<recommended_",
        "<in-app-browser",
        "# AGENTS.md",
        "# In app browser",
        "<turn_aborted",
        "<user_shell_command",
        "<skill",
    ]
    .iter()
    .any(|p| t.starts_with(p))
}

fn norm(t: &str) -> String {
    t.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(400).collect()
}

/// Parse a Codex rollout (legacy or paginated history).
pub fn parse(text: &str, path: Option<&Path>) -> Result<Session> {
    let lines = json_lines(text);
    let mut meta = Meta { path: path.map(PathBuf::from), ..Default::default() };
    // Threads archived in Codex move to `archived_sessions`.
    meta.archived = path.map(|p| p.components().any(|c| c.as_os_str() == "archived_sessions")).unwrap_or(false);
    let (conv, spawns) = convert(&lines, path, &mut meta);
    let mut s = finish(conv, AGENT, meta)?;
    attach_children(&mut s.messages, &spawns, path, 0);
    s.external_turn = turn_open(&lines);
    Ok(s)
}

/// A thread's token totals so far: the last `token_count` in its rollout.
/// `codex exec resume` reports thread totals on `turn.completed`, so this
/// is the baseline a resumed turn's own usage is measured from.
pub fn thread_usage(home: Option<&Path>, thread: &str) -> Option<Usage> {
    let path = rollout_path(&root(home), thread)?;
    let mut lines = util::tail_lines(&path, 4 * 1024 * 1024);
    if !lines.iter().any(|l| l.contains("\"token_count\"")) {
        lines = util::read_text(&path).ok()?.lines().map(String::from).collect();
    }
    lines.iter().rev().filter(|l| l.contains("\"token_count\"")).find_map(|l| {
        let v: Value = serde_json::from_str(l).ok()?;
        token_totals(&v["payload"]["info"]["total_token_usage"])
    })
}

/// Codex token totals (`input_tokens` includes the cached part).
fn token_totals(u: &Value) -> Option<Usage> {
    let n = |k: &str| u[k].as_u64().unwrap_or(0);
    u.is_object().then(|| Usage {
        input_tokens: n("input_tokens").saturating_sub(n("cached_input_tokens")),
        output_tokens: n("output_tokens"),
        cache_read_tokens: n("cached_input_tokens"),
        cache_write_tokens: n("cache_write_input_tokens"),
        reasoning_tokens: n("reasoning_output_tokens"),
        cost_usd: None,
        ..Default::default()
    })
}

/// Whether Codex is still mid-turn: a `task_started` event with no
/// `task_complete` / `turn_aborted` after it.
fn turn_open(lines: &[Value]) -> bool {
    let mut open = false;
    for v in lines {
        if v.get("type").and_then(|t| t.as_str()) != Some("event_msg") {
            continue;
        }
        match v["payload"].get("type").and_then(|t| t.as_str()) {
            Some("task_started") | Some("turn_started") => open = true,
            Some("task_complete") | Some("turn_complete") | Some("turn_aborted") => open = false,
            _ => {}
        }
    }
    open
}

/// A code-mode `exec` call: a JavaScript cell whose commands, MCP calls,
/// patches and image views arrive as their own `item_completed` records
/// and are shown as tools. The script itself is shown only when it ran
/// none of those, or failed.
struct Cell {
    tool: ToolCall,
    children: usize,
    last_child: Option<String>,
    shown: bool,
    answered: bool,
}

/// Convert rollout lines. Returns `(spawn call_id, child thread id)` pairs.
fn convert(lines: &[Value], path: Option<&Path>, meta: &mut Meta) -> (Conv, Vec<(String, String)>) {
    let mut spawns: Vec<(String, String)> = Vec::new();
    let mut spawn_calls: HashSet<String> = HashSet::new();

    // Real user prompts from the event stream.
    let mut event_users: Vec<(String, Option<DateTime<Utc>>, Vec<Part>)> = Vec::new();
    for v in lines {
        let pl = &v["payload"];
        if s(v, "type") != Some("event_msg") {
            continue;
        }
        let ts = s(v, "timestamp").and_then(parse_ts);
        match s(pl, "type") {
            Some("user_message") => {
                let mut parts = Vec::new();
                for img in pl["images"].as_array().into_iter().flatten() {
                    if let Some(u) = img.as_str() {
                        if let Some(p) = util::image_part(None, None, Some(u)) {
                            parts.push(p);
                        }
                    }
                }
                for img in pl["local_images"].as_array().into_iter().flatten() {
                    if let Some(p) = img.as_str().and_then(local_image) {
                        parts.push(p);
                    }
                }
                event_users.push((s(pl, "message").unwrap_or("").to_string(), ts, parts));
            }
            Some("item_completed") if s(&pl["item"], "type") == Some("UserMessage") => {
                let (text, parts) = user_item(&pl["item"]);
                event_users.push((text, ts, parts));
            }
            _ => {}
        }
    }
    let use_events = !event_users.is_empty();
    let mut pending_users: Vec<(String, Option<DateTime<Utc>>, Vec<Part>)> = event_users.clone();
    pending_users.reverse(); // pop from the end in order

    let mut conv = Conv::new(AGENT);
    let mut usage_total: Option<Usage> = None;
    let mut model: Option<String> = None;
    let mut reasoning_gap = false;
    let mut seen_users: HashSet<String> = HashSet::new();
    let codex_root = path.and_then(codex_root_of);

    // Every call id, so items recorded before their call can wait for it.
    let call_ids: HashSet<&str> = lines.iter().filter(|v| s(v, "type") == Some("response_item")).filter_map(|v| s(&v["payload"], "call_id")).collect();
    let mut early: HashMap<String, Value> = HashMap::new();
    let mut cells: HashMap<String, Cell> = HashMap::new();
    let mut cell_order: Vec<String> = Vec::new();
    let mut open_cell: Option<String> = None;
    // Code-mode cell number → `exec` call id, and `wait` call id → `exec` call id.
    let mut cell_ids: HashMap<String, String> = HashMap::new();
    let mut waits: HashMap<String, String> = HashMap::new();
    // Unified-exec session id → `exec_command` call id, and `write_stdin` poll → that call.
    let mut exec_sessions: HashMap<String, String> = HashMap::new();
    let mut polls: HashMap<String, String> = HashMap::new();
    // Subagent task name → `spawn_agent` call id.
    let mut task_tools: HashMap<String, String> = HashMap::new();
    // Generated image id → saved file.
    let mut generated: HashMap<String, String> = HashMap::new();
    let mut goal: Option<(String, String)> = None;

    for v in lines {
        let ts = s(v, "timestamp").and_then(parse_ts);
        let pl = &v["payload"];
        match s(v, "type").unwrap_or("") {
            "session_meta" => {
                meta.native_id = string(pl, "id").or_else(|| string(pl, "session_id")).unwrap_or_default();
                meta.cwd = string(pl, "cwd");
                meta.version = string(pl, "cli_version");
                meta.created = s(pl, "timestamp").and_then(parse_ts);
                meta.branch = string(&pl["git"], "branch");
                meta.remote = string(&pl["git"], "repository_url");
            }
            "turn_context" => {
                if let Some(m) = s(pl, "model") {
                    model = Some(m.to_string());
                }
            }
            "compacted" => compacted(&mut conv, ts),
            "response_item" => match s(pl, "type").unwrap_or("") {
                "message" => {
                    let role = s(pl, "role").unwrap_or("");
                    let text = content_text(&pl["content"]);
                    if role == "assistant" {
                        let (text, actions) = app_directives(&text);
                        conv.text(&text, ts, model.as_deref());
                        if !actions.is_empty() {
                            conv.notice(format!("Git: {}", actions.join(", ")), ts);
                        }
                    } else if role == "user" {
                        if use_events {
                            // Place the matching event prompt here (keeps ordering).
                            let key = norm(&text);
                            let has_image = pl["content"].as_array().map(|a| a.iter().any(|c| s(c, "type") == Some("input_image"))).unwrap_or(false);
                            let pos = pending_users.iter().rposition(|(t, _, parts)| {
                                norm(t) == key || (!t.trim().is_empty() && key.contains(&norm(t))) || (t.trim().is_empty() && !parts.is_empty() && has_image)
                            });
                            if let Some(pos) = pos {
                                let (t, ets, parts) = pending_users.remove(pos);
                                emit_user(&mut conv, &t, ets.or(ts), parts, &mut seen_users);
                            }
                        } else if !is_injected(&text) {
                            emit_user(&mut conv, &text, ts, Vec::new(), &mut seen_users);
                        }
                    }
                }
                "reasoning" => {
                    let summary = pl["summary"]
                        .as_array()
                        .map(|a| a.iter().filter_map(|x| x.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join("\n\n"))
                        .unwrap_or_default();
                    let content = content_text(&pl["content"]);
                    let t = if summary.trim().is_empty() { content } else { summary };
                    if t.trim().is_empty() {
                        reasoning_gap = true;
                    } else {
                        conv.reasoning(&t, None, ts, model.as_deref());
                        reasoning_gap = false;
                    }
                }
                "function_call" | "custom_tool_call" | "local_shell_call" => {
                    let rtype = s(pl, "type").unwrap_or("");
                    let raw = s(pl, "name").unwrap_or(if rtype == "local_shell_call" { "shell" } else { "tool" });
                    let name = qualified_name(s(pl, "namespace").unwrap_or(""), raw);
                    let id = s(pl, "call_id").or_else(|| s(pl, "id")).unwrap_or("").to_string();
                    let mut input = if let Some(a) = s(pl, "arguments") {
                        json_or_string(a)
                    } else if let Some(i) = s(pl, "input") {
                        let v = json_or_string(i);
                        if v.is_string() && matches!(name.as_str(), "exec" | "shell" | "exec_command") {
                            json!({ "command": i })
                        } else {
                            v
                        }
                    } else {
                        pl.get("action").cloned().unwrap_or(Value::Null)
                    };
                    normalize_input(&name, &mut input);
                    if rtype == "custom_tool_call" && name == "exec" {
                        // A script that only runs one literal command reads as that command.
                        let js = s(&input, "command").unwrap_or("").to_string();
                        let mut tool = match script_command(&js) {
                            Some(cmd) => ToolCall::new(&id, "exec", json!({ "command": cmd, "script": js })),
                            None => {
                                let mut t = ToolCall::new(&id, "exec", input);
                                t.title = Some("Ran script".into());
                                t
                            }
                        };
                        tool.started_at = ts;
                        cells.insert(id.clone(), Cell { tool, children: 0, last_child: None, shown: false, answered: false });
                        cell_order.push(id.clone());
                        open_cell = Some(id);
                        continue;
                    }
                    // Polls of a running code cell or command fold into it.
                    if name == "wait" {
                        if let Some(origin) = input.get("cell_id").and_then(id_string).and_then(|c| cell_ids.get(&c)) {
                            waits.insert(id, origin.clone());
                            open_cell = Some(origin.clone());
                            continue;
                        }
                    }
                    if name == "write_stdin" {
                        if let Some(origin) = input.get("session_id").and_then(id_string).and_then(|c| exec_sessions.get(&c)) {
                            polls.insert(id, origin.clone());
                            continue;
                        }
                    }
                    if name == "spawn_agent" {
                        spawn_calls.insert(id.clone());
                        if let Some(task) = s(&input, "task_name") {
                            task_tools.insert(agent_label(task), id.clone());
                        }
                    }
                    let mut tool = ToolCall::new(&id, &name, input);
                    polish(&mut tool);
                    tool.started_at = ts;
                    conv.tool(tool, ts, model.as_deref());
                    if let Some(item) = early.remove(&id) {
                        enrich(&mut conv, &id, &item);
                    }
                }
                "web_search_call" => {
                    let id = s(pl, "id").unwrap_or("");
                    let a = &pl["action"];
                    let mut tool = match s(a, "type") {
                        Some("open_page") => ToolCall::new(id, "web_fetch", json!({ "url": s(a, "url").unwrap_or("") })),
                        Some("find_in_page") => {
                            let pattern = s(a, "pattern").unwrap_or("");
                            let mut t = ToolCall::new(id, "web_fetch", json!({ "url": s(a, "url").unwrap_or(""), "pattern": pattern }));
                            t.title = Some(format!("Searched page for {pattern}"));
                            t
                        }
                        _ => {
                            let q = s(a, "query").map(String::from).unwrap_or_else(|| {
                                a["queries"].as_array().map(|q| q.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join("; ")).unwrap_or_default()
                            });
                            ToolCall::new(id, "web_search", json!({ "query": q }))
                        }
                    };
                    tool.status = if s(pl, "status") == Some("failed") { ToolStatus::Failed } else { ToolStatus::Completed };
                    conv.tool(tool, ts, model.as_deref());
                }
                "image_generation_call" => {
                    let id = s(pl, "id").unwrap_or("");
                    let mut tool = ToolCall::new(id, "image_generation", json!({ "prompt": s(pl, "revised_prompt").unwrap_or("") }));
                    tool.title = Some("Generated an image".into());
                    let saved = generated
                        .get(id)
                        .cloned()
                        .or_else(|| codex_root.as_ref().map(|r| r.join("generated_images").join(&meta.native_id).join(format!("{id}.png")).to_string_lossy().to_string()))
                        .filter(|p| Path::new(p).is_file());
                    let png = Some("image/png".to_string());
                    if let Some(p) = saved {
                        tool.images.push(Part::Image { mime: png, source: MediaSource::Path { path: p } });
                    } else if let Some(b) = s(pl, "result").filter(|b| !b.is_empty()) {
                        tool.images.push(Part::Image { mime: png, source: MediaSource::Base64 { data: b.to_string() } });
                    }
                    tool.status = if s(pl, "status") == Some("failed") { ToolStatus::Failed } else { ToolStatus::Completed };
                    conv.tool(tool, ts, model.as_deref());
                }
                "tool_search_call" => {
                    let id = s(pl, "call_id").or_else(|| s(pl, "id")).unwrap_or("");
                    let q = s(&pl["arguments"], "query").unwrap_or("");
                    let mut tool = ToolCall::new(id, "tool_search", json!({ "query": q }));
                    tool.kind = ToolKind::Other;
                    tool.title = Some(if q.is_empty() { "Searched tools".into() } else { format!("Searched tools for {q}") });
                    conv.tool(tool, ts, model.as_deref());
                }
                "tool_search_output" => {
                    let id = s(pl, "call_id").unwrap_or("");
                    let mut names = Vec::new();
                    for t in pl["tools"].as_array().into_iter().flatten() {
                        let ns = s(t, "name").unwrap_or("");
                        match t["tools"].as_array() {
                            Some(inner) if s(t, "type") == Some("namespace") => names.extend(inner.iter().filter_map(|x| s(x, "name")).map(|n| qualified_name(ns, n))),
                            _ => names.push(ns.to_string()),
                        }
                    }
                    let out = if names.is_empty() { "No tools found".to_string() } else { format!("Loaded {}", names.join(", ")) };
                    conv.tool_result(id, Some(out), s(pl, "status") == Some("failed"), Vec::new());
                }
                "agent_message" => inter_agent(&mut conv, pl, &task_tools, ts),
                "compaction" => compacted(&mut conv, ts),
                "function_call_output" | "custom_tool_call_output" | "local_shell_call_output" => {
                    let id = s(pl, "call_id").unwrap_or("").to_string();
                    let out = tool_output(&pl["output"]);
                    if let Some(origin) = waits.remove(&id) {
                        if let Some(cell) = cells.get_mut(&origin) {
                            cell_poll(&mut conv, &origin, cell, out, ts, model.as_deref());
                        }
                        if open_cell.as_deref() == Some(origin.as_str()) {
                            open_cell = None;
                        }
                    } else if let Some(origin) = polls.remove(&id) {
                        append_output(&mut conv, &origin, out);
                    } else if let Some(cell) = cells.get_mut(&id) {
                        if let Some(n) = &out.cell {
                            cell_ids.insert(n.clone(), id.clone());
                        }
                        cell_output(&mut conv, &id, cell, out, ts, model.as_deref());
                        if open_cell.as_deref() == Some(id.as_str()) {
                            open_cell = None;
                        }
                    } else {
                        if let Some(sid) = &out.session {
                            exec_sessions.insert(sid.clone(), id.clone());
                        }
                        apply_output(&mut conv, &id, out);
                    }
                }
                _ => {}
            },
            "event_msg" => match s(pl, "type").unwrap_or("") {
                "token_count" => {
                    if let Some(mut u) = token_totals(&pl["info"]["total_token_usage"]) {
                        // The latest call's prompt (cached part included) and
                        // the model's window.
                        u.context_tokens = pl["info"]["last_token_usage"]["input_tokens"].as_u64();
                        u.context_window = pl["info"]["model_context_window"].as_u64();
                        usage_total = Some(u);
                    }
                }
                "agent_reasoning" if reasoning_gap => {
                    conv.reasoning(s(pl, "text").unwrap_or(""), None, ts, model.as_deref());
                    reasoning_gap = false;
                }
                "item_completed" => {
                    let item = &pl["item"];
                    match s(item, "type").unwrap_or("") {
                        "Reasoning" if reasoning_gap => {
                            let t = item["summary_text"]
                                .as_array()
                                .map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join("\n\n"))
                                .unwrap_or_default();
                            conv.reasoning(&t, None, ts, model.as_deref());
                            reasoning_gap = false;
                        }
                        "SubAgentActivity" => {
                            // Activity is reported for every multi-agent call;
                            // only the spawn links a call to its child thread.
                            if let (Some(call), Some(thread)) = (s(item, "id"), s(item, "agent_thread_id")) {
                                if spawn_calls.remove(call) {
                                    spawns.push((call.to_string(), thread.to_string()));
                                }
                            }
                        }
                        kind @ ("CommandExecution" | "McpToolCall" | "FileChange" | "ImageView") => {
                            let iid = s(item, "id").unwrap_or("").to_string();
                            if conv.has_tool(&iid) {
                                enrich(&mut conv, &iid, item);
                            } else if call_ids.contains(iid.as_str()) {
                                // Recorded before its call.
                                early.insert(iid, item.clone());
                            } else if kind == "FileChange" && !iid.starts_with("exec-") {
                                // Older rollouts: patch results not tied to a call id.
                                let diffs = file_change_diffs(&item["changes"]);
                                if !diffs.is_empty() {
                                    attach_latest_patch(&mut conv, diffs);
                                }
                            } else if let Some(mut tool) = item_tool(item) {
                                // Run by a code-mode script (possibly one still
                                // running after its call returned).
                                tool.started_at = pl["started_at_ms"].as_i64().and_then(util::ts_from_ms).or(ts);
                                conv.tool(tool, ts, model.as_deref());
                                if let Some(cell) = open_cell.as_ref().or(cell_order.last()).and_then(|c| cells.get_mut(c)) {
                                    cell.children += 1;
                                    cell.last_child = Some(iid);
                                }
                            }
                        }
                        "Extension" => {
                            if let (Some(id), Some(p)) = (s(item, "id"), s(item, "savedPath")) {
                                generated.insert(id.to_string(), p.to_string());
                            }
                        }
                        _ => {}
                    }
                }
                "task_complete" => {
                    if let Some(e) = pl.get("error").filter(|e| !e.is_null()) {
                        let raw = s(e, "message").map(String::from).unwrap_or_else(|| e.to_string());
                        conv.error(&error_message(&raw), ts);
                    }
                }
                "error" => conv.error(&error_message(s(pl, "message").unwrap_or("error")), ts),
                "turn_aborted" => {
                    let note = match s(pl, "reason") {
                        None | Some("interrupted") => "Interrupted".to_string(),
                        Some(r) => format!("Turn aborted ({})", r.replace('_', " ")),
                    };
                    conv.notice(note, ts);
                }
                "thread_settings_applied" => {
                    if let Some(m) = s(&pl["thread_settings"], "model") {
                        model = Some(m.to_string());
                    }
                }
                "thread_goal_updated" => {
                    let g = &pl["goal"];
                    let objective = s(g, "objective").unwrap_or("").trim().to_string();
                    let status = s(g, "status").unwrap_or("").to_string();
                    let first: String = objective.lines().next().unwrap_or("").chars().take(160).collect();
                    let note = match &goal {
                        Some((o, _)) if *o != objective => Some(format!("Goal: {first}")),
                        None if !objective.is_empty() => Some(format!("Goal: {first}")),
                        Some((_, st)) if *st != status && !status.is_empty() => Some(format!("Goal {}", status.replace('_', " "))),
                        _ => None,
                    };
                    if let Some(n) = note {
                        conv.notice(n, ts);
                    }
                    goal = Some((objective, status));
                }
                _ => {}
            },
            _ => {}
        }
    }
    // A code cell still running when the rollout ends (a live turn).
    for id in &cell_order {
        if let Some(cell) = cells.get_mut(id) {
            if !cell.answered && !cell.shown && cell.children == 0 {
                conv.tool(cell.tool.clone(), cell.tool.started_at, model.as_deref());
                cell.shown = true;
            }
        }
    }
    // Event prompts that never matched a response item.
    for (t, ts, parts) in pending_users.into_iter().rev() {
        emit_user(&mut conv, &t, ts, parts, &mut seen_users);
    }
    if conv.model.is_none() {
        conv.model = model;
    }
    if meta.native_id.is_empty() {
        meta.native_id = path.and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    }
    meta.usage = usage_total;
    (conv, spawns)
}

/// Nesting levels of child threads inlined into a parent.
const MAX_CHILD_DEPTH: usize = 3;
/// Parts kept per inlined child transcript.
const MAX_NESTED_PARTS: usize = 400;
/// Child rollouts larger than this are not inlined.
const MAX_CHILD_BYTES: u64 = 48 * 1024 * 1024;

/// Inline child threads into their `spawn_agent` tool calls.
fn attach_children(messages: &mut [Message], spawns: &[(String, String)], path: Option<&Path>, depth: usize) {
    if spawns.is_empty() || depth >= MAX_CHILD_DEPTH {
        return;
    }
    let root = path.and_then(codex_root_of).unwrap_or_else(|| root(None));
    let mut seen = HashSet::new();
    for (call, thread) in spawns {
        if !seen.insert(thread) {
            continue;
        }
        let Some(at) = find_tool_path(messages, call) else { continue };
        if !tool_at_path_mut(messages, &at).map(|t| t.is_subagent() && t.subagent.is_empty()).unwrap_or(false) {
            continue;
        }
        let Some(child) = rollout_path(&root, thread) else { continue };
        let Some((msgs, role)) = child_transcript(&child, depth + 1) else { continue };
        let Some(t) = tool_at_path_mut(messages, &at) else { continue };
        if t.subagent.is_empty() {
            t.subagent = msgs;
            if t.agent_name.is_none() {
                t.agent_name = role;
            }
        }
    }
}

/// A child thread's messages (without the leading prompt) and its role or
/// nickname.
fn child_transcript(path: &Path, depth: usize) -> Option<(Vec<Message>, Option<String>)> {
    if std::fs::metadata(path).map(|m| m.len() > MAX_CHILD_BYTES).unwrap_or(true) {
        return None;
    }
    let text = util::read_text(path).ok()?;
    let lines = json_lines(&text);
    let head = lines.first().filter(|v| s(v, "type") == Some("session_meta")).map(|v| &v["payload"]);
    let spawn = head.map(|pl| &pl["source"]["subagent"]["thread_spawn"]);
    let role = [head.and_then(|pl| s(pl, "agent_role")), spawn.and_then(|t| s(t, "agent_role")), head.and_then(|pl| s(pl, "agent_nickname")), spawn.and_then(|t| s(t, "agent_nickname"))]
        .into_iter()
        .flatten()
        .find(|r| !r.is_empty())
        .map(|r| r.to_string());
    let mut meta = Meta::default();
    let (conv, spawns) = convert(&lines, Some(path), &mut meta);
    let mut msgs = conv.messages;
    attach_children(&mut msgs, &spawns, Some(path), depth);
    let lead = msgs.iter().take_while(|m| m.role == Role::User).count();
    msgs.drain(..lead);
    msgs.retain(|m| !(m.role == Role::Assistant && m.parts.is_empty()));
    for m in msgs.iter_mut() {
        m.sidechain = true;
    }
    close_active_tools(&mut msgs, ToolStatus::Completed);
    slim_transcript(&mut msgs, MAX_NESTED_PARTS);
    (!msgs.is_empty()).then_some((msgs, role))
}

/// `<root>/sessions/YYYY/MM/DD/rollout-….jsonl` → `<root>`.
fn codex_root_of(path: &Path) -> Option<PathBuf> {
    path.ancestors().find(|a| matches!(a.file_name().and_then(|n| n.to_str()), Some("sessions" | "archived_sessions"))).and_then(|a| a.parent()).map(PathBuf::from)
}

/// Thread id → rollout path, rebuilt (from file names only) on a miss.
static ROLLOUTS: Mutex<Option<(PathBuf, Instant, HashMap<String, PathBuf>)>> = Mutex::new(None);

fn rollout_path(root: &Path, thread: &str) -> Option<PathBuf> {
    let mut g = ROLLOUTS.lock().ok()?;
    if let Some((r, built, map)) = g.as_ref() {
        if r == root {
            if let Some(p) = map.get(thread) {
                return Some(p.clone());
            }
            if built.elapsed() < Duration::from_secs(10) {
                return None;
            }
        }
    }
    let mut map = HashMap::new();
    for sub in ["sessions", "archived_sessions"] {
        for e in walkdir::WalkDir::new(root.join(sub)).into_iter().flatten() {
            let name = e.file_name().to_string_lossy();
            let Some(stem) = name.strip_prefix("rollout-").and_then(|n| n.strip_suffix(".jsonl.zst").or_else(|| n.strip_suffix(".jsonl"))) else { continue };
            if stem.len() >= 36 && stem.is_char_boundary(stem.len() - 36) {
                map.entry(stem[stem.len() - 36..].to_string()).or_insert_with(|| e.path().to_path_buf());
            }
        }
    }
    let found = map.get(thread).cloned();
    *g = Some((root.to_path_buf(), Instant::now(), map));
    found
}

fn emit_user(conv: &mut Conv, text: &str, ts: Option<DateTime<Utc>>, mut parts: Vec<Part>, seen: &mut HashSet<String>) {
    if text.trim().is_empty() && parts.is_empty() {
        return;
    }
    let _ = seen;
    // The Codex app sends attached files as a list ahead of the request.
    let (text, files) = mentioned_files(text).unwrap_or_else(|| (text.to_string(), Vec::new()));
    for path in files {
        let have = parts.iter().any(|p| matches!(p, Part::Image { source: MediaSource::Path { path: q }, .. } | Part::File { path: q, .. } if q.replace('\\', "/") == path.replace('\\', "/")));
        if have {
            continue;
        }
        parts.push(match util::image_mime(&path) {
            Some(mime) => Part::Image { mime: Some(mime), source: MediaSource::Path { path } },
            None => Part::File { path, mime: None, text: None },
        });
    }
    parts.insert(0, Part::Text { text });
    conv.user_parts(parts, ts);
}

/// "# Files mentioned by the user:\n\n## shot.png: C:/…/shot.png\n\n## My
/// request for Codex:\nfix it" → ("fix it", ["C:/…/shot.png"]).
fn mentioned_files(text: &str) -> Option<(String, Vec<String>)> {
    let rest = text.trim_start().strip_prefix("# Files mentioned by the user:")?;
    let (list, request) = match rest.find("## My request for Codex:") {
        Some(i) => (&rest[..i], rest[i + "## My request for Codex:".len()..].trim().to_string()),
        None => (rest, String::new()),
    };
    let files = list
        .lines()
        .filter_map(|l| l.trim().strip_prefix("## "))
        .filter_map(|l| l.split_once(": ").map(|(_, p)| p.trim().to_string()))
        .filter(|p| !p.is_empty())
        .collect();
    Some((request, files))
}

/// Codex app directives in a reply (`::git-stage{cwd="…"} ::git-commit{…}
/// ::git-push{… branch="main"}`, `::codex-inline-vis{…}`), which the Codex
/// app turns into widgets: the text without them, and the git actions
/// among them ("stage", "commit", "push").
pub fn app_directives(text: &str) -> (String, Vec<String>) {
    let mut out = String::with_capacity(text.len());
    let mut names = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("::") {
        let after = &rest[i + 2..];
        let name_len = after.find(|c: char| !(c.is_ascii_alphanumeric() || c == '-')).unwrap_or(after.len());
        let name = &after[..name_len];
        let body = &after[name_len..];
        // Directives start a line or follow a space (not `std::fs`).
        let starts_word = rest[..i].chars().last().or_else(|| out.chars().last()).map_or(true, char::is_whitespace);
        match (!starts_word || name.is_empty() || !name.starts_with(|c: char| c.is_ascii_lowercase()), body.starts_with('{').then(|| body.find('}')).flatten()) {
            (false, Some(end)) => {
                out.push_str(&rest[..i]);
                if let Some(action) = name.strip_prefix("git-") {
                    names.push(action.replace('-', " "));
                }
                rest = &body[end + 1..];
            }
            _ => {
                out.push_str(&rest[..i + 2]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    if out.len() == text.len() {
        return (text.to_string(), names);
    }
    (out.trim_end().to_string(), names)
}

/// A tool output with Codex's run header parsed off.
#[derive(Default)]
struct ToolOut {
    text: String,
    is_error: bool,
    images: Vec<Part>,
    wall_ms: Option<u64>,
    /// Still running: a unified-exec session or a code-mode cell to poll.
    running: bool,
    session: Option<String>,
    cell: Option<String>,
}

fn tool_output(v: &Value) -> ToolOut {
    let (text, is_error) = match v {
        Value::String(s) => {
            // custom tool outputs are often JSON {"output": .., "metadata": {"exit_code": ..}}
            match serde_json::from_str::<Value>(s).ok().filter(|o| o.get("output").and_then(|x| x.as_str()).is_some()) {
                Some(o) => (o["output"].as_str().unwrap_or("").to_string(), o["metadata"]["exit_code"].as_i64().unwrap_or(0) != 0),
                None => (s.clone(), false),
            }
        }
        Value::Array(_) => (texts(v), false),
        Value::Object(o) => {
            let out = o.get("content").map(texts).or_else(|| o.get("output").and_then(|x| x.as_str()).map(String::from)).unwrap_or_default();
            (out, o.get("success").and_then(|b| b.as_bool()) == Some(false))
        }
        _ => (String::new(), false),
    };
    let mut out = ToolOut { text, is_error, images: util::image_parts(v), ..Default::default() };
    // A script's output may itself be a command's output (`Script
    // completed … Output:` wrapping `Exit code: 0 … Output:`).
    for level in 0..2 {
        // Several commands' outputs in a row keep their own headers.
        let headers = out.text.lines().filter(|l| ["Exit code:", "Chunk ID:", "Script completed", "Script failed", "Script running"].iter().any(|h| l.starts_with(h))).count();
        if level > 0 && headers != 1 {
            break;
        }
        let Some((head, body)) = split_run_header(&out.text) else { break };
        for line in head.lines().map(str::trim) {
            if let Some(n) = line.strip_prefix("Exit code:").or_else(|| line.strip_prefix("Process exited with code")) {
                out.is_error |= n.trim().parse::<i64>().map(|c| c != 0).unwrap_or(false);
            } else if line == "Script failed" {
                out.is_error = true;
            } else if let Some(n) = line.strip_prefix("Script running with cell ID") {
                out.running = true;
                out.cell = Some(n.trim().to_string());
            } else if let Some(n) = line.strip_prefix("Process running with session ID") {
                out.running = true;
                out.session = Some(n.trim().to_string());
            } else if let Some(w) = line.strip_prefix("Wall time:").or_else(|| line.strip_prefix("Wall time")) {
                if out.wall_ms.is_none() {
                    out.wall_ms = w.trim().trim_end_matches("seconds").trim().parse::<f64>().ok().map(|s| (s * 1000.0) as u64);
                }
            }
        }
        out.text = body;
    }
    // MCP results come back as a serialized content array.
    let t = out.text.trim();
    if t.starts_with("[{") {
        if let Ok(Value::Array(items)) = serde_json::from_str::<Value>(t) {
            if items.iter().all(|i| i.get("type").is_some()) {
                let arr = Value::Array(items);
                out.images.extend(util::image_parts(&arr));
                out.text = texts(&arr);
            }
        }
    }
    // Multi-agent tools report failures as plain text.
    if out.text.starts_with("collab ") && out.text.contains(" failed") {
        out.is_error = true;
    }
    out
}

/// Split Codex's run header (`Exit code: 0`, `Chunk ID: …`, `Script
/// completed`, `Wall time: …`, … up to `Output:`) from a tool output.
fn split_run_header(text: &str) -> Option<(&str, String)> {
    const HEADS: [&str; 6] = ["Exit code:", "Chunk ID:", "Wall time", "Script completed", "Script failed", "Script running with cell ID"];
    if !HEADS.iter().any(|h| text.starts_with(h)) {
        return None;
    }
    if let Some(i) = text.find("\nOutput:") {
        let body = text[i + "\nOutput:".len()..].trim_start_matches(['\n', '\r']);
        return Some((&text[..i], body.to_string()));
    }
    // No `Output:` marker: the header is the leading run of known lines.
    const LINES: [&str; 9] = ["Exit code:", "Chunk ID:", "Wall time", "Script completed", "Script failed", "Script running", "Process exited", "Process running", "Original token count"];
    let mut end = 0;
    for line in text.split_inclusive('\n') {
        if !LINES.iter().any(|h| line.starts_with(h)) {
            break;
        }
        end += line.len();
    }
    Some((&text[..end], text[end..].trim_start_matches(['\n', '\r']).to_string()))
}

/// The command of a code-mode script whose only tool call is
/// `tools.exec_command({cmd: "…"})` or `tools.shell_command({command: "…"})`.
fn script_command(js: &str) -> Option<String> {
    if js.matches("tools.").count() != 1 {
        return None;
    }
    let (at, key) = [("tools.exec_command(", "cmd"), ("tools.shell_command(", "command")].into_iter().find_map(|(call, key)| js.find(call).map(|i| (i + call.len(), key)))?;
    let args = &js[at..];
    // `cmd: "…"` or `"cmd": "…"`.
    let k = args.find(key)? + key.len();
    let rest = args[k..].trim_start_matches(['"', '\'']).trim_start();
    let cmd = unwrap_shell(&js_string(rest.strip_prefix(':')?.trim_start())?);
    (!cmd.is_empty()).then_some(cmd)
}

/// A JavaScript string literal at the start of `s`, unescaped.
fn js_string(s: &str) -> Option<String> {
    let mut chars = s.chars();
    let q = chars.next().filter(|c| matches!(c, '"' | '\'' | '`'))?;
    let mut out = String::new();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => {}
                other => out.push(other),
            },
            '$' if q == '`' && chars.clone().next() == Some('{') => return None, // template with interpolation
            c if c == q => return Some(out),
            c => out.push(c),
        }
    }
    None
}

/// Text of a content array without image placeholders (images are kept as
/// parts).
fn texts(v: &Value) -> String {
    match v.as_array() {
        Some(a) => content_text(&Value::Array(a.iter().filter(|i| !matches!(s(i, "type"), Some("image" | "input_image" | "image_url"))).cloned().collect())),
        None => content_text(v),
    }
}

/// Attach a call's output (status, duration, images).
fn apply_output(conv: &mut Conv, id: &str, out: ToolOut) {
    if !conv.tool_result(id, Some(out.text), out.is_error, Vec::new()) {
        return;
    }
    conv.tool_images(id, out.images);
    if let Some(t) = conv.tool_mut(id) {
        if out.running && !t.is_error {
            t.status = ToolStatus::Running;
        }
        if t.duration_ms.is_none() {
            t.duration_ms = out.wall_ms;
        }
    }
}

/// Fold a later poll (`write_stdin`, code-mode `wait`) into the call that
/// started the work.
fn append_output(conv: &mut Conv, id: &str, out: ToolOut) {
    let Some(t) = conv.tool_mut(id) else { return };
    let text = out.text.trim_end();
    if !text.is_empty() {
        let joined = match t.output.as_deref().filter(|o| !o.trim().is_empty()) {
            Some(prev) => format!("{}\n{text}", prev.trim_end()),
            None => text.to_string(),
        };
        t.output = Some(trim_output(&joined));
    }
    t.is_error |= out.is_error;
    t.status = if t.is_error {
        ToolStatus::Failed
    } else if out.running {
        ToolStatus::Running
    } else {
        ToolStatus::Completed
    };
    if let Some(w) = out.wall_ms {
        t.duration_ms = Some(t.duration_ms.unwrap_or(0) + w);
    }
    t.images.extend(out.images);
}

/// A code cell's own output: show the script when it ran nothing visible
/// or failed; otherwise keep only the images it returned.
fn cell_output(conv: &mut Conv, id: &str, cell: &mut Cell, out: ToolOut, ts: Option<DateTime<Utc>>, model: Option<&str>) {
    cell.answered = true;
    if !cell.shown && (cell.children == 0 || out.is_error) {
        conv.tool(cell.tool.clone(), ts, model);
        cell.shown = true;
    }
    if cell.shown {
        apply_output(conv, id, out);
    } else {
        child_images(conv, cell, out.images);
    }
}

/// A code-mode `wait` on a running cell.
fn cell_poll(conv: &mut Conv, id: &str, cell: &mut Cell, out: ToolOut, ts: Option<DateTime<Utc>>, model: Option<&str>) {
    if !cell.shown && out.is_error {
        conv.tool(cell.tool.clone(), ts, model);
        cell.shown = true;
        apply_output(conv, id, out);
    } else if cell.shown {
        append_output(conv, id, out);
    } else {
        child_images(conv, cell, out.images);
    }
}

/// Images a script returned go to the last tool it ran, unless that tool
/// already has them (an image view).
fn child_images(conv: &mut Conv, cell: &Cell, images: Vec<Part>) {
    if let Some(child) = cell.last_child.as_deref() {
        if conv.tool_mut(child).map(|t| t.images.is_empty()).unwrap_or(false) {
            conv.tool_images(child, images);
        }
    }
}

/// Details from an `item_completed` record for a call already shown.
fn enrich(conv: &mut Conv, id: &str, item: &Value) {
    let Some(t) = conv.tool_mut(id) else { return };
    if t.duration_ms.is_none() {
        t.duration_ms = duration_ms(&item["duration"]);
    }
    match s(item, "type") {
        Some("FileChange") => merge_diffs(t, file_change_diffs(&item["changes"])),
        Some("McpToolCall") => {
            let (_, images, failed) = mcp_result(item);
            if t.images.is_empty() {
                t.images = images;
            }
            if failed {
                t.is_error = true;
                t.status = ToolStatus::Failed;
            }
        }
        _ => {}
    }
}

/// A tool run by a code-mode script, from its `item_completed` record.
fn item_tool(item: &Value) -> Option<ToolCall> {
    let id = s(item, "id")?;
    let status = s(item, "status").unwrap_or("completed");
    let mut t = match s(item, "type")? {
        "CommandExecution" => {
            let command = match &item["command"] {
                Value::Array(a) => unwrap_argv(a),
                Value::String(c) => unwrap_shell(c),
                _ => String::new(),
            };
            let mut t = ToolCall::new(id, "shell", json!({ "command": command }));
            explore_kind(&mut t, &item["parsed_cmd"]);
            let mut out = s(item, "aggregated_output").or_else(|| s(item, "formatted_output")).unwrap_or("").to_string();
            let exit = item["exit_code"].as_i64();
            if let Some(code) = exit.filter(|c| *c != 0) {
                out.push_str(&format!("\n[exit code {code}]"));
            }
            t.output = Some(trim_output(&out));
            t.is_error = status == "failed" || exit.unwrap_or(0) != 0;
            t.status = match status {
                "declined" => ToolStatus::Cancelled,
                "in_progress" => ToolStatus::Running,
                _ if t.is_error => ToolStatus::Failed,
                _ => ToolStatus::Completed,
            };
            t
        }
        "McpToolCall" => {
            let name = format!("mcp__{}__{}", s(item, "server").unwrap_or("mcp"), s(item, "tool").unwrap_or("tool"));
            let mut t = ToolCall::new(id, &name, item["arguments"].clone());
            polish(&mut t);
            let (out, images, failed) = mcp_result(item);
            t.output = Some(trim_output(&out));
            t.images = images;
            t.is_error = failed;
            t.status = if failed { ToolStatus::Failed } else { ToolStatus::Completed };
            t
        }
        "FileChange" => {
            let diffs = file_change_diffs(&item["changes"]);
            let kinds: Vec<&str> = item["changes"].as_object().map(|m| m.values().filter_map(|c| s(c, "type")).collect()).unwrap_or_default();
            let first = diffs.first().map(|d| d.path.clone()).unwrap_or_default();
            let mut t = ToolCall::new(id, "apply_patch", json!({ "file_path": first }));
            t.kind = match kinds.as_slice() {
                ["add"] => ToolKind::Write,
                ["delete"] => ToolKind::Delete,
                _ => ToolKind::Edit,
            };
            t.title = Some(if diffs.len() > 1 { format!("Edit {} files", diffs.len()) } else { t.describe() });
            t.locations = diffs.iter().map(|d| d.path.clone()).collect();
            t.diffs = diffs;
            t.output = s(item, "stdout").or_else(|| s(item, "stderr")).filter(|o| !o.is_empty()).map(trim_output);
            t.is_error = status == "failed";
            t.status = if t.is_error { ToolStatus::Failed } else { ToolStatus::Completed };
            t
        }
        "ImageView" => {
            let path = util::file_url_path(s(item, "path")?);
            let mut t = ToolCall::new(id, "view_image", json!({ "path": path }));
            polish(&mut t);
            if Path::new(&path).is_file() {
                t.images.push(Part::Image { mime: util::image_mime(&path), source: MediaSource::Path { path } });
            }
            t.status = ToolStatus::Completed;
            t
        }
        _ => return None,
    };
    t.duration_ms = duration_ms(&item["duration"]);
    Some(t)
}

/// Codex classifies simple commands (`parsed_cmd`): show a lone read,
/// listing or search as such instead of a raw shell command.
fn explore_kind(t: &mut ToolCall, parsed: &Value) {
    let Some(list) = parsed.as_array().filter(|a| !a.is_empty()) else { return };
    let kinds: HashSet<&str> = list.iter().filter_map(|p| s(p, "type")).collect();
    if kinds.len() != 1 || list.len() != kinds.len() && !kinds.contains("read") {
        return;
    }
    let first = &list[0];
    let path = s(first, "path").or_else(|| s(first, "name")).unwrap_or("");
    match kinds.into_iter().next() {
        Some("read") => {
            t.kind = ToolKind::Read;
            t.input["path"] = json!(path);
            let names: Vec<&str> = list.iter().filter_map(|p| s(p, "name").or_else(|| s(p, "path"))).collect();
            t.title = Some(format!("Read {}", names.join(", ")));
        }
        Some("list_files") => {
            t.kind = ToolKind::Read;
            t.input["path"] = json!(path);
            t.title = Some(if path.is_empty() { "Listed files".into() } else { format!("Listed {path}") });
        }
        Some("search") => {
            t.kind = ToolKind::Search;
            if let Some(q) = s(first, "query") {
                t.input["query"] = json!(q);
            }
            if !path.is_empty() {
                t.input["path"] = json!(path);
            }
            t.title = Some(t.describe());
        }
        _ => {}
    }
}

/// `(output, images, failed)` of an `McpToolCall` item (current
/// `{content, isError}` results and older `{Ok|Err}` ones).
fn mcp_result(item: &Value) -> (String, Vec<Part>, bool) {
    let res = &item["result"];
    let ok = res.get("Ok").unwrap_or(res);
    let content = &ok["content"];
    let err = item
        .get("error")
        .filter(|e| !e.is_null())
        .map(|e| s(e, "message").map(String::from).unwrap_or_else(|| e.to_string()))
        .or_else(|| res.get("Err").map(|e| e.as_str().map(String::from).unwrap_or_else(|| e.to_string())));
    let failed = err.is_some() || ok["isError"].as_bool() == Some(true) || s(item, "status") == Some("failed");
    (err.unwrap_or_else(|| texts(content)), util::image_parts(content), failed)
}

/// `{"secs": 1, "nanos": 500000000}` → 1500 ms.
fn duration_ms(v: &Value) -> Option<u64> {
    let secs = v.get("secs")?.as_u64()?;
    Some(secs * 1000 + v.get("nanos").and_then(|n| n.as_u64()).unwrap_or(0) / 1_000_000)
}

/// Prompt text and attached images of a `UserMessage` item.
fn user_item(item: &Value) -> (String, Vec<Part>) {
    let mut text = Vec::new();
    let mut parts = Vec::new();
    for c in item["content"].as_array().into_iter().flatten() {
        match s(c, "type") {
            Some("text") => text.extend(s(c, "text")),
            Some("local_image") => parts.extend(s(c, "path").and_then(local_image)),
            Some("image") => parts.extend(util::image_part(None, None, s(c, "image_url").or_else(|| s(c, "url")))),
            _ => {}
        }
    }
    (text.join("\n"), parts)
}

/// A pasted image saved to disk (dropped when the file is gone).
fn local_image(path: &str) -> Option<Part> {
    let path = util::file_url_path(path);
    Path::new(&path).is_file().then(|| Part::Image { mime: util::image_mime(&path), source: MediaSource::Path { path } })
}

/// Tool name for a namespaced call: `mcp__server__tool` for MCP namespaces
/// (`mcp__node_repl__` + `js`, `mcp__codex_apps__github` + `_fetch`), the
/// bare name for built-in ones (`collaboration`, `multi_agent_v1`, …).
fn qualified_name(ns: &str, name: &str) -> String {
    if ns.starts_with("mcp__") {
        format!("{}__{}", ns.trim_end_matches('_'), name.trim_start_matches('_'))
    } else if ns == "web" && name == "run" {
        "web_search".into()
    } else {
        name.to_string()
    }
}

fn normalize_input(name: &str, input: &mut Value) {
    let Some(obj) = input.as_object_mut() else { return };
    // Legacy `shell` / `local_shell_call`: an argv vector in a login shell.
    if let Some(argv) = obj.get("command").and_then(|c| c.as_array()).cloned() {
        obj.insert("command".into(), json!(unwrap_argv(&argv)));
    }
    // `web.run` searches.
    if name == "web_search" {
        if let Some(qs) = obj.get("search_query").and_then(|q| q.as_array()) {
            let q = qs.iter().filter_map(|x| s(x, "q")).collect::<Vec<_>>().join("; ");
            *input = json!({ "query": q });
            return;
        }
    }
    // Multi-agent messages are encrypted for the model; nothing to show.
    if obj.get("message").and_then(|m| m.as_str()).map(is_encrypted).unwrap_or(false) {
        obj.remove("message");
    }
}

/// Titles and kinds for Codex tools the generic classifier gets wrong.
fn polish(t: &mut ToolCall) {
    let get = |k: &str| t.input.get(k).and_then(|v| v.as_str()).map(String::from);
    let agent = get("target").or_else(|| get("id")).map(|a| agent_label(&a)).unwrap_or_else(|| "subagent".into());
    let title = match t.name.as_str() {
        "view_image" => {
            t.kind = ToolKind::Read;
            Some(t.describe())
        }
        "write_stdin" => {
            t.kind = ToolKind::Shell;
            Some("Wrote to a running command".into())
        }
        "request_user_input" | "request_user_input_async" => {
            let q = &t.input["questions"][0];
            let text = s(q, "question").or_else(|| s(q, "title")).or_else(|| s(q, "header")).unwrap_or("");
            Some(if text.is_empty() { "Asked a question".into() } else { format!("Asked: {}", text.lines().next().unwrap_or("")) })
        }
        "update_goal" => Some(format!("Marked goal {}", get("status").unwrap_or_else(|| "updated".into()).replace('_', " "))),
        "sleep" => Some(match t.input["duration_ms"].as_u64() {
            Some(ms) => format!("Waited {}s", (ms + 500) / 1000),
            None => "Waited".into(),
        }),
        "list_mcp_resources" | "list_mcp_resource_templates" => Some("Listed MCP resources".into()),
        "send_message" => Some(format!("Messaged {agent}")),
        "followup_task" => Some(format!("Sent a follow-up to {agent}")),
        "interrupt_agent" => Some(format!("Interrupted {agent}")),
        "close_agent" => Some(format!("Closed {agent}")),
        "resume_agent" => Some(format!("Resumed {agent}")),
        "wait" | "wait_agent" => Some("Waited for subagents".into()),
        "list_agents" => Some("Listed subagents".into()),
        "read_thread_terminal" | "load_workspace_dependencies" | "automation_update" => {
            t.kind = ToolKind::Other;
            Some(t.name.replace('_', " "))
        }
        // `node_repl` / `cua_repl` scripts carry a display title.
        _ if t.kind == ToolKind::Mcp => get("title").filter(|x| !x.trim().is_empty()),
        _ => None,
    };
    if let Some(x) = title {
        t.title = Some(x);
    }
}

/// `/root/backend_audit` → `backend_audit`.
fn agent_label(path: &str) -> String {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or(path).to_string()
}

/// Fernet tokens (`gAAAAA…`) carry encrypted multi-agent messages.
fn is_encrypted(s: &str) -> bool {
    s.starts_with("gAAAAA") && !s.contains(char::is_whitespace)
}

fn id_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// A message between agents (`Message Type: …`, `Sender: …`, `Payload:`).
/// A subagent's final answer becomes its spawn call's result.
fn inter_agent(conv: &mut Conv, pl: &Value, task_tools: &HashMap<String, String>, ts: Option<DateTime<Utc>>) {
    let text = content_text(&pl["content"]);
    let (head, payload) = text.split_once("Payload:").unwrap_or((text.as_str(), ""));
    let field = |k: &str| head.lines().find_map(|l| l.strip_prefix(k)).map(|v| v.trim().to_string());
    let sender = field("Sender:").or_else(|| string(pl, "author")).unwrap_or_default();
    let who = agent_label(&sender);
    let payload = payload.trim();
    if field("Message Type:").as_deref() == Some("FINAL_ANSWER") {
        if let Some(spawn) = task_tools.get(&who).filter(|_| !payload.is_empty()) {
            if conv.tool_result(spawn, Some(payload.to_string()), false, Vec::new()) {
                return;
            }
        }
    }
    let first: String = payload.lines().find(|l| !l.trim().is_empty()).unwrap_or("").chars().take(160).collect();
    conv.notice(if first.is_empty() { format!("Message from {who}") } else { format!("{who}: {first}") }, ts);
}

/// One notice per compaction (the same event is recorded several ways).
fn compacted(conv: &mut Conv, ts: Option<DateTime<Utc>>) {
    let last = conv.messages.last().and_then(|m| m.parts.last());
    if !matches!(last, Some(Part::Notice { text }) if text == "Context compacted") {
        conv.notice("Context compacted", ts);
    }
}

/// Codex reports API failures as a JSON string; the human message inside.
pub fn error_message(raw: &str) -> String {
    serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|j| j["error"]["message"].as_str().or(j["message"].as_str()).map(String::from))
        .unwrap_or_else(|| raw.to_string())
}

/// The command inside Codex's shell wrappers: `bash -lc '…'`,
/// `"C:\…\cmd.exe" /c 'cmd.exe /d /c …'`, `pwsh -Command "…"`.
pub fn unwrap_shell(cmd: &str) -> String {
    let mut cur = cmd.trim().to_string();
    for _ in 0..4 {
        match strip_wrapper(&cur) {
            Some(inner) if !inner.trim().is_empty() => cur = inner.trim().to_string(),
            _ => break,
        }
    }
    // Codex prefixes PowerShell scripts with a UTF-8 console setup line.
    if cur.starts_with("try { [Console]::OutputEncoding") {
        if let Some((_, rest)) = cur.split_once('\n') {
            cur = rest.trim().to_string();
        }
    }
    cur
}

/// [`unwrap_shell`] for an argv vector.
pub fn unwrap_argv(argv: &[Value]) -> String {
    let joined = argv
        .iter()
        .filter_map(|a| a.as_str())
        .map(|a| if a.is_empty() || a.contains(char::is_whitespace) || a.contains('\'') { format!("'{}'", a.replace('\'', "'\\''")) } else { a.to_string() })
        .collect::<Vec<_>>()
        .join(" ");
    unwrap_shell(&joined)
}

fn strip_wrapper(s: &str) -> Option<String> {
    let (prog, mut rest) = first_token(s)?;
    let base = prog.rsplit(['/', '\\']).next().unwrap_or(&prog).to_ascii_lowercase();
    let base = base.strip_suffix(".exe").unwrap_or(&base);
    loop {
        let (tok, after) = first_token(rest)?;
        let lower = tok.to_ascii_lowercase();
        match base {
            "bash" | "sh" | "zsh" | "dash" | "ksh" => {
                if !tok.starts_with('-') {
                    return None;
                }
                if !tok.starts_with("--") && tok[1..].contains('c') {
                    return Some(unquote(after));
                }
            }
            "cmd" => {
                if !tok.starts_with('/') {
                    return None;
                }
                if lower == "/c" || lower == "/k" {
                    return Some(unquote(after));
                }
            }
            "pwsh" | "powershell" => {
                if !tok.starts_with('-') {
                    return None;
                }
                if lower == "-command" || lower == "-c" {
                    return Some(unquote(after));
                }
                if lower == "-file" || lower == "-encodedcommand" {
                    return None;
                }
                if lower == "-executionpolicy" || lower == "-workingdirectory" {
                    rest = first_token(after)?.1;
                    continue;
                }
            }
            _ => return None,
        }
        rest = after;
    }
}

/// The first shell word (quotes removed) and the rest of the line.
fn first_token(s: &str) -> Option<(String, &str)> {
    let s = s.trim_start();
    let mut chars = s.char_indices();
    let (_, c0) = chars.next()?;
    if c0 == '"' || c0 == '\'' {
        let mut prev = '\0';
        for (i, c) in chars {
            if c == c0 && !(c0 == '"' && prev == '\\') {
                return Some((s[1..i].to_string(), &s[i + 1..]));
            }
            prev = c;
        }
        return Some((s[1..].to_string(), ""));
    }
    let end = s.find(char::is_whitespace).unwrap_or(s.len());
    Some((s[..end].to_string(), &s[end..]))
}

/// Remove one level of quoting around a whole script argument.
fn unquote(s: &str) -> String {
    let t = s.trim();
    if t.len() >= 2 && t.starts_with('\'') && t.ends_with('\'') {
        t[1..t.len() - 1].replace("'\\''", "'")
    } else if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        t[1..t.len() - 1].replace("\\\"", "\"")
    } else {
        t.to_string()
    }
}

fn file_change_diffs(changes: &Value) -> Vec<FileDiff> {
    let mut out = Vec::new();
    if let Some(map) = changes.as_object() {
        for (path, c) in map {
            let kind = c.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if let Some(d) = c.get("unified_diff").and_then(|d| d.as_str()) {
                out.push(FileDiff::from_unified(path.clone(), d.to_string()));
            } else if kind == "add" {
                out.push(FileDiff::from_texts(path.clone(), None, c.get("content").and_then(|x| x.as_str()).map(String::from)));
            } else if kind == "delete" {
                out.push(FileDiff::from_texts(path.clone(), c.get("content").and_then(|x| x.as_str()).map(String::from), None));
            }
        }
    } else if let Some(arr) = changes.as_array() {
        for c in arr {
            if let Some(p) = c.get("path").and_then(|p| p.as_str()) {
                out.push(FileDiff { path: p.to_string(), ..Default::default() });
            }
        }
    }
    out
}

/// Keep patch-derived counts; only fill paths the patch lacked.
fn merge_diffs(t: &mut ToolCall, diffs: Vec<FileDiff>) {
    for d in diffs {
        if !t.diffs.iter().any(|x| x.path == d.path || d.path.ends_with(&x.path)) {
            if !t.locations.contains(&d.path) {
                t.locations.push(d.path.clone());
            }
            t.diffs.push(d);
        }
    }
}

fn attach_latest_patch(conv: &mut Conv, diffs: Vec<FileDiff>) {
    for m in conv.messages.iter_mut().rev() {
        if m.role != Role::Assistant {
            break;
        }
        for p in m.parts.iter_mut().rev() {
            if let Part::Tool(t) = p {
                if t.kind.is_mutation() {
                    merge_diffs(t, diffs);
                    return;
                }
            }
        }
    }
}

// ---------------------------------------------------------------- export

pub fn export_lines(session: &Session, id: &str, now: DateTime<Utc>) -> String {
    let cwd = util::session_cwd(session);
    let ts = |t: DateTime<Utc>| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let mut out = String::new();
    let mut push = |v: Value| {
        out.push_str(&v.to_string());
        out.push('\n');
    };
    let mut git = json!({});
    if let Some(w) = &session.workspace {
        if let Some(b) = &w.branch {
            git["branch"] = json!(b);
        }
        if let Some(r) = &w.remote {
            git["repository_url"] = json!(r);
        }
    }
    push(json!({
        "timestamp": ts(now),
        "type": "session_meta",
        "payload": {
            "id": id,
            "session_id": id,
            "timestamp": ts(session.created_at),
            "cwd": cwd,
            "originator": "codex_cli_rs",
            "cli_version": "0.154.0",
            "source": "cli",
            "model_provider": "openai",
            "git": git,
        }
    }));
    for (role, text, t, _) in util::turns(session) {
        match role {
            Role::User => {
                push(json!({"timestamp": ts(t), "type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": text}]}}));
                push(json!({"timestamp": ts(t), "type": "event_msg", "payload": {"type": "user_message", "message": text, "images": []}}));
            }
            _ => {
                push(json!({"timestamp": ts(t), "type": "response_item", "payload": {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": text}]}}));
                push(json!({"timestamp": ts(t), "type": "event_msg", "payload": {"type": "agent_message", "message": text}}));
            }
        }
    }
    out
}

pub fn export_native(session: &Session, home_override: Option<&Path>) -> Result<NativeLink> {
    let root = root(home_override);
    let now = Utc::now();
    let id = uuid::Uuid::now_v7().to_string();
    let local = now.with_timezone(&chrono::Local);
    let dir = root.join("sessions").join(local.format("%Y").to_string()).join(local.format("%m").to_string()).join(local.format("%d").to_string());
    let path = dir.join(format!("rollout-{}-{id}.jsonl", local.format("%Y-%m-%dT%H-%M-%S")));
    util::write_new_file(&path, &export_lines(session, &id, now))?;
    // Give the thread its title in Codex's picker (append-only index).
    let index = root.join("session_index.jsonl");
    if index.exists() {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&index) {
            let line = json!({"id": id, "thread_name": session.title, "updated_at": now.to_rfc3339()});
            let _ = writeln!(f, "{line}");
        }
    }
    Ok(NativeLink { agent: AGENT, id, path: Some(path.to_string_lossy().to_string()), synced_messages: session.messages.len(), account_id: None, resume_at: None })
}

#[cfg(test)]
mod tests {
    #[test]
    fn running_turn_until_task_complete() {
        let open = r#"{"timestamp":"2026-10-29T10:00:00Z","type":"session_meta","payload":{"id":"T","cwd":"/r"}}
{"timestamp":"2026-10-29T10:00:01Z","type":"event_msg","payload":{"type":"task_started"}}
{"timestamp":"2026-10-29T10:00:02Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"fix it"}]}}"#;
        assert!(super::turn_open(&crate::util::json_lines(open)));
        let done = format!("{open}
{}", r#"{"timestamp":"2026-10-29T10:00:09Z","type":"event_msg","payload":{"type":"task_complete"}}"#);
        assert!(!super::turn_open(&crate::util::json_lines(&done)));
    }

    use super::*;

    fn rollout(body: &[&str]) -> Session {
        let mut lines = vec![
            r#"{"timestamp":"2026-10-29T10:00:00Z","type":"session_meta","payload":{"id":"T","cwd":"C:\\proj","cli_version":"0.154.0"}}"#,
            r#"{"timestamp":"2026-10-29T10:00:01Z","type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","content":[{"type":"text","text":"go"}]}}}"#,
            r#"{"timestamp":"2026-10-29T10:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"go"}]}}"#,
        ];
        lines.extend_from_slice(body);
        parse(&lines.join("\n"), None).unwrap()
    }

    fn tools(s: &Session) -> Vec<&ToolCall> {
        s.messages.iter().flat_map(|m| m.tool_calls()).collect()
    }

    #[test]
    fn thread_usage_reads_last_total() {
        let dir = tempfile::tempdir().unwrap();
        let day = dir.path().join("sessions").join("2026").join("10").join("29");
        std::fs::create_dir_all(&day).unwrap();
        let id = "01a00000-0000-7000-8000-00000000abcd";
        let rollout = [
            format!(r#"{{"type":"session_meta","payload":{{"id":"{id}"}}}}"#),
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":7}}}}"#.to_string(),
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":300,"cached_input_tokens":200,"output_tokens":9,"reasoning_output_tokens":3}}}}"#.to_string(),
            r#"{"type":"event_msg","payload":{"type":"token_count","info":null}}"#.to_string(),
        ];
        std::fs::write(day.join(format!("rollout-2026-10-29T10-00-00-{id}.jsonl")), rollout.join("\n")).unwrap();
        let u = thread_usage(Some(dir.path()), id).unwrap();
        assert_eq!((u.input_tokens, u.cache_read_tokens, u.output_tokens, u.reasoning_tokens), (100, 200, 9, 3));
        assert!(thread_usage(Some(dir.path()), "01a00000-0000-7000-8000-00000000ffff").is_none());
    }

    #[test]
    fn codex_app_files_and_directives() {
        let (req, files) = mentioned_files("# Files mentioned by the user:\n\n## shot.png: C:/tmp/shot.png\n\n## notes.md: D:/repo/notes.md\n\n## My request for Codex:\nwhat is wrong?\n").unwrap();
        assert_eq!(req, "what is wrong?");
        assert_eq!(files, ["C:/tmp/shot.png", "D:/repo/notes.md"]);
        assert!(mentioned_files("plain prompt").is_none());

        let (text, names) = app_directives("Pushed.\n\n::git-stage{cwd=\"D:\\r\"} ::git-commit{cwd=\"D:\\r\"} ::git-push{cwd=\"D:\\r\" branch=\"main\"}");
        assert_eq!(text, "Pushed.");
        assert_eq!(names, ["stage", "commit", "push"]);
        // Other widgets go without a notice.
        assert_eq!(app_directives("Here it is.\n::codex-inline-vis{path=\"a.html\"}"), ("Here it is.".to_string(), Vec::new()));
        // Rust paths and C++ scopes stay as they are.
        assert_eq!(app_directives("use std::fs; a::b{}").0, "use std::fs; a::b{}");
    }

    #[test]
    fn unwraps_shell_wrappers() {
        assert_eq!(unwrap_shell("bash -lc 'ls -la'"), "ls -la");
        assert_eq!(unwrap_shell("/bin/zsh -lc \"echo 'hi'\""), "echo 'hi'");
        assert_eq!(unwrap_shell(r#""C:\\WINDOWS\\system32\\cmd.exe" /c 'cmd.exe /d /c type notes.txt'"#), "type notes.txt");
        assert_eq!(
            unwrap_shell("'C:\\Program Files\\PowerShell\\7\\pwsh.exe' -NoProfile -Command 'try { [Console]::OutputEncoding=[System.Text.Encoding]::UTF8 } catch {}\nGet-ChildItem -Name'"),
            "Get-ChildItem -Name"
        );
        assert_eq!(unwrap_shell("git status"), "git status");
        assert_eq!(unwrap_argv(&[json!("bash"), json!("-lc"), json!("echo 'a b'")]), "echo 'a b'");
        assert_eq!(unwrap_argv(&[json!("C:\\WINDOWS\\system32\\cmd.exe"), json!("/c"), json!("cmd.exe /d /c exit 3")]), "exit 3");
    }

    /// Code mode: an `exec` script's commands and patches are the tools; the
    /// script shows only when it ran nothing or failed.
    #[test]
    fn code_mode_cells() {
        let s = rollout(&[
            // A script that ran one command (a plain read) and one patch.
            r#"{"timestamp":"2026-10-29T10:00:02Z","type":"response_item","payload":{"type":"custom_tool_call","status":"completed","call_id":"c1","name":"exec","input":"const r = await tools.exec_command({cmd:\"type notes.txt\"});"}}"#,
            r#"{"timestamp":"2026-10-29T10:00:03Z","type":"event_msg","payload":{"type":"item_completed","item":{"type":"CommandExecution","id":"exec-1","command":["C:\\WINDOWS\\system32\\cmd.exe","/c","cmd.exe /d /c type notes.txt"],"parsed_cmd":[{"type":"read","cmd":"type notes.txt","name":"notes.txt","path":"notes.txt"}],"status":"completed","aggregated_output":"alpha line\nbeta line","exit_code":0,"duration":{"secs":0,"nanos":157400000}}}}"#,
            r#"{"timestamp":"2026-10-29T10:00:04Z","type":"event_msg","payload":{"type":"item_completed","item":{"type":"FileChange","id":"exec-2","changes":{"C:\\proj\\out.txt":{"type":"add","content":"ok\n"}},"status":"completed","stdout":"Success."}}}"#,
            r#"{"timestamp":"2026-10-29T10:00:05Z","type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"c1","output":[{"type":"input_text","text":"Script completed\nWall time 0.6 seconds\nOutput:\n"},{"type":"input_text","text":"alpha line\nbeta line"}]}}"#,
            // A script that failed before running anything.
            r#"{"timestamp":"2026-10-29T10:00:06Z","type":"response_item","payload":{"type":"custom_tool_call","call_id":"c2","name":"exec","input":"await tools.exec_command({cmd:\"x\"});"}}"#,
            r#"{"timestamp":"2026-10-29T10:00:07Z","type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"c2","output":[{"type":"input_text","text":"Script failed\nWall time 0.4 seconds\nOutput:\n"},{"type":"input_text","text":"Script error:\nexec_command failed: Access is denied."}]}}"#,
            // A script still running after its call returned, finished by `wait`.
            r#"{"timestamp":"2026-10-29T10:00:08Z","type":"response_item","payload":{"type":"custom_tool_call","call_id":"c3","name":"exec","input":"text(await longTask());"}}"#,
            r#"{"timestamp":"2026-10-29T10:00:09Z","type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"c3","output":"Script running with cell ID 7\nWall time 31.0 seconds\nOutput:\n\npart one"}}"#,
            r#"{"timestamp":"2026-10-29T10:00:10Z","type":"response_item","payload":{"type":"function_call","name":"wait","arguments":"{\"cell_id\":\"7\",\"yield_time_ms\":1000}","call_id":"w1"}}"#,
            r#"{"timestamp":"2026-10-29T10:00:11Z","type":"response_item","payload":{"type":"function_call_output","call_id":"w1","output":"Script completed\nWall time 1.0 seconds\nOutput:\n\npart two"}}"#,
            // Older code mode: one shell call, no item record, nested header.
            r#"{"timestamp":"2026-10-29T10:00:12Z","type":"response_item","payload":{"type":"custom_tool_call","call_id":"c4","name":"exec","input":"const r = await tools.shell_command({command:\"git status --short; rg -n \\\"a\\\\(\\\" src\"}); text(r);"}}"#,
            r#"{"timestamp":"2026-10-29T10:00:13Z","type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"c4","output":[{"type":"input_text","text":"Script completed\nWall time 0.4 seconds\nOutput:\n"},{"type":"input_text","text":"Exit code: 1\nWall time: 0.3 seconds\nOutput:\n M a.rs"}]}}"#,
        ]);
        let t = tools(&s);
        assert_eq!(t.len(), 5, "{:#?}", t.iter().map(|t| (&t.name, &t.title)).collect::<Vec<_>>());
        assert_eq!(t[4].title.as_deref(), Some(r#"Ran git status --short; rg -n "a\(" src"#));
        assert_eq!((t[4].output.as_deref(), t[4].status), (Some(" M a.rs"), ToolStatus::Failed), "inner header parsed");
        assert_eq!(t[0].kind, ToolKind::Read);
        assert_eq!(t[0].title.as_deref(), Some("Read notes.txt"));
        assert_eq!(t[0].input["command"], "type notes.txt");
        assert_eq!(t[0].output.as_deref(), Some("alpha line\nbeta line"));
        assert_eq!(t[0].duration_ms, Some(157));
        assert_eq!(t[1].kind, ToolKind::Write);
        assert_eq!(t[1].diffs[0].added, 1);
        assert_eq!((t[2].title.as_deref(), t[2].status), (Some("Ran x"), ToolStatus::Failed), "a one-command script reads as the command");
        assert!(t[2].output.as_deref().unwrap().starts_with("Script error:"), "header stripped");
        assert_eq!(t[3].output.as_deref(), Some("part one\npart two"), "the wait folds into its cell");
        assert_eq!(t[3].status, ToolStatus::Completed);
    }

    #[test]
    fn run_headers_polls_and_namespaces() {
        let s = rollout(&[
            r#"{"timestamp":"2026-10-29T10:00:02Z","type":"response_item","payload":{"type":"function_call","name":"shell_command","arguments":"{\"command\":\"cargo test\"}","call_id":"a"}}"#,
            r#"{"timestamp":"2026-10-29T10:00:03Z","type":"response_item","payload":{"type":"function_call_output","call_id":"a","output":"Exit code: 2\nWall time: 0.5 seconds\nOutput:\nboom"}}"#,
            // A long command polled with write_stdin.
            r#"{"timestamp":"2026-10-29T10:00:04Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"npm run build\"}","call_id":"b"}}"#,
            r#"{"timestamp":"2026-10-29T10:00:05Z","type":"response_item","payload":{"type":"function_call_output","call_id":"b","output":"Chunk ID: 12a3ce\nWall time: 10.0 seconds\nProcess running with session ID 42\nOriginal token count: 1\nOutput:\nbuilding"}}"#,
            r#"{"timestamp":"2026-10-29T10:00:06Z","type":"response_item","payload":{"type":"function_call","name":"write_stdin","arguments":"{\"session_id\":42,\"chars\":\"\"}","call_id":"b2"}}"#,
            r#"{"timestamp":"2026-10-29T10:00:07Z","type":"response_item","payload":{"type":"function_call_output","call_id":"b2","output":"Chunk ID: 469471\nWall time: 1.8 seconds\nProcess exited with code 1\nOriginal token count: 3\nOutput:\nerror TS2304"}}"#,
            // An MCP call whose item is recorded before the call itself.
            r#"{"timestamp":"2026-10-29T10:00:08Z","type":"event_msg","payload":{"type":"item_completed","item":{"type":"McpToolCall","id":"m","server":"node_repl","tool":"js","arguments":{},"status":"completed","result":{"content":[],"isError":false},"duration":{"secs":2,"nanos":0}}}}"#,
            r#"{"timestamp":"2026-10-29T10:00:08Z","type":"response_item","payload":{"type":"function_call","name":"js","namespace":"mcp__node_repl__","arguments":"{\"code\":\"1+1\",\"title\":\"Check the sum\"}","call_id":"m"}}"#,
            r#"{"timestamp":"2026-10-29T10:00:09Z","type":"response_item","payload":{"type":"function_call_output","call_id":"m","output":"Wall time: 0.1 seconds\nOutput:\n[{\"type\":\"text\",\"text\":\"2\"},{\"type\":\"image\",\"data\":\"iVBORw0KGgo=\",\"mimeType\":\"image/png\"}]"}}"#,
            r#"{"timestamp":"2026-10-29T10:00:10Z","type":"response_item","payload":{"type":"function_call","name":"read","namespace":"mcp__site_builder","arguments":"{\"table\":\"T\"}","call_id":"n"}}"#,
            r#"{"timestamp":"2026-10-29T10:00:11Z","type":"response_item","payload":{"type":"function_call","name":"run","namespace":"web","arguments":"{\"search_query\":[{\"q\":\"rust serde\"}]}","call_id":"w"}}"#,
            r#"{"timestamp":"2026-10-29T10:00:12Z","type":"response_item","payload":{"type":"function_call","name":"view_image","arguments":"{\"path\":\"shot.png\"}","call_id":"v"}}"#,
            r#"{"timestamp":"2026-10-29T10:00:13Z","type":"response_item","payload":{"type":"function_call_output","call_id":"v","output":[{"type":"input_image","image_url":"data:image/png;base64,iVBORw0KGgo="}]}}"#,
        ]);
        let t = tools(&s);
        assert_eq!(t.len(), 6, "write_stdin folds into its command");
        assert_eq!((t[0].output.as_deref(), t[0].status, t[0].duration_ms), (Some("boom"), ToolStatus::Failed, Some(500)));
        assert_eq!(t[1].output.as_deref(), Some("building\nerror TS2304"));
        assert_eq!(t[1].status, ToolStatus::Failed);
        assert_eq!((t[2].name.as_str(), t[2].title.as_deref()), ("mcp__node_repl__js", Some("Check the sum")));
        assert_eq!((t[2].output.as_deref(), t[2].images.len(), t[2].duration_ms), (Some("2"), 1, Some(2000)));
        assert_eq!((t[3].name.as_str(), t[3].kind), ("mcp__site_builder__read", ToolKind::Mcp));
        assert_eq!((t[4].kind, t[4].input["query"].as_str()), (ToolKind::WebSearch, Some("rust serde")));
        assert_eq!(t[5].kind, ToolKind::Read);
        assert!(matches!(&t[5].images[0], Part::Image { mime: Some(m), .. } if m == "image/png"));
    }

    /// A patch result recorded before its call must not land on the
    /// previous patch.
    #[test]
    fn early_file_change_finds_its_call() {
        let s = rollout(&[
            r#"{"timestamp":"2026-10-29T10:00:02Z","type":"response_item","payload":{"type":"custom_tool_call","name":"apply_patch","call_id":"p1","input":"*** Begin Patch\n*** Update File: a.rs\n@@\n-x\n+y\n*** End Patch"}}"#,
            r#"{"timestamp":"2026-10-29T10:00:03Z","type":"event_msg","payload":{"type":"item_completed","item":{"type":"FileChange","id":"p2","changes":{"b.rs":{"type":"add","content":"one\ntwo\n"}},"status":"completed"}}}"#,
            r#"{"timestamp":"2026-10-29T10:00:03Z","type":"response_item","payload":{"type":"custom_tool_call","name":"apply_patch","call_id":"p2","input":"*** Begin Patch\n*** Add File: b.rs\n+one\n+two\n*** End Patch"}}"#,
        ]);
        let t = tools(&s);
        assert_eq!(t[0].diffs.iter().map(|d| d.path.as_str()).collect::<Vec<_>>(), ["a.rs"]);
        assert_eq!(t[1].diffs.iter().map(|d| d.path.as_str()).collect::<Vec<_>>(), ["b.rs"]);
    }

    #[test]
    fn agents_images_goals_and_failures() {
        let s = rollout(&[
            r#"{"timestamp":"2026-10-29T10:00:02Z","type":"event_msg","payload":{"type":"thread_settings_applied","thread_settings":{"model":"gpt-5.6-luna"}}}"#,
            r#"{"timestamp":"2026-10-29T10:00:02Z","type":"event_msg","payload":{"type":"thread_goal_updated","goal":{"objective":"Ship the parser","status":"active"}}}"#,
            r#"{"timestamp":"2026-10-29T10:00:03Z","type":"response_item","payload":{"type":"function_call","name":"spawn_agent","namespace":"collaboration","arguments":"{\"task_name\":\"backend_audit\",\"message\":\"gAAAAABqUBA546ERi89poR4F\"}","call_id":"s1"}}"#,
            r#"{"timestamp":"2026-10-29T10:00:04Z","type":"response_item","payload":{"type":"function_call_output","call_id":"s1","output":"{\"task_name\":\"/root/backend_audit\"}"}}"#,
            r#"{"timestamp":"2026-10-29T10:00:05Z","type":"response_item","payload":{"type":"function_call","name":"send_message","namespace":"collaboration","arguments":"{\"target\":\"/root/backend_audit\",\"message\":\"gAAAAABqUBFhiQtNtKlW09LJ\"}","call_id":"s2"}}"#,
            r#"{"timestamp":"2026-10-29T10:00:06Z","type":"response_item","payload":{"type":"agent_message","author":"/root/backend_audit","recipient":"/root","content":[{"type":"input_text","text":"Message Type: FINAL_ANSWER\nTask name: /root\nSender: /root/backend_audit\nPayload:\nNo blockers found."}]}}"#,
            r#"{"timestamp":"2026-10-29T10:00:07Z","type":"response_item","payload":{"type":"image_generation_call","id":"ig_1","status":"generating","revised_prompt":"A red cube","result":"iVBORw0KGgo="}}"#,
            r#"{"timestamp":"2026-10-29T10:00:08Z","type":"response_item","payload":{"type":"tool_search_call","call_id":"ts","status":"completed","execution":"client","arguments":{"query":"node repl","limit":10}}}"#,
            r#"{"timestamp":"2026-10-29T10:00:08Z","type":"response_item","payload":{"type":"tool_search_output","call_id":"ts","status":"completed","tools":[{"type":"namespace","name":"mcp__node_repl","tools":[{"type":"function","name":"js_reset"}]}]}}"#,
            r#"{"timestamp":"2026-10-29T10:00:09Z","type":"compacted","payload":{"message":""}}"#,
            r#"{"timestamp":"2026-10-29T10:00:09Z","type":"event_msg","payload":{"type":"item_completed","item":{"type":"ContextCompaction","id":"x"}}}"#,
            r#"{"timestamp":"2026-10-29T10:00:09Z","type":"response_item","payload":{"type":"compaction","id":"cmp_1","encrypted_content":"gAAAAA"}}"#,
            r#"{"timestamp":"2026-10-29T10:00:10Z","type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","content":[{"type":"text","text":"look"},{"type":"image","image_url":"data:image/jpeg;base64,/9j/4AAQ"},{"type":"local_image","path":"C:\\missing\\clip.png"}]}}}"#,
            r#"{"timestamp":"2026-10-29T10:00:10Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"look"},{"type":"input_image","image_url":"data:image/jpeg;base64,/9j/4AAQ"}]}}"#,
            r#"{"timestamp":"2026-10-29T10:00:11Z","type":"event_msg","payload":{"type":"turn_aborted","reason":"review_ended"}}"#,
            r#"{"timestamp":"2026-10-29T10:00:12Z","type":"event_msg","payload":{"type":"task_complete","error":{"message":"{\"type\":\"error\",\"status\":400,\"error\":{\"type\":\"invalid_request_error\",\"message\":\"The model is not supported.\"}}"}}}"#,
        ]);
        assert_eq!(s.model.as_deref(), Some("gpt-5.6-luna"));
        let t = tools(&s);
        assert_eq!((t[0].kind, t[0].title.as_deref()), (ToolKind::Task, Some("Subagent backend_audit")));
        assert!(t[0].input.get("message").is_none(), "encrypted message dropped");
        assert_eq!(t[0].output.as_deref(), Some("No blockers found."), "final answer is the spawn result");
        assert_eq!(t[1].title.as_deref(), Some("Messaged backend_audit"));
        assert_eq!((t[2].title.as_deref(), t[2].images.len()), (Some("Generated an image"), 1));
        assert_eq!((t[3].title.as_deref(), t[3].output.as_deref()), (Some("Searched tools for node repl"), Some("Loaded mcp__node_repl__js_reset")));
        let notices: Vec<String> = s.messages.iter().flat_map(|m| &m.parts).filter_map(|p| match p { Part::Notice { text } => Some(text.clone()), _ => None }).collect();
        assert_eq!(notices, ["Goal: Ship the parser", "Context compacted", "Turn aborted (review ended)"]);
        let look = s.messages.iter().find(|m| m.role == Role::User && m.text() == "look").expect("image prompt in place");
        assert_eq!(look.parts.iter().filter(|p| matches!(p, Part::Image { mime: Some(m), .. } if m == "image/jpeg")).count(), 1, "missing local file dropped");
        assert!(s.messages.iter().flat_map(|m| &m.parts).any(|p| matches!(p, Part::Error { message } if message == "The model is not supported.")));
    }

    #[test]
    fn parses_legacy_and_paginated() {
        let legacy = [
            r#"{"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"id":"T1","cwd":"/r","cli_version":"0.1","git":{"branch":"dev"}}}"#,
            r#"{"timestamp":"2026-01-01T00:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>x</environment_context>"}]}}"#,
            r#"{"timestamp":"2026-01-01T00:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"list files"}]}}"#,
            r#"{"timestamp":"2026-01-01T00:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"list files"}}"#,
            r#"{"timestamp":"2026-01-01T00:00:02Z","type":"response_item","payload":{"type":"function_call","name":"shell_command","arguments":"{\"command\":\"ls\"}","call_id":"c1"}}"#,
            r#"{"timestamp":"2026-01-01T00:00:03Z","type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":"a.rs"}}"#,
            r#"{"timestamp":"2026-01-01T00:00:03Z","type":"response_item","payload":{"type":"custom_tool_call","name":"apply_patch","call_id":"c2","input":"*** Begin Patch\n*** Update File: a.rs\n@@\n-x\n+y\n*** End Patch"}}"#,
            r#"{"timestamp":"2026-01-01T00:00:04Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Here they are"}]}}"#,
            r#"{"timestamp":"2026-01-01T00:00:04Z","type":"event_msg","payload":{"type":"agent_message","message":"Here they are"}}"#,
            r#"{"timestamp":"2026-01-01T00:00:05Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":7}}}}"#,
        ];
        let s = parse(&legacy.join("\n"), None).unwrap();
        assert_eq!(s.origin.native_id.as_deref(), Some("T1"));
        assert_eq!(s.messages.len(), 2, "{:#?}", s.messages);
        assert_eq!(s.messages[0].text(), "list files");
        let a = &s.messages[1];
        assert_eq!(a.tool_calls().count(), 2);
        assert_eq!(a.tool_calls().next().unwrap().output.as_deref(), Some("a.rs"));
        assert_eq!(a.tool_calls().nth(1).unwrap().diffs.len(), 1);
        assert_eq!(a.text(), "Here they are");
        assert_eq!(s.usage.cache_read_tokens, 40);

        let paginated = [
            r#"{"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"id":"T2","cwd":"/r","history_mode":"paginated"}}"#,
            r#"{"timestamp":"2026-01-01T00:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"The following is injected"}]}}"#,
            r#"{"timestamp":"2026-01-01T00:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hi there"}]}}"#,
            r#"{"timestamp":"2026-01-01T00:00:01Z","type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","content":[{"type":"text","text":"hi there"}]}}}"#,
            r#"{"timestamp":"2026-01-01T00:00:02Z","type":"response_item","payload":{"type":"reasoning","summary":[],"encrypted_content":"x"}}"#,
            r#"{"timestamp":"2026-01-01T00:00:02Z","type":"event_msg","payload":{"type":"item_completed","item":{"type":"Reasoning","summary_text":["thinking hard"]}}}"#,
            r#"{"timestamp":"2026-01-01T00:00:03Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"}]}}"#,
            r#"{"timestamp":"2026-01-01T00:00:03Z","type":"event_msg","payload":{"type":"item_completed","item":{"type":"AgentMessage","content":[{"type":"Text","text":"hello"}]}}}"#,
        ];
        let s = parse(&paginated.join("\n"), None).unwrap();
        assert_eq!(s.messages.len(), 2, "{:#?}", s.messages);
        assert_eq!(s.messages[0].text(), "hi there");
        assert!(matches!(&s.messages[1].parts[0], Part::Reasoning { text, .. } if text == "thinking hard"));
        assert_eq!(s.messages[1].text(), "hello");
    }

    #[test]
    fn links_spawned_child_threads() {
        let dir = tempfile::tempdir().unwrap();
        let day = dir.path().join("sessions").join("2026").join("01").join("02");
        std::fs::create_dir_all(&day).unwrap();
        let parent_id = "0199a213-81c0-7800-8aa1-bbab2a035a53";
        let child_id = "0199a213-81c0-7800-8aa1-cccccccccccc";
        let parent = [
            r#"{"timestamp":"2026-01-02T00:00:00Z","type":"session_meta","payload":{"id":"0199a213-81c0-7800-8aa1-bbab2a035a53","cwd":"/r"}}"#,
            r#"{"timestamp":"2026-01-02T00:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"audit with a helper"}}"#,
            r#"{"timestamp":"2026-01-02T00:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"audit with a helper"}]}}"#,
            r#"{"timestamp":"2026-01-02T00:00:02Z","type":"response_item","payload":{"type":"function_call","name":"spawn_agent","arguments":"{\"task_name\":\"audit_parser\",\"message\":\"Audit the parser\"}","call_id":"call_spawn"}}"#,
            r#"{"timestamp":"2026-01-02T00:00:03Z","type":"response_item","payload":{"type":"function_call_output","call_id":"call_spawn","output":"spawned"}}"#,
            r#"{"timestamp":"2026-01-02T00:00:03Z","type":"event_msg","payload":{"type":"item_completed","item":{"type":"SubAgentActivity","id":"call_spawn","agent_thread_id":"0199a213-81c0-7800-8aa1-cccccccccccc","agent_path":"/root/audit_parser","kind":"spawned"}}}"#,
            r#"{"timestamp":"2026-01-02T00:00:09Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"All good"}]}}"#,
        ];
        let child = [
            r#"{"timestamp":"2026-01-02T00:00:03Z","type":"session_meta","payload":{"id":"0199a213-81c0-7800-8aa1-cccccccccccc","cwd":"/r","parent_thread_id":"0199a213-81c0-7800-8aa1-bbab2a035a53","agent_role":"explorer","source":{"subagent":{"thread_spawn":{"parent_thread_id":"0199a213-81c0-7800-8aa1-bbab2a035a53","depth":1,"agent_role":"explorer"}}}}}"#,
            r#"{"timestamp":"2026-01-02T00:00:04Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Audit the parser"}]}}"#,
            r#"{"timestamp":"2026-01-02T00:00:05Z","type":"response_item","payload":{"type":"function_call","name":"shell_command","arguments":"{\"command\":\"rg parse\"}","call_id":"c_rg"}}"#,
            r#"{"timestamp":"2026-01-02T00:00:06Z","type":"response_item","payload":{"type":"function_call_output","call_id":"c_rg","output":"src/parser.rs"}}"#,
            r#"{"timestamp":"2026-01-02T00:00:07Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Parser is fine"}]}}"#,
        ];
        let parent_path = day.join(format!("rollout-2026-01-02T00-00-00-{parent_id}.jsonl"));
        std::fs::write(&parent_path, parent.join("\n")).unwrap();
        std::fs::write(day.join(format!("rollout-2026-01-02T00-00-03-{child_id}.jsonl")), child.join("\n")).unwrap();

        let s = parse(&parent.join("\n"), Some(&parent_path)).unwrap();
        let spawn = s.messages.iter().flat_map(|m| m.tool_calls()).find(|t| t.id == "call_spawn").unwrap();
        assert!(spawn.is_subagent());
        assert_eq!(spawn.agent_name.as_deref(), Some("audit_parser"));
        assert_eq!(spawn.subagent.len(), 1, "{:#?}", spawn.subagent);
        assert_eq!(spawn.subagent_steps(), 1);
        assert_eq!(spawn.subagent[0].text(), "Parser is fine");
        assert!(spawn.subagent[0].sidechain);
        assert_eq!(codex_root_of(&parent_path).as_deref(), Some(dir.path()));
    }

    #[test]
    fn export_roundtrip() {
        let mut s = Session::new(AGENT, Some(larrier_core::WorkspaceRef::from_path("/tmp/x")));
        s.push(larrier_core::Message::user_text("hello"));
        let mut a = larrier_core::Message::assistant(AGENT, None);
        a.push_text("world");
        s.push(a);
        let back = parse(&export_lines(&s, "ID", Utc::now()), None).unwrap();
        assert_eq!(back.messages.len(), 2);
        assert_eq!(back.messages[0].text(), "hello");
        assert_eq!(back.messages[1].text(), "world");
        assert_eq!(back.origin.native_id.as_deref(), Some("ID"));
    }
}
