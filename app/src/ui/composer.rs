//! The prompt composer, shaped like Cursor's: a pill with a "+" menu, the
//! input, one agent + model picker (brand logos) and send / stop, with the
//! branch and environment pickers underneath.

use dioxus::prelude::*;
use larrier_core::{lsf::short_path, AgentKind, Part, Protocol};

use super::{Dropdown, MenuItem};
use crate::icons::{Icon, I};
use crate::logos::{agent_logo, model_logo, Logo};
use crate::state::{use_app, App, SettingsTab};

/// Model choices for an agent (Chat agent lists provider models).
pub fn model_options(app: &App, agent: &AgentKind) -> Vec<String> {
    let cfg = app.config.read();
    if agent.spec().protocol == Protocol::HttpChat {
        return cfg
            .providers
            .iter()
            .filter(|p| p.enabled)
            .flat_map(|p| p.models.iter().map(move |m| format!("{}/{}", p.id, m)))
            .collect();
    }
    let mut v: Vec<String> = match agent {
        AgentKind::Custom(id) => cfg.custom_agent(id).map(|c| c.models.clone()).unwrap_or_default(),
        _ => agent.spec().models.iter().map(|s| s.to_string()).collect(),
    };
    for m in cfg.agent(agent).extra_models {
        if !v.contains(&m) {
            v.push(m);
        }
    }
    v
}

pub fn mode_label(mode: &str) -> String {
    match mode {
        "agent" => "Agent".into(),
        "plan" => "Plan".into(),
        "ask" => "Ask".into(),
        "auto" => "Auto".into(),
        "bypass" => "Bypass permissions".into(),
        "read-only" => "Read only".into(),
        "auto-edit" => "Auto edit".into(),
        "yolo" => "YOLO".into(),
        "build" => "Build".into(),
        other => {
            let mut c = other.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
    }
}

fn model_display(agent: &AgentKind, model: &Option<String>) -> String {
    match model {
        Some(m) if agent.spec().protocol == Protocol::HttpChat => m.split_once('/').map(|(_, b)| b.to_string()).unwrap_or(m.clone()),
        Some(m) if m != "default" && m != "auto" => m.clone(),
        _ => "Auto".into(),
    }
}

const TIPS: &[&str] = &[
    "Type @ to mention a file, / for commands",
    "Continue any session with another agent from the model picker",
    "Ctrl+K searches every agent and chat you have",
    "Import ChatGPT and Claude.ai exports from File → Import File",
    "Multitask runs one prompt on several agents in parallel",
    "Right-click anything for Larrier's menu",
    "Every session is a portable .lsf file — export it from the … menu",
];

#[component]
pub fn Composer(session_id: Option<String>, #[props(default = false)] home: bool) -> Element {
    let mut app = use_app();
    let mut text = use_signal(String::new);
    let attachments: Signal<Vec<Part>> = use_signal(Vec::new);
    let plus_menu = use_signal(|| false);
    let mut model_menu = use_signal(|| false);
    let env_menu = use_signal(|| false);
    let mut ws_menu = use_signal(|| false);
    let mut multi_menu = use_signal(|| false);
    let mut model_query = use_signal(String::new);
    let mut multi: Signal<Vec<AgentKind>> = use_signal(Vec::new);
    let tip_idx = use_hook(|| (chrono::Utc::now().timestamp() as usize) % TIPS.len());

    // Focus when asked (Ctrl+L, new agent, opening a session).
    let focus = app.ui.read().focus_composer;
    use_effect(use_reactive!(|focus| {
        let _ = focus;
        document::eval("setTimeout(() => document.getElementById('composer-input')?.focus(), 30)");
    }));

    // Workspace file list for @-mentions (indexed in the background).
    let file_root: Option<String> = match &session_id {
        Some(id) => app.open.read().get(id).and_then(|s| s.cwd()),
        None => app.draft.read().workspace.clone(),
    };
    let fr2 = file_root.clone();
    let file_index = use_resource(use_reactive!(|fr2| async move {
        let Some(root) = fr2 else { return Vec::new() };
        tokio::task::spawn_blocking(move || list_files(&root, 30_000)).await.unwrap_or_default()
    }));
    let fr3 = file_root.clone();
    let branch = use_resource(use_reactive!(|fr3| async move {
        let root = fr3?;
        tokio::task::spawn_blocking(move || larrier_agents::git::current_branch(std::path::Path::new(&root))).await.ok().flatten()
    }));

    // Current selections: from the session, or the new-agent draft.
    let (agent, model, mode, worktree, usage) = match &session_id {
        Some(id) => match app.open.read().get(id) {
            Some(s) => (
                s.agent.clone(),
                s.model.clone(),
                s.mode.clone(),
                matches!(s.environment, larrier_core::Environment::Worktree { .. }),
                Some(s.usage.clone()),
            ),
            None => return rsx! {},
        },
        None => {
            let d = app.draft.read();
            (d.agent.clone(), d.model.clone(), d.mode.clone(), d.worktree, None)
        }
    };
    let running = session_id.as_ref().map(|id| app.is_running(id)).unwrap_or(false);
    let modes: Vec<String> = agent.spec().modes.iter().map(|s| s.to_string()).collect();
    let mode_now = mode.clone().or_else(|| modes.first().cloned()).unwrap_or_else(|| "agent".into());
    let ctrl_enter = app.config.read().settings.send_with_ctrl_enter;
    let placeholder = if home { "Plan, Build, / for commands, @ for context" } else { "Send follow-up" };
    let atts = attachments.read().clone();
    let can_send = !text.read().trim().is_empty() || !atts.is_empty();
    let quote = app.ui.read().quote.clone();
    let detected = app.detected.read().clone();
    let multi_now = multi.read().clone();
    let tall = home || text.read().contains('\n') || text.read().len() > 90 || !atts.is_empty() || quote.is_some();
    let model_logo_svg = model.as_deref().and_then(model_logo).or_else(|| agent_logo(&agent));
    let model_text = model_display(&agent, &model);
    let env_text = if worktree { "Worktree" } else { "This PC" };

    // "@file" mentions and "/command" suggestions.
    let mention = mention_query(&text.read());
    let mention_hits: Vec<String> = match &mention {
        Some(q) => {
            let ql = q.to_lowercase();
            let idx = file_index.read();
            let mut hits: Vec<String> = idx.as_ref().map(|v| v.iter().filter(|p| p.to_lowercase().contains(&ql)).take(200).cloned().collect()).unwrap_or_default();
            hits.sort_by_key(|p| (!p.rsplit('/').next().unwrap_or(p).to_lowercase().starts_with(&ql), p.len()));
            hits.truncate(8);
            hits
        }
        None => Vec::new(),
    };
    let slash = slash_query(&text.read());
    let slash_hits: Vec<(&'static str, &'static str)> = match &slash {
        Some(q) => SLASH.iter().filter(|(c, _)| c[1..].starts_with(q.as_str())).filter(|(c, _)| home || !matches!(*c, "/worktree" | "/local")).copied().collect(),
        None => Vec::new(),
    };
    let first_mention = mention_hits.first().cloned();
    let first_slash = slash_hits.first().map(|s| s.0);

    // Copy-able helpers so every menu row and key handler can hold one.
    let set_mode = move |m: Option<String>, sid: Option<String>| {
        let mut app = app;
        match &sid {
            Some(id) => app.set_session_mode(id, m),
            None => app.draft.write().mode = m,
        }
    };
    let run_slash = move |cmd: &str, sid: Option<String>| {
        let mut app = app;
        let mut text = text;
        text.set(String::new());
        match cmd {
            "/plan" | "/agent" | "/ask" => set_mode(Some(cmd[1..].to_string()), sid),
            "/new" => app.new_agent(),
            "/fork" => if let Some(id) = &sid { app.fork(id, None) },
            "/export" => if let Some(id) = &sid { app.export(id, larrier_formats::ExportFormat::LsfJsonl) },
            "/terminal" => if let Some(id) = &sid { app.open_in_terminal(id) },
            "/worktree" => app.draft.write().worktree = true,
            "/local" => app.draft.write().worktree = false,
            "/settings" => app.open_settings(SettingsTab::General),
            _ => {}
        }
    };
    let pick_mention = move |path: String| {
        let mut text = text;
        let t = text.read().clone();
        let cut = t.rfind('@').unwrap_or(t.len());
        text.set(format!("{}@{} ", &t[..cut], path));
        document::eval("setTimeout(() => document.getElementById('composer-input')?.focus(), 10)");
    };
    // Choose agent (+ model). On an existing session this ports it.
    let choose = move |k: AgentKind, m: Option<String>, sid: Option<String>| {
        let mut app = app;
        match &sid {
            Some(id) => {
                let current = app.open.read().get(id).map(|s| s.agent.clone());
                if current.as_ref() != Some(&k) {
                    app.port(id, k);
                }
                app.set_session_model(id, m);
            }
            None => {
                let mut d = app.draft.write();
                if d.agent != k {
                    d.mode = None;
                    d.account_id = None;
                }
                d.agent = k;
                d.model = m;
            }
        }
    };
    let do_send = move |sid: Option<String>| {
        let mut app = app;
        let mut text = text;
        let mut at = attachments;
        let t = text.read().clone();
        let a = at.read().clone();
        if t.trim().is_empty() && a.is_empty() {
            return;
        }
        text.set(String::new());
        at.set(Vec::new());
        let agents = multi.read().clone();
        if sid.is_none() && agents.len() > 1 {
            // Multitask: the same prompt on several agents, side by side.
            let keep = app.draft.read().clone();
            for k in agents {
                {
                    let mut d = app.draft.write();
                    d.agent = k;
                    d.model = None;
                    d.mode = keep.mode.clone();
                }
                app.send(None, t.clone(), a.clone());
            }
            *app.draft.write() = keep;
            app.toast("Started the prompt on every selected agent");
            return;
        }
        app.send(sid, t, a);
    };
    let sid_send = session_id.clone();
    let sid_key = session_id.clone();
    let (modes_k, mode_k) = (modes.clone(), mode_now.clone());
    let sid_stop = session_id.clone();

    let mq = model_query.read().to_lowercase();
    let workspaces = app.config.read().workspaces.clone();
    let ws_now = app.draft.read().workspace.clone();
    let ws_label = ws_now
        .as_ref()
        .map(|p| std::path::Path::new(p).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or(p.clone()))
        .unwrap_or_else(|| "No Repo".into());
    let is_repo = branch.read().clone().flatten().is_some();

    // The agent + model picker menu.
    let picker_menu = rsx! {
        Dropdown { open: model_menu, up: !home, right: true,
            div { class: "menu-search",
                Icon { i: I::Search, size: 12 }
                input {
                    placeholder: "Search agents and models, or type a model id…",
                    value: "{model_query}",
                    autofocus: true,
                    oninput: move |e| model_query.set(e.value()),
                    onkeydown: {
                        let sid = session_id.clone();
                        let a = agent.clone();
                        move |e: KeyboardEvent| {
                            let q = model_query.read().trim().to_string();
                            if e.key() == Key::Enter && !q.is_empty() {
                                model_menu.set(false);
                                choose(a.clone(), Some(q), sid.clone());
                            }
                        }
                    },
                }
            }
            for spec in larrier_core::agent::runnable() {
                {
                    let det = detected.iter().find(|d| d.kind == spec.kind).cloned();
                    let installed = spec.binaries.is_empty() || det.as_ref().map(|d| d.installed()).unwrap_or(false);
                    let models: Vec<String> = std::iter::once("default".to_string()).chain(model_options(&app, &spec.kind).into_iter().filter(|m| m != "default" && m != "auto")).collect();
                    let agent_match = mq.is_empty() || spec.name.to_lowercase().contains(&mq);
                    let shown: Vec<String> = models.into_iter().filter(|m| agent_match || m.to_lowercase().contains(&mq)).collect();
                    let sub = match (&det, spec.binaries.is_empty()) {
                        (_, true) => "API".to_string(),
                        (Some(d), _) if d.installed() => d.version.clone().map(|v| format!("v{v}")).unwrap_or_default(),
                        (Some(_), _) => "not installed".to_string(),
                        _ => String::new(),
                    };
                    let kind = spec.kind.clone();
                    let sid = session_id.clone();
                    let current_agent = agent.clone();
                    let current_model = model.clone();
                    let logo = agent_logo(&spec.kind);
                    rsx! {
                        if !shown.is_empty() && (installed || !mq.is_empty()) {
                            div { key: "{spec.kind.id()}", class: "menu-label", style: "display: flex; align-items: center; gap: 6px; padding-top: 8px;",
                                if let Some(svg) = logo { Logo { svg, size: 13 } }
                                span { style: "color: var(--fg-2); font-weight: 500;", "{spec.name}" }
                                span { style: "margin-left: auto;", "{sub}" }
                            }
                            for m in shown {
                                {
                                    let k = kind.clone();
                                    let sid = sid.clone();
                                    let val = if m == "default" { None } else { Some(m.clone()) };
                                    let label = if m == "default" { "Auto".to_string() } else { m.clone() };
                                    let checked = current_agent == kind && (current_model.as_deref() == val.as_deref());
                                    let mlogo = val.as_deref().and_then(model_logo);
                                    rsx! {
                                        button { key: "{kind.id()}-{m}", class: "menu-item", style: "padding-left: 26px;",
                                            onclick: move |_| {
                                                model_menu.set(false);
                                                choose(k.clone(), val.clone(), sid.clone());
                                            },
                                            span { class: "lg", if let Some(svg) = mlogo { Logo { svg, size: 13 } } }
                                            span { class: "ellipsis", "{label}" }
                                            if checked { span { class: "check", Icon { i: I::Check, size: 13 } } }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            for c in app.config.read().custom_agents.clone() {
                {
                    let k = AgentKind::Custom(c.id.clone());
                    let sid = session_id.clone();
                    rsx! {
                        MenuItem { key: "{c.id}", label: c.name.clone(), sub: Some("custom".to_string()), checked: k == agent,
                            onclick: move |_| { model_menu.set(false); choose(k.clone(), None, sid.clone()); } }
                    }
                }
            }
            div { class: "menu-sep" }
            MenuItem { label: "Manage agents & models…".to_string(), icon: Some(I::Settings), onclick: move |_| { model_menu.set(false); app.open_settings(SettingsTab::Agents); } }
        }
    };

    rsx! {
        div { class: "composer-wrap",
            if home {
                div { class: "row", style: "gap: 2px; margin: 0 0 8px 4px;",
                    div { class: "menu-anchor",
                        button { class: "env-chip", onclick: move |_| ws_menu.set(true),
                            span { "{ws_label}" }
                            Icon { i: I::ChevronDown, size: 12 }
                        }
                        Dropdown { open: ws_menu, up: false,
                            div { class: "menu-label", "Repository" }
                            for w in workspaces.clone() {
                                MenuItem {
                                    key: "{w.id}",
                                    label: w.name.clone(),
                                    sub: Some(short_path(&w.path)),
                                    icon: Some(I::Folder),
                                    checked: ws_now.as_deref() == Some(w.path.as_str()),
                                    onclick: {
                                        let p = w.path.clone();
                                        move |_| { ws_menu.set(false); app.draft.write().workspace = Some(p.clone()); }
                                    },
                                }
                            }
                            MenuItem { label: "No Repo".to_string(), icon: Some(I::Home), checked: ws_now.is_none(),
                                onclick: move |_| { ws_menu.set(false); app.draft.write().workspace = None; } }
                            div { class: "menu-sep" }
                            MenuItem { label: "Open Folder…".to_string(), icon: Some(I::FolderOpen), onclick: move |_| { ws_menu.set(false); app.pick_workspace(true); } }
                        }
                    }
                    EnvPicker { open: env_menu, worktree, is_repo, session: false, label: env_text.to_string() }
                }
            }
            if !mention_hits.is_empty() {
                div { class: "menu up", style: "left: 28px; bottom: calc(100% - 4px); min-width: 340px;",
                    div { class: "menu-label", "Files" }
                    for (i, p) in mention_hits.iter().enumerate() {
                        {
                            let p2 = p.clone();
                            let (dir, name) = match p.rsplit_once('/') { Some((d, n)) => (d.to_string(), n.to_string()), None => (String::new(), p.clone()) };
                            rsx! {
                                button { key: "{p}", class: if i == 0 { "menu-item hl" } else { "menu-item" },
                                    onmousedown: move |e| { e.prevent_default(); pick_mention(p2.clone()); },
                                    span { class: "muted", Icon { i: I::File, size: 13 } }
                                    span { class: "ellipsis", "{name}" }
                                    span { class: "sub ellipsis", style: "max-width: 200px;", "{dir}" }
                                }
                            }
                        }
                    }
                }
            }
            if !slash_hits.is_empty() {
                div { class: "menu up", style: "left: 28px; bottom: calc(100% - 4px); min-width: 300px;",
                    div { class: "menu-label", "Commands" }
                    for (i, (c, d)) in slash_hits.iter().copied().enumerate() {
                        button { key: "{c}", class: if i == 0 { "menu-item hl" } else { "menu-item" },
                            onmousedown: {
                                let sid = session_id.clone();
                                move |e: MouseEvent| { e.prevent_default(); run_slash(c, sid.clone()); }
                            },
                            span { class: "mono", style: "font-size: 12px;", "{c}" }
                            span { class: "sub", "{d}" }
                        }
                    }
                }
            }
            div { class: format!("composer{}{}", if tall { " tall" } else { "" }, if home { " home-c" } else { "" }),
                if let Some(q) = quote {
                    div { class: "quote-chip",
                        Icon { i: I::Quote, size: 13 }
                        span { class: "tx", "{q}" }
                        button { class: "icon-btn sm", onclick: move |_| app.ui.write().quote = None, Icon { i: I::X, size: 11 } }
                    }
                }
                if !atts.is_empty() {
                    div { class: "attachments",
                        for (i, a) in atts.iter().enumerate() {
                            span { key: "{i}", class: "attach-chip",
                                Icon { i: if matches!(a, Part::Image { .. }) { I::Image } else { I::Paperclip }, size: 11 }
                                span { class: "ellipsis",
                                    match a {
                                        Part::File { path, .. } => short_path(path),
                                        Part::Image { source: larrier_core::lsf::MediaSource::Path { path }, .. } => short_path(path),
                                        _ => "attachment".to_string(),
                                    }
                                }
                                button { class: "icon-btn sm",
                                    onclick: move |_| { let mut at = attachments; at.write().remove(i); },
                                    Icon { i: I::X, size: 11 }
                                }
                            }
                        }
                    }
                }
                div { class: "composer-row",
                    if !home {
                        PlusMenu { open: plus_menu, attachments, session_id: session_id.clone(), modes: modes.clone(), mode_now: mode_now.clone(), text }
                    }
                    textarea {
                        id: "composer-input",
                        rows: if home { "3" } else { "1" },
                        placeholder: "{placeholder}",
                        value: "{text}",
                        oninput: move |e| text.set(e.value()),
                        onkeydown: move |e| {
                            if matches!(e.key(), Key::Enter | Key::Tab) && !e.modifiers().shift() {
                                if let Some(p) = first_mention.clone() {
                                    e.prevent_default();
                                    pick_mention(p);
                                    return;
                                }
                                if let Some(c) = first_slash {
                                    e.prevent_default();
                                    run_slash(c, sid_key.clone());
                                    return;
                                }
                            }
                            if e.key() == Key::Tab && e.modifiers().shift() && !modes_k.is_empty() {
                                // Shift+Tab cycles the agent's modes (Agent → Plan → …).
                                e.prevent_default();
                                let i = modes_k.iter().position(|m| *m == mode_k).unwrap_or(0);
                                set_mode(Some(modes_k[(i + 1) % modes_k.len()].clone()), sid_key.clone());
                                return;
                            }
                            if e.key() == Key::Enter {
                                let m = e.modifiers();
                                let wants = if ctrl_enter { m.ctrl() || m.meta() } else { !m.shift() };
                                if wants && !running {
                                    e.prevent_default();
                                    do_send(sid_key.clone());
                                }
                            }
                        },
                    }
                    if home {
                        PlusMenu { open: plus_menu, attachments, session_id: None, modes: modes.clone(), mode_now: mode_now.clone(), text }
                        div { class: "spacer" }
                    }
                    if mode_now != "agent" && mode_now != "build" && !modes.is_empty() {
                        span { class: "picker", style: "color: var(--blue); cursor: default;", title: "Shift+Tab to change mode",
                            Icon { i: if mode_now == "plan" { I::List } else { I::Message }, size: 13 }
                            "{mode_label(&mode_now)}"
                        }
                    }
                    div { class: "menu-anchor",
                        button { class: if model_menu() { "picker open" } else { "picker" }, title: "{agent.display_name()}",
                            onclick: move |_| { model_query.set(String::new()); model_menu.set(true); },
                            if let Some(svg) = model_logo_svg { Logo { svg, size: 14 } }
                            span { class: "lb sub", "{agent.display_name()}" }
                            span { class: "lb", "{model_text}" }
                            span { class: "chev", Icon { i: I::ChevronDown, size: 12 } }
                        }
                        {picker_menu}
                    }
                    if running {
                        button { class: "send-btn stop", title: "Stop (Ctrl+.)",
                            onclick: move |_| if let Some(id) = &sid_stop { app.stop(id) },
                            Icon { i: I::Stop, size: 10 }
                        }
                    } else {
                        button { class: if can_send { "send-btn ready" } else { "send-btn" }, disabled: !can_send, title: "Send (Enter)",
                            onclick: move |_| do_send(sid_send.clone()),
                            Icon { i: I::ArrowUp, size: 15, stroke: 2.2 }
                        }
                    }
                }
            }
            if home {
                div { class: "row", style: "gap: 6px; margin: 10px 0 0 2px; flex-wrap: wrap;",
                    button {
                        class: "pill-btn",
                        style: if mode_now == "plan" { "border-color: var(--blue); color: var(--blue);" } else { "" },
                        onclick: move |_| set_mode(Some(if mode_now == "plan" { "agent".into() } else { "plan".into() }), None),
                        "Plan New Idea"
                        span { class: "faint", style: "font-size: 11px;", "⇧Tab" }
                    }
                    div { class: "menu-anchor",
                        button {
                            class: "pill-btn",
                            style: if multi_now.len() > 1 { "border-color: var(--blue); color: var(--blue);" } else { "" },
                            onclick: move |_| multi_menu.set(true),
                            "Multitask"
                            if multi_now.len() > 1 { span { "· {multi_now.len()} agents" } }
                        }
                        Dropdown { open: multi_menu, up: false,
                            div { class: "menu-label", "Run the prompt on several agents at once" }
                            for spec in larrier_core::agent::runnable().filter(|s| s.binaries.is_empty() || detected.iter().any(|d| d.kind == s.kind && d.installed())) {
                                {
                                    let k = spec.kind.clone();
                                    let on = multi_now.contains(&k);
                                    let logo = agent_logo(&k);
                                    rsx! {
                                        button { key: "{k.id()}", class: "menu-item",
                                            onclick: move |e| {
                                                e.stop_propagation();
                                                let mut v = multi.write();
                                                if let Some(i) = v.iter().position(|x| *x == k) { v.remove(i); } else { v.push(k.clone()); }
                                            },
                                            span { class: "lg", if let Some(svg) = logo { Logo { svg, size: 14 } } }
                                            span { "{spec.name}" }
                                            if on { span { class: "check", Icon { i: I::Check, size: 13 } } }
                                        }
                                    }
                                }
                            }
                            div { class: "menu-sep" }
                            MenuItem { label: "Clear".to_string(), onclick: move |_| { multi.set(Vec::new()); multi_menu.set(false); } }
                        }
                    }
                }
            } else {
                div { class: "below-composer",
                    if let Some(b) = branch.read().clone().flatten() {
                        span { class: "env-chip", style: "cursor: default;", title: "Current branch", Icon { i: I::GitBranch, size: 13, stroke: 1.5 } "{b}" }
                    }
                    EnvPicker { open: env_menu, worktree, is_repo, session: true, label: env_text.to_string() }
                    div { class: "grow" }
                    if let Some(u) = usage.filter(|u| u.total_tokens() > 0) {
                        span { class: "faint", style: "font-size: 11px; margin-right: 2px;",
                            title: "Tokens used in this agent",
                            "{super::transcript::human_count(u.total_tokens())}"
                            if let Some(c) = u.cost_usd { " · ${c:.2}" }
                        }
                    }
                    span { class: if running { "ring busy" } else { "ring" }, title: if running { "Working" } else { "Idle" } }
                }
            }
            if home {
                div { style: "position: fixed; bottom: 22px; left: var(--sidebar-w, 300px); right: 0; text-align: center; color: var(--fg-3); font-size: 12px; pointer-events: none;",
                    "{TIPS[tip_idx]}"
                }
            }
        }
    }
}

/// The "+" menu: attachments, mentions, mode, commands.
#[component]
fn PlusMenu(open: Signal<bool>, attachments: Signal<Vec<Part>>, session_id: Option<String>, modes: Vec<String>, mode_now: String, text: Signal<String>) -> Element {
    let mut app = use_app();
    let mut open = open;
    let mut text = text;
    rsx! {
        div { class: "menu-anchor",
            button { class: if open() { "plus-btn open" } else { "plus-btn" }, title: "Add context, change mode", onclick: move |_| open.set(true),
                Icon { i: I::Plus, size: 16, stroke: 1.6 }
            }
            Dropdown { open, up: true,
                MenuItem { label: "Attach files or images".to_string(), icon: Some(I::Paperclip), onclick: move |_| { open.set(false); app.attach_files(attachments); } }
                MenuItem { label: "Mention a file".to_string(), sub: Some("@".to_string()), icon: Some(I::AtSign), onclick: move |_| {
                    open.set(false);
                    let t = text.read().clone();
                    text.set(if t.is_empty() || t.ends_with(' ') { format!("{t}@") } else { format!("{t} @") });
                    document::eval("setTimeout(() => document.getElementById('composer-input')?.focus(), 10)");
                } }
                if modes.len() > 1 {
                    div { class: "menu-sep" }
                    div { class: "menu-label", "Mode" }
                    for m in modes.clone() {
                        {
                            let sid = session_id.clone();
                            let m2 = m.clone();
                            rsx! {
                                MenuItem { key: "{m}", label: mode_label(&m), checked: m == mode_now,
                                    icon: Some(if m == "plan" { I::List } else if m == "ask" { I::Message } else { I::Infinity }),
                                    onclick: move |_| {
                                        open.set(false);
                                        match &sid {
                                            Some(id) => app.set_session_mode(id, Some(m2.clone())),
                                            None => app.draft.write().mode = Some(m2.clone()),
                                        }
                                    } }
                            }
                        }
                    }
                }
                if let Some(id) = session_id.clone() {
                    div { class: "menu-sep" }
                    {
                        let (a, b, c) = (id.clone(), id.clone(), id.clone());
                        rsx! {
                            MenuItem { label: "Fork this agent".to_string(), icon: Some(I::Fork), onclick: move |_| { open.set(false); app.fork(&a, None); } }
                            MenuItem { label: "Open in the agent's CLI".to_string(), icon: Some(I::Terminal), onclick: move |_| { open.set(false); app.open_in_terminal(&b); } }
                            MenuItem { label: "Export session".to_string(), icon: Some(I::Upload), onclick: move |_| { open.set(false); app.export(&c, larrier_formats::ExportFormat::LsfJsonl); } }
                        }
                    }
                }
            }
        }
    }
}

/// "This PC ▾" / "Worktree ▾" environment picker.
#[component]
fn EnvPicker(open: Signal<bool>, worktree: bool, is_repo: bool, session: bool, label: String) -> Element {
    let mut app = use_app();
    let mut open = open;
    rsx! {
        div { class: "menu-anchor",
            button { class: "env-chip", onclick: move |_| open.set(true),
                Icon { i: if worktree { I::Layers } else { I::Laptop }, size: 14, stroke: 1.5 }
                span { "{label}" }
                Icon { i: I::ChevronDown, size: 12 }
            }
            Dropdown { open, up: session,
                div { class: "menu-label", "Run on" }
                MenuItem { label: "This PC".to_string(), sub: Some("in the folder".to_string()), icon: Some(I::Laptop), checked: !worktree,
                    disabled: session,
                    onclick: move |_| { open.set(false); app.draft.write().worktree = false; } }
                MenuItem { label: "Worktree".to_string(), sub: Some(if is_repo { "isolated git checkout".to_string() } else { "needs a git repo".to_string() }), icon: Some(I::Layers), checked: worktree,
                    disabled: session || !is_repo,
                    onclick: move |_| { open.set(false); app.draft.write().worktree = true; } }
                if session {
                    div { class: "menu-label", style: "padding-top: 4px;", "The environment is fixed once an agent starts" }
                }
            }
        }
    }
}

const SLASH: &[(&str, &str)] = &[
    ("/plan", "Switch to Plan mode"),
    ("/agent", "Switch to Agent mode"),
    ("/ask", "Switch to Ask mode"),
    ("/fork", "Fork this agent"),
    ("/export", "Export as Larrier session"),
    ("/terminal", "Open in the agent's CLI"),
    ("/worktree", "Run in an isolated worktree"),
    ("/local", "Run in the folder itself"),
    ("/new", "Start a new agent"),
    ("/settings", "Open settings"),
];

/// Text after a trailing `@` token, if the caret is inside one.
fn mention_query(text: &str) -> Option<String> {
    if text.ends_with(char::is_whitespace) {
        return None;
    }
    let token = text.rsplit(char::is_whitespace).next()?;
    let q = token.strip_prefix('@')?;
    (!q.contains('@')).then(|| q.to_string())
}

/// `/command` typed as the whole prompt.
fn slash_query(text: &str) -> Option<String> {
    let t = text.strip_prefix('/')?;
    (!t.contains(char::is_whitespace)).then(|| t.to_string())
}

const SKIP_DIRS: &[&str] = &[".git", "node_modules", "target", ".next", "dist", "build", "__pycache__", ".venv", "venv", ".idea", ".turbo", ".cache", "vendor"];

/// Relative file paths under `root` (forward slashes), breadth-limited.
fn list_files(root: &str, max: usize) -> Vec<String> {
    let base = std::path::Path::new(root);
    let mut out = Vec::new();
    let mut stack = vec![base.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let p = e.path();
            if p.is_dir() {
                if !SKIP_DIRS.contains(&name.as_str()) && !name.starts_with('.') {
                    stack.push(p);
                }
            } else if let Ok(rel) = p.strip_prefix(base) {
                out.push(rel.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/"));
                if out.len() >= max {
                    return out;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn mentions() {
        assert_eq!(super::mention_query("look at @src/ma").as_deref(), Some("src/ma"));
        assert_eq!(super::mention_query("look at @src/main.rs "), None);
        assert_eq!(super::slash_query("/pl").as_deref(), Some("pl"));
        assert_eq!(super::slash_query("/plan this"), None);
    }
}
