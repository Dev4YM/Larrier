//! Gemini CLI: `~/.gemini/tmp/<project>/chats/session-*.jsonl` (JSONL since
//! v0.39; earlier versions wrote a single `.json` document).

use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{DateTime, Utc};
use larrier_core::{AgentKind, NativeLink, Role, Session, ToolCall, Usage};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::util::{self, env_dir, finish, head_lines, json_lines, parse_ts, s, string, Conv, Meta};
use crate::NativeSessionInfo;

const AGENT: AgentKind = AgentKind::Gemini;

pub fn root(home_override: Option<&Path>) -> PathBuf {
    home_override
        .map(PathBuf::from)
        .or_else(|| env_dir("GEMINI_CLI_HOME").map(|p| p.join(".gemini")))
        .unwrap_or_else(|| util::home().join(".gemini"))
}

/// slug → project path, from `projects.json`.
fn project_map(root: &Path) -> Vec<(String, String)> {
    let Ok(t) = std::fs::read_to_string(root.join("projects.json")) else { return Vec::new() };
    let v: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
    v["projects"].as_object().map(|m| m.iter().filter_map(|(path, slug)| slug.as_str().map(|s| (s.to_string(), path.clone()))).collect()).unwrap_or_default()
}

pub fn discover() -> Vec<NativeSessionInfo> {
    let root = root(None);
    let tmp = root.join("tmp");
    let projects = project_map(&root);
    let mut out = Vec::new();
    let Ok(dirs) = std::fs::read_dir(&tmp) else { return out };
    for d in dirs.flatten() {
        let chats = d.path().join("chats");
        let Ok(files) = std::fs::read_dir(&chats) else { continue };
        let slug = d.file_name().to_string_lossy().to_string();
        let cwd = projects.iter().find(|(s, _)| *s == slug).map(|(_, p)| p.clone());
        for f in files.flatten() {
            let p = f.path();
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            if !name.starts_with("session-") || !(name.ends_with(".json") || name.ends_with(".jsonl")) {
                continue;
            }
            let Some(first) = head_lines(&p, 1).into_iter().next() else { continue };
            let head: Value = if name.ends_with(".json") {
                std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null)
            } else {
                serde_json::from_str(&first).unwrap_or(Value::Null)
            };
            if s(&head, "kind") == Some("subagent") {
                continue;
            }
            let id = string(&head, "sessionId").unwrap_or_else(|| name.clone());
            let mut info = util::info(AGENT, id, &p);
            info.cwd = cwd.clone();
            out.push(info);
        }
    }
    out
}

pub fn import(info: &NativeSessionInfo) -> Result<Session> {
    let text = util::read_text(&info.path)?;
    let mut s = parse(&text, Some(&info.path))?;
    if s.workspace.is_none() {
        if let Some(c) = &info.cwd {
            s.workspace = Some(larrier_core::WorkspaceRef::from_path(c.clone()));
        }
    }
    Ok(s)
}

/// Parse either the JSONL record log or the legacy single JSON document.
pub fn parse(text: &str, path: Option<&Path>) -> Result<Session> {
    let trimmed = text.trim_start();
    let mut header = Value::Null;
    let mut records: Vec<Value> = Vec::new();
    let doc: Option<Value> = if !trimmed.contains('\n') || trimmed.starts_with("{\n") || trimmed.starts_with("{\r") {
        serde_json::from_str(trimmed).ok()
    } else {
        None
    };
    if let Some(d) = doc.filter(|d| d.get("messages").is_some()) {
        header = d.clone();
        records = d["messages"].as_array().cloned().unwrap_or_default();
    } else {
        for v in json_lines(text) {
            if v.get("sessionId").is_some() && v.get("type").is_none() && header.is_null() {
                header = v;
            } else if let Some(set) = v.get("$set") {
                if let (Value::Object(h), Value::Object(m)) = (&mut header, set) {
                    for (k, x) in m {
                        h.insert(k.clone(), x.clone());
                    }
                }
            } else if let Some(to) = v.get("$rewindTo").and_then(|x| x.as_str()) {
                if let Some(i) = records.iter().position(|r| s(r, "id") == Some(to)) {
                    records.truncate(i);
                }
            } else if v.get("$patch").is_some() {
                // Partial updates to an existing record.
                if let Some(id) = v["$patch"]["id"].as_str() {
                    if let Some(r) = records.iter_mut().find(|r| s(r, "id") == Some(id)) {
                        if let (Value::Object(r), Value::Object(p)) = (r, &v["$patch"]) {
                            for (k, x) in p {
                                r.insert(k.clone(), x.clone());
                            }
                        }
                    }
                }
            } else if v.get("type").is_some() {
                // A repeated id replaces the earlier record.
                if let Some(id) = s(&v, "id") {
                    if let Some(r) = records.iter_mut().find(|r| s(r, "id") == Some(id)) {
                        *r = v;
                        continue;
                    }
                }
                records.push(v);
            }
        }
    }
    let mut conv = Conv::new(AGENT);
    for r in &records {
        let ts = s(r, "timestamp").and_then(parse_ts);
        let model = s(r, "model");
        match s(r, "type").unwrap_or("") {
            "user" => {
                let t = r.get("displayContent").filter(|d| !d.is_null()).map(gemini_text).unwrap_or_else(|| gemini_text(&r["content"]));
                conv.user(t, ts);
            }
            "gemini" | "model" | "assistant" => {
                for th in r["thoughts"].as_array().into_iter().flatten() {
                    let t = match (s(th, "subject"), s(th, "description")) {
                        (Some(a), Some(b)) => format!("**{a}** {b}"),
                        (Some(a), None) => a.to_string(),
                        (None, Some(b)) => b.to_string(),
                        _ => String::new(),
                    };
                    conv.reasoning(&t, None, ts, model);
                }
                for tc in r["toolCalls"].as_array().into_iter().flatten() {
                    let id = s(tc, "id").unwrap_or("").to_string();
                    let tool = ToolCall::new(id.clone(), s(tc, "name").unwrap_or("tool"), tc.get("args").cloned().unwrap_or(Value::Null));
                    conv.tool(tool, ts, model);
                    let out = tool_result_text(&tc["result"]).or_else(|| tc.get("resultDisplay").and_then(|d| d.as_str()).map(String::from));
                    let err = matches!(s(tc, "status"), Some("error") | Some("cancelled"));
                    conv.tool_result(&id, out, err, Vec::new());
                }
                conv.text(&gemini_text(&r["content"]), ts, model);
                if let Some(t) = r.get("tokens") {
                    conv.add_usage(&Usage {
                        input_tokens: t["input"].as_u64().unwrap_or(0),
                        output_tokens: t["output"].as_u64().unwrap_or(0),
                        cache_read_tokens: t["cached"].as_u64().unwrap_or(0),
                        reasoning_tokens: t["thoughts"].as_u64().unwrap_or(0),
                        ..Default::default()
                    });
                }
            }
            "error" => conv.error(&gemini_text(&r["content"]), ts),
            "info" | "warning" => {
                let t = gemini_text(&r["content"]);
                if !t.trim().is_empty() {
                    conv.notice(t, ts);
                }
            }
            _ => {}
        }
    }
    let native_id = string(&header, "sessionId")
        .or_else(|| path.and_then(|p| p.file_stem()).map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_default();
    let meta = Meta {
        native_id,
        path: path.map(PathBuf::from),
        title: string(&header, "summary").or_else(|| string(&header, "title")),
        created: s(&header, "startTime").and_then(parse_ts),
        updated: s(&header, "lastUpdated").and_then(parse_ts),
        cwd: header["directories"].as_array().and_then(|d| d.first()).and_then(|d| d.as_str()).map(String::from),
        resume_path: path.map(|p| p.to_string_lossy().to_string()),
        ..Default::default()
    };
    finish(conv, AGENT, meta)
}

fn gemini_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().map(gemini_text).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n"),
        Value::Object(o) => {
            if o.get("thought").and_then(|t| t.as_bool()).unwrap_or(false) {
                return String::new();
            }
            o.get("text").and_then(|t| t.as_str()).map(String::from).unwrap_or_default()
        }
        _ => String::new(),
    }
}

fn tool_result_text(v: &Value) -> Option<String> {
    let items: Vec<&Value> = match v {
        Value::Array(a) => a.iter().collect(),
        Value::Null => return None,
        other => vec![other],
    };
    let mut out = Vec::new();
    for i in items {
        if let Some(r) = i.get("functionResponse") {
            let resp = &r["response"];
            let t = resp.get("output").or_else(|| resp.get("error")).or_else(|| resp.get("content")).map(|x| match x {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            });
            if let Some(t) = t {
                out.push(t);
            }
        } else if let Some(t) = i.get("text").and_then(|t| t.as_str()) {
            out.push(t.to_string());
        } else if let Some(t) = i.as_str() {
            out.push(t.to_string());
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out.join("\n"))
    }
}

// ---------------------------------------------------------------- export

pub fn project_slug(cwd: &str) -> String {
    let base = Path::new(cwd).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "project".into());
    let slug: String = base.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    if slug.trim_matches('-').is_empty() {
        "project".into()
    } else {
        slug
    }
}

pub fn export_lines(session: &Session, sid: &str, cwd: &str, now: DateTime<Utc>) -> String {
    let ts = |t: DateTime<Utc>| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let hash = hex::encode(Sha256::digest(cwd.as_bytes()));
    let mut out = String::new();
    out.push_str(
        &json!({"sessionId": sid, "projectHash": hash, "startTime": ts(session.created_at), "lastUpdated": ts(now), "kind": "main", "directories": [cwd]}).to_string(),
    );
    out.push('\n');
    for (role, text, t, model) in util::turns(session) {
        let rec = match role {
            Role::User => json!({"id": uuid::Uuid::new_v4().to_string(), "timestamp": ts(t), "type": "user", "content": [{"text": text}]}),
            _ => json!({"id": uuid::Uuid::new_v4().to_string(), "timestamp": ts(t), "type": "gemini", "content": text, "model": model.filter(|m| m.starts_with("gemini")).unwrap_or_else(|| "gemini-2.5-pro".into())}),
        };
        out.push_str(&rec.to_string());
        out.push('\n');
    }
    out
}

pub fn export_native(session: &Session, home_override: Option<&Path>) -> Result<NativeLink> {
    let root = root(home_override);
    let cwd = util::session_cwd(session);
    let key = if cfg!(windows) { cwd.to_lowercase() } else { cwd.clone() };
    // Reuse or register the project's slug.
    let pj_path = root.join("projects.json");
    let mut pj: Value = std::fs::read_to_string(&pj_path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| json!({"projects": {}}));
    if !pj["projects"].is_object() {
        pj["projects"] = json!({});
    }
    let slug = match pj["projects"].get(&key).and_then(|s| s.as_str()) {
        Some(s) => s.to_string(),
        None => {
            let base = project_slug(&cwd);
            let taken: Vec<String> = pj["projects"].as_object().unwrap().values().filter_map(|v| v.as_str().map(String::from)).collect();
            let mut slug = base.clone();
            let mut n = 1;
            while taken.contains(&slug) {
                slug = format!("{base}-{n}");
                n += 1;
            }
            pj["projects"][&key] = json!(slug);
            util::write_new_file(&pj_path, &serde_json::to_string_pretty(&pj)?)?;
            slug
        }
    };
    let now = Utc::now();
    let sid = uuid::Uuid::new_v4().to_string();
    let local = now.with_timezone(&chrono::Local);
    let path = root.join("tmp").join(&slug).join("chats").join(format!("session-{}-{}.jsonl", local.format("%Y-%m-%dT%H-%M"), &sid[..8]));
    util::write_new_file(&path, &export_lines(session, &sid, &cwd, now))?;
    Ok(NativeLink { agent: AGENT, id: sid, path: Some(path.to_string_lossy().to_string()), synced_messages: session.messages.len(), account_id: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_jsonl_and_legacy() {
        let jl = [
            r#"{"sessionId":"G1","projectHash":"h","startTime":"2026-01-01T00:00:00Z","lastUpdated":"2026-01-01T00:01:00Z","kind":"main"}"#,
            r#"{"id":"1","timestamp":"2026-01-01T00:00:01Z","type":"user","content":[{"text":"explain this repo"}]}"#,
            r#"{"id":"2","timestamp":"2026-01-01T00:00:02Z","type":"gemini","content":"partial","model":"gemini-2.5-pro"}"#,
            r#"{"id":"2","timestamp":"2026-01-01T00:00:03Z","type":"gemini","content":"It is a Rust app.","model":"gemini-2.5-pro","thoughts":[{"subject":"Scan","description":"look at files"}],"toolCalls":[{"id":"t1","name":"read_file","args":{"file_path":"a.rs"},"status":"success","result":[{"functionResponse":{"response":{"output":"fn main"}}}]}],"tokens":{"input":3,"output":4}}"#,
            r#"{"$set":{"summary":"Repo explainer"}}"#,
        ];
        let s = parse(&jl.join("\n"), None).unwrap();
        assert_eq!(s.title, "Repo explainer");
        assert_eq!(s.messages.len(), 2);
        assert!(s.messages[1].text().contains("Rust app"));
        assert!(!s.messages[1].text().contains("partial"));
        assert_eq!(s.messages[1].tool_calls().next().unwrap().output.as_deref(), Some("fn main"));

        let legacy = r#"{"sessionId":"G2","projectHash":"h","startTime":"2026-01-01T00:00:00Z","messages":[{"id":"1","timestamp":"2026-01-01T00:00:01Z","type":"user","content":"hi"},{"id":"2","timestamp":"2026-01-01T00:00:02Z","type":"gemini","content":"hello"}]}"#;
        let s = parse(legacy, None).unwrap();
        assert_eq!(s.messages.len(), 2);
        assert_eq!(s.origin.native_id.as_deref(), Some("G2"));
    }

    #[test]
    fn export_roundtrip() {
        let mut s = Session::new(AGENT, Some(larrier_core::WorkspaceRef::from_path("/tmp/x")));
        s.push(larrier_core::Message::user_text("hello"));
        let mut a = larrier_core::Message::assistant(AGENT, None);
        a.push_text("world");
        s.push(a);
        let back = parse(&export_lines(&s, "SID", "/tmp/x", Utc::now()), None).unwrap();
        assert_eq!(back.messages.len(), 2);
        assert_eq!(back.messages[0].text(), "hello");
        assert_eq!(back.messages[1].text(), "world");
    }
}
