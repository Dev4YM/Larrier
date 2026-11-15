//! Left sidebar: navigation, pinned agents, then every project with its
//! agents nested underneath (or every agent, or projects with their agents
//! grouped by harness). Chats without a project live in Home.

use std::cmp::Reverse;
use std::collections::BTreeMap;

use dioxus::prelude::*;
use larrier_core::{AgentKind, SessionStatus, SessionSummary, SidebarGroup, SidebarSort};

use dioxus::html::input_data::MouseButton;

use super::{AgentGlyph, Dropdown, MenuItem};
use crate::icons::{Icon, I};
use crate::keep::DRAFT_ID;
use crate::state::{project_key, rel_time, use_app, use_ui, CtxMenu, SettingsTab, View, HOME_KEY};

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
    /// A project's look and place (see `larrier_core::Workspace`).
    icon: Option<String>,
    color: Option<String>,
    pinned: bool,
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
            icon: None,
            color: None,
            pinned: false,
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
    let mut home = Repo {
        key: HOME_KEY.into(),
        name: "Home".into(),
        path: None,
        items: Vec::new(),
        latest: chrono::DateTime::<chrono::Utc>::MIN_UTC,
        agent: None,
        home: true,
        icon: None,
        color: None,
        pinned: false,
    };
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
        // A chat started in one of a project's folders is that project's.
        let project = larrier_core::config::project_in(workspaces, &w.path);
        let key = project.map(|p| project_key(Some(&p.path))).unwrap_or(key);
        let e = map.entry(key.clone()).or_insert_with(|| match project {
            Some(p) => Repo { key: key.clone(), name: p.name.clone(), path: Some(p.path.clone()), items: Vec::new(), latest: s.activity(), agent: None, home: false, icon: p.icon.clone(), color: p.color.clone(), pinned: p.pinned },
            None => Repo {
                key: key.clone(),
                name: if w.name.is_empty() { w.path.clone() } else { w.name.clone() },
                path: Some(w.path.clone()),
                items: Vec::new(),
                latest: s.activity(),
                agent: None,
                home: false,
                icon: None,
                color: None,
                pinned: false,
            },
        });
        e.latest = e.latest.max(s.activity());
        e.items.push(s.clone());
    }
    // Folders added in settings show even without agents yet.
    for w in workspaces {
        let key = project_key(Some(&w.path));
        if key != HOME_KEY {
            map.entry(key.clone()).or_insert_with(|| Repo {
                key,
                name: w.name.clone(),
                path: Some(w.path.clone()),
                items: Vec::new(),
                latest: w.added_at,
                agent: None,
                home: false,
                icon: w.icon.clone(),
                color: w.color.clone(),
                pinned: w.pinned,
            });
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

/// Pinned projects first; then the user's order (dragged in the sidebar),
/// projects not in it yet first, newest first, and Home last.
fn order_projects(projects: &mut [Repo], order: &[String]) {
    projects.sort_by_key(|r| {
        let place = match order.iter().position(|k| *k == r.key) {
            Some(i) => (1, i, Reverse(r.latest)),
            None if r.home => (2, 0, Reverse(r.latest)),
            None => (0, 0, Reverse(r.latest)),
        };
        (!r.pinned, place)
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
    // A search left typed (in a past launch too) stays open.
    let mut searching = use_signal(|| !app.ui.peek().sidebar_query.trim().is_empty());
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
    let mut scroll_gen = use_signal(|| 0u64);
    // Scrolled down: the pinned nav at the top shows its edge.
    let mut scrolled = use_signal(|| false);
    let mut me_menu = use_signal(|| false);
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
    // Room for the box beside the icon (up to Back), or a row of its own.
    let roomy = app.config.read().settings.sidebar_width >= 262;
    let focus_find = || document::eval("setTimeout(() => (document.querySelector('.side-find.open input') || document.querySelector('.side-find-row.open input'))?.focus(), 40)");
    let name = crate::state::display_name(&app.config.read());
    let ctx = SideCtx { selected: selected.clone(), running: running.clone(), searching: !query.is_empty() };

    rsx! {
        aside { class: if parked { "sidebar parked" } else if side_open { "sidebar" } else { "sidebar closed" }, "aria-hidden": if side_open && !parked { "false" } else { "true" },
            div { class: "side-top",
                div { class: if show_search && roomy { "side-find open" } else { "side-find" },
                    button { class: if show_search && !roomy { "icon-btn active" } else { "icon-btn" }, title: "Search agents", tabindex: if show_search && roomy { "-1" } else { "0" },
                        onclick: move |_| {
                            searching.set(true);
                            focus_find();
                        },
                        Icon { i: I::Search, size: 15, stroke: 1.5 }
                    }
                    if roomy {
                        input {
                            placeholder: "Search agents",
                            tabindex: if show_search { "0" } else { "-1" },
                            value: "{query_raw}",
                            oninput: move |e| app.ui.write().sidebar_query = e.value(),
                            onkeydown: move |e| if e.key() == Key::Escape { app.ui.write().sidebar_query.clear(); searching.set(false); },
                            onblur: move |_| if app.ui.peek().sidebar_query.trim().is_empty() { searching.set(false); },
                        }
                    }
                }
                div { class: "grow" }
                button { class: "icon-btn", title: "Back", disabled: !can_back, onclick: move |_| app.go(-1),
                    Icon { i: I::ArrowLeft, size: 16, stroke: 1.5 }
                }
                button { class: "icon-btn", title: "Forward", disabled: !can_fwd, onclick: move |_| app.go(1),
                    Icon { i: I::ArrowRight, size: 16, stroke: 1.5 }
                }
            }
            if !roomy {
                div { class: if show_search { "side-find-row open" } else { "side-find-row" },
                    div { class: "side-search",
                        Icon { i: I::Search, size: 13 }
                        input {
                            placeholder: "Search agents",
                            tabindex: if show_search { "0" } else { "-1" },
                            value: "{query_raw}",
                            oninput: move |e| app.ui.write().sidebar_query = e.value(),
                            onkeydown: move |e| if e.key() == Key::Escape { app.ui.write().sidebar_query.clear(); searching.set(false); },
                            onblur: move |_| if app.ui.peek().sidebar_query.trim().is_empty() { searching.set(false); },
                        }
                    }
                }
            }
            div { class: "side-scroll",
                // Scrolled where it was left, in a past launch too.
                onmounted: move |_| {
                    if let Some(y) = crate::keep::value::<f64>(crate::keep::WINDOW, "sidebar.scroll") {
                        document::eval(&format!("requestAnimationFrame(() => {{ const s = document.querySelector('.side-scroll'); if (s) s.scrollTop = {y}; }})"));
                    }
                },
                onscroll: move |e| {
                    scroll_gen += 1;
                    let (gen, y) = (*scroll_gen.peek(), e.data().scroll_top());
                    if *scrolled.peek() != (y > 0.0) {
                        scrolled.set(y > 0.0);
                    }
                    spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                        if *scroll_gen.peek() == gen {
                            crate::keep::set_value(crate::keep::WINDOW, "sidebar.scroll", &(y > 0.0).then_some(y));
                        }
                    });
                },
                div { class: if scrolled() { "side-nav stuck" } else { "side-nav" },
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
                        button { class: "icon-btn", title: "New project", onclick: move |_| app.edit_project(None),
                            Icon { i: I::FolderPlus, size: 15, stroke: 1.5 }
                        }
                    }
                    if repos.is_empty() {
                        div { style: "padding: 8px 12px; color: var(--fg-3); font-size: var(--ui-sm); line-height: 18px;",
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
                            icon: r.icon.clone(),
                            color: r.color.clone(),
                            pinned: r.pinned,
                        }
                    }
                }
            }
            div { class: "side-profile",
                // You: one badge, its menu has your profile, the settings
                // pages you go to most and help.
                {
                    let outdated = crate::agent_versions::outdated(&app);
                    let go = move |tab: SettingsTab| {
                        let mut m = me_menu;
                        m.set(false);
                        app.open_settings(tab);
                    };
                    rsx! {
                        div { class: "menu-anchor side-me-anchor",
                            button { class: if me_menu() { "side-me open" } else { "side-me" }, title: if outdated > 0 { "You and Settings · agent updates available" } else { "You and Settings" },
                                onclick: move |_| me_menu.set(true),
                                span { class: "side-me-av",
                                    super::profile::Avatar { size: 28 }
                                    if outdated > 0 { span { class: "gear-dot" } }
                                }
                                span { class: "nm", "{name}" }
                            }
                            Dropdown { open: me_menu, up: true, class: "me-menu".to_string(),
                                MenuItem { label: "Profile".to_string(), icon: Some(I::CircleUser), onclick: move |_| go(SettingsTab::Profile) }
                                MenuItem { label: "Settings".to_string(), icon: Some(I::Settings), sub: Some(crate::platform::keys("Ctrl+,")), onclick: move |_| go(SettingsTab::General) }
                                MenuItem { label: "Plan & Usage".to_string(), icon: Some(I::Gauge), onclick: move |_| go(SettingsTab::Usage) }
                                MenuItem { label: "Appearance".to_string(), icon: Some(I::Sun), onclick: move |_| go(SettingsTab::Appearance) }
                                MenuItem { label: "Harnesses".to_string(), icon: Some(I::Bot), sub: (outdated > 0).then(|| format!("{outdated} update{}", if outdated == 1 { "" } else { "s" })), onclick: move |_| go(SettingsTab::Agents) }
                                div { class: "menu-sep" }
                                MenuItem { label: "What's new".to_string(), icon: Some(I::Sparkle),
                                    onclick: move |_| { me_menu.set(false); app.ui.write().modal = Some(crate::state::Modal::WhatsNew(crate::version::VERSION.to_string(), false)); } }
                                MenuItem { label: "About Larrier".to_string(), icon: Some(I::Info), onclick: move |_| go(SettingsTab::About) }
                            }
                        }
                    }
                }
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
fn RepoGroup(
    rkey: String,
    name: String,
    path: Option<String>,
    items: Vec<SessionSummary>,
    default_open: bool,
    ctx: SideCtx,
    home: bool,
    agent: Option<AgentKind>,
    mixed: bool,
    draggable: bool,
    #[props(default)] icon: Option<String>,
    #[props(default)] color: Option<String>,
    #[props(default)] pinned: bool,
) -> Element {
    let app = use_app();
    let mut menu = use_signal(|| false);
    // Right-clicked: its menu at the pointer.
    let mut ctx_at = use_signal(|| None::<(f64, f64)>);
    let ctx_id = use_hook(|| format!("pjctx-{}", crate::state::rand_id()));
    let ctx_id2 = ctx_id.clone();
    let at = ctx_at();
    use_effect(use_reactive!(|at| {
        if let Some((x, y)) = at {
            document::eval(&super::place_at_js(&ctx_id2, x, y, true));
        }
    }));
    let rk = rkey.clone();
    let user_choice = use_ui(move |u| if u.repo_open.contains(&rk) { Some(true) } else if u.repo_closed.contains(&rk) { Some(false) } else { None })();
    let open = user_choice.unwrap_or(default_open);
    let k = rkey.clone();
    let count = items.iter().filter(|s| s.id != DRAFT_ID).count();
    let title = path.clone().unwrap_or_else(|| if agent.is_some() { name.clone() } else { "Chats without a project".into() });
    let drag = if draggable { rkey.clone() } else { String::new() };
    rsx! {
        div { class: if open && !items.is_empty() { "repo-group open" } else { "repo-group" }, "data-project": "{drag}",
            div { class: if open { "repo-row open" } else { "repo-row" }, role: "button", title: "{title}", "data-drag-project": "{drag}", "data-ctx": "project",
                onclick: move |_| app.set_group_open(&k, !open),
                oncontextmenu: move |e| {
                    e.prevent_default();
                    if agent.is_none() {
                        let c = e.client_coordinates();
                        ctx_at.set(Some((c.x, c.y)));
                    }
                },
                // The folder icon turns into the collapse chevron on hover.
                span { class: "ic",
                    span { class: "ic-rest",
                        if let Some(k) = agent.clone() { AgentGlyph { kind: k } }
                        else if home { Icon { i: I::Home, size: 16, stroke: 1.5 } }
                        else { super::projects::ProjectMark { icon: icon.clone(), color: color.clone(), open } }
                    }
                    span { class: "ic-chev", Icon { i: I::ChevronRight, size: 15, stroke: 1.6 } }
                }
                span { class: "nm", dir: "auto", "{name}" }
                if pinned { span { class: "pin-mark", title: "Pinned", Icon { i: I::Pin, size: 12, stroke: 1.6 } } }
                if count > 0 { span { class: "cnt", "{count}" } }
                if agent.is_none() {
                    div { class: "acts",
                        if let Some(p) = path.clone() {
                            div { class: "menu-anchor",
                                button { class: "icon-btn sm", title: "Project options",
                                    onclick: move |e: MouseEvent| { e.stop_propagation(); menu.set(true); },
                                    Icon { i: I::More, size: 15 }
                                }
                                super::Dropdown { open: menu, up: false, right: true,
                                    ProjectActions { path: Some(p.clone()), pinned, onclose: move |_| menu.set(false) }
                                }
                            }
                        }
                        button { class: "icon-btn sm", title: if path.is_some() { "New agent in {name}" } else { "New agent in Home" },
                            onclick: {
                                let p = path.clone();
                                move |e: MouseEvent| {
                                    e.stop_propagation();
                                    app.pick_project(p.clone());
                                    app.new_agent();
                                }
                            },
                            Icon { i: I::Plus, size: 15, stroke: 1.6 }
                        }
                    }
                }
            }
            if let Some((x, y)) = at {
                div { class: "menu-backdrop", onclick: move |_| ctx_at.set(None), oncontextmenu: move |e| { e.prevent_default(); ctx_at.set(None); } }
                div { id: "{ctx_id}", class: "menu ctx-menu", style: "left: {x}px; top: {y}px; visibility: hidden;", onclick: move |e| e.stop_propagation(),
                    ProjectActions { path: path.clone(), pinned, onclose: move |_| ctx_at.set(None) }
                }
            }
            super::Collapse { open,
                if mixed && agent.is_none() {
                    for (a, list) in by_agent(&items) {
                        AgentSub { key: "{a.id()}", pkey: rkey.clone(), agent: a.clone(), items: list, ctx: ctx.clone() }
                    }
                } else {
                    Rows { group: rkey.clone(), items: items.clone(), ctx: ctx.clone() }
                }
            }
        }
    }
}

/// What can be done with a project (Home when `path` is `None`): from its
/// "…" button and from a right-click on it.
#[component]
fn ProjectActions(path: Option<String>, pinned: bool, onclose: EventHandler<()>) -> Element {
    let app = use_app();
    let stored = path.as_deref().is_some_and(|p| app.config.read().workspace_by_path(p).is_some());
    let p = path.clone().unwrap_or_default();
    let (p1, p2, p3, p4, p5, p6) = (p.clone(), p.clone(), p.clone(), p.clone(), p.clone(), p.clone());
    rsx! {
        MenuItem { label: "New chat".to_string(), icon: Some(I::SquarePen),
            onclick: { let path = path.clone(); move |_| { onclose.call(()); app.pick_project(path.clone()); app.new_agent(); } } }
        if path.is_some() {
            MenuItem { label: if pinned { "Unpin project".to_string() } else { "Pin project".to_string() }, icon: Some(if pinned { I::PinOff } else { I::Pin }),
                onclick: move |_| { onclose.call(()); app.toggle_project_pin(&p1); } }
            MenuItem { label: "Edit project…".to_string(), icon: Some(I::Pencil), onclick: move |_| { onclose.call(()); app.edit_project(Some(&p2)); } }
            div { class: "menu-sep" }
            MenuItem { label: "Open folder".to_string(), icon: Some(I::FolderOpen), onclick: move |_| { onclose.call(()); let _ = ::open::that(&p3); } }
            MenuItem { label: "Open in Terminal".to_string(), icon: Some(I::Terminal), onclick: move |_| { onclose.call(()); let _ = crate::state::open_terminal(&p4, if cfg!(windows) { "cd ." } else { "true" }); } }
            MenuItem { label: "Copy path".to_string(), icon: Some(I::Copy), onclick: move |_| { onclose.call(()); app.copy_text(&p5); } }
            if stored {
                div { class: "menu-sep" }
                MenuItem { label: "Remove project".to_string(), icon: Some(I::Trash), danger: true, title: Some("Its chats stay, listed by their folder".to_string()),
                    onclick: move |_| {
                        onclose.call(());
                        let p = p6.clone();
                        app.update_config(move |c| {
                            let n = larrier_core::config::normalize_path(&p);
                            c.workspaces.retain(|w| larrier_core::config::normalize_path(&w.path) != n);
                        });
                        app.toast("Project removed; its chats stay");
                    } }
            }
        }
    }
}

/// One agent's chats inside a project (Mixed grouping).
#[component]
fn AgentSub(pkey: String, agent: AgentKind, items: Vec<SessionSummary>, ctx: SideCtx) -> Element {
    let app = use_app();
    let key = format!("{pkey}|{}", agent.id());
    let k2 = key.clone();
    let group = key.clone();
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
                Rows { group, items: items.clone(), ctx: ctx.clone() }
            }
        }
    }
}

/// A list of chats, the first few then "More" (shown all from then on).
#[component]
fn Rows(group: String, items: Vec<SessionSummary>, ctx: SideCtx) -> Element {
    let mut app = use_app();
    let g = group.clone();
    let more = use_ui(move |u| u.expanded_sections.contains(&g))();
    let shown: Vec<SessionSummary> = if more { items.clone() } else { items.iter().take(PAGE).cloned().collect() };
    let hidden = items.len().saturating_sub(shown.len());
    rsx! {
        for s in shown {
            ChatRow { key: "{s.id}", s: s.clone(), ctx: ctx.clone(), logo: false }
        }
        if hidden > 0 {
            button { class: "show-more", onclick: move |_| { app.ui.write().expanded_sections.insert(group.clone()); }, "More" }
        }
    }
}

/// The new agent draft: gray, marked [Draft]; it opens the new agent screen
/// with the message as it was left.
#[component]
fn DraftRow(s: SessionSummary, logo: bool) -> Element {
    let app = use_app();
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
    // Which agent, and the title in full (the row may cut it short).
    let tip = format!("{} · {}", s.agent.display_name(), s.title);
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
                button { class: "icon-btn sm", title: if s.pinned { "Unpin" } else { "Pin" },
                    onclick: { let id = s.id.clone(); move |e: MouseEvent| { e.stop_propagation(); app.toggle_pin(&id); } },
                    Icon { i: if s.pinned { I::PinOff } else { I::Pin }, size: 13 }
                }
                button { class: "icon-btn sm",
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
