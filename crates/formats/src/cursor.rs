//! Cursor: the IDE / Agents Window store (`state.vscdb`), agent transcripts
//! (`~/.cursor/projects/*/agent-transcripts/<id>/<id>.jsonl`) and the CLI
//! chat store (`~/.cursor/chats/<hash>/<id>/store.db`).
//!
//! `state.vscdb` can be many gigabytes: only indexed key lookups and key
//! range scans are used, never `LIKE`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use larrier_core::lsf::{close_active_tools, slim_transcript, title_from_prompt, MediaSource};
use larrier_core::{AgentKind, FileDiff, Message, Part, Role, Session, ToolCall, ToolKind, ToolStatus, Usage};
use serde_json::Value;

use crate::util::{self, content_text, env_dir, finish, head_lines, json_lines, json_or_string, open_sqlite, parse_ts, s, string, ts_from_ms, Conv, Meta};
use crate::NativeSessionInfo;

const AGENT: AgentKind = AgentKind::Cursor;

pub fn config_dir() -> PathBuf {
    env_dir("CURSOR_CONFIG_DIR")
        .or_else(|| env_dir("XDG_CONFIG_HOME").map(|p| p.join("cursor")).filter(|p| p.exists()))
        .unwrap_or_else(|| util::home().join(".cursor"))
}

pub fn ide_db() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        env_dir("APPDATA").map(|p| p.join("Cursor"))
    } else if cfg!(target_os = "macos") {
        Some(util::home().join("Library").join("Application Support").join("Cursor"))
    } else {
        Some(env_dir("XDG_CONFIG_HOME").unwrap_or_else(|| util::home().join(".config")).join("Cursor"))
    }?;
    let p = base.join("User").join("globalStorage").join("state.vscdb");
    p.exists().then_some(p)
}

thread_local! {
    static CONNS: RefCell<HashMap<PathBuf, rusqlite::Connection>> = RefCell::new(HashMap::new());
}

fn with_db<R>(path: &Path, f: impl FnOnce(&rusqlite::Connection) -> Result<R>) -> Result<R> {
    CONNS.with(|c| {
        let mut map = c.borrow_mut();
        if !map.contains_key(path) {
            map.insert(path.to_path_buf(), open_sqlite(path)?);
        }
        f(map.get(path).unwrap())
    })
}

fn kv(con: &rusqlite::Connection, key: &str) -> Option<Value> {
    let raw: Option<Vec<u8>> = con
        .query_row("SELECT value FROM cursorDiskKV WHERE key = ?1", [key], |r| {
            let v: rusqlite::types::Value = r.get(0)?;
            Ok(match v {
                rusqlite::types::Value::Text(t) => Some(t.into_bytes()),
                rusqlite::types::Value::Blob(b) => Some(b),
                _ => None,
            })
        })
        .ok()
        .flatten();
    raw.and_then(|b| serde_json::from_slice(&b).ok())
}

pub fn discover() -> Vec<NativeSessionInfo> {
    let mut out = Vec::new();
    let mut ide_ids: HashSet<String> = HashSet::new();
    if let Some(db) = ide_db() {
        match discover_ide(&db) {
            Ok(list) => {
                for i in list {
                    ide_ids.insert(i.native_id.clone());
                    out.push(i);
                }
            }
            Err(e) => tracing::warn!("cursor ide discovery: {e}"),
        }
    }
    let cfg = config_dir();
    // Agent transcripts (Agents Window / CLI runs). Prefer the richer IDE copy.
    if let Ok(projects) = std::fs::read_dir(cfg.join("projects")) {
        for p in projects.flatten() {
            let tdir = p.path().join("agent-transcripts");
            let Ok(chats) = std::fs::read_dir(&tdir) else { continue };
            for c in chats.flatten() {
                let cp = c.path();
                let (id, file) = if cp.is_dir() {
                    let id = cp.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    (id.clone(), cp.join(format!("{id}.jsonl")))
                } else if cp.extension().map(|e| e == "jsonl").unwrap_or(false) {
                    (cp.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), cp.clone())
                } else {
                    continue;
                };
                if ide_ids.contains(&id) || !file.exists() {
                    continue;
                }
                let mut info = util::info(AGENT, id, &file);
                if info.size < 16 {
                    continue;
                }
                for l in head_lines(&file, 4) {
                    if let Ok(v) = serde_json::from_str::<Value>(&l) {
                        if s(&v, "role") == Some("user") {
                            let t = strip_query(&content_text(&v["message"]["content"]));
                            if !t.trim().is_empty() {
                                info.title = Some(title_from_prompt(&t));
                                break;
                            }
                        }
                    }
                }
                out.push(info);
            }
        }
    }
    // CLI chat stores.
    let chats = cfg.join("chats");
    if chats.exists() {
        for e in walkdir::WalkDir::new(&chats).max_depth(3).into_iter().flatten() {
            if e.file_name() == "store.db" {
                let id = e.path().parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                if ide_ids.contains(&id) {
                    continue;
                }
                let mut info = util::info(AGENT, id, e.path());
                if let Ok(m) = cli_meta(e.path()) {
                    info.title = chat_name(&m);
                }
                out.push(info);
            }
        }
    }
    out
}

fn discover_ide(db: &Path) -> Result<Vec<NativeSessionInfo>> {
    with_db(db, |con| {
        let (size, _) = util::file_info(db);
        let mut out = Vec::new();
        if util::has_table(con, "composerHeaders") {
            let mut st = con.prepare("SELECT composerId, lastUpdatedAt, createdAt, isArchived, value FROM composerHeaders WHERE isSubagent = 0 OR isSubagent IS NULL")?;
            let rows = st.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, Option<i64>>(2)?, r.get::<_, Option<i64>>(3)?, r.get::<_, Option<String>>(4)?))
            })?;
            for row in rows.flatten() {
                let (id, updated, created, _archived, value) = row;
                let v: Value = value.as_deref().and_then(|t| serde_json::from_str(t).ok()).unwrap_or(Value::Null);
                if v["isDraft"].as_bool().unwrap_or(false) || v["isBestOfNSubcomposer"].as_bool().unwrap_or(false) {
                    continue;
                }
                let upd = updated.or(created).unwrap_or(0);
                out.push(NativeSessionInfo {
                    agent: AGENT,
                    native_id: id,
                    path: db.to_path_buf(),
                    title: string(&v, "name").filter(|n| !n.trim().is_empty()),
                    cwd: workspace_path(&v),
                    updated_at: ts_from_ms(upd),
                    size,
                    mtime_ms: upd,
                });
            }
        } else if let Ok(raw) = con.query_row("SELECT value FROM ItemTable WHERE key = 'composer.composerData'", [], |r| r.get::<_, String>(0)) {
            // Older Cursor: one JSON blob listing every composer.
            let v: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
            for c in v["allComposers"].as_array().into_iter().flatten() {
                let Some(id) = s(c, "composerId") else { continue };
                let upd = c["lastUpdatedAt"].as_i64().or(c["createdAt"].as_i64()).unwrap_or(0);
                out.push(NativeSessionInfo {
                    agent: AGENT,
                    native_id: id.to_string(),
                    path: db.to_path_buf(),
                    title: string(c, "name"),
                    cwd: None,
                    updated_at: ts_from_ms(upd),
                    size,
                    mtime_ms: upd,
                });
            }
        }
        Ok(out)
    })
}

fn workspace_path(v: &Value) -> Option<String> {
    let uri = &v["workspaceIdentifier"]["uri"];
    string(uri, "fsPath")
        .or_else(|| string(&v["agentLocation"]["environment"]["uri"], "fsPath"))
        .or_else(|| v["trackedGitRepos"].as_array().and_then(|a| a.first()).and_then(|r| string(r, "path").or_else(|| string(r, "repoPath"))))
}

pub fn import(info: &NativeSessionInfo) -> Result<Session> {
    let name = info.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let mut s = if name.ends_with(".vscdb") {
        import_ide(&info.path, &info.native_id, info)?
    } else if name == "store.db" {
        import_cli(&info.path, &info.native_id)?
    } else {
        let text = util::read_text(&info.path)?;
        parse_transcript(&text, &info.native_id, Some(&info.path), info.updated_at)?
    };
    if s.workspace.is_none() {
        if let Some(c) = &info.cwd {
            s.workspace = Some(larrier_core::WorkspaceRef::from_path(c.clone()));
        }
    }
    Ok(s)
}

fn import_ide(db: &Path, id: &str, info: &NativeSessionInfo) -> Result<Session> {
    with_db(db, |con| {
        let data = kv(con, &format!("composerData:{id}")).ok_or_else(|| util::skip("composer has no data"))?;
        let mut bubbles: Vec<Value> = Vec::new();
        let headers = data["fullConversationHeadersOnly"].as_array().cloned().unwrap_or_default();
        if !headers.is_empty() {
            for h in &headers {
                if let Some(bid) = s(h, "bubbleId") {
                    if let Some(b) = kv(con, &format!("bubbleId:{id}:{bid}")) {
                        bubbles.push(b);
                    }
                }
            }
        } else if let Some(map) = data["conversation"].as_array().filter(|a| !a.is_empty()) {
            bubbles = map.clone(); // very old inline format
        } else {
            // v18+: no header list; scan the composer's bubble key range.
            let lo = format!("bubbleId:{id}:");
            let hi = format!("bubbleId:{id};");
            let mut st = con.prepare("SELECT key, value FROM cursorDiskKV WHERE key >= ?1 AND key < ?2")?;
            let rows = st.query_map([&lo, &hi], |r| {
                let k: String = r.get(0)?;
                let v: rusqlite::types::Value = r.get(1)?;
                Ok((k, v))
            })?;
            let mut keyed: Vec<(String, Value)> = Vec::new();
            for row in rows.flatten() {
                let bytes = match row.1 {
                    rusqlite::types::Value::Text(t) => t.into_bytes(),
                    rusqlite::types::Value::Blob(b) => b,
                    _ => continue,
                };
                if let Ok(v) = serde_json::from_slice::<Value>(&bytes) {
                    keyed.push((row.0, v));
                }
            }
            keyed.sort_by(|a, b| {
                let ka = (s(&a.1, "createdAt").unwrap_or(""), a.1["startedAtMs"].as_i64().unwrap_or(0));
                let kb = (s(&b.1, "createdAt").unwrap_or(""), b.1["startedAtMs"].as_i64().unwrap_or(0));
                ka.cmp(&kb).then(a.0.cmp(&b.0))
            });
            bubbles = keyed.into_iter().map(|(_, v)| v).collect();
        }
        let model = data["modelConfig"]["modelName"].as_str().map(String::from);
        let mut conv = Conv::new(AGENT);
        conv.model = model.clone().filter(|m| m != "default");
        for b in &bubbles {
            bubble_into(&mut conv, b);
        }
        if let Some(entries) = util::plan_from(&data["todos"]) {
            if !conv.messages.iter().any(|m| m.parts.iter().any(|p| matches!(p, Part::Plan { .. }))) {
                let m = conv.assistant(None, None);
                m.parts.push(Part::Plan { entries });
            }
        }
        let meta = Meta {
            native_id: id.to_string(),
            path: Some(db.to_path_buf()),
            title: info.title.clone().or_else(|| string(&data, "name")),
            cwd: info.cwd.clone(),
            created: data["createdAt"].as_i64().and_then(ts_from_ms),
            updated: data["lastUpdatedAt"].as_i64().and_then(ts_from_ms).or(info.updated_at),
            archived: false,
            ..Default::default()
        };
        finish(conv, AGENT, meta)
    })
}

fn bubble_into(conv: &mut Conv, b: &Value) {
    let ts = s(b, "createdAt").and_then(parse_ts).or_else(|| b["createdAt"].as_i64().and_then(ts_from_ms));
    let kind = b["type"].as_i64().unwrap_or(0);
    let text = s(b, "text").unwrap_or("").to_string();
    if kind == 1 {
        if b["isSimulatedMsg"].as_bool().unwrap_or(false) {
            let title = s(&b["simulatedMessageMetadata"], "title").unwrap_or("Background task update");
            conv.notice(title, ts);
            return;
        }
        let mut parts = vec![Part::Text { text: strip_query(&text) }];
        for img in b["images"].as_array().into_iter().flatten() {
            if let Some(p) = util::image_part(None, None, s(img, "path").or_else(|| s(img, "url"))) {
                parts.push(p);
            }
        }
        conv.user_parts(parts, ts);
        return;
    }
    if let Some(th) = b.get("thinking").filter(|t| t.is_object()) {
        conv.reasoning(s(th, "text").unwrap_or(""), b["thinkingDurationMs"].as_u64(), ts, None);
    }
    if let Some(tf) = b.get("toolFormerData").filter(|t| t.is_object()) {
        let name = s(tf, "name").filter(|n| !n.is_empty()).unwrap_or("tool");
        let args = s(tf, "params").filter(|p| !p.is_empty()).or_else(|| s(tf, "rawArgs").filter(|p| !p.is_empty())).map(json_or_string).unwrap_or(Value::Null);
        // Cursor wraps some params one level deep ({"relativeWorkspacePath":..} etc.)
        let input = normalize_args(args);
        let id = s(tf, "toolCallId").filter(|i| !i.is_empty()).map(String::from).unwrap_or_else(larrier_core::lsf::new_id);
        let tool = ToolCall::new(id.clone(), name, input);
        let status = s(tf, "status").unwrap_or("completed");
        conv.tool(tool, ts, None);
        let result = s(tf, "result").map(|r| {
            let v = json_or_string(r);
            match &v {
                Value::Object(o) => o
                    .get("output")
                    .or_else(|| o.get("contents"))
                    .or_else(|| o.get("content"))
                    .map(content_text)
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| r.to_string()),
                _ => content_text(&v),
            }
        });
        let is_err = matches!(status, "error" | "failed" | "rejected" | "cancelled");
        conv.tool_result(&id, result, is_err, Vec::new());
    }
    if !text.trim().is_empty() {
        conv.text(&text, ts, None);
    }
    if let Some(tc) = b.get("tokenCount") {
        let u = Usage { input_tokens: tc["inputTokens"].as_u64().unwrap_or(0), output_tokens: tc["outputTokens"].as_u64().unwrap_or(0), ..Default::default() };
        conv.add_usage(&u);
    }
}

fn normalize_args(v: Value) -> Value {
    let mut v = v;
    if let Value::Object(map) = &mut v {
        // Map Cursor's path keys onto the common ones LSF understands.
        for key in ["relativeWorkspacePath", "targetFile", "target_file", "filePath", "file"] {
            if let Some(p) = map.get(key).cloned() {
                if !map.contains_key("file_path") {
                    map.insert("file_path".into(), p);
                }
            }
        }
        if let Some(c) = map.get("codeEdit").cloned().or_else(|| map.get("code_edit").cloned()) {
            map.entry("new_string").or_insert(c);
        }
        // Agent / CLI tool argument names.
        for (from, to) in [("glob_pattern", "pattern"), ("search_term", "query"), ("target_directory", "path")] {
            if let Some(x) = map.get(from).cloned() {
                map.entry(to).or_insert(x);
            }
        }
    }
    v
}

/// Remove Cursor's `<user_query>` wrapper and attached-context preambles.
pub fn strip_query(t: &str) -> String {
    if let Some(i) = t.find("<user_query>") {
        let rest = &t[i + "<user_query>".len()..];
        let end = rest.find("</user_query>").unwrap_or(rest.len());
        return rest[..end].trim().to_string();
    }
    t.trim().to_string()
}

/// One Cursor tool call (agent transcript / CLI store) in LSF terms. Cursor
/// names tools in CamelCase (`StrReplace`, `ReadFile`) and routes MCP and
/// deferred tools through `CallMcpTool` / `CallDynamicTool`.
pub fn cursor_tool(id: &str, name: &str, input: Value) -> ToolCall {
    let mut name = name.to_string();
    let mut input = input;
    let mut title: Option<String> = None;
    if name == "CallMcpTool" || name == "CallDynamicTool" {
        let server = s(&input, "server").or_else(|| s(&input, "namespace")).unwrap_or("mcp").to_string();
        let tool = s(&input, "toolName").unwrap_or("tool").to_string();
        title = s(&input, "description").or_else(|| s(&input["mcpDetails"], "description")).map(String::from);
        let args = match input.get("arguments") {
            Some(Value::String(a)) => json_or_string(a),
            Some(a) => a.clone(),
            None => Value::Object(Default::default()),
        };
        // `cursor` is Cursor's own deferred tools (TodoWrite, …); the rest are
        // MCP servers (`user-<name>` for the user's own).
        name = if server == "cursor" { tool } else { format!("mcp__{}__{}", server.strip_prefix("user-").unwrap_or(&server), tool) };
        input = args;
    }
    let mut t = ToolCall::new(id, &name, normalize_args(input));
    let kind = match name.as_str() {
        "rg" => ToolKind::Search,
        "GetMcpTools" | "GetDynamicTools" | "UpdateCurrentStep" | "AwaitShell" | "Await" | "ReadLints" | "AskQuestion" | "SwitchMode" | "CreatePlan" | "SearchConversations" => ToolKind::Other,
        n if n.starts_with("mcp__") => ToolKind::Mcp,
        n => ToolKind::classify(&snake(n)),
    };
    if kind != t.kind {
        t.kind = kind;
        t.agent_name = None;
        if kind == ToolKind::Task {
            t.agent_name = s(&t.input, "subagent_type").map(String::from);
        }
    }
    let get = |k: &str| t.input.get(k).and_then(|v| v.as_str()).map(String::from);
    let named = match name.as_str() {
        "GetMcpTools" | "GetDynamicTools" => Some(format!("Loaded {}", get("toolName").unwrap_or_else(|| "tools".into()))),
        "UpdateCurrentStep" => get("current_step"),
        "AwaitShell" => Some("Waited for a running command".into()),
        "Await" => Some("Waited for a background task".into()),
        "ReadLints" => Some("Checked lints".into()),
        "AskQuestion" => {
            let q = get("title").or_else(|| t.input["questions"][0]["prompt"].as_str().map(String::from));
            Some(q.map(|q| format!("Asked: {q}")).unwrap_or_else(|| "Asked a question".into()))
        }
        "SwitchMode" => Some(format!("Switched to {} mode", get("target_mode_id").unwrap_or_default()).replace("  ", " ")),
        "CreatePlan" => Some(get("name").map(|n| format!("Planned {n}")).unwrap_or_else(|| "Created a plan".into())),
        "SearchConversations" => Some("Searched past chats".into()),
        _ => None,
    };
    t.title = title.filter(|x| !x.trim().is_empty()).or(named).or_else(|| Some(t.describe()));
    t
}

/// `StrReplace` → `str_replace`, `ReadFile` → `read_file`, `LS` → `ls`.
fn snake(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    let mut prev_lower = false;
    for c in name.chars() {
        if c.is_ascii_uppercase() && prev_lower {
            out.push('_');
        }
        prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        out.push(c.to_ascii_lowercase());
    }
    out
}

/// `<timestamp>Thursday, Oct 29, 2026, 4:58 PM (UTC+3)</timestamp>` in a prompt.
fn query_time(t: &str) -> Option<DateTime<Utc>> {
    let i = t.find("<timestamp>")? + "<timestamp>".len();
    let body = &t[i..i + t[i..].find("</timestamp>")?];
    let (date, zone) = body.split_once(" (UTC")?;
    let date = date.split_once(", ")?.1; // drop the weekday
    let naive = chrono::NaiveDateTime::parse_from_str(date.trim(), "%b %d, %Y, %I:%M %p").ok()?;
    let zone = zone.trim_end_matches(')');
    let (sign, hm) = match zone.as_bytes().first() {
        Some(b'+') => (1, &zone[1..]),
        Some(b'-') => (-1, &zone[1..]),
        _ => (1, "0"),
    };
    let (h, m) = hm.split_once(':').unwrap_or((hm, "0"));
    let offset = chrono::FixedOffset::east_opt(sign * (h.parse::<i32>().ok()? * 3600 + m.parse::<i32>().unwrap_or(0) * 60))?;
    naive.and_local_timezone(offset).single().map(|d| d.with_timezone(&Utc))
}

/// Images pasted into a prompt, listed in its `<image_files>` block
/// (`1. C:\…\image-….png`); files that are gone are skipped.
fn image_files(t: &str) -> Vec<Part> {
    let Some(i) = t.find("<image_files>") else { return Vec::new() };
    let end = t[i..].find("</image_files>").map(|e| i + e).unwrap_or(t.len());
    t[i..end]
        .lines()
        .filter_map(|l| {
            let (n, p) = l.trim().split_once(". ")?;
            n.parse::<u32>().ok()?;
            let p = p.trim();
            Path::new(p).is_file().then(|| Part::Image { mime: util::image_mime(p), source: MediaSource::Path { path: p.to_string() } })
        })
        .collect()
}

/// A prompt: the `<user_query>` text plus any pasted images.
fn prompt_parts(raw: &str) -> Vec<Part> {
    let mut parts = vec![Part::Text { text: strip_query(raw) }];
    parts.extend(image_files(raw));
    parts
}

/// Agent transcript lines: `{role, message:{content:[text|tool_use]}}` and
/// `{type: "turn_ended", status, error?}`. Tool results are not recorded.
pub fn parse_transcript(text: &str, id: &str, path: Option<&Path>, mtime: Option<chrono::DateTime<chrono::Utc>>) -> Result<Session> {
    let (conv, open) = transcript_conv(text, mtime);
    let meta = Meta { native_id: id.to_string(), path: path.map(PathBuf::from), updated: mtime, ..Default::default() };
    let mut s = finish(conv, AGENT, meta)?;
    if let Some(p) = path {
        attach_subagents(&mut s.messages, p);
    }
    s.external_turn = open;
    Ok(s)
}

/// A transcript's conversation, and whether its last turn is still open
/// (a prompt with no `turn_ended` after it, in transcripts that record them).
fn transcript_conv(text: &str, mtime: Option<DateTime<Utc>>) -> (Conv, bool) {
    let mut conv = Conv::new(AGENT);
    let mut ts = None;
    let (mut open, mut marked) = (false, false);
    for v in json_lines(text) {
        if s(&v, "type") == Some("turn_ended") {
            marked = true;
            open = false;
            if s(&v, "status") == Some("error") {
                match s(&v, "error").unwrap_or("Turn failed") {
                    "User aborted request" => conv.notice("Interrupted", ts.or(mtime)),
                    e => conv.error(e, ts.or(mtime)),
                }
            }
            continue;
        }
        let content = &v["message"]["content"];
        match s(&v, "role").unwrap_or("") {
            "user" => {
                let raw = content_text(content);
                ts = query_time(&raw).or(ts);
                conv.user_parts(prompt_parts(&raw), ts.or(mtime));
                open = true;
            }
            "assistant" => {
                for c in content.as_array().into_iter().flatten() {
                    match s(c, "type") {
                        Some("text") => conv.text(s(c, "text").unwrap_or(""), ts.or(mtime), None),
                        Some("thinking") | Some("reasoning") => conv.reasoning(s(c, "thinking").or_else(|| s(c, "text")).unwrap_or(""), None, ts.or(mtime), None),
                        Some("tool_use") | Some("tool-call") => {
                            let name = s(c, "name").or_else(|| s(c, "toolName")).unwrap_or("tool");
                            let input = c.get("input").or_else(|| c.get("args")).cloned().unwrap_or(Value::Null);
                            let id = s(c, "id").or_else(|| s(c, "toolCallId")).unwrap_or("");
                            conv.tool(cursor_tool(id, name, input), ts.or(mtime), None);
                        }
                        _ => {}
                    }
                }
                if let Some(t) = content.as_str() {
                    conv.text(t, ts.or(mtime), None);
                }
            }
            _ => {}
        }
    }
    (conv, open && marked)
}

/// Parts kept per inlined subagent transcript.
const MAX_SUBAGENT_PARTS: usize = 400;

/// Subagent transcripts (`<id>/subagents/*.jsonl`) inlined into the Task
/// calls that started them. Transcripts carry no call ids, so a subagent is
/// matched by its prompt.
fn attach_subagents(messages: &mut [Message], path: &Path) {
    let Some(dir) = path.parent().map(|d| d.join("subagents")) else { return };
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    let mut subs: Vec<(String, PathBuf)> = Vec::new();
    for p in entries.flatten().map(|e| e.path()).filter(|p| p.extension().map(|e| e == "jsonl").unwrap_or(false)) {
        let first = head_lines(&p, 4).iter().find_map(|l| {
            let v: Value = serde_json::from_str(l).ok()?;
            (s(&v, "role") == Some("user")).then(|| norm(&strip_query(&content_text(&v["message"]["content"]))))
        });
        if let Some(f) = first.filter(|f| !f.is_empty()) {
            subs.push((f, p));
        }
    }
    for m in messages.iter_mut() {
        for t in m.tool_calls_mut() {
            if subs.is_empty() {
                return;
            }
            if t.kind != ToolKind::Task || !t.subagent.is_empty() {
                continue;
            }
            let key = t.input.get("prompt").and_then(|p| p.as_str()).map(norm).unwrap_or_default();
            // Transcripts shorten long inputs to `…[N more chars]`.
            let key = match key.rfind("…[") {
                Some(i) if key.ends_with("more chars]") => key[..i].trim_end().to_string(),
                _ => key,
            };
            let Some(i) = subs.iter().position(|(f, _)| !key.is_empty() && f.contains(&key)) else { continue };
            let (_, p) = subs.remove(i);
            let Ok(text) = util::read_text(&p) else { continue };
            let mtime = std::fs::metadata(&p).ok().and_then(|m| m.modified().ok()).map(DateTime::<Utc>::from);
            let mut msgs = transcript_conv(&text, mtime).0.messages;
            let lead = msgs.iter().take_while(|m| m.role == Role::User).count();
            msgs.drain(..lead);
            msgs.retain(|m| !(m.role == Role::Assistant && m.parts.is_empty()));
            for m in msgs.iter_mut() {
                m.sidechain = true;
            }
            close_active_tools(&mut msgs, ToolStatus::Completed);
            slim_transcript(&mut msgs, MAX_SUBAGENT_PARTS);
            t.subagent = msgs;
        }
    }
}

fn norm(t: &str) -> String {
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---------------------------------------------------------------- CLI store.db

fn cli_meta(db: &Path) -> Result<Value> {
    let con = open_sqlite(db)?;
    let raw: String = con.query_row("SELECT value FROM meta WHERE key = '0'", [], |r| r.get(0))?;
    let bytes = hex::decode(raw.trim()).unwrap_or_else(|_| raw.into_bytes());
    Ok(serde_json::from_slice(&bytes)?)
}

/// A chat's name, unless it is Cursor's placeholder.
fn chat_name(meta: &Value) -> Option<String> {
    string(meta, "name").filter(|n| !n.trim().is_empty() && n != "New Agent")
}

/// Best effort: JSON message blobs in insertion order (the protobuf tree that
/// orders them is not decoded).
fn import_cli(db: &Path, id: &str) -> Result<Session> {
    let meta_v = cli_meta(db).unwrap_or(Value::Null);
    let con = open_sqlite(db)?;
    let mut st = con.prepare("SELECT data FROM blobs ORDER BY rowid")?;
    let rows = st.query_map([], |r| r.get::<_, Vec<u8>>(0))?;
    let mut conv = Conv::new(AGENT);
    let created = meta_v["createdAt"].as_i64().and_then(ts_from_ms);
    let mut ts = created;
    // Text of the previous assistant message: a retried model call is stored twice.
    let mut last_text: Option<String> = None;
    for data in rows.flatten() {
        let Ok(v) = serde_json::from_slice::<Value>(&data) else { continue };
        let Some(role) = s(&v, "role") else { continue };
        let content = &v["content"];
        match role {
            "user" => {
                let raw = content_text(content);
                // Injected context (`<user_info>`, rules, git status) is a
                // plain string; prompts are text parts wrapping `<user_query>`.
                if content.is_string() && !raw.contains("<user_query>") {
                    continue;
                }
                ts = query_time(&raw).or(ts);
                conv.user_parts(prompt_parts(&raw), ts);
                last_text = None;
            }
            "assistant" => {
                let items = content.as_array().map(|a| a.as_slice()).unwrap_or(&[]);
                let text: String = items.iter().filter(|c| s(c, "type") == Some("text")).filter_map(|c| s(c, "text")).collect();
                let has_tools = items.iter().any(|c| s(c, "type") == Some("tool-call"));
                if !has_tools && !text.trim().is_empty() && last_text.as_deref() == Some(text.as_str()) {
                    continue;
                }
                for c in items {
                    match s(c, "type") {
                        Some("text") => conv.text(s(c, "text").unwrap_or(""), ts, None),
                        Some("reasoning") => conv.reasoning(s(c, "text").unwrap_or(""), None, ts, None),
                        Some("tool-call") => conv.tool(cursor_tool(s(c, "toolCallId").unwrap_or(""), s(c, "toolName").unwrap_or("tool"), c["args"].clone()), ts, None),
                        _ => {}
                    }
                }
                if let Some(t) = content.as_str() {
                    conv.text(t, ts, None);
                }
                last_text = (!has_tools).then_some(text);
            }
            "tool" => {
                // The structured result (as in stream-json): success / rejected / error.
                let hl = &v["providerOptions"]["cursor"]["highLevelToolCallResult"];
                let (outcome, r) = hl["output"].as_object().and_then(|m| m.iter().next()).map(|(k, r)| (k.as_str(), r)).unwrap_or(("", &Value::Null));
                for c in content.as_array().into_iter().flatten().filter(|c| s(c, "type") == Some("tool-result")) {
                    let call = s(c, "toolCallId").unwrap_or("");
                    let out = match &c["result"] {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    let failed = outcome == "error" || (hl["isError"].as_bool() == Some(true) && outcome != "rejected");
                    conv.tool_result(call, Some(out), failed, result_diffs(r));
                    if outcome == "rejected" {
                        if let Some(t) = conv.tool_mut(call) {
                            t.status = ToolStatus::Cancelled;
                        }
                    }
                }
                last_text = None;
            }
            _ => {}
        }
    }
    if conv.messages.is_empty() {
        return Err(anyhow!("no readable messages in Cursor CLI store"));
    }
    let meta = Meta {
        native_id: id.to_string(),
        path: Some(db.to_path_buf()),
        title: chat_name(&meta_v),
        created,
        ..Default::default()
    };
    finish(conv, AGENT, meta)
}

/// Diff stats from an edit / write result.
fn result_diffs(r: &Value) -> Vec<FileDiff> {
    let path = s(r, "path").unwrap_or("").to_string();
    if let Some(d) = s(r, "diffString") {
        vec![FileDiff::from_unified(path, d.to_string())]
    } else if r.get("linesAdded").is_some() || r.get("linesRemoved").is_some() {
        vec![FileDiff { path, added: r["linesAdded"].as_u64().unwrap_or(0) as u32, removed: r["linesRemoved"].as_u64().unwrap_or(0) as u32, ..Default::default() }]
    } else if let Some(n) = r["linesCreated"].as_u64() {
        vec![FileDiff { path, added: n as u32, ..Default::default() }]
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript() {
        let t = r#"{"role":"user","message":{"content":[{"type":"text","text":"<user_query>\nadd a button\n</user_query>"}]}}
{"role":"assistant","message":{"content":[{"type":"text","text":"Sure."},{"type":"tool_use","name":"Edit","input":{"path":"a.tsx","old_string":"x","new_string":"y"}}]}}
{"type":"turn_ended","status":"success"}"#;
        let s = parse_transcript(t, "C1", None, None).unwrap();
        assert_eq!(s.messages[0].text(), "add a button");
        assert_eq!(s.messages[1].tool_calls().count(), 1);
        assert_eq!(s.title, "Add a button");
    }

    fn tools(s: &Session) -> Vec<&ToolCall> {
        s.messages.iter().flat_map(|m| m.tool_calls()).collect()
    }

    /// Tool shapes from real agent transcripts.
    #[test]
    fn transcript_tools_and_turns() {
        let dir = tempfile::tempdir().unwrap();
        let shot = dir.path().join("image-1.png");
        std::fs::write(&shot, b"png").unwrap();
        let prompt = format!(
            "[Image]\n<image_files>\nThe following images were provided by the user and saved to disk for future use:\n1. {}\n\nThese files can be read with tools.\n</image_files>\n<timestamp>Sunday, Sep 6, 2026, 3:32 PM (UTC+3)</timestamp>\n<user_query>\nfix the layout\n</user_query>",
            shot.display()
        );
        let lines = [
            serde_json::json!({"role":"user","message":{"content":[{"type":"text","text":prompt}]}}).to_string(),
            r#"{"role":"assistant","message":{"content":[{"type":"tool_use","name":"StrReplace","input":{"path":"a.css","old_string":"x","new_string":"y\nz"}},{"type":"tool_use","name":"Glob","input":{"glob_pattern":"*.css","target_directory":"src"}},{"type":"tool_use","name":"CallMcpTool","input":{"server":"cursor-ide-browser","toolName":"browser_navigate","arguments":{"url":"http://localhost"},"description":"Open the page"}},{"type":"tool_use","name":"CallDynamicTool","input":{"namespace":"cursor","toolName":"TodoWrite","arguments":{"merge":false,"todos":[{"id":"1","content":"Fix CSS","status":"completed"}]}}},{"type":"tool_use","name":"CallDynamicTool","input":{"namespace":"user-site_builder","toolName":"get_schema","mcpDetails":{"description":"Read the schema"},"arguments":{"table":"T"}}},{"type":"tool_use","name":"GetMcpTools","input":{"server":"cursor-app-control","toolName":"rename_chat"}},{"type":"tool_use","name":"UpdateCurrentStep","input":{"current_step":"Fixing styles"}},{"type":"tool_use","name":"ReadFile","input":{"path":"a.css"}},{"type":"tool_use","name":"WebSearch","input":{"search_term":"css grid"}}]}}"#.to_string(),
            r#"{"type":"turn_ended","status":"error","error":"Model not available"}"#.to_string(),
            r#"{"role":"user","message":{"content":[{"type":"text","text":"<user_query>\nagain\n</user_query>"}]}}"#.to_string(),
            r#"{"type":"turn_ended","status":"error","error":"User aborted request"}"#.to_string(),
            r#"{"role":"user","message":{"content":[{"type":"text","text":"<user_query>\nand now?\n</user_query>"}]}}"#.to_string(),
        ];
        let s = parse_transcript(&lines.join("\n"), "C2", None, None).unwrap();
        let first = &s.messages[0];
        assert_eq!(first.text(), "fix the layout");
        assert_eq!(first.parts.iter().filter(|p| matches!(p, Part::Image { .. })).count(), 1, "pasted image");
        assert_eq!(first.created_at.to_rfc3339(), "2026-09-06T12:32:00+00:00", "prompt time from <timestamp>");
        let t = tools(&s);
        assert_eq!((t[0].kind, t[0].diffs.len()), (ToolKind::Edit, 1));
        assert_eq!((t[1].kind, t[1].title.as_deref()), (ToolKind::Search, Some("Searched *.css")));
        assert_eq!((t[2].name.as_str(), t[2].kind, t[2].title.as_deref()), ("mcp__cursor-ide-browser__browser_navigate", ToolKind::Mcp, Some("Open the page")));
        assert_eq!((t[3].name.as_str(), t[3].kind), ("TodoWrite", ToolKind::Todo));
        assert!(first.parts.is_empty() || s.messages[1].parts.iter().any(|p| matches!(p, Part::Plan { .. })), "to-do list becomes a plan");
        assert_eq!((t[4].name.as_str(), t[4].title.as_deref()), ("mcp__site_builder__get_schema", Some("Read the schema")));
        assert_eq!((t[5].kind, t[5].title.as_deref()), (ToolKind::Other, Some("Loaded rename_chat")));
        assert_eq!(t[6].title.as_deref(), Some("Fixing styles"));
        assert_eq!(t[7].kind, ToolKind::Read);
        assert_eq!((t[8].kind, t[8].title.as_deref()), (ToolKind::WebSearch, Some("Searched web for css grid")));
        let parts: Vec<&Part> = s.messages.iter().flat_map(|m| &m.parts).collect();
        assert!(parts.iter().any(|p| matches!(p, Part::Error { message } if message == "Model not available")));
        assert!(parts.iter().any(|p| matches!(p, Part::Notice { text } if text == "Interrupted")));
        assert!(s.external_turn, "the last prompt has no turn_ended yet");
    }

    #[test]
    fn subagent_transcripts_attach_by_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let chat = dir.path().join("C3");
        std::fs::create_dir_all(chat.join("subagents")).unwrap();
        let main = [
            r#"{"role":"user","message":{"content":[{"type":"text","text":"<user_query>\naudit it\n</user_query>"}]}}"#,
            r#"{"role":"assistant","message":{"content":[{"type":"tool_use","name":"Task","input":{"description":"Audit backend","subagent_type":"explore","prompt":"Audit the backend\nthorou…[3 more chars]"}}]}}"#,
            r#"{"type":"turn_ended","status":"success"}"#,
        ];
        let sub = [
            r#"{"role":"user","message":{"content":[{"type":"text","text":"<timestamp>Sunday, Sep 6, 2026, 3:32 PM (UTC+3)</timestamp>\n<user_query>\nAudit the backend thoroughly.\n</user_query>"}]}}"#,
            r#"{"role":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"path":"api.rs"}},{"type":"text","text":"Looks fine."}]}}"#,
        ];
        let path = chat.join("C3.jsonl");
        std::fs::write(&path, main.join("\n")).unwrap();
        std::fs::write(chat.join("subagents").join("s1.jsonl"), sub.join("\n")).unwrap();
        let s = parse_transcript(&main.join("\n"), "C3", Some(&path), None).unwrap();
        let task = tools(&s)[0];
        assert_eq!((task.kind, task.agent_name.as_deref()), (ToolKind::Task, Some("explore")));
        assert_eq!(task.subagent.len(), 1, "the subagent's prompt is dropped");
        assert_eq!(task.subagent[0].text(), "Looks fine.");
        assert!(!s.external_turn);
    }

    /// A CLI chat store laid out like real ones: injected context, empty
    /// signed reasoning, structured results and a retried message.
    #[test]
    fn cli_store() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("store.db");
        let con = rusqlite::Connection::open(&db).unwrap();
        con.execute_batch("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT); CREATE TABLE blobs (id TEXT PRIMARY KEY, data BLOB);").unwrap();
        let meta = hex::encode(r#"{"agentId":"A","name":"New Agent","mode":"default","createdAt":1793282282246}"#);
        con.execute("INSERT INTO meta VALUES ('0', ?1)", [meta]).unwrap();
        let blobs = [
            r#"{"role":"system","content":"You are an AI coding assistant"}"#,
            r#"{"role":"user","content":"<user_info>\nOS Version: win32\n</user_info>"}"#,
            r#"{"role":"user","content":[{"type":"text","text":"<timestamp>Thursday, Oct 29, 2026, 4:58 PM (UTC+3)</timestamp>\n<user_query>\nedit notes\n</user_query>"}]}"#,
            r#"{"role":"assistant","content":[{"type":"reasoning","text":"","signature":"sig"},{"type":"text","text":"On it."},{"type":"tool-call","toolCallId":"c1","toolName":"StrReplace","args":{"path":"notes.txt","old_string":"beta","new_string":"BETA"}},{"type":"tool-call","toolCallId":"c2","toolName":"Shell","args":{"command":"cmd /c exit 3","description":"Exit"}}]}"#,
            r#"{"role":"tool","content":[{"type":"tool-result","toolCallId":"c1","toolName":"StrReplace","result":"The file notes.txt has been updated."}],"providerOptions":{"cursor":{"highLevelToolCallResult":{"output":{"success":{"path":"notes.txt","linesAdded":1,"linesRemoved":1,"diffString":"--- a/notes.txt\n+++ b/notes.txt\n@@ -1,1 +1,1 @@\n-beta\n+BETA\n"}},"isError":false}}}}"#,
            r#"{"role":"tool","content":[{"type":"tool-result","toolCallId":"c2","toolName":"Shell","result":"Rejected: "}],"providerOptions":{"cursor":{"highLevelToolCallResult":{"output":{"rejected":{"command":"cmd /c exit 3"}},"isError":true}}}}"#,
            r#"{"role":"assistant","content":[{"type":"reasoning","text":"","signature":"s2"},{"type":"text","text":"done"}]}"#,
            r#"{"role":"assistant","content":[{"type":"reasoning","text":"","signature":"s3"},{"type":"text","text":"done"}]}"#,
        ];
        for (i, b) in blobs.iter().enumerate() {
            con.execute("INSERT INTO blobs VALUES (?1, ?2)", rusqlite::params![i.to_string(), b.as_bytes()]).unwrap();
        }
        drop(con);
        let s = import_cli(&db, "A").unwrap();
        assert_eq!(s.title, "Edit notes", "placeholder name ignored; injected context is not a prompt");
        assert_eq!(s.messages.iter().filter(|m| m.role == Role::User).count(), 1);
        assert_eq!(s.messages[0].created_at.to_rfc3339(), "2026-10-29T13:58:00+00:00");
        let t = tools(&s);
        assert_eq!((t[0].kind, t[0].status, t[0].diffs[0].added), (ToolKind::Edit, ToolStatus::Completed, 1));
        assert_eq!((t[1].status, t[1].is_error), (ToolStatus::Cancelled, false), "rejected, not failed");
        let text: Vec<String> = s.messages.iter().filter(|m| m.role == Role::Assistant).map(|m| m.text()).collect();
        assert_eq!(text.concat().matches("done").count(), 1, "retried message kept once");
        assert!(!s.messages.iter().flat_map(|m| &m.parts).any(|p| matches!(p, Part::Reasoning { .. })), "signed-only reasoning has no text");
    }
}
