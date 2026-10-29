//! OpenCode: SQLite `~/.local/share/opencode/opencode.db` (tables `session`,
//! `message`, `part` with JSON `data`) and the pre-1.2 JSON layout under
//! `storage/{session,message,part}`.

use std::path::{Path, PathBuf};

use anyhow::Result;
use larrier_core::{AgentKind, FileDiff, Part, Session, ToolCall, Usage};
use serde_json::Value;

use crate::util::{self, env_dir, finish, open_sqlite, s, string, ts_from_ms, Conv, Meta};
use crate::NativeSessionInfo;

const AGENT: AgentKind = AgentKind::OpenCode;

pub fn data_dir() -> PathBuf {
    env_dir("XDG_DATA_HOME").map(|p| p.join("opencode")).unwrap_or_else(|| util::home().join(".local").join("share").join("opencode"))
}

pub fn db_path() -> PathBuf {
    env_dir("OPENCODE_DB").unwrap_or_else(|| data_dir().join("opencode.db"))
}

pub fn discover() -> Vec<NativeSessionInfo> {
    let mut out = Vec::new();
    let db = db_path();
    if db.exists() {
        match discover_db(&db) {
            Ok(v) => out.extend(v),
            Err(e) => tracing::warn!("opencode discovery: {e}"),
        }
    }
    // Legacy JSON storage (only sessions not already in the database).
    let legacy = data_dir().join("storage").join("session");
    if legacy.exists() {
        for e in walkdir::WalkDir::new(&legacy).max_depth(2).into_iter().flatten() {
            let p = e.path();
            if p.extension().map(|x| x == "json").unwrap_or(false) {
                let Ok(t) = std::fs::read_to_string(p) else { continue };
                let Ok(v) = serde_json::from_str::<Value>(&t) else { continue };
                let Some(id) = s(&v, "id") else { continue };
                if out.iter().any(|i| i.native_id == id) || v.get("parentID").map(|x| !x.is_null()).unwrap_or(false) {
                    continue;
                }
                let mut info = util::info(AGENT, id, p);
                info.title = string(&v, "title");
                info.cwd = string(&v, "directory");
                out.push(info);
            }
        }
    }
    out
}

fn discover_db(db: &Path) -> Result<Vec<NativeSessionInfo>> {
    let con = open_sqlite(db)?;
    let cols = util::table_columns(&con, "session");
    let parent = if cols.iter().any(|c| c == "parent_id") { "WHERE parent_id IS NULL" } else { "" };
    let mut st = con.prepare(&format!("SELECT id, title, directory, time_created, time_updated FROM session {parent}"))?;
    let (size, _) = util::file_info(db);
    let rows = st.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<i64>>(3)?, r.get::<_, Option<i64>>(4)?))
    })?;
    let mut out = Vec::new();
    for (id, title, dir, created, updated) in rows.flatten() {
        let upd = updated.or(created).unwrap_or(0);
        out.push(NativeSessionInfo { agent: AGENT, native_id: id, path: db.to_path_buf(), title, cwd: dir, updated_at: ts_from_ms(upd), size, mtime_ms: upd });
    }
    Ok(out)
}

pub fn import(info: &NativeSessionInfo) -> Result<Session> {
    if info.path.extension().map(|x| x == "json").unwrap_or(false) {
        return import_legacy(info);
    }
    let con = open_sqlite(&info.path)?;
    let srow = con.query_row("SELECT title, directory, time_created, time_updated FROM session WHERE id = ?1", [&info.native_id], |r| {
        Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<i64>>(2)?, r.get::<_, Option<i64>>(3)?))
    })?;
    let mut msgs: Vec<(String, Value)> = Vec::new();
    {
        let mut st = con.prepare("SELECT id, data FROM message WHERE session_id = ?1 ORDER BY time_created, id")?;
        for r in st.query_map([&info.native_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.flatten() {
            if let Ok(v) = serde_json::from_str::<Value>(&r.1) {
                msgs.push((r.0, v));
            }
        }
    }
    let mut parts: std::collections::HashMap<String, Vec<Value>> = std::collections::HashMap::new();
    {
        let mut st = con.prepare("SELECT message_id, data FROM part WHERE session_id = ?1 ORDER BY id")?;
        for r in st.query_map([&info.native_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.flatten() {
            if let Ok(v) = serde_json::from_str::<Value>(&r.1) {
                parts.entry(r.0).or_default().push(v);
            }
        }
    }
    let list: Vec<(Value, Vec<Value>)> = msgs.into_iter().map(|(id, m)| (m, parts.remove(&id).unwrap_or_default())).collect();
    let meta = Meta {
        native_id: info.native_id.clone(),
        path: Some(info.path.clone()),
        title: srow.0,
        cwd: srow.1.map(|d| if cfg!(windows) { d.replace('/', "\\") } else { d }),
        created: srow.2.and_then(ts_from_ms),
        updated: srow.3.and_then(ts_from_ms),
        ..Default::default()
    };
    build(list, meta)
}

fn import_legacy(info: &NativeSessionInfo) -> Result<Session> {
    let storage = info.path.parent().and_then(|p| p.parent()).and_then(|p| p.parent()).map(PathBuf::from).unwrap_or_default();
    let sv: Value = serde_json::from_str(&std::fs::read_to_string(&info.path)?)?;
    let mut msgs: Vec<Value> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(storage.join("message").join(&info.native_id)) {
        for e in rd.flatten() {
            if let Ok(v) = std::fs::read_to_string(e.path()).map_err(anyhow::Error::from).and_then(|t| Ok(serde_json::from_str::<Value>(&t)?)) {
                msgs.push(v);
            }
        }
    }
    msgs.sort_by_key(|m| m["time"]["created"].as_i64().unwrap_or(0));
    let mut list = Vec::new();
    for m in msgs {
        let mid = s(&m, "id").unwrap_or("").to_string();
        let mut ps: Vec<(String, Value)> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(storage.join("part").join(&mid)) {
            for e in rd.flatten() {
                if let Ok(t) = std::fs::read_to_string(e.path()) {
                    if let Ok(v) = serde_json::from_str::<Value>(&t) {
                        ps.push((s(&v, "id").unwrap_or("").to_string(), v));
                    }
                }
            }
        }
        ps.sort_by(|a, b| a.0.cmp(&b.0));
        list.push((m, ps.into_iter().map(|p| p.1).collect()));
    }
    let meta = Meta {
        native_id: info.native_id.clone(),
        path: Some(info.path.clone()),
        title: string(&sv, "title"),
        cwd: string(&sv, "directory"),
        created: sv["time"]["created"].as_i64().and_then(ts_from_ms),
        updated: sv["time"]["updated"].as_i64().and_then(ts_from_ms),
        ..Default::default()
    };
    build(list, meta)
}

fn build(list: Vec<(Value, Vec<Value>)>, meta: Meta) -> Result<Session> {
    let mut conv = Conv::new(AGENT);
    for (m, parts) in list {
        let ts = m["time"]["created"].as_i64().and_then(ts_from_ms);
        let role = s(&m, "role").unwrap_or("");
        if role == "user" {
            let mut out = Vec::new();
            for p in &parts {
                if p["synthetic"].as_bool().unwrap_or(false) {
                    continue;
                }
                match s(p, "type") {
                    Some("text") => out.push(Part::Text { text: s(p, "text").unwrap_or("").to_string() }),
                    Some("file") => {
                        let mime = string(p, "mime");
                        let url = s(p, "url").unwrap_or("");
                        if mime.as_deref().map(|m| m.starts_with("image/")).unwrap_or(false) {
                            if let Some(img) = util::image_part(mime, None, Some(url)) {
                                out.push(img);
                            }
                        } else {
                            let path = s(p, "filename").map(String::from).unwrap_or_else(|| url.trim_start_matches("file://").to_string());
                            out.push(Part::File { path, mime, text: None });
                        }
                    }
                    _ => {}
                }
            }
            conv.user_parts(out, ts);
            continue;
        }
        let model = s(&m, "modelID");
        let _ = conv.assistant(ts, model);
        for p in &parts {
            let pts = p["time"]["start"].as_i64().and_then(ts_from_ms).or(ts);
            match s(p, "type") {
                Some("text") if !p["synthetic"].as_bool().unwrap_or(false) => conv.text(s(p, "text").unwrap_or(""), pts, model),
                Some("reasoning") => {
                    let dur = match (p["time"]["start"].as_i64(), p["time"]["end"].as_i64()) {
                        (Some(a), Some(b)) if b >= a => Some((b - a) as u64),
                        _ => None,
                    };
                    conv.reasoning(s(p, "text").unwrap_or(""), dur, pts, model);
                }
                Some("tool") => {
                    let st = &p["state"];
                    let name = s(p, "tool").unwrap_or("tool");
                    let id = s(p, "callID").or_else(|| s(p, "id")).unwrap_or("").to_string();
                    let mut tool = ToolCall::new(id.clone(), name, st.get("input").cloned().unwrap_or(Value::Null));
                    if let Some(t) = s(st, "title").filter(|t| !t.is_empty()) {
                        tool.title = Some(format!("{} {}", tool.describe().split(' ').next().unwrap_or(name), t));
                    }
                    conv.tool(tool, pts, model);
                    let status = s(st, "status").unwrap_or("completed");
                    let output = string(st, "output").or_else(|| string(st, "error"));
                    let diffs = tool_diffs(st);
                    conv.tool_result(&id, output, status == "error", diffs);
                }
                Some("step-finish") => {
                    let t = &p["tokens"];
                    let u = Usage {
                        input_tokens: t["input"].as_u64().unwrap_or(0),
                        output_tokens: t["output"].as_u64().unwrap_or(0),
                        cache_read_tokens: t["cache"]["read"].as_u64().unwrap_or(0),
                        cache_write_tokens: t["cache"]["write"].as_u64().unwrap_or(0),
                        reasoning_tokens: t["reasoning"].as_u64().unwrap_or(0),
                        cost_usd: p["cost"].as_f64().filter(|c| *c > 0.0),
                    };
                    conv.add_usage(&u);
                }
                _ => {}
            }
        }
        if let Some(err) = m.get("error").filter(|e| !e.is_null()) {
            let msg = err["data"]["message"].as_str().map(String::from).unwrap_or_else(|| s(err, "name").unwrap_or("error").to_string());
            if !msg.contains("MessageAbortedError") {
                conv.error(&msg, ts);
            }
        }
    }
    finish(conv, AGENT, meta)
}

fn tool_diffs(state: &Value) -> Vec<FileDiff> {
    let md = &state["metadata"];
    let path = s(&state["input"], "filePath").or_else(|| s(&state["input"], "file_path")).unwrap_or("");
    if let Some(d) = md.get("diff").and_then(|d| d.as_str()).filter(|d| !d.is_empty()) {
        return vec![FileDiff::from_unified(path, d.to_string())];
    }
    if let Some(fd) = md.get("filediff").filter(|f| f.is_object()) {
        let p = s(fd, "file").unwrap_or(path);
        let mut d = FileDiff::from_texts(p, string(fd, "before"), string(fd, "after"));
        if let (Some(a), Some(r)) = (fd["additions"].as_u64(), fd["deletions"].as_u64()) {
            d.added = a as u32;
            d.removed = r as u32;
        }
        return vec![d];
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_from_rows() {
        let user: Value = serde_json::json!({"role":"user","time":{"created":1700000000000i64}});
        let up = vec![serde_json::json!({"type":"text","text":"make it faster"})];
        let asst: Value = serde_json::json!({"role":"assistant","modelID":"m1","time":{"created":1700000001000i64}});
        let ap = vec![
            serde_json::json!({"type":"reasoning","text":"plan","time":{"start":1,"end":2001}}),
            serde_json::json!({"type":"tool","tool":"edit","callID":"c1","state":{"status":"completed","input":{"filePath":"a.ts","oldString":"a","newString":"b"},"output":"ok","metadata":{"diff":"@@ -1 +1 @@\n-a\n+b\n"}}}),
            serde_json::json!({"type":"text","text":"done"}),
            serde_json::json!({"type":"step-finish","tokens":{"input":10,"output":5,"cache":{"read":1,"write":0}},"cost":0.01}),
        ];
        let s = build(vec![(user, up), (asst, ap)], Meta { native_id: "ses_1".into(), ..Default::default() }).unwrap();
        assert_eq!(s.messages.len(), 2);
        let t = s.messages[1].tool_calls().next().unwrap();
        assert_eq!(t.diffs[0].added, 1);
        assert_eq!(s.usage.input_tokens, 10);
        assert_eq!(s.model.as_deref(), Some("m1"));
    }
}
