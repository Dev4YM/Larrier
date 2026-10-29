//! Chat app exports: ChatGPT `conversations.json` and claude.ai
//! `conversations.json`. Imported conversations are continued with Larrier's
//! built-in Chat agent.

use std::collections::HashMap;

use anyhow::Result;
use larrier_core::{AgentKind, Part, Session, ToolCall};
use serde_json::Value;

use crate::util::{self, content_text, finish_as, s, string, ts_value, Conv, Meta};

// ---------------------------------------------------------------- ChatGPT

pub fn is_chatgpt(v: &Value) -> bool {
    v.as_array().and_then(|a| a.first()).map(|c| c.get("mapping").is_some()).unwrap_or(false)
}

pub fn parse_chatgpt(v: &Value) -> Vec<Result<Session>> {
    v.as_array().into_iter().flatten().map(chatgpt_one).collect()
}

fn chatgpt_one(c: &Value) -> Result<Session> {
    let mapping = c["mapping"].as_object().cloned().unwrap_or_default();
    // Linearize from current_node up through parents.
    let mut chain: Vec<String> = Vec::new();
    let mut cur = string(c, "current_node").or_else(|| {
        // Fallback: deepest leaf.
        mapping.iter().filter(|(_, n)| n["children"].as_array().map(|a| a.is_empty()).unwrap_or(true)).map(|(k, _)| k.clone()).next_back()
    });
    let mut guard = 0;
    while let Some(id) = cur {
        if guard > mapping.len() {
            break;
        }
        guard += 1;
        let Some(node) = mapping.get(&id) else { break };
        chain.push(id.clone());
        cur = string(node, "parent");
    }
    chain.reverse();
    let default_model = string(c, "default_model_slug");
    let mut conv = Conv::new(AgentKind::Chat);
    conv.model = default_model.clone();
    for id in chain {
        let m = &mapping[&id]["message"];
        if m.is_null() {
            continue;
        }
        let md = &m["metadata"];
        if md["is_visually_hidden_from_conversation"].as_bool().unwrap_or(false) || md["is_user_system_message"].as_bool().unwrap_or(false) {
            continue;
        }
        let role = s(&m["author"], "role").unwrap_or("");
        let ts = m.get("create_time").and_then(ts_value);
        let model = s(md, "model_slug");
        let content = &m["content"];
        let ctype = s(content, "content_type").unwrap_or("text");
        match role {
            "user" => {
                let mut parts = Vec::new();
                for p in content["parts"].as_array().into_iter().flatten() {
                    match p {
                        Value::String(t) => parts.push(Part::Text { text: t.clone() }),
                        Value::Object(_) if s(p, "content_type") == Some("image_asset_pointer") => {
                            parts.push(Part::File { path: s(p, "asset_pointer").unwrap_or("image").to_string(), mime: Some("image/*".into()), text: None })
                        }
                        _ => {}
                    }
                }
                if parts.is_empty() {
                    if let Some(t) = s(content, "text") {
                        parts.push(Part::Text { text: t.to_string() });
                    }
                }
                for a in md["attachments"].as_array().into_iter().flatten() {
                    if let Some(n) = s(a, "name") {
                        parts.push(Part::File { path: n.to_string(), mime: string(a, "mime_type"), text: None });
                    }
                }
                conv.user_parts(parts, ts);
            }
            "assistant" => match ctype {
                "text" | "multimodal_text" => {
                    let t = content["parts"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default();
                    conv.text(&t, ts, model);
                }
                "code" => {
                    let recipient = s(m, "recipient").unwrap_or("all");
                    let code = s(content, "text").unwrap_or("");
                    if recipient == "all" {
                        conv.text(&format!("```{}\n{}\n```", s(content, "language").unwrap_or(""), code), ts, model);
                    } else {
                        let id = s(m, "id").unwrap_or("").to_string();
                        conv.tool(ToolCall::new(id, recipient, serde_json::json!({ "code": code })), ts, model);
                    }
                }
                "thoughts" => {
                    let t = content["thoughts"]
                        .as_array()
                        .map(|a| a.iter().map(|x| format!("{}\n{}", s(x, "summary").unwrap_or(""), s(x, "content").unwrap_or("")).trim().to_string()).collect::<Vec<_>>().join("\n\n"))
                        .unwrap_or_default();
                    conv.reasoning(&t, None, ts, model);
                }
                "reasoning_recap" | "tether_browsing_display" | "tether_quote" | "user_editable_context" => {}
                "system_error" => conv.error(s(content, "text").unwrap_or("error"), ts),
                _ => {
                    let t = content_text(&content["parts"]);
                    conv.text(&t, ts, model);
                }
            },
            "tool" => {
                let name = s(&m["author"], "name").unwrap_or("tool").to_string();
                let out = match ctype {
                    "execution_output" => s(content, "text").unwrap_or("").to_string(),
                    _ => content_text(&content["parts"]).trim().to_string(),
                };
                if out.is_empty() {
                    continue;
                }
                let id = s(m, "id").unwrap_or("").to_string();
                let mut t = ToolCall::new(id, name, Value::Null);
                t.output = Some(util::trim_output(&out));
                conv.tool(t, ts, model);
            }
            _ => {}
        }
    }
    let meta = Meta {
        native_id: string(c, "conversation_id").or_else(|| string(c, "id")).unwrap_or_default(),
        title: string(c, "title"),
        created: c.get("create_time").and_then(ts_value),
        updated: c.get("update_time").and_then(ts_value),
        archived: c["is_archived"].as_bool().unwrap_or(false),
        ..Default::default()
    };
    finish_as(conv, AgentKind::ChatGpt, AgentKind::Chat, meta)
}

// ---------------------------------------------------------------- claude.ai

pub fn is_claude_ai(v: &Value) -> bool {
    v.as_array().and_then(|a| a.first()).map(|c| c.get("chat_messages").is_some()).unwrap_or(false)
}

pub fn parse_claude_ai(v: &Value) -> Vec<Result<Session>> {
    v.as_array().into_iter().flatten().map(claude_ai_one).collect()
}

fn claude_ai_one(c: &Value) -> Result<Session> {
    let msgs: Vec<&Value> = c["chat_messages"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
    // Follow the branch ending at the current leaf when the export has one.
    let ordered: Vec<&Value> = match s(c, "current_leaf_message_uuid") {
        Some(leaf) => {
            let by: HashMap<&str, &Value> = msgs.iter().filter_map(|m| s(m, "uuid").map(|u| (u, *m))).collect();
            let mut chain = Vec::new();
            let mut cur = by.get(leaf).copied();
            while let Some(m) = cur {
                if chain.len() > msgs.len() {
                    break;
                }
                chain.push(m);
                cur = s(m, "parent_message_uuid").and_then(|p| by.get(p).copied());
            }
            chain.reverse();
            if chain.is_empty() {
                msgs.clone()
            } else {
                chain
            }
        }
        None => msgs.clone(),
    };
    let mut conv = Conv::new(AgentKind::Chat);
    for m in ordered {
        let ts = m.get("created_at").and_then(ts_value);
        let blocks = m["content"].as_array().cloned().unwrap_or_default();
        match s(m, "sender").unwrap_or("") {
            "human" => {
                let mut parts = Vec::new();
                let text = if blocks.is_empty() { s(m, "text").unwrap_or("").to_string() } else { content_text(&Value::Array(blocks.clone())) };
                parts.push(Part::Text { text });
                for a in m["attachments"].as_array().into_iter().flatten() {
                    parts.push(Part::File {
                        path: s(a, "file_name").unwrap_or("attachment").to_string(),
                        mime: string(a, "file_type"),
                        text: string(a, "extracted_content"),
                    });
                }
                for f in m["files"].as_array().into_iter().flatten() {
                    if let Some(n) = s(f, "file_name") {
                        parts.push(Part::File { path: n.to_string(), mime: None, text: None });
                    }
                }
                conv.user_parts(parts, ts);
            }
            "assistant" => {
                if blocks.is_empty() {
                    conv.text(s(m, "text").unwrap_or(""), ts, None);
                }
                for b in &blocks {
                    match s(b, "type") {
                        Some("text") => conv.text(s(b, "text").unwrap_or(""), ts, None),
                        Some("thinking") => conv.reasoning(s(b, "thinking").unwrap_or(""), None, ts, None),
                        Some("tool_use") => {
                            let id = s(b, "id").map(String::from).unwrap_or_else(|| format!("{}-{}", s(m, "uuid").unwrap_or(""), s(b, "name").unwrap_or("")));
                            conv.tool(ToolCall::new(id, s(b, "name").unwrap_or("tool"), b.get("input").cloned().unwrap_or(Value::Null)), ts, None);
                        }
                        Some("tool_result") => {
                            let out = content_text(&b["content"]);
                            let id = s(b, "tool_use_id").map(String::from).unwrap_or_else(|| format!("{}-{}", s(m, "uuid").unwrap_or(""), s(b, "name").unwrap_or("")));
                            if !conv.tool_result(&id, Some(out.clone()), b["is_error"].as_bool().unwrap_or(false), Vec::new()) {
                                // Results are often keyed by name only; attach to the latest call.
                                let last = conv.messages.last().and_then(|mm| mm.tool_calls().last().map(|t| t.id.clone()));
                                if let Some(lid) = last {
                                    conv.tool_result(&lid, Some(out), false, Vec::new());
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    let meta = Meta {
        native_id: string(c, "uuid").unwrap_or_default(),
        title: string(c, "name").filter(|n| !n.trim().is_empty()),
        created: c.get("created_at").and_then(ts_value),
        updated: c.get("updated_at").and_then(ts_value),
        ..Default::default()
    };
    finish_as(conv, AgentKind::ClaudeAi, AgentKind::Chat, meta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chatgpt_mapping() {
        let v: Value = serde_json::json!([{
            "id": "c1", "title": "Pasta", "create_time": 1700000000.0, "update_time": 1700000100.0,
            "current_node": "n3", "default_model_slug": "gpt-4o",
            "mapping": {
                "root": {"id": "root", "parent": null, "children": ["n1"], "message": null},
                "n1": {"id": "n1", "parent": "root", "children": ["n2", "x"], "message": {"author": {"role": "user"}, "create_time": 1700000001.0, "content": {"content_type": "text", "parts": ["how to cook pasta"]}, "metadata": {}}},
                "x": {"id": "x", "parent": "n1", "children": [], "message": {"author": {"role": "assistant"}, "content": {"content_type": "text", "parts": ["old"]}, "metadata": {}}},
                "n2": {"id": "n2", "parent": "n1", "children": ["n3"], "message": {"author": {"role": "assistant"}, "content": {"content_type": "thoughts", "thoughts": [{"summary": "Cooking", "content": "boil"}]}, "metadata": {}}},
                "n3": {"id": "n3", "parent": "n2", "children": [], "message": {"author": {"role": "assistant"}, "content": {"content_type": "text", "parts": ["Boil water."]}, "metadata": {"model_slug": "gpt-4o"}}}
            }
        }]);
        assert!(is_chatgpt(&v));
        let s = parse_chatgpt(&v).remove(0).unwrap();
        assert_eq!(s.title, "Pasta");
        assert_eq!(s.agent, AgentKind::Chat);
        assert_eq!(s.origin.agent, AgentKind::ChatGpt);
        assert_eq!(s.messages.len(), 2);
        assert_eq!(s.messages[1].text(), "Boil water.");
    }

    #[test]
    fn claude_ai_export() {
        let v: Value = serde_json::json!([{
            "uuid": "u1", "name": "Rust help", "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:01:00Z",
            "chat_messages": [
                {"uuid": "a", "sender": "human", "text": "what is a trait", "created_at": "2026-01-01T00:00:01Z", "content": [{"type": "text", "text": "what is a trait"}], "attachments": [{"file_name": "notes.txt", "extracted_content": "x"}]},
                {"uuid": "b", "sender": "assistant", "text": "A trait is...", "created_at": "2026-01-01T00:00:02Z", "content": [{"type": "thinking", "thinking": "explain"}, {"type": "text", "text": "A trait is..."}]}
            ]
        }]);
        assert!(is_claude_ai(&v));
        let s = parse_claude_ai(&v).remove(0).unwrap();
        assert_eq!(s.title, "Rust help");
        assert_eq!(s.origin.agent, AgentKind::ClaudeAi);
        assert_eq!(s.messages.len(), 2);
        assert!(s.messages[0].parts.iter().any(|p| matches!(p, Part::File { .. })));
    }
}
