//! Hermes Agent (Nous Research): SQLite `$HERMES_HOME/state.db`
//! (`%LOCALAPPDATA%\hermes` on Windows, `~/.hermes` elsewhere).

use std::path::{Path, PathBuf};

use anyhow::Result;
use larrier_core::{AgentKind, Session, ToolCall, Usage};
use serde_json::Value;

use crate::util::{self, content_text, env_dir, finish, json_or_string, open_sqlite, s, ts_from_secs, Conv, Meta};
use crate::NativeSessionInfo;

const AGENT: AgentKind = AgentKind::Hermes;

pub fn home_dir() -> PathBuf {
    if let Some(h) = env_dir("HERMES_HOME") {
        return h;
    }
    if cfg!(windows) {
        if let Some(la) = env_dir("LOCALAPPDATA") {
            let p = la.join("hermes");
            if p.join("state.db").exists() {
                return p;
            }
        }
    }
    util::home().join(".hermes")
}

pub fn discover() -> Vec<NativeSessionInfo> {
    let db = home_dir().join("state.db");
    if !db.exists() {
        return Vec::new();
    }
    match discover_db(&db) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("hermes discovery: {e}");
            Vec::new()
        }
    }
}

fn discover_db(db: &Path) -> Result<Vec<NativeSessionInfo>> {
    let con = open_sqlite(db)?;
    let cols = util::table_columns(&con, "sessions");
    let has = |c: &str| cols.iter().any(|x| x == c);
    let mut filters = Vec::new();
    if has("parent_session_id") {
        filters.push("(s.parent_session_id IS NULL OR s.parent_session_id = '')");
    }
    if has("source") {
        filters.push("(s.source IS NULL OR s.source <> 'subagent')");
    }
    if has("hidden") {
        filters.push("(s.hidden IS NULL OR s.hidden = 0)");
    }
    let wh = if filters.is_empty() { String::new() } else { format!("WHERE {}", filters.join(" AND ")) };
    let title = if has("title") { "s.title" } else { "NULL" };
    let cwd = if has("cwd") { "s.cwd" } else { "NULL" };
    let sql = format!(
        "SELECT s.id, {title}, {cwd}, s.started_at, (SELECT max(m.timestamp) FROM messages m WHERE m.session_id = s.id), (SELECT count(*) FROM messages m WHERE m.session_id = s.id) FROM sessions s {wh}"
    );
    let mut st = con.prepare(&sql)?;
    let rows = st.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<f64>>(3)?, r.get::<_, Option<f64>>(4)?, r.get::<_, i64>(5)?))
    })?;
    let mut out = Vec::new();
    for (id, title, cwd, started, last, count) in rows.flatten() {
        if count == 0 {
            continue;
        }
        let upd = last.or(started).unwrap_or(0.0);
        let mtime = (upd * 1000.0) as i64;
        out.push(NativeSessionInfo { agent: AGENT, native_id: id, path: db.to_path_buf(), title, cwd, updated_at: ts_from_secs(upd), size: count as u64, mtime_ms: mtime });
    }
    Ok(out)
}

/// Hermes stores structured content as `"\0json:" + JSON`.
fn decode_content(raw: &str) -> String {
    match raw.strip_prefix("\u{0}json:") {
        Some(j) => match serde_json::from_str::<Value>(j) {
            Ok(v) => content_text(&v),
            Err(_) => j.to_string(),
        },
        None => raw.to_string(),
    }
}

/// Hermes tools answer with a JSON object (`{"output": …, "exit_code": 0}`,
/// `{"content": …, "total_lines": …}`, `{"success": false, "error": …}`):
/// the readable part and whether the call failed.
pub fn tool_output(raw: &str) -> (String, bool) {
    let Ok(Value::Object(o)) = serde_json::from_str::<Value>(raw.trim()) else { return (raw.to_string(), false) };
    let text = |k: &str| o.get(k).and_then(|v| v.as_str()).map(str::trim).filter(|t| !t.is_empty()).map(String::from);
    let error = text("error");
    let failed = o.get("success").and_then(|v| v.as_bool()) == Some(false)
        || text("status").as_deref() == Some("error")
        || o.get("exit_code").and_then(|v| v.as_i64()).is_some_and(|c| c != 0);
    let body = ["output", "content", "matches_text", "diff", "message", "result"].iter().find_map(|k| text(k)).or_else(|| {
        if let Some(n) = o.get("bytes_written").and_then(|v| v.as_u64()) {
            return Some(format!("Wrote {n} bytes"));
        }
        match o.get("total_count").and_then(|v| v.as_u64()) {
            Some(0) if error.is_none() => Some("No matches".to_string()),
            _ => None,
        }
    });
    let out = match (body, &error) {
        (Some(b), Some(e)) => format!("{b}\n{e}"),
        (Some(b), None) => b,
        (None, Some(e)) => e.clone(),
        (None, None) => raw.to_string(),
    };
    // A tool that only reports an error failed, whatever else it says.
    let failed = failed || (error.is_some() && ["output", "content", "matches_text", "diff"].iter().all(|k| text(k).is_none()));
    (out, failed)
}

pub fn import(info: &NativeSessionInfo) -> Result<Session> {
    let con = open_sqlite(&info.path)?;
    let scols = util::table_columns(&con, "sessions");
    let mcols = util::table_columns(&con, "messages");
    let has_s = |c: &str| scols.iter().any(|x| x == c);
    let has_m = |c: &str| mcols.iter().any(|x| x == c);
    let model: Option<String> = if has_s("model") {
        con.query_row("SELECT model FROM sessions WHERE id = ?1", [&info.native_id], |r| r.get(0)).ok().flatten()
    } else {
        None
    };
    let branch: Option<String> = if has_s("git_branch") {
        con.query_row("SELECT git_branch FROM sessions WHERE id = ?1", [&info.native_id], |r| r.get(0)).ok().flatten()
    } else {
        None
    };
    let ended = has_s("ended_at") && con.query_row("SELECT ended_at IS NOT NULL FROM sessions WHERE id = ?1", [&info.native_id], |r| r.get::<_, bool>(0)).unwrap_or(true);
    // Session totals, kept by Hermes per API call.
    let scol = |c: &str| if has_s(c) { format!("COALESCE({c}, 0)") } else { "0".to_string() };
    let totals = con
        .query_row(
            &format!(
                "SELECT {}, {}, {}, {}, {}, {}, {} FROM sessions WHERE id = ?1",
                scol("input_tokens"),
                scol("output_tokens"),
                scol("cache_read_tokens"),
                scol("cache_write_tokens"),
                scol("reasoning_tokens"),
                scol("actual_cost_usd"),
                scol("estimated_cost_usd"),
            ),
            [&info.native_id],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?, r.get::<_, f64>(5)?, r.get::<_, f64>(6)?)),
        )
        .ok()
        .filter(|t| t.0 + t.1 > 0)
        .map(|(i, o, cr, cw, r, actual, est)| Usage {
            input_tokens: i as u64,
            output_tokens: o as u64,
            cache_read_tokens: cr as u64,
            cache_write_tokens: cw as u64,
            reasoning_tokens: r as u64,
            cost_usd: Some(if actual > 0.0 { actual } else { est }).filter(|c| *c > 0.0),
        });
    let col = |c: &str| if has_m(c) { c.to_string() } else { format!("NULL AS {c}") };
    let active = if has_m("active") { "AND (active IS NULL OR active = 1)" } else { "" };
    let sql = format!(
        "SELECT role, content, {}, {}, {}, timestamp, {}, {} FROM messages WHERE session_id = ?1 {active} ORDER BY timestamp, id",
        col("tool_call_id"),
        col("tool_calls"),
        col("tool_name"),
        col("reasoning"),
        col("token_count"),
    );
    let mut st = con.prepare(&sql)?;
    let rows = st.query_map([&info.native_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, Option<f64>>(5)?,
            r.get::<_, Option<String>>(6)?,
            r.get::<_, Option<i64>>(7)?,
        ))
    })?;
    let mut conv = Conv::new(AGENT);
    conv.model = model.clone();
    // Whether Hermes is still mid-turn: the last record is a prompt, a tool
    // result or a reply that asked for tools.
    let mut open = false;
    for (role, content, tool_call_id, tool_calls, tool_name, ts, reasoning, tokens) in rows.flatten() {
        open = match role.as_str() {
            "user" | "tool" => true,
            "assistant" => tool_calls.as_deref().is_some_and(|t| !t.trim().is_empty() && t != "null" && t != "[]"),
            _ => open,
        };
        let ts = ts.and_then(ts_from_secs);
        let text = content.as_deref().map(decode_content).unwrap_or_default();
        match role.as_str() {
            "user" => conv.user(text, ts),
            "assistant" => {
                if let Some(r) = reasoning.filter(|r| !r.trim().is_empty()) {
                    conv.reasoning(&r, None, ts, None);
                }
                if let Some(tc) = tool_calls.filter(|t| !t.trim().is_empty() && t != "null") {
                    for call in json_or_string(&tc).as_array().into_iter().flatten() {
                        let f = &call["function"];
                        let id = s(call, "id").or_else(|| s(call, "call_id")).unwrap_or("");
                        let args = s(f, "arguments").map(json_or_string).unwrap_or_else(|| f.get("arguments").cloned().unwrap_or(Value::Null));
                        conv.tool(ToolCall::new(id, s(f, "name").unwrap_or("tool"), args), ts, None);
                    }
                }
                conv.text(&text, ts, None);
                if totals.is_none() {
                    if let Some(n) = tokens.filter(|n| *n > 0) {
                        conv.add_usage(&Usage { output_tokens: n as u64, ..Default::default() });
                    }
                }
            }
            "tool" => {
                let id = tool_call_id.unwrap_or_default();
                let (out, failed) = tool_output(&text);
                if !conv.tool_result(&id, Some(out.clone()), failed, Vec::new()) {
                    // Orphan tool output: keep it visible as a completed call.
                    let mut t = ToolCall::new(id.clone(), tool_name.unwrap_or_else(|| "tool".into()), Value::Null);
                    t.output = Some(util::trim_output(&out));
                    t.is_error = failed;
                    conv.tool(t, ts, None);
                }
            }
            _ => {}
        }
    }
    let meta = Meta {
        native_id: info.native_id.clone(),
        path: Some(info.path.clone()),
        title: info.title.clone(),
        cwd: info.cwd.clone(),
        branch,
        updated: info.updated_at,
        usage: totals,
        ..Default::default()
    };
    let mut session = finish(conv, AGENT, meta)?;
    session.external_turn = open && !ended;
    Ok(session)
}

#[cfg(test)]
mod tests {
    #[test]
    fn decodes_json_content() {
        assert_eq!(super::decode_content("\u{0}json:[{\"type\":\"text\",\"text\":\"hi\"}]"), "hi");
        assert_eq!(super::decode_content("plain"), "plain");
    }

    #[test]
    fn reads_tool_answers() {
        use super::tool_output;
        assert_eq!(tool_output(r#"{"output": "ok", "exit_code": 0, "error": null}"#), ("ok".to_string(), false));
        assert_eq!(tool_output(r#"{"output": "", "exit_code": 2, "error": "boom"}"#), ("boom".to_string(), true));
        assert_eq!(tool_output(r#"{"content": "1|fn main", "total_lines": 1}"#), ("1|fn main".to_string(), false));
        assert_eq!(tool_output(r#"{"total_count": 0, "error": "Path not found: src"}"#), ("Path not found: src".to_string(), true));
        assert_eq!(tool_output(r#"{"total_count": 0}"#), ("No matches".to_string(), false));
        assert_eq!(tool_output(r#"{"bytes_written": 12, "dirs_created": true}"#), ("Wrote 12 bytes".to_string(), false));
        assert_eq!(tool_output(r#"{"success": false, "error": "denied"}"#), ("denied".to_string(), true));
        assert_eq!(tool_output("plain text"), ("plain text".to_string(), false));
    }
}
