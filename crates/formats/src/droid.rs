//! Factory Droid: `~/.factory/sessions/<encoded-cwd>/<uuid>.jsonl`
//! (`session_start`, Claude-style `message` records, `todo_state`, …).

use std::path::{Path, PathBuf};

use anyhow::Result;
use larrier_core::{AgentKind, Part, Session, ToolCall};
use serde_json::Value;

use crate::util::{self, content_text, finish, head_lines, json_lines, json_or_string, parse_ts, s, string, Conv, Meta};
use crate::NativeSessionInfo;

const AGENT: AgentKind = AgentKind::Droid;

pub fn root() -> PathBuf {
    util::home().join(".factory").join("sessions")
}

pub fn discover() -> Vec<NativeSessionInfo> {
    let mut out = Vec::new();
    let Ok(dirs) = std::fs::read_dir(root()) else { return out };
    for d in dirs.flatten() {
        let Ok(files) = std::fs::read_dir(d.path()) else { continue };
        for f in files.flatten() {
            let p = f.path();
            if p.extension().map(|x| x != "jsonl").unwrap_or(true) {
                continue;
            }
            let Some(first) = head_lines(&p, 1).into_iter().next() else { continue };
            let Ok(v) = serde_json::from_str::<Value>(&first) else { continue };
            if s(&v, "type") != Some("session_start") {
                continue;
            }
            let id = string(&v, "id").unwrap_or_else(|| p.file_stem().unwrap().to_string_lossy().to_string());
            let mut info = util::info(AGENT, id, &p);
            info.title = string(&v, "sessionTitle").or_else(|| string(&v, "title")).filter(|t| !t.is_empty());
            info.cwd = string(&v, "cwd");
            out.push(info);
        }
    }
    out
}

pub fn import(info: &NativeSessionInfo) -> Result<Session> {
    parse(&util::read_text(&info.path)?, Some(&info.path))
}

pub fn parse(text: &str, path: Option<&Path>) -> Result<Session> {
    let mut conv = Conv::new(AGENT);
    let mut meta = Meta { path: path.map(PathBuf::from), ..Default::default() };
    for v in json_lines(text) {
        let ts = s(&v, "timestamp").and_then(parse_ts);
        match s(&v, "type").unwrap_or("") {
            "session_start" => {
                meta.native_id = string(&v, "id").unwrap_or_default();
                meta.title = string(&v, "sessionTitle").or_else(|| string(&v, "title")).filter(|t| !t.is_empty());
                meta.cwd = string(&v, "cwd");
            }
            "compaction_state" => conv.notice("Context compacted", ts),
            "todo_state" => {
                let todos = &v["todos"]["todos"];
                let parsed = match todos {
                    Value::String(t) => json_or_string(t),
                    other => other.clone(),
                };
                if let Some(entries) = util::plan_from(&parsed) {
                    let m = conv.assistant(ts, None);
                    if let Some(Part::Plan { entries: e }) = m.parts.iter_mut().rev().find(|p| matches!(p, Part::Plan { .. })) {
                        *e = entries;
                    } else {
                        m.parts.push(Part::Plan { entries });
                    }
                }
            }
            "message" => {
                let m = &v["message"];
                let blocks = m["content"].as_array().cloned().unwrap_or_default();
                match s(m, "role").unwrap_or("") {
                    "user" => {
                        let mut parts = Vec::new();
                        for b in &blocks {
                            match s(b, "type") {
                                Some("text") => {
                                    let t = s(b, "text").unwrap_or("");
                                    if !t.starts_with("<system-reminder>") {
                                        parts.push(Part::Text { text: t.to_string() });
                                    }
                                }
                                Some("tool_result") => {
                                    let out = content_text(&b["content"]);
                                    conv.tool_result(s(b, "tool_use_id").unwrap_or(""), Some(out), b["is_error"].as_bool().unwrap_or(false), Vec::new());
                                }
                                _ => {}
                            }
                        }
                        if let Some(t) = m["content"].as_str() {
                            parts.push(Part::Text { text: t.to_string() });
                        }
                        conv.user_parts(parts, ts);
                    }
                    "assistant" => {
                        for b in &blocks {
                            match s(b, "type") {
                                Some("text") => conv.text(s(b, "text").unwrap_or(""), ts, None),
                                Some("thinking") => conv.reasoning(s(b, "thinking").unwrap_or(""), None, ts, None),
                                Some("tool_use") => {
                                    let t = ToolCall::new(s(b, "id").unwrap_or(""), s(b, "name").unwrap_or("tool"), b.get("input").cloned().unwrap_or(Value::Null));
                                    conv.tool(t, ts, None);
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    if meta.native_id.is_empty() {
        meta.native_id = path.and_then(|p| p.file_stem()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    }
    finish(conv, AGENT, meta)
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_session() {
        let t = r#"{"type":"session_start","id":"D1","title":"Fix CI","sessionTitle":"Fix CI","cwd":"/r","version":2}
{"type":"message","id":"m1","timestamp":"2026-01-01T00:00:00Z","message":{"role":"user","content":[{"type":"text","text":"fix ci"}]}}
{"type":"message","id":"m2","timestamp":"2026-01-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"tool_use","id":"t","name":"Execute","input":{"command":"cargo test"}}]}}
{"type":"message","id":"m3","timestamp":"2026-01-01T00:00:02Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t","content":"ok"}]}}
{"type":"todo_state","todos":{"todos":"[{\"content\":\"run tests\",\"status\":\"completed\"}]"}}"#;
        let s = super::parse(t, None).unwrap();
        assert_eq!(s.title, "Fix CI");
        assert_eq!(s.messages.len(), 2);
        assert_eq!(s.messages[1].tool_calls().next().unwrap().output.as_deref(), Some("ok"));
    }
}
