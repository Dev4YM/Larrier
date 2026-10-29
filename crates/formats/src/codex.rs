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
use larrier_core::lsf::{close_active_tools, find_tool_path, slim_transcript, tool_at_path_mut};
use larrier_core::{AgentKind, FileDiff, Message, NativeLink, Part, Role, Session, ToolCall, ToolStatus, Usage};
use serde_json::{json, Value};

use crate::util::{self, content_text, env_dir, finish, head_lines, json_lines, json_or_string, parse_ts, s, string, Conv, Meta};
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
    let (conv, spawns) = convert(&lines, path, &mut meta);
    let mut s = finish(conv, AGENT, meta)?;
    attach_children(&mut s.messages, &spawns, path, 0);
    Ok(s)
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
                event_users.push((s(pl, "message").unwrap_or("").to_string(), ts, parts));
            }
            Some("item_completed") if s(&pl["item"], "type") == Some("UserMessage") => {
                let t = pl["item"]["content"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|c| c.get("text").and_then(|x| x.as_str())).collect::<Vec<_>>().join("\n"))
                    .unwrap_or_default();
                event_users.push((t, ts, Vec::new()));
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
            "compacted" => conv.notice("Context compacted", ts),
            "response_item" => match s(pl, "type").unwrap_or("") {
                "message" => {
                    let role = s(pl, "role").unwrap_or("");
                    let text = content_text(&pl["content"]);
                    if role == "assistant" {
                        conv.text(&text, ts, model.as_deref());
                    } else if role == "user" {
                        if use_events {
                            // Place the matching event prompt here (keeps ordering).
                            let key = norm(&text);
                            if let Some(pos) = pending_users.iter().rposition(|(t, _, _)| norm(t) == key || (!t.trim().is_empty() && key.contains(&norm(t)))) {
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
                    let name = s(pl, "name").unwrap_or(if s(pl, "type") == Some("local_shell_call") { "shell" } else { "tool" });
                    let id = s(pl, "call_id").or_else(|| s(pl, "id")).unwrap_or("");
                    let input = if let Some(a) = s(pl, "arguments") {
                        json_or_string(a)
                    } else if let Some(i) = s(pl, "input") {
                        let v = json_or_string(i);
                        if v.is_string() && matches!(name, "exec" | "shell" | "exec_command") {
                            json!({ "command": i })
                        } else {
                            v
                        }
                    } else {
                        pl.get("action").cloned().unwrap_or(Value::Null)
                    };
                    if name == "spawn_agent" {
                        spawn_calls.insert(id.to_string());
                    }
                    let mut tool = ToolCall::new(id, name, input);
                    tool.started_at = ts;
                    conv.tool(tool, ts, model.as_deref());
                }
                "web_search_call" => {
                    let id = s(pl, "id").unwrap_or("");
                    let q = pl["action"]["query"].as_str().unwrap_or("");
                    let mut tool = ToolCall::new(id, "web_search", json!({ "query": q }));
                    tool.status = larrier_core::ToolStatus::Completed;
                    conv.tool(tool, ts, model.as_deref());
                }
                "function_call_output" | "custom_tool_call_output" | "local_shell_call_output" => {
                    let id = s(pl, "call_id").unwrap_or("");
                    let (out, is_err) = tool_output(&pl["output"]);
                    conv.tool_result(id, Some(out), is_err, Vec::new());
                }
                _ => {}
            },
            "event_msg" => match s(pl, "type").unwrap_or("") {
                "token_count" => {
                    let u = &pl["info"]["total_token_usage"];
                    if u.is_object() {
                        usage_total = Some(Usage {
                            input_tokens: u["input_tokens"].as_u64().unwrap_or(0).saturating_sub(u["cached_input_tokens"].as_u64().unwrap_or(0)),
                            output_tokens: u["output_tokens"].as_u64().unwrap_or(0),
                            cache_read_tokens: u["cached_input_tokens"].as_u64().unwrap_or(0),
                            cache_write_tokens: u["cache_write_input_tokens"].as_u64().unwrap_or(0),
                            reasoning_tokens: u["reasoning_output_tokens"].as_u64().unwrap_or(0),
                            cost_usd: None,
                        });
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
                        "FileChange" => {
                            // Enrich the latest apply_patch call with the real file content.
                            let diffs = file_change_diffs(&item["changes"]);
                            if !diffs.is_empty() {
                                attach_latest_patch(&mut conv, diffs);
                            }
                        }
                        _ => {}
                    }
                }
                "turn_aborted" => conv.notice("Interrupted", ts),
                _ => {}
            },
            _ => {}
        }
    }
    // Event prompts that never matched a response item (e.g. images-only).
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
    parts.insert(0, Part::Text { text: text.to_string() });
    conv.user_parts(parts, ts);
}

fn tool_output(v: &Value) -> (String, bool) {
    match v {
        Value::String(s) => {
            // custom tool outputs are often JSON {"output": .., "metadata": {"exit_code": ..}}
            if let Ok(o) = serde_json::from_str::<Value>(s) {
                if let Some(out) = o.get("output").and_then(|x| x.as_str()) {
                    let code = o["metadata"]["exit_code"].as_i64().unwrap_or(0);
                    return (out.to_string(), code != 0);
                }
            }
            (s.clone(), false)
        }
        Value::Array(_) => (content_text(v), false),
        Value::Object(o) => {
            let out = o.get("content").map(content_text).or_else(|| o.get("output").and_then(|x| x.as_str()).map(String::from)).unwrap_or_default();
            (out, o.get("success").and_then(|b| b.as_bool()) == Some(false))
        }
        _ => (String::new(), false),
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

fn attach_latest_patch(conv: &mut Conv, diffs: Vec<FileDiff>) {
    for m in conv.messages.iter_mut().rev() {
        if m.role != Role::Assistant {
            break;
        }
        for p in m.parts.iter_mut().rev() {
            if let Part::Tool(t) = p {
                if t.kind.is_mutation() {
                    // Keep patch-derived counts; only fill paths the patch lacked.
                    for d in diffs {
                        if !t.diffs.iter().any(|x| x.path == d.path || d.path.ends_with(&x.path)) {
                            t.diffs.push(d);
                        }
                    }
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
    Ok(NativeLink { agent: AGENT, id, path: Some(path.to_string_lossy().to_string()), synced_messages: session.messages.len(), account_id: None })
}

#[cfg(test)]
mod tests {
    use super::*;

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
