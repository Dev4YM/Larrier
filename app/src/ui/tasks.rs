//! Subagents and background tasks.
//!
//! A subagent call (Task / Agent / spawn_agent) shows in the transcript as a
//! one-line row; clicking it opens the subagent's transcript in its own tab
//! ([`SubagentTranscript`]). While the session works, its running subagents
//! and terminals show as chips above the composer that expand into a card
//! ([`BackgroundChips`]); the side panel lists every one ([`TasksPanel`]).

use std::time::Duration;

use chrono::{DateTime, Utc};
use dioxus::prelude::*;
use larrier_core::{Message, Part, Role, ToolCall, ToolKind, ToolStatus};

use crate::icons::{Icon, I};
use crate::state::{use_app, App, RightTab, View};

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
.tk-tx-top { flex-shrink: 0; padding: 10px 0 14px; border-bottom: 1px solid var(--border); }
.tk-tx-wrap { max-width: var(--tw, 860px); margin: 0 auto; padding: 0 28px; display: flex; flex-direction: column; gap: 10px; min-width: 0; }
.tk-back { align-self: flex-start; display: inline-flex; align-items: center; gap: 5px; height: 26px; padding: 0 8px; margin-left: -8px; border-radius: 7px;
  color: var(--fg-3); font-size: 12.5px; max-width: 100%; min-width: 0; }
.tk-back:hover { background: var(--hover); color: var(--fg); }
.tk-back span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.tk-tx-title { display: flex; align-items: center; gap: 8px; min-width: 0; font-size: 15px; line-height: 22px; }
.tk-tx-title .tk-ic { color: var(--fg-2); }
.tk-tx-title .tk-name { font-weight: 600; color: var(--fg); white-space: nowrap; }
.tk-tx-title .tk-desc { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--fg-2); min-width: 0; }
.tk-facts { display: flex; flex-wrap: wrap; gap: 6px; }
.tk-fact { display: inline-flex; align-items: center; gap: 6px; height: 26px; padding: 0 10px; border-radius: 7px; border: 1px solid var(--border);
  background: var(--card); font-size: 12px; white-space: nowrap; max-width: 100%; min-width: 0; }
.tk-fact .tk-ic { color: var(--fg-3); }
.tk-fact .k { color: var(--fg-3); }
.tk-fact .v { color: var(--fg); font-variant-numeric: tabular-nums; overflow: hidden; text-overflow: ellipsis; }
.tk-tx-body { flex: 1; min-height: 0; overflow-y: auto; overflow-x: hidden; }
.tk-tx-body .transcript-inner { padding-top: 18px; }
.tk-user { background: var(--card); border: 1px solid var(--border); border-radius: 10px; padding: 10px 12px; margin: 0 0 14px;
  font-size: 13px; line-height: 19px; white-space: pre-wrap; word-wrap: break-word; max-height: 240px; overflow: auto; }
.tk-user .tk-role { display: block; font-size: 11.5px; color: var(--fg-3); margin-bottom: 4px; }
.tk-empty { padding: 24px 4px; color: var(--fg-3); font-size: 12px; }
@media (max-width: 560px) {
  .tk-tx-wrap { padding: 0 14px; }
}

.above-composer { position: relative; }
.tk-chip { height: 28px; padding: 0 12px 0 10px; gap: 7px; font-size: 12.5px; color: var(--fg-2); transition: background .15s, color .15s; }
.tk-chip:hover { background: var(--hover); color: var(--fg); }
.tk-chip.on { background: var(--hover); color: var(--fg); }
.tk-chip .n { font-variant-numeric: tabular-nums; }
.tk-card { position: absolute; left: 0; right: 0; bottom: calc(100% + 4px); z-index: 40; background: var(--card-2); border-radius: 14px;
  box-shadow: var(--shadow-pop); padding: 6px; transform-origin: 50% 100%; animation: tk-card-in .2s cubic-bezier(.2, .8, .2, 1); }
@keyframes tk-card-in { from { opacity: 0; transform: translateY(6px) scale(.985); } }
.app.no-motion .tk-card { animation: none; }
.tk-card-head { display: flex; align-items: center; gap: 8px; height: 38px; padding: 0 4px 0 12px; font-size: 13px; color: var(--fg-3); }
.tk-card-head .tk-ic { color: var(--fg-3); }
.tk-card-head .ttl { color: var(--fg); font-weight: 600; }
.tk-card-head .grow { flex: 1; }
.tk-stop-all { height: 26px; padding: 0 8px; border-radius: 6px; color: var(--fg-2); font-size: 12.5px; }
.tk-stop-all:hover { background: var(--hover); color: var(--red); }
.tk-card-list { max-height: min(304px, 42vh); overflow-y: auto; display: flex; flex-direction: column; gap: 2px; padding-top: 6px; border-top: 1px solid var(--border); }
.tk-item { flex: none; box-sizing: border-box; display: flex; align-items: center; gap: 10px; height: 38px; padding: 0 12px; border-radius: 9px;
  font-size: 13px; line-height: 20px; color: var(--fg); cursor: pointer; min-width: 0; transition: background .12s; }
.tk-item:hover { background: var(--hover); }
.tk-item > .lead { display: inline-flex; align-items: center; justify-content: center; width: 16px; flex: none; color: var(--fg-3); }
.tk-item .ttl { font-weight: 500; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; min-width: 0; flex: 0 1 auto; max-width: 48%; }
.tk-item .act { flex: 1; min-width: 0; color: var(--fg-3); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.tk-item .when { color: var(--fg-3); font-size: 12px; font-variant-numeric: tabular-nums; flex: none; min-width: 34px; text-align: right; }
.tk-item .go { display: inline-flex; flex: none; color: var(--fg-3); opacity: 0; transition: opacity .12s; }
.tk-item:hover .go { opacity: 1; }
.tk-card.terms .tk-item .ttl { flex: 1; max-width: none; font-family: var(--mono); font-size: 12.5px; font-weight: 400; }

.tk-panel { flex: 1; min-height: 0; overflow-y: auto; padding: 6px 10px 16px; display: flex; flex-direction: column; }
.tk-panel-hint { max-width: 260px; color: var(--fg-3); }
.tk-group { display: flex; flex-direction: column; }
.tk-group-head { display: flex; align-items: center; gap: 6px; height: 30px; padding: 0 8px; margin-top: 6px; border-radius: 7px;
  width: 100%; text-align: left; font-size: 12px; color: var(--fg-3); }
.tk-group-head:hover { background: var(--hover); color: var(--fg-2); }
.tk-group-head .chev { display: inline-flex; transition: transform .18s ease; }
.tk-group.closed .tk-group-head .chev { transform: rotate(-90deg); }
.tk-group-head .n { font-variant-numeric: tabular-nums; padding: 0 6px; line-height: 17px; border-radius: 9px; background: var(--chip); color: var(--fg-2); font-size: 11px; }
.tk-group-body { display: grid; grid-template-rows: 1fr; transition: grid-template-rows .22s cubic-bezier(.2, .8, .2, 1), opacity .18s; }
.tk-group.closed .tk-group-body { grid-template-rows: 0fr; opacity: 0; pointer-events: none; }
.tk-group-inner { min-height: 0; overflow: hidden; display: flex; flex-direction: column; gap: 2px; padding-top: 2px; }
.app.no-motion .tk-group-body, .app.no-motion .tk-group-head .chev { transition: none; }
.tk-row { flex: none; box-sizing: border-box; height: 52px; display: flex; align-items: center; gap: 10px; padding: 0 10px; border-radius: 9px;
  color: var(--fg); min-width: 0; cursor: pointer; transition: background .12s; }
.tk-row:hover { background: var(--hover); }
.tk-row > .tk-lead { display: inline-flex; align-items: center; justify-content: center; width: 16px; flex: none; color: var(--fg-3); }
.tk-row-main { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
.tk-row-top { display: flex; align-items: center; gap: 6px; min-width: 0; height: 19px; }
.tk-row-title { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 13px; }
.tk-row.term .tk-row-title { font-family: var(--mono); font-size: 12.5px; }
.tk-row.done .tk-row-title { color: var(--fg-2); }
.tk-row-when { color: var(--fg-3); font-size: 11.5px; white-space: nowrap; font-variant-numeric: tabular-nums; flex: none; }
.tk-row-sub { height: 17px; font-size: 12px; line-height: 17px; color: var(--fg-3); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.tk-row.term .tk-row-sub.out { font-family: var(--mono); font-size: 11.5px; }
"#;

/// Six dots (a braille ⠿) lit one after another: background work running.
#[component]
pub fn Dots(#[props(default = 12)] size: u32) -> Element {
    // Clockwise around the 2×3 grid, so the light travels in a loop.
    const AT: [(u8, u8); 6] = [(2, 2), (6, 2), (6, 6), (6, 10), (2, 10), (2, 6)];
    let w = size * 2 / 3;
    let dots = AT.iter().enumerate().map(|(k, (x, y))| (*x, *y, format!("animation-delay: {:.1}s", k as f32 * 0.2 - 1.2)));
    rsx! {
        svg { class: "tk-dots", width: "{w}", height: "{size}", view_box: "0 0 8 12", role: "img", "aria-label": "Running",
            for (x, y, delay) in dots {
                circle { key: "{x}-{y}", cx: "{x}", cy: "{y}", r: "1.35", style: "{delay}" }
            }
        }
    }
}

/// Compact row for a subagent call; opens its transcript in a tab.
#[component]
pub fn SubagentCard(session_id: String, tool: ToolCall) -> Element {
    let app = use_app();
    let label = agent_label(&tool);
    let desc = agent_desc(&tool, &label);
    let info = sub_info(&tool, Utc::now());
    let mut meta = Vec::new();
    if let Some(m) = &info.model {
        meta.push(m.clone());
    }
    if info.tools > 0 {
        meta.push(format!("{} tool{}", info.tools, if info.tools == 1 { "" } else { "s" }));
    }
    if let Some(ms) = info.ms {
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
            span { class: "tk-ic", Icon { i: I::Orbit, size: 13 } }
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
    let info = sub_info(&tool, Utc::now());
    let msgs = tool.subagent.clone();
    let n = msgs.len();
    let output = tool.output.clone().filter(|o| !o.trim().is_empty());
    let result_html = output.as_deref().filter(|_| n == 0 && !failed).map(crate::markdown::to_html);
    let sid_back = session_id.clone();
    let tools = format!("{}", info.tools);
    rsx! {
        style { {CSS} }
        div { class: "tk-tx",
            div { class: "tk-tx-top",
                div { class: "tk-tx-wrap",
                    button {
                        class: "tk-back",
                        title: "Back to the parent chat",
                        onclick: move |_| app.navigate(View::Agent(sid_back.clone())),
                        Icon { i: I::ArrowLeft, size: 13 }
                        span { "{parent_title}" }
                    }
                    div { class: "tk-tx-title",
                        StatusIcon { status: tool.status, error: tool.is_error }
                        span { class: "tk-ic", Icon { i: I::Orbit, size: 15 } }
                        span { class: "tk-name", "{label}" }
                        if !desc.is_empty() { span { class: "tk-desc", "{desc}" } }
                    }
                    div { class: "tk-facts",
                        if let Some(m) = info.model.clone() {
                            Fact { icon: I::Cpu, k: "Model", v: m }
                        }
                        if let Some(e) = info.effort.clone() {
                            Fact { icon: I::Brain, k: "Reasoning", v: e }
                        }
                        Fact { icon: I::Layers, k: "Tools", v: tools }
                        if let Some(c) = info.context {
                            Fact { icon: I::Gauge, k: "Context", v: larrier_core::models::tokens_short(c) }
                        }
                        span { class: "tk-fact", title: "Run time",
                            span { class: "tk-ic", Icon { i: I::Timer, size: 12 } }
                            span { class: "k", "Time" }
                            Elapsed { class: "v", active: running, started: tool.started_at, ms: info.ms }
                        }
                        Fact { icon: I::CircleDot, k: "Status", v: status_word(tool.status, tool.is_error).to_string() }
                        if tool.background {
                            Fact { icon: I::Workflow, k: "Runs", v: "In background".to_string() }
                        }
                    }
                }
            }
            div { class: "tk-tx-body",
                div { class: "transcript-inner selectable",
                    if let Some(p) = prompt {
                        div { class: "tk-user", span { class: "tk-role", "Task" } "{p}" }
                    }
                    for (i, m) in msgs.into_iter().enumerate() {
                        if m.role == Role::User {
                            div { key: "{i}-{m.id}", class: "tk-user", "{m.text()}" }
                        } else {
                            super::transcript::AssistantMessage {
                                key: "{i}-{m.id}",
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

#[component]
fn Fact(icon: I, k: &'static str, v: String) -> Element {
    rsx! {
        span { class: "tk-fact", title: "{k}: {v}",
            span { class: "tk-ic", Icon { i: icon, size: 12 } }
            span { class: "k", "{k}" }
            span { class: "v", "{v}" }
        }
    }
}

/// Chips above the composer while the session's subagents ("Working 2") or
/// terminals ("1 Terminal") run. Each opens its own card listing them.
#[component]
pub fn BackgroundChips(session_id: String) -> Element {
    let app = use_app();
    let mut open = use_signal(|| None::<TaskKind>);
    let rows = use_memo(use_reactive!(|session_id| {
        if !app.is_active(&session_id) {
            return Vec::new();
        }
        app.with_session(&session_id, |s| live_rows(&s.messages)).unwrap_or_default()
    }));
    let rows = rows();
    if rows.is_empty() {
        return rsx! {};
    }
    let (agents, terms): (Vec<TaskRow>, Vec<TaskRow>) = rows.into_iter().partition(|r| r.kind == TaskKind::Subagent);
    let mut toggle = move |k: TaskKind| open.set(if open() == Some(k) { None } else { Some(k) });
    let chip = |on: bool| if on { "pill-btn tk-chip on" } else { "pill-btn tk-chip" };
    let plural = |n: usize, one: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {one}s") };
    let shown = match open() {
        Some(TaskKind::Subagent) if !agents.is_empty() => Some((true, agents.clone())),
        Some(TaskKind::Shell) if !terms.is_empty() => Some((false, terms.clone())),
        _ => None,
    };
    let can_stop = app.is_running(&session_id);
    let sid_stop = session_id.clone();
    let n_terms = plural(terms.len(), "Terminal");
    rsx! {
        style { {CSS} }
        if !agents.is_empty() {
            button { class: chip(open() == Some(TaskKind::Subagent)), title: "Running subagents", onclick: move |_| toggle(TaskKind::Subagent),
                Dots { size: 12 }
                span { "Working " span { class: "n", "{agents.len()}" } }
            }
        }
        if !terms.is_empty() {
            button { class: chip(open() == Some(TaskKind::Shell)), title: "Running terminals", onclick: move |_| toggle(TaskKind::Shell),
                Dots { size: 12 }
                span { class: "n", "{n_terms}" }
            }
        }
        if let Some((is_agents, list)) = shown {
            div { class: if is_agents { "tk-card agents" } else { "tk-card terms" },
                div { class: "tk-card-head",
                    span { class: "tk-ic", Icon { i: if is_agents { I::Orbit } else { I::Prompt }, size: 14 } }
                    span { class: "ttl", {if is_agents { plural(list.len(), "Agent") } else { plural(list.len(), "Terminal") }} }
                    if can_stop {
                        span { "·" }
                        button { class: "tk-stop-all", title: "Stop the agent and everything it runs",
                            onclick: move |_| {
                                open.set(None);
                                app.stop(&sid_stop);
                            },
                            "Stop All"
                        }
                    }
                    span { class: "grow" }
                    button { class: "icon-btn", title: "Collapse", onclick: move |_| open.set(None),
                        Icon { i: I::Minus, size: 14 }
                    }
                }
                div { class: "tk-card-list",
                    for r in list {
                        CardItem { key: "{r.id}", session_id: session_id.clone(), row: r.clone() }
                    }
                }
            }
        }
    }
}

#[component]
fn CardItem(session_id: String, row: TaskRow) -> Element {
    let app = use_app();
    let id = row.id.clone();
    let kind = row.kind;
    let agent = kind == TaskKind::Subagent;
    rsx! {
        div {
            class: "tk-item",
            role: "button",
            title: if agent { "Open the subagent" } else { "Show the terminal" },
            onclick: move |_| open_row(app, &session_id, &id, kind),
            span { class: "lead",
                if agent { Dots { size: 12 } } else { Icon { i: I::Prompt, size: 14 } }
            }
            span { class: "ttl", title: "{row.title}", "{row.title}" }
            if agent {
                span { class: "act", {row.activity.clone().unwrap_or_default()} }
            }
            Elapsed { class: "when", active: true, started: row.started, ms: row.ms }
            span { class: "go", Icon { i: I::ChevronRight, size: 13 } }
        }
    }
}

/// Subagents open their transcript; terminals open read-only in the side
/// panel's Terminal tab.
fn open_row(app: App, session_id: &str, id: &str, kind: TaskKind) {
    match kind {
        TaskKind::Subagent => app.open_subagent(session_id, id),
        _ => open_agent_term(app, session_id, id),
    }
}

/// Show an agent's terminal (a shell tool call) in the Terminal tab.
pub fn open_agent_term(mut app: App, session_id: &str, tool_id: &str) {
    let mut ui = app.ui.write();
    ui.agent_term = Some((session_id.to_string(), tool_id.to_string()));
    ui.open_panel(RightTab::Terminal, false);
}

/// The side panel's Background Tasks tab: every background shell, monitor
/// and subagent of the session in collapsible Running / Finished groups.
#[component]
pub fn TasksPanel(session_id: String) -> Element {
    let app = use_app();
    let mut closed = use_signal(|| (false, false));
    let rows = use_memo(use_reactive!(|session_id| app.with_session(&session_id, |s| all_rows(&s.messages)).unwrap_or_default()));
    let mut rows = rows();
    rows.reverse();
    let (running, done): (Vec<TaskRow>, Vec<TaskRow>) = rows.into_iter().partition(|r| r.active);
    let (c_run, c_done) = closed();
    rsx! {
        style { {CSS} }
        div { class: "tk-panel",
            if running.is_empty() && done.is_empty() {
                div { class: "rp-empty",
                    Icon { i: I::Workflow, size: 18, stroke: 1.5 }
                    "No background tasks yet"
                    span { class: "tk-panel-hint", "Subagents and commands an agent runs in the background show up here." }
                }
            }
            if !running.is_empty() {
                div { class: if c_run { "tk-group closed" } else { "tk-group" },
                    button { class: "tk-group-head", onclick: move |_| closed.with_mut(|c| c.0 = !c.0),
                        span { class: "chev", Icon { i: I::ChevronDown, size: 12 } }
                        "Running"
                        span { class: "n", "{running.len()}" }
                    }
                    div { class: "tk-group-body",
                        div { class: "tk-group-inner",
                            for r in running {
                                TaskRowView { key: "{r.id}", session_id: session_id.clone(), row: r.clone() }
                            }
                        }
                    }
                }
            }
            if !done.is_empty() {
                div { class: if c_done { "tk-group closed" } else { "tk-group" },
                    button { class: "tk-group-head", onclick: move |_| closed.with_mut(|c| c.1 = !c.1),
                        span { class: "chev", Icon { i: I::ChevronDown, size: 12 } }
                        "Finished"
                        span { class: "n", "{done.len()}" }
                    }
                    div { class: "tk-group-body",
                        div { class: "tk-group-inner",
                            for r in done {
                                TaskRowView { key: "{r.id}", session_id: session_id.clone(), row: r.clone() }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Run time that counts up each second while the task runs; the recorded
/// time once it is done.
#[component]
fn Elapsed(class: &'static str, active: bool, started: Option<DateTime<Utc>>, ms: Option<u64>) -> Element {
    let mut now = use_signal(Utc::now);
    let mut on = use_signal(|| active);
    use_effect(use_reactive!(|active| on.set(active)));
    use_future(move || async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if *on.peek() {
                now.set(Utc::now());
            }
        }
    });
    let text = match (active, started) {
        (true, Some(s)) => fmt_ms((now() - s).num_milliseconds().max(0) as u64),
        _ => ms.map(fmt_ms).unwrap_or_default(),
    };
    rsx! { span { class: "{class}", "{text}" } }
}

#[component]
fn TaskRowView(session_id: String, row: TaskRow) -> Element {
    let app = use_app();
    let icon = match row.kind {
        TaskKind::Subagent => I::Orbit,
        TaskKind::Monitor => I::Monitor,
        TaskKind::Shell => I::Prompt,
    };
    let class = match (row.kind == TaskKind::Subagent, row.active) {
        (true, true) => "tk-row",
        (true, false) => "tk-row done",
        (false, true) => "tk-row term",
        (false, false) => "tk-row term done",
    };
    let id = row.id.clone();
    let kind = row.kind;
    let word = status_word(row.status, row.is_error);
    // One line under the title: what it is doing (or printed last), else
    // what it is.
    let sub_out = kind != TaskKind::Subagent && row.activity.is_some();
    let sub = match (&row.activity, &row.meta) {
        (Some(a), Some(_)) if row.active => a.clone(),
        (_, Some(m)) => m.clone(),
        (Some(a), None) => a.clone(),
        (None, None) => if kind == TaskKind::Monitor { "Monitor".into() } else { "Background command".into() },
    };
    let done = match row.ms {
        Some(ms) => format!("{word} · {}", fmt_ms(ms)),
        None => word.to_string(),
    };
    rsx! {
        div {
            class: "{class}",
            title: if kind == TaskKind::Subagent { "Open the subagent" } else { "Show the terminal" },
            onclick: move |_| open_row(app, &session_id, &id, kind),
            span { class: "tk-lead",
                if row.active { Dots { size: 12 } } else { StatusIcon { status: row.status, error: row.is_error } }
            }
            div { class: "tk-row-main",
                div { class: "tk-row-top",
                    span { class: "tk-ic", Icon { i: icon, size: 13 } }
                    span { class: "tk-row-title", title: "{row.title}", "{row.title}" }
                    if row.active {
                        Elapsed { class: "tk-row-when", active: true, started: row.started, ms: None }
                    } else {
                        span { class: "tk-row-when", "{done}" }
                    }
                }
                div { class: if sub_out { "tk-row-sub out" } else { "tk-row-sub" }, title: "{sub}", "{sub}" }
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
            span { class: "tk-st", title: "Running", super::Spinner { size: 13 } }
        },
        ToolStatus::Cancelled => rsx! { span { class: "tk-st muted", title: "Stopped", Icon { i: I::Stop, size: 10 } } },
        _ => rsx! { span { class: "tk-st ok", title: "Done", Icon { i: I::CircleCheck, size: 13 } } },
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TaskKind {
    Subagent,
    Shell,
    Monitor,
}

#[derive(Clone, PartialEq, Debug)]
pub struct TaskRow {
    pub id: String,
    pub kind: TaskKind,
    /// A subagent's task, or a terminal's command (first line).
    pub title: String,
    pub status: ToolStatus,
    pub is_error: bool,
    pub active: bool,
    pub started: Option<DateTime<Utc>>,
    /// Run time once finished (running tasks count up from `started`).
    pub ms: Option<u64>,
    /// Agent type, model and tool count of a subagent.
    pub meta: Option<String>,
    /// What it is doing now: a subagent's latest step, a command's latest
    /// output line.
    pub activity: Option<String>,
}

/// Subagents and terminals running right now (recursive): the chips.
fn live_rows(messages: &[Message]) -> Vec<TaskRow> {
    let mut out = Vec::new();
    collect(messages, &mut out, 0, &|t: &ToolCall| t.is_active() && (t.is_subagent() || t.background || t.kind == ToolKind::Shell));
    out
}

/// Whether any subagent or terminal is running (recursive).
pub fn has_live(messages: &[Message]) -> bool {
    !live_rows(messages).is_empty()
}

/// Every background task and subagent of the session (recursive).
fn all_rows(messages: &[Message]) -> Vec<TaskRow> {
    let mut out = Vec::new();
    collect(messages, &mut out, 0, &|t: &ToolCall| t.background || t.is_subagent());
    out
}

/// The agent's terminals for the Terminal tab: background commands and
/// commands running now, plus `keep` (the one on screen). Running first,
/// then newest first.
pub fn shell_rows(messages: &[Message], keep: Option<&str>) -> Vec<TaskRow> {
    let mut out = Vec::new();
    collect(messages, &mut out, 0, &|t: &ToolCall| !t.is_subagent() && (t.background || (t.kind == ToolKind::Shell && t.is_active()) || Some(t.id.as_str()) == keep));
    out.reverse();
    out.sort_by_key(|r| !r.active);
    out.truncate(40);
    out
}

fn collect(messages: &[Message], out: &mut Vec<TaskRow>, depth: usize, pick: &dyn Fn(&ToolCall) -> bool) {
    for m in messages {
        for t in m.tool_calls() {
            // A call seen twice (a subagent transcript linked into two
            // calls) is listed once: rows are keyed by call id.
            if pick(t) && !out.iter().any(|r| r.id == t.id) {
                out.push(task_row(t));
            }
            if depth < 4 {
                collect(&t.subagent, out, depth + 1, pick);
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
    let active = t.is_active();
    let (title, meta) = match kind {
        TaskKind::Subagent => {
            let label = agent_label(t);
            let desc = agent_desc(t, &label);
            let info = sub_info(t, Utc::now());
            let mut meta = vec![label.clone()];
            meta.extend(info.model);
            if info.tools > 0 {
                meta.push(format!("{} tool{}", info.tools, if info.tools == 1 { "" } else { "s" }));
            }
            (if desc.is_empty() { label } else { desc }, Some(meta.join(" · ")))
        }
        _ => (shell_title(t), None),
    };
    TaskRow {
        id: t.id.clone(),
        kind,
        title,
        status: t.status,
        is_error: t.is_error,
        active,
        started: t.started_at,
        // Running times tick in the view: keep rows equal between ticks so
        // the lists only redraw when something changes.
        ms: if active { None } else { run_ms(t, Utc::now()) },
        meta,
        activity: activity(t),
    }
}

/// A shell call's command (first line), or what describes it.
pub fn shell_title(t: &ToolCall) -> String {
    input_str(t, &["command", "cmd", "description"])
        .map(|c| c.lines().next().unwrap_or("").to_string())
        .or_else(|| t.title.clone())
        .unwrap_or_else(|| t.name.clone())
}

/// The latest step of a subagent, or the latest output line of a command.
fn activity(t: &ToolCall) -> Option<String> {
    let line = if !t.subagent.is_empty() {
        t.subagent.iter().rev().flat_map(|m| m.parts.iter().rev()).find_map(|p| match p {
            Part::Tool(x) => Some(x.title.clone().unwrap_or_else(|| x.name.clone())),
            Part::Text { text } => text.lines().rev().map(str::trim).find(|l| !l.is_empty()).map(str::to_string),
            _ => None,
        })?
    } else if t.is_subagent() {
        // Without its transcript a subagent's output is the launch notice
        // ("Async agent launched … output_file: …"), not its work.
        return None;
    } else {
        // A background command's output is where to find it, not what it
        // printed.
        t.output.as_deref()?.lines().rev().map(str::trim_end).find(|l| !l.trim().is_empty() && !is_launch_notice(l))?.to_string()
    };
    Some(if line.chars().count() > 160 { line.chars().take(159).collect::<String>() + "…" } else { line })
}

/// "Command running in background with ID: …" and similar launch lines.
pub fn is_launch_notice(line: &str) -> bool {
    let l = line.trim_start();
    l.starts_with("Command running in background") || l.starts_with("Output is being written to") || l.starts_with("output_file:")
}

/// A tool call by id anywhere in the transcript, subagents included.
pub fn find_tool<'a>(messages: &'a [Message], id: &str) -> Option<&'a ToolCall> {
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

/// What a subagent ran on and how much it did.
#[derive(Clone, PartialEq, Default)]
struct SubInfo {
    model: Option<String>,
    effort: Option<String>,
    tools: usize,
    /// Tokens in its context at its latest call.
    context: Option<u64>,
    ms: Option<u64>,
}

fn sub_info(t: &ToolCall, now: DateTime<Utc>) -> SubInfo {
    // The model it ran on, else the one it was asked to use.
    let model = t
        .subagent
        .iter()
        .find_map(|m| m.model.clone())
        .or_else(|| input_str(t, &["model"]).map(str::to_string))
        .map(|m| larrier_core::models::display_name(&m));
    let effort = input_str(t, &["reasoning_effort", "model_reasoning_effort", "effort"]).map(|e| {
        let mut c = e.chars();
        c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
    });
    let context = t.subagent.iter().rev().find_map(|m| m.usage.as_ref().and_then(|u| u.context_tokens)).filter(|c| *c > 0);
    SubInfo { model, effort, tools: t.subagent_steps(), context, ms: run_ms(t, now) }
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

pub fn fmt_ms(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0 => "0s".into(),
        1..=59 => format!("{s}s"),
        60..=3599 => format!("{}m {:02}s", s / 60, s % 60),
        _ => format!("{}h {:02}m", s / 3600, (s % 3600) / 60),
    }
}

pub fn status_word(status: ToolStatus, error: bool) -> &'static str {
    if error || status == ToolStatus::Failed {
        return "Failed";
    }
    match status {
        ToolStatus::Cancelled => "Stopped",
        ToolStatus::Running | ToolStatus::Pending | ToolStatus::Approval => "Running",
        _ => "Done",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use larrier_core::AgentKind;
    use serde_json::json;

    fn msg(tools: Vec<ToolCall>) -> Message {
        let mut m = Message::assistant(AgentKind::ClaudeCode, None);
        m.parts = tools.into_iter().map(Part::Tool).collect();
        m
    }

    #[test]
    fn chips_count_running_subagents_and_terminals() {
        let mut agent = ToolCall::new("a1", "Agent", json!({"description": "Audit the parser", "subagent_type": "Explore"}));
        agent.kind = ToolKind::Task;
        agent.agent_name = Some("Explore".into());
        agent.status = ToolStatus::Running;
        let mut read = ToolCall::new("r1", "Read", json!({"file_path": "src/a.rs"}));
        read.title = Some("Read src/a.rs".into());
        let mut inner = Message::assistant(AgentKind::ClaudeCode, Some("claude-haiku-4-5".into()));
        inner.parts.push(Part::Tool(read));
        agent.subagent.push(inner);
        let mut dev = ToolCall::new("b1", "Bash", json!({"command": "npm run dev", "run_in_background": true}));
        dev.kind = ToolKind::Shell;
        dev.background = true;
        dev.status = ToolStatus::Running;
        dev.output = Some("Command running in background with ID: b1. Output is being written to: /tmp/b1.output".into());
        let mut ls = ToolCall::new("s1", "Bash", json!({"command": "ls"}));
        ls.kind = ToolKind::Shell;
        ls.status = ToolStatus::Completed;
        let rows = live_rows(&[msg(vec![agent, dev, ls])]);
        assert_eq!(rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["a1", "b1"]);
        assert_eq!(rows[0].title, "Audit the parser");
        assert_eq!(rows[0].activity.as_deref(), Some("Read src/a.rs"));
        assert!(rows[0].meta.as_deref().unwrap().starts_with("Explore · "));
        assert_eq!(rows[1].title, "npm run dev");
        assert_eq!(rows[1].activity, None, "the launch notice is not output");
        assert_eq!(rows[1].ms, None);
    }

    #[test]
    fn terminal_list_keeps_the_shown_command() {
        let mut ls = ToolCall::new("s1", "Bash", json!({"command": "ls"}));
        ls.kind = ToolKind::Shell;
        ls.status = ToolStatus::Completed;
        let mut bg = ToolCall::new("b1", "Bash", json!({"command": "cargo build", "run_in_background": true}));
        bg.kind = ToolKind::Shell;
        bg.background = true;
        bg.status = ToolStatus::Completed;
        let mut run = ToolCall::new("s2", "Bash", json!({"command": "cargo test"}));
        run.kind = ToolKind::Shell;
        run.status = ToolStatus::Running;
        let msgs = [msg(vec![ls, bg, run])];
        let ids = |keep| shell_rows(&msgs, keep).into_iter().map(|r| r.id).collect::<Vec<_>>();
        assert_eq!(ids(None), ["s2", "b1"]);
        assert_eq!(ids(Some("s1")), ["s2", "b1", "s1"]);
    }
}
