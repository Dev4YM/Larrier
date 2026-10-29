//! Claude Code: `~/.claude/projects/<encoded-cwd>/<sessionId>.jsonl`.
//!
//! Assistant replies are written one content block per line (sharing
//! `message.id`); the visible conversation is the `parentUuid` chain from the
//! newest message back to the root, continuing across compaction boundaries
//! through `logicalParentUuid`.
//!
//! Subagents (Task / Agent tool) write their own transcript to
//! `<project>/<sessionId>/subagents/agent-<agentId>.jsonl`, with
//! `agent-<agentId>.meta.json` naming the spawning `toolUseId`; older
//! versions wrote them inline as `isSidechain` lines.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::Utc;
use larrier_core::lsf::{close_active_tools, find_tool_deep_mut, slim_transcript, title_from_prompt};
use larrier_core::{AgentKind, FileDiff, Message, NativeLink, Part, Role, Session, ToolCall, ToolStatus, Usage};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::util::{self, content_text, env_dir, finish, head_lines, image_part, image_parts, image_refs, json_lines, parse_ts, s, string, tail_lines, Conv, Meta};
use crate::NativeSessionInfo;

const AGENT: AgentKind = AgentKind::ClaudeCode;

pub fn root(home_override: Option<&Path>) -> PathBuf {
    home_override
        .map(PathBuf::from)
        .or_else(|| env_dir("CLAUDE_CONFIG_DIR"))
        .unwrap_or_else(|| util::home().join(".claude"))
}

/// Claude's project folder name: every non-alphanumeric char becomes `-`.
pub fn encode_project(cwd: &str) -> String {
    let enc: String = cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    if enc.len() <= 200 {
        return enc;
    }
    let hash = hex::encode(Sha256::digest(cwd.as_bytes()));
    format!("{}-{}", &enc[..200], &hash[..8])
}

pub fn discover() -> Vec<NativeSessionInfo> {
    let projects = root(None).join("projects");
    let mut out = Vec::new();
    let Ok(dirs) = std::fs::read_dir(&projects) else { return out };
    for d in dirs.flatten() {
        let dir = d.path();
        if !dir.is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(&dir) else { continue };
        for f in files.flatten() {
            if let Some(info) = file_info(&f.path()) {
                out.push(info);
            }
        }
    }
    out
}

/// Session info for one transcript file; `None` for anything that isn't a
/// top-level session transcript.
pub fn file_info(p: &Path) -> Option<NativeSessionInfo> {
    let name = p.file_name()?.to_string_lossy().to_string();
    if !name.ends_with(".jsonl") || name.starts_with("agent-") {
        return None;
    }
    let id = name.trim_end_matches(".jsonl").to_string();
    let mut info = util::info(AGENT, id, p);
    if info.size < 64 {
        return None;
    }
    peek(p, &mut info);
    Some(info)
}

/// The session transcript a changed file belongs to: a subagent transcript
/// (`<project>/<sessionId>/subagents/agent-*.jsonl`) maps to its parent.
pub fn session_file_for(changed: &Path) -> Option<PathBuf> {
    let name = changed.file_name()?.to_string_lossy().to_string();
    if name.ends_with(".jsonl") && !name.starts_with("agent-") {
        return Some(changed.to_path_buf());
    }
    // <project>/<sessionId>/subagents/agent-*.jsonl (or its .meta.json)
    let sub = changed.parent()?;
    if sub.file_name()?.to_string_lossy() != "subagents" {
        return None;
    }
    let session_dir = sub.parent()?;
    let id = session_dir.file_name()?.to_string_lossy().to_string();
    Some(session_dir.parent()?.join(format!("{id}.jsonl")))
}

fn peek(path: &Path, info: &mut NativeSessionInfo) {
    for l in head_lines(path, 60) {
        let Ok(v) = serde_json::from_str::<Value>(&l) else { continue };
        if info.cwd.is_none() {
            info.cwd = string(&v, "cwd");
        }
        if info.title.is_none() && s(&v, "type") == Some("user") && v.get("isMeta").is_none() {
            let t = clean_user_text(&content_text(&v["message"]["content"]));
            if !t.trim().is_empty() {
                info.title = Some(title_from_prompt(&t));
            }
        }
        if info.cwd.is_some() && info.title.is_some() {
            break;
        }
    }
    let mut custom = None;
    let mut ai = None;
    for l in tail_lines(path, 96 * 1024) {
        let Ok(v) = serde_json::from_str::<Value>(&l) else { continue };
        match s(&v, "type") {
            Some("custom-title") => custom = string(&v, "customTitle").or(custom),
            Some("ai-title") => ai = string(&v, "aiTitle").or(ai),
            Some("summary") => ai = string(&v, "summary").or(ai),
            _ => {}
        }
        if let Some(t) = s(&v, "timestamp").and_then(parse_ts) {
            info.updated_at = Some(t);
        }
    }
    if let Some(t) = custom.or(ai) {
        info.title = Some(t);
    }
}

pub fn import(info: &NativeSessionInfo) -> Result<Session> {
    let text = util::read_text(&info.path)?;
    parse(&text, Some(&info.path), &info.native_id)
}

/// Strip harness wrappers from user text; empty means "not a real prompt".
pub fn clean_user_text(text: &str) -> String {
    let t = text.trim();
    if t.starts_with("Caveat: The messages below were generated")
        || t.starts_with("<local-command-stdout>")
        || t.starts_with("<local-command-stderr>")
        || t.starts_with("<bash-stdout>")
        || t.starts_with("<bash-stderr>")
        || t.starts_with("<task-notification>")
        || t.starts_with("<local-command-caveat>")
    {
        return String::new();
    }
    if let Some(name) = between(t, "<command-name>", "</command-name>") {
        let args = between(t, "<command-args>", "</command-args>").unwrap_or_default();
        return format!("{} {}", name.trim(), args.trim()).trim().to_string();
    }
    if let Some(cmd) = between(t, "<bash-input>", "</bash-input>") {
        return format!("! {}", cmd.trim());
    }
    strip_tag(&strip_tag(t, "system-reminder"), "user-prompt-submit-hook").trim().to_string()
}

fn between(t: &str, a: &str, b: &str) -> Option<String> {
    let i = t.find(a)? + a.len();
    let j = t[i..].find(b)? + i;
    Some(t[i..j].to_string())
}

fn strip_tag(t: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut out = t.to_string();
    while let Some(i) = out.find(&open) {
        match out[i..].find(&close) {
            Some(j) => out.replace_range(i..i + j + close.len(), ""),
            None => break,
        }
    }
    out
}

/// Parse a Claude Code session JSONL.
pub fn parse(text: &str, path: Option<&Path>, fallback_id: &str) -> Result<Session> {
    let lines = json_lines(text);
    let mut meta = Meta { native_id: fallback_id.to_string(), path: path.map(PathBuf::from), ..Default::default() };
    let mut custom_title = None;
    let mut ai_title = None;
    let mut leaf: Option<String> = None;
    let mut index: HashMap<String, usize> = HashMap::new();
    for (i, v) in lines.iter().enumerate() {
        if meta.native_id.is_empty() {
            if let Some(sid) = s(v, "sessionId") {
                meta.native_id = sid.to_string();
            }
        }
        if meta.cwd.is_none() {
            meta.cwd = string(v, "cwd");
        }
        if meta.branch.is_none() {
            meta.branch = string(v, "gitBranch").filter(|b| !b.is_empty());
        }
        if let Some(ver) = s(v, "version") {
            meta.version = Some(ver.to_string());
        }
        match s(v, "type") {
            Some("custom-title") => custom_title = string(v, "customTitle").or(custom_title.take()),
            Some("ai-title") => ai_title = string(v, "aiTitle").or(ai_title.take()),
            Some("summary") => ai_title = string(v, "summary").or(ai_title.take()),
            Some("last-prompt") => leaf = string(v, "leafUuid").or(leaf.take()),
            _ => {}
        }
        if let Some(u) = s(v, "uuid") {
            index.insert(u.to_string(), i);
        }
    }
    meta.title = custom_title.or(ai_title);

    // Pick the active branch.
    let is_msg = |v: &Value| matches!(s(v, "type"), Some("user") | Some("assistant")) && !v["isSidechain"].as_bool().unwrap_or(false);
    let leaf_idx = leaf
        .as_ref()
        .and_then(|l| index.get(l).copied())
        .filter(|&i| i < lines.len())
        .into_iter()
        .chain(lines.iter().enumerate().rev().filter(|(_, v)| is_msg(v)).map(|(i, _)| i).take(1))
        .max();
    let mut active: Option<HashSet<usize>> = None;
    if let Some(start) = leaf_idx {
        let mut set = HashSet::new();
        let mut cur = Some(start);
        let mut guard = 0;
        while let Some(i) = cur {
            if !set.insert(i) || guard > lines.len() {
                break;
            }
            guard += 1;
            let v = &lines[i];
            let parent = s(v, "parentUuid").or_else(|| s(v, "logicalParentUuid"));
            cur = parent.and_then(|p| index.get(p).copied());
        }
        let total = lines.iter().filter(|v| is_msg(v)).count();
        let covered = set.iter().filter(|&&i| is_msg(&lines[i])).count();
        // Older files without reliable chains: fall back to file order.
        if covered * 10 >= total * 3 {
            active = Some(set);
        }
    }

    let (mut conv, agent_ids) = convert(&lines, active.as_ref(), false);
    attach_inline_sidechains(&mut conv.messages, &lines);
    if let Some(p) = path {
        attach_subagent_files(&mut conv.messages, p, &agent_ids);
    }
    if let Some(p) = path {
        meta.resume_path = Some(p.to_string_lossy().to_string());
    }
    let mut session = finish(conv, AGENT, meta)?;
    session.external_turn = turn_open(&lines);
    Ok(session)
}

/// Whether Claude Code is still mid-turn: the last prompt (or queued prompt)
/// has no `end_turn` / `stop_sequence` reply and no stop-hook summary after it.
/// Every assistant record carries `stop_reason`; `tool_use` means more is
/// coming.
fn turn_open(lines: &[Value]) -> bool {
    let mut open = false;
    for v in lines {
        if v["isSidechain"].as_bool().unwrap_or(false) {
            continue;
        }
        match s(v, "type") {
            Some("user") if !v["isMeta"].as_bool().unwrap_or(false) => {
                let c = &v["message"]["content"];
                let text = match c {
                    Value::String(t) => t.clone(),
                    Value::Array(b) => b.iter().filter_map(|x| s(x, "text")).collect::<Vec<_>>().join(" "),
                    _ => String::new(),
                };
                if text.contains("[Request interrupted") {
                    open = false;
                } else {
                    open = true;
                }
            }
            Some("assistant") => {
                open = !matches!(s(&v["message"], "stop_reason"), Some("end_turn") | Some("stop_sequence") | Some("max_tokens") | Some("refusal"));
            }
            Some("system") if s(v, "subtype") == Some("stop_hook_summary") => open = false,
            Some("attachment") => {
                let a = &v["attachment"];
                if s(a, "type") == Some("queued_command") && s(a, "commandMode") != Some("task-notification") {
                    open = true;
                }
            }
            _ => {}
        }
    }
    open
}

/// Result fields that describe subagents and background tasks.
const TASK_KEYS: [&str; 7] = ["agentId", "agentType", "totalDurationMs", "isAsync", "backgroundTaskId", "taskId", "shellId"];

/// Convert transcript lines (the active branch, or all lines of a subagent
/// file when `sidechain`) into a conversation. Returns subagent `agentId` →
/// spawning tool id.
fn convert(lines: &[Value], active: Option<&HashSet<usize>>, sidechain: bool) -> (Conv, HashMap<String, String>) {
    let is_msg = |v: &Value| matches!(s(v, "type"), Some("user") | Some("assistant")) && (sidechain || !v["isSidechain"].as_bool().unwrap_or(false));
    let mut conv = Conv::new(AGENT);
    let mut usage_by_msg: Vec<(String, Usage, usize)> = Vec::new();
    let mut results: Vec<(String, Value)> = Vec::new();
    let mut notes: Vec<TaskNote> = Vec::new();
    for (i, v) in lines.iter().enumerate() {
        let ty = s(v, "type").unwrap_or("");
        if ty == "system" {
            if s(v, "subtype") == Some("compact_boundary") && active.as_ref().map(|a| a.contains(&i)).unwrap_or(true) {
                conv.notice("Context compacted", s(v, "timestamp").and_then(parse_ts));
            }
            continue;
        }
        // Prompts sent while the agent was working arrive as queued commands
        // (never as user records): show them where they were sent.
        if ty == "attachment" && !sidechain {
            let a = &v["attachment"];
            if s(a, "type") == Some("queued_command") {
                // A plain string, or content blocks when the prompt carries
                // images ([{type: image, …}, {type: text, …}]).
                let prompt = content_text(&a["prompt"]);
                let blocks_images = image_parts(&a["prompt"]);
                let ts = s(a, "timestamp").or_else(|| s(v, "timestamp")).and_then(parse_ts);
                if s(a, "commandMode") == Some("task-notification") || prompt.trim_start().starts_with("<task-notification>") {
                    notes.extend(task_notes(&prompt));
                } else {
                    let (text, images) = image_refs(&clean_user_text(&prompt));
                    let text = if blocks_images.is_empty() { text } else { text.lines().filter(|l| l.trim() != "[image]").collect::<Vec<_>>().join("\n").trim().to_string() };
                    let mut parts = Vec::new();
                    if !text.is_empty() {
                        parts.push(Part::Text { text });
                    }
                    parts.extend(blocks_images);
                    parts.extend(images);
                    if !parts.is_empty() {
                        conv.user_parts(parts, ts);
                    }
                }
            }
            continue;
        }
        if !is_msg(v) {
            continue;
        }
        if let Some(a) = active {
            if !a.contains(&i) {
                continue;
            }
        }
        let ts = s(v, "timestamp").and_then(parse_ts);
        if ty == "user" {
            if v["isMeta"].as_bool().unwrap_or(false) {
                continue;
            }
            if v["isCompactSummary"].as_bool().unwrap_or(false) {
                continue;
            }
            let content = &v["message"]["content"];
            let tool_meta = v.get("toolUseResult").or_else(|| v.get("tool_use_result"));
            let mut parts = Vec::new();
            match content {
                Value::String(t) => {
                    notes.extend(task_notes(t));
                    let (t, images) = image_refs(&clean_user_text(t));
                    if t.starts_with("[Request interrupted") {
                        conv.notice("Interrupted", ts);
                    } else if !t.is_empty() {
                        parts.push(Part::Text { text: t });
                    }
                    parts.extend(images);
                }
                Value::Array(blocks) => {
                    for b in blocks {
                        match s(b, "type") {
                            Some("text") => {
                                notes.extend(task_notes(s(b, "text").unwrap_or("")));
                                let (t, images) = image_refs(&clean_user_text(s(b, "text").unwrap_or("")));
                                if t.starts_with("[Request interrupted") {
                                    conv.notice("Interrupted", ts);
                                } else if !t.is_empty() {
                                    parts.push(Part::Text { text: t });
                                }
                                parts.extend(images);
                            }
                            Some("image") => {
                                let src = &b["source"];
                                if let Some(p) = image_part(string(src, "media_type"), s(src, "data"), s(src, "url")) {
                                    parts.push(p);
                                }
                            }
                            Some("tool_result") => {
                                let id = s(b, "tool_use_id").unwrap_or("");
                                let images = image_parts(&b["content"]);
                                let mut out = content_text(&b["content"]);
                                if !images.is_empty() {
                                    out = out.lines().filter(|l| l.trim() != "[image]").collect::<Vec<_>>().join("\n");
                                }
                                let is_err = b["is_error"].as_bool().unwrap_or(false);
                                let diffs = tool_meta.map(diffs_from_result).unwrap_or_default();
                                conv.tool_result(id, Some(out), is_err, diffs);
                                conv.tool_images(id, images);
                                if let Some(m) = tool_meta.filter(|m| TASK_KEYS.iter().any(|k| m.get(*k).is_some())) {
                                    results.push((id.to_string(), m.clone()));
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
            if !parts.is_empty() {
                conv.user_parts(parts, ts);
            }
            continue;
        }
        // assistant
        let msg = &v["message"];
        let model = s(msg, "model").filter(|m| *m != "<synthetic>");
        if v["isApiErrorMessage"].as_bool().unwrap_or(false) {
            let t = content_text(&msg["content"]);
            if !t.is_empty() {
                conv.error(&t, ts);
            }
            continue;
        }
        if let Some(blocks) = msg["content"].as_array() {
            for b in blocks {
                match s(b, "type") {
                    Some("text") => conv.text(s(b, "text").unwrap_or(""), ts, model),
                    Some("thinking") => {
                        let dur = v["thinkingDurationMs"].as_u64();
                        conv.reasoning(s(b, "thinking").unwrap_or(""), dur, ts, model)
                    }
                    Some("tool_use") | Some("server_tool_use") => {
                        let mut t = ToolCall::new(s(b, "id").unwrap_or(""), s(b, "name").unwrap_or("tool"), b.get("input").cloned().unwrap_or(Value::Null));
                        t.started_at = ts;
                        conv.tool(t, ts, model);
                    }
                    _ => {}
                }
            }
        } else if let Some(t) = msg["content"].as_str() {
            conv.text(t, ts, model);
        }
        if let (Some(mid), Some(u)) = (s(msg, "id"), msg.get("usage")) {
            let usage = Usage {
                input_tokens: u["input_tokens"].as_u64().unwrap_or(0),
                output_tokens: u["output_tokens"].as_u64().unwrap_or(0),
                cache_read_tokens: u["cache_read_input_tokens"].as_u64().unwrap_or(0),
                cache_write_tokens: u["cache_creation_input_tokens"].as_u64().unwrap_or(0),
                reasoning_tokens: 0,
                cost_usd: None,
            };
            let idx = conv.messages.len().saturating_sub(1);
            if let Some(e) = usage_by_msg.iter_mut().find(|e| e.0 == mid) {
                e.1 = usage;
                e.2 = idx;
            } else {
                usage_by_msg.push((mid.to_string(), usage, idx));
            }
        }
    }
    for (_, u, idx) in usage_by_msg {
        if let Some(m) = conv.messages.get_mut(idx) {
            if m.role == Role::Assistant {
                let mut acc = m.usage.clone().unwrap_or_default();
                acc.add(&u);
                m.usage = Some(acc);
            }
        }
    }
    let agent_ids = link_tasks(&mut conv.messages, &results, &notes);
    (conv, agent_ids)
}

/// Parts kept per imported subagent transcript.
const MAX_NESTED_PARTS: usize = 400;
/// Subagent transcripts larger than this are not inlined.
const MAX_SUBAGENT_BYTES: u64 = 48 * 1024 * 1024;

/// A `<task-notification>`: Claude Code reporting background work finishing.
#[derive(Debug, Default, Clone, PartialEq)]
struct TaskNote {
    task_id: Option<String>,
    tool_use_id: Option<String>,
    status: Option<String>,
}

fn task_notes(text: &str) -> Vec<TaskNote> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("<task-notification>") {
        let after = &rest[i + "<task-notification>".len()..];
        let end = after.find("</task-notification>").unwrap_or(after.len());
        let block = &after[..end];
        let tag = |name: &str| between(block, &format!("<{name}>"), &format!("</{name}>")).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        out.push(TaskNote { task_id: tag("task-id"), tool_use_id: tag("tool-use-id"), status: tag("status").map(|v| v.to_lowercase()) });
        rest = &after[end..];
    }
    out
}

/// "Command running in background with ID: b8b97ntfm." → `b8b97ntfm`.
fn background_id(output: &str) -> Option<String> {
    let i = output.find("with ID:")? + "with ID:".len();
    let id: String = output[i..].trim_start().chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect();
    (!id.is_empty()).then_some(id)
}

fn walk_tools(messages: &mut [Message], f: &mut dyn FnMut(&mut ToolCall)) {
    for m in messages.iter_mut() {
        for t in m.tool_calls_mut() {
            f(t);
            walk_tools(&mut t.subagent, f);
        }
    }
}

/// Apply subagent facts (duration, type, async) from tool results and the
/// final state of background tasks (TaskStop / KillShell, notifications).
fn link_tasks(messages: &mut [Message], results: &[(String, Value)], notes: &[TaskNote]) -> HashMap<String, String> {
    let mut agent_ids = HashMap::new();
    let mut tasks: HashMap<String, String> = HashMap::new();
    for (id, m) in results {
        let Some(t) = find_tool_deep_mut(messages, id) else { continue };
        if let Some(d) = m["totalDurationMs"].as_u64() {
            t.duration_ms = Some(d);
        }
        if let Some(a) = s(m, "agentType").filter(|a| !a.is_empty()) {
            t.agent_name = Some(a.to_string());
        }
        if m["isAsync"].as_bool().unwrap_or(false) || s(m, "status") == Some("async_launched") {
            t.background = true;
        }
        if let Some(a) = s(m, "agentId") {
            agent_ids.insert(a.to_string(), id.clone());
            if t.background {
                tasks.insert(a.to_string(), id.clone());
            }
        }
        for k in ["backgroundTaskId", "taskId", "shellId"] {
            if let Some(x) = s(m, k) {
                tasks.insert(x.to_string(), id.clone());
            }
        }
    }
    let mut stops = Vec::new();
    walk_tools(messages, &mut |t| {
        if t.background {
            if let Some(x) = t.output.as_deref().and_then(background_id) {
                tasks.entry(x).or_insert_with(|| t.id.clone());
            }
        }
        if matches!(t.name.as_str(), "TaskStop" | "KillShell" | "KillBash") && !t.is_error {
            if let Some(x) = ["task_id", "shell_id", "bash_id"].iter().find_map(|k| t.input.get(*k).and_then(|v| v.as_str())) {
                stops.push(x.to_string());
            }
        }
    });
    let mut finals: Vec<(String, ToolStatus)> = stops.into_iter().filter_map(|x| tasks.get(&x).map(|id| (id.clone(), ToolStatus::Cancelled))).collect();
    for n in notes {
        let Some(id) = n.tool_use_id.clone().or_else(|| n.task_id.as_ref().and_then(|x| tasks.get(x).cloned())) else { continue };
        let status = match n.status.as_deref().unwrap_or("completed") {
            "failed" | "error" => ToolStatus::Failed,
            "killed" | "stopped" | "cancelled" | "canceled" => ToolStatus::Cancelled,
            _ => ToolStatus::Completed,
        };
        finals.push((id, status));
    }
    for (id, status) in finals {
        if let Some(t) = find_tool_deep_mut(messages, &id) {
            t.status = status;
        }
    }
    agent_ids
}

/// Shape an imported subagent transcript: the prompt already lives in the
/// tool input, nothing is still running, and the size is bounded.
fn nested(mut msgs: Vec<Message>) -> Vec<Message> {
    let lead = msgs.iter().take_while(|m| m.role == Role::User).count();
    msgs.drain(..lead);
    msgs.retain(|m| !(m.role == Role::Assistant && m.parts.is_empty()));
    for m in msgs.iter_mut() {
        m.sidechain = true;
    }
    close_active_tools(&mut msgs, ToolStatus::Completed);
    slim_transcript(&mut msgs, MAX_NESTED_PARTS);
    msgs
}

/// Older Claude Code versions wrote subagent traffic into the main file as
/// `isSidechain` chains; link each chain to the Task call with its prompt.
fn attach_inline_sidechains(messages: &mut [Message], lines: &[Value]) {
    let mut root_of: HashMap<&str, usize> = HashMap::new();
    let mut groups: Vec<(usize, Vec<usize>)> = Vec::new();
    for (i, v) in lines.iter().enumerate() {
        if !v["isSidechain"].as_bool().unwrap_or(false) || !matches!(s(v, "type"), Some("user") | Some("assistant")) {
            continue;
        }
        let root = s(v, "parentUuid").and_then(|p| root_of.get(p).copied()).unwrap_or(i);
        if let Some(u) = s(v, "uuid") {
            root_of.insert(u, root);
        }
        match groups.iter_mut().find(|(r, _)| *r == root) {
            Some((_, g)) => g.push(i),
            None => groups.push((root, vec![i])),
        }
    }
    for (root, idxs) in groups {
        if s(&lines[root], "type") != Some("user") {
            continue;
        }
        let prompt = clean_user_text(&content_text(&lines[root]["message"]["content"]));
        if prompt.is_empty() {
            continue;
        }
        let matches = |t: &ToolCall| t.is_subagent() && t.subagent.is_empty() && t.input.get("prompt").and_then(|p| p.as_str()).map(|p| p.trim() == prompt.trim()).unwrap_or(false);
        if !messages.iter().any(|m| m.tool_calls().any(matches)) {
            continue;
        }
        let sub: Vec<Value> = idxs.iter().map(|&i| lines[i].clone()).collect();
        let msgs = nested(convert(&sub, None, true).0.messages);
        if let Some(t) = messages.iter_mut().flat_map(|m| m.tool_calls_mut()).find(|t| matches(t)) {
            t.subagent = msgs;
        }
    }
}

/// Inline `<sessionId>/subagents/agent-*.jsonl` into their spawning tool calls.
fn attach_subagent_files(messages: &mut [Message], session_path: &Path, agent_ids: &HashMap<String, String>) {
    let dir = session_path.with_extension("").join("subagents");
    let Ok(rd) = std::fs::read_dir(&dir) else { return };
    struct Pending {
        agent_id: String,
        tool_use_id: Option<String>,
        agent_type: Option<String>,
        msgs: Vec<Message>,
    }
    let mut ids = agent_ids.clone();
    let mut pending = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(agent_id) = name.strip_prefix("agent-").and_then(|n| n.strip_suffix(".jsonl")) else { continue };
        let p = e.path();
        if e.metadata().map(|m| m.len() > MAX_SUBAGENT_BYTES).unwrap_or(true) {
            continue;
        }
        let meta: Value = std::fs::read_to_string(p.with_extension("meta.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
        let Ok(text) = util::read_text(&p) else { continue };
        let (conv, inner_ids) = convert(&json_lines(&text), None, true);
        ids.extend(inner_ids);
        pending.push(Pending {
            agent_id: agent_id.to_string(),
            tool_use_id: string(&meta, "toolUseId"),
            agent_type: string(&meta, "agentType").filter(|a| !a.is_empty()),
            msgs: nested(conv.messages),
        });
    }
    // Subagents can spawn subagents: attach until nothing more links.
    loop {
        let mut progress = false;
        for p in pending.iter_mut().filter(|p| !p.msgs.is_empty()) {
            let Some(tid) = p.tool_use_id.clone().or_else(|| ids.get(&p.agent_id).cloned()) else { continue };
            if let Some(t) = find_tool_deep_mut(messages, &tid).filter(|t| t.subagent.is_empty()) {
                t.subagent = std::mem::take(&mut p.msgs);
                if t.agent_name.is_none() {
                    t.agent_name = p.agent_type.clone();
                }
                progress = true;
            }
        }
        if !progress {
            break;
        }
    }
}

/// Claude's `toolUseResult` carries structured edit info.
pub fn diffs_from_result(meta: &Value) -> Vec<FileDiff> {
    let Some(path) = meta.get("filePath").and_then(|p| p.as_str()) else { return Vec::new() };
    if let Some(hunks) = meta.get("structuredPatch").and_then(|h| h.as_array()) {
        let mut unified = String::new();
        for h in hunks {
            unified.push_str(&format!(
                "@@ -{},{} +{},{} @@\n",
                h["oldStart"].as_u64().unwrap_or(0),
                h["oldLines"].as_u64().unwrap_or(0),
                h["newStart"].as_u64().unwrap_or(0),
                h["newLines"].as_u64().unwrap_or(0)
            ));
            for l in h["lines"].as_array().into_iter().flatten() {
                unified.push_str(l.as_str().unwrap_or(""));
                unified.push('\n');
            }
        }
        if !unified.is_empty() {
            return vec![FileDiff::from_unified(path, unified)];
        }
    }
    if meta.get("type").and_then(|t| t.as_str()) == Some("create") {
        let content = meta.get("content").and_then(|c| c.as_str()).map(String::from);
        return vec![FileDiff::from_texts(path, None, content)];
    }
    Vec::new()
}

// ---------------------------------------------------------------- export

/// Render a session as a Claude Code transcript with id `sid`.
pub fn export_lines(session: &Session, sid: &str) -> String {
    let cwd = util::session_cwd(session);
    let branch = session.workspace.as_ref().and_then(|w| w.branch.clone()).unwrap_or_default();
    let mut out = String::new();
    let mut parent: Option<String> = None;
    let fallback_model = session.model.clone().filter(|m| m.starts_with("claude")).unwrap_or_else(|| "claude-sonnet-4-5".into());
    for (i, (role, text, ts, model)) in util::turns(session).into_iter().enumerate() {
        let uuid = uuid::Uuid::new_v4().to_string();
        let timestamp = ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let mut line = json!({
            "parentUuid": parent,
            "isSidechain": false,
            "userType": "external",
            "entrypoint": "cli",
            "cwd": cwd,
            "sessionId": sid,
            "version": "2.1.310",
            "gitBranch": branch,
        });
        let obj = line.as_object_mut().unwrap();
        match role {
            Role::User => {
                obj.insert("type".into(), json!("user"));
                obj.insert("message".into(), json!({"role": "user", "content": text}));
            }
            _ => {
                let model = model.filter(|m| m.starts_with("claude")).unwrap_or_else(|| fallback_model.clone());
                obj.insert("type".into(), json!("assistant"));
                obj.insert(
                    "message".into(),
                    json!({
                        "id": format!("msg_larrier_{i:04}"),
                        "type": "message",
                        "role": "assistant",
                        "model": model,
                        "content": [{"type": "text", "text": text}],
                        "stop_reason": "end_turn",
                        "stop_sequence": null,
                        "usage": {"input_tokens": 0, "output_tokens": 0}
                    }),
                );
            }
        }
        obj.insert("uuid".into(), json!(uuid));
        obj.insert("timestamp".into(), json!(timestamp));
        out.push_str(&line.to_string());
        out.push('\n');
        parent = Some(uuid);
    }
    out.push_str(&json!({"type": "custom-title", "customTitle": session.title, "sessionId": sid}).to_string());
    out.push('\n');
    out
}

pub fn export_native(session: &Session, home_override: Option<&Path>) -> Result<NativeLink> {
    let sid = uuid::Uuid::new_v4().to_string();
    let cwd = util::session_cwd(session);
    let path = root(home_override).join("projects").join(encode_project(&cwd)).join(format!("{sid}.jsonl"));
    util::write_new_file(&path, &export_lines(session, &sid))?;
    let _ = Utc::now();
    Ok(NativeLink {
        agent: AGENT,
        id: sid,
        path: Some(path.to_string_lossy().to_string()),
        synced_messages: session.messages.len(),
        account_id: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_paths() {
        assert_eq!(encode_project(r"D:\src\Projects\Server\Larrier"), "D--src-Projects-Server-Larrier");
        assert_eq!(encode_project(r"C:\Users\dev"), "C--Users-dev");
    }

    #[test]
    fn parses_split_assistant_and_tools() {
        let lines = [
            r#"{"type":"user","uuid":"u1","parentUuid":null,"sessionId":"S","cwd":"/r","gitBranch":"main","version":"2.1.0","timestamp":"2026-10-01T10:00:00Z","message":{"role":"user","content":"fix the bug"}}"#,
            r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","sessionId":"S","timestamp":"2026-10-01T10:00:01Z","message":{"id":"m1","model":"claude-x","role":"assistant","content":[{"type":"thinking","thinking":"hmm"}],"usage":{"input_tokens":5,"output_tokens":1}}}"#,
            r#"{"type":"assistant","uuid":"a2","parentUuid":"a1","sessionId":"S","timestamp":"2026-10-01T10:00:02Z","message":{"id":"m1","model":"claude-x","role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Edit","input":{"file_path":"/r/a.rs","old_string":"a","new_string":"b"}}],"usage":{"input_tokens":5,"output_tokens":9}}}"#,
            r#"{"type":"user","uuid":"u2","parentUuid":"a2","sessionId":"S","timestamp":"2026-10-01T10:00:03Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]},"toolUseResult":{"filePath":"/r/a.rs","structuredPatch":[{"oldStart":1,"oldLines":1,"newStart":1,"newLines":1,"lines":["-a","+b"]}]}}"#,
            r#"{"type":"assistant","uuid":"a3","parentUuid":"u2","sessionId":"S","timestamp":"2026-10-01T10:00:04Z","message":{"id":"m2","model":"claude-x","role":"assistant","content":[{"type":"text","text":"Done."}],"usage":{"input_tokens":1,"output_tokens":2}}}"#,
            r#"{"type":"user","uuid":"x9","parentUuid":"u1","sessionId":"S","timestamp":"2026-10-01T09:59:00Z","message":{"role":"user","content":"abandoned branch"},"isSidechain":true}"#,
            r#"{"type":"ai-title","aiTitle":"Fix the bug","sessionId":"S"}"#,
        ];
        let s = parse(&lines.join("\n"), None, "S").unwrap();
        assert_eq!(s.title, "Fix the bug");
        assert_eq!(s.messages.len(), 2, "{:#?}", s.messages);
        let a = &s.messages[1];
        assert_eq!(a.tool_calls().count(), 1);
        let t = a.tool_calls().next().unwrap();
        assert_eq!(t.output.as_deref(), Some("ok"));
        assert_eq!((t.diffs[0].added, t.diffs[0].removed), (1, 1));
        assert!(a.text().contains("Done."));
        assert_eq!(s.usage.output_tokens, 11);
        assert_eq!(s.workspace.as_ref().unwrap().branch.as_deref(), Some("main"));
    }

    #[test]
    fn running_turns_queued_prompts_and_images() {
        let png = "iVBORw0KGgo=";
        let mut lines = vec![
            r#"{"type":"user","uuid":"u1","parentUuid":null,"sessionId":"S","cwd":"/r","timestamp":"2026-10-01T10:00:00Z","message":{"role":"user","content":"take a screenshot"}}"#.to_string(),
            r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","sessionId":"S","timestamp":"2026-10-01T10:00:01Z","message":{"id":"m1","role":"assistant","stop_reason":"tool_use","content":[{"type":"tool_use","id":"t1","name":"mcp__browser__screenshot","input":{}}]}}"#.to_string(),
            format!(r#"{{"type":"user","uuid":"u2","parentUuid":"a1","sessionId":"S","timestamp":"2026-10-01T10:00:02Z","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"t1","content":[{{"type":"text","text":"done"}},{{"type":"image","source":{{"type":"base64","media_type":"image/png","data":"{png}"}}}}]}}]}}}}"#),
            r#"{"type":"attachment","uuid":"x1","parentUuid":"u2","sessionId":"S","timestamp":"2026-10-01T10:00:03Z","attachment":{"type":"queued_command","prompt":"also check the footer","commandMode":"prompt","timestamp":"2026-10-01T10:00:03Z"}}"#.to_string(),
            r#"{"type":"assistant","uuid":"a2","parentUuid":"x1","sessionId":"S","timestamp":"2026-10-01T10:00:04Z","message":{"id":"m2","role":"assistant","stop_reason":"tool_use","content":[{"type":"tool_use","id":"t2","name":"Read","input":{"file_path":"/r/footer.html"}}]}}"#.to_string(),
        ];
        // A queued prompt that carries an image is stored as content blocks.
        lines.push(format!(r#"{{"type":"attachment","uuid":"x2","parentUuid":"a2","sessionId":"S","timestamp":"2026-10-01T10:00:04Z","attachment":{{"type":"queued_command","prompt":[{{"type":"image","source":{{"type":"base64","media_type":"image/png","data":"{png}"}}}},{{"type":"text","text":"and make it smooth"}}],"commandMode":"prompt"}}}}"#));
        let s = parse(&lines.join("\n"), None, "S").unwrap();
        assert!(s.external_turn, "a tool_use stop reason means the turn is still running");
        let with_image = s.messages.iter().find(|m| m.role == Role::User && m.text().contains("and make it smooth")).expect("queued prompt with an image");
        assert!(with_image.parts.iter().any(|p| matches!(p, Part::Image { .. })), "its image comes along");
        let users: Vec<String> = s.messages.iter().filter(|m| m.role == Role::User).map(|m| m.text()).collect();
        assert!(users.iter().any(|t| t.contains("also check the footer")), "queued prompt imported: {users:?}");
        let shot = s.messages.iter().flat_map(|m| m.tool_calls()).find(|t| t.id == "t1").unwrap();
        assert_eq!(shot.images.len(), 1);
        assert_eq!(shot.output.as_deref(), Some("done"));

        lines.push(r#"{"type":"assistant","uuid":"a3","parentUuid":"a2","sessionId":"S","timestamp":"2026-10-01T10:00:05Z","message":{"id":"m3","role":"assistant","stop_reason":"end_turn","content":[{"type":"text","text":"All good."}]}}"#.to_string());
        lines.push(r#"{"type":"system","subtype":"stop_hook_summary","uuid":"y1","parentUuid":"a3","sessionId":"S","timestamp":"2026-10-01T10:00:06Z"}"#.to_string());
        assert!(!parse(&lines.join("\n"), None, "S").unwrap().external_turn, "end_turn closes the turn");
    }

    #[test]
    fn pasted_image_references_become_images() {
        let dir = tempfile::tempdir().unwrap();
        let img = dir.path().join("1.png");
        std::fs::write(&img, b"png").unwrap();
        let text = format!("look at this\n[Image: source: {}]", img.display());
        let (t, parts) = crate::util::image_refs(&text);
        assert_eq!(t, "look at this");
        assert_eq!(parts.len(), 1);
        let (t, parts) = crate::util::image_refs("[Image: source: /no/such/missing.png] hi");
        assert!(parts.is_empty() && t.contains("missing.png"), "missing files stay as text");
    }

    #[test]
    fn links_subagent_files_and_background_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let session = dir.path().join("S.jsonl");
        let main = [
            r#"{"type":"user","uuid":"u1","parentUuid":null,"sessionId":"S","cwd":"/r","timestamp":"2026-10-01T10:00:00Z","message":{"role":"user","content":"audit and serve"}}"#,
            r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","sessionId":"S","timestamp":"2026-10-01T10:00:01Z","message":{"id":"m1","role":"assistant","content":[{"type":"tool_use","id":"toolu_agent","name":"Agent","input":{"description":"Audit parser","prompt":"Audit the parser","subagent_type":"Explore"}}]}}"#,
            r#"{"type":"user","uuid":"u2","parentUuid":"a1","sessionId":"S","timestamp":"2026-10-01T10:00:09Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_agent","content":[{"type":"text","text":"Parser is fine"}]}]},"toolUseResult":{"status":"completed","agentId":"abc123","agentType":"Explore","totalDurationMs":8000}}"#,
            r#"{"type":"assistant","uuid":"a2","parentUuid":"u2","sessionId":"S","timestamp":"2026-10-01T10:00:10Z","message":{"id":"m2","role":"assistant","content":[{"type":"tool_use","id":"toolu_bg","name":"Bash","input":{"command":"npm run dev","run_in_background":true}}]}}"#,
            r#"{"type":"user","uuid":"u3","parentUuid":"a2","sessionId":"S","timestamp":"2026-10-01T10:00:11Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_bg","content":"Command running in background with ID: bx1"}]},"toolUseResult":{"stdout":"","backgroundTaskId":"bx1"}}"#,
            r#"{"type":"user","uuid":"u4","parentUuid":"u3","sessionId":"S","timestamp":"2026-10-01T10:00:30Z","message":{"role":"user","content":"<task-notification>\n<task-id>bx1</task-id>\n<status>failed</status>\n<summary>exit 1</summary>\n</task-notification>"}}"#,
            r#"{"type":"assistant","uuid":"a3","parentUuid":"u4","sessionId":"S","timestamp":"2026-10-01T10:00:31Z","message":{"id":"m3","role":"assistant","content":[{"type":"text","text":"The dev server crashed."}]}}"#,
        ];
        std::fs::write(&session, main.join("\n")).unwrap();
        let subs = dir.path().join("S").join("subagents");
        std::fs::create_dir_all(&subs).unwrap();
        let sub = [
            r#"{"type":"user","uuid":"s1","parentUuid":null,"isSidechain":true,"agentId":"abc123","sessionId":"S","timestamp":"2026-10-01T10:00:02Z","message":{"role":"user","content":"Audit the parser"}}"#,
            r#"{"type":"assistant","uuid":"s2","parentUuid":"s1","isSidechain":true,"agentId":"abc123","sessionId":"S","timestamp":"2026-10-01T10:00:03Z","message":{"id":"sm1","role":"assistant","content":[{"type":"tool_use","id":"toolu_read","name":"Read","input":{"file_path":"/r/parser.rs"}}]}}"#,
            r#"{"type":"user","uuid":"s3","parentUuid":"s2","isSidechain":true,"agentId":"abc123","sessionId":"S","timestamp":"2026-10-01T10:00:04Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_read","content":"fn parse()"}]}}"#,
            r#"{"type":"assistant","uuid":"s4","parentUuid":"s3","isSidechain":true,"agentId":"abc123","sessionId":"S","timestamp":"2026-10-01T10:00:05Z","message":{"id":"sm2","role":"assistant","content":[{"type":"text","text":"Parser is fine"}]}}"#,
        ];
        std::fs::write(subs.join("agent-abc123.jsonl"), sub.join("\n")).unwrap();
        std::fs::write(subs.join("agent-abc123.meta.json"), r#"{"agentType":"Explore","description":"Audit parser","toolUseId":"toolu_agent"}"#).unwrap();

        let s = import(&util::info(AGENT, "S", &session)).unwrap();
        let tools: Vec<&ToolCall> = s.messages.iter().flat_map(|m| m.tool_calls()).collect();
        let agent = tools.iter().find(|t| t.id == "toolu_agent").unwrap();
        assert!(agent.is_subagent());
        assert_eq!(agent.agent_name.as_deref(), Some("Explore"));
        assert_eq!(agent.duration_ms, Some(8000));
        assert_eq!(agent.subagent.len(), 1, "prompt dropped, one assistant message: {:#?}", agent.subagent);
        assert!(agent.subagent[0].sidechain);
        assert_eq!(agent.subagent_steps(), 1);
        let read = agent.subagent[0].tool_calls().next().unwrap();
        assert_eq!((read.output.as_deref(), read.status), (Some("fn parse()"), ToolStatus::Completed));
        assert_eq!(agent.subagent[0].text(), "Parser is fine");
        let bg = tools.iter().find(|t| t.id == "toolu_bg").unwrap();
        assert!(bg.background);
        assert_eq!(bg.status, ToolStatus::Failed, "notification status wins");
        assert!(!s.messages.iter().any(|m| m.text().contains("task-notification")));
    }

    #[test]
    fn links_inline_sidechains_by_prompt() {
        let lines = [
            r#"{"type":"user","uuid":"u1","parentUuid":null,"sessionId":"S","cwd":"/r","timestamp":"2026-10-01T10:00:00Z","message":{"role":"user","content":"look around"}}"#,
            r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","sessionId":"S","timestamp":"2026-10-01T10:00:01Z","message":{"id":"m1","role":"assistant","content":[{"type":"tool_use","id":"t_task","name":"Task","input":{"description":"Scan","prompt":"Scan the repo"}}]}}"#,
            r#"{"type":"user","uuid":"x1","parentUuid":null,"isSidechain":true,"sessionId":"S","timestamp":"2026-10-01T10:00:02Z","message":{"role":"user","content":"Scan the repo"}}"#,
            r#"{"type":"assistant","uuid":"x2","parentUuid":"x1","isSidechain":true,"sessionId":"S","timestamp":"2026-10-01T10:00:03Z","message":{"id":"sx","role":"assistant","content":[{"type":"text","text":"3 crates"}]}}"#,
            r#"{"type":"user","uuid":"u2","parentUuid":"a1","sessionId":"S","timestamp":"2026-10-01T10:00:04Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t_task","content":"3 crates"}]}}"#,
        ];
        let s = parse(&lines.join("\n"), None, "S").unwrap();
        let t = s.messages.iter().flat_map(|m| m.tool_calls()).find(|t| t.id == "t_task").unwrap();
        assert_eq!(t.subagent.len(), 1);
        assert_eq!(t.subagent[0].text(), "3 crates");
        assert_eq!(s.messages.len(), 2, "sidechain lines stay out of the main transcript");
    }

    #[test]
    fn export_roundtrip() {
        let mut s = Session::new(AGENT, Some(larrier_core::WorkspaceRef::from_path("/tmp/x")));
        s.push(larrier_core::Message::user_text("hello"));
        let mut a = larrier_core::Message::assistant(AGENT, None);
        a.push_text("world");
        s.push(a);
        let text = export_lines(&s, "SID");
        let back = parse(&text, None, "SID").unwrap();
        assert_eq!(back.messages.len(), 2);
        assert_eq!(back.messages[0].text(), "hello");
        assert_eq!(back.messages[1].text(), "world");
    }
}
