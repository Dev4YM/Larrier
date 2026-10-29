//! Subagents and background tasks.
//!
//! A subagent call (Task / Agent / spawn_agent) shows in the transcript as a
//! one-line row; clicking it opens the subagent's transcript in its own tab
//! ([`SubagentTranscript`]). Background shells, monitors and subagents of the
//! current turn are listed by the pill above the composer ([`BackgroundTasks`]).

use std::time::Duration;

use chrono::{DateTime, Utc};
use dioxus::prelude::*;
use larrier_core::{Message, Part, Role, ToolCall, ToolStatus};

use crate::icons::{Icon, I};
use crate::state::{use_app, View};

const CSS: &str = r#"
.tk-sub { display: flex; align-items: center; gap: 6px; width: 100%; min-width: 0; height: 30px; padding: 0 8px; margin: 2px 0 6px;
  border: 1px solid var(--border); border-radius: 8px; background: var(--card); font-size: 12px; color: var(--fg-2); text-align: left; cursor: pointer; }
.tk-sub:hover { background: var(--hover); color: var(--fg); }
.tk-sub .tk-name { color: var(--fg); font-weight: 500; white-space: nowrap; flex-shrink: 0; max-width: 40%; overflow: hidden; text-overflow: ellipsis; }
.tk-sub .tk-desc { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.tk-sub .tk-meta { flex-shrink: 0; color: var(--fg-3); white-space: nowrap; font-variant-numeric: tabular-nums; }
.tk-sub .tk-go { display: inline-flex; color: var(--fg-3); flex-shrink: 0; }
.tk-sub.failed .tk-name { color: var(--red); }
.tk-st { display: inline-flex; align-items: center; justify-content: center; width: 14px; height: 14px; flex-shrink: 0; color: var(--fg-2); }
.tk-st.ok { color: var(--green); }
.tk-st.err { color: var(--red); }
.tk-st.muted { color: var(--fg-3); }
.tk-ic { display: inline-flex; color: var(--fg-3); flex-shrink: 0; }
.tk-badge { font-size: 10px; line-height: 16px; padding: 0 5px; border-radius: 4px; background: var(--chip); color: var(--fg-2); flex-shrink: 0; }
@media (max-width: 560px) {
  .tk-sub .tk-desc, .tk-sub .tk-badge { display: none; }
  .tk-sub .tk-name { flex: 1; max-width: none; }
}

.tk-tx { display: flex; flex-direction: column; flex: 1; min-height: 0; min-width: 0; height: 100%; }
.tk-tx-head { display: flex; align-items: center; gap: 8px; padding: 8px 16px; border-bottom: 1px solid var(--border); font-size: 12px; min-width: 0; flex-wrap: wrap; }
.tk-back { display: inline-flex; align-items: center; gap: 4px; height: 24px; padding: 0 6px; border-radius: 6px; color: var(--fg-2); max-width: 220px; min-width: 0; }
.tk-back:hover { background: var(--hover); color: var(--fg); }
.tk-back span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.tk-tx-title { display: flex; align-items: center; gap: 6px; min-width: 0; flex: 1; }
.tk-tx-title .tk-name { font-weight: 500; color: var(--fg); white-space: nowrap; }
.tk-tx-title .tk-desc { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--fg-2); min-width: 0; }
.tk-tx-meta { color: var(--fg-3); white-space: nowrap; font-variant-numeric: tabular-nums; }
.tk-tx-body { flex: 1; min-height: 0; overflow-y: auto; overflow-x: hidden; }
.tk-tx-body .transcript-inner { padding-top: 12px; }
.tk-user { background: var(--card); border: 1px solid var(--border); border-radius: 8px; padding: 6px 8px; margin: 0 0 10px;
  font-size: 13px; line-height: 18px; white-space: pre-wrap; word-wrap: break-word; max-height: 220px; overflow: auto; }
.tk-user .tk-role { display: block; font-size: 11px; color: var(--fg-3); margin-bottom: 2px; }
.tk-empty { padding: 24px 4px; color: var(--fg-3); font-size: 12px; }
@media (max-width: 560px) {
  .tk-tx-head { padding: 6px 10px; }
  .tk-tx-meta { width: 100%; }
}

.tk-bg { display: flex; min-width: 0; padding: 0 0 6px; }
.tk-pill { display: inline-flex; align-items: center; gap: 6px; height: 24px; padding: 0 9px; border-radius: 12px; min-width: 0; max-width: 100%;
  background: var(--card); border: 1px solid var(--border); font-size: 12px; color: var(--fg-2); }
.tk-pill:hover, .tk-pill.on { background: var(--hover); color: var(--fg); }
.tk-pill .lb { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.menu.tk-pop { width: min(400px, calc(100vw - 32px)); max-width: none; max-height: min(380px, 60vh); }
.tk-row { display: flex; align-items: flex-start; gap: 8px; padding: 6px 8px; border-radius: 6px; color: var(--fg); min-width: 0; }
.tk-row.link { cursor: pointer; }
.tk-row.link:hover { background: var(--hover); }
.tk-row.done { opacity: .55; }
.tk-row > .tk-ic, .tk-row > .tk-st { margin-top: 1px; }
.tk-row-main { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
.tk-row-top { display: flex; align-items: center; gap: 6px; min-width: 0; }
.tk-row-title { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 12px; }
.tk-row-when { color: var(--fg-3); font-size: 11px; white-space: nowrap; font-variant-numeric: tabular-nums; }
.tk-tail { font-family: var(--mono); font-size: 10.5px; line-height: 15px; color: var(--fg-3); white-space: pre; overflow: hidden; text-overflow: ellipsis; }
"#;

/// Compact row for a subagent call; opens its transcript in a tab.
#[component]
pub fn SubagentCard(session_id: String, tool: ToolCall) -> Element {
    let app = use_app();
    let label = agent_label(&tool);
    let desc = agent_desc(&tool, &label);
    let steps = tool.subagent_steps();
    let mut meta = Vec::new();
    if steps > 0 {
        meta.push(format!("{steps} tool{}", if steps == 1 { "" } else { "s" }));
    }
    if let Some(ms) = run_ms(&tool, Utc::now()) {
        meta.push(fmt_ms(ms));
    }
    let meta = meta.join(" · ");
    let failed = tool.is_error || tool.status == ToolStatus::Failed;
    let tid = tool.id.clone();
    rsx! {
        style { {CSS} }
        div {
            class: if failed { "tk-sub failed" } else { "tk-sub" },
            role: "button",
            title: "Open subagent transcript",
            onclick: move |e| {
                e.stop_propagation();
                app.open_subagent(&session_id, &tid);
            },
            StatusIcon { status: tool.status, error: tool.is_error }
            span { class: "tk-ic", Icon { i: I::Bot, size: 13 } }
            span { class: "tk-name", "{label}" }
            span { class: "tk-desc", "{desc}" }
            if tool.background { span { class: "tk-badge", "background" } }
            if !meta.is_empty() { span { class: "tk-meta", "{meta}" } }
            span { class: "tk-go", Icon { i: I::ChevronRight, size: 12 } }
        }
    }
}

/// Read-only transcript of one subagent, shown in its own tab.
#[component]
pub fn SubagentTranscript(session_id: String, tool_id: String) -> Element {
    let app = use_app();
    let show_reasoning = app.config.read().settings.show_reasoning;
    let found = app.with_session(&session_id, |s| (s.title.clone(), find_tool(&s.messages, &tool_id).cloned()));
    let Some((parent_title, tool)) = found else {
        return rsx! { div { class: "empty-state", "Loading…" } };
    };
    let Some(tool) = tool else {
        return rsx! { div { class: "empty-state", "This subagent is no longer in the session" } };
    };
    let label = agent_label(&tool);
    let desc = agent_desc(&tool, &label);
    let prompt = input_str(&tool, &["prompt", "message"]).map(|p| p.to_string());
    let running = tool.is_active();
    let failed = tool.is_error || tool.status == ToolStatus::Failed;
    let steps = tool.subagent_steps();
    let mut meta = vec![format!("{steps} tool{}", if steps == 1 { "" } else { "s" })];
    if let Some(ms) = run_ms(&tool, Utc::now()) {
        meta.push(fmt_ms(ms));
    }
    if tool.background {
        meta.push("background".into());
    }
    let meta = meta.join(" · ");
    let msgs = tool.subagent.clone();
    let n = msgs.len();
    let output = tool.output.clone().filter(|o| !o.trim().is_empty());
    let result_html = output.as_deref().filter(|_| n == 0 && !failed).map(crate::markdown::to_html);
    let sid_back = session_id.clone();
    rsx! {
        style { {CSS} }
        div { class: "tk-tx",
            div { class: "tk-tx-head",
                button {
                    class: "tk-back",
                    title: "Back to the parent chat",
                    onclick: move |_| app.navigate(View::Agent(sid_back.clone())),
                    Icon { i: I::ArrowLeft, size: 13 }
                    span { "{parent_title}" }
                }
                div { class: "tk-tx-title",
                    StatusIcon { status: tool.status, error: tool.is_error }
                    span { class: "tk-ic", Icon { i: I::Bot, size: 13 } }
                    span { class: "tk-name", "{label}" }
                    if !desc.is_empty() { span { class: "tk-desc", "{desc}" } }
                }
                span { class: "tk-tx-meta", "{meta}" }
            }
            div { class: "tk-tx-body",
                div { class: "transcript-inner selectable",
                    if let Some(p) = prompt {
                        div { class: "tk-user", span { class: "tk-role", "Task" } "{p}" }
                    }
                    for (i, m) in msgs.into_iter().enumerate() {
                        if m.role == Role::User {
                            div { key: "{m.id}", class: "tk-user", "{m.text()}" }
                        } else {
                            super::transcript::AssistantMessage {
                                key: "{m.id}",
                                session_id: session_id.clone(),
                                msg: m.clone(),
                                streaming: running && i + 1 == n,
                                show_reasoning,
                            }
                        }
                    }
                    if n == 0 {
                        if running {
                            div { class: "thinking-row", span { class: "shimmer", "Starting subagent" } }
                        } else if let Some(html) = result_html {
                            div { class: "a-text md", dir: "auto", dangerous_inner_html: "{html}" }
                        } else if !failed {
                            div { class: "tk-empty", "No transcript was recorded for this subagent." }
                        }
                    }
                    if failed {
                        if let Some(o) = output { div { class: "err-card", "{o}" } }
                    }
                }
            }
        }
    }
}

/// "N background tasks" pill above the composer, with a popover listing the
/// current turn's background shells, monitors and subagents.
#[component]
pub fn BackgroundTasks(session_id: String) -> Element {
    let app = use_app();
    let mut open = use_signal(|| false);
    let mut now = use_signal(Utc::now);
    // Tick elapsed times only while the list is showing.
    use_future(move || async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if *open.peek() {
                now.set(Utc::now());
            }
        }
    });
    let rows = app.with_session(&session_id, |s| task_rows(&s.messages)).unwrap_or_default();
    let active = rows.iter().filter(|r| r.active).count();
    // Keep the pill while the list is open so it closes on the user's terms.
    if active == 0 && !open() {
        return rsx! {};
    }
    let now = now();
    let label = match active {
        0 => "Background tasks finished".to_string(),
        1 => "1 background task".to_string(),
        n => format!("{n} background tasks"),
    };
    rsx! {
        style { {CSS} }
        div { class: "tk-bg",
            div { class: "menu-anchor",
                button {
                    class: if open() { "tk-pill on" } else { "tk-pill" },
                    title: "Show background tasks",
                    onclick: move |_| open.toggle(),
                    if active > 0 {
                        span { class: "sunburst tk-st", Icon { i: I::Loader, size: 12 } }
                    } else {
                        span { class: "tk-st ok", Icon { i: I::CircleCheck, size: 12 } }
                    }
                    span { class: "lb", "{label}" }
                    Icon { i: if open() { I::ChevronDown } else { I::ChevronRight }, size: 11 }
                }
                if open() {
                    div { class: "menu-backdrop", onclick: move |_| open.set(false) }
                    div { class: "menu up tk-pop",
                        div { class: "menu-label", "Background tasks" }
                        for r in rows {
                            TaskRowView { key: "{r.id}", session_id: session_id.clone(), row: r.clone(), now, on_open: move |_| open.set(false) }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn TaskRowView(session_id: String, row: TaskRow, now: DateTime<Utc>, on_open: EventHandler<()>) -> Element {
    let app = use_app();
    let when = match (row.active, row.started) {
        (true, Some(s)) => fmt_ms((now - s).num_milliseconds().max(0) as u64),
        (true, None) => "running".into(),
        (false, _) => {
            let st = status_word(row.status, row.is_error);
            match row.ms {
                Some(ms) => format!("{st} · {}", fmt_ms(ms)),
                None => st.to_string(),
            }
        }
    };
    let icon = match row.kind {
        TaskKind::Subagent => I::Bot,
        TaskKind::Monitor => I::Monitor,
        TaskKind::Shell => I::Terminal,
    };
    let link = row.kind == TaskKind::Subagent;
    let class = match (link, row.active) {
        (true, true) => "tk-row link",
        (true, false) => "tk-row link done",
        (false, true) => "tk-row",
        (false, false) => "tk-row done",
    };
    let tid = row.id.clone();
    rsx! {
        div {
            class: "{class}",
            onclick: move |_| {
                if link {
                    on_open.call(());
                    app.open_subagent(&session_id, &tid);
                }
            },
            span { class: "tk-ic", Icon { i: icon, size: 13 } }
            div { class: "tk-row-main",
                div { class: "tk-row-top",
                    span { class: "tk-row-title", title: "{row.title}", "{row.title}" }
                    StatusIcon { status: row.status, error: row.is_error }
                    span { class: "tk-row-when", "{when}" }
                }
                for (i, l) in row.tail.iter().enumerate() {
                    div { key: "{i}", class: "tk-tail", "{l}" }
                }
            }
        }
    }
}

#[component]
fn StatusIcon(status: ToolStatus, error: bool) -> Element {
    if error || status == ToolStatus::Failed {
        return rsx! { span { class: "tk-st err", title: "Failed", Icon { i: I::Alert, size: 13 } } };
    }
    match status {
        ToolStatus::Running | ToolStatus::Pending | ToolStatus::Approval => rsx! {
            span { class: "sunburst tk-st", title: "Running", Icon { i: I::Loader, size: 13 } }
        },
        ToolStatus::Cancelled => rsx! { span { class: "tk-st muted", title: "Stopped", Icon { i: I::Stop, size: 10 } } },
        _ => rsx! { span { class: "tk-st ok", title: "Done", Icon { i: I::CircleCheck, size: 13 } } },
    }
}

#[derive(Clone, Copy, PartialEq)]
enum TaskKind {
    Subagent,
    Shell,
    Monitor,
}

#[derive(Clone, PartialEq)]
struct TaskRow {
    id: String,
    kind: TaskKind,
    title: String,
    status: ToolStatus,
    is_error: bool,
    active: bool,
    started: Option<DateTime<Utc>>,
    ms: Option<u64>,
    tail: Vec<String>,
}

/// Background work and subagents of the latest turn (recursive).
fn task_rows(messages: &[Message]) -> Vec<TaskRow> {
    let start = messages.iter().rposition(|m| m.role == Role::User).unwrap_or(0);
    let mut out = Vec::new();
    collect_rows(&messages[start..], &mut out, 0);
    out
}

fn collect_rows(messages: &[Message], out: &mut Vec<TaskRow>, depth: usize) {
    for m in messages {
        for t in m.tool_calls() {
            if t.background || t.is_subagent() {
                out.push(task_row(t));
            }
            if depth < 4 {
                collect_rows(&t.subagent, out, depth + 1);
            }
        }
    }
}

fn task_row(t: &ToolCall) -> TaskRow {
    let kind = if t.is_subagent() {
        TaskKind::Subagent
    } else if t.name == "Monitor" {
        TaskKind::Monitor
    } else {
        TaskKind::Shell
    };
    let title = match kind {
        TaskKind::Subagent => {
            let label = agent_label(t);
            let desc = agent_desc(t, &label);
            if desc.is_empty() { label } else { format!("{label} · {desc}") }
        }
        _ => input_str(t, &["command", "description"])
            .map(|c| c.lines().next().unwrap_or("").to_string())
            .or_else(|| t.title.clone())
            .unwrap_or_else(|| t.name.clone()),
    };
    TaskRow {
        id: t.id.clone(),
        kind,
        title,
        status: t.status,
        is_error: t.is_error,
        active: t.is_active(),
        started: t.started_at,
        ms: run_ms(t, Utc::now()),
        tail: tail_lines(t),
    }
}

/// Last three lines of output, or of a subagent's latest steps.
fn tail_lines(t: &ToolCall) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    if !t.subagent.is_empty() {
        'outer: for m in t.subagent.iter().rev() {
            for p in m.parts.iter().rev() {
                let line = match p {
                    Part::Tool(x) => x.title.clone().unwrap_or_else(|| x.name.clone()),
                    Part::Text { text } => text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string(),
                    _ => continue,
                };
                if !line.is_empty() {
                    lines.push(line);
                }
                if lines.len() == 3 {
                    break 'outer;
                }
            }
        }
    } else if let Some(o) = &t.output {
        lines = o.lines().rev().filter(|l| !l.trim().is_empty()).take(3).map(|l| l.trim_end().to_string()).collect();
    }
    lines.reverse();
    lines.into_iter().map(|l| if l.chars().count() > 160 { l.chars().take(159).collect::<String>() + "…" } else { l }).collect()
}

/// A tool call by id anywhere in the transcript, subagents included.
fn find_tool<'a>(messages: &'a [Message], id: &str) -> Option<&'a ToolCall> {
    for m in messages.iter().rev() {
        for t in m.tool_calls() {
            if t.id == id {
                return Some(t);
            }
            if let Some(found) = find_tool(&t.subagent, id) {
                return Some(found);
            }
        }
    }
    None
}

fn input_str<'a>(t: &'a ToolCall, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| t.input.get(*k).and_then(|v| v.as_str()).map(str::trim).filter(|v| !v.is_empty()))
}

fn agent_label(t: &ToolCall) -> String {
    t.agent_name.clone().filter(|a| !a.trim().is_empty()).unwrap_or_else(|| "Subagent".into())
}

/// One-line description that doesn't repeat the label.
fn agent_desc(t: &ToolCall, label: &str) -> String {
    let first = |s: &str| s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
    let d = input_str(t, &["description", "task_name"]).map(first).filter(|d| d != label);
    d.or_else(|| input_str(t, &["prompt", "message"]).map(first))
        .or_else(|| t.title.as_deref().map(|ti| ti.trim_start_matches("Subagent").trim().to_string()))
        .unwrap_or_default()
}

/// Elapsed time while running, otherwise the recorded (or derived) run time.
fn run_ms(t: &ToolCall, now: DateTime<Utc>) -> Option<u64> {
    if t.is_active() {
        return t.started_at.map(|s| (now - s).num_milliseconds().max(0) as u64);
    }
    t.duration_ms.or_else(|| {
        let start = t.started_at?;
        let end = t.subagent.iter().map(|m| m.created_at).max()?;
        let ms = (end - start).num_milliseconds();
        (ms > 0).then_some(ms as u64)
    })
}

fn fmt_ms(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0 => "<1s".into(),
        1..=59 => format!("{s}s"),
        60..=3599 => format!("{}m {:02}s", s / 60, s % 60),
        _ => format!("{}h {:02}m", s / 3600, (s % 3600) / 60),
    }
}

fn status_word(status: ToolStatus, error: bool) -> &'static str {
    if error || status == ToolStatus::Failed {
        return "Failed";
    }
    match status {
        ToolStatus::Cancelled => "Stopped",
        ToolStatus::Running | ToolStatus::Pending | ToolStatus::Approval => "Running",
        _ => "Done",
    }
}
