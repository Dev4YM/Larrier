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
use larrier_core::lsf::{close_active_tools, find_tool_deep_mut, is_narration_block, slim_transcript, title_from_prompt};
use larrier_core::{AgentKind, FileDiff, Message, NativeLink, Part, Role, Session, ToolCall, ToolKind, ToolStatus, Usage};
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

/// The default folder, then the folders of accounts that sign in from one
/// of their own.
pub fn roots() -> Vec<PathBuf> {
    let default = root(None);
    let extra = crate::account_homes(&AGENT, &default);
    std::iter::once(default).chain(extra.into_iter().map(|(p, _)| p)).collect()
}

pub fn discover() -> Vec<NativeSessionInfo> {
    let mut out = Vec::new();
    for home in roots() {
        let Ok(dirs) = std::fs::read_dir(home.join("projects")) else { continue };
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
    info.account = crate::account_of(&AGENT, p);
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

/// A command run in Claude Code itself (`/login`, `/model`), its output or
/// its caveat: the model never sees it, so it opens no turn.
fn local_command(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with("<command-name>") || t.starts_with("<local-command-") || t.starts_with("Caveat: The messages below were generated")
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
        || is_envelope(t)
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

/// `<agent-message from=…>` (a subagent or another session reporting back)
/// or `<cross-session-message …>`, optionally after the "Another Claude
/// session sent a message:" lead-in.
fn is_envelope(t: &str) -> bool {
    let t = t.trim_start();
    let t = t.strip_prefix("Another Claude session sent a message").map(|r| r.split_once('\n').map(|(_, b)| b).unwrap_or("").trim_start()).unwrap_or(t);
    t.starts_with("<agent-message") || t.starts_with("<cross-session-message")
}

/// Notice for a message another session or a subagent delivered into this
/// one (`origin.kind == "peer"`, or a bare envelope from older versions).
/// These arrive mid-turn: they are not prompts and don't open a turn.
fn peer_notice(text: &str, origin: &Value) -> Option<String> {
    if s(origin, "kind") != Some("peer") && !is_envelope(text) {
        return None;
    }
    let handback = origin["handback"].as_bool().unwrap_or(false) || text.contains("[Subagent hand-back]");
    let name = s(origin, "name").map(str::trim).filter(|n| !n.is_empty());
    Some(match (handback, name) {
        (true, Some(n)) => format!("Subagent reported back: {n}"),
        (true, None) => "Subagent reported back".to_string(),
        (false, Some(n)) => format!("Message from {n}"),
        (false, None) => "Message from another session".to_string(),
    })
}

/// First line of a slash command's output (`/model`, `/goal`, …), without
/// the `<local-command-stdout>` wrapper or terminal colour codes.
fn local_output(text: &str) -> Option<String> {
    if text.contains("<command-name>") {
        // The command itself (recorded this way for some local commands).
        return Some(clean_user_text(text)).filter(|c| !c.is_empty());
    }
    let t = strip_ansi(text);
    let t = between(&t, "<local-command-stdout>", "</local-command-stdout>").or_else(|| between(&t, "<local-command-stderr>", "</local-command-stderr>")).unwrap_or(t);
    let first = t.lines().map(str::trim).find(|l| !l.is_empty())?;
    if matches!(first, "{" | "[") {
        // JSON output: one squeezed line instead of a lone brace.
        return Some(short(&t.split_whitespace().collect::<Vec<_>>().join(" "), 160));
    }
    Some(short(first, 300))
}

fn strip_ansi(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut chars = t.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn short(t: &str, max: usize) -> String {
    let t = t.trim();
    if t.chars().count() <= max {
        return t.to_string();
    }
    t.chars().take(max - 1).collect::<String>() + "…"
}

/// Notice for a `system` record, or `None` for subtypes ignored on purpose:
/// `api_error` (a retry attempt; a final failure is an `isApiErrorMessage`
/// assistant record), `turn_duration`, `stop_hook_summary` without errors.
fn system_notice(v: &Value) -> Option<String> {
    let content = s(v, "content").map(str::trim).filter(|c| !c.is_empty());
    match s(v, "subtype")? {
        "compact_boundary" => Some("Context compacted".to_string()),
        "local_command" => content.and_then(local_output),
        "informational" => content.map(|c| short(c, 300)),
        "model_refusal_fallback" => Some(content.map(|c| short(c, 300)).unwrap_or_else(|| "Safeguards flagged this message; switched to the fallback model".to_string())),
        "model_refusal_no_fallback" => Some(content.map(|c| short(c, 300)).unwrap_or_else(|| {
            let why = s(v, "apiRefusalExplanation").map(|e| format!(": {}", short(e, 200))).unwrap_or_default();
            format!("Safeguards stopped this response{why}")
        })),
        "away_summary" => content.map(|c| format!("Recap: {}", short(c.trim_end_matches("(disable recaps in /config)").trim(), 400))),
        "stop_hook_summary" => {
            let errors = v["hookErrors"].as_array().filter(|e| !e.is_empty());
            if let Some(e) = errors {
                let first = content_text(&e[0]);
                let first = if first.is_empty() { e[0].to_string() } else { first };
                Some(format!("Stop hook error: {}", short(&first, 200)))
            } else if v["preventedContinuation"].as_bool().unwrap_or(false) {
                Some(s(v, "stopReason").filter(|r| !r.trim().is_empty()).map(|r| format!("Stopped by hook: {}", short(r, 200))).unwrap_or_else(|| "Stopped by hook".to_string()))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Claude Code tools the generic classifier gets wrong: the task-list, team
/// and cron tools read as file writes, `ToolSearch` loads tool schemas and
/// `ReadNotifications` / `ReadMcpResourceTool` read no file.
pub fn fix_tool_kind(t: &mut ToolCall) {
    const OTHER: [&str; 13] = [
        "TaskCreate", "TaskUpdate", "TaskGet", "TaskList", "TeamCreate", "TeamDelete", "CronCreate", "CronDelete", "CronList", "ToolSearch", "ReadNotifications", "ReadMcpResourceTool", "ListMcpResourcesTool",
    ];
    if OTHER.contains(&t.name.as_str()) && t.kind != ToolKind::Other {
        t.kind = ToolKind::Other;
        t.diffs.clear();
        t.title = Some(t.describe());
    }
}

/// `tool_reference` blocks (ToolSearch loading deferred tools) as text.
pub fn loaded_tools(content: &Value) -> Option<String> {
    let names: Vec<&str> = content.as_array()?.iter().filter(|b| s(b, "type") == Some("tool_reference")).filter_map(|b| s(b, "tool_name")).collect();
    (!names.is_empty()).then(|| format!("Loaded {}", names.join(", ")))
}

/// A server tool's result embedded in the assistant message
/// (`web_search_tool_result`, `web_fetch_tool_result`, …): tool id, text,
/// and whether it failed.
pub fn server_tool_result(b: &Value) -> Option<(String, String, bool)> {
    if !s(b, "type")?.ends_with("_tool_result") {
        return None;
    }
    let id = s(b, "tool_use_id")?.to_string();
    let c = &b["content"];
    let is_error = c.get("error_code").is_some() || s(c, "type").is_some_and(|t| t.ends_with("_error"));
    let text = match c.as_array() {
        Some(items) => items
            .iter()
            .map(|i| match (s(i, "title"), s(i, "url")) {
                (Some(t), Some(u)) => format!("{t} — {u}"),
                (None, Some(u)) => u.to_string(),
                _ => content_text(i),
            })
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        None if is_error => s(c, "error_code").unwrap_or("error").replace('_', " "),
        None => content_text(c),
    };
    Some((id, text, is_error))
}

/// Parse a Claude Code session JSONL.
pub fn parse(text: &str, path: Option<&Path>, fallback_id: &str) -> Result<Session> {
    let lines = json_lines(text);
    let mut meta = Meta { native_id: fallback_id.to_string(), path: path.map(PathBuf::from), ..Default::default() };
    let mut custom_title = None;
    let mut ai_title = None;
    let mut leaf: Option<String> = None;
    let mut permission_mode: Option<String> = None;
    let mut prs: Vec<Value> = Vec::new();
    let mut relocated: Option<String> = None;
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
            Some("relocated") => relocated = string(v, "relocatedCwd").or(relocated.take()),
            Some("pr-link") => {
                if let Some(url) = s(v, "prUrl") {
                    prs.retain(|p| p["url"] != url);
                    prs.push(json!({"number": v["prNumber"], "url": url, "repository": v["prRepository"]}));
                }
            }
            _ => {}
        }
        // `permission-mode` records and prompts both carry the mode in force.
        if let Some(m) = s(v, "permissionMode") {
            permission_mode = Some(m.to_string());
        }
        if let Some(u) = s(v, "uuid") {
            index.insert(u.to_string(), i);
        }
    }
    meta.title = custom_title.or(ai_title);
    // The session was moved to another folder; resume it there.
    if relocated.is_some() {
        meta.cwd = relocated;
    }

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

    // Only commands run in Claude Code itself (a terminal's `/login`):
    // nothing was said to the model, so there's no chat to show.
    let said = |v: &&Value| {
        let c = &v["message"]["content"];
        let text = match c {
            Value::String(t) => t.clone(),
            Value::Array(b) => b.iter().filter_map(|x| s(x, "text")).collect::<Vec<_>>().join(" "),
            _ => String::new(),
        };
        !v["isMeta"].as_bool().unwrap_or(false) && !local_command(&text)
    };
    let users = lines.iter().filter(|v| s(v, "type") == Some("user"));
    if !lines.iter().any(|v| s(v, "type") == Some("assistant")) && !users.clone().any(|v| said(&v)) && users.count() > 0 {
        return Err(util::skip("only commands run in Claude Code itself"));
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
    session.mode = permission_mode.as_deref().and_then(imported_mode).map(str::to_string);
    if let Some(m) = &session.mode {
        session.extra.insert("import_mode".into(), Value::String(m.clone()));
    }
    if !prs.is_empty() {
        session.extra.insert("pull_requests".into(), Value::Array(prs));
    }
    Ok(session)
}

/// The Larrier mode for a recorded Claude Code permission mode. Bypass is
/// never carried over: continuing in Larrier must not skip permission
/// prompts unless the user picks that mode here.
fn imported_mode(m: &str) -> Option<&'static str> {
    Some(match m {
        "auto" => "auto",
        "default" | "manual" => "default",
        "acceptEdits" => "acceptEdits",
        "plan" => "plan",
        _ => return None,
    })
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
                // Reports from subagents / other sessions are delivered
                // mid-turn; commands run in Claude Code itself get no reply.
                if peer_notice(&text, &v["origin"]).is_some() || local_command(&text) {
                    continue;
                }
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
                if s(a, "type") == Some("queued_command") && is_queued_prompt(a) {
                    open = true;
                }
            }
            _ => {}
        }
    }
    open
}

/// A `queued_command` attachment the user typed while the agent worked, not
/// a task notification, a peer report or another harness-injected message.
fn is_queued_prompt(a: &Value) -> bool {
    let prompt = content_text(&a["prompt"]);
    s(a, "commandMode") != Some("task-notification")
        && s(&a["origin"], "kind") != Some("task-notification")
        && !prompt.trim_start().starts_with("<task-notification>")
        && !a["isMeta"].as_bool().unwrap_or(false)
        && peer_notice(&prompt, &a["origin"]).is_none()
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
    // A safeguard-refusal fallback already announced the model switch that
    // the next assistant record's `fallback` block repeats.
    let mut switch_noted = false;
    let mut stop_noted: HashSet<String> = HashSet::new();
    // System records after the newest message hang off the active branch
    // without being on its parent chain.
    let uuid_idx: HashMap<&str, usize> = if active.is_some() { lines.iter().enumerate().filter_map(|(i, v)| s(v, "uuid").map(|u| (u, i))).collect() } else { HashMap::new() };
    let on_branch = |i: usize| -> bool {
        let Some(a) = active else { return true };
        let mut cur = i;
        for _ in 0..64 {
            if a.contains(&cur) {
                return true;
            }
            if cur != i && matches!(s(&lines[cur], "type"), Some("user") | Some("assistant")) {
                return false;
            }
            match s(&lines[cur], "parentUuid").and_then(|p| uuid_idx.get(p)) {
                Some(&j) => cur = j,
                None => return false,
            }
        }
        false
    };
    for (i, v) in lines.iter().enumerate() {
        let ty = s(v, "type").unwrap_or("");
        if ty == "system" {
            if on_branch(i) {
                if let Some(n) = system_notice(v) {
                    conv.notice(n, s(v, "timestamp").and_then(parse_ts));
                    switch_noted |= s(v, "subtype") == Some("model_refusal_fallback");
                }
            }
            continue;
        }
        // Attachments are context Claude Code adds around a turn (hook output,
        // edited-file snippets, @-mentioned file contents already named in the
        // prompt, reminders, mode and skill listings): ignored on purpose,
        // except queued prompts and background task status.
        if ty == "attachment" && !sidechain {
            let a = &v["attachment"];
            if s(a, "type") == Some("task_status") {
                // Status of background work, written when it changes state
                // (and again after compaction).
                let status = s(a, "status").map(str::to_lowercase).filter(|st| !matches!(st.as_str(), "running" | "pending"));
                if let Some(status) = status {
                    notes.push(TaskNote { task_id: string(a, "taskId"), tool_use_id: string(&a["shell"], "toolUseId"), status: Some(status) });
                }
            }
            // Prompts sent while the agent was working arrive as queued commands
            // (never as user records): show them where they were sent.
            if s(a, "type") == Some("queued_command") {
                // A plain string, or content blocks when the prompt carries
                // images ([{type: image, …}, {type: text, …}]).
                let prompt = content_text(&a["prompt"]);
                let blocks_images = image_parts(&a["prompt"]);
                let ts = s(a, "timestamp").or_else(|| s(v, "timestamp")).and_then(parse_ts);
                if let Some(n) = peer_notice(&prompt, &a["origin"]) {
                    conv.notice(n, ts);
                } else if s(a, "commandMode") == Some("task-notification") || prompt.trim_start().starts_with("<task-notification>") {
                    notes.extend(task_notes(&prompt));
                } else if is_queued_prompt(a) {
                    switch_noted = false;
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
            // Results of calls made side by side hang off their own call,
            // beside the branch: they still belong to it (and only fill in
            // calls that are on it).
            let results_only = v["message"]["content"].as_array().map(|b| !b.is_empty() && b.iter().all(|x| s(x, "type") == Some("tool_result"))).unwrap_or(false);
            if !a.contains(&i) && !(ty == "user" && results_only) {
                continue;
            }
        }
        let ts = s(v, "timestamp").and_then(parse_ts);
        if ty == "user" {
            let first_text = match &v["message"]["content"] {
                Value::String(t) => t.as_str(),
                Value::Array(b) => b.iter().find_map(|x| s(x, "text")).unwrap_or(""),
                _ => "",
            };
            if let Some(n) = peer_notice(first_text, &v["origin"]) {
                conv.notice(n, ts);
                continue;
            }
            // Harness-injected text: skill bodies, image metadata
            // ("[Image: original …]"), usage-limit notes, reminders.
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
                                if let Some(loaded) = loaded_tools(&b["content"]) {
                                    out = if out.trim().is_empty() { loaded } else { format!("{out}\n{loaded}") };
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
                switch_noted = false;
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
            for (i, b) in blocks.iter().enumerate() {
                match s(b, "type") {
                    Some("text") => conv.text(s(b, "text").unwrap_or(""), ts, model),
                    // Narration: words for the reader that the API sends as a
                    // thinking block (its signature, or Claude Code, says so).
                    Some("thinking") if is_narration_block(b) || v["narration_block_indexes"].as_array().is_some_and(|a| a.iter().any(|x| x.as_u64() == Some(i as u64))) => {
                        conv.text(s(b, "thinking").unwrap_or(""), ts, model)
                    }
                    Some("thinking") => {
                        let dur = v["thinkingDurationMs"].as_u64();
                        conv.reasoning(s(b, "thinking").unwrap_or(""), dur, ts, model)
                    }
                    Some("tool_use") | Some("server_tool_use") => {
                        let mut t = ToolCall::new(s(b, "id").unwrap_or(""), s(b, "name").unwrap_or("tool"), b.get("input").cloned().unwrap_or(Value::Null));
                        fix_tool_kind(&mut t);
                        t.started_at = ts;
                        conv.tool(t, ts, model);
                    }
                    // Model switch (`--fallback-model`, or a safeguard
                    // refusal already announced by its system record).
                    Some("fallback") => {
                        if !std::mem::take(&mut switch_noted) {
                            let from = s(&b["from"], "model").unwrap_or("the requested model");
                            let to = s(&b["to"], "model").unwrap_or("a fallback model");
                            conv.notice(format!("Switched from {from} to {to}"), ts);
                        }
                    }
                    // `redacted_thinking` is encrypted: nothing to show.
                    _ => {
                        if let Some((id, out, is_err)) = server_tool_result(b) {
                            conv.tool_result(&id, Some(out), is_err, Vec::new());
                        }
                    }
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
                context_tokens: Some(["input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens"].iter().map(|k| u[*k].as_u64().unwrap_or(0)).sum()),
                ..Default::default()
            };
            let idx = conv.messages.iter().rposition(|m| m.role == Role::Assistant).unwrap_or(0);
            if let Some(e) = usage_by_msg.iter_mut().find(|e| e.0 == mid) {
                e.1 = usage;
                e.2 = idx;
            } else {
                usage_by_msg.push((mid.to_string(), usage, idx));
            }
        }
        // Every block record of a message repeats its final stop reason:
        // note the cut-off once, after the message's last block. Refusals
        // come with their own system records.
        if s(msg, "stop_reason") == Some("max_tokens") {
            let mid = s(msg, "id").unwrap_or("");
            let more = lines[i + 1..].iter().find(|x| s(x, "type") == Some("assistant")).is_some_and(|x| s(&x["message"], "id") == Some(mid));
            if !more && stop_noted.insert(mid.to_string()) {
                conv.notice("Response hit the output token limit", ts);
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

/// Where a Claude Code session resumes so that one of its prompts, and
/// everything after it, are dropped.
#[derive(Debug, PartialEq)]
pub enum Rewind {
    /// Resume up to and including this chain entry (`--resume-session-at`).
    At(String),
    /// The prompt opens the conversation: start a new session instead.
    Fresh,
}

/// The rewind point for the prompt `later` prompts before the newest one on
/// the session's active branch. `text` is the prompt as Larrier shows it,
/// to make sure the right one was found; `None` when it can't be told.
/// The branch is followed back to the last compaction only: Claude Code
/// can't resume a conversation from before it.
pub fn rewind_point(path: &Path, later: usize, text: &str) -> Option<Rewind> {
    let raw = util::read_text(path).ok()?;
    let lines: Vec<Value> = raw.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let index: HashMap<&str, usize> = lines.iter().enumerate().filter_map(|(i, v)| s(v, "uuid").map(|u| (u, i))).collect();
    let on_branch = |v: &Value| matches!(s(v, "type"), Some("user") | Some("assistant")) && !v["isSidechain"].as_bool().unwrap_or(false);
    let leaf = lines.iter().rposition(|v| on_branch(v))?;
    // Prompts on the active branch, newest first.
    let mut prompts: Vec<usize> = Vec::new();
    let mut seen = HashSet::new();
    let mut cur = Some(leaf);
    while let Some(i) = cur {
        if !seen.insert(i) {
            break;
        }
        if prompt_text(&lines[i]).is_some() {
            prompts.push(i);
        }
        cur = s(&lines[i], "parentUuid").and_then(|p| index.get(p).copied());
    }
    let at = *prompts.get(later)?;
    let shown = prompt_text(&lines[at])?;
    let squash = |t: &str| t.split_whitespace().collect::<Vec<_>>().join(" ");
    let want: String = squash(text).chars().take(120).collect();
    if want.is_empty() || !squash(&shown).contains(&want) {
        return None;
    }
    if prompts.len() == later + 1 {
        return Some(Rewind::Fresh);
    }
    s(&lines[at], "parentUuid").map(|p| Rewind::At(p.to_string()))
}

/// The text of a prompt the user typed (not a tool result, a harness note
/// or another session's message).
fn prompt_text(v: &Value) -> Option<String> {
    if s(v, "type") != Some("user") || v["isSidechain"].as_bool().unwrap_or(false) || v["isMeta"].as_bool().unwrap_or(false) || v["isCompactSummary"].as_bool().unwrap_or(false) {
        return None;
    }
    let text = match &v["message"]["content"] {
        Value::String(t) => t.clone(),
        Value::Array(blocks) => {
            if blocks.iter().any(|b| s(b, "type") == Some("tool_result")) {
                return None;
            }
            blocks.iter().filter_map(|b| s(b, "text")).collect::<Vec<_>>().join("\n")
        }
        _ => return None,
    };
    if peer_notice(&text, &v["origin"]).is_some() {
        return None;
    }
    let t = clean_user_text(&text);
    (!t.is_empty() && !t.starts_with("[Request interrupted")).then_some(t)
}

/// The file of a Claude Code session by id: in its project's folder, else
/// wherever it is.
pub fn session_file(id: &str, cwd: Option<&str>, home_override: Option<&Path>) -> Option<PathBuf> {
    let projects = root(home_override).join("projects");
    let name = format!("{id}.jsonl");
    if let Some(p) = cwd.map(|c| projects.join(encode_project(c)).join(&name)).filter(|p| p.exists()) {
        return Some(p);
    }
    std::fs::read_dir(&projects).ok()?.flatten().map(|d| d.path().join(&name)).find(|p| p.exists())
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
        resume_at: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A terminal's `/login` is three records the model never saw: it's no
    /// chat, and a chat that ran one isn't left mid-turn by it.
    #[test]
    fn local_commands_are_no_chat_and_no_turn() {
        let login = [
            r#"{"type":"user","uuid":"u0","parentUuid":null,"isMeta":true,"sessionId":"S","cwd":"/r","timestamp":"2026-11-01T07:53:00Z","message":{"role":"user","content":"<local-command-caveat>The command below was run directly in Claude Code, not sent to you as a request.</local-command-caveat>"}}"#,
            r#"{"type":"user","uuid":"u1","parentUuid":"u0","sessionId":"S","timestamp":"2026-11-01T07:53:01Z","message":{"role":"user","content":"<command-name>/login</command-name>
<command-message>login</command-message>
<command-args></command-args>"}}"#,
            r#"{"type":"user","uuid":"u2","parentUuid":"u1","sessionId":"S","timestamp":"2026-11-01T07:53:28Z","message":{"role":"user","content":"<local-command-stdout>Login successful</local-command-stdout>"}}"#,
        ];
        let err = parse(&login.join("
"), None, "S").unwrap_err();
        assert!(err.downcast_ref::<util::ImportSkip>().is_some(), "{err}");
        // In a real chat, a /login after the reply leaves the turn closed.
        let mut chat = vec![
            r#"{"type":"user","uuid":"p1","parentUuid":null,"sessionId":"S","cwd":"/r","timestamp":"2026-11-01T07:50:00Z","message":{"role":"user","content":"hi"}}"#,
            r#"{"type":"assistant","uuid":"a1","parentUuid":"p1","sessionId":"S","timestamp":"2026-11-01T07:50:01Z","message":{"id":"m1","role":"assistant","stop_reason":"end_turn","content":[{"type":"text","text":"Hello"}]}}"#,
        ];
        chat.extend(login.iter().copied());
        let s = parse(&chat.join("
"), None, "S").unwrap();
        assert!(!s.external_turn, "a local command opens no turn");
    }

    /// A second account signed in from a folder of its own: its sessions are
    /// found there and run as that account.
    #[test]
    fn an_accounts_own_folder_is_read_too() {
        let home = tempfile::tempdir().unwrap();
        let proj = home.path().join("projects").join("C--work");
        std::fs::create_dir_all(&proj).unwrap();
        let mut s = Session::new(AGENT, None);
        s.push(Message::user_text("hello from the second account"));
        let mut a = Message::assistant(AGENT, None);
        a.push_text("Hi.");
        s.push(a);
        let id = "11111111-2222-3333-4444-555555555555";
        let file = proj.join(format!("{id}.jsonl"));
        std::fs::write(&file, export_lines(&s, id)).unwrap();
        crate::set_account_homes(vec![(AGENT, home.path().to_path_buf(), "acct-work".into())]);
        assert!(roots().iter().any(|r| r == home.path()), "{:?}", roots());
        let info = file_info(&file).unwrap();
        assert_eq!(info.account.as_deref(), Some("acct-work"));
        let imported = crate::import_native(&info).unwrap();
        assert_eq!(imported.account_id.as_deref(), Some("acct-work"));
        // The default folder listed again as an account isn't read twice.
        crate::set_account_homes(vec![(AGENT, root(None), "same".into())]);
        assert_eq!(roots().len(), 1);
        crate::set_account_homes(Vec::new());
    }

    /// Claude Code hangs the result of a call made beside another off that
    /// call, off the branch the conversation continues on.
    #[test]
    fn results_of_parallel_calls_are_kept() {
        let lines = [
            r#"{"type":"user","uuid":"u1","parentUuid":null,"sessionId":"S","message":{"role":"user","content":"check both"}}"#,
            r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","sessionId":"S","message":{"id":"m1","role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#,
            r#"{"type":"assistant","uuid":"a2","parentUuid":"a1","sessionId":"S","message":{"id":"m1","role":"assistant","content":[{"type":"tool_use","id":"t2","name":"Read","input":{"file_path":"/r/a.rs"}}]}}"#,
            r#"{"type":"user","uuid":"r1","parentUuid":"a1","sessionId":"S","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"a.rs"}]}}"#,
            r#"{"type":"user","uuid":"r2","parentUuid":"a2","sessionId":"S","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t2","content":"fn a()"}]}}"#,
            r#"{"type":"assistant","uuid":"a3","parentUuid":"r2","sessionId":"S","message":{"id":"m2","role":"assistant","content":[{"type":"text","text":"Both fine."}]}}"#,
        ];
        let s = parse(&lines.join("\n"), None, "S").unwrap();
        let out = |id: &str| s.messages.iter().flat_map(|m| m.tool_calls()).find(|t| t.id == id).and_then(|t| t.output.clone());
        assert_eq!(out("t1").as_deref(), Some("a.rs"));
        assert_eq!(out("t2").as_deref(), Some("fn a()"));
    }

    #[test]
    fn rewind_point_is_the_entry_before_the_prompt() {
        let lines = [
            r#"{"type":"user","uuid":"u1","parentUuid":null,"sessionId":"S","message":{"role":"user","content":"Add a multiply function"}}"#,
            r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","sessionId":"S","message":{"id":"m1","role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Edit","input":{}}]}}"#,
            r#"{"type":"user","uuid":"r1","parentUuid":"a1","sessionId":"S","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}"#,
            r#"{"type":"assistant","uuid":"a2","parentUuid":"r1","sessionId":"S","message":{"id":"m2","role":"assistant","content":[{"type":"text","text":"Done."}]}}"#,
            r#"{"type":"user","uuid":"x1","parentUuid":"a2","isMeta":true,"sessionId":"S","message":{"role":"user","content":"<system-reminder>note</system-reminder>"}}"#,
            r#"{"type":"user","uuid":"u2","parentUuid":"x1","sessionId":"S","message":{"role":"user","content":[{"type":"text","text":"Now   add tests\nfor it"}]}}"#,
            r#"{"type":"assistant","uuid":"a3","parentUuid":"u2","sessionId":"S","message":{"id":"m3","role":"assistant","content":[{"type":"text","text":"Added."}]}}"#,
            r#"{"type":"user","uuid":"u3","parentUuid":"a3","sessionId":"S","message":{"role":"user","content":"Thanks"}}"#,
        ];
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("S.jsonl");
        std::fs::write(&path, lines.join("\n")).unwrap();
        assert_eq!(rewind_point(&path, 0, "Thanks"), Some(Rewind::At("a3".into())));
        assert_eq!(rewind_point(&path, 1, "Now add tests for it"), Some(Rewind::At("x1".into())));
        assert_eq!(rewind_point(&path, 2, "Add a multiply function"), Some(Rewind::Fresh));
        assert_eq!(rewind_point(&path, 1, "Something else"), None, "the prompt must match");
        assert_eq!(rewind_point(&path, 3, "Thanks"), None);
    }

    #[test]
    fn encodes_paths() {
        assert_eq!(encode_project(r"D:\src\Projects\Server\Larrier"), "D--src-Projects-Server-Larrier");
        assert_eq!(encode_project(r"C:\Users\dev"), "C--Users-dev");
    }

    #[test]
    fn imports_mode_pull_requests_and_relocation() {
        let lines = [
            r#"{"type":"user","uuid":"u1","parentUuid":null,"sessionId":"S","cwd":"/old","permissionMode":"plan","timestamp":"2026-10-01T10:00:00Z","message":{"role":"user","content":"ship it"}}"#,
            r#"{"type":"permission-mode","permissionMode":"acceptEdits","sessionId":"S"}"#,
            r#"{"type":"pr-link","sessionId":"S","prNumber":7,"prUrl":"https://github.com/o/r/pull/7","prRepository":"o/r","timestamp":"2026-10-01T10:00:05Z"}"#,
            r#"{"type":"pr-link","sessionId":"S","prNumber":7,"prUrl":"https://github.com/o/r/pull/7","prRepository":"o/r","timestamp":"2026-10-01T10:00:06Z"}"#,
            r#"{"type":"relocated","sessionId":"S","relocatedCwd":"/new"}"#,
        ];
        let s = parse(&lines.join("
"), None, "S").unwrap();
        assert_eq!(s.mode.as_deref(), Some("acceptEdits"));
        assert_eq!(s.cwd().as_deref(), Some("/new"));
        let prs = s.extra["pull_requests"].as_array().unwrap();
        assert_eq!(prs.len(), 1);
        assert_eq!(prs[0]["number"], 7);

        // Bypass is never carried into Larrier.
        let bypass = [lines[0], r#"{"type":"permission-mode","permissionMode":"bypassPermissions","sessionId":"S"}"#];
        assert_eq!(parse(&bypass.join("
"), None, "S").unwrap().mode, None);
    }


    /// A thinking block's signature naming its kind, as the API writes it.
    fn signature(kind: &str) -> String {
        use base64::Engine;
        let mut b = vec![0x08, 0x04, 0x12, 0x60, 0x0a, 0x11, 0x08, 0x13, 0x18, 0x02, 0x38, 0x01, 0x42, kind.len() as u8];
        b.extend_from_slice(kind.as_bytes());
        b.extend((0..90u8).map(|i| i.wrapping_mul(37)));
        base64::engine::general_purpose::STANDARD.encode(b)
    }

    /// Claude Code stores narration as a thinking record; its signature
    /// says it's words for the reader.
    #[test]
    fn narration_records_import_as_text() {
        let rec = |uuid: &str, parent: &str, thinking: &str, sig: &str| {
            serde_json::json!({"type": "assistant", "uuid": uuid, "parentUuid": parent, "sessionId": "S", "timestamp": "2026-10-01T10:00:01Z",
                "message": {"id": "m1", "model": "claude-x", "role": "assistant", "content": [{"type": "thinking", "thinking": thinking, "signature": sig}]}, "thinkingDurationMs": 31260})
            .to_string()
        };
        let lines = [
            r#"{"type":"user","uuid":"u1","parentUuid":null,"sessionId":"S","cwd":"/r","timestamp":"2026-10-01T10:00:00Z","message":{"role":"user","content":"the background is ugly"}}"#.to_string(),
            rec("a1", "u1", "The user wants something more artistic.", &signature("thinking")),
            rec("a2", "a1", "Agreed, the rings felt cheap. I'll make a few options.", &signature("narration")),
        ];
        let s = parse(&lines.join("\n"), None, "S").unwrap();
        let parts = &s.messages[1].parts;
        assert!(matches!(&parts[0], Part::Reasoning { text, .. } if text.starts_with("The user wants")), "{parts:?}");
        assert!(matches!(&parts[1], Part::Text { text } if text.starts_with("Agreed")), "{parts:?}");
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
    fn peer_messages_are_notices_not_prompts() {
        let mut lines = vec![
            r#"{"type":"user","uuid":"u1","parentUuid":null,"sessionId":"S","cwd":"/r","timestamp":"2026-10-01T10:00:00Z","message":{"role":"user","content":"coordinate the audit"}}"#,
            r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","sessionId":"S","timestamp":"2026-10-01T10:00:01Z","message":{"id":"m1","role":"assistant","stop_reason":"end_turn","content":[{"type":"text","text":"Started the workers."}]}}"#,
            // Mid-turn hand-back, queued as an attachment.
            r#"{"type":"attachment","uuid":"x1","parentUuid":"a1","sessionId":"S","timestamp":"2026-10-01T10:00:02Z","attachment":{"type":"queued_command","commandMode":"prompt","isMeta":true,"prompt":"<agent-message from=\"local_1\">\n[Subagent hand-back] The text below is the final report of a subagent.\nAll checks pass.\n</agent-message>","origin":{"kind":"peer","from":"local_1","name":"Audit worker","handback":true,"body":"All checks pass."}}}"#,
            // Delivered while idle, as a meta user record.
            r#"{"type":"user","uuid":"u2","parentUuid":"x1","isMeta":true,"sessionId":"S","timestamp":"2026-10-01T10:00:03Z","origin":{"kind":"peer","from":"local_2","body":"ping"},"message":{"role":"user","content":"Another Claude session sent a message:\n<agent-message from=\"local_2\">ping</agent-message>"}}"#,
            // An older bare envelope with no origin.
            r#"{"type":"user","uuid":"u3","parentUuid":"u2","sessionId":"S","timestamp":"2026-10-01T10:00:04Z","message":{"role":"user","content":[{"type":"text","text":"<cross-session-message from=\"local_3\">status?</cross-session-message>"}]}}"#,
        ];
        let s = parse(&lines.join("\n"), None, "S").unwrap();
        let users: Vec<String> = s.messages.iter().filter(|m| m.role == Role::User).map(|m| m.text()).collect();
        assert_eq!(users, vec!["coordinate the audit".to_string()], "envelopes are not prompts");
        let notices: Vec<&str> = s.messages.iter().flat_map(|m| &m.parts).filter_map(|p| match p { Part::Notice { text } => Some(text.as_str()), _ => None }).collect();
        assert_eq!(notices, vec!["Subagent reported back: Audit worker", "Message from another session", "Message from another session"]);
        assert!(!s.external_turn, "peer reports don't open a turn");
        assert_eq!(clean_user_text("<agent-message from=\"x\">hi</agent-message>"), "");

        // A real queued prompt still opens one.
        lines.push(r#"{"type":"attachment","uuid":"x2","parentUuid":"u3","sessionId":"S","timestamp":"2026-10-01T10:00:05Z","attachment":{"type":"queued_command","commandMode":"prompt","prompt":"and the docs","origin":{"kind":"human"}}}"#);
        let s = parse(&lines.join("\n"), None, "S").unwrap();
        assert!(s.external_turn);
        assert!(s.messages.iter().any(|m| m.role == Role::User && m.text() == "and the docs"));
    }

    #[test]
    fn system_records_cutoffs_and_tool_fixups() {
        let lines = [
            r#"{"type":"user","uuid":"u1","parentUuid":null,"sessionId":"S","cwd":"/r","timestamp":"2026-10-01T10:00:00Z","message":{"role":"user","content":"<command-name>/model</command-name>\n<command-args>opus</command-args>"}}"#,
            r#"{"type":"system","subtype":"local_command","uuid":"y1","parentUuid":"u1","sessionId":"S","timestamp":"2026-10-01T10:00:01Z","content":"<local-command-stdout>Set model to \u001b[1mOpus\u001b[22m</local-command-stdout>","level":"info"}"#,
            r#"{"type":"system","subtype":"api_error","uuid":"y2","parentUuid":"y1","sessionId":"S","timestamp":"2026-10-01T10:00:02Z","level":"error","error":{"status":529,"message":"Overloaded"},"retryInMs":500,"retryAttempt":1,"maxRetries":10}"#,
            r#"{"type":"user","uuid":"u2","parentUuid":"y2","sessionId":"S","timestamp":"2026-10-01T10:00:03Z","message":{"role":"user","content":"plan the work"}}"#,
            r#"{"type":"system","subtype":"model_refusal_fallback","uuid":"y3","parentUuid":"u2","sessionId":"S","timestamp":"2026-10-01T10:00:04Z","content":"Safeguards flagged this message. Switched to Opus.","direction":"retry"}"#,
            r#"{"type":"assistant","uuid":"a1","parentUuid":"y3","sessionId":"S","timestamp":"2026-10-01T10:00:05Z","message":{"id":"m1","role":"assistant","stop_reason":"max_tokens","content":[{"type":"fallback","from":{"model":"claude-fable-5"},"to":{"model":"claude-opus-5"}},{"type":"tool_use","id":"t1","name":"TaskCreate","input":{"subject":"Write tests","description":"cover the importer"}}]}}"#,
            r#"{"type":"user","uuid":"u3","parentUuid":"a1","sessionId":"S","timestamp":"2026-10-01T10:00:06Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"Task #1 created"}]}}"#,
            r#"{"type":"assistant","uuid":"a2","parentUuid":"u3","sessionId":"S","timestamp":"2026-10-01T10:00:07Z","message":{"id":"m1","role":"assistant","stop_reason":"max_tokens","content":[{"type":"tool_use","id":"t2","name":"ToolSearch","input":{"query":"select:Monitor"}}]}}"#,
            r#"{"type":"user","uuid":"u4","parentUuid":"a2","sessionId":"S","timestamp":"2026-10-01T10:00:08Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t2","content":[{"type":"tool_reference","tool_name":"Monitor"}]}]}}"#,
            r#"{"type":"assistant","uuid":"a3","parentUuid":"u4","sessionId":"S","timestamp":"2026-10-01T10:00:09Z","message":{"id":"m2","role":"assistant","stop_reason":"tool_use","content":[{"type":"fallback","from":{"model":"claude-opus-5"},"to":{"model":"claude-sonnet-5"}},{"type":"tool_use","id":"t3","name":"Bash","input":{"command":"npm test","run_in_background":true}}]}}"#,
            r#"{"type":"user","uuid":"u5","parentUuid":"a3","sessionId":"S","timestamp":"2026-10-01T10:00:10Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t3","content":"Command running in background with ID: bt1"}]},"toolUseResult":{"backgroundTaskId":"bt1"}}"#,
            r#"{"type":"attachment","uuid":"x1","parentUuid":"u5","sessionId":"S","timestamp":"2026-10-01T10:00:11Z","attachment":{"type":"task_status","taskId":"bt1","taskType":"local_bash","status":"failed","description":"npm test","outputFilePath":"/tmp/bt1.output"}}"#,
            r#"{"type":"system","subtype":"informational","uuid":"y4","parentUuid":"x1","sessionId":"S","timestamp":"2026-10-01T10:00:12Z","content":"Backgrounding after the current tool finishes…","level":"warning"}"#,
            r#"{"type":"system","subtype":"turn_duration","uuid":"y5","parentUuid":"y4","sessionId":"S","timestamp":"2026-10-01T10:00:13Z","durationMs":13000}"#,
        ];
        let s = parse(&lines.join("\n"), None, "S").unwrap();
        let notices: Vec<&str> = s.messages.iter().flat_map(|m| &m.parts).filter_map(|p| match p { Part::Notice { text } => Some(text.as_str()), _ => None }).collect();
        assert_eq!(
            notices,
            vec![
                "Set model to Opus",
                "Safeguards flagged this message. Switched to Opus.",
                "Response hit the output token limit",
                "Switched from claude-opus-5 to claude-sonnet-5",
                "Backgrounding after the current tool finishes…",
            ],
            "api_error retries and turn_duration stay silent; the refusal's fallback block isn't repeated; one cut-off note per message"
        );
        assert_eq!(s.messages[0].text(), "/model opus");
        let tools: Vec<&ToolCall> = s.messages.iter().flat_map(|m| m.tool_calls()).collect();
        let create = tools.iter().find(|t| t.id == "t1").unwrap();
        assert_eq!(create.kind, ToolKind::Other, "TaskCreate is not a file write");
        assert!(!create.kind.is_mutation());
        let search = tools.iter().find(|t| t.id == "t2").unwrap();
        assert_eq!((search.kind, search.output.as_deref()), (ToolKind::Other, Some("Loaded Monitor")));
        let bg = tools.iter().find(|t| t.id == "t3").unwrap();
        assert_eq!(bg.status, ToolStatus::Failed, "task_status attachment closes background work");
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
