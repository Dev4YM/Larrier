//! Rendering of an LSF conversation the way Cursor's Agents Window does it:
//! sticky prompt cards, quiet "Read / Searched / Thought" steps grouped into
//! "Explored …", command cards, edit cards with inline diff previews, plans
//! and live streaming state.

use dioxus::prelude::*;
use larrier_core::{
    lsf::{short_path, MediaSource},
    FileDiff, Message, Part, PlanStatus, Role, ToolCall, ToolKind, ToolStatus,
};

use crate::icons::{Icon, I};
use crate::markdown;
use crate::state::{use_app, RightTab};

#[component]
pub fn Transcript(id: String) -> Element {
    let app = use_app();
    let mut at_bottom = use_signal(|| true);
    let (messages, updated) = match app.open.read().get(&id) {
        Some(s) => (s.messages.clone(), s.updated_at),
        None => return rsx! {},
    };
    let feedback: std::collections::HashMap<String, String> = app
        .open
        .read()
        .get(&id)
        .map(|s| s.extra.iter().filter_map(|(k, v)| k.strip_prefix("feedback:").map(|m| (m.to_string(), v.as_str().unwrap_or("").to_string()))).collect())
        .unwrap_or_default();
    let running = app.is_running(&id);
    let permission = app.permissions.read().get(&id).cloned();
    let show_reasoning = app.config.read().settings.show_reasoning;
    let waiting = running && messages.last().map(|m| m.role == Role::User).unwrap_or(false);
    let turns = build_turns(&messages, updated);
    let n_turns = turns.len();
    let user_ids: Vec<String> = turns.iter().filter_map(|t| t.user.as_ref().map(|u| u.id.clone())).collect();

    // Stick to the bottom while streaming unless the user scrolled up; jump
    // to the bottom when a session opens.
    use_effect(use_reactive!(|updated| {
        let _ = updated;
        document::eval(
            r#"requestAnimationFrame(() => {
                const el = document.getElementById('transcript');
                if (!el) return;
                const near = el.scrollHeight - el.scrollTop - el.clientHeight < 260;
                if (near || el.dataset.fresh !== '0') { el.scrollTop = el.scrollHeight; el.dataset.fresh = '0'; }
            });"#,
        );
    }));

    rsx! {
        if user_ids.len() > 1 {
            div { class: "turn-nav",
                for (i, uid) in user_ids.iter().enumerate() {
                    button {
                        key: "{uid}",
                        class: if i + 1 == user_ids.len() { "on" } else { "" },
                        title: "Jump to prompt {i + 1}",
                        onclick: {
                            let uid = uid.clone();
                            move |_| { document::eval(&format!("document.getElementById('m-{uid}')?.scrollIntoView({{block: 'start', behavior: 'smooth'}})")); }
                        },
                    }
                }
            }
        }
        div { class: "transcript", id: "transcript", "data-fresh": "1",
            onscroll: move |e| {
                let d = e.data();
                let near = (d.scroll_height() as f64 - d.scroll_top() - d.client_height() as f64) < 120.0;
                if near != at_bottom() { at_bottom.set(near); }
            },
            div { class: "transcript-inner selectable",
                div { class: "spacer-msg" }
                if messages.is_empty() {
                    div { class: "empty-state", style: "padding-top: 80px;", "Send a message to get started" }
                }
                for (ti, t) in turns.into_iter().enumerate() {
                    TurnView {
                        key: "{t.key}",
                        session_id: id.clone(),
                        turn: t.clone(),
                        live: running && ti + 1 == n_turns,
                        show_reasoning,
                        feedback: feedback.clone(),
                    }
                }
                if let Some(req) = permission {
                    div { class: "question-card",
                        div { class: "q-head", "Permission required" }
                        div { class: "q-title", "{req.title}" }
                        if let Some(d) = req.detail.clone() {
                            div { class: "q-detail", "{d}" }
                        }
                        for (n, opt) in req.options.clone().into_iter().enumerate() {
                            button {
                                key: "{opt.id}",
                                class: "q-opt",
                                onclick: {
                                    let sid = id.clone();
                                    let oid = opt.id.clone();
                                    move |_| app.respond_permission(&sid, Some(oid.clone()))
                                },
                                span { class: "n", "{n + 1}" }
                                span { "{opt.label}" }
                            }
                        }
                        div { class: "row", style: "justify-content: flex-end; margin-top: 6px;",
                            button { class: "ghost-btn", style: "border: none; background: none;",
                                onclick: { let sid = id.clone(); move |_| app.respond_permission(&sid, None) },
                                "Skip"
                            }
                        }
                    }
                }
                if waiting {
                    div { class: "thinking-row", span { class: "shimmer", "Planning next moves" } }
                }
            }
        }
        if !at_bottom() {
            div { class: "scroll-bottom",
                button { class: "round-btn", title: "Scroll to bottom",
                    onclick: move |_| { document::eval("const el = document.getElementById('transcript'); el && el.scrollTo({top: el.scrollHeight, behavior: 'smooth'});"); },
                    Icon { i: I::ArrowDown, size: 15 }
                }
            }
        }
    }
}

/// One prompt and everything the agent(s) did in response.
#[derive(Clone, PartialEq)]
struct Turn {
    key: String,
    user: Option<Message>,
    replies: Vec<Message>,
    system: Vec<Message>,
    started: chrono::DateTime<chrono::Utc>,
    ended: chrono::DateTime<chrono::Utc>,
}

fn build_turns(messages: &[Message], session_updated: chrono::DateTime<chrono::Utc>) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    for m in messages {
        match m.role {
            Role::User => turns.push(Turn { key: m.id.clone(), user: Some(m.clone()), replies: Vec::new(), system: Vec::new(), started: m.created_at, ended: m.created_at }),
            _ => {
                if turns.is_empty() {
                    turns.push(Turn { key: m.id.clone(), user: None, replies: Vec::new(), system: Vec::new(), started: m.created_at, ended: m.created_at });
                }
                let t = turns.last_mut().unwrap();
                if m.role == Role::System {
                    t.system.push(m.clone());
                } else {
                    t.replies.push(m.clone());
                }
            }
        }
    }
    let n = turns.len();
    for i in 0..n {
        turns[i].ended = if i + 1 < n { turns[i + 1].started } else { session_updated };
    }
    turns
}

fn worked_for(secs: i64) -> String {
    if secs <= 0 {
        String::new()
    } else if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

#[component]
fn TurnView(session_id: String, turn: Turn, live: bool, show_reasoning: bool, feedback: std::collections::HashMap<String, String>) -> Element {
    let app = use_app();
    let mut open = use_signal(|| false);
    // Final answer = the last text part of the last reply that has one.
    let mut final_at: Option<(usize, usize)> = None;
    for (mi, m) in turn.replies.iter().enumerate().rev() {
        if let Some(pi) = m.parts.iter().rposition(|p| matches!(p, Part::Text { text } if !text.trim().is_empty())) {
            final_at = Some((mi, pi));
            break;
        }
    }
    let has_work = turn.replies.iter().enumerate().any(|(mi, m)| m.parts.iter().enumerate().any(|(pi, p)| Some((mi, pi)) != final_at && !matches!(p, Part::Error { .. })));
    let secs = (turn.ended - turn.started).num_seconds();
    let label = match worked_for(secs) {
        s if s.is_empty() || secs > 6 * 3600 => "Worked".to_string(),
        s => format!("Worked for {s}"),
    };
    let last_reply = turn.replies.last().cloned();
    rsx! {
        if let Some(u) = turn.user.clone() {
            UserMessage { key: "{u.id}", session_id: session_id.clone(), msg: u }
        }
        for m in turn.system.iter() {
            for p in m.parts.iter() {
                match p {
                    Part::Notice { text } | Part::Text { text } => rsx! { div { class: "notice", "{text}" } },
                    _ => rsx! {},
                }
            }
        }
        if live {
            // Running: show every step as it happens.
            for (mi, m) in turn.replies.iter().enumerate() {
                AssistantMessage { key: "{m.id}", session_id: session_id.clone(), msg: m.clone(), streaming: mi + 1 == turn.replies.len(), show_reasoning, skip_final: false }
            }
        } else {
            if has_work {
                div { class: "step", onclick: move |_| open.toggle(),
                    span { class: "verb", "{label}" }
                    span { class: "chev", if open() { Icon { i: I::ChevronDown, size: 12 } } else { Icon { i: I::ChevronRight, size: 12 } } }
                }
                if open() {
                    div { class: "sub-steps",
                        for m in turn.replies.iter() {
                            AssistantMessage { key: "w-{m.id}", session_id: session_id.clone(), msg: m.clone(), streaming: false, show_reasoning, skip_final: true }
                        }
                    }
                }
            }
            // Errors stay visible even when the work is collapsed.
            for m in turn.replies.iter() {
                for (pi, p) in m.parts.iter().enumerate() {
                    if let Part::Error { .. } = p {
                        PartView { key: "e-{m.id}-{pi}", session_id: session_id.clone(), part: p.clone(), live: false, show_reasoning }
                    }
                }
            }
            if let Some((mi, pi)) = final_at {
                PartView { key: "f-{turn.key}", session_id: session_id.clone(), part: turn.replies[mi].parts[pi].clone(), live: false, show_reasoning }
            }
            if let Some(m) = last_reply {
                {
                    let fb = feedback.get(&m.id).cloned().unwrap_or_default();
                    let text = m.text();
                    let (sid1, sid2, sid3, mid1, mid2, mid3) = (session_id.clone(), session_id.clone(), session_id.clone(), m.id.clone(), m.id.clone(), m.id.clone());
                    let when = crate::state::rel_time(turn.ended);
                    let ago = if when == "now" { "just now".to_string() } else { format!("{when} ago") };
                    rsx! {
                        div { class: "msg-actions",
                            button { class: if fb == "up" { "icon-btn on" } else { "icon-btn" }, title: "Good response",
                                onclick: move |_| app.set_feedback(&sid1, &mid1, if fb == "up" { None } else { Some("up") }),
                                Icon { i: I::ThumbsUp, size: 14, stroke: 1.5 }
                            }
                            button { class: if feedback.get(&m.id).map(|f| f == "down").unwrap_or(false) { "icon-btn on" } else { "icon-btn" }, title: "Bad response",
                                onclick: move |_| app.set_feedback(&sid2, &mid2, Some("down")),
                                Icon { i: I::ThumbsDown, size: 14, stroke: 1.5 }
                            }
                            button { class: "icon-btn", title: "Copy",
                                onclick: move |_| { app.copy_text(&text); app.toast("Copied"); },
                                Icon { i: I::Copy, size: 14, stroke: 1.5 }
                            }
                            button { class: "icon-btn", title: "Fork from here",
                                onclick: move |_| app.fork(&sid3, Some(mid3.clone())),
                                Icon { i: I::Fork, size: 14, stroke: 1.5 }
                            }
                            span { class: "when", "{ago}" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn UserMessage(session_id: String, msg: Message) -> Element {
    let app = use_app();
    let mut expanded = use_signal(|| false);
    let mut editing = use_signal(|| false);
    let mut draft = use_signal(String::new);
    let text = msg.text();
    let (quote, body) = split_quote(&text);
    let mid = msg.id.clone();
    let attachments: Vec<String> = msg
        .parts
        .iter()
        .filter_map(|p| match p {
            Part::File { path, .. } => Some(short_path(path)),
            Part::Image { source: MediaSource::Path { path }, .. } => Some(short_path(path)),
            Part::Image { .. } => Some("image".into()),
            _ => None,
        })
        .collect();
    let short = text.lines().count() <= 4 && text.len() < 360;
    let text2 = text.clone();
    let (sid_e, mid_e) = (session_id.clone(), mid.clone());
    let agent_label = app.open.read().get(&session_id).map(|s| {
        let m = s.model.clone().filter(|m| m != "default").unwrap_or_else(|| "Auto".into());
        format!("{} · {}", s.agent.display_name(), m)
    });
    if editing() {
        return rsx! {
            div { class: "user-msg", id: "m-{msg.id}",
                div { class: "composer tall", style: "box-shadow: 0 4px 18px rgba(0,0,0,.08);",
                    textarea {
                        id: "edit-{msg.id}",
                        style: "padding: 10px 14px 4px; min-height: 60px; font-size: 14px; line-height: 22px; border: none; outline: none; resize: none; background: transparent; field-sizing: content;",
                        value: "{draft}",
                        autofocus: true,
                        oninput: move |e| draft.set(e.value()),
                        onkeydown: move |e| {
                            if e.key() == Key::Escape { editing.set(false); }
                            if e.key() == Key::Enter && !e.modifiers().shift() {
                                e.prevent_default();
                                let t = draft();
                                editing.set(false);
                                app.edit_and_resend(&sid_e, &mid_e, t);
                            }
                        },
                    }
                    div { class: "composer-row", style: "padding-top: 0;",
                        span { class: "faint", style: "font-size: 12px; padding-left: 8px;", "Edit and resend — continues in a fork, the original stays" }
                        div { class: "grow" }
                        if let Some(l) = agent_label { span { class: "picker", style: "cursor: default;", span { class: "lb sub", "{l}" } } }
                        button { class: "ghost-btn", style: "border: none;", onclick: move |_| editing.set(false), "Cancel" }
                        button { class: "send-btn ready", title: "Resend (Enter)",
                            onclick: {
                                let (s, m) = (session_id.clone(), mid.clone());
                                move |_| { let t = draft(); editing.set(false); app.edit_and_resend(&s, &m, t); }
                            },
                            Icon { i: I::ArrowUp, size: 15, stroke: 2.2 }
                        }
                    }
                }
            }
        };
    }
    rsx! {
        div { class: "user-msg", id: "m-{msg.id}",
            div {
                class: if expanded() || short { "user-card expanded" } else { "user-card" },
                dir: "auto",
                title: "Click to edit · double-click to expand",
                onclick: move |_| {
                    draft.set(text2.clone());
                    editing.set(true);
                },
                ondoubleclick: move |e| { e.stop_propagation(); expanded.toggle(); },
                if let Some(q) = quote {
                    span { class: "q", Icon { i: I::Quote, size: 13 } "\u{201c}{q}\u{201d} " }
                }
                "{body}"
                if !attachments.is_empty() {
                    div { class: "attach-row",
                        for a in attachments {
                            span { class: "attach-chip", Icon { i: I::Paperclip, size: 11 } span { class: "ellipsis", "{a}" } }
                        }
                    }
                }
            }
        }
    }
}

/// Leading `> quoted` lines (or Cursor's ```chat_selection block) →
/// (quote, rest).
fn split_quote(text: &str) -> (Option<String>, String) {
    if let Some(start) = text.find("```chat_selection") {
        let after = &text[start + 17..];
        let end = after.find("```").unwrap_or(after.len());
        let block = &after[..end];
        let sel = block
            .find("selected_text:")
            .map(|i| block[i + 14..].trim().to_string())
            .unwrap_or_else(|| block.trim().to_string());
        let rest = format!("{}{}", &text[..start], after.get(end + 3..).unwrap_or(""));
        let mut quote = sel.split_whitespace().collect::<Vec<_>>().join(" ");
        if quote.chars().count() > 60 {
            quote = quote.chars().take(57).collect::<String>() + "…";
        }
        return (Some(quote), rest.trim().to_string());
    }
    let mut q = Vec::new();
    let mut rest = Vec::new();
    let mut in_quote = true;
    for l in text.lines() {
        if in_quote && l.starts_with("> ") {
            q.push(l[2..].trim().to_string());
        } else if in_quote && l.trim().is_empty() && !q.is_empty() {
            in_quote = false;
        } else {
            in_quote = false;
            rest.push(l);
        }
    }
    if q.is_empty() {
        return (None, text.to_string());
    }
    let mut quote = q.join(" ");
    if quote.chars().count() > 60 {
        quote = quote.chars().take(57).collect::<String>() + "…";
    }
    (Some(quote), rest.join("\n"))
}

/// A part, or a run of exploration tools collapsed into one step.
#[derive(Clone, PartialEq)]
enum Item {
    Part(Part),
    Explore(Vec<ToolCall>),
}

fn is_explore(t: &ToolCall) -> bool {
    matches!(t.kind, ToolKind::Read | ToolKind::Search | ToolKind::Fetch | ToolKind::WebSearch) && t.diffs.is_empty()
}

fn group_parts(parts: &[Part]) -> Vec<Item> {
    let mut out: Vec<Item> = Vec::new();
    for p in parts {
        match p {
            Part::Tool(t) if t.kind == ToolKind::Todo || t.kind == ToolKind::Think => {}
            Part::Tool(t) if is_explore(t) => match out.last_mut() {
                Some(Item::Explore(v)) => v.push(t.clone()),
                _ => out.push(Item::Explore(vec![t.clone()])),
            },
            other => out.push(Item::Part(other.clone())),
        }
    }
    out
}

#[component]
fn AssistantMessage(session_id: String, msg: Message, streaming: bool, show_reasoning: bool, #[props(default = false)] skip_final: bool) -> Element {
    let mut parts = msg.parts.clone();
    if skip_final {
        // Inside "Worked for…": everything except the answer and errors.
        if let Some(i) = parts.iter().rposition(|p| matches!(p, Part::Text { text } if !text.trim().is_empty())) {
            parts.remove(i);
        }
        parts.retain(|p| !matches!(p, Part::Error { .. }));
    }
    let items = group_parts(&parts);
    let n = items.len();
    let meta = msg.usage.as_ref().map(|u| {
        let mut bits = Vec::new();
        if let Some(m) = &msg.model {
            bits.push(m.clone());
        }
        let toks = u.total_tokens();
        if toks > 0 {
            bits.push(format!("{} tokens", human_count(toks)));
        }
        if let Some(c) = u.cost_usd {
            bits.push(format!("${c:.2}"));
        }
        bits.join(" · ")
    });
    rsx! {
        div { class: "a-block",
            for (i, it) in items.into_iter().enumerate() {
                match it {
                    Item::Explore(tools) => rsx! { ExploreGroup { key: "{msg.id}-{i}", tools } },
                    Item::Part(p) => rsx! {
                        PartView {
                            key: "{msg.id}-{i}",
                            session_id: session_id.clone(),
                            part: p,
                            live: streaming && i + 1 == n,
                            show_reasoning,
                        }
                    },
                }
            }
            if streaming && !last_is_live(&msg) {
                div { class: "thinking-row", span { class: "shimmer", "Generating" } }
            }
            if let Some(m) = meta.filter(|m| !m.is_empty() && !streaming && skip_final) {
                div { class: "turn-meta", "{m}" }
            }
        }
    }
}

/// Whether the last part already shows its own live indicator.
fn last_is_live(m: &Message) -> bool {
    match m.parts.last() {
        Some(Part::Tool(t)) => matches!(t.status, ToolStatus::Running | ToolStatus::Pending),
        Some(Part::Reasoning { duration_ms: None, .. }) => true,
        Some(Part::Text { .. }) => true,
        _ => false,
    }
}

#[component]
fn PartView(session_id: String, part: Part, live: bool, show_reasoning: bool) -> Element {
    let app = use_app();
    match part {
        Part::Text { text } => {
            let html = markdown::to_html(&text);
            rsx! { div { class: "a-text md", dir: "auto", dangerous_inner_html: "{html}" } }
        }
        Part::Reasoning { text, duration_ms } => {
            if !show_reasoning {
                return rsx! {};
            }
            rsx! { Reasoning { text, duration_ms, live } }
        }
        Part::Tool(t) => rsx! { ToolView { tool: t } },
        Part::Plan { entries } => {
            let done = entries.iter().filter(|e| e.status == PlanStatus::Completed).count();
            rsx! {
                div { class: "plan-card",
                    div { class: "plan-head", "{entries.len()} To-dos · {done} done" }
                    for (i, e) in entries.iter().enumerate() {
                        div {
                            key: "{i}",
                            class: match e.status {
                                PlanStatus::Completed => "plan-entry completed",
                                PlanStatus::InProgress => "plan-entry in_progress",
                                _ => "plan-entry",
                            },
                            span { class: "box",
                                if e.status == PlanStatus::Completed { Icon { i: I::Check, size: 9, stroke: 3.0 } }
                            }
                            span { class: "txt", "{e.content}" }
                        }
                    }
                }
            }
        }
        Part::Error { message } => {
            let m = message.to_lowercase();
            let needs_login = m.contains("logged in") || m.contains("/login") || m.contains("authenticat") || m.contains("api key");
            rsx! {
                div { class: "err-card",
                    "{message}"
                    if needs_login {
                        div { style: "margin-top: 8px;",
                            button { class: "ghost-btn",
                                onclick: move |_| {
                                    let s = app.open.read().get(&session_id).cloned();
                                    if let Some(s) = s {
                                        let spec = s.agent.spec();
                                        let cwd = s.cwd().unwrap_or_else(|| ".".into());
                                        match spec.login_command {
                                            Some(cmd) => {
                                                let _ = crate::state::open_terminal(&cwd, cmd);
                                                app.toast(format!("Finish signing in to {} in the terminal, then retry", spec.name));
                                            }
                                            None => app.open_settings(crate::state::SettingsTab::Accounts),
                                        }
                                    }
                                },
                                Icon { i: I::Key, size: 12 }
                                "Sign in"
                            }
                        }
                    }
                }
            }
        }
        Part::Notice { text } => rsx! { div { class: "notice", "{text}" } },
        Part::File { path, .. } => rsx! {
            div { class: "tool-card", div { class: "tool-card-head", span { class: "ic", Icon { i: I::Paperclip, size: 13 } } span { class: "name", "{short_path(&path)}" } } }
        },
        Part::Image { source, .. } => {
            let src = match &source {
                MediaSource::Base64 { data } => Some(format!("data:image/png;base64,{data}")),
                MediaSource::Url { url } if url.starts_with("http") => Some(url.clone()),
                _ => None,
            };
            let label = match &source {
                MediaSource::Path { path } => short_path(path),
                MediaSource::Url { url } => url.clone(),
                MediaSource::Base64 { .. } => "image".into(),
            };
            rsx! {
                if let Some(src) = src {
                    img { src: "{src}", style: "max-width: 100%; border-radius: 8px; margin: 4px 0; border: 1px solid var(--border);" }
                } else {
                    div { class: "tool-card", div { class: "tool-card-head", span { class: "ic", Icon { i: I::Image, size: 13 } } span { class: "name", "{label}" } } }
                }
            }
        }
    }
}

#[component]
fn Reasoning(text: String, duration_ms: Option<u64>, live: bool) -> Element {
    let mut open = use_signal(|| false);
    let (verb, rest) = match duration_ms {
        Some(ms) if ms >= 1500 => ("Thought".to_string(), format!("for {}s", (ms + 500) / 1000)),
        Some(_) => ("Thought".to_string(), "briefly".to_string()),
        None if live => ("Thinking".to_string(), String::new()),
        None => ("Thought".to_string(), String::new()),
    };
    let is_live = live && duration_ms.is_none();
    rsx! {
        div {
            class: if is_live { "step running" } else { "step" },
            onclick: move |_| open.toggle(),
            span { class: "verb", "{verb}" }
            if !rest.is_empty() { span { class: "target", "{rest}" } }
            span { class: "chev", if open() { Icon { i: I::ChevronDown, size: 12 } } else { Icon { i: I::ChevronRight, size: 12 } } }
        }
        if open() || is_live {
            div { class: "reasoning-text", "{text.trim()}" }
        }
    }
}

fn is_running(t: &ToolCall) -> bool {
    matches!(t.status, ToolStatus::Running | ToolStatus::Pending | ToolStatus::Approval)
}

/// Verb + target for a step ("Read" + "src/main.rs").
fn verb_target(t: &ToolCall) -> (&'static str, String) {
    let running = is_running(t);
    let title = t.title.clone().unwrap_or_default();
    let rest = title.split_once(' ').map(|(_, r)| r.trim().to_string()).unwrap_or_default();
    let verb = match (t.kind, running) {
        (ToolKind::Read, true) => "Reading",
        (ToolKind::Read, false) => "Read",
        (ToolKind::Edit, true) => "Editing",
        (ToolKind::Edit, false) => "Edited",
        (ToolKind::Write, true) => "Writing",
        (ToolKind::Write, false) => "Wrote",
        (ToolKind::Delete, _) => "Deleted",
        (ToolKind::Move, _) => "Moved",
        (ToolKind::Search, true) => "Searching",
        (ToolKind::Search, false) => "Searched",
        (ToolKind::Shell, true) => "Running",
        (ToolKind::Shell, false) => "Ran",
        (ToolKind::Fetch, true) => "Fetching",
        (ToolKind::Fetch, false) => "Fetched",
        (ToolKind::WebSearch, true) => "Searching web",
        (ToolKind::WebSearch, false) => "Searched web",
        (ToolKind::Think, _) => "Thought",
        (ToolKind::Task, true) => "Running subagent",
        (ToolKind::Task, false) => "Subagent",
        (ToolKind::Todo, _) => "Updated to-dos",
        (ToolKind::Mcp, true) => "Calling",
        (ToolKind::Mcp, false) => "Called",
        (ToolKind::Other, true) => "Running",
        (ToolKind::Other, false) => "Used",
    };
    let target = match t.kind {
        ToolKind::Other | ToolKind::Mcp => {
            let name = t.name.trim_start_matches("mcp__").replace("__", " › ");
            if rest.is_empty() || t.kind == ToolKind::Other { name } else { format!("{name} {rest}") }
        }
        ToolKind::Todo | ToolKind::Think => String::new(),
        _ => rest,
    };
    (verb, target)
}

/// Short present-tense label for the sidebar ("Reading main.rs").
pub fn running_label(t: &ToolCall) -> String {
    let (v, target) = verb_target(t);
    format!("{v} {target}").trim().to_string()
}

#[component]
fn ExploreGroup(tools: Vec<ToolCall>) -> Element {
    let mut open = use_signal(|| false);
    let running = tools.iter().any(is_running);
    if tools.len() == 1 {
        return rsx! { Step { tool: tools[0].clone() } };
    }
    let files = tools.iter().filter(|t| t.kind == ToolKind::Read).count();
    let searches = tools.len() - files;
    let mut bits = Vec::new();
    if files > 0 {
        bits.push(format!("{files} file{}", if files == 1 { "" } else { "s" }));
    }
    if searches > 0 {
        bits.push(format!("{searches} search{}", if searches == 1 { "" } else { "es" }));
    }
    let summary = bits.join(", ");
    rsx! {
        div { class: if running { "step running" } else { "step" }, onclick: move |_| open.toggle(),
            span { class: "verb", if running { "Exploring" } else { "Explored" } }
            span { class: "target", "{summary}" }
            span { class: "chev", if open() { Icon { i: I::ChevronDown, size: 12 } } else { Icon { i: I::ChevronRight, size: 12 } } }
        }
        if open() {
            div { class: "sub-steps",
                for (i, t) in tools.into_iter().enumerate() {
                    Step { key: "{i}", tool: t }
                }
            }
        }
    }
}

#[component]
fn Step(tool: ToolCall) -> Element {
    let mut open = use_signal(|| false);
    let (verb, target) = verb_target(&tool);
    let failed = tool.is_error || tool.status == ToolStatus::Failed;
    let has_output = tool.output.as_ref().map(|o| !o.trim().is_empty()).unwrap_or(false);
    let output = clip(tool.output.clone().unwrap_or_default(), 12_000);
    rsx! {
        div {
            class: if is_running(&tool) { "step running" } else if failed { "step failed" } else { "step" },
            onclick: move |_| if has_output { open.toggle() },
            span { class: "verb", "{verb}" }
            span { class: "target", "{target}" }
            if has_output {
                span { class: "chev", if open() { Icon { i: I::ChevronDown, size: 12 } } else { Icon { i: I::ChevronRight, size: 12 } } }
            }
        }
        if open() {
            div { class: "tool-card", pre { "{output}" } }
        }
    }
}

#[component]
fn ToolView(tool: ToolCall) -> Element {
    if tool.kind.is_mutation() && !tool.diffs.is_empty() && !tool.is_error {
        return rsx! {
            for (i, d) in tool.diffs.iter().enumerate() {
                EditCard { key: "{i}", diff: d.clone(), running: is_running(&tool) }
            }
        };
    }
    match tool.kind {
        ToolKind::Shell => rsx! { CommandCard { tool } },
        _ => rsx! { Step { tool } },
    }
}

/// `▤ file.rs +10 -9` card with a few changed lines previewed underneath.
#[component]
fn EditCard(diff: FileDiff, running: bool) -> Element {
    let mut app = use_app();
    let mut open = use_signal(|| false);
    let name = std::path::Path::new(&diff.path).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or(diff.path.clone());
    let lines = preview_lines(&diff);
    let path = diff.path.clone();
    rsx! {
        div { class: "tool-card",
            div { class: "tool-card-head", title: "{diff.path}", "data-path": "{diff.path}",
                onclick: move |_| {
                    let mut ui = app.ui.write();
                    ui.right_open = true;
                    ui.right_tab = RightTab::Changes;
                    ui.reveal_file = Some(path.clone());
                },
                span { class: "ic", if running { span { class: "sunburst", Icon { i: I::Loader, size: 13 } } } else { Icon { i: I::Lines, size: 13 } } }
                span { class: "name", "{name}" }
                span { class: "stats",
                    if diff.added > 0 || diff.removed == 0 { span { class: "add", "+{diff.added}" } }
                    if diff.removed > 0 { span { class: "del", "-{diff.removed}" } }
                }
            }
            if !lines.is_empty() {
                div { class: if open() { "preview open" } else { "preview" }, onclick: move |_| open.toggle(),
                    for (i, (k, t)) in lines.into_iter().enumerate() {
                        div { key: "{i}", class: "pv-line {k}", "{t}" }
                    }
                }
            }
        }
    }
}

fn preview_lines(d: &FileDiff) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    if let Some(u) = &d.unified {
        for l in u.lines() {
            if l.starts_with("@@") || l.starts_with("+++") || l.starts_with("---") || l.starts_with("diff ") || l.starts_with("index ") {
                continue;
            }
            if let Some(t) = l.strip_prefix('+') {
                out.push(("add", t.to_string()));
            } else if let Some(t) = l.strip_prefix('-') {
                out.push(("del", t.to_string()));
            } else {
                out.push(("ctx", l.strip_prefix(' ').unwrap_or(l).to_string()));
            }
            if out.len() >= 400 {
                break;
            }
        }
        // Start the preview at the first change.
        if let Some(first) = out.iter().position(|(k, _)| *k != "ctx") {
            out.drain(..first.saturating_sub(1));
        }
        return out;
    }
    let old = d.old_text.as_deref().unwrap_or("");
    let new = d.new_text.as_deref().unwrap_or("");
    for (k, t) in line_ops(old, new, 1) {
        out.push((k, t));
        if out.len() >= 400 {
            break;
        }
    }
    out
}

/// Minimal line diff with `context` unchanged lines around each change.
pub fn line_ops(old: &str, new: &str, context: usize) -> Vec<(&'static str, String)> {
    use similar::{ChangeTag, TextDiff};
    let diff = TextDiff::from_lines(old, new);
    let mut out = Vec::new();
    for (gi, group) in diff.grouped_ops(context).iter().enumerate() {
        if gi > 0 {
            out.push(("ctx", "⋯".to_string()));
        }
        for op in group {
            for ch in diff.iter_changes(op) {
                let kind = match ch.tag() {
                    ChangeTag::Insert => "add",
                    ChangeTag::Delete => "del",
                    ChangeTag::Equal => "ctx",
                };
                out.push((kind, ch.value().trim_end_matches(['\n', '\r']).to_string()));
            }
        }
    }
    out
}

/// Shell command card: description first, the commands it ran muted.
#[component]
fn CommandCard(tool: ToolCall) -> Element {
    let mut open = use_signal(|| false);
    let command = tool
        .input
        .get("command")
        .and_then(|c| c.as_str().map(|s| s.to_string()).or_else(|| c.as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(" "))))
        .or_else(|| tool.input.get("cmd").and_then(|c| c.as_str()).map(|s| s.to_string()))
        .unwrap_or_else(|| tool.title.clone().unwrap_or_default().trim_start_matches("Ran ").to_string());
    let description = tool.input.get("description").and_then(|d| d.as_str()).map(|s| s.to_string());
    let programs = command_programs(&command);
    let running = is_running(&tool);
    let failed = tool.is_error || tool.status == ToolStatus::Failed;
    let output = clip(tool.output.clone().unwrap_or_default(), 16_000);
    rsx! {
        div { class: "tool-card",
            div { class: "tool-card-head", onclick: move |_| open.toggle(),
                span { class: "ic",
                    if running { span { class: "sunburst", Icon { i: I::Loader, size: 13 } } }
                    else if failed { span { style: "color: var(--red)", Icon { i: I::Alert, size: 13 } } }
                    else { Icon { i: I::Terminal, size: 13 } }
                }
                match description {
                    Some(d) => rsx! {
                        span { class: "desc", "{d}" }
                        span { class: "meta", "{programs}" }
                    },
                    None => rsx! { span { class: "desc mono", style: "font-size: 11px;", "{larrier_core::lsf::first_line(&command)}" } },
                }
            }
            if open() {
                div { class: "cmd", "$ {command}" }
                if !output.trim().is_empty() {
                    pre { "{output}" }
                }
            }
        }
    }
}

/// "cd, git" from "cd repo && git log --oneline | head".
fn command_programs(cmd: &str) -> String {
    let mut progs: Vec<String> = Vec::new();
    for seg in cmd.split(['|', ';', '&', '\n']) {
        let w = seg.split_whitespace().find(|w| !w.contains('=')).unwrap_or("");
        let w = w.trim_matches(['(', ')', '"', '\'']);
        let base = w.rsplit(['/', '\\']).next().unwrap_or(w);
        if !base.is_empty() && !progs.iter().any(|p| p == base) {
            progs.push(base.to_string());
        }
    }
    if progs.len() > 3 {
        let extra = progs.len() - 2;
        progs.truncate(2);
        return format!("{}, {}+", progs.join(", "), extra);
    }
    progs.join(", ")
}

fn clip(s: String, max: usize) -> String {
    if s.len() <= max {
        return s;
    }
    let cut = s.char_indices().nth(max).map(|(i, _)| i).unwrap_or(s.len());
    format!("{}\n… ({} more bytes)", &s[..cut], s.len() - cut)
}

pub fn human_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn programs() {
        assert_eq!(super::command_programs("cd repo && git log --oneline | head -5"), "cd, git, head");
        assert_eq!(super::command_programs("ls -la"), "ls");
    }
}
