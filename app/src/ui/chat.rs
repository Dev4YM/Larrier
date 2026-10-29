//! The agent view: tab strip header, transcript + composer column (two side
//! by side in a split), the "On <repo>" tool list, the side panel (Changes /
//! Browser / Terminal / Files) and drop targets for dragged chats — laid out
//! like Cursor's Agents Window.

use std::path::Path;

use dioxus::html::input_data::MouseButton;
use dioxus::prelude::*;
use larrier_agents::git;
use larrier_core::{Environment, Session, ToolKind, ToolStatus};
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

#[component]
pub fn AgentView(id: String) -> Element {
    let mut app = use_app();
    let right_open = use_ui(|u| u.right_open)();
    let loaded = app.open.read().contains_key(&id);
    let split = app.split.read().clone().filter(|s| *s != id && app.open.read().contains_key(s));
    let mut dragging = use_signal(|| false);
    let id_m = id.clone();
    let ws_name = use_memo(move || app.with_session(&id_m, |s| s.workspace.as_ref().map(|w| w.name.clone())).flatten());
    if !loaded {
        return rsx! { div { class: "empty-state", "Loading…" } };
    }
    rsx! {
        div { class: if split.is_some() { "agent-body split" } else { "agent-body" },
            div {
                class: if right_open && split.is_none() { "chat-col primary fixed" } else { "chat-col primary" },
                ChatHeader { id: id.clone() }
                ChatColumn { id: id.clone(), input_id: "composer-input" }
                if right_open && split.is_none() {
                    div {
                        class: if dragging() { "resizer dragging" } else { "resizer" },
                        onmousedown: move |_| dragging.set(true),
                    }
                }
            }
            if let Some(other) = split.clone() {
                div { key: "split-{other}", class: "chat-col secondary",
                    SplitHeader { id: other.clone() }
                    ChatColumn { id: other.clone(), input_id: "composer-input-2" }
                }
            } else if right_open {
                RightPane { id: id.clone() }
            } else if let Some(name) = ws_name() {
                div { class: "tools-col",
                    div { style: "height: 38px;" }
                    div { class: "h", "On {name}" }
                    ToolLink { tab: RightTab::Changes, icon: I::Diff, label: "Changes" }
                    ToolLink { tab: RightTab::Browser, icon: I::Globe, label: "Browser" }
                    ToolLink { tab: RightTab::Terminal, icon: I::Terminal, label: "Terminal" }
                    ToolLink { tab: RightTab::Files, icon: I::Files, label: "Files" }
                }
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

/// Transcript, background tasks, changes bar and composer of one session.
#[component]
fn ChatColumn(id: String, input_id: &'static str) -> Element {
    rsx! {
        Transcript { key: "t-{id}", id: id.clone() }
        div { class: "composer-wrap", style: "padding-top: 0; padding-bottom: 0;",
            super::tasks::BackgroundTasks { session_id: id.clone() }
        }
        AboveComposer { id: id.clone() }
        Composer { session_id: Some(id.clone()), input_id }
    }
}

#[component]
fn ToolLink(tab: RightTab, icon: I, label: &'static str) -> Element {
    let app = use_app();
    rsx! {
        button { class: "tool-link", onclick: move |_| open_tool(app, tab),
            span { class: "ic", Icon { i: icon, size: 16, stroke: 1.5 } }
            "{label}"
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
    }
}

#[component]
fn ChatHeader(id: String) -> Element {
    let mut app = use_app();
    let id_m = id.clone();
    let info = use_memo(move || app.with_session(&id_m, head_info));
    let mut menu = use_signal(|| false);
    let mut export_menu = use_signal(|| false);
    let right_open = use_ui(|u| u.right_open)();
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
            if !right_open && app.split.read().is_none() {
                button { class: "icon-btn", title: "Show side panel (Ctrl+Shift+J)",
                    onclick: move |_| {
                        app.ui.write().right_open = true;
                        app.update_config(|c| c.settings.right_panel_open = true);
                    },
                    Icon { i: I::PanelRight, size: 16, stroke: 1.5 }
                }
            }
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
                    app.open_session_tab(&a);
                },
                Icon { i: I::Swap, size: 15, stroke: 1.5 }
            }
            button { class: "icon-btn", title: "Open in its own tab",
                onclick: move |_| {
                    app.split.set(None);
                    app.open_session_tab(&b);
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

/// The chat tabs, Cursor / VS Code style. Single-click opens a preview tab
/// (italic) that the next open replaces; double-click keeps it.
#[component]
pub fn TabStrip() -> Element {
    let app = use_app();
    let tabs = app.tabs.read().clone();
    let view = app.view.read().clone();
    let home = view == View::Home;
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
            button { class: if home { "icon-btn tab-new on" } else { "icon-btn tab-new" }, title: "New Agent (Ctrl+N)",
                onclick: move |_| app.new_agent(),
                Icon { i: I::Plus, size: 15, stroke: 1.6 }
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
        Some(s) => format!("{s} · subagent of {title}"),
        None => format!("{} · {title}{}", agent.display_name(), if tab.preview { "\nPreview — double-click to keep open" } else { "" }),
    };
    let preview = tab.preview;
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
                if e.trigger_button() == Some(MouseButton::Auxiliary) {
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
                    Icon { i: I::Bot, size: 14, stroke: 1.5 }
                } else {
                    AgentGlyph { kind: agent }
                }
            }
            span { class: "t", dir: "auto", "{label}" }
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

// ------------------------------------------------------------------ drop targets

/// Where a chat dragged from the sidebar or a tab can go. Shown while one
/// is being dragged; the hovered card lights up.
#[component]
pub fn DropZones(target: Option<String>) -> Element {
    let app = use_app();
    let dragged = app.drag.read().clone();
    let Some(dragged) = dragged else { return rsx! {} };
    if target.as_deref() == Some(dragged.as_str()) {
        return rsx! {};
    }
    let title = app.sessions.read().iter().find(|s| s.id == dragged).map(|s| s.title.clone()).unwrap_or_else(|| "this chat".into());
    let t = target.clone().unwrap_or_default();
    rsx! {
        div { class: "drop-overlay",
            div { class: "drop-panel",
                div { class: "drop-title", span { class: "ellipsis", "“{title}”" } }
                div { class: "drop-grid",
                    if target.is_some() {
                        DropCard { zone: "split", target: t.clone(), icon: I::Columns, label: "Side by side", sub: "Work in both chats at once".to_string() }
                    }
                    DropCard { zone: "tab", target: t.clone(), icon: I::CopyPlus, label: "Open in new tab", sub: if target.is_some() { "Keep this chat open too".to_string() } else { "Open it next to your tabs".to_string() } }
                    if target.is_some() {
                        DropCard { zone: "here", target: t.clone(), icon: I::AppWindow, label: "Open here", sub: "Replace this tab".to_string() }
                    }
                    DropCard { zone: "context", target: t.clone(), icon: I::Link, label: "Add as context", sub: if target.is_some() { "Reference it in your next message".to_string() } else { "Start a new agent with it".to_string() } }
                }
            }
        }
    }
}

#[component]
fn DropCard(zone: &'static str, target: String, icon: I, label: &'static str, sub: String) -> Element {
    rsx! {
        div { class: "drop-card", "data-drop": "{zone}", "data-drop-target": "{target}",
            span { class: "ic", Icon { i: icon, size: 20, stroke: 1.5 } }
            span { class: "lb", "{label}" }
            span { class: "sub", "{sub}" }
        }
    }
}

// ------------------------------------------------------------------ subagent tab

/// A subagent's transcript in its own tab (its header links back to the
/// parent chat).
#[component]
pub fn SubagentView(session: String, tool: String) -> Element {
    rsx! {
        div { class: "agent-body",
            div { class: "chat-col primary", style: "flex: 1;",
                div { class: "main-head",
                    SidebarToggle {}
                    TabStrip {}
                    DragRegion {}
                }
                super::tasks::SubagentTranscript { session_id: session.clone(), tool_id: tool.clone() }
            }
        }
    }
}

/// "Changes +N -M" and "Commit & Push ▾" above the composer.
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
    if files == 0 || running {
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
            div { class: "row", style: "gap: 6px;",
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
