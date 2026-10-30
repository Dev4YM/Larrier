//! The agent view: tab strip header, transcript + composer column (two side
//! by side in a split), the "On <repo>" tool list, the side panel (Changes /
//! Browser / Terminal / Files) and drop targets for dragged chats — laid out
//! like Cursor's Agents Window.

use std::path::Path;

use dioxus::html::input_data::MouseButton;
use dioxus::prelude::*;
use larrier_agents::git;
use larrier_core::{Environment, Part, Session, ToolKind, ToolStatus};
use larrier_formats::ExportFormat;

use super::composer::Composer;
use super::right::RightPane;
use super::transcript::Transcript;
use super::{open_tool, AgentGlyph, Dropdown, DragRegion, MenuItem, SidebarToggle};
use crate::icons::{Icon, I};
use crate::state::{use_app, use_ui, ChatTab, Modal, RightTab, TabCtx, View};

/// Changes whenever the agent may have changed files: a file edit or a shell
/// command finished. Git status is re-read on this, never per token.
pub fn change_key(s: &Session) -> usize {
    s.messages
        .iter()
        .flat_map(|m| m.tool_calls())
        .filter(|t| (t.kind.is_mutation() || t.kind == ToolKind::Shell) && !matches!(t.status, ToolStatus::Running | ToolStatus::Pending | ToolStatus::Approval))
        .count()
}

/// A real commit message for an agent's work: the chat title as the subject
/// (≤ 72 chars) and the agent's closing summary as the body.
pub fn commit_message(title: &str, summary: &str) -> String {
    let mut subject: String = title.lines().next().unwrap_or("").trim().to_string();
    if subject.is_empty() {
        subject = "Update from Larrier agent".into();
    }
    if subject.chars().count() > 72 {
        subject = subject.chars().take(71).collect::<String>().trim_end().to_string() + "…";
    }
    let mut body: Vec<String> = Vec::new();
    let mut in_code = false;
    for line in summary.lines() {
        let l = line.trim_end();
        if l.trim_start().starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            continue;
        }
        let plain = l.replace("**", "").replace('`', "");
        if plain.trim().is_empty() && body.last().map(|b| b.is_empty()).unwrap_or(true) {
            continue;
        }
        body.push(plain);
        if body.len() >= 20 {
            break;
        }
    }
    while body.last().map(|b| b.trim().is_empty()).unwrap_or(false) {
        body.pop();
    }
    if body.is_empty() {
        subject
    } else {
        format!("{subject}\n\n{}", body.join("\n"))
    }
}

/// A session: its chat (or one of its subagents' transcripts, `sub`), with
/// the side panel, the session rail or a second session beside it.
#[component]
pub fn AgentView(id: String, #[props(default)] sub: Option<String>) -> Element {
    let mut app = use_app();
    let right_open = use_ui(|u| u.right_open)();
    let loaded = app.open.read().contains_key(&id);
    let split = app.split.read().clone().filter(|s| *s != id && app.open.read().contains_key(s));
    let mut dragging = use_signal(|| false);
    let mut last_press = use_signal(|| None::<std::time::Instant>);
    if !loaded {
        return rsx! { div { class: "empty-state", "Loading…" } };
    }
    // With the session rail, the header runs the whole width and the rail
    // sits under it, like Cursor's; beside a panel or a second chat, each
    // column keeps its own header.
    let rail = split.is_none() && !right_open;
    let col_class = format!(
        "chat-col primary{}{}{}",
        if right_open && split.is_none() { " fixed" } else { "" },
        if rail { " bare" } else { "" },
        if sub.is_some() { " showing-sub" } else { "" },
    );
    rsx! {
        div { class: if split.is_none() { "agent-frame with-toggle" } else { "agent-frame" },
        if split.is_none() {
            PanelToggle {}
        }
        if rail {
            ChatHeader { id: id.clone() }
        }
        div { class: if split.is_some() { "agent-body split" } else { "agent-body" },
            div {
                class: "{col_class}",
                "data-session": "{id}",
                if !rail {
                    ChatHeader { id: id.clone() }
                }
                // The chat stays laid out under a subagent's transcript, so
                // going back finds it as it was.
                ChatColumn { id: id.clone(), input_id: "composer-input" }
                if let Some(tool) = sub.clone() {
                    div { key: "{tool}", class: "sub-layer",
                        super::tasks::SubagentTranscript { session_id: id.clone(), tool_id: tool }
                    }
                }
                if right_open && split.is_none() {
                    div {
                        class: if dragging() { "resizer dragging" } else { "resizer" },
                        title: "Drag to resize · double-click to fit the window",
                        onmousedown: move |_| {
                            if super::second_press(&mut last_press.write()) {
                                let (win_w, side) = { let ui = app.ui.peek(); (ui.win_w, ui.sidebar_open) };
                                let sidebar = side.then(|| app.config.peek().settings.sidebar_width);
                                let w = super::auto_chat_width(win_w, sidebar);
                                app.update_config(|c| c.settings.chat_width = w);
                            } else {
                                dragging.set(true);
                            }
                        },
                    }
                }
            }
            if let Some(other) = split.clone() {
                div { key: "split-{other}", class: "chat-col secondary", "data-session": "{other}",
                    SplitHeader { id: other.clone() }
                    ChatColumn { id: other.clone(), input_id: "composer-input-2" }
                }
            } else if right_open {
                RightPane { id: id.clone() }
            } else {
                SessionRail { id: id.clone() }
            }
            DropZones { target: Some(id.clone()) }
            if dragging() {
                div {
                    style: "position: fixed; inset: 0; z-index: 300; cursor: col-resize;",
                    onmousemove: move |e| {
                        let side = if app.ui.read().sidebar_open { app.config.read().settings.sidebar_width as f64 } else { 0.0 };
                        let w = (e.client_coordinates().x - side).clamp(420.0, 1400.0) as u32;
                        app.config.write().settings.chat_width = w;
                    },
                    onmouseup: move |_| {
                        dragging.set(false);
                        app.save_config();
                    },
                }
            }
        }
        }
    }
}

/// Transcript, background tasks, changes bar and composer of one session.
#[component]
fn ChatColumn(id: String, input_id: &'static str) -> Element {
    rsx! {
        Transcript { key: "t-{id}", id: id.clone() }
        AboveComposer { id: id.clone() }
        Composer { session_id: Some(id.clone()), input_id }
    }
}

/// What the session rail shows about a session.
#[derive(Clone, PartialEq)]
struct RailInfo {
    agent: larrier_core::AgentKind,
    model: Option<String>,
    /// The user's own agent the chat runs as (Claude Code's `--agent`).
    run_as: Option<String>,
    workspace: Option<String>,
    branch: Option<String>,
    env: String,
    created: chrono::DateTime<chrono::Utc>,
    updated: chrono::DateTime<chrono::Utc>,
    /// When the latest prompt was sent (how long the agent has worked).
    turn_started: Option<chrono::DateTime<chrono::Utc>>,
    prompts: usize,
    input: u64,
    output: u64,
    cache: u64,
    cost: Option<f64>,
    /// Tokens in the model's context at the latest reply, and its window.
    context: Option<(u64, Option<u64>)>,
    plan: Vec<(String, larrier_core::PlanStatus)>,
    /// Latest tool steps: (title, running, failed).
    recent: Vec<(String, bool, bool)>,
    /// Changed files (name, path, +, -), most changed first, and the total.
    files: Vec<(String, String, u32, u32)>,
    files_total: (usize, u32, u32),
    /// Running background work: (title, is a subagent).
    tasks: Vec<(String, bool)>,
}

fn rail_info(s: &Session) -> RailInfo {
    use larrier_core::{PlanStatus, Role};
    let model_raw = s.model.clone().or_else(|| s.messages.iter().rev().find_map(|m| m.model.clone()));
    // Context in use at the latest call, as the agent reported it (a reply
    // spanning several calls sums their tokens, so its usage cannot tell).
    let context = s.messages.iter().rev().find_map(|m| m.usage.as_ref().and_then(|u| u.context_tokens.map(|c| (c, u.context_window)))).or(s.usage.context_tokens.map(|c| (c, s.usage.context_window))).map(|(used, window)| {
        (used, window.or_else(|| model_raw.as_deref().and_then(larrier_core::models::lookup).and_then(|m| m.context)))
    });
    let cost = s.usage.cost_usd.or_else(|| {
        let price = larrier_core::models::price(model_raw.as_deref()?)?;
        let u = &s.usage;
        Some(price.cost(u.input_tokens, u.output_tokens + u.reasoning_tokens, u.cache_read_tokens, u.cache_write_tokens)).filter(|c| *c > 0.0)
    });
    // The newest to-do list, from a plan part or the call that wrote it.
    let plan = s
        .messages
        .iter()
        .rev()
        .flat_map(|m| m.parts.iter().rev())
        .find_map(|p| match p {
            Part::Plan { entries } => Some(entries.clone()),
            Part::Tool(t) if t.kind == ToolKind::Todo => larrier_formats::util::plan_from(&t.input),
            _ => None,
        })
        .unwrap_or_default()
        .into_iter()
        .map(|e| (e.content, e.status))
        .collect::<Vec<(String, PlanStatus)>>();
    let mut recent = Vec::new();
    'outer: for m in s.messages.iter().rev() {
        for t in m.tool_calls().collect::<Vec<_>>().into_iter().rev() {
            if matches!(t.kind, ToolKind::Think | ToolKind::Todo) {
                continue;
            }
            let title = t.title.clone().unwrap_or_else(|| t.describe());
            recent.push((larrier_core::lsf::first_line(&title), t.is_active(), t.is_error || t.status == ToolStatus::Failed));
            if recent.len() == 5 {
                break 'outer;
            }
        }
    }
    let mut changed = s.changed_files();
    let files_total = (changed.len(), changed.iter().map(|f| f.added).sum(), changed.iter().map(|f| f.removed).sum());
    changed.sort_by_key(|f| std::cmp::Reverse(f.added + f.removed));
    let files = changed
        .into_iter()
        .take(5)
        .map(|f| {
            let name = Path::new(&f.path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| f.path.clone());
            (name, f.path, f.added, f.removed)
        })
        .collect();
    let tasks = larrier_core::lsf::active_tasks(&s.messages)
        .into_iter()
        .map(|t| (t.agent_name.clone().or_else(|| t.title.clone()).unwrap_or_else(|| t.describe()), t.is_subagent()))
        .collect();
    RailInfo {
        agent: s.agent.clone(),
        model: model_raw.as_deref().map(larrier_core::models::display_name),
        run_as: s.run_as.clone(),
        workspace: s.workspace.as_ref().map(|w| if w.name.is_empty() { w.path.clone() } else { w.name.clone() }),
        branch: s.workspace.as_ref().and_then(|w| w.branch.clone()),
        env: match &s.environment {
            Environment::Worktree { .. } => "Worktree".into(),
            Environment::Ssh { host, .. } => format!("SSH · {host}"),
            Environment::Local => "This PC".into(),
        },
        created: s.created_at,
        updated: s.updated_at,
        turn_started: s.messages.iter().rev().find(|m| m.role == Role::User).map(|m| m.created_at),
        prompts: s.messages.iter().filter(|m| m.role == Role::User).count(),
        input: s.usage.input_tokens,
        output: s.usage.output_tokens + s.usage.reasoning_tokens,
        cache: s.usage.cache_read_tokens + s.usage.cache_write_tokens,
        cost,
        context,
        plan,
        recent,
        files,
        files_total,
        tasks,
    }
}

/// "12m", "3h 5m", "4d 2h": how long something has run.
fn span_short(d: chrono::Duration) -> String {
    let m = d.num_minutes().max(0);
    match m {
        0 => format!("{}s", d.num_seconds().max(0)),
        1..=59 => format!("{m}m"),
        60..=1439 => format!("{}h {}m", m / 60, m % 60),
        _ => format!("{}d {}h", m / 1440, m % 1440 / 60),
    }
}

/// The column beside the chat while the side panel is closed: the session
/// at a glance (agent, context, to-dos, activity, changes, background work)
/// and its tools. Folds down to icons.
#[component]
fn SessionRail(id: String) -> Element {
    let app = use_app();
    let collapsed = app.config.read().settings.session_rail_collapsed;
    // One frame whose width animates between the folded card and the full
    // rail, so the chat beside it eases instead of jumping.
    rsx! {
        div { class: if collapsed { "rail-wrap folded" } else { "rail-wrap" },
            RailBody { id }
        }
    }
}

#[component]
fn RailBody(id: String) -> Element {
    use larrier_core::PlanStatus;
    let mut app = use_app();
    let id_m = id.clone();
    let info = use_memo(move || app.with_session(&id_m, rail_info));
    let collapsed = app.config.read().settings.session_rail_collapsed;
    let running = app.is_active(&id);
    let Some(h) = info() else { return rsx! {} };
    let toggle = move |_| app.update_config(|c| c.settings.session_rail_collapsed = !c.settings.session_rail_collapsed);
    let tasks_badge = (!h.tasks.is_empty()).then(|| h.tasks.len().to_string());
    if collapsed {
        return rsx! {
            div { class: "tools-col collapsed",
                div { class: "rail-head",
                    button { class: "icon-btn", title: "Show session details", onclick: toggle, Icon { i: I::ChevronsLeft, size: 15, stroke: 1.6 } }
                }
                ToolLink { tab: RightTab::Changes, icon: I::Diff, label: "Changes", icon_only: true }
                ToolLink { tab: RightTab::Browser, icon: I::Globe, label: "Browser", icon_only: true }
                ToolLink { tab: RightTab::Terminal, icon: I::Terminal, label: "Terminal", icon_only: true }
                ToolLink { tab: RightTab::Files, icon: I::Files, label: "Files", icon_only: true }
                ToolLink { tab: RightTab::Tasks, icon: I::Workflow, label: "Background Tasks", icon_only: true, badge: tasks_badge }
            }
        };
    }
    let now = chrono::Utc::now();
    let status = match (running, h.turn_started) {
        (true, Some(t)) => format!("Working · {}", span_short(now - t)),
        (true, None) => "Working".to_string(),
        (false, _) => format!("Idle · {}", crate::state::rel_time(h.updated)),
    };
    let started = h.created.with_timezone(&chrono::Local).format("%b %-d, %H:%M").to_string();
    let lifetime = span_short(h.updated - h.created);
    let fmt = super::usage::fmt_tokens;
    let (ctx_used, ctx_max) = h.context.unwrap_or((0, None));
    let ctx_pct = ctx_max.filter(|m| *m > 0).map(|m| (ctx_used as f64 / m as f64 * 100.0).clamp(0.0, 100.0));
    let ctx_sev = match ctx_pct {
        Some(p) if p >= 85.0 => "crit",
        Some(p) if p >= 65.0 => "warn",
        _ => "",
    };
    let done = h.plan.iter().filter(|(_, st)| *st == PlanStatus::Completed).count();
    let plan_more = h.plan.len().saturating_sub(7);
    let (fcount, fadd, frem) = h.files_total;
    rsx! {
        div { class: "tools-col",
            div { class: "rail-head",
                span { class: "h", if let Some(w) = &h.workspace { "On {w}" } else { "Session" } }
                button { class: "icon-btn", title: "Collapse", onclick: toggle, Icon { i: I::ChevronsRight, size: 15, stroke: 1.6 } }
            }
            div { class: "rail-card",
                div { class: "who",
                    AgentGlyph { kind: h.agent.clone() }
                    span { class: "a", "{h.agent.display_name()}" }
                    if let Some(m) = &h.model { span { class: "m", title: "{m}", "{m}" } }
                }
                if let Some(a) = &h.run_as {
                    div { class: "rail-line run-as", title: "This chat runs as your {a} agent: its instructions, tools and MCP servers",
                        Icon { i: I::Orbit, size: 12, stroke: 1.6 }
                        span { "{a} agent" }
                    }
                }
                div { class: "rail-line",
                    if running { super::Spinner { size: 12 } } else { span { class: "dot" } }
                    span { "{status}" }
                }
                div { class: "rail-line",
                    Icon { i: I::GitBranch, size: 12, stroke: 1.6 }
                    span { title: "{h.env}", if let Some(b) = &h.branch { "{b} · {h.env}" } else { "{h.env}" } }
                }
                div { class: "rail-line muted",
                    Icon { i: I::Clock, size: 12, stroke: 1.6 }
                    span { title: "Started {started}", "{h.prompts} prompts · {lifetime} · since {started}" }
                }
            }

            // ---- context and usage
            if h.context.is_some() || h.input + h.output > 0 {
                div { class: "rail-sec",
                    div { class: "rail-sec-head", span { "Context" }
                        if let Some(p) = ctx_pct { span { class: "meta", "{p:.0}%" } }
                    }
                    if let Some(p) = ctx_pct {
                        div { class: "rail-bar {ctx_sev}", div { class: "fill", style: "width: {p:.1}%;" } }
                    }
                    div { class: "rail-stats",
                        if ctx_used > 0 {
                            span { title: "Tokens in the model's context at the latest reply",
                                if let Some(m) = ctx_max { "{fmt(ctx_used)} of {fmt(m)}" } else { "{fmt(ctx_used)} in context" }
                            }
                        }
                    }
                    div { class: "rail-stats",
                        span { title: "Input tokens", "{fmt(h.input)} in" }
                        span { title: "Output tokens", "{fmt(h.output)} out" }
                        if h.cache > 0 { span { title: "Cache reads and writes", "{fmt(h.cache)} cache" } }
                        if let Some(c) = h.cost { span { title: "Cost (estimated from list prices when the agent does not report it)", "${c:.2}" } }
                    }
                }
            }

            // ---- to-dos
            if !h.plan.is_empty() {
                div { class: "rail-sec",
                    div { class: "rail-sec-head", span { "To-dos" } span { class: "meta", "{done} of {h.plan.len()}" } }
                    div { class: "rail-bar", div { class: "fill ok", style: "width: {done as f64 / h.plan.len() as f64 * 100.0:.1}%;" } }
                    for (i, (text, st)) in h.plan.iter().take(7).enumerate() {
                        div { key: "{i}", class: match st { PlanStatus::Completed => "rail-todo done", PlanStatus::InProgress => "rail-todo now", _ => "rail-todo" },
                            span { class: "box", if *st == PlanStatus::Completed { Icon { i: I::Check, size: 9, stroke: 3.0 } } }
                            span { class: "t", title: "{text}", "{text}" }
                        }
                    }
                    if plan_more > 0 { div { class: "rail-more", "+{plan_more} more" } }
                }
            }

            // ---- latest activity
            if !h.recent.is_empty() {
                div { class: "rail-sec",
                    div { class: "rail-sec-head", span { "Activity" } }
                    for (i, (title, active, failed)) in h.recent.iter().enumerate() {
                        div { key: "{i}", class: if *failed { "rail-act failed" } else { "rail-act" },
                            span { class: "ic",
                                if *active { super::Spinner { size: 11 } }
                                else if *failed { Icon { i: I::X, size: 11, stroke: 2.0 } }
                                else { Icon { i: I::Check, size: 11, stroke: 2.0 } }
                            }
                            span { class: "t", title: "{title}", "{title}" }
                        }
                    }
                }
            }

            // ---- changed files
            if fcount > 0 {
                div { class: "rail-sec",
                    div { class: "rail-sec-head", span { "Files changed · {fcount}" }
                        span { class: "meta", span { class: "add", "+{fadd}" } " " span { class: "del", "-{frem}" } }
                    }
                    for (name, path, a, r) in h.files.iter().cloned() {
                        button { key: "{path}", class: "rail-file", title: "{path}",
                            onclick: move |_| {
                                let mut ui = app.ui.write();
                                ui.open_panel(RightTab::Changes, false);
                                ui.reveal_file = Some(path.clone());
                            },
                            span { class: "t", "{name}" }
                            span { class: "add", "+{a}" }
                            span { class: "del", "-{r}" }
                        }
                    }
                }
            }

            // ---- background work
            if !h.tasks.is_empty() {
                div { class: "rail-sec",
                    div { class: "rail-sec-head", span { "Running in background · {h.tasks.len()}" } }
                    for (i, (title, sub)) in h.tasks.iter().take(4).enumerate() {
                        button { key: "{i}", class: "rail-act link", onclick: move |_| app.ui.write().open_panel(RightTab::Tasks, false),
                            span { class: "ic", super::Spinner { size: 11 } }
                            Icon { i: if *sub { I::Orbit } else { I::Prompt }, size: 12, stroke: 1.6 }
                            span { class: "t", title: "{title}", "{title}" }
                        }
                    }
                }
            }

            div { class: "rail-sec",
                div { class: "rail-sec-head", span { "Tools" } }
                ToolLink { tab: RightTab::Changes, icon: I::Diff, label: "Changes" }
                ToolLink { tab: RightTab::Browser, icon: I::Globe, label: "Browser" }
                ToolLink { tab: RightTab::Terminal, icon: I::Terminal, label: "Terminal" }
                ToolLink { tab: RightTab::Files, icon: I::Files, label: "Files" }
                ToolLink { tab: RightTab::Tasks, icon: I::Workflow, label: "Background Tasks", badge: tasks_badge }
            }
        }
    }
}

#[component]
fn ToolLink(tab: RightTab, icon: I, label: &'static str, #[props(default)] icon_only: bool, #[props(default)] badge: Option<String>) -> Element {
    let app = use_app();
    rsx! {
        button { class: "tool-link", title: if icon_only { label } else { "" }, onclick: move |_| open_tool(app, tab),
            span { class: "ic", Icon { i: icon, size: 16, stroke: 1.5 } }
            if !icon_only { span { class: "lb", "{label}" } }
            if let Some(b) = badge { span { class: "badge", "{b}" } }
        }
    }
}

/// What the header shows about a session (memoized: a streaming reply never
/// re-renders the header).
#[derive(Clone, PartialEq)]
struct HeadInfo {
    repo: Option<(String, String)>,
    env: (I, String),
    pinned: bool,
    archived: bool,
    has_cwd: bool,
    /// The latest pull request the agent opened: (number, url, repository).
    pr: Option<(u64, String, String)>,
}

fn head_info(s: &Session) -> HeadInfo {
    let env = match &s.environment {
        Environment::Worktree { branch, .. } => (I::Layers, format!("Worktree · {branch}")),
        Environment::Ssh { host, .. } => (I::Server, format!("SSH · {host}")),
        Environment::Local => (I::Laptop, "This PC".to_string()),
    };
    HeadInfo {
        repo: s.workspace.as_ref().map(|w| (if w.name.is_empty() { w.path.clone() } else { w.name.clone() }, w.path.clone())),
        env,
        pinned: s.pinned,
        archived: s.archived,
        has_cwd: s.cwd().is_some(),
        pr: s.extra.get("pull_requests").and_then(|p| p.as_array()).and_then(|p| p.last()).and_then(|p| {
            let url = p["url"].as_str()?.to_string();
            Some((p["number"].as_u64().unwrap_or(0), url, p["repository"].as_str().unwrap_or_default().to_string()))
        }),
    }
}

#[component]
fn ChatHeader(id: String) -> Element {
    let mut app = use_app();
    let id_m = id.clone();
    let info = use_memo(move || app.with_session(&id_m, head_info));
    let mut menu = use_signal(|| false);
    let mut export_menu = use_signal(|| false);
    let Some(h) = info() else { return rsx! {} };
    let (id_c, id_d, id_e, id_f, id_g, id_h, id_i) = (id.clone(), id.clone(), id.clone(), id.clone(), id.clone(), id.clone(), id.clone());
    let (env_icon, env_tip) = h.env.clone();

    rsx! {
        div { class: "main-head",
            SidebarToggle {}
            if let Some((name, path)) = h.repo.clone() {
                div { class: "repo-chip", title: "{path} · {env_tip}",
                    Icon { i: env_icon, size: 14, stroke: 1.5 }
                    span { class: "nm", dir: "auto", "{name}" }
                }
            }
            TabStrip {}
            DragRegion {}
            if let Some((number, url, repo)) = h.pr.clone() {
                button { class: "head-link", title: "{repo} · {url}", onclick: move |_| { let _ = open::that(&url); },
                    Icon { i: I::PullRequest, size: 13 }
                    if number > 0 { "PR #{number}" } else { "Pull request" }
                }
            }
            if h.has_cwd {
                button { class: "head-link", title: "Open the folder in Cursor / VS Code", onclick: move |_| app.open_in_ide(&id_c),
                    "IDE" Icon { i: I::ArrowUpRight, size: 12 }
                }
            }
            div { class: "menu-anchor",
                button { class: "icon-btn", title: "More", onclick: move |_| menu.set(true), Icon { i: I::More, size: 16 } }
                Dropdown { open: menu, up: false, right: true,
                    MenuItem { label: "Rename".to_string(), icon: Some(I::Pencil), onclick: move |_| { menu.set(false); app.ui.write().modal = Some(Modal::Rename(id_d.clone())); } }
                    MenuItem { label: "Fork".to_string(), icon: Some(I::Fork), onclick: move |_| { menu.set(false); app.fork(&id_e, None); } }
                    MenuItem { label: "Open in Terminal".to_string(), icon: Some(I::Terminal), onclick: move |_| { menu.set(false); app.open_in_terminal(&id_f); } }
                    MenuItem { label: "Export…".to_string(), icon: Some(I::Upload), onclick: move |_| { menu.set(false); export_menu.set(true); } }
                    MenuItem { label: if h.pinned { "Unpin".to_string() } else { "Pin".to_string() }, icon: Some(if h.pinned { I::PinOff } else { I::Pin }), onclick: move |_| { menu.set(false); app.toggle_pin(&id_g); } }
                    MenuItem { label: if h.archived { "Unarchive".to_string() } else { "Archive".to_string() }, icon: Some(I::Archive), onclick: move |_| { menu.set(false); app.toggle_archive(&id_i); } }
                    div { class: "menu-sep" }
                    MenuItem { label: "Delete".to_string(), icon: Some(I::Trash), danger: true, onclick: move |_| { menu.set(false); app.ui.write().modal = Some(Modal::ConfirmDelete(id_h.clone())); } }
                }
                Dropdown { open: export_menu, up: false, right: true,
                    div { class: "menu-label", "Export as" }
                    for f in ExportFormat::ALL.iter().copied() {
                        MenuItem {
                            key: "{f.label()}",
                            label: f.label().to_string(),
                            onclick: {
                                let id = id.clone();
                                move |_| {
                                    export_menu.set(false);
                                    app.export(&id, f);
                                }
                            },
                        }
                    }
                }
            }
        }
    }
}

/// The side panel's toggle: one button that stays where it is whether the
/// panel is open or not (the headers under it leave room for it).
#[component]
fn PanelToggle() -> Element {
    let mut app = use_app();
    let open = use_ui(|u| u.right_open)();
    rsx! {
        button { class: if open { "icon-btn pane-toggle panel on" } else { "icon-btn pane-toggle panel" },
            title: if open { "Hide side panel (Ctrl+Shift+J)" } else { "Show side panel (Ctrl+Shift+J)" },
            onclick: move |_| {
                app.ui.write().right_open = !open;
                app.update_config(|c| c.settings.right_panel_open = !open);
            },
            Icon { i: I::PanelRight, size: 16, stroke: 1.5 }
        }
    }
}

/// Header of the right-hand chat in a split.
#[component]
fn SplitHeader(id: String) -> Element {
    let mut app = use_app();
    let id_m = id.clone();
    let info = use_memo(move || app.sessions.read().iter().find(|s| s.id == id_m).map(|s| (s.title.clone(), s.agent.clone())));
    let running = app.is_active(&id);
    let (title, agent) = info().unwrap_or_else(|| ("Agent".into(), larrier_core::AgentKind::ClaudeCode));
    let (a, b) = (id.clone(), id.clone());
    rsx! {
        div { class: "main-head split-head",
            span { class: "ctab on static",
                span { class: "ic",
                    if running { super::Spinner { size: 14 } } else { AgentGlyph { kind: agent } }
                }
                span { class: "t", dir: "auto", "{title}" }
            }
            DragRegion {}
            button { class: "icon-btn", title: "Swap sides",
                onclick: move |_| {
                    let main = app.current_id();
                    app.split.set(main);
                    app.open_session(&a);
                },
                Icon { i: I::Swap, size: 15, stroke: 1.5 }
            }
            button { class: "icon-btn", title: "Open in the whole window",
                onclick: move |_| {
                    app.split.set(None);
                    app.open_session(&b);
                },
                Icon { i: I::AppWindow, size: 15, stroke: 1.5 }
            }
            button { class: "icon-btn", title: "Close side by side", onclick: move |_| app.split.set(None),
                Icon { i: I::X, size: 15, stroke: 1.5 }
            }
        }
    }
}

// ------------------------------------------------------------------ tabs

fn tab_key(v: &View) -> String {
    match v {
        View::Agent(id) => id.clone(),
        View::Subagent { session, tool } => format!("{session}:{tool}"),
        _ => String::new(),
    }
}

/// The window's tabs: the session's own tab, then the subagent chats opened
/// from it (single-click opens a preview tab, double-click keeps it).
#[component]
pub fn TabStrip() -> Element {
    let app = use_app();
    let tabs = app.window_tabs();
    let view = app.view.read().clone();
    let active = tabs.iter().position(|t| t.view == view);
    use_effect(use_reactive!(|active| {
        if active.is_some() {
            document::eval("requestAnimationFrame(() => document.querySelector('.tab-strip .ctab.on')?.scrollIntoView({block: 'nearest', inline: 'nearest'}))");
        }
    }));
    rsx! {
        div { class: "tab-strip", "data-drop": "tabstrip", role: "tablist",
            onwheel: move |e| {
                // Vertical wheel scrolls the strip sideways.
                let dy = e.delta().strip_units().y;
                if dy != 0.0 {
                    document::eval(&format!("document.querySelector('.tab-strip')?.scrollBy({{left: {dy}}})"));
                }
            },
            for t in tabs {
                ChatTabChip { key: "{tab_key(&t.view)}", active: t.view == view, tab: t }
            }
        }
    }
}

#[component]
fn ChatTabChip(tab: ChatTab, active: bool) -> Element {
    let mut app = use_app();
    let sid = tab.view.session_id().unwrap_or("").to_string();
    let sid_m = sid.clone();
    let summary = use_memo(move || app.sessions.read().iter().find(|s| s.id == sid_m).map(|s| (s.title.clone(), s.agent.clone(), s.unread)));
    let running = app.is_active(&sid);
    let sub = match &tab.view {
        View::Subagent { session, tool } => Some(
            app.peek_session(session, |s| {
                s.messages
                    .iter()
                    .flat_map(|m| m.tool_calls())
                    .find(|t| &t.id == tool)
                    .and_then(|t| t.agent_name.clone().or_else(|| t.title.clone()))
            })
            .flatten()
            .unwrap_or_else(|| "Subagent".into()),
        ),
        _ => None,
    };
    let (title, agent, unread) = summary().unwrap_or_else(|| ("Agent".into(), larrier_core::AgentKind::ClaudeCode, false));
    let label = sub.clone().unwrap_or_else(|| title.clone());
    let tip = match &sub {
        Some(s) => format!("{s} · subagent of {title}{}", if tab.preview { "\nPreview — double-click to keep open" } else { "" }),
        None => format!("{} · {title}", agent.display_name()),
    };
    let preview = tab.preview;
    let sub_tab = sub.is_some();
    let (v1, v2, v3, v4, v5) = (tab.view.clone(), tab.view.clone(), tab.view.clone(), tab.view.clone(), tab.view.clone());
    let mut cls = String::from("ctab");
    if active {
        cls.push_str(" on");
    }
    if preview {
        cls.push_str(" preview");
    }
    if unread && !active {
        cls.push_str(" unread");
    }
    rsx! {
        div {
            class: "{cls}",
            role: "tab",
            title: "{tip}",
            "data-ctx": "tab",
            "data-drag-session": if sub.is_none() { "{sid}" } else { "" },
            "data-drag-title": "{label}",
            onclick: move |_| app.navigate(v1.clone()),
            ondoubleclick: move |_| {
                if preview {
                    app.keep_tab(&v2);
                } else if let View::Agent(id) = &v2 {
                    app.ui.write().modal = Some(Modal::Rename(id.clone()));
                }
            },
            onmousedown: move |e| {
                if e.trigger_button() == Some(MouseButton::Auxiliary) && sub_tab {
                    e.prevent_default();
                    app.close_tab(&v3);
                }
            },
            oncontextmenu: move |e| {
                e.prevent_default();
                let c = e.client_coordinates();
                app.ui.write().tab_ctx = Some(TabCtx { view: v4.clone(), x: c.x, y: c.y });
            },
            span { class: "ic",
                if running {
                    super::Spinner { size: 14 }
                } else if sub.is_some() {
                    Icon { i: I::Orbit, size: 14, stroke: 1.5 }
                } else {
                    AgentGlyph { kind: agent }
                }
            }
            span { class: "t", dir: "auto", "{label}" }
            // The session's own tab stays; only subagent tabs close.
            if sub_tab {
                button { class: "x", title: "Close (Ctrl+W)",
                    onclick: move |e| {
                        e.stop_propagation();
                        app.close_tab(&v5);
                    },
                    Icon { i: I::X, size: 12, stroke: 1.8 }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ drop targets

/// Where a chat dragged from the sidebar or a tab will land, shown while
/// one is dragged over the window. Nothing to aim at: the drag engine reads
/// where the pointer is (an edge: side by side; near a composer: attach as
/// context; elsewhere: open here) and lights up that area.
#[component]
pub fn DropZones(target: Option<String>) -> Element {
    let app = use_app();
    let dragged = app.drag.read().clone();
    let Some(dragged) = dragged else { return rsx! {} };
    if target.as_deref() == Some(dragged.as_str()) {
        return rsx! {};
    }
    let t = target.clone().unwrap_or_default();
    let here = if target.is_some() { "Open here" } else { "Open this chat" };
    let context = if target.is_some() { "Attach as context" } else { "Start a new agent with it" };
    rsx! {
        div { class: if target.is_some() { "drop-overlay can-split" } else { "drop-overlay" }, "data-drop-target": "{t}",
            div { class: "drop-preview p-left", span { class: "drop-label", Icon { i: I::Columns, size: 14, stroke: 1.6 } "Open side by side" } }
            div { class: "drop-preview p-right", span { class: "drop-label", Icon { i: I::Columns, size: 14, stroke: 1.6 } "Open side by side" } }
            div { class: "drop-preview p-here", span { class: "drop-label", Icon { i: I::AppWindow, size: 14, stroke: 1.6 } "{here}" } }
            div { class: "drop-preview p-context", span { class: "drop-label", Icon { i: I::Link, size: 14, stroke: 1.6 } "{context}" } }
        }
    }
}

// ------------------------------------------------------------------ subagent tab

/// Background-task chips, then "Changes +N -M" and "Commit & Push ▾", above
/// the composer.
#[component]
fn AboveComposer(id: String) -> Element {
    let app = use_app();
    let mut menu = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let mut nonce = use_signal(|| 0u32);
    let id_m = id.clone();
    let info = use_memo(move || app.with_session(&id_m, |s| (s.cwd(), change_key(s), s.title.clone())));
    let running = app.is_active(&id);
    let (cwd, ck, title) = info().unwrap_or_default();
    let cwd2 = cwd.clone();
    let stats = use_resource(use_reactive!(|cwd2, ck, running| {
        let _ = (ck, running, nonce());
        async move {
            let c = cwd2?;
            tokio::task::spawn_blocking(move || {
                let p = Path::new(&c);
                if !git::is_repo(p) {
                    return None;
                }
                let st = git::status(p).ok()?;
                let (a, r) = st.totals();
                Some((st.files.len(), a, r))
            })
            .await
            .ok()
            .flatten()
        }
    }));
    let id_f = id.clone();
    let fallback = use_memo(move || {
        let _ = info();
        app.peek_session(&id_f, |s| s.diff_stats()).unwrap_or_default()
    });
    let id_t = id.clone();
    let tasks = use_memo(move || app.is_active(&id_t) && app.with_session(&id_t, |s| super::tasks::has_live(&s.messages)).unwrap_or(false));
    if info().is_none() {
        return rsx! {};
    }
    let git_stats = stats.read().clone().flatten();
    let (files, added, removed) = match git_stats {
        Some(g) => g,
        None => {
            let (a, r, f) = fallback();
            (f, a, r)
        }
    };
    // Changes and Commit wait for the agent; its background work shows
    // while it runs.
    let show_changes = files > 0 && !running;
    if !show_changes && !tasks() {
        return rsx! {};
    }
    let in_git = git_stats.is_some();
    let id_s = id.clone();
    let run = move |what: &'static str| {
        let Some(c) = cwd.clone() else { return };
        let t = title.clone();
        let body = app.peek_session(&id_s, |s| s.last_assistant_text().unwrap_or_default()).unwrap_or_default();
        busy.set(true);
        menu.set(false);
        spawn(async move {
            let r = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
                let p = Path::new(&c);
                let msg = commit_message(&t, &body);
                match what {
                    "commit" => git::commit(p, &msg, true),
                    "push" => {
                        git::commit(p, &msg, true)?;
                        git::push(p)
                    }
                    _ => {
                        let branch = git::current_branch(p).unwrap_or_default();
                        if branch == git::default_branch(p) {
                            git::checkout_new_branch(p, &format!("larrier/{}", git::branch_slug(&t)))?;
                        }
                        git::commit(p, &msg, true)?;
                        git::push(p)?;
                        git::create_pr(p, &t, &format!("{body}\n\n— Created with Larrier"), false)
                    }
                }
            })
            .await;
            busy.set(false);
            match r {
                Ok(Ok(out)) => app.toast(out.lines().last().unwrap_or("Done").to_string()),
                Ok(Err(e)) => app.toast_err(e.to_string()),
                Err(e) => app.toast_err(e.to_string()),
            }
            nonce += 1;
        });
    };
    let mut run2 = run.clone();
    let mut run3 = run.clone();
    let mut run4 = run.clone();
    let mut run = run;
    rsx! {
        div { class: "composer-wrap", style: "padding-bottom: 8px;",
            div { class: "row above-composer", style: "gap: 6px;",
                super::tasks::BackgroundChips { session_id: id.clone() }
                if show_changes {
                    button { class: "pill-btn", onclick: move |_| open_tool(app, RightTab::Changes),
                        "Changes"
                        span { class: "add num", "+{added}" }
                        span { class: "del num", "-{removed}" }
                    }
                    if in_git {
                        div { class: "menu-anchor",
                            div { class: "pill-btn split-btn",
                                button { class: "main", disabled: busy(), onclick: move |_| run("push"),
                                    if busy() { "Working…" } else { "Commit & Push" }
                                }
                                button { class: "chev", onclick: move |_| menu.set(true),
                                    Icon { i: I::ChevronDown, size: 12 }
                                }
                            }
                            Dropdown { open: menu, up: true,
                                MenuItem { label: "Commit".to_string(), icon: Some(I::Commit), onclick: move |_| run2("commit") }
                                MenuItem { label: "Commit & Push".to_string(), icon: Some(I::Upload), onclick: move |_| run3("push") }
                                MenuItem { label: "Create Pull Request".to_string(), icon: Some(I::PullRequest), onclick: move |_| run4("pr") }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::commit_message;

    #[test]
    fn commit_message_has_subject_and_summary_body() {
        let msg = commit_message("Add multiply to calc.py", "Added **multiply(a, b)**.\n\n```py\ndef multiply(a, b): ...\n```\n\nRan `pytest`: all green.\n");
        assert_eq!(msg, "Add multiply to calc.py\n\nAdded multiply(a, b).\n\nRan pytest: all green.");
    }

    #[test]
    fn commit_message_caps_the_subject() {
        let msg = commit_message(&"x".repeat(100), "");
        assert_eq!(msg.chars().count(), 72);
        assert!(msg.ends_with('…'));
        assert_eq!(commit_message("", ""), "Update from Larrier agent");
    }
}
