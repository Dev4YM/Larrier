//! What a running agent is waiting for, answered in place: approval to run
//! a tool, answers to its questions, or a go-ahead for its plan.

use dioxus::prelude::*;
use larrier_agents::{PermissionRequest, Reply, RequestKind};

use super::ask::{Opt, Question, QuestionList};
use crate::icons::{Icon, I};
use crate::state::use_app;

#[component]
pub fn RequestCard(session_id: String, request: PermissionRequest, more: usize) -> Element {
    let app = use_app();
    let mut note = use_signal(String::new);
    let agent = app.peek_session(&session_id, |s| s.agent.display_name()).unwrap_or_else(|| "The agent".into());
    let rid = request.id.clone();
    let answer = {
        let sid = session_id.clone();
        let rid = rid.clone();
        move |reply: Reply| {
            note.set(String::new());
            app.respond_permission(&sid, &rid, reply);
        }
    };
    let waiting = if more > 0 { format!("{} more waiting", more) } else { String::new() };
    match request.kind {
        RequestKind::Question => {
            let qs: Vec<Question> = request
                .questions
                .iter()
                .map(|q| Question {
                    id: q.id.clone(),
                    header: q.header.clone(),
                    text: q.question.clone(),
                    multi: q.multi,
                    options: q.options.iter().map(|o| Opt { label: o.label.clone(), description: o.description.clone() }).collect(),
                    answers: Vec::new(),
                })
                .collect();
            let n = qs.len();
            let mut submit = answer.clone();
            let mut dismiss = answer.clone();
            rsx! {
                div { key: "{rid}", class: "req-card ask-card open",
                    div { class: "ask-head",
                        span { class: "ic", Icon { i: I::Message, size: 13 } }
                        span { class: "grow", "{request.title}" }
                        if n > 1 { span { class: "ask-count", "{n} questions" } }
                        if more > 0 { span { class: "ask-count", "{waiting}" } }
                    }
                    QuestionList {
                        questions: qs,
                        editable: true,
                        hint: format!("{agent} is waiting for your answer"),
                        submit: "Answer".to_string(),
                        disabled: false,
                        onsubmit: move |picks: Vec<Vec<String>>| submit(Reply { answers: picks, ..Default::default() }),
                        ondismiss: move |_| dismiss(Reply::default()),
                    }
                }
            }
        }
        RequestKind::Plan => {
            let html = request.plan.as_deref().map(crate::markdown::to_html).unwrap_or_default();
            let opts = request.options.clone();
            rsx! {
                div { key: "{rid}", class: "req-card plan",
                    div { class: "req-head",
                        span { class: "ic", Icon { i: I::List, size: 13 } }
                        span { class: "grow", "{request.title}" }
                        if more > 0 { span { class: "faint", "{waiting}" } }
                    }
                    if !html.is_empty() {
                        div { class: "req-plan md", dangerous_inner_html: "{html}" }
                    }
                    div { class: "req-actions",
                        input { class: "req-note",
                            placeholder: "What should change? (Keep planning)",
                            value: "{note}",
                            oninput: move |e| note.set(e.value()),
                            onkeydown: {
                                let mut answer = answer.clone();
                                move |e: KeyboardEvent| {
                                    if e.key() == Key::Enter && !note.read().trim().is_empty() {
                                        e.prevent_default();
                                        let m = note.read().clone();
                                        answer(Reply { option_id: Some("keep".into()), message: Some(m), ..Default::default() });
                                    }
                                }
                            },
                        }
                        for o in opts {
                            {
                                let mut answer = answer.clone();
                                let id = o.id.clone();
                                let primary = o.kind == "allow_always";
                                rsx! {
                                    button { key: "{o.id}", class: if primary { "primary-btn" } else { "ghost-btn" },
                                        onclick: move |_| {
                                            let m = note.read().trim().to_string();
                                            answer(Reply { option_id: Some(id.clone()), message: (!m.is_empty()).then_some(m), ..Default::default() });
                                        },
                                        "{o.label}"
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        RequestKind::Tool => {
            let opts = request.options.clone();
            let deny = opts.iter().find(|o| o.kind.starts_with("reject")).map(|o| o.id.clone());
            // What it is asking for, by the drivers' titles ("Write a.txt").
            let lead = if ["Write ", "Edit ", "Delete ", "Change files"].iter().any(|p| request.title.starts_with(p)) {
                "wants to change files"
            } else if request.title.starts_with(agent.as_str()) || request.title.contains(" asks you ") {
                "needs your OK"
            } else {
                "wants to run"
            };
            rsx! {
                div { key: "{rid}", class: "req-card",
                    div { class: "req-head",
                        span { class: "ic", Icon { i: I::Lock, size: 13 } }
                        span { class: "grow", "{agent} {lead}" }
                        if more > 0 { span { class: "faint", "{waiting}" } }
                    }
                    div { class: "req-title", "{request.title}" }
                    if let Some(d) = request.detail.clone() {
                        div { class: "req-detail", "{d}" }
                    }
                    div { class: "req-actions",
                        if let Some(deny) = deny.clone() {
                            input { class: "req-note",
                                placeholder: "Or tell {agent} what to do instead",
                                value: "{note}",
                                oninput: move |e| note.set(e.value()),
                                onkeydown: {
                                    let mut answer = answer.clone();
                                    move |e: KeyboardEvent| {
                                        if e.key() == Key::Enter && !note.read().trim().is_empty() {
                                            e.prevent_default();
                                            let m = note.read().clone();
                                            answer(Reply { option_id: Some(deny.clone()), message: Some(m), ..Default::default() });
                                        }
                                    }
                                },
                            }
                        }
                        for o in opts {
                            {
                                let mut answer = answer.clone();
                                let id = o.id.clone();
                                let cls = match o.kind.as_str() {
                                    "allow_once" => "primary-btn",
                                    k if k.starts_with("reject") => "ghost-btn danger",
                                    _ => "ghost-btn",
                                };
                                let is_deny = o.kind.starts_with("reject");
                                rsx! {
                                    button { key: "{o.id}", class: "{cls}",
                                        onclick: move |_| {
                                            let m = note.read().trim().to_string();
                                            let message = (is_deny && !m.is_empty()).then_some(m);
                                            answer(Reply { option_id: Some(id.clone()), message, ..Default::default() });
                                        },
                                        "{o.label}"
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
