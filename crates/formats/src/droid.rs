//! Factory Droid: `~/.factory/sessions/<encoded-cwd>/<uuid>.jsonl`
//! (`session_start`, Claude-style `message` records, `todo_state`, …) with a
//! `<uuid>.settings.json` beside it (model, autonomy, token usage).

use std::path::{Path, PathBuf};

use anyhow::Result;
use larrier_core::{AgentKind, Part, Session, ToolCall, Usage};
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
            if let Some(info) = file_info(&f.path()) {
                out.push(info);
            }
        }
    }
    out
}

/// Session info for one Droid session file.
pub fn file_info(p: &Path) -> Option<NativeSessionInfo> {
    if p.extension().map(|x| x != "jsonl").unwrap_or(true) {
        return None;
    }
    let first = head_lines(p, 1).into_iter().next()?;
    let v = serde_json::from_str::<Value>(&first).ok()?;
    if s(&v, "type") != Some("session_start") {
        return None;
    }
    let id = string(&v, "id").unwrap_or_else(|| p.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
    let mut info = util::info(AGENT, id, p);
    info.title = string(&v, "sessionTitle").or_else(|| string(&v, "title")).filter(|t| !t.is_empty());
    info.cwd = string(&v, "cwd");
    Some(info)
}

pub fn import(info: &NativeSessionInfo) -> Result<Session> {
    parse(&util::read_text(&info.path)?, Some(&info.path))
}

pub fn parse(text: &str, path: Option<&Path>) -> Result<Session> {
    let settings = path
        .map(|p| p.with_extension("settings.json"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .unwrap_or(Value::Null);
    parse_with(text, path, &settings)
}

fn parse_with(text: &str, path: Option<&Path>, settings: &Value) -> Result<Session> {
    let mut conv = Conv::new(AGENT);
    let mut meta = Meta { path: path.map(PathBuf::from), ..Default::default() };
    meta.usage = usage(&settings["tokenUsage"]);
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
                // Context Droid injects for the model only (tool lists, dates).
                if s(m, "visibility") == Some("llm_only") {
                    continue;
                }
                let blocks = m["content"].as_array().cloned().unwrap_or_default();
                let model = s(m, "modelId");
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
                                Some("text") => conv.text(s(b, "text").unwrap_or(""), ts, model),
                                Some("thinking") => conv.reasoning(s(b, "thinking").unwrap_or(""), b["durationMs"].as_u64(), ts, model),
                                Some("tool_use") => {
                                    let t = ToolCall::new(s(b, "id").unwrap_or(""), s(b, "name").unwrap_or("tool"), b.get("input").cloned().unwrap_or(Value::Null));
                                    conv.tool(t, ts, model);
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
    let mut session = finish(conv, AGENT, meta)?;
    if session.model.is_none() {
        session.model = string(settings, "model").filter(|m| m != "auto");
    }
    // Autonomy as the session ran; "off" is Droid's default (read only).
    session.permission = match s(settings, "autonomyLevel") {
        Some(l @ ("low" | "medium" | "high")) => Some(l.to_string()),
        _ => None,
    };
    if let Some(p) = &session.permission {
        session.extra.insert("import_permission".into(), Value::String(p.clone()));
    }
    Ok(session)
}

/// Session totals from `settings.json` (`tokenUsage`).
fn usage(u: &Value) -> Option<Usage> {
    let n = |k: &str| u[k].as_u64().unwrap_or(0);
    let usage = Usage {
        input_tokens: n("inputTokens"),
        output_tokens: n("outputTokens"),
        cache_read_tokens: n("cacheReadTokens"),
        cache_write_tokens: n("cacheCreationTokens"),
        reasoning_tokens: n("thinkingTokens"),
        cost_usd: None,
    };
    (usage.input_tokens + usage.output_tokens > 0).then_some(usage)
}

#[cfg(test)]
mod tests {
    use larrier_core::{Part, PlanStatus, ToolKind};

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

    #[test]
    fn reads_models_text_todos_and_settings() {
        let t = r#"{"type":"session_start","id":"D2","title":"Review","sessionTitle":"Review","cwd":"/r","version":2}
{"type":"message","id":"context-1","timestamp":"2026-01-01T00:00:00Z","message":{"role":"user","content":[{"type":"text","text":"Tools available: Read, Execute"}],"visibility":"llm_only"}}
{"type":"message","id":"m1","timestamp":"2026-01-01T00:00:01Z","message":{"role":"user","content":[{"type":"text","text":"review the change"}]}}
{"type":"message","id":"m2","timestamp":"2026-01-01T00:00:02Z","message":{"role":"assistant","content":[{"type":"thinking","signature":"","signatureProvider":"factory","durationMs":2371,"thinking":"read it"},{"type":"tool_use","id":"t1","name":"TodoWrite","input":{"todos":"1. [completed] Read the diff\n2. [in_progress] Run tests\n3. [pending] Write verdict"}},{"type":"tool_use","id":"t2","name":"FetchUrl","input":{"url":"https://example.com"}}],"modelId":"glm-5.2"}}"#;
        let settings = serde_json::json!({"model": "glm-5.2", "autonomyLevel": "high", "tokenUsage": {"inputTokens": 50, "outputTokens": 7, "cacheReadTokens": 3, "cacheCreationTokens": 0, "thinkingTokens": 2}});
        let s = super::parse_with(t, None, &settings).unwrap();
        assert_eq!(s.messages.len(), 2, "the llm_only context is skipped");
        assert_eq!(s.model.as_deref(), Some("glm-5.2"));
        assert_eq!(s.permission.as_deref(), Some("high"));
        assert_eq!((s.usage.input_tokens, s.usage.output_tokens, s.usage.reasoning_tokens), (50, 7, 2));
        let a = &s.messages[1];
        assert!(a.parts.iter().any(|p| matches!(p, Part::Reasoning { duration_ms: Some(2371), .. })));
        let plan = a.parts.iter().find_map(|p| match p { Part::Plan { entries } => Some(entries), _ => None }).unwrap();
        assert_eq!(plan.len(), 3);
        assert_eq!(plan[1].status, PlanStatus::InProgress);
        let fetch = a.tool_calls().find(|t| t.name == "FetchUrl").unwrap();
        assert_eq!(fetch.kind, ToolKind::Fetch);
    }
}
