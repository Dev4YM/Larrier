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
use larrier_core::{lsf::title_from_prompt, AgentKind, Part, Session, ToolCall, Usage};
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
                    info.title = string(&m, "name");
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

/// Agent transcript lines: `{role, message:{content:[text|tool_use]}}`.
pub fn parse_transcript(text: &str, id: &str, path: Option<&Path>, mtime: Option<chrono::DateTime<chrono::Utc>>) -> Result<Session> {
    let mut conv = Conv::new(AGENT);
    for v in json_lines(text) {
        let role = s(&v, "role").unwrap_or("");
        let content = &v["message"]["content"];
        match role {
            "user" => {
                let t = strip_query(&content_text(content));
                conv.user(t, mtime);
            }
            "assistant" => {
                for c in content.as_array().into_iter().flatten() {
                    match s(c, "type") {
                        Some("text") => conv.text(s(c, "text").unwrap_or(""), mtime, None),
                        Some("thinking") | Some("reasoning") => conv.reasoning(s(c, "thinking").or_else(|| s(c, "text")).unwrap_or(""), None, mtime, None),
                        Some("tool_use") | Some("tool-call") => {
                            let name = s(c, "name").or_else(|| s(c, "toolName")).unwrap_or("tool");
                            let input = normalize_args(c.get("input").or_else(|| c.get("args")).cloned().unwrap_or(Value::Null));
                            let id = s(c, "id").or_else(|| s(c, "toolCallId")).map(String::from).unwrap_or_default();
                            conv.tool(ToolCall::new(id, name, input), mtime, None);
                        }
                        _ => {}
                    }
                }
                if let Some(t) = content.as_str() {
                    conv.text(t, mtime, None);
                }
            }
            _ => {}
        }
    }
    let meta = Meta { native_id: id.to_string(), path: path.map(PathBuf::from), created: mtime, updated: mtime, ..Default::default() };
    finish(conv, AGENT, meta)
}

// ---------------------------------------------------------------- CLI store.db

fn cli_meta(db: &Path) -> Result<Value> {
    let con = open_sqlite(db)?;
    let raw: String = con.query_row("SELECT value FROM meta WHERE key = '0'", [], |r| r.get(0))?;
    let bytes = hex::decode(raw.trim()).unwrap_or_else(|_| raw.into_bytes());
    Ok(serde_json::from_slice(&bytes)?)
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
    for data in rows.flatten() {
        let Ok(v) = serde_json::from_slice::<Value>(&data) else { continue };
        let Some(role) = s(&v, "role") else { continue };
        let content = &v["content"];
        match role {
            "user" => conv.user(strip_query(&content_text(content)), created),
            "assistant" => {
                for c in content.as_array().into_iter().flatten() {
                    match s(c, "type") {
                        Some("text") => conv.text(s(c, "text").unwrap_or(""), created, None),
                        Some("reasoning") => conv.reasoning(s(c, "text").unwrap_or(""), None, created, None),
                        Some("tool-call") => {
                            let t = ToolCall::new(s(c, "toolCallId").unwrap_or(""), s(c, "toolName").unwrap_or("tool"), normalize_args(c["args"].clone()));
                            conv.tool(t, created, None);
                        }
                        _ => {}
                    }
                }
                if let Some(t) = content.as_str() {
                    conv.text(t, created, None);
                }
            }
            "tool" => {
                for c in content.as_array().into_iter().flatten() {
                    if s(c, "type") == Some("tool-result") {
                        let out = match &c["result"] {
                            Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        conv.tool_result(s(c, "toolCallId").unwrap_or(""), Some(out), false, Vec::new());
                    }
                }
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
        title: string(&meta_v, "name"),
        created,
        ..Default::default()
    };
    finish(conv, AGENT, meta)
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
}
