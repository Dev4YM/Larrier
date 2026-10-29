//! Larrier's built-in Chat agent: streaming chat over provider HTTP APIs.
//!
//! * OpenAI-compatible `/chat/completions` (OpenAI, OpenRouter, Ollama, custom)
//! * Anthropic `/v1/messages`
//! * Gemini `models/<m>:streamGenerateContent?alt=sse`
//!
//! The whole LSF history is replayed every turn, so any session — from any
//! agent — can be continued as a plain chat.

use base64::Engine;
use futures::StreamExt;
use larrier_core::{lsf::MediaSource, Message, Part, ProviderKind, Role, Usage};
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedReceiver;

use super::{s, u};
use crate::events::{AgentEvent, EventSink, TurnCommand};
use crate::request::{ChatTarget, TurnRequest};

const SYSTEM_PROMPT: &str = "You are a helpful assistant running inside Larrier, a workspace for coding agents and AI chats. \
Answer clearly; use Markdown for structure and fenced code blocks for code.";

pub async fn run(req: TurnRequest, sink: EventSink, mut control: UnboundedReceiver<TurnCommand>) {
    let Some(target) = req.chat.clone() else {
        sink.error("No chat provider selected. Add an API key in Settings → Models.");
        sink.done(false, Some("error".into()));
        return;
    };
    sink.send(AgentEvent::Model { model: target.model.clone() });
    let turns = history_turns(&req.history);
    let fut = stream_chat(&target, &turns, &sink);
    tokio::pin!(fut);
    let result = loop {
        tokio::select! {
            r = &mut fut => break Some(r),
            cmd = control.recv() => match cmd {
                Some(TurnCommand::Cancel) | None => break None,
                Some(_) => {}
            },
        }
    };
    match result {
        None => sink.done(false, Some("cancelled".into())),
        Some(Ok(usage)) => {
            if let Some(u) = usage {
                sink.send(AgentEvent::Usage { usage: u });
            }
            sink.done(true, Some("end_turn".into()));
        }
        Some(Err(e)) => {
            sink.error(e);
            sink.done(false, Some("error".into()));
        }
    }
}

/// One provider-neutral chat turn.
#[derive(Clone, Debug, PartialEq)]
pub struct Turn {
    pub role: Role,
    pub text: String,
    /// (mime, base64 data)
    pub images: Vec<(String, String)>,
}

/// LSF messages → alternating user/assistant turns. Tool activity is
/// flattened into text; reasoning and notices are dropped.
pub fn history_turns(history: &[Message]) -> Vec<Turn> {
    let mut out: Vec<Turn> = Vec::new();
    for m in history {
        if m.role == Role::System {
            continue;
        }
        let mut text = String::new();
        let mut images = Vec::new();
        for p in &m.parts {
            match p {
                Part::Text { text: t } => {
                    if !text.is_empty() {
                        text.push_str("\n\n");
                    }
                    text.push_str(t);
                }
                Part::Tool(t) => {
                    text.push_str(&format!("\n[tool] {}", t.title.clone().unwrap_or_else(|| t.name.clone())));
                    if let Some(o) = t.output.as_deref().filter(|o| !o.trim().is_empty()) {
                        let o = larrier_core::lsf::truncate_middle(o.trim(), 800);
                        text.push_str(&format!("\n→ {o}"));
                    }
                }
                Part::File { path, text: Some(body), .. } => text.push_str(&format!("\n\n<file path=\"{path}\">\n{body}\n</file>")),
                Part::File { path, .. } => {
                    let body = std::fs::read_to_string(path).ok().filter(|b| b.len() < 200_000);
                    match body {
                        Some(b) => text.push_str(&format!("\n\n<file path=\"{path}\">\n{b}\n</file>")),
                        None => text.push_str(&format!("\n[attached file: {path}]")),
                    }
                }
                Part::Image { mime, source } if m.role == Role::User => {
                    let data = match source {
                        MediaSource::Base64 { data } => Some(data.clone()),
                        MediaSource::Path { path } => std::fs::read(path).ok().map(|b| base64::engine::general_purpose::STANDARD.encode(b)),
                        MediaSource::Url { .. } => None,
                    };
                    if let Some(d) = data {
                        let mt = mime.clone().unwrap_or_else(|| match source {
                            MediaSource::Path { path } if path.to_lowercase().ends_with(".jpg") || path.to_lowercase().ends_with(".jpeg") => "image/jpeg".into(),
                            MediaSource::Path { path } if path.to_lowercase().ends_with(".webp") => "image/webp".into(),
                            MediaSource::Path { path } if path.to_lowercase().ends_with(".gif") => "image/gif".into(),
                            _ => "image/png".into(),
                        });
                        images.push((mt, d));
                    }
                }
                Part::Error { message } if m.role == Role::Assistant => text.push_str(&format!("\n[error] {message}")),
                _ => {}
            }
        }
        let text = text.trim().to_string();
        if text.is_empty() && images.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.role == m.role => {
                last.text.push_str("\n\n");
                last.text.push_str(&text);
                last.images.extend(images);
            }
            _ => out.push(Turn { role: m.role, text, images }),
        }
    }
    // Providers want the conversation to start with the user.
    while out.first().map(|t| t.role != Role::User).unwrap_or(false) {
        out.remove(0);
    }
    out
}

async fn stream_chat(target: &ChatTarget, turns: &[Turn], sink: &EventSink) -> Result<Option<Usage>, String> {
    let client = reqwest::Client::builder().build().map_err(|e| e.to_string())?;
    let base = target.base_url.trim_end_matches('/');
    let key = target.api_key.clone().unwrap_or_default();
    let (rb, mut state) = match target.kind {
        ProviderKind::Anthropic => {
            let url = if base.ends_with("/v1") { format!("{base}/messages") } else { format!("{base}/v1/messages") };
            let body = anthropic_body(&target.model, turns);
            (
                client.post(url).header("x-api-key", key).header("anthropic-version", "2023-06-01").json(&body),
                Decoder::Anthropic(Usage::default()),
            )
        }
        ProviderKind::Gemini => {
            let url = format!("{base}/models/{}:streamGenerateContent?alt=sse", target.model);
            (client.post(url).header("x-goog-api-key", key).json(&gemini_body(turns)), Decoder::Gemini(None))
        }
        ProviderKind::OpenAi | ProviderKind::OpenRouter | ProviderKind::Ollama | ProviderKind::OpenAiCompatible => {
            let url = format!("{base}/chat/completions");
            let mut rb = client.post(url).json(&openai_body(&target.model, turns, target.kind != ProviderKind::Ollama));
            if !key.is_empty() {
                rb = rb.bearer_auth(key);
            }
            if target.kind == ProviderKind::OpenRouter {
                rb = rb.header("HTTP-Referer", "https://github.com/Dev4YM/Larrier").header("X-Title", "Larrier");
            }
            (rb, Decoder::OpenAi(None))
        }
    };
    let resp = rb.send().await.map_err(|e| format!("{}: {e}", target.provider_name))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        let msg = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().or_else(|| v["message"].as_str()).map(|s| s.to_string()))
            .unwrap_or_else(|| body.chars().take(400).collect());
        return Err(format!("{} returned {status}: {msg}", target.provider_name));
    }
    let mut sse = SseParser::default();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        for ev in sse.feed(&String::from_utf8_lossy(&chunk)) {
            if ev.data == "[DONE]" {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(&ev.data) else { continue };
            state.apply(ev.event.as_deref(), &v, sink)?;
        }
    }
    for ev in sse.finish() {
        if let Ok(v) = serde_json::from_str::<Value>(&ev.data) {
            state.apply(ev.event.as_deref(), &v, sink)?;
        }
    }
    Ok(state.usage())
}

pub fn openai_body(model: &str, turns: &[Turn], include_usage: bool) -> Value {
    let mut messages = vec![json!({ "role": "system", "content": SYSTEM_PROMPT })];
    for t in turns {
        let role = if t.role == Role::User { "user" } else { "assistant" };
        if t.images.is_empty() {
            messages.push(json!({ "role": role, "content": t.text }));
        } else {
            let mut parts = vec![json!({ "type": "text", "text": t.text })];
            for (mt, d) in &t.images {
                parts.push(json!({ "type": "image_url", "image_url": { "url": format!("data:{mt};base64,{d}") } }));
            }
            messages.push(json!({ "role": role, "content": parts }));
        }
    }
    let mut body = json!({ "model": model, "messages": messages, "stream": true });
    if include_usage {
        body["stream_options"] = json!({ "include_usage": true });
    }
    body
}

pub fn anthropic_body(model: &str, turns: &[Turn]) -> Value {
    let messages: Vec<Value> = turns
        .iter()
        .map(|t| {
            let mut content: Vec<Value> = t
                .images
                .iter()
                .map(|(mt, d)| json!({ "type": "image", "source": { "type": "base64", "media_type": mt, "data": d } }))
                .collect();
            content.push(json!({ "type": "text", "text": if t.text.is_empty() { "(image)" } else { &t.text } }));
            json!({ "role": if t.role == Role::User { "user" } else { "assistant" }, "content": content })
        })
        .collect();
    json!({ "model": model, "max_tokens": 16000, "stream": true, "system": SYSTEM_PROMPT, "messages": messages })
}

pub fn gemini_body(turns: &[Turn]) -> Value {
    let contents: Vec<Value> = turns
        .iter()
        .map(|t| {
            let mut parts: Vec<Value> = t.images.iter().map(|(mt, d)| json!({ "inlineData": { "mimeType": mt, "data": d } })).collect();
            parts.push(json!({ "text": t.text }));
            json!({ "role": if t.role == Role::User { "user" } else { "model" }, "parts": parts })
        })
        .collect();
    json!({ "systemInstruction": { "parts": [{ "text": SYSTEM_PROMPT }] }, "contents": contents })
}

/// Per-provider streaming state.
pub enum Decoder {
    OpenAi(Option<Usage>),
    Anthropic(Usage),
    Gemini(Option<Usage>),
}

impl Decoder {
    pub fn apply(&mut self, event: Option<&str>, v: &Value, sink: &EventSink) -> Result<(), String> {
        if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
            return Err(s(err, "message").map(|m| m.to_string()).unwrap_or_else(|| err.to_string()));
        }
        match self {
            Decoder::OpenAi(usage) => {
                if let Some(ch) = v["choices"].as_array().and_then(|c| c.first()) {
                    let d = &ch["delta"];
                    for key in ["reasoning_content", "reasoning"] {
                        if let Some(r) = d[key].as_str().filter(|r| !r.is_empty()) {
                            sink.send(AgentEvent::ReasoningDelta { text: r.to_string() });
                        }
                    }
                    if let Some(t) = d["content"].as_str().filter(|t| !t.is_empty()) {
                        sink.text(t);
                    }
                }
                let us = &v["usage"];
                if us.is_object() {
                    let cached = u(&us["prompt_tokens_details"], "cached_tokens");
                    *usage = Some(Usage {
                        input_tokens: u(us, "prompt_tokens").saturating_sub(cached),
                        output_tokens: u(us, "completion_tokens"),
                        cache_read_tokens: cached,
                        cache_write_tokens: 0,
                        reasoning_tokens: u(&us["completion_tokens_details"], "reasoning_tokens"),
                        cost_usd: us["cost"].as_f64(),
                        context_tokens: Some(u(us, "prompt_tokens")),
                        ..Default::default()
                    });
                }
            }
            Decoder::Anthropic(usage) => match event.or_else(|| s(v, "type")).unwrap_or("") {
                "message_start" => {
                    let us = &v["message"]["usage"];
                    usage.input_tokens = u(us, "input_tokens");
                    usage.cache_read_tokens = u(us, "cache_read_input_tokens");
                    usage.cache_write_tokens = u(us, "cache_creation_input_tokens");
                    usage.context_tokens = Some(usage.input_tokens + usage.cache_read_tokens + usage.cache_write_tokens);
                }
                "content_block_delta" => {
                    let d = &v["delta"];
                    match s(d, "type").unwrap_or("") {
                        "text_delta" => sink.text(s(d, "text").unwrap_or("")),
                        "thinking_delta" => sink.send(AgentEvent::ReasoningDelta { text: s(d, "thinking").unwrap_or("").to_string() }),
                        _ => {}
                    }
                }
                "message_delta" => {
                    usage.output_tokens = u(&v["usage"], "output_tokens");
                    if s(&v["delta"], "stop_reason") == Some("max_tokens") {
                        sink.notice("Stopped: max tokens");
                    }
                }
                _ => {}
            },
            Decoder::Gemini(usage) => {
                if let Some(parts) = v["candidates"].as_array().and_then(|c| c.first()).and_then(|c| c["content"]["parts"].as_array()) {
                    for p in parts {
                        if let Some(t) = p["text"].as_str().filter(|t| !t.is_empty()) {
                            if p["thought"].as_bool().unwrap_or(false) {
                                sink.send(AgentEvent::ReasoningDelta { text: t.to_string() });
                            } else {
                                sink.text(t);
                            }
                        }
                    }
                }
                let um = &v["usageMetadata"];
                if um.is_object() {
                    let cached = u(um, "cachedContentTokenCount");
                    *usage = Some(Usage {
                        input_tokens: u(um, "promptTokenCount").saturating_sub(cached),
                        output_tokens: u(um, "candidatesTokenCount"),
                        cache_read_tokens: cached,
                        cache_write_tokens: 0,
                        reasoning_tokens: u(um, "thoughtsTokenCount"),
                        cost_usd: None,
                        context_tokens: Some(u(um, "promptTokenCount")),
                        ..Default::default()
                    });
                }
            }
        }
        Ok(())
    }

    pub fn usage(&self) -> Option<Usage> {
        match self {
            Decoder::OpenAi(u) | Decoder::Gemini(u) => u.clone(),
            Decoder::Anthropic(u) => Some(u.clone()).filter(|u| !u.is_empty()),
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

/// Minimal Server-Sent Events decoder (handles chunk boundaries anywhere).
#[derive(Default)]
pub struct SseParser {
    buf: String,
    event: Option<String>,
    data: Vec<String>,
}

impl SseParser {
    pub fn feed(&mut self, chunk: &str) -> Vec<SseEvent> {
        self.buf.push_str(chunk);
        let mut out = Vec::new();
        while let Some(i) = self.buf.find('\n') {
            let line: String = self.buf.drain(..=i).collect();
            let line = line.trim_end_matches(['\n', '\r']);
            self.line(line, &mut out);
        }
        out
    }

    pub fn finish(&mut self) -> Vec<SseEvent> {
        let mut out = Vec::new();
        let rest = std::mem::take(&mut self.buf);
        if !rest.trim().is_empty() {
            self.line(rest.trim_end_matches(['\n', '\r']), &mut out);
        }
        self.line("", &mut out);
        out
    }

    fn line(&mut self, line: &str, out: &mut Vec<SseEvent>) {
        if line.is_empty() {
            if !self.data.is_empty() {
                out.push(SseEvent { event: self.event.take(), data: self.data.join("\n") });
                self.data.clear();
            }
            self.event = None;
            return;
        }
        if line.starts_with(':') {
            return;
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        match field {
            "event" => self.event = Some(value.to_string()),
            "data" => self.data.push(value.to_string()),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use larrier_core::{AgentKind, ToolCall};
    use tokio::sync::mpsc;

    fn collect(dec: &mut Decoder, events: Vec<SseEvent>) -> Vec<AgentEvent> {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sink = EventSink::new(tx);
        for e in events {
            if e.data == "[DONE]" {
                continue;
            }
            let v: Value = serde_json::from_str(&e.data).unwrap();
            dec.apply(e.event.as_deref(), &v, &sink).unwrap();
        }
        let mut out = Vec::new();
        while let Ok(e) = rx.try_recv() {
            out.push(e);
        }
        out
    }

    #[test]
    fn sse_handles_split_chunks() {
        let mut p = SseParser::default();
        let mut ev = p.feed("event: message_start\ndata: {\"a\":");
        assert!(ev.is_empty());
        ev.extend(p.feed("1}\n\n: keepalive\n\ndata: [DONE]\r\n\r\n"));
        assert_eq!(ev, vec![
            SseEvent { event: Some("message_start".into()), data: "{\"a\":1}".into() },
            SseEvent { event: None, data: "[DONE]".into() },
        ]);
    }

    #[test]
    fn openai_stream() {
        let mut p = SseParser::default();
        let evs = p.feed("data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"hm\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\ndata: {\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":3,\"prompt_tokens_details\":{\"cached_tokens\":2}}}\n\ndata: [DONE]\n\n");
        let mut d = Decoder::OpenAi(None);
        let out = collect(&mut d, evs);
        assert!(matches!(&out[0], AgentEvent::ReasoningDelta { text } if text == "hm"));
        assert!(matches!(&out[1], AgentEvent::TextDelta { text } if text == "Hi"));
        let u = d.usage().unwrap();
        assert_eq!((u.input_tokens, u.cache_read_tokens, u.output_tokens), (10, 2, 3));
    }

    #[test]
    fn anthropic_stream() {
        let mut p = SseParser::default();
        let evs = p.feed(concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":20,\"cache_read_input_tokens\":5}}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"x\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":7}}\n\n",
        ));
        let mut d = Decoder::Anthropic(Usage::default());
        let out = collect(&mut d, evs);
        assert!(matches!(&out[0], AgentEvent::ReasoningDelta { .. }));
        assert!(matches!(&out[1], AgentEvent::TextDelta { text } if text == "Hello"));
        let u = d.usage().unwrap();
        assert_eq!((u.input_tokens, u.cache_read_tokens, u.output_tokens), (20, 5, 7));
    }

    #[test]
    fn gemini_stream() {
        let mut p = SseParser::default();
        let evs = p.feed("data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"plan\",\"thought\":true},{\"text\":\"Answer\"}]}}],\"usageMetadata\":{\"promptTokenCount\":9,\"candidatesTokenCount\":4,\"thoughtsTokenCount\":2}}\n\n");
        let mut d = Decoder::Gemini(None);
        let out = collect(&mut d, evs);
        assert!(matches!(&out[0], AgentEvent::ReasoningDelta { .. }));
        assert!(matches!(&out[1], AgentEvent::TextDelta { text } if text == "Answer"));
        assert_eq!(d.usage().unwrap().reasoning_tokens, 2);
    }

    #[test]
    fn history_flattens_and_alternates() {
        let mut a = Message::assistant(AgentKind::ClaudeCode, None);
        a.push_text("I'll check.");
        let mut t = ToolCall::new("1", "Bash", json!({"command": "ls"}));
        t.output = Some("a.rs".into());
        a.parts.push(Part::Tool(t));
        let sys = Message::new(Role::System, vec![Part::Notice { text: "ported".into() }]);
        let turns = history_turns(&[a.clone(), Message::user_text("hi"), a, sys, Message::user_text("next"), Message::user_text("more")]);
        // Leading assistant dropped, consecutive users merged, notices skipped.
        assert_eq!(turns.len(), 3);
        assert_eq!(turns[0].role, Role::User);
        assert!(turns[1].text.contains("[tool] Ran ls") && turns[1].text.contains("a.rs"));
        assert_eq!(turns[2].text, "next\n\nmore");
        let b = anthropic_body("claude-opus-5-5", &turns);
        assert_eq!(b["messages"].as_array().unwrap().len(), 3);
        let o = openai_body("gpt", &turns, true);
        assert_eq!(o["messages"][0]["role"], "system");
    }
}
