//! Questions an agent asks the user (Claude Code `AskUserQuestion`, Codex
//! `request_user_input`, Cursor `AskQuestion`, Hermes `clarify`, Droid
//! `AskUser`, …), read into one shape and shown as one card.
//!
//! An unanswered question can be answered in the card: the choices go back
//! to the agent as the next message, which every agent accepts (and which
//! waits in the queue while the agent is still working).

use dioxus::prelude::*;
use larrier_core::{Role, ToolCall};
use serde_json::Value;

use crate::icons::{Icon, I};
use crate::state::use_app;

/// Tool names agents use to ask the user something.
pub const QUESTION_TOOLS: &[&str] = &["AskUserQuestion", "AskQuestion", "request_user_input", "clarify", "AskUser", "ask_user", "ask_followup_question", "question"];

#[derive(Clone, Debug, PartialEq)]
pub struct Opt {
    pub label: String,
    pub description: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Question {
    pub id: Option<String>,
    pub header: Option<String>,
    pub text: String,
    pub multi: bool,
    pub options: Vec<Opt>,
    /// What the user answered, when the transcript records it.
    pub answers: Vec<String>,
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(|x| x.as_str()).map(str::trim).filter(|x| !x.is_empty()).map(String::from)
}

fn options(v: &Value) -> Vec<Opt> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|o| match o {
            Value::String(l) => Some(Opt { label: l.clone(), description: None }),
            _ => Some(Opt { label: s(o, "label").or_else(|| s(o, "text")).or_else(|| s(o, "value"))?, description: s(o, "description") }),
        })
        .collect()
}

/// The questions of a question tool call, with any recorded answers.
pub fn parse(tool: &ToolCall) -> Vec<Question> {
    let input = &tool.input;
    let output = tool.output.clone().unwrap_or_default();
    let mut qs: Vec<Question> = Vec::new();
    if let Some(list) = input.get("questions").and_then(|q| q.as_array()) {
        // Claude, Codex, Cursor: a list of questions with options.
        for q in list {
            let text = s(q, "question").or_else(|| s(q, "prompt")).or_else(|| s(q, "text")).unwrap_or_default();
            if text.is_empty() {
                continue;
            }
            qs.push(Question {
                id: s(q, "id"),
                header: s(q, "header"),
                text,
                multi: ["multiSelect", "multi_select", "allowMultiple", "allow_multiple"].iter().any(|k| q.get(*k).and_then(|b| b.as_bool()).unwrap_or(false)),
                options: options(&q["options"]),
                answers: Vec::new(),
            });
        }
    } else if let Some(text) = input.get("questionnaire").and_then(|q| q.as_str()) {
        // Droid: "1. [question] …\n[topic] …\n[option] …".
        for block in text.split("\n\n") {
            let mut q = Question { id: None, header: None, text: String::new(), multi: false, options: Vec::new(), answers: Vec::new() };
            for line in block.lines() {
                let l = line.trim().trim_start_matches(|c: char| c.is_ascii_digit()).trim_start_matches('.').trim();
                if let Some(t) = l.strip_prefix("[question]") {
                    q.text = t.trim().to_string();
                } else if let Some(t) = l.strip_prefix("[topic]") {
                    q.header = Some(t.trim().to_string());
                } else if let Some(t) = l.strip_prefix("[option]") {
                    q.options.push(Opt { label: t.trim().to_string(), description: None });
                }
            }
            if !q.text.is_empty() {
                qs.push(q);
            }
        }
    } else if let Some(text) = s(input, "question") {
        // Hermes `clarify` and single-question tools.
        let opts = ["choices", "options", "suggestions"].iter().map(|k| options(&input[*k])).find(|o| !o.is_empty()).unwrap_or_default();
        qs.push(Question { id: None, header: None, text, multi: false, options: opts, answers: Vec::new() });
    }
    read_answers(&mut qs, &output);
    qs
}

/// Fill in answers recorded in the tool's output, in each agent's format.
fn read_answers(qs: &mut [Question], output: &str) {
    let out = output.trim();
    if out.is_empty() {
        return;
    }
    if let Ok(v) = serde_json::from_str::<Value>(out) {
        // Codex: {"answers": {"<id>": {"answers": ["…"]}}}.
        if let Some(map) = v.get("answers").and_then(|a| a.as_object()) {
            for q in qs.iter_mut() {
                if let Some(a) = q.id.as_ref().and_then(|id| map.get(id)) {
                    q.answers = a["answers"].as_array().map(|l| l.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
                }
            }
        }
        // Hermes: {"user_response": "…"}.
        if let (Some(r), Some(q)) = (s(&v, "user_response"), qs.first_mut()) {
            q.answers = vec![r];
        }
        return;
    }
    for q in qs.iter_mut() {
        // Claude: `"<question>"="<answer>"` (several answers joined by ", ").
        let key = format!("\"{}\"=\"", q.text);
        if let Some(i) = out.find(&key) {
            let rest = &out[i + key.len()..];
            if let Some(end) = rest.find('"') {
                let a = &rest[..end];
                q.answers = if q.multi { a.split(", ").map(String::from).collect() } else { vec![a.to_string()] };
                continue;
            }
        }
        // Droid: "[question] <text>" followed by "[answer] <answer>".
        if let Some(i) = out.find(&q.text) {
            if let Some(j) = out[i..].find("[answer]") {
                let a = out[i + j + "[answer]".len()..].lines().next().unwrap_or("").trim();
                if !a.is_empty() {
                    q.answers = vec![a.to_string()];
                }
            }
        }
    }
}

/// The reply that answers `qs` with the user's picks.
pub fn reply(qs: &[Question], picks: &[Vec<String>]) -> String {
    if qs.len() == 1 {
        return picks.first().map(|p| p.join(", ")).unwrap_or_default();
    }
    let mut out = String::from("My answers:");
    for (q, p) in qs.iter().zip(picks) {
        out.push_str(&format!("\n- {}: {}", q.header.clone().unwrap_or_else(|| q.text.clone()), p.join(", ")));
    }
    out
}

/// A question card. Answered questions show what was chosen; unanswered
/// ones can be answered here.
#[component]
pub fn AskCard(session_id: String, tool: ToolCall) -> Element {
    let app = use_app();
    let qs = parse(&tool);
    let n = qs.len();
    let mut picks = use_signal(|| vec![Vec::<String>::new(); n]);
    let mut other = use_signal(|| vec![String::new(); n]);
    let mut sent = use_signal(|| false);
    if qs.is_empty() {
        return rsx! {};
    }
    let answered = qs.iter().any(|q| !q.answers.is_empty());
    // A later prompt answered it in the chat already.
    let replied_later = app
        .peek_session(&session_id, |s| {
            let at = s.messages.iter().position(|m| m.tool_calls().any(|t| t.id == tool.id))?;
            Some(s.messages[at + 1..].iter().any(|m| m.role == Role::User))
        })
        .flatten()
        .unwrap_or(true);
    let open = !answered && !replied_later && !sent();
    let title = tool.input.get("title").and_then(|t| t.as_str()).map(String::from);
    // What counts as each question's answer: picks, else the typed text.
    let chosen: Vec<Vec<String>> = (0..n)
        .map(|i| {
            let typed = other.read()[i].trim().to_string();
            let mut p = picks.read()[i].clone();
            if !typed.is_empty() {
                p.push(typed);
            }
            p
        })
        .collect();
    let ready = chosen.iter().all(|c| !c.is_empty());
    let qs_send = qs.clone();
    let chosen_send = chosen.clone();
    let sid = session_id.clone();
    rsx! {
        div { class: if open { "ask-card open" } else { "ask-card" },
            div { class: "ask-head",
                span { class: "ic", Icon { i: if open { I::Message } else { I::CircleCheck }, size: 13 } }
                span { class: "grow", if let Some(t) = &title { "{t}" } else if open { "The agent has a question" } else { "Answered" } }
                if n > 1 { span { class: "ask-count", "{n} questions" } }
            }
            for (qi, q) in qs.iter().cloned().enumerate() {
                div { key: "{qi}", class: "ask-q",
                    if let Some(h) = &q.header { span { class: "ask-hdr", "{h}" } }
                    div { class: "ask-text", "{q.text}" }
                    div { class: "ask-opts",
                        for (oi, o) in q.options.iter().cloned().enumerate() {
                            {
                                let on = if open { picks.read()[qi].contains(&o.label) } else { q.answers.iter().any(|a| a == &o.label) };
                                let dim = !open && !q.answers.is_empty() && !on;
                                let label = o.label.clone();
                                let multi = q.multi;
                                let mut cls = String::from("ask-opt");
                                if on { cls.push_str(" on"); }
                                if dim { cls.push_str(" dim"); }
                                if multi { cls.push_str(" multi"); }
                                rsx! {
                                    button { key: "{oi}", class: "{cls}", disabled: !open,
                                        onclick: move |_| {
                                            let mut p = picks.write();
                                            let cur = &mut p[qi];
                                            if multi {
                                                if let Some(i) = cur.iter().position(|x| x == &label) { cur.remove(i); } else { cur.push(label.clone()); }
                                            } else {
                                                *cur = vec![label.clone()];
                                                other.write()[qi].clear();
                                            }
                                        },
                                        span { class: "ask-box", if on { Icon { i: I::Check, size: 10, stroke: 3.0 } } }
                                        span { class: "ask-lbl",
                                            span { class: "l", "{o.label}" }
                                            if let Some(d) = &o.description { span { class: "d", "{d}" } }
                                        }
                                    }
                                }
                            }
                        }
                        // Free-text answers recorded without a matching option.
                        if !open {
                            for (ai, a) in q.answers.iter().filter(|a| !q.options.iter().any(|o| &o.label == *a)).cloned().enumerate() {
                                div { key: "x{ai}", class: "ask-opt on", span { class: "ask-box", Icon { i: I::Check, size: 10, stroke: 3.0 } } span { class: "ask-lbl", span { class: "l", "{a}" } } }
                            }
                        }
                    }
                    if open {
                        input { class: "ask-other",
                            placeholder: if q.options.is_empty() { "Your answer" } else { "Other…" },
                            value: "{other.read()[qi]}",
                            oninput: {
                                let multi = q.multi;
                                move |e: FormEvent| {
                                    let v = e.value();
                                    // A typed answer replaces a single choice.
                                    if !multi && !v.trim().is_empty() {
                                        picks.write()[qi].clear();
                                    }
                                    other.write()[qi] = v;
                                }
                            },
                        }
                    }
                }
            }
            if open {
                div { class: "ask-actions",
                    span { class: "grow hint", if app.is_active(&session_id) { "Sends when the agent finishes this turn" } else { "Sends your answers to the agent" } }
                    button { class: "primary-btn", disabled: !ready,
                        onclick: move |_| {
                            let text = reply(&qs_send, &chosen_send);
                            sent.set(true);
                            app.send(Some(sid.clone()), text, Vec::new());
                        },
                        "Send answers"
                    }
                }
            } else if sent() {
                div { class: "ask-actions", span { class: "grow hint", "Answers sent" } }
            } else if !answered {
                // The agent did not record the answer (Cursor): it was given
                // in the next message.
                div { class: "ask-actions", span { class: "grow hint", "Answered in the chat" } }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(name: &str, input: Value, output: &str) -> ToolCall {
        let mut t = ToolCall::new("t1", name, input);
        t.output = (!output.is_empty()).then(|| output.to_string());
        t
    }

    #[test]
    fn reads_every_agents_questions_and_answers() {
        let claude = call(
            "AskUserQuestion",
            json!({"questions": [{"question": "Which output?", "header": "Output", "multiSelect": false, "options": [{"label": "Files only", "description": "Safest"}, {"label": "CRM"}]}]}),
            r#"Your questions have been answered: "Which output?"="Files only". You can now continue."#,
        );
        let q = parse(&claude);
        assert_eq!((q[0].header.as_deref(), q[0].options.len(), q[0].answers.clone()), (Some("Output"), 2, vec!["Files only".to_string()]));

        let codex = call(
            "request_user_input",
            json!({"questions": [{"id": "role", "header": "AI Role", "question": "How central?", "options": [{"label": "Core", "description": "x"}]}]}),
            r#"{"answers":{"role":{"answers":["Core"]}}}"#,
        );
        assert_eq!(parse(&codex)[0].answers, ["Core"]);

        let cursor = call("AskQuestion", json!({"title": "Before we start", "questions": [{"id": "pay", "prompt": "How to pay?", "options": [{"id": "a", "label": "Bank"}], "allowMultiple": true}]}), "");
        let q = parse(&cursor);
        assert!(q[0].multi && q[0].answers.is_empty() && q[0].text == "How to pay?");

        let hermes = call("clarify", json!({"question": "Which policy?", "choices": ["Full", "Read-only"]}), r#"{"question": "Which policy?", "user_response": "Full"}"#);
        let q = parse(&hermes);
        assert_eq!((q[0].options.len(), q[0].answers.clone()), (2, vec!["Full".to_string()]));

        let droid = call(
            "AskUser",
            json!({"questionnaire": "1. [question] How wide?\n[topic] Scope\n[option] Backend\n[option] Everything\n\n2. [question] Which way?\n[option] Speed"}),
            "1. [question] How wide?\n[answer] Backend\n\n2. [question] Which way?\n[answer] Speed",
        );
        let q = parse(&droid);
        assert_eq!(q.len(), 2);
        assert_eq!((q[0].header.as_deref(), q[0].options.len(), q[0].answers.clone()), (Some("Scope"), 2, vec!["Backend".to_string()]));
        assert_eq!(q[1].answers, ["Speed"]);
    }

    #[test]
    fn writes_the_reply() {
        let q = |t: &str, h: Option<&str>| Question { id: None, header: h.map(String::from), text: t.into(), multi: false, options: Vec::new(), answers: Vec::new() };
        assert_eq!(reply(&[q("Which?", None)], &[vec!["A".into()]]), "A");
        assert_eq!(reply(&[q("Which?", Some("Scope")), q("How?", None)], &[vec!["A".into(), "B".into()], vec!["C".into()]]), "My answers:\n- Scope: A, B\n- How?: C");
    }
}
