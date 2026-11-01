//! Repairs of sessions Larrier recorded itself, from the agent's own
//! transcript of the same conversation (linked native sessions are never
//! re-imported, so an importer fix doesn't reach them).

use std::path::Path;

use larrier_core::lsf::is_narration_block;
use larrier_core::{Message, Part, Store};
use serde_json::Value;

/// Shorter narration only counts when it is a whole thought: as the start
/// or end of a longer one it could be a coincidence.
const MIN_JOINED: usize = 12;

/// Narration (words for the reader that Claude models send as thinking
/// blocks) that Larrier recorded as thinking before it told them apart: a
/// thought that is, starts or ends with one of the transcript's narration
/// blocks gives that part back as text. Returns whether the session changed.
pub fn narration(store: &Store, session_id: &str, transcript: &Path) -> bool {
    let texts = narration_texts(transcript);
    if texts.is_empty() {
        return false;
    }
    let Ok(mut s) = store.load(session_id) else { return false };
    let mut changed = false;
    for m in &mut s.messages {
        changed |= fix_message(m, &texts);
    }
    changed && store.save(&s).is_ok()
}

/// Every narration block in a JSONL transcript and, for Claude Code, its
/// subagents' transcripts beside it (`<session>/subagents/*.jsonl`).
/// Longest first, so a longer block wins over one it contains.
fn narration_texts(path: &Path) -> Vec<String> {
    let mut files = vec![path.to_path_buf()];
    if let Some(stem) = path.file_stem() {
        if let Ok(dir) = std::fs::read_dir(path.with_file_name(stem).join("subagents")) {
            files.extend(dir.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "jsonl")));
        }
    }
    let mut out = Vec::new();
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        // Only lines with a signature can hold narration.
        for line in text.lines().filter(|l| l.contains("ignature\"")) {
            if let Ok(v) = serde_json::from_str::<Value>(line) {
                collect(&v, &mut out);
            }
        }
    }
    out.sort();
    out.dedup();
    out.sort_by_key(|t| std::cmp::Reverse(t.len()));
    out
}

fn collect(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(o) => {
            if matches!(o.get("type").and_then(|t| t.as_str()), Some("thinking" | "reasoning")) && is_narration_block(v) {
                if let Some(t) = o.get("thinking").or_else(|| o.get("text")).and_then(|t| t.as_str()).map(str::trim).filter(|t| !t.is_empty()) {
                    out.push(t.to_string());
                }
            }
            o.values().for_each(|x| collect(x, out));
        }
        Value::Array(a) => a.iter().for_each(|x| collect(x, out)),
        _ => {}
    }
}

/// Fix a message's thoughts, and those of the subagents it ran.
fn fix_message(m: &mut Message, texts: &[String]) -> bool {
    let mut changed = false;
    let mut parts = Vec::with_capacity(m.parts.len());
    for p in std::mem::take(&mut m.parts) {
        match p {
            Part::Reasoning { text, duration_ms } => {
                let pieces = split(&text, texts);
                if !pieces.iter().any(|(narration, _)| *narration) {
                    parts.push(Part::Reasoning { text, duration_ms });
                    continue;
                }
                changed = true;
                for (narration, t) in pieces {
                    parts.push(if narration { Part::Text { text: t } } else { Part::Reasoning { text: t, duration_ms } });
                }
            }
            Part::Tool(mut t) => {
                for sm in &mut t.subagent {
                    changed |= fix_message(sm, texts);
                }
                parts.push(Part::Tool(t));
            }
            other => parts.push(other),
        }
    }
    m.parts = parts;
    changed
}

/// A thought as (is narration, text) pieces, in order: the narration it is,
/// starts with or ends with (a narration block streamed right before or
/// after thinking joined it), around what is left of the thought.
fn split(text: &str, texts: &[String]) -> Vec<(bool, String)> {
    let fits = |rest: &str, n: &str, at_end: bool| rest == n || (n.len() >= MIN_JOINED && if at_end { rest.ends_with(n) } else { rest.starts_with(n) });
    let mut rest = text.trim();
    let (mut head, mut tail) = (Vec::new(), Vec::new());
    while !rest.is_empty() {
        if let Some(n) = texts.iter().find(|n| fits(rest, n, true)) {
            tail.push((true, n.clone()));
            rest = rest[..rest.len() - n.len()].trim_end();
        } else if let Some(n) = texts.iter().find(|n| fits(rest, n, false)) {
            head.push((true, n.clone()));
            rest = rest[n.len()..].trim_start();
        } else {
            break;
        }
    }
    tail.reverse();
    if !rest.is_empty() {
        head.push((false, rest.to_string()));
    }
    head.extend(tail);
    head
}

#[cfg(test)]
mod tests {
    use super::*;
    use larrier_core::{AgentKind, Session, ToolCall};

    fn signature(kind: &str) -> String {
        use base64::Engine;
        let mut b = vec![0x08, 0x04, 0x12, 0x60, 0x0a, 0x11, 0x08, 0x13, 0x18, 0x02, 0x38, 0x01, 0x42, kind.len() as u8];
        b.extend_from_slice(kind.as_bytes());
        b.extend((0..90u8).map(|i| i.wrapping_mul(37)));
        base64::engine::general_purpose::STANDARD.encode(b)
    }

    fn record(thinking: &str, kind: &str) -> String {
        serde_json::json!({"type": "assistant", "message": {"role": "assistant", "content": [{"type": "thinking", "thinking": thinking, "signature": signature(kind)}]}}).to_string()
    }

    fn reasoning(t: &str) -> Part {
        Part::Reasoning { text: t.into(), duration_ms: Some(900) }
    }

    /// Thoughts Larrier recorded live before 0.1.33: narration alone, joined
    /// after or before real thinking, and in a subagent; real thinking and a
    /// thought that only mentions a narration line in passing stay thoughts.
    #[test]
    fn repairs_narration_recorded_as_thinking() {
        let dir = tempfile::tempdir().unwrap();
        let transcript = dir.path().join("sess.jsonl");
        let lines = [
            record("A retry can race the first delivery.", "thinking"),
            record("Agreed, the rings felt cheap.", "narration"),
            record("Now the tests run.", "narration"),
            record("Done.", "narration"),
        ];
        std::fs::write(&transcript, lines.join("\n")).unwrap();
        std::fs::create_dir_all(dir.path().join("sess").join("subagents")).unwrap();
        std::fs::write(dir.path().join("sess").join("subagents").join("agent-1.jsonl"), record("Looking for writers now.", "narration")).unwrap();

        let store = Store::open(dir.path().join("home")).unwrap();
        let mut s = Session::new(AgentKind::ClaudeCode, None);
        s.push(Message::user_text("fix it"));
        let mut a = Message::assistant(AgentKind::ClaudeCode, None);
        let mut task = ToolCall::new("t1", "Task", serde_json::json!({}));
        let mut sub = Message::assistant(AgentKind::ClaudeCode, None);
        sub.parts = vec![reasoning("Looking for writers now.")];
        task.subagent = vec![sub];
        a.parts = vec![
            reasoning("A retry can race the first delivery.Agreed, the rings felt cheap."),
            Part::Tool(task),
            reasoning("Now the tests run.Check the lock."),
            reasoning("Done."),
            reasoning("Thinking it through. Done."),
        ];
        s.push(a);
        store.save(&s).unwrap();

        assert!(narration(&store, &s.id, &transcript));
        let got = store.load(&s.id).unwrap();
        let parts = &got.messages[1].parts;
        let shape: Vec<(bool, &str)> = parts
            .iter()
            .filter_map(|p| match p {
                Part::Reasoning { text, .. } => Some((false, text.as_str())),
                Part::Text { text } => Some((true, text.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(
            shape,
            vec![
                (false, "A retry can race the first delivery."),
                (true, "Agreed, the rings felt cheap."),
                (true, "Now the tests run."),
                (false, "Check the lock."),
                (true, "Done."),
                (false, "Thinking it through. Done."),
            ]
        );
        let Part::Tool(t) = &parts[2] else { panic!("{parts:?}") };
        assert_eq!(t.subagent[0].parts, vec![Part::Text { text: "Looking for writers now.".into() }]);
        // Once repaired there is nothing left to do.
        assert!(!narration(&store, &s.id, &transcript));
    }
}
