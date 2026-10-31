//! Left sidebar, laid out like Cursor's Agents Window: navigation, pinned
//! agents, then every project with its agents nested underneath (or every
//! agent, or projects with their agents grouped by harness). Chats without
//! a project live in Home.

use std::cmp::Reverse;
use std::collections::BTreeMap;

use dioxus::prelude::*;
use larrier_core::{AgentKind, SessionStatus, SessionSummary, SidebarGroup, SidebarSort};

use dioxus::html::input_data::MouseButton;

use super::{AgentGlyph, Dropdown, MenuItem};
use crate::icons::{Icon, I};
use crate::keep::DRAFT_ID;
use crate::state::{project_key, rel_time, use_app, use_ui, user_name, CtxMenu, SettingsTab, View, HOME_KEY};

const PAGE: usize = 5;
const OPEN_IF_ACTIVE_WITHIN_DAYS: i64 = 7;

/// One group (a project, or an agent) with its chats.
struct Repo {
    key: String,
    name: String,
    path: Option<String>,
    items: Vec<SessionSummary>,
    latest: chrono::DateTime<chrono::Utc>,
    agent: Option<AgentKind>,
    home: bool,
}

fn sort_items(items: &mut [SessionSummary], sort: SidebarSort) {
    match sort {
        // A chat with a new message (a prompt, a reply, a turn from the
        // agent's own app) comes first; streaming within a reply doesn't
        // reorder anything.
        SidebarSort::Recent => items.sort_by(|a, b| b.activity().cmp(&a.activity())),
        SidebarSort::Created => items.sort_by(|a, b| b.created_at.cmp(&a.created_at)),
        SidebarSort::Name => items.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase())),
    }
    // The new agent draft heads its project.
    items.sort_by_key(|s| s.id != DRAFT_ID);
}

/// The new agent draft as a sidebar row: in the project it would start in.
fn draft_row(row: &crate::keep::HomeRow, d: &crate::state::Draft) -> SessionSummary {
    let ws = d.workspace.clone().map(larrier_core::WorkspaceRef::from_path);
    let mut s = larrier_core::Session::new(d.agent.clone(), ws).summary();
    s.id = DRAFT_ID.to_string();
    s.title = row.title.clone();
    s.created_at = row.at;
    s.updated_at = row.at;
    s.active_at = Some(row.at);
    s
}

/// Group by agent instead of repository.
fn build_agent_groups(list: &[SessionSummary], sort: SidebarSort) -> Vec<Repo> {
    let mut map: BTreeMap<String, Repo> = BTreeMap::new();
    for s in list {
        let e = map.entry(s.agent.id().to_string()).or_insert_with(|| Repo {
            key: format!("agent:{}", s.agent.id()),
            name: s.agent.display_name().to_string(),
            path: None,
            items: Vec::new(),
            latest: s.activity(),
            agent: Some(s.agent.clone()),
            home: false,
        });
        e.latest = e.latest.max(s.activity());
        e.items.push(s.clone());
    }
    let mut v: Vec<Repo> = map.into_values().collect();
    v.sort_by(|a, b| b.latest.cmp(&a.latest));
    for r in v.iter_mut() {
        sort_items(&mut r.items, sort);
    }
    v
}

fn repo_key(s: &SessionSummary) -> String {
    project_key(s.workspace.as_ref().map(|w| w.path.as_str()))
}

/// Projects with their chats (Home among them), in the user's order.
fn build_projects(list: &[SessionSummary], workspaces: &[larrier_core::Workspace], sort: SidebarSort, order: &[String]) -> (Vec<SessionSummary>, Vec<Repo>) {
    let mut pinned = Vec::new();
    let mut map: BTreeMap<String, Repo> = BTreeMap::new();
    let mut home = Repo { key: HOME_KEY.into(), name: "Home".into(), path: None, items: Vec::new(), latest: chrono::DateTime::<chrono::Utc>::MIN_UTC, agent: None, home: true };
    for s in list.iter() {
        if s.pinned {
            pinned.push(s.clone());
        }
        let key = repo_key(s);
        if key == HOME_KEY {
            home.latest = home.latest.max(s.activity());
            home.items.push(s.clone());
            continue;
        }
        let w = s.workspace.as_ref().unwrap();
        let e = map.entry(key.clone()).or_insert_with(|| Repo {
            key: key.clone(),
            name: if w.name.is_empty() { w.path.clone() } else { w.name.clone() },
            path: Some(w.path.clone()),
            items: Vec::new(),
            latest: s.activity(),
            agent: None,
            home: false,
        });
        e.latest = e.latest.max(s.activity());
        e.items.push(s.clone());
    }
    // Folders added in settings show even without agents yet.
    for w in workspaces {
        let key = project_key(Some(&w.path));
        if key != HOME_KEY {
            map.entry(key.clone()).or_insert_with(|| Repo { key, name: w.name.clone(), path: Some(w.path.clone()), items: Vec::new(), latest: w.added_at, agent: None, home: false });
        }
    }
    let mut projects: Vec<Repo> = map.into_values().collect();
    if !home.items.is_empty() {
        projects.push(home);
    }
    order_projects(&mut projects, order);
    for r in projects.iter_mut() {
        sort_items(&mut r.items, sort);
    }
    (pinned, projects)
}

/// The user's order (dragged in the sidebar); projects not in it yet come
/// first, newest first, and Home last.
fn order_projects(projects: &mut [Repo], order: &[String]) {
    projects.sort_by_key(|r| match order.iter().position(|k| *k == r.key) {
        Some(i) => (1, i, Reverse(r.latest)),
        None if r.home => (2, 0, Reverse(r.latest)),
        None => (0, 0, Reverse(r.latest)),
    });
}

/// A project's chats by agent, the most recently used agent first.
fn by_agent(items: &[SessionSummary]) -> Vec<(AgentKind, Vec<SessionSummary>)> {
    let mut groups: Vec<(AgentKind, Vec<SessionSummary>)> = Vec::new();
    for s in items {
        match groups.iter_mut().find(|(a, _)| *a == s.agent) {
            Some((_, v)) => v.push(s.clone()),
            None => groups.push((s.agent.clone(), vec![s.clone()])),
        }
    }
    groups.sort_by_key(|(_, v)| Reverse(v.iter().map(|s| s.activity()).max()));
    groups
}

#[component]
pub fn Sidebar(#[props(default)] parked: bool) -> Element {
    let mut app = use_app();
    let side_open = use_ui(|u| u.sidebar_open)();
    let mut searching = use_signal(|| false);
    let mut dragging = use_signal(|| false);
    let mut last_press = use_signal(|| None::<std::time::Instant>);
    let query_raw = use_ui(|u| u.sidebar_query.clone())();
    let query = query_raw.to_lowercase();
    let running: Vec<String> = app.active_ids();
    let all = app.sessions.read().clone();
    let workspaces = app.config.read().workspaces.clone();
    let (group, sort, show_archived, order) = {
        let c = app.config.read();
        (c.settings.sidebar_group, c.settings.sidebar_sort, c.settings.show_archived, c.settings.project_order.clone())
    };
    let mut filter_menu = use_signal(|| false);
    // A harness turned off in Settings takes its chats out of the list too.
    let off: Vec<larrier_core::AgentKind> = app.config.read().agents.iter().filter(|a| !a.enabled).map(|a| a.kind.clone()).collect();
    let all: Vec<SessionSummary> = all.into_iter().filter(|s| (show_archived || !s.archived) && !off.contains(&s.agent)).collect();
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
    let mut rest: Vec<SessionSummary> = filtered.clone();
    // A message left on the new agent screen, as a draft.
    if let Some(row) = crate::keep::HOME_ROW() {
        let d = draft_row(&row, &app.draft.read());
        if query.is_empty() || d.title.to_lowercase().contains(&query) {
            rest.insert(0, d);
        }
    }
    let (pinned, repos) = if group == SidebarGroup::Agent {
        let pinned: Vec<SessionSummary> = rest.iter().filter(|s| s.pinned).cloned().collect();
        (pinned, build_agent_groups(&rest, sort))
    } else {
        build_projects(&rest, if query.is_empty() { &workspaces } else { &[] }, sort, &order)
    };
    let group_label = match group {
        SidebarGroup::Agent => "Agents",
        _ => "Projects",
    };
    let mixed = group == SidebarGroup::Mixed;
    // Projects can be dragged into another order (not agents, not while
    // searching).
    let draggable = group != SidebarGroup::Agent && query.is_empty();
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
        aside { class: if parked { "sidebar parked" } else if side_open { "sidebar" } else { "sidebar closed" }, "aria-hidden": if side_open && !parked { "false" } else { "true" },
            div { class: "side-top",
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
                        value: "{query_raw}",
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
                        span { class: "grow", "{group_label}" }
                        div { class: "menu-anchor",
                            button { class: if filter_menu() { "icon-btn active" } else { "icon-btn" }, title: "Group, sort and filter", onclick: move |_| filter_menu.set(true),
                                Icon { i: I::Filter, size: 14, stroke: 1.5 }
                            }
                            Dropdown { open: filter_menu, up: false, right: true,
                                div { class: "menu-label", "Group by" }
                                MenuItem { label: "Project".to_string(), checked: matches!(group, SidebarGroup::Workspace | SidebarGroup::Status), onclick: move |_| app.update_config(|c| c.settings.sidebar_group = SidebarGroup::Workspace) }
                                MenuItem { label: "Agent".to_string(), checked: group == SidebarGroup::Agent, onclick: move |_| app.update_config(|c| c.settings.sidebar_group = SidebarGroup::Agent) }
                                MenuItem { label: "Mixed".to_string(), sub: Some("Project, then agent".to_string()), checked: mixed, onclick: move |_| app.update_config(|c| c.settings.sidebar_group = SidebarGroup::Mixed) }
                                div { class: "menu-sep" }
                                div { class: "menu-label", "Sort by" }
                                MenuItem { label: "Recent activity".to_string(), checked: sort == SidebarSort::Recent, onclick: move |_| app.update_config(|c| c.settings.sidebar_sort = SidebarSort::Recent) }
                                MenuItem { label: "Date created".to_string(), checked: sort == SidebarSort::Created, onclick: move |_| app.update_config(|c| c.settings.sidebar_sort = SidebarSort::Created) }
                                MenuItem { label: "Name".to_string(), checked: sort == SidebarSort::Name, onclick: move |_| app.update_config(|c| c.settings.sidebar_sort = SidebarSort::Name) }
                                div { class: "menu-sep" }
                                MenuItem { label: "Show archived".to_string(), checked: show_archived, onclick: move |_| app.update_config(|c| c.settings.show_archived = !c.settings.show_archived) }
                                div { class: "menu-sep" }
                                MenuItem { label: "Mark all as read".to_string(), icon: Some(I::CheckCheck), onclick: move |_| app.mark_all_read() }
                            }
                        }
                        button { class: "icon-btn", title: "Add a project", onclick: move |_| app.pick_workspace(false),
                            Icon { i: I::FolderPlus, size: 15, stroke: 1.5 }
                        }
                    }
                    if repos.is_empty() {
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
                            default_open: ctx.searching || selected_repo.as_deref() == Some(r.key.as_str()) || r.items.iter().any(|s| s.id == DRAFT_ID) || (!r.home && i < 3 && (chrono::Utc::now() - r.latest).num_days() < OPEN_IF_ACTIVE_WITHIN_DAYS),
                            ctx: ctx.clone(),
                            home: r.home,
                            agent: r.agent.clone(),
                            mixed,
                            draggable,
                        }
                    }
                }
            }
            div { class: "side-profile",
                div { class: "avatar", "{initial}" }
                span { class: "nm", "{name}" }
                // Only when there is an update: download it (the label slides
                // out on hover), its progress, then Restart.
                {
                    let upd = app.update.read().clone();
                    match upd {
                        crate::state::UpdateState::Available(r) => rsx! {
                            button { class: "update-btn", title: "Download update · Larrier {r.version}", onclick: move |_| app.download_update(),
                                Icon { i: I::Download, size: 15, stroke: 1.6 }
                                span { class: "lbl", "Download" }
                            }
                        },
                        crate::state::UpdateState::Downloading { version, got, total } => rsx! {
                            button { class: "update-btn busy", title: "Downloading Larrier {version}…", disabled: true,
                                super::loaders::Orbit { size: 13 }
                                span { class: "lbl", {if total > 0 { format!("{}%", got * 100 / total) } else { "Downloading".to_string() }} }
                            }
                        },
                        crate::state::UpdateState::Ready { version, .. } => rsx! {
                            button { class: "update-btn ready", title: "Restart to update to Larrier {version}", onclick: move |_| app.restart_to_update(),
                                Icon { i: I::Refresh, size: 14, stroke: 1.8 }
                                span { class: "lbl", "Restart" }
                            }
                        },
                        _ => rsx! {},
                    }
                }
                button { class: "icon-btn", title: "Settings (Ctrl+,)", onclick: move |_| app.open_settings(SettingsTab::General),
                    Icon { i: I::Settings, size: 16, stroke: 1.5 }
                }
            }
            div {
                class: if dragging() { "resizer dragging" } else { "resizer" },
                title: "Drag to resize · double-click to fit the window",
                onmousedown: move |_| {
                    if super::second_press(&mut last_press.write()) {
                        let w = super::auto_sidebar_width(app.ui.peek().win_w);
                        app.update_config(|c| c.settings.sidebar_width = w);
                    } else {
                        dragging.set(true);
                    }
                },
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
fn RepoGroup(rkey: String, name: String, path: Option<String>, items: Vec<SessionSummary>, default_open: bool, ctx: SideCtx, home: bool, agent: Option<AgentKind>, mixed: bool, draggable: bool) -> Element {
    let mut app = use_app();
    let rk = rkey.clone();
    let user_choice = use_ui(move |u| if u.repo_open.contains(&rk) { Some(true) } else if u.repo_closed.contains(&rk) { Some(false) } else { None })();
    let open = user_choice.unwrap_or(default_open);
    let k = rkey.clone();
    let count = items.iter().filter(|s| s.id != DRAFT_ID).count();
    let title = path.clone().unwrap_or_else(|| if agent.is_some() { name.clone() } else { "Chats without a project".into() });
    let drag = if draggable { rkey.clone() } else { String::new() };
    rsx! {
        div { class: if open && !items.is_empty() { "repo-group open" } else { "repo-group" }, "data-project": "{drag}",
            div { class: if open { "repo-row open" } else { "repo-row" }, role: "button", title: "{title}", "data-drag-project": "{drag}",
                onclick: move |_| app.set_group_open(&k, !open),
                // The folder icon turns into the collapse chevron on hover.
                span { class: "ic",
                    span { class: "ic-rest",
                        if let Some(k) = agent.clone() { AgentGlyph { kind: k } }
                        else if home { Icon { i: I::Home, size: 16, stroke: 1.5 } }
                        else if open { Icon { i: I::FolderOpen, size: 16, stroke: 1.5 } }
                        else { Icon { i: I::Folder, size: 16, stroke: 1.5 } }
                    }
                    span { class: "ic-chev", Icon { i: I::ChevronRight, size: 15, stroke: 1.6 } }
                }
                span { class: "nm", dir: "auto", "{name}" }
                if count > 0 { span { class: "cnt", "{count}" } }
                if agent.is_none() {
                    button { class: "icon-btn sm add", title: if path.is_some() { "New agent in {name}" } else { "New agent in Home" },
                        onclick: {
                            let p = path.clone();
                            move |e: MouseEvent| {
                                e.stop_propagation();
                                app.draft.write().workspace = p.clone();
                                app.new_agent();
                            }
                        },
                        Icon { i: I::Plus, size: 15, stroke: 1.6 }
                    }
                }
            }
            super::Collapse { open,
                if mixed && agent.is_none() {
                    for (a, list) in by_agent(&items) {
                        AgentSub { key: "{a.id()}", pkey: rkey.clone(), agent: a.clone(), items: list, ctx: ctx.clone() }
                    }
                } else {
                    Rows { items: items.clone(), ctx: ctx.clone() }
                }
            }
        }
    }
}

/// One agent's chats inside a project (Mixed grouping).
#[component]
fn AgentSub(pkey: String, agent: AgentKind, items: Vec<SessionSummary>, ctx: SideCtx) -> Element {
    let mut app = use_app();
    let key = format!("{pkey}|{}", agent.id());
    let k2 = key.clone();
    let open = !use_ui(move |u| u.repo_closed.contains(&k2))();
    let count = items.iter().filter(|s| s.id != DRAFT_ID).count();
    rsx! {
        div { class: "agent-sub",
            div { class: if open { "agent-sub-row open" } else { "agent-sub-row" }, role: "button",
                onclick: move |_| app.set_group_open(&key, !open),
                span { class: "chev", Icon { i: I::ChevronRight, size: 12, stroke: 1.7 } }
                span { class: "lg", AgentGlyph { kind: agent.clone() } }
                span { class: "nm", "{agent.display_name()}" }
                span { class: "cnt", "{count}" }
            }
            super::Collapse { open,
                Rows { items: items.clone(), ctx: ctx.clone() }
            }
        }
    }
}

/// A list of chats, the first few then "More".
#[component]
fn Rows(items: Vec<SessionSummary>, ctx: SideCtx) -> Element {
    let mut more = use_signal(|| false);
    let shown: Vec<SessionSummary> = if more() { items.clone() } else { items.iter().take(PAGE).cloned().collect() };
    let hidden = items.len().saturating_sub(shown.len());
    rsx! {
        for s in shown {
            ChatRow { key: "{s.id}", s: s.clone(), ctx: ctx.clone(), logo: false }
        }
        if hidden > 0 {
            button { class: "show-more", onclick: move |_| more.set(true), "More" }
        }
    }
}

/// The new agent draft: gray, marked [Draft]; it opens the new agent screen
/// with the message as it was left.
#[component]
fn DraftRow(s: SessionSummary, logo: bool) -> Element {
    let mut app = use_app();
    let here = *app.view.read() == View::Home;
    let tip = format!("Draft · {}\n{}", s.agent.display_name(), s.title);
    rsx! {
        div {
            class: if here { "chat-row draft selected" } else { "chat-row draft" },
            role: "button",
            title: "{tip}",
            onclick: move |_| app.new_agent(),
            oncontextmenu: move |e| e.prevent_default(),
            span { class: "bullet",
                if logo { AgentGlyph { kind: s.agent.clone() } } else { span { class: "pt" } }
            }
            span { class: "t", dir: "auto",
                span { class: "draft-tag", "[Draft] " }
                "{s.title}"
            }
            if !here {
                div { class: "more",
                    button { class: "icon-btn sm", style: "background: var(--selected);", title: "Discard the draft",
                        onclick: move |e| {
                            e.stop_propagation();
                            crate::keep::discard_home();
                        },
                        Icon { i: I::X, size: 13 }
                    }
                }
            }
        }
    }
}

#[component]
fn ChatRow(s: SessionSummary, ctx: SideCtx, logo: bool) -> Element {
    if s.id == DRAFT_ID {
        return rsx! { DraftRow { s, logo } };
    }
    let mut app = use_app();
    let running = ctx.running.contains(&s.id);
    let selected = ctx.selected.as_deref() == Some(s.id.as_str());
    let (id, id2, id3) = (s.id.clone(), s.id.clone(), s.id.clone());
    let mut cls = String::from("chat-row");
    if selected {
        cls.push_str(" selected");
    }
    // Just started here: it slides into its project.
    if chrono::Utc::now() - s.created_at < chrono::Duration::seconds(8) {
        cls.push_str(" fresh");
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
    let (id4, id5) = (s.id.clone(), s.id.clone());
    let tip = format!("{} · {}{}", s.agent.display_name(), s.title, if s.preview.is_empty() { String::new() } else { format!("\n{}", s.preview) });
    rsx! {
        div {
            class: "{cls}",
            role: "button",
            title: "{tip}",
            "data-ctx": "session",
            "data-drag-session": "{s.id}",
            "data-drag-title": "{s.title}",
            // Click: show it. Ctrl / middle click: side by side.
            onclick: move |e| {
                if e.modifiers().ctrl() || e.modifiers().meta() {
                    app.open_split(&id);
                } else {
                    app.open_session(&id);
                }
            },
            ondoubleclick: move |_| app.open_session(&id4),
            onmousedown: move |e| {
                if e.trigger_button() == Some(MouseButton::Auxiliary) {
                    e.prevent_default();
                    app.open_split(&id5);
                }
            },
            oncontextmenu: move |e| {
                e.prevent_default();
                let c = e.client_coordinates();
                app.ui.write().ctx = Some(CtxMenu { session_id: id2.clone(), x: c.x, y: c.y });
            },
            span { class: "bullet",
                if running {
                    super::loaders::Orbit { size: 14 }
                } else if logo {
                    AgentGlyph { kind: s.agent.clone() }
                    if s.unread { span { class: "unread-dot" } }
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
    }
}
