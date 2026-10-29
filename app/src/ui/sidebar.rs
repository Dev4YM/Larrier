//! Left sidebar, laid out like Cursor's Agents Window: navigation, pinned
//! agents, then every repository with its agents nested underneath.

use std::collections::BTreeMap;

use dioxus::prelude::*;
use larrier_core::{Part, Role, SessionStatus, SessionSummary, ToolStatus};

use super::AgentGlyph;
use crate::icons::{Icon, I};
use crate::state::{rel_time, use_app, user_name, App, CtxMenu, SettingsTab, View};

const PAGE: usize = 5;
const OPEN_IF_ACTIVE_WITHIN_DAYS: i64 = 7;

/// One repository (workspace) with its agents, newest first.
struct Repo {
    key: String,
    name: String,
    path: Option<String>,
    items: Vec<SessionSummary>,
    latest: chrono::DateTime<chrono::Utc>,
}

fn repo_key(s: &SessionSummary) -> String {
    match &s.workspace {
        Some(w) => w.path.replace('\\', "/").trim_end_matches('/').to_lowercase(),
        None => String::new(),
    }
}

fn build_repos(list: &[SessionSummary], workspaces: &[larrier_core::Workspace]) -> (Vec<SessionSummary>, Option<Repo>, Vec<Repo>) {
    let mut pinned = Vec::new();
    let mut map: BTreeMap<String, Repo> = BTreeMap::new();
    let mut no_repo = Repo { key: String::new(), name: "No Repo".into(), path: None, items: Vec::new(), latest: chrono::DateTime::<chrono::Utc>::MIN_UTC };
    for s in list.iter().filter(|s| !s.archived) {
        if s.pinned {
            pinned.push(s.clone());
        }
        let key = repo_key(s);
        if key.is_empty() {
            no_repo.latest = no_repo.latest.max(s.updated_at);
            no_repo.items.push(s.clone());
            continue;
        }
        let w = s.workspace.as_ref().unwrap();
        let e = map.entry(key.clone()).or_insert_with(|| Repo {
            key: key.clone(),
            name: if w.name.is_empty() { w.path.clone() } else { w.name.clone() },
            path: Some(w.path.clone()),
            items: Vec::new(),
            latest: s.updated_at,
        });
        e.latest = e.latest.max(s.updated_at);
        e.items.push(s.clone());
    }
    // Workspaces added in settings show even without agents yet.
    for w in workspaces {
        let key = w.path.replace('\\', "/").trim_end_matches('/').to_lowercase();
        map.entry(key.clone()).or_insert_with(|| Repo { key, name: w.name.clone(), path: Some(w.path.clone()), items: Vec::new(), latest: w.added_at });
    }
    let mut repos: Vec<Repo> = map.into_values().collect();
    repos.sort_by(|a, b| b.latest.cmp(&a.latest));
    for r in repos.iter_mut() {
        r.items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    }
    no_repo.items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    let no_repo = if no_repo.items.is_empty() { None } else { Some(no_repo) };
    (pinned, no_repo, repos)
}

/// What a running agent is doing right now ("Reading main.rs", "Thinking").
pub fn activity(app: &App, id: &str) -> Option<String> {
    let open = app.open.read();
    let s = open.get(id)?;
    let m = s.messages.last()?;
    if m.role == Role::User {
        return Some("Starting".into());
    }
    for p in m.parts.iter().rev() {
        match p {
            Part::Tool(t) if matches!(t.status, ToolStatus::Running | ToolStatus::Pending | ToolStatus::Approval) => {
                return Some(super::transcript::running_label(t));
            }
            Part::Reasoning { duration_ms: None, .. } => return Some("Thinking".into()),
            Part::Text { text } => return Some(larrier_core::lsf::preview_line(text, 80)),
            _ => {}
        }
    }
    Some("Working".into())
}

#[component]
pub fn Sidebar() -> Element {
    let mut app = use_app();
    let mut searching = use_signal(|| false);
    let mut dragging = use_signal(|| false);
    let query = app.ui.read().sidebar_query.to_lowercase();
    let running: Vec<String> = app.running.read().keys().cloned().collect();
    let all = app.sessions.read().clone();
    let workspaces = app.config.read().workspaces.clone();
    let filtered: Vec<SessionSummary> = if query.is_empty() {
        all
    } else {
        all.into_iter()
            .filter(|s| {
                s.title.to_lowercase().contains(&query)
                    || s.preview.to_lowercase().contains(&query)
                    || s.agent.display_name().to_lowercase().contains(&query)
                    || s.workspace.as_ref().map(|w| w.name.to_lowercase().contains(&query)).unwrap_or(false)
            })
            .collect()
    };
    let (pinned, no_repo, repos) = build_repos(&filtered, if query.is_empty() { &workspaces } else { &[] });
    let selected = app.current_id();
    let selected_repo = selected
        .as_ref()
        .and_then(|id| filtered.iter().find(|s| &s.id == id))
        .map(repo_key);
    let view = app.view.read().clone();
    let (can_back, can_fwd) = (app.can_go(-1), app.can_go(1));
    let show_search = searching() || !query.is_empty();
    let name = user_name();
    let initial = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
    let ctx = SideCtx { selected: selected.clone(), running: running.clone(), searching: !query.is_empty() };

    rsx! {
        aside { class: "sidebar",
            div { class: "side-top",
                button { class: "icon-btn", title: "Hide sidebar (Ctrl+B)",
                    onclick: move |_| app.ui.write().sidebar_open = false,
                    Icon { i: I::PanelLeft, size: 16, stroke: 1.5 }
                }
                button { class: if show_search { "icon-btn active" } else { "icon-btn" }, title: "Search agents",
                    onclick: move |_| {
                        let now = !searching();
                        searching.set(now);
                        if !now { app.ui.write().sidebar_query.clear(); }
                    },
                    Icon { i: I::Search, size: 15, stroke: 1.5 }
                }
                div { class: "grow" }
                button { class: "icon-btn", title: "Back", disabled: !can_back, onclick: move |_| app.go(-1),
                    Icon { i: I::ArrowLeft, size: 16, stroke: 1.5 }
                }
                button { class: "icon-btn", title: "Forward", disabled: !can_fwd, onclick: move |_| app.go(1),
                    Icon { i: I::ArrowRight, size: 16, stroke: 1.5 }
                }
            }
            if show_search {
                div { class: "side-search",
                    Icon { i: I::Search, size: 13 }
                    input {
                        placeholder: "Search agents",
                        autofocus: true,
                        value: "{app.ui.read().sidebar_query}",
                        oninput: move |e| app.ui.write().sidebar_query = e.value(),
                        onkeydown: move |e| if e.key() == Key::Escape { app.ui.write().sidebar_query.clear(); searching.set(false); },
                    }
                }
            }
            div { class: "side-scroll",
                div { class: "side-nav",
                    button { class: if view == View::Home { "nav-row on" } else { "nav-row" }, onclick: move |_| app.new_agent(),
                        span { class: "ic", Icon { i: I::SquarePen, size: 16, stroke: 1.5 } }
                        "New Agent"
                    }
                    button { class: if view == View::Automations { "nav-row on" } else { "nav-row" }, onclick: move |_| app.navigate(View::Automations),
                        span { class: "ic", Icon { i: I::CalendarClock, size: 16, stroke: 1.5 } }
                        "Automations"
                    }
                    button { class: if view == View::Customize { "nav-row on" } else { "nav-row" }, onclick: move |_| app.navigate(View::Customize),
                        span { class: "ic", Icon { i: I::Blocks, size: 16, stroke: 1.5 } }
                        "Customize"
                    }
                }
                if !pinned.is_empty() {
                    div { class: "side-sec",
                        div { class: "side-sec-head", span { class: "grow", "Pinned" } }
                        for s in pinned {
                            ChatRow { key: "pin-{s.id}", s: s.clone(), ctx: ctx.clone(), logo: true }
                        }
                    }
                }
                div { class: "side-sec",
                    div { class: "side-sec-head",
                        span { class: "grow", "Repositories" }
                        button { class: "icon-btn", title: "Group by status / workspace / agent", onclick: move |_| app.open_settings(SettingsTab::General),
                            Icon { i: I::Filter, size: 14, stroke: 1.5 }
                        }
                        button { class: "icon-btn", title: "Add a repository", onclick: move |_| app.pick_workspace(false),
                            Icon { i: I::FolderPlus, size: 15, stroke: 1.5 }
                        }
                    }
                    if repos.is_empty() && no_repo.is_none() {
                        div { style: "padding: 8px 12px; color: var(--fg-3); font-size: 12px; line-height: 18px;",
                            if query.is_empty() { "Add a folder, or import your agent sessions from Settings → Sessions." } else { "No matching agents" }
                        }
                    }
                    for (i, r) in repos.into_iter().enumerate() {
                        RepoGroup {
                            key: "{r.key}",
                            rkey: r.key.clone(),
                            name: r.name.clone(),
                            path: r.path.clone(),
                            items: r.items.clone(),
                            default_open: ctx.searching || selected_repo.as_deref() == Some(r.key.as_str()) || (i < 3 && (chrono::Utc::now() - r.latest).num_days() < OPEN_IF_ACTIVE_WITHIN_DAYS),
                            ctx: ctx.clone(),
                            home: false,
                        }
                    }
                    if let Some(r) = no_repo {
                        RepoGroup {
                            rkey: "~no-repo".to_string(),
                            name: r.name.clone(),
                            path: None,
                            items: r.items.clone(),
                            default_open: ctx.searching || selected_repo.as_deref() == Some(""),
                            ctx: ctx.clone(),
                            home: true,
                        }
                    }
                }
            }
            div { class: "side-profile",
                div { class: "avatar", "{initial}" }
                span { class: "nm", "{name}" }
                button { class: "icon-btn", title: "Import sessions from all agents", onclick: move |_| app.sync_imports(false),
                    Icon { i: I::Download, size: 15, stroke: 1.5 }
                }
                button { class: "icon-btn", title: "Settings (Ctrl+,)", onclick: move |_| app.open_settings(SettingsTab::General),
                    Icon { i: I::Settings, size: 16, stroke: 1.5 }
                }
            }
            div {
                class: if dragging() { "resizer dragging" } else { "resizer" },
                onmousedown: move |_| dragging.set(true),
            }
            if dragging() {
                div {
                    style: "position: fixed; inset: 0; z-index: 300; cursor: col-resize;",
                    onmousemove: move |e| {
                        let x = e.client_coordinates().x.clamp(220.0, 480.0) as u32;
                        app.config.write().settings.sidebar_width = x;
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

#[derive(Clone, PartialEq)]
struct SideCtx {
    selected: Option<String>,
    running: Vec<String>,
    searching: bool,
}

#[component]
fn RepoGroup(rkey: String, name: String, path: Option<String>, items: Vec<SessionSummary>, default_open: bool, ctx: SideCtx, home: bool) -> Element {
    let mut app = use_app();
    let mut more = use_signal(|| false);
    let open = {
        let ui = app.ui.read();
        if ui.repo_open.contains(&rkey) {
            true
        } else if ui.repo_closed.contains(&rkey) {
            false
        } else {
            default_open
        }
    };
    let k = rkey.clone();
    let count = items.len();
    let shown: Vec<SessionSummary> = if more() { items.clone() } else { items.iter().take(PAGE).cloned().collect() };
    let hidden = count.saturating_sub(shown.len());
    let title = path.clone().unwrap_or_else(|| "Agents and chats without a folder".into());
    rsx! {
        div { class: if open && !items.is_empty() { "repo-group" } else { "" },
            button { class: "repo-row", title: "{title}",
                onclick: move |_| {
                    let mut ui = app.ui.write();
                    if open {
                        ui.repo_open.remove(&k);
                        ui.repo_closed.insert(k.clone());
                    } else {
                        ui.repo_closed.remove(&k);
                        ui.repo_open.insert(k.clone());
                    }
                },
                span { class: "ic",
                    if home { Icon { i: I::Home, size: 16, stroke: 1.5 } }
                    else if open { Icon { i: I::FolderOpen, size: 16, stroke: 1.5 } }
                    else { Icon { i: I::Folder, size: 16, stroke: 1.5 } }
                }
                span { class: "nm", dir: "auto", "{name}" }
                if count > 0 { span { class: "cnt", "{count}" } }
            }
            if open {
                for s in shown {
                    ChatRow { key: "{s.id}", s: s.clone(), ctx: ctx.clone(), logo: false }
                }
                if hidden > 0 {
                    button { class: "show-more", onclick: move |_| more.set(true), "More" }
                }
            }
        }
    }
}

#[component]
fn ChatRow(s: SessionSummary, ctx: SideCtx, logo: bool) -> Element {
    let mut app = use_app();
    let running = ctx.running.contains(&s.id);
    let selected = ctx.selected.as_deref() == Some(s.id.as_str());
    let (id, id2, id3) = (s.id.clone(), s.id.clone(), s.id.clone());
    let mut cls = String::from("chat-row");
    if selected {
        cls.push_str(" selected");
    }
    if s.unread {
        cls.push_str(" unread");
    }
    match s.status {
        SessionStatus::NeedsInput => cls.push_str(" needs"),
        SessionStatus::Error if s.unread => cls.push_str(" error"),
        SessionStatus::Review if s.unread => cls.push_str(" review"),
        _ => {}
    }
    let time = rel_time(s.updated_at);
    let doing = if running { activity(&app, &s.id) } else { None };
    let tip = format!("{} · {}{}", s.agent.display_name(), s.title, if s.preview.is_empty() { String::new() } else { format!("\n{}", s.preview) });
    rsx! {
        div {
            class: "{cls}",
            role: "button",
            title: "{tip}",
            "data-ctx": "session",
            onclick: move |_| app.open_session(&id),
            oncontextmenu: move |e| {
                e.prevent_default();
                let c = e.client_coordinates();
                app.ui.write().ctx = Some(CtxMenu { session_id: id2.clone(), x: c.x, y: c.y });
            },
            span { class: "bullet",
                if running {
                    span { class: "sunburst", Icon { i: I::Loader, size: 13, stroke: 1.6 } }
                } else if logo {
                    AgentGlyph { kind: s.agent.clone() }
                } else {
                    span { class: "pt" }
                }
            }
            span { class: "t", dir: "auto", "{s.title}" }
            span { class: "tm", "{time}" }
            div { class: "more",
                button { class: "icon-btn sm", style: "background: var(--selected);",
                    onclick: move |e| {
                        e.stop_propagation();
                        let c = e.client_coordinates();
                        app.ui.write().ctx = Some(CtxMenu { session_id: id3.clone(), x: c.x, y: c.y });
                    },
                    Icon { i: I::More, size: 14 }
                }
            }
        }
        if let Some(d) = doing {
            div { class: "chat-sub", "{d}" }
        }
    }
}
