//! Claude Code: `~/.claude/projects/<encoded-cwd>/<sessionId>.jsonl`.
//!
//! Assistant replies are written one content block per line (sharing
//! `message.id`); the visible conversation is the `parentUuid` chain from the
//! newest message back to the root, continuing across compaction boundaries
//! through `logicalParentUuid`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::Utc;
use larrier_core::{lsf::title_from_prompt, AgentKind, FileDiff, NativeLink, Part, Role, Session, ToolCall, Usage};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::util::{self, content_text, env_dir, finish, head_lines, image_part, json_lines, parse_ts, s, string, tail_lines, Conv, Meta};
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
            let p = f.path();
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            if !name.ends_with(".jsonl") || name.starts_with("agent-") {
                continue;
            }
            let id = name.trim_end_matches(".jsonl").to_string();
            let mut info = util::info(AGENT, id, &p);
            if info.size < 64 {
                continue;
            }
            peek(&p, &mut info);
            out.push(info);
        }
    }
    out
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

    let mut conv = Conv::new(AGENT);
    let mut usage_by_msg: Vec<(String, Usage, usize)> = Vec::new();
    for (i, v) in lines.iter().enumerate() {
        let ty = s(v, "type").unwrap_or("");
        if ty == "system" {
            if s(v, "subtype") == Some("compact_boundary") && active.as_ref().map(|a| a.contains(&i)).unwrap_or(true) {
                conv.notice("Context compacted", s(v, "timestamp").and_then(parse_ts));
            }
            continue;
        }
        if !is_msg(v) {
            continue;
        }
        if let Some(a) = &active {
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
                    let t = clean_user_text(t);
                    if t.starts_with("[Request interrupted") {
                        conv.notice("Interrupted", ts);
                    } else if !t.is_empty() {
                        parts.push(Part::Text { text: t });
                    }
                }
                Value::Array(blocks) => {
                    for b in blocks {
                        match s(b, "type") {
                            Some("text") => {
                                let t = clean_user_text(s(b, "text").unwrap_or(""));
                                if t.starts_with("[Request interrupted") {
                                    conv.notice("Interrupted", ts);
                                } else if !t.is_empty() {
                                    parts.push(Part::Text { text: t });
                                }
                            }
                            Some("image") => {
                                let src = &b["source"];
                                if let Some(p) = image_part(string(src, "media_type"), s(src, "data"), s(src, "url")) {
                                    parts.push(p);
                                }
                            }
                            Some("tool_result") => {
                                let id = s(b, "tool_use_id").unwrap_or("");
                                let out = content_text(&b["content"]);
                                let is_err = b["is_error"].as_bool().unwrap_or(false);
                                let diffs = tool_meta.map(diffs_from_result).unwrap_or_default();
                                conv.tool_result(id, Some(out), is_err, diffs);
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
                        let t = ToolCall::new(s(b, "id").unwrap_or(""), s(b, "name").unwrap_or("tool"), b.get("input").cloned().unwrap_or(Value::Null));
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
    if let Some(p) = path {
        meta.resume_path = Some(p.to_string_lossy().to_string());
    }
    finish(conv, AGENT, meta)
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
