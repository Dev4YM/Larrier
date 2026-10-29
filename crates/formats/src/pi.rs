//! Pi coding agent: `~/.pi/agent/sessions/--<cwd>--/<ts>_<uuid>.jsonl`.
//! A header line, then entries linked by `parentId`; the active branch is
//! the path from the last entry back to the root.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{DateTime, Utc};
use larrier_core::{AgentKind, NativeLink, Part, Role, Session, ToolCall, Usage};
use serde_json::{json, Value};

use crate::util::{self, content_text, env_dir, finish, head_lines, image_parts, json_lines, parse_ts, s, string, ts_value, Conv, Meta};
use crate::NativeSessionInfo;

const AGENT: AgentKind = AgentKind::Pi;

pub fn root(home_override: Option<&Path>) -> PathBuf {
    home_override.map(PathBuf::from).or_else(|| env_dir("PI_CODING_AGENT_DIR")).unwrap_or_else(|| util::home().join(".pi").join("agent"))
}

pub fn encode_cwd(cwd: &str) -> String {
    let t = cwd.trim_start_matches(['/', '\\']);
    let enc: String = t.chars().map(|c| if c == '/' || c == '\\' || c == ':' { '-' } else { c }).collect();
    format!("--{enc}--")
}

pub fn discover() -> Vec<NativeSessionInfo> {
    let dir = root(None).join("sessions");
    let mut out = Vec::new();
    if !dir.exists() {
        return out;
    }
    for e in walkdir::WalkDir::new(&dir).max_depth(2).into_iter().flatten() {
        if let Some(info) = file_info(e.path()) {
            out.push(info);
        }
    }
    out
}

/// Session info for one Pi session file.
pub fn file_info(p: &Path) -> Option<NativeSessionInfo> {
    if p.extension().map(|x| x != "jsonl").unwrap_or(true) {
        return None;
    }
    let first = head_lines(p, 1).into_iter().next()?;
    let h = serde_json::from_str::<Value>(&first).ok()?;
    if s(&h, "type") != Some("session") {
        return None;
    }
    let mut info = util::info(AGENT, s(&h, "id").unwrap_or_default(), p);
    info.cwd = string(&h, "cwd");
    Some(info)
}

pub fn import(info: &NativeSessionInfo) -> Result<Session> {
    parse(&util::read_text(&info.path)?, Some(&info.path))
}

pub fn parse(text: &str, path: Option<&Path>) -> Result<Session> {
    let lines = json_lines(text);
    let header = lines.iter().find(|v| s(v, "type") == Some("session")).cloned().unwrap_or(Value::Null);
    let entries: Vec<&Value> = lines.iter().filter(|v| s(v, "type") != Some("session")).collect();
    let by_id: HashMap<&str, usize> = entries.iter().enumerate().filter_map(|(i, e)| s(e, "id").map(|id| (id, i))).collect();
    // Active branch: walk from the leaf.
    let mut chain: Vec<usize> = Vec::new();
    if let Some(mut cur) = entries.len().checked_sub(1) {
        let mut guard = 0;
        loop {
            chain.push(cur);
            guard += 1;
            let parent = s(entries[cur], "parentId");
            match parent.and_then(|p| by_id.get(p)) {
                Some(&p) if guard <= entries.len() => cur = p,
                _ => break,
            }
        }
        chain.reverse();
        // Unlinked files (no parent ids): use file order.
        if chain.len() < entries.len() / 3 {
            chain = (0..entries.len()).collect();
        }
    }
    let mut conv = Conv::new(AGENT);
    let mut title = None;
    // Whether Pi is still mid-turn: the last message is a prompt, a tool
    // result or a reply that called tools.
    let mut open = false;
    for &i in &chain {
        let e = entries[i];
        let ts = e.get("timestamp").and_then(ts_value);
        if s(e, "type") == Some("message") {
            let m = &e["message"];
            open = match s(m, "role") {
                Some("user") | Some("toolResult") => true,
                Some("assistant") => s(m, "stopReason") == Some("toolUse"),
                _ => open,
            };
        }
        match s(e, "type").unwrap_or("") {
            "session_info" => title = string(e, "name").or(title.take()),
            "compaction" => conv.notice("Context compacted", ts),
            // Moving to another point of the session tree summarizes the
            // branch left behind.
            "branch_summary" => conv.notice("Summarized the branch left behind", ts),
            // Messages extensions inject; only those meant to be shown.
            "custom_message" if e["display"].as_bool().unwrap_or(false) => {
                let t = content_text(&e["content"]);
                if !t.trim().is_empty() {
                    conv.notice(larrier_core::lsf::first_line(&t), ts);
                }
            }
            "model_change" => {
                if let Some(m) = s(e, "modelId") {
                    conv.model = Some(m.to_string());
                }
            }
            "message" => {
                let m = &e["message"];
                let mts = m.get("timestamp").and_then(ts_value).or(ts);
                match s(m, "role").unwrap_or("") {
                    "user" => {
                        let mut parts = Vec::new();
                        match &m["content"] {
                            Value::String(t) => parts.push(Part::Text { text: t.clone() }),
                            Value::Array(a) => {
                                for c in a {
                                    match s(c, "type") {
                                        Some("text") => parts.push(Part::Text { text: s(c, "text").unwrap_or("").to_string() }),
                                        Some("image") => {
                                            if let Some(p) = util::image_part(string(c, "mimeType"), s(c, "data"), None) {
                                                parts.push(p);
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            _ => {}
                        }
                        conv.user_parts(parts, mts);
                    }
                    "assistant" => {
                        let model = s(m, "model");
                        for c in m["content"].as_array().into_iter().flatten() {
                            match s(c, "type") {
                                Some("text") => conv.text(s(c, "text").unwrap_or(""), mts, model),
                                Some("thinking") => conv.reasoning(s(c, "thinking").unwrap_or(""), None, mts, model),
                                Some("toolCall") => {
                                    let t = ToolCall::new(s(c, "id").unwrap_or(""), s(c, "name").unwrap_or("tool"), c.get("arguments").cloned().unwrap_or(Value::Null));
                                    conv.tool(t, mts, model);
                                }
                                _ => {}
                            }
                        }
                        if let Some(u) = m.get("usage") {
                            conv.add_usage(&Usage {
                                input_tokens: u["input"].as_u64().unwrap_or(0),
                                output_tokens: u["output"].as_u64().unwrap_or(0),
                                cache_read_tokens: u["cacheRead"].as_u64().unwrap_or(0),
                                cache_write_tokens: u["cacheWrite"].as_u64().unwrap_or(0),
                                reasoning_tokens: 0,
                                cost_usd: u["cost"]["total"].as_f64().filter(|c| *c > 0.0),
                                context_tokens: Some(u["input"].as_u64().unwrap_or(0) + u["cacheRead"].as_u64().unwrap_or(0) + u["cacheWrite"].as_u64().unwrap_or(0)),
                                ..Default::default()
                            });
                        }
                        match s(m, "stopReason") {
                            Some("error") => {
                                if let Some(err) = s(m, "errorMessage") {
                                    conv.error(err, mts);
                                }
                            }
                            Some("aborted") => conv.notice("Interrupted", mts),
                            _ => {}
                        }
                    }
                    "toolResult" => {
                        let id = s(m, "toolCallId").unwrap_or("");
                        let out = content_text(&m["content"]);
                        conv.tool_result(id, Some(out), m["isError"].as_bool().unwrap_or(false), Vec::new());
                        conv.tool_images(id, image_parts(&m["content"]));
                    }
                    // A command the user ran with `!` (or `!!`, kept out of
                    // the model's context).
                    "bashExecution" => {
                        let mut t = ToolCall::new(larrier_core::lsf::new_id(), "bash", json!({"command": m["command"]}));
                        let code = m["exitCode"].as_i64();
                        t.output = Some(util::trim_output(&string(m, "output").unwrap_or_default()));
                        t.is_error = m["cancelled"].as_bool().unwrap_or(false) || code.is_some_and(|c| c != 0);
                        t.status = larrier_core::ToolStatus::Completed;
                        conv.tool(t, mts, None);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    let meta = Meta {
        native_id: string(&header, "id").unwrap_or_default(),
        path: path.map(PathBuf::from),
        title,
        cwd: string(&header, "cwd"),
        created: s(&header, "timestamp").and_then(parse_ts),
        resume_path: path.map(|p| p.to_string_lossy().to_string()),
        ..Default::default()
    };
    let mut session = finish(conv, AGENT, meta)?;
    session.external_turn = open;
    Ok(session)
}

// ---------------------------------------------------------------- export

pub fn export_lines(session: &Session, id: &str, cwd: &str) -> String {
    let ts = |t: DateTime<Utc>| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let mut out = String::new();
    out.push_str(&json!({"type": "session", "version": 3, "id": id, "timestamp": ts(session.created_at), "cwd": cwd}).to_string());
    out.push('\n');
    let mut parent: Option<String> = None;
    let mut n: u32 = 0;
    let next_id = |n: &mut u32| {
        *n += 1;
        format!("{:08x}", (*n as u64).wrapping_mul(2654435761) & 0xffff_ffff)
    };
    if !session.title.is_empty() {
        let eid = next_id(&mut n);
        out.push_str(&json!({"type": "session_info", "id": eid, "parentId": null, "timestamp": ts(session.created_at), "name": session.title}).to_string());
        out.push('\n');
        parent = Some(eid);
    }
    for (role, text, t, model) in util::turns(session) {
        let ms = t.timestamp_millis();
        let body = match role {
            Role::User => json!({"role": "user", "content": [{"type": "text", "text": text}], "timestamp": ms}),
            _ => json!({
                "role": "assistant",
                "content": [{"type": "text", "text": text}],
                "api": "anthropic-messages",
                "provider": "anthropic",
                "model": model.unwrap_or_else(|| "claude-sonnet-4-5".into()),
                "usage": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 0, "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0}},
                "stopReason": "stop",
                "timestamp": ms
            }),
        };
        let eid = next_id(&mut n);
        let mut v = json!({"type": "message", "id": eid, "parentId": parent, "timestamp": ts(t)});
        v["message"] = body;
        out.push_str(&v.to_string());
        out.push('\n');
        parent = Some(eid);
    }
    out
}

pub fn export_native(session: &Session, home_override: Option<&Path>) -> Result<NativeLink> {
    let cwd = util::session_cwd(session);
    let id = uuid::Uuid::now_v7().to_string();
    let stamp = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true).replace([':', '.'], "-");
    let path = root(home_override).join("sessions").join(encode_cwd(&cwd)).join(format!("{stamp}_{id}.jsonl"));
    util::write_new_file(&path, &export_lines(session, &id, &cwd))?;
    Ok(NativeLink { agent: AGENT, id, path: Some(path.to_string_lossy().to_string()), synced_messages: session.messages.len(), account_id: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_active_branch() {
        let t = [
            r#"{"type":"session","version":3,"id":"P1","timestamp":"2026-01-01T00:00:00Z","cwd":"/r"}"#,
            r#"{"type":"message","id":"a","parentId":null,"timestamp":"2026-01-01T00:00:01Z","message":{"role":"user","content":"first","timestamp":1}}"#,
            r#"{"type":"message","id":"b","parentId":"a","timestamp":"2026-01-01T00:00:02Z","message":{"role":"assistant","content":[{"type":"text","text":"old answer"}],"model":"m"}}"#,
            r#"{"type":"message","id":"c","parentId":"a","timestamp":"2026-01-01T00:00:03Z","message":{"role":"assistant","content":[{"type":"toolCall","id":"t1","name":"bash","arguments":{"command":"ls"}}],"model":"m","usage":{"input":2,"output":3}}}"#,
            r#"{"type":"message","id":"d","parentId":"c","timestamp":"2026-01-01T00:00:04Z","message":{"role":"toolResult","toolCallId":"t1","content":[{"type":"text","text":"a.rs"}],"isError":false}}"#,
            r#"{"type":"message","id":"e","parentId":"d","timestamp":"2026-01-01T00:00:05Z","message":{"role":"assistant","content":[{"type":"text","text":"new answer"}],"model":"m"}}"#,
        ];
        let s = parse(&t.join("\n"), None).unwrap();
        assert_eq!(s.messages.len(), 2);
        let a = s.messages[1].text();
        assert!(a.contains("new answer") && !a.contains("old answer"));
        assert_eq!(s.messages[1].tool_calls().next().unwrap().output.as_deref(), Some("a.rs"));
        assert!(!s.external_turn);
    }

    #[test]
    fn user_shell_runs_aborts_tool_images_and_open_turns() {
        let t = [
            r#"{"type":"session","version":3,"id":"P2","timestamp":"2026-01-01T00:00:00Z","cwd":"/r"}"#,
            r#"{"type":"message","id":"a","parentId":null,"timestamp":"2026-01-01T00:00:01Z","message":{"role":"user","content":"look at the screenshot"}}"#,
            r#"{"type":"message","id":"b","parentId":"a","timestamp":"2026-01-01T00:00:02Z","message":{"role":"assistant","content":[{"type":"toolCall","id":"t1","name":"read","arguments":{"path":"shot.png"}}],"model":"m","stopReason":"toolUse"}}"#,
            r#"{"type":"message","id":"c","parentId":"b","timestamp":"2026-01-01T00:00:03Z","message":{"role":"toolResult","toolCallId":"t1","content":[{"type":"text","text":"Read image file"},{"type":"image","data":"iVBORw0KGgo=","mimeType":"image/png"}],"isError":false}}"#,
            r#"{"type":"message","id":"d","parentId":"c","timestamp":"2026-01-01T00:00:04Z","message":{"role":"assistant","content":[{"type":"text","text":"It shows"}],"model":"m","stopReason":"aborted"}}"#,
            r#"{"type":"message","id":"e","parentId":"d","timestamp":"2026-01-01T00:00:05Z","message":{"role":"bashExecution","command":"cargo test","output":"1 failed","exitCode":101,"cancelled":false,"truncated":false}}"#,
            r#"{"type":"message","id":"f","parentId":"e","timestamp":"2026-01-01T00:00:06Z","message":{"role":"user","content":"fix it"}}"#,
        ];
        let s = parse(&t.join("\n"), None).unwrap();
        assert!(s.external_turn, "the last prompt has no reply yet");
        let tools: Vec<&ToolCall> = s.messages.iter().flat_map(|m| m.tool_calls()).collect();
        assert_eq!(tools[0].images.len(), 1);
        let bash = tools.iter().find(|t| t.name == "bash").unwrap();
        assert!(bash.is_error);
        assert_eq!(bash.output.as_deref(), Some("1 failed"));
        assert!(s.messages.iter().flat_map(|m| &m.parts).any(|p| matches!(p, Part::Notice { text } if text == "Interrupted")));
    }

    #[test]
    fn export_roundtrip() {
        let mut s = Session::new(AGENT, Some(larrier_core::WorkspaceRef::from_path("/tmp/x")));
        s.title = "T".into();
        s.push(larrier_core::Message::user_text("hello"));
        let mut a = larrier_core::Message::assistant(AGENT, None);
        a.push_text("world");
        s.push(a);
        let back = parse(&export_lines(&s, "ID", "/tmp/x"), None).unwrap();
        assert_eq!(back.messages.len(), 2);
        assert_eq!(back.messages[0].text(), "hello");
        assert_eq!(back.messages[1].text(), "world");
        assert_eq!(back.title, "T");
        assert_eq!(encode_cwd(r"D:\src\P"), "--D--src-P--");
    }
}
