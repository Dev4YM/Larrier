//! Rendering of an LSF conversation the way Cursor's Agents Window does it:
//! sticky prompt cards, quiet "Read / Searched / Thought" steps grouped into
//! "Explored …", command cards, edit cards with inline diff previews, plans
//! and live streaming state.
//!
//! Built for very long chats: only the newest turns are mounted (older ones
//! load as you scroll up), and each turn re-renders only when its own
//! content changes — a streaming reply never re-renders the history.

use std::hash::{Hash, Hasher};

use dioxus::prelude::*;
use larrier_core::{
    lsf::{short_path, MediaSource},
    FileDiff, Message, Part, PlanStatus, Role, ToolCall, ToolKind, ToolStatus,
};

use crate::icons::{Icon, I};
use crate::markdown;
use crate::state::{use_app, RightTab};

/// Turns mounted at first; scrolling up loads this many more each time.
const TURN_WINDOW: usize = 24;
/// Most prompts shown as dashes in the turn navigator.
const NAV_MAX: usize = 48;
/// Steps of one turn mounted at first; huge turns reveal more on demand.
const STEP_WINDOW: usize = 80;

/// Where a turn sits in the message list.
#[derive(Clone, PartialEq)]
struct TurnSpan {
    key: String,
    start: usize,
    end: usize,
    user_id: Option<String>,
}

fn turn_spans(messages: &[Message]) -> Vec<TurnSpan> {
    let mut out: Vec<TurnSpan> = Vec::new();
    for (i, m) in messages.iter().enumerate() {
        if m.role == Role::User || out.is_empty() {
            out.push(TurnSpan { key: m.id.clone(), start: i, end: i + 1, user_id: (m.role == Role::User).then(|| m.id.clone()) });
        } else if let Some(t) = out.last_mut() {
            t.end = i + 1;
        }
    }
    out
}

/// Cheap content fingerprint of a turn: changes whenever anything it shows
/// changes (text grows, a tool finishes, feedback is given).
fn turn_fp(msgs: &[Message], feedback: Option<&str>) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for m in msgs {
        m.id.hash(&mut h);
        m.parts.len().hash(&mut h);
        for p in &m.parts {
            match p {
                Part::Text { text } => (0u8, text.len()).hash(&mut h),
                Part::Reasoning { text, duration_ms } => (1u8, text.len(), duration_ms).hash(&mut h),
                Part::Tool(t) => {
                    2u8.hash(&mut h);
                    std::mem::discriminant(&t.status).hash(&mut h);
                    (t.output.as_ref().map(|o| o.len()), t.diffs.len(), t.is_error, t.subagent.len(), t.title.as_ref().map(|x| x.len())).hash(&mut h);
                }
                Part::Plan { entries } => {
                    3u8.hash(&mut h);
                    for e in entries {
                        (e.content.len(), std::mem::discriminant(&e.status)).hash(&mut h);
                    }
                }
                Part::Error { message } => (4u8, message.len()).hash(&mut h),
                Part::Notice { text } => (5u8, text.len()).hash(&mut h),
                Part::File { path, .. } => (6u8, path.len()).hash(&mut h),
                Part::Image { .. } => 7u8.hash(&mut h),
            }
        }
    }
    feedback.hash(&mut h);
    h.finish()
}

/// What the transcript mounts for one turn.
#[derive(Clone, PartialEq)]
struct TurnRow {
    key: String,
    start: usize,
    end: usize,
    fp: u64,
    started: chrono::DateTime<chrono::Utc>,
    ended: chrono::DateTime<chrono::Utc>,
}

#[component]
pub fn Transcript(id: String) -> Element {
    let app = use_app();
    let mut at_bottom = use_signal(|| true);
    let mut shown = use_signal(|| TURN_WINDOW);
    let Some(sig) = app.session_sig(&id) else { return rsx! {} };
    let tid = format!("transcript-{}", &id[id.len().saturating_sub(10)..]);

    // Everything the view needs, computed without cloning the conversation.
    let (rows, nav, total, empty, waiting, updated) = {
        let s = sig.read();
        let spans = turn_spans(&s.messages);
        let total = spans.len();
        let first = total.saturating_sub(shown());
        let rows: Vec<TurnRow> = spans[first..]
            .iter()
            .enumerate()
            .map(|(i, sp)| {
                let msgs = &s.messages[sp.start..sp.end];
                let last_reply = msgs.iter().rev().find(|m| m.role != Role::User && m.role != Role::System);
                let fb = last_reply.and_then(|m| s.extra.get(&format!("feedback:{}", m.id))).and_then(|v| v.as_str());
                let next = spans.get(first + i + 1).map(|n| s.messages[n.start].created_at);
                TurnRow {
                    key: sp.key.clone(),
                    start: sp.start,
                    end: sp.end,
                    fp: turn_fp(msgs, fb),
                    started: msgs[0].created_at,
                    ended: next.unwrap_or(s.updated_at),
                }
            })
            .collect();
        let users: Vec<(usize, String)> = spans.iter().enumerate().filter_map(|(i, t)| t.user_id.clone().map(|u| (i, u))).collect();
        let nav: Vec<(usize, String)> = users[users.len().saturating_sub(NAV_MAX)..].to_vec();
        let running = app.is_active(&id);
        let waiting = running && s.messages.last().map(|m| m.role == Role::User).unwrap_or(false);
        (rows, nav, total, s.messages.is_empty(), waiting, s.updated_at)
    };
    let first = total - rows.len();
    let running = app.is_active(&id);
    let permission = app.permissions.read().get(&id).cloned();
    let show_reasoning = app.config.read().settings.show_reasoning;
    let n_rows = rows.len();

    // Stick to the bottom while streaming unless the user scrolled up; jump
    // to the bottom when a session opens.
    let tid2 = tid.clone();
    use_effect(use_reactive!(|updated| {
        let _ = updated;
        document::eval(&format!(
            r#"requestAnimationFrame(() => {{
                const el = document.getElementById('{tid2}');
                if (!el) return;
                if (!el.__wired) {{
                    el.__wired = true;
                    const mark = () => {{ el.__userAt = performance.now(); }};
                    el.addEventListener('wheel', mark, {{ passive: true }});
                    el.addEventListener('touchmove', mark, {{ passive: true }});
                    el.addEventListener('keydown', mark);
                }}
                const fresh = el.dataset.fresh !== '0';
                const near = el.scrollHeight - el.scrollTop - el.clientHeight < 260;
                if (!fresh && !near) return;
                el.dataset.fresh = '0';
                // Opening a chat, or reduced motion: jump. Otherwise glide.
                if (fresh || el.closest('.no-motion')) {{ el.scrollTop = el.scrollHeight; return; }}
                if (el.__gliding) return;
                el.__gliding = true;
                const step = () => {{
                    if (performance.now() - (el.__userAt || 0) < 700) {{ el.__gliding = false; return; }}
                    const d = el.scrollHeight - el.clientHeight - el.scrollTop;
                    if (d <= 1) {{ el.__gliding = false; return; }}
                    el.scrollTop += Math.max(1, Math.ceil(d * 0.2));
                    requestAnimationFrame(step);
                }};
                requestAnimationFrame(step);
            }});"#
        ));
    }));

    let tid3 = tid.clone();
    rsx! {
        if nav.len() > 1 {
            div { class: "turn-nav",
                for (n, (ti, uid)) in nav.iter().cloned().enumerate() {
                    button {
                        key: "{uid}",
                        class: if n + 1 == nav.len() { "on" } else { "" },
                        title: "Jump to prompt {ti + 1}",
                        onclick: move |_| {
                            // Mount the turn first if it is above the loaded window.
                            if ti < first {
                                shown.set(total - ti);
                            }
                            document::eval(&format!("setTimeout(() => document.getElementById('m-{uid}')?.scrollIntoView({{block: 'start', behavior: 'smooth'}}), 30)"));
                        },
                    }
                }
            }
        }
        div { class: "transcript", id: "{tid}", "data-fresh": "1",
            onscroll: move |e| {
                let d = e.data();
                let near = (d.scroll_height() as f64 - d.scroll_top() - d.client_height() as f64) < 120.0;
                if near != at_bottom() { at_bottom.set(near); }
                // Load earlier turns before the user reaches the top; scroll
                // anchoring keeps the view steady while they mount.
                if d.scroll_top() < 600.0 && first > 0 {
                    let want = total - first + TURN_WINDOW;
                    if shown() < want {
                        shown.set(want);
                    }
                }
            },
            // Clicking empty space or another message ends editing a prompt.
            onclick: move |_| {
                if app.editing.peek().msg.is_some() {
                    app.request_edit(None);
                }
            },
            div { class: "transcript-inner selectable",
                div { class: "spacer-msg" }
                if first > 0 {
                    button { class: "load-earlier", onclick: move |e| { e.stop_propagation(); shown.set(shown() + TURN_WINDOW * 4); },
                        "Show earlier messages ({first})"
                    }
                }
                if empty {
                    div { class: "empty-state", style: "padding-top: 80px;", "Send a message to get started" }
                }
                for (i, r) in rows.into_iter().enumerate() {
                    TurnView {
                        key: "{r.key}",
                        session_id: id.clone(),
                        start: r.start,
                        end: r.end,
                        fp: r.fp,
                        started: r.started,
                        ended: r.ended,
                        live: running && i + 1 == n_rows,
                        show_reasoning,
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
            // Sticky inside the scroller: always just above the composer,
            // however tall the composer grows.
            if !at_bottom() {
                div { class: "scroll-bottom",
                    button { class: "round-btn", title: "Scroll to bottom",
                        onclick: move |e| {
                            e.stop_propagation();
                            document::eval(&format!("const el = document.getElementById('{tid3}'); el && el.scrollTo({{top: el.scrollHeight, behavior: 'smooth'}});"));
                        },
                        Icon { i: I::ArrowDown, size: 15 }
                    }
                }
            }
        }
    }
}

/// One prompt and everything the agent(s) did in response.
#[derive(Clone, PartialEq)]
struct Turn {
    user: Option<Message>,
    replies: Vec<Message>,
    system: Vec<Message>,
}

fn build_turn(msgs: &[Message]) -> Turn {
    let mut t = Turn { user: None, replies: Vec::new(), system: Vec::new() };
    for m in msgs {
        match m.role {
            Role::User if t.user.is_none() => t.user = Some(m.clone()),
            Role::System => t.system.push(m.clone()),
            _ => t.replies.push(m.clone()),
        }
    }
    t
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

/// A turn. Reads its messages without subscribing: it re-renders only when
/// its fingerprint (`fp`) or liveness changes.
#[component]
fn TurnView(
    session_id: String,
    start: usize,
    end: usize,
    fp: u64,
    started: chrono::DateTime<chrono::Utc>,
    ended: chrono::DateTime<chrono::Utc>,
    live: bool,
    show_reasoning: bool,
) -> Element {
    let app = use_app();
    let mut open = use_signal(|| false);
    let mut steps = use_signal(|| STEP_WINDOW);
    let _ = fp;
    let Some((turn, feedback)) = app.peek_session(&session_id, |s| {
        let end = end.min(s.messages.len());
        let start = start.min(end);
        let t = build_turn(&s.messages[start..end]);
        let fb = t
            .replies
            .last()
            .and_then(|m| s.extra.get(&format!("feedback:{}", m.id)))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        (t, fb)
    }) else {
        return rsx! {};
    };
    // Final answer = the last text part of the last reply that has one.
    let mut final_at: Option<(usize, usize)> = None;
    for (mi, m) in turn.replies.iter().enumerate().rev() {
        if let Some(pi) = m.parts.iter().rposition(|p| matches!(p, Part::Text { text } if !text.trim().is_empty())) {
            final_at = Some((mi, pi));
            break;
        }
    }
    let has_work = turn.replies.iter().enumerate().any(|(mi, m)| m.parts.iter().enumerate().any(|(pi, p)| Some((mi, pi)) != final_at && !matches!(p, Part::Error { .. })));
    let secs = (ended - started).num_seconds();
    let label = match worked_for(secs) {
        s if s.is_empty() || secs > 6 * 3600 => "Worked".to_string(),
        s => format!("Worked for {s}"),
    };
    let last_reply = turn.replies.last().cloned();
    let hidden = turn.replies.len().saturating_sub(steps());
    let turn_key = turn.user.as_ref().or(turn.replies.first()).map(|m| m.id.clone()).unwrap_or_default();
    // Each turn is its own box so its sticky prompt card scrolls away when
    // the next prompt arrives (instead of stacking under it).
    rsx! {
        div { class: if live { "turn live" } else { "turn" },
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
            // Running: show the steps as they happen (the newest ones; long
            // turns reveal earlier steps on demand).
            if hidden > 0 {
                button { class: "load-earlier", onclick: move |e| { e.stop_propagation(); steps.set(steps() + STEP_WINDOW * 3); }, "Show {hidden} earlier steps" }
            }
            for (mi, m) in turn.replies.iter().enumerate().skip(hidden) {
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
                        if hidden > 0 {
                            button { class: "load-earlier", onclick: move |e| { e.stop_propagation(); steps.set(steps() + STEP_WINDOW * 3); }, "Show {hidden} earlier steps" }
                        }
                        for m in turn.replies.iter().skip(hidden) {
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
                PartView { key: "f-{turn_key}", session_id: session_id.clone(), part: turn.replies[mi].parts[pi].clone(), live: false, show_reasoning }
            }
            if let Some(m) = last_reply {
                {
                    let fb = feedback.clone();
                    let text = m.text();
                    let (sid1, sid2, sid3, mid1, mid2, mid3) = (session_id.clone(), session_id.clone(), session_id.clone(), m.id.clone(), m.id.clone(), m.id.clone());
                    let when = crate::state::rel_time(ended);
                    let ago = if when == "now" { "just now".to_string() } else { format!("{when} ago") };
                    let up = fb == "up";
                    let down = fb == "down";
                    rsx! {
                        div { class: "msg-actions",
                            button { class: if up { "icon-btn on" } else { "icon-btn" }, title: "Good response",
                                onclick: move |_| app.set_feedback(&sid1, &mid1, if up { None } else { Some("up") }),
                                Icon { i: I::ThumbsUp, size: 14, stroke: 1.5 }
                            }
                            button { class: if down { "icon-btn on" } else { "icon-btn" }, title: "Bad response",
                                onclick: move |_| app.set_feedback(&sid2, &mid2, if down { None } else { Some("down") }),
                                Icon { i: I::ThumbsDown, size: 14, stroke: 1.5 }
                            }
                            super::CopyButton { text: text.clone() }
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
}

#[component]
pub fn UserMessage(session_id: String, msg: Message) -> Element {
    let mut app = use_app();
    let mut expanded = use_signal(|| false);
    let mid_m = msg.id.clone();
    // Only this message re-renders when editing starts or stops.
    let editing = use_memo(move || app.editing.read().msg.as_deref() == Some(mid_m.as_str()));
    let text = msg.text();
    if let Some(note) = task_notification_note(&text) {
        return rsx! { div { class: "notice task-note", id: "m-{msg.id}", Icon { i: I::Bot, size: 12, stroke: 1.5 } "{note}" } };
    }
    let (quote, body) = split_quote(&text);
    // Mentioned chats, files and skills show as chips, not as the raw tokens
    // and context blocks that were sent to the agent.
    let (body, chips) = super::mentions::split_mentions(&body);
    let mid = msg.id.clone();
    let attachments: Vec<String> = msg
        .parts
        .iter()
        .filter_map(|p| match p {
            Part::File { path, text: Some(_), .. } => Some(path.clone()),
            Part::File { path, .. } => Some(short_path(path)),
            _ => None,
        })
        .collect();
    let images: Vec<String> = msg.parts.iter().filter(|p| matches!(p, Part::Image { .. })).filter_map(crate::state::image_src).collect();
    let short = body.lines().count() <= 4 && body.len() < 360;
    let text2 = text.clone();
    let (sid_e, mid_e) = (session_id.clone(), mid.clone());
    if editing() {
        let st = app.editing.read().clone();
        let confirm = st.confirm.is_some();
        let agent_label = app.peek_session(&session_id, |s| {
            let m = s.model.clone().filter(|m| m != "default").unwrap_or_else(|| "Auto".into());
            format!("{} · {}", s.agent.display_name(), m)
        });
        let resend = move |sid: &str, mid: &str| {
            let t = app.editing.peek().draft.clone();
            app.set_edit(None);
            app.edit_and_resend(sid, mid, t);
        };
        return rsx! {
            div { class: if confirm { "user-msg editing confirming" } else { "user-msg editing" }, id: "m-{msg.id}",
                onclick: move |e| e.stop_propagation(),
                div { class: "composer tall edit-box",
                    textarea {
                        id: "edit-{msg.id}",
                        class: "edit-input",
                        value: "{st.draft}",
                        autofocus: true,
                        oninput: move |e| app.editing.write().draft = e.value(),
                        onkeydown: move |e| {
                            if e.key() == Key::Escape {
                                e.stop_propagation();
                                if app.editing.peek().confirm.is_some() {
                                    app.editing.write().confirm = None;
                                } else {
                                    app.request_edit(None);
                                }
                            }
                            if e.key() == Key::Enter && !e.modifiers().shift() {
                                e.prevent_default();
                                resend(&sid_e, &mid_e);
                            }
                        },
                    }
                    if confirm {
                        div { class: "edit-confirm",
                            Icon { i: I::Alert, size: 14 }
                            span { class: "grow", "Discard your changes to this message?" }
                            button { class: "ghost-btn",
                                onclick: move |_| {
                                    app.editing.write().confirm = None;
                                    document::eval("document.querySelector('.user-msg.editing textarea')?.focus()");
                                },
                                "Keep editing"
                            }
                            button { class: "danger-btn",
                                onclick: move |_| {
                                    let next = app.editing.peek().confirm.clone().flatten();
                                    app.set_edit(next);
                                },
                                "Discard"
                            }
                        }
                    } else {
                        div { class: "composer-row", style: "padding-top: 0;",
                            span { class: "faint edit-hint", "Edit and resend — continues in a fork, the original stays · Esc to cancel" }
                            div { class: "grow" }
                            if let Some(l) = agent_label { span { class: "picker", style: "cursor: default;", span { class: "lb sub", "{l}" } } }
                            button { class: "ghost-btn", style: "border: none;", onclick: move |_| app.request_edit(None), "Cancel" }
                            button { class: "send-btn ready", title: "Resend (Enter)",
                                onclick: {
                                    let (s, m) = (session_id.clone(), mid.clone());
                                    move |_| resend(&s, &m)
                                },
                                Icon { i: I::ArrowUp, size: 15, stroke: 2.2 }
                            }
                        }
                    }
                }
            }
        };
    }
    let mid_c = msg.id.clone();
    rsx! {
        div { class: "user-msg", id: "m-{msg.id}",
            div {
                class: if expanded() || short { "user-card expanded" } else { "user-card" },
                dir: "auto",
                title: "Click to edit · double-click to expand",
                onclick: move |e| {
                    e.stop_propagation();
                    app.request_edit(Some((mid_c.clone(), text2.clone())));
                },
                ondoubleclick: move |e| { e.stop_propagation(); expanded.toggle(); },
                if !chips.is_empty() {
                    super::mentions::MentionChips { chips: chips.clone() }
                }
                if let Some(q) = quote {
                    span { class: "q", Icon { i: I::Quote, size: 13 } "\u{201c}{q}\u{201d} " }
                }
                "{body}"
                if !images.is_empty() {
                    div { class: "msg-images",
                        for (i, src) in images.into_iter().enumerate() {
                            img { key: "{i}", src: "{src}",
                                onclick: move |e| { e.stop_propagation(); app.ui.write().lightbox = Some(src.clone()); },
                            }
                        }
                    }
                }
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

/// "Background task finished · …" for a prompt that is only a Claude Code
/// `<task-notification>` envelope.
fn task_notification_note(text: &str) -> Option<String> {
    let t = text.trim();
    if !t.starts_with("<task-notification>") {
        return None;
    }
    let tag = |name: &str| {
        let open = format!("<{name}>");
        let close = format!("</{name}>");
        let i = t.find(&open)? + open.len();
        let j = t[i..].find(&close)? + i;
        Some(t[i..j].trim().to_string())
    };
    let status = tag("status").unwrap_or_else(|| "finished".into());
    let summary = tag("summary").or_else(|| tag("task-id")).unwrap_or_default();
    Some(if summary.is_empty() { format!("Background task {status}") } else { format!("Background task {status} · {summary}") })
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
    // Only the newest to-do list of a message is shown (each call repeats it).
    let last_todo = parts.iter().rposition(|p| matches!(p, Part::Tool(t) if t.kind == ToolKind::Todo));
    let mut out: Vec<Item> = Vec::new();
    for (i, p) in parts.iter().enumerate() {
        match p {
            Part::Tool(t) if t.kind == ToolKind::Think => {}
            Part::Tool(t) if t.kind == ToolKind::Todo && Some(i) != last_todo => {}
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
pub fn AssistantMessage(session_id: String, msg: Message, streaming: bool, show_reasoning: bool, #[props(default = false)] skip_final: bool) -> Element {
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
pub fn PartView(session_id: String, part: Part, live: bool, show_reasoning: bool) -> Element {
    let app = use_app();
    match part {
        Part::Text { text } => {
            let html = markdown::to_html(&text);
            rsx! { div { class: if live { "a-text md streaming" } else { "a-text md" }, dir: "auto", dangerous_inner_html: "{html}" } }
        }
        Part::Reasoning { text, duration_ms } => {
            if !show_reasoning {
                return rsx! {};
            }
            rsx! { Reasoning { text, duration_ms, live } }
        }
        Part::Tool(t) => rsx! { ToolView { session_id: session_id.clone(), tool: t } },
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
                                    let s = app.peek_session(&session_id, |s| s.clone());
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
        Part::Image { .. } => {
            let src = crate::state::image_src(&part);
            let label = match &part {
                Part::Image { source: MediaSource::Path { path }, .. } => short_path(path),
                Part::Image { source: MediaSource::Url { url }, .. } => url.clone(),
                _ => "image".into(),
            };
            rsx! {
                if let Some(src) = src {
                    img { class: "a-img", src: "{src}", onclick: { let s2 = src.clone(); let mut a = app; move |_| a.ui.write().lightbox = Some(s2.clone()) } }
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

fn verb_target(t: &ToolCall) -> (&'static str, String) {
    let running = is_running(t);
    let pick = |a: &'static str, b: &'static str| if running { a } else { b };
    let arg = |keys: &[&str]| keys.iter().find_map(|k| t.input.get(*k).and_then(|v| v.as_str())).map(|v| larrier_core::lsf::first_line(v).to_string()).unwrap_or_default();
    // Tools named by what they do, whatever kind their name suggests.
    let named = match t.name.as_str() {
        "AskUserQuestion" | "ask_user" | "request_user_input" => Some((pick("Asking", "Asked"), question_text(t))),
        "ExitPlanMode" => Some((pick("Proposing plan", "Proposed plan"), String::new())),
        "EnterPlanMode" => Some(("Entered plan mode", String::new())),
        "Skill" => Some((pick("Using skill", "Used skill"), arg(&["skill", "name", "command"]))),
        "SlashCommand" => Some((pick("Running", "Ran"), arg(&["command"]))),
        "ToolSearch" => Some((pick("Loading tools", "Loaded tools"), arg(&["query"]))),
        "SendMessage" => Some((pick("Messaging", "Messaged"), arg(&["to", "recipient"]))),
        "SendUserFile" => Some((pick("Sending", "Sent"), t.input.get("files").and_then(|f| f.as_array()).map(|a| format!("{} file{}", a.len(), if a.len() == 1 { "" } else { "s" })).unwrap_or_default())),
        "PushNotification" => Some((pick("Notifying", "Notified"), arg(&["message"]))),
        "BashOutput" | "TaskOutput" | "AgentOutputTool" => Some((pick("Checking output", "Checked output"), arg(&["bash_id", "task_id", "shell_id", "agentId"]))),
        "KillShell" | "KillBash" | "TaskStop" => Some((pick("Stopping", "Stopped"), arg(&["shell_id", "task_id", "bash_id"]))),
        "Monitor" => Some((pick("Watching", "Watched"), arg(&["description", "command"]))),
        "ScheduleWakeup" => Some((pick("Scheduling", "Scheduled"), arg(&["reason"]))),
        "LSP" => Some((pick("Querying code", "Queried code"), arg(&["operation", "file_path"]))),
        "Workflow" => Some((pick("Running workflow", "Ran workflow"), arg(&["name", "description"]))),
        "ReadNotifications" => Some((pick("Reading notifications", "Read notifications"), String::new())),
        "WebFetch" | "web_fetch" | "fetch" => Some((pick("Fetching", "Fetched"), arg(&["url"]))),
        "WebSearch" | "web_search" | "google_web_search" => Some((pick("Searching web", "Searched web"), arg(&["query"]))),
        "Glob" => Some((pick("Finding", "Found"), arg(&["pattern"]))),
        "Grep" => Some((pick("Searching", "Searched"), arg(&["pattern"]))),
        _ => None,
    };
    if let Some(v) = named {
        return v;
    }
    if t.kind == ToolKind::Mcp {
        // mcp__server__tool → "server › tool" plus its main argument.
        let mut it = t.name.trim_start_matches("mcp__").trim_start_matches("mcp_").splitn(2, "__");
        let server = it.next().unwrap_or("").replace('_', " ");
        let tool = it.next().unwrap_or("").replace('_', " ");
        let main = generic_arg(t);
        let target = if tool.is_empty() { server } else { format!("{server} › {tool}") };
        return (pick("Calling", "Called"), if main.is_empty() { target } else { format!("{target} · {main}") });
    }
    let title = t.title.clone().unwrap_or_default();
    let rest = title.split_once(' ').map(|(_, r)| r.trim().to_string()).unwrap_or_default();
    let verb = match t.kind {
        ToolKind::Read => pick("Reading", "Read"),
        ToolKind::Edit => pick("Editing", "Edited"),
        ToolKind::Write => pick("Writing", "Wrote"),
        ToolKind::Delete => "Deleted",
        ToolKind::Move => "Moved",
        ToolKind::Search => pick("Searching", "Searched"),
        ToolKind::Shell => pick("Running", "Ran"),
        ToolKind::Fetch => pick("Fetching", "Fetched"),
        ToolKind::WebSearch => pick("Searching web", "Searched web"),
        ToolKind::Think => "Thought",
        ToolKind::Task => pick("Running subagent", "Subagent"),
        ToolKind::Todo => "Updated to-dos",
        ToolKind::Mcp | ToolKind::Other => pick("Running", "Used"),
    };
    let target = match t.kind {
        ToolKind::Other => {
            let main = generic_arg(t);
            let name = humanize_tool(&t.name);
            if main.is_empty() { name } else { format!("{name} · {main}") }
        }
        ToolKind::Todo | ToolKind::Think => String::new(),
        _ => rest,
    };
    (verb, target)
}

/// The argument that best describes a call to an unfamiliar tool.
fn generic_arg(t: &ToolCall) -> String {
    const KEYS: [&str; 14] = ["description", "command", "url", "query", "pattern", "file_path", "path", "name", "title", "prompt", "message", "text", "id", "repo"];
    KEYS.iter()
        .find_map(|k| t.input.get(*k).and_then(|v| v.as_str()).filter(|v| !v.trim().is_empty()))
        .map(|v| {
            let l = larrier_core::lsf::first_line(v);
            if l.chars().count() > 80 { l.chars().take(77).collect::<String>() + "…" } else { l.to_string() }
        })
        .unwrap_or_default()
}

/// "ReadNotifications" → "Read notifications", "create_issue" → "Create issue".
fn humanize_tool(name: &str) -> String {
    let mut out = String::new();
    let mut prev_lower = false;
    for c in name.trim_start_matches("mcp__").chars() {
        if c == '_' || c == '-' {
            out.push(' ');
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() && prev_lower {
            out.push(' ');
        }
        prev_lower = c.is_lowercase() || c.is_ascii_digit();
        out.push(c);
    }
    let lower = out.to_lowercase();
    let mut ch = lower.chars();
    match ch.next() {
        Some(f) => f.to_uppercase().collect::<String>() + ch.as_str(),
        None => String::new(),
    }
}

/// First question of an AskUserQuestion call.
fn question_text(t: &ToolCall) -> String {
    t.input
        .get("questions")
        .and_then(|q| q.as_array())
        .and_then(|a| a.first())
        .and_then(|q| q.get("question").and_then(|x| x.as_str()))
        .or_else(|| t.input.get("question").and_then(|x| x.as_str()))
        .map(|s| s.to_string())
        .unwrap_or_default()
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
    // MCP and unfamiliar tools also show what they were called with.
    let show_input = matches!(tool.kind, ToolKind::Mcp | ToolKind::Other) && tool.input.as_object().map(|o| !o.is_empty()).unwrap_or(false);
    let expandable = has_output || show_input;
    let output = clip(tool.output.clone().unwrap_or_default(), 12_000);
    let input = if show_input { clip(serde_json::to_string_pretty(&tool.input).unwrap_or_default(), 6_000) } else { String::new() };
    let dur = tool.duration_ms.filter(|d| *d >= 1000).map(|d| format!("{}s", (d + 500) / 1000));
    rsx! {
        div {
            class: if is_running(&tool) { "step running" } else if failed { "step failed" } else { "step" },
            onclick: move |_| if expandable { open.toggle() },
            span { class: "verb", "{verb}" }
            span { class: "target", "{target}" }
            if let Some(d) = dur { span { class: "dur", "{d}" } }
            if expandable {
                span { class: "chev", if open() { Icon { i: I::ChevronDown, size: 12 } } else { Icon { i: I::ChevronRight, size: 12 } } }
            }
        }
        ToolImages { images: tool.images.clone() }
        if open() {
            div { class: "tool-card tool-io",
                if show_input {
                    div { class: "io-label", "Input" }
                    pre { class: "io", "{input}" }
                }
                if has_output {
                    if show_input { div { class: "io-label", "Output" } }
                    pre { class: "io", "{output}" }
                }
            }
        }
    }
}

/// Images a tool returned, as thumbnails that open full size.
#[component]
fn ToolImages(images: Vec<Part>) -> Element {
    let mut app = use_app();
    let srcs: Vec<String> = images.iter().filter_map(crate::state::image_src).collect();
    if srcs.is_empty() {
        return rsx! {};
    }
    rsx! {
        div { class: "tool-images",
            for (i, src) in srcs.into_iter().enumerate() {
                img { key: "{i}", src: "{src}", onclick: move |e| { e.stop_propagation(); app.ui.write().lightbox = Some(src.clone()); } }
            }
        }
    }
}

/// The agent's to-do list (TodoWrite, update_plan, write_todos).
#[component]
fn TodoCard(tool: ToolCall) -> Element {
    // Claude: {todos: [{content, status, activeForm}]}; Codex: {plan: [{step, status}]}.
    let list = tool.input.get("todos").or_else(|| tool.input.get("plan")).and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let items: Vec<(String, String)> = list
        .iter()
        .map(|e| {
            let text = ["content", "step", "text", "title"].iter().find_map(|k| e.get(*k).and_then(|v| v.as_str())).unwrap_or("").to_string();
            let status = e.get("status").and_then(|v| v.as_str()).unwrap_or("pending").to_string();
            (text, status)
        })
        .filter(|(t, _)| !t.is_empty())
        .collect();
    if items.is_empty() {
        return rsx! { Step { tool } };
    }
    let done = items.iter().filter(|(_, s)| s == "completed").count();
    rsx! {
        div { class: "plan-card",
            div { class: "plan-head", "{items.len()} To-dos · {done} done" }
            for (i, (text, status)) in items.into_iter().enumerate() {
                div {
                    key: "{i}",
                    class: match status.as_str() {
                        "completed" => "plan-entry completed",
                        "in_progress" => "plan-entry in_progress",
                        _ => "plan-entry",
                    },
                    span { class: "box",
                        if status == "completed" { Icon { i: I::Check, size: 9, stroke: 3.0 } }
                    }
                    span { class: "txt", "{text}" }
                }
            }
        }
    }
}

/// A plan the agent proposed (ExitPlanMode).
#[component]
fn PlanProposal(tool: ToolCall) -> Element {
    let plan = tool.input.get("plan").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if plan.trim().is_empty() {
        return rsx! { Step { tool } };
    }
    let html = markdown::to_html(&plan);
    rsx! {
        div { class: "tool-card plan-proposal",
            div { class: "tool-card-head", span { class: "ic", Icon { i: I::List, size: 13 } } span { class: "name", "Plan" } }
            div { class: "a-text md", dangerous_inner_html: "{html}" }
        }
    }
}

/// Questions the agent asked (AskUserQuestion) with the answers given.
#[component]
fn QuestionView(tool: ToolCall) -> Element {
    let qs: Vec<(String, Vec<String>)> = tool
        .input
        .get("questions")
        .and_then(|q| q.as_array())
        .map(|a| {
            a.iter()
                .map(|q| {
                    let text = q.get("question").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    let opts = q.get("options").and_then(|o| o.as_array()).map(|o| o.iter().filter_map(|x| x.get("label").and_then(|l| l.as_str()).or_else(|| x.as_str())).map(String::from).collect()).unwrap_or_default();
                    (text, opts)
                })
                .collect()
        })
        .unwrap_or_default();
    if qs.is_empty() {
        return rsx! { Step { tool } };
    }
    let answer = tool.output.clone().unwrap_or_default();
    rsx! {
        div { class: "question-card answered",
            for (i, (q, opts)) in qs.into_iter().enumerate() {
                div { key: "{i}",
                    div { class: "q-title", "{q}" }
                    div { class: "q-opts",
                        for (j, o) in opts.into_iter().enumerate() {
                            span { key: "{j}", class: if answer.contains(&o) { "q-chip on" } else { "q-chip" }, "{o}" }
                        }
                    }
                }
            }
            if !answer.trim().is_empty() {
                div { class: "q-answer", "{larrier_core::lsf::first_line(&answer)}" }
            }
        }
    }
}

#[component]
fn ToolView(session_id: String, tool: ToolCall) -> Element {
    if tool.kind == ToolKind::Task || !tool.subagent.is_empty() {
        return rsx! { super::tasks::SubagentCard { session_id, tool } };
    }
    match tool.name.as_str() {
        "ExitPlanMode" => return rsx! { PlanProposal { tool } },
        "AskUserQuestion" => return rsx! { QuestionView { tool } },
        "BashOutput" | "TaskOutput" | "KillShell" | "KillBash" | "TaskStop" | "Monitor" => return rsx! { Step { tool } },
        _ => {}
    }
    if tool.kind == ToolKind::Todo {
        return rsx! { TodoCard { tool } };
    }
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
