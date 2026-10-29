//! The prompt composer, shaped like Cursor's: a pill with a "+" menu, the
//! input, one agent + model picker (brand logos) and send / stop, with the
//! branch and environment pickers underneath.

use std::sync::{Mutex, OnceLock};

use dioxus::html::HasFileData;
use dioxus::prelude::*;
use larrier_agents::harness::{HarnessItem, ItemKind};
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

/// What the composer needs from its session (memoized so a streaming
/// reply doesn't re-render the composer).
#[derive(Clone, PartialEq)]
struct SessInfo {
    agent: AgentKind,
    model: Option<String>,
    mode: Option<String>,
    worktree: bool,
    usage: larrier_core::Usage,
    cwd: Option<String>,
    account: Option<String>,
}

#[component]
pub fn Composer(
    session_id: Option<String>,
    #[props(default = false)] home: bool,
    /// DOM id of the text box (a split view has two composers).
    #[props(default = "composer-input")] input_id: &'static str,
) -> Element {
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
    let mut hl = use_signal(|| 0usize);
    let mut dismissed = use_signal(|| None::<String>);
    let mut dragging = use_signal(|| false);
    let mut overflow = use_signal(|| false);
    // Only the newest width measurement counts (older ones can land late).
    let mut measure_seq = use_signal(|| 0u64);

    let primary = input_id == "composer-input";
    let quote_memo = crate::state::use_ui(|u| u.quote.clone());
    // Pick up attachments produced elsewhere (paste, drop, menus).
    let inbox_len = crate::state::use_ui(|u| u.attach_inbox.len())();
    use_effect(use_reactive!(|inbox_len| {
        if inbox_len > 0 && primary {
            let mut app = app;
            let items = std::mem::take(&mut app.ui.write().attach_inbox);
            let mut at = attachments;
            at.write().extend(items);
        }
    }));
    // Text dropped in for this composer (a dragged chat added as context).
    let compose_len = crate::state::use_ui(|u| u.compose_inbox.len())();
    let sid_in = session_id.clone();
    use_effect(use_reactive!(|compose_len| {
        if compose_len == 0 {
            return;
        }
        let mut app = app;
        let mut text = text;
        let mine: Vec<String> = {
            let mut ui = app.ui.write();
            let (m, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut ui.compose_inbox).into_iter().partition(|(sid, _)| *sid == sid_in);
            ui.compose_inbox = rest;
            m.into_iter().map(|(_, t)| t).collect()
        };
        if !mine.is_empty() {
            let cur = text.read().clone();
            let sep = if cur.is_empty() || cur.ends_with(' ') || cur.ends_with('\n') { "" } else { " " };
            text.set(format!("{cur}{sep}{} ", mine.join(" ")));
            document::eval(&format!("setTimeout(() => document.getElementById('{input_id}')?.focus(), 30)"));
        }
    }));

    // Focus when asked (Ctrl+L, new agent, opening a session).
    let focus = crate::state::use_ui(|u| u.focus_composer)();
    use_effect(use_reactive!(|focus| {
        let _ = focus;
        if primary {
            document::eval("setTimeout(() => document.getElementById('composer-input')?.focus(), 30)");
        }
    }));

    let sid_memo = session_id.clone();
    let sess = use_memo(move || {
        let id = sid_memo.as_ref()?;
        app.with_session(id, |s| SessInfo {
            agent: s.agent.clone(),
            model: s.model.clone(),
            mode: s.mode.clone(),
            worktree: matches!(s.environment, larrier_core::Environment::Worktree { .. }),
            usage: s.usage.clone(),
            cwd: s.cwd(),
            account: s.account_id.clone(),
        })
    });

    // Workspace file list for @-mentions (indexed in the background).
    let file_root: Option<String> = match &session_id {
        Some(_) => sess.read().as_ref().and_then(|s| s.cwd.clone()),
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
        Some(_) => match sess.read().clone() {
            Some(s) => (s.agent, s.model, s.mode, s.worktree, Some(s.usage)),
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
    let queue_ok = app.config.read().settings.queue_followups;
    let placeholder = if home { "Plan, Build, / for commands, @ for context" } else { "Send follow-up" };
    let atts = attachments.read().clone();
    let can_send = !text.read().trim().is_empty() || !atts.is_empty();
    let quote = quote_memo();
    let detected = app.detected.read().clone();
    let multi_now = multi.read().clone();
    let tall = home || overflow() || text.read().contains('\n') || !atts.is_empty() || quote.is_some();
    let model_logo_svg = model.as_deref().and_then(model_logo).or_else(|| agent_logo(&agent));
    let model_text = model_display(&agent, &model);
    let env_text = if worktree { "Worktree" } else { "This PC" };

    // "@" mentions and "/" commands: one suggestion list, keyboard navigable.
    let text_now = text.read().clone();
    let is_dismissed = dismissed.read().as_deref() == Some(text_now.as_str());
    let suggs: Vec<Sugg> = if is_dismissed {
        Vec::new()
    } else {
        suggestions(&app, &text_now, file_index.read().as_deref().unwrap_or(&[]), home, &agent, session_id.as_deref())
    };
    let hl_i = if suggs.is_empty() { 0 } else { hl().min(suggs.len() - 1) };
    let queued: Vec<crate::state::Queued> = session_id.as_ref().and_then(|id| app.queue.read().get(id).cloned()).unwrap_or_default();

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
    // Apply a suggestion: replace the token being typed, or run a command.
    let apply = move |sg: Sugg, sid: Option<String>| {
        let mut text = text;
        match sg.act {
            SuggAct::Insert(ins) => {
                let t = text.read().clone();
                let cut = t.rfind(|c: char| c.is_whitespace()).map(|i| i + 1).unwrap_or(0);
                text.set(format!("{}{}", &t[..cut], ins));
            }
            SuggAct::Larrier(cmd) => run_slash(cmd, sid),
        }
        document::eval(&format!("setTimeout(() => document.getElementById('{input_id}')?.focus(), 10)"));
    };
    // Choose agent (+ model). On an existing session this ports it.
    let choose = move |k: AgentKind, m: Option<String>, sid: Option<String>| {
        let mut app = app;
        match &sid {
            Some(id) => {
                let current = app.peek_session(id, |s| s.agent.clone());
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
        let raw = text.read().clone();
        let a = at.read().clone();
        if raw.trim().is_empty() && a.is_empty() {
            return;
        }
        let cwd = match &sid {
            Some(id) => app.peek_session(id, |s| s.cwd()).flatten(),
            None => app.draft.read().workspace.clone(),
        };
        let t = expand_mentions(&app, &raw, cwd.as_deref());
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
    let suggs_k = suggs.clone();
    let sid_stop = session_id.clone();

    let workspaces = app.config.read().workspaces.clone();
    let ws_now = app.draft.read().workspace.clone();
    let ws_label = ws_now
        .as_ref()
        .map(|p| std::path::Path::new(p).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or(p.clone()))
        .unwrap_or_else(|| "No Repo".into());
    let is_repo = branch.read().clone().flatten().is_some();

    // The agent + model picker menu.
    let picker_menu = rsx! {
        ModelPicker {
            open: model_menu,
            up: !home,
            agent: agent.clone(),
            model: model.clone(),
            onpick: {
                let sid = session_id.clone();
                move |(k, m): (AgentKind, Option<String>)| {
                    model_menu.set(false);
                    choose(k, m, sid.clone());
                }
            },
        }
    };

    rsx! {
        div { class: "composer-wrap",
            ondragover: move |e| { e.prevent_default(); if !dragging() { dragging.set(true); } },
            ondragleave: move |_| dragging.set(false),
            ondrop: move |e| {
                e.prevent_default();
                dragging.set(false);
                let mut at = attachments;
                for f in e.files() {
                    let p = f.path().to_string_lossy().to_string();
                    if !p.is_empty() {
                        at.write().push(crate::state::attachment_for_path(&p));
                    }
                }
            },
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
            if !suggs.is_empty() {
                div { class: "menu mention-menu in flip-up",
                    {
                        let mut last_group = "";
                        rsx! {
                            for (i, sg) in suggs.iter().enumerate() {
                                {
                                    let show_head = sg.group != last_group;
                                    last_group = sg.group;
                                    let sg2 = sg.clone();
                                    let sid = session_id.clone();
                                    rsx! {
                                        if show_head { div { key: "h-{i}", class: "menu-label", "{sg.group}" } }
                                        button { key: "{i}", class: if i == hl_i { "menu-item hl" } else { "menu-item" },
                                            onmouseenter: move |_| hl.set(i),
                                            onmousedown: move |e| { e.prevent_default(); apply(sg2.clone(), sid.clone()); },
                                            span { class: "ic",
                                                if let Some(svg) = sg.logo { Logo { svg, size: 14 } } else { Icon { i: sg.icon, size: 14 } }
                                            }
                                            span { class: "ellipsis", "{sg.label}" }
                                            if !sg.sub.is_empty() { span { class: "sub", "{sg.sub}" } }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if !queued.is_empty() {
                div { class: "queue",
                    for q in queued.clone() {
                        div { key: "{q.id}", class: "queue-item",
                            Icon { i: I::CornerDownLeft, size: 13 }
                            span { class: "t", "{q.text}" }
                            span { class: "faint", style: "font-size: 11px;", "Queued" }
                            button { class: "icon-btn sm", title: "Remove from queue",
                                onclick: {
                                    let sid = session_id.clone();
                                    move |_| {
                                        if let Some(id) = &sid {
                                            if let Some(v) = app.queue.write().get_mut(id) { v.retain(|x| x.id != q.id); }
                                        }
                                    }
                                },
                                Icon { i: I::X, size: 12 }
                            }
                        }
                    }
                }
            }
            if dragging() {
                div { class: "drop-zone", "Drop files or images to attach" }
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
                    div { class: "thumb-row",
                        for (i, a) in atts.iter().enumerate() {
                            match a {
                                Part::Image { .. } => rsx! {
                                    div { key: "{i}", class: "thumb", title: "Image",
                                        if let Some(src) = crate::state::image_src(a) {
                                            img { src: "{src}", onclick: { let s2 = src.clone(); move |_| app.ui.write().lightbox = Some(s2.clone()) } }
                                        } else {
                                            Icon { i: I::Image, size: 20 }
                                        }
                                        button { class: "x", onclick: move |_| { let mut at = attachments; at.write().remove(i); }, Icon { i: I::X, size: 11 } }
                                    }
                                },
                                Part::File { path, text, .. } => rsx! {
                                    span { key: "{i}", class: "file-pill", title: "{path}",
                                        Icon { i: if text.is_some() { I::Lines } else { I::Paperclip }, size: 12 }
                                        span { class: "ellipsis", if text.is_some() { "{path}" } else { "{short_path(path)}" } }
                                        button { class: "x", onclick: move |_| { let mut at = attachments; at.write().remove(i); }, Icon { i: I::X, size: 11 } }
                                    }
                                },
                                _ => rsx! {},
                            }
                        }
                    }
                }
                // One grid for both layouts so the text box is never re-created
                // (and never loses focus) when the composer grows.
                div { class: "composer-row cgrid",
                    div { class: "c-plus",
                        PlusMenu { open: plus_menu, attachments, session_id: session_id.clone(), modes: modes.clone(), mode_now: mode_now.clone(), text, input_id }
                    }
                    textarea {
                        id: "{input_id}",
                        class: "c-text",
                        rows: if home { "3" } else { "1" },
                        placeholder: "{placeholder}",
                        value: "{text}",
                        oninput: move |e| {
                            hl.set(0);
                            let v = e.value();
                            if v.is_empty() {
                                overflow.set(false);
                            }
                            text.set(v);
                            // Measure against the one-line width so the layout
                            // doesn't flip back and forth.
                            let js = format!(
                                r#"const el = document.getElementById('{input_id}'); if (!el) return false;
                                const box = el.closest('.composer');
                                if (box && !box.classList.contains('tall')) el.dataset.cw = el.clientWidth;
                                const w = parseFloat(el.dataset.cw || el.clientWidth);
                                const c = window.__lmc || (window.__lmc = document.createElement('canvas').getContext('2d'));
                                const cs = getComputedStyle(el);
                                c.font = cs.fontStyle + ' ' + cs.fontWeight + ' ' + cs.fontSize + ' ' + cs.fontFamily;
                                return el.value.includes('\n') || c.measureText(el.value).width > w - 14;"#
                            );
                            let seq = measure_seq() + 1;
                            measure_seq.set(seq);
                            spawn(async move {
                                if let Ok(b) = document::eval(&js).join::<bool>().await {
                                    if *measure_seq.peek() == seq && overflow() != b {
                                        overflow.set(b);
                                    }
                                }
                            });
                        },
                        onkeydown: move |e| {
                            if !suggs_k.is_empty() {
                                match e.key() {
                                    Key::ArrowDown => { e.prevent_default(); hl.set((hl_i + 1) % suggs_k.len()); return; }
                                    Key::ArrowUp => { e.prevent_default(); hl.set((hl_i + suggs_k.len() - 1) % suggs_k.len()); return; }
                                    Key::Escape => { e.prevent_default(); dismissed.set(Some(text.read().clone())); return; }
                                    Key::Enter | Key::Tab if !e.modifiers().shift() => {
                                        e.prevent_default();
                                        apply(suggs_k[hl_i].clone(), sid_key.clone());
                                        return;
                                    }
                                    _ => {}
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
                                if wants && (!running || queue_ok) {
                                    e.prevent_default();
                                    do_send(sid_key.clone());
                                }
                            }
                        },
                    }
                    if mode_now != "agent" && mode_now != "build" && !modes.is_empty() {
                        span { class: "picker c-mode", style: "color: var(--blue); cursor: default;", title: "Shift+Tab to change mode",
                            Icon { i: if mode_now == "plan" { I::List } else { I::Message }, size: 13 }
                            "{mode_label(&mode_now)}"
                        }
                    }
                    div { class: "menu-anchor c-model",
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
                        button { class: "send-btn stop c-send", title: "Stop (Ctrl+.)",
                            onclick: move |_| if let Some(id) = &sid_stop { app.stop(id) },
                            Icon { i: I::Stop, size: 10 }
                        }
                    } else {
                        button { class: if can_send { "send-btn ready c-send" } else { "send-btn c-send" }, disabled: !can_send, title: "Send (Enter)",
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
                        ChatUsage { usage: u }
                    }
                    super::usage::QuotaChip { agent: agent.clone(), account_id: sess.read().as_ref().and_then(|s| s.account.clone()) }
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

/// Agents that can be picked: enabled and installed (API agents need a
/// provider), plus the one in use.
fn pickable_agents(app: &App, current: &AgentKind) -> Vec<(AgentKind, String, String)> {
    let cfg = app.config.read();
    let detected = app.detected.read();
    let mut out: Vec<(AgentKind, String, String)> = Vec::new();
    for spec in larrier_core::agent::runnable() {
        let det = detected.iter().find(|d| d.kind == spec.kind);
        let api = spec.binaries.is_empty();
        let installed = api || det.map(|d| d.installed()).unwrap_or(false);
        let usable = if spec.protocol == Protocol::HttpChat { cfg.providers.iter().any(|p| p.enabled) } else { installed };
        if !(cfg.agent(&spec.kind).enabled && usable) && spec.kind != *current {
            continue;
        }
        let sub = if api { "API".to_string() } else { det.and_then(|d| d.version.clone()).map(|v| format!("v{v}")).unwrap_or_default() };
        out.push((spec.kind.clone(), spec.name.to_string(), sub));
    }
    for c in cfg.custom_agents.iter() {
        out.push((AgentKind::Custom(c.id.clone()), c.name.clone(), "custom".into()));
    }
    out
}

fn agent_models(app: &App, kind: &AgentKind) -> Vec<Option<String>> {
    std::iter::once(None).chain(model_options(app, kind).into_iter().filter(|m| m != "default" && m != "auto").map(Some)).collect()
}

/// Agent + model picker: enabled agents on the left, the hovered agent's
/// models on the right; typing searches every model. Fixed size so it never
/// jumps while you browse, and it folds into one column on narrow windows.
#[component]
fn ModelPicker(open: Signal<bool>, up: bool, agent: AgentKind, model: Option<String>, onpick: EventHandler<(AgentKind, Option<String>)>) -> Element {
    let app = use_app();
    let mut query = use_signal(String::new);
    let mut browse = use_signal(|| agent.clone());
    let is_open = open();
    let a0 = agent.clone();
    use_effect(use_reactive!(|is_open| {
        if is_open {
            browse.set(a0.clone());
            query.set(String::new());
        }
    }));
    let agents = pickable_agents(&app, &agent);
    let q = query.read().trim().to_lowercase();
    let cur = browse();
    let cur_meta = agents.iter().find(|(k, _, _)| *k == cur).cloned();
    let results: Vec<(AgentKind, String, Option<String>)> = if q.is_empty() {
        Vec::new()
    } else {
        let mut v = Vec::new();
        for (k, name, _) in agents.iter() {
            let agent_hit = name.to_lowercase().contains(&q);
            for m in agent_models(&app, k) {
                let label = m.clone().unwrap_or_else(|| "Auto".into());
                if agent_hit || label.to_lowercase().contains(&q) {
                    v.push((k.clone(), name.clone(), m));
                }
            }
        }
        v
    };
    let typed = query.read().trim().to_string();
    let cur_name = cur_meta.as_ref().map(|m| m.1.clone()).unwrap_or_else(|| cur.display_name().to_string());
    rsx! {
        Dropdown { open, up, right: true, class: "model-picker",
            div { class: "menu-search",
                Icon { i: I::Search, size: 12 }
                input {
                    placeholder: "Search models, or type a model id…",
                    value: "{query}",
                    autofocus: true,
                    oninput: move |e| query.set(e.value()),
                    onkeydown: {
                        let cur = cur.clone();
                        let first = results.first().cloned();
                        move |e: KeyboardEvent| {
                            if e.key() == Key::Enter {
                                e.prevent_default();
                                match &first {
                                    Some((k, _, m)) => onpick.call((k.clone(), m.clone())),
                                    None => {
                                        let t = query.read().trim().to_string();
                                        if !t.is_empty() {
                                            onpick.call((cur.clone(), Some(t)));
                                        }
                                    }
                                }
                            }
                        }
                    },
                }
            }
            if q.is_empty() {
                div { class: "mp-body",
                    div { class: "mp-agents",
                        for (k, name, sub) in agents.clone() {
                            {
                                let on = k == cur;
                                let using = k == agent;
                                let (k1, k2) = (k.clone(), k.clone());
                                let logo = agent_logo(&k);
                                rsx! {
                                    button { key: "{k.id()}", class: if on { "mp-agent on" } else { "mp-agent" }, title: "{name} {sub}",
                                        onmouseenter: move |_| browse.set(k1.clone()),
                                        onclick: move |_| browse.set(k2.clone()),
                                        span { class: "lg", if let Some(svg) = logo { Logo { svg, size: 15 } } else { Icon { i: I::Bot, size: 14 } } }
                                        span { class: "nm ellipsis", "{name}" }
                                        if using { span { class: "using", title: "In use" } }
                                    }
                                }
                            }
                        }
                    }
                    div { class: "mp-models",
                        div { class: "mp-head",
                            span { class: "nm", "{cur_name}" }
                            if let Some((_, _, sub)) = cur_meta.clone() { span { class: "sub", "{sub}" } }
                        }
                        for m in agent_models(&app, &cur) {
                            {
                                let checked = cur == agent && model.as_deref() == m.as_deref();
                                let label = m.clone().unwrap_or_else(|| "Auto".into());
                                let desc = if m.is_none() { "Agent's default" } else { "" };
                                let mlogo = m.as_deref().and_then(model_logo);
                                let k = cur.clone();
                                let m2 = m.clone();
                                rsx! {
                                    button { key: "{label}", class: if checked { "menu-item mp-row checked" } else { "menu-item mp-row" },
                                        onclick: move |_| onpick.call((k.clone(), m2.clone())),
                                        span { class: "lg", if let Some(svg) = mlogo { Logo { svg, size: 13 } } else { Icon { i: I::Sparkle, size: 13, stroke: 1.5 } } }
                                        span { class: "ellipsis", "{label}" }
                                        if !desc.is_empty() { span { class: "sub", "{desc}" } }
                                        if checked { span { class: "check", Icon { i: I::Check, size: 13 } } }
                                    }
                                }
                            }
                        }
                    }
                }
            } else {
                div { class: "mp-results",
                    for (k, name, m) in results.clone() {
                        {
                            let checked = k == agent && model.as_deref() == m.as_deref();
                            let label = m.clone().unwrap_or_else(|| "Auto".into());
                            let mlogo = m.as_deref().and_then(model_logo).or_else(|| agent_logo(&k));
                            let (k2, m2) = (k.clone(), m.clone());
                            rsx! {
                                button { key: "{k.id()}-{label}", class: if checked { "menu-item mp-row checked" } else { "menu-item mp-row" },
                                    onclick: move |_| onpick.call((k2.clone(), m2.clone())),
                                    span { class: "lg", if let Some(svg) = mlogo { Logo { svg, size: 13 } } }
                                    span { class: "ellipsis", "{label}" }
                                    span { class: "sub", "{name}" }
                                    if checked { span { class: "check", Icon { i: I::Check, size: 13 } } }
                                }
                            }
                        }
                    }
                    if results.is_empty() {
                        div { class: "menu-empty", "No matching models" }
                    }
                    {
                        let k = cur.clone();
                        let t = typed.clone();
                        rsx! {
                            button { class: "menu-item mp-row",
                                onclick: move |_| onpick.call((k.clone(), Some(t.clone()))),
                                span { class: "lg", Icon { i: I::CornerDownLeft, size: 13 } }
                                span { class: "ellipsis", "Use “{typed}”" }
                                span { class: "sub", "with {cur_name}" }
                            }
                        }
                    }
                }
            }
            div { class: "mp-foot",
                button { class: "menu-item",
                    onclick: move |_| {
                        let mut o = open;
                        o.set(false);
                        app.open_settings(SettingsTab::Agents);
                    },
                    span { class: "muted", Icon { i: I::Settings, size: 14 } }
                    span { class: "ellipsis", "Manage agents & models…" }
                }
                span { class: "mp-keys", "↑↓ ↵" }
            }
        }
    }
}

/// This chat's token use, compact, with the breakdown on hover.
#[component]
fn ChatUsage(usage: larrier_core::Usage) -> Element {
    use super::transcript::human_count;
    let u = &usage;
    let mut tip = format!(
        "This chat\nInput {} · Output {}\nCache read {} · Cache write {}",
        human_count(u.input_tokens),
        human_count(u.output_tokens),
        human_count(u.cache_read_tokens),
        human_count(u.cache_write_tokens)
    );
    if u.reasoning_tokens > 0 {
        tip.push_str(&format!("\nReasoning {}", human_count(u.reasoning_tokens)));
    }
    if let Some(c) = u.cost_usd {
        tip.push_str(&format!("\nCost ${c:.2}"));
    }
    let fresh = u.input_tokens + u.output_tokens;
    rsx! {
        span { class: "chat-usage", title: "{tip}",
            Icon { i: I::Cpu, size: 12, stroke: 1.5 }
            span { class: "num", "{human_count(fresh)}" }
            span { class: "dim", "+{human_count(u.cache_read_tokens + u.cache_write_tokens)} cache" }
            if let Some(c) = u.cost_usd { span { class: "num", "${c:.2}" } }
        }
    }
}

/// The "+" menu: attachments, mentions, mode, commands.
#[component]
fn PlusMenu(open: Signal<bool>, attachments: Signal<Vec<Part>>, session_id: Option<String>, modes: Vec<String>, mode_now: String, text: Signal<String>, input_id: &'static str) -> Element {
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
                    document::eval(&format!("setTimeout(() => document.getElementById('{input_id}')?.focus(), 10)"));
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

#[derive(Clone, PartialEq)]
enum SuggAct {
    /// Replace the token being typed with this text.
    Insert(String),
    /// Run a Larrier command.
    Larrier(&'static str),
}

#[derive(Clone, PartialEq)]
struct Sugg {
    group: &'static str,
    icon: I,
    logo: Option<&'static str>,
    label: String,
    sub: String,
    act: SuggAct,
}

static HARNESS: OnceLock<Mutex<Option<Vec<HarnessItem>>>> = OnceLock::new();

/// Agent skills and commands (discovered once in the background).
fn harness_items(app: &App) -> Vec<HarnessItem> {
    let cell = HARNESS.get_or_init(|| Mutex::new(None));
    if let Some(v) = cell.lock().ok().and_then(|g| g.clone()) {
        return v;
    }
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        let ws: Vec<std::path::PathBuf> = app.config.read().workspaces.iter().map(|w| std::path::PathBuf::from(&w.path)).collect();
        std::thread::spawn(move || {
            let items = larrier_agents::harness::discover(&ws);
            if let Some(c) = HARNESS.get() {
                if let Ok(mut g) = c.lock() {
                    *g = Some(items);
                }
            }
        });
    }
    Vec::new()
}

fn suggestions(app: &App, text: &str, files: &[String], home: bool, agent: &AgentKind, current: Option<&str>) -> Vec<Sugg> {
    let mut out = Vec::new();
    if let Some(q) = mention_query(text) {
        let ql = q.to_lowercase();
        // Files
        let mut hits: Vec<&String> = files.iter().filter(|p| p.to_lowercase().contains(&ql)).take(300).collect();
        hits.sort_by_key(|p| (!p.rsplit('/').next().unwrap_or(p).to_lowercase().starts_with(&ql), p.len()));
        for p in hits.into_iter().take(6) {
            let (dir, name) = match p.rsplit_once('/') { Some((d, n)) => (d.to_string(), n.to_string()), None => (String::new(), p.clone()) };
            out.push(Sugg { group: "Files", icon: I::File, logo: None, label: name, sub: dir, act: SuggAct::Insert(format!("@{p} ")) });
        }
        // Folders
        let mut dirs: Vec<String> = files
            .iter()
            .filter_map(|p| p.rsplit_once('/').map(|(d, _)| d.to_string()))
            .filter(|d| d.to_lowercase().contains(&ql))
            .collect();
        dirs.sort();
        dirs.dedup();
        dirs.sort_by_key(|d| d.len());
        for d in dirs.into_iter().take(3) {
            out.push(Sugg { group: "Folders", icon: I::Folder, logo: None, label: format!("{d}/"), sub: String::new(), act: SuggAct::Insert(format!("@{d}/ ")) });
        }
        // Other agents' conversations
        let sessions = app.sessions.read();
        for s in sessions.iter().filter(|s| Some(s.id.as_str()) != current && !s.archived && (ql.is_empty() || s.title.to_lowercase().contains(&ql))).take(4) {
            let short = &s.id[s.id.len().saturating_sub(8)..];
            out.push(Sugg {
                group: "Agents",
                icon: I::Message,
                logo: crate::logos::agent_logo(&s.agent),
                label: s.title.clone(),
                sub: format!("{} · {}", s.agent.display_name(), crate::state::rel_time(s.updated_at)),
                act: SuggAct::Insert(format!("@agent:{short} ")),
            });
        }
        if "git".starts_with(&ql) || ql.starts_with("git") || "diff".starts_with(&ql) {
            out.push(Sugg { group: "Context", icon: I::Diff, logo: None, label: "Git diff".into(), sub: "uncommitted changes".into(), act: SuggAct::Insert("@git:diff ".into()) });
        }
        return out;
    }
    if let Some(q) = slash_query(text) {
        let ql = q.to_lowercase();
        for (c, d) in SLASH.iter().copied() {
            if c[1..].starts_with(&ql) && (home || !matches!(c, "/worktree" | "/local")) {
                out.push(Sugg { group: "Larrier", icon: I::Command, logo: None, label: c.to_string(), sub: d.to_string(), act: SuggAct::Larrier(c) });
            }
        }
        let logo = crate::logos::agent_logo(agent);
        let mut seen = std::collections::BTreeSet::new();
        for it in harness_items(app).into_iter().filter(|it| matches!(it.kind, ItemKind::Command | ItemKind::Skill) && it.agents.contains(agent)) {
            let name = it.name.trim_start_matches('/').to_string();
            if !name.to_lowercase().contains(&ql) || !seen.insert(name.clone()) {
                continue;
            }
            let group = if it.kind == ItemKind::Skill { "Skills" } else { "Commands" };
            out.push(Sugg {
                group,
                icon: if it.kind == ItemKind::Skill { I::Sparkle } else { I::Slash },
                logo: if it.kind == ItemKind::Command { logo } else { None },
                label: format!("/{name}"),
                sub: it.description.clone().unwrap_or_default().chars().take(80).collect(),
                act: SuggAct::Insert(format!("/{name} ")),
            });
            if out.len() > 40 {
                break;
            }
        }
        out.sort_by_key(|s| match s.group { "Larrier" => 0, "Commands" => 1, _ => 2 });
    }
    out
}

/// Expand `@agent:<id>` and `@git:diff` into context the agent can read.
fn expand_mentions(app: &App, text: &str, cwd: Option<&str>) -> String {
    let mut extra = String::new();
    for tok in text.split_whitespace() {
        if let Some(short) = tok.strip_prefix("@agent:") {
            let found = app.sessions.read().iter().find(|s| s.id.ends_with(short)).map(|s| s.id.clone());
            if let Some(id) = found {
                if let Ok(s) = crate::state::store().load(&id) {
                    let ctx = s.handoff_context(0, 24_000);
                    extra.push_str(&format!("\n\n<agent_context title=\"{}\" agent=\"{}\">\n{}\n</agent_context>", s.title, s.agent.display_name(), ctx.trim()));
                }
            }
        } else if tok == "@git:diff" {
            if let Some(dir) = cwd {
                let out = std::process::Command::new("git").args(["diff", "HEAD", "--no-color"]).current_dir(dir).output();
                if let Ok(o) = out {
                    let d = String::from_utf8_lossy(&o.stdout).to_string();
                    let d: String = d.chars().take(60_000).collect();
                    if !d.trim().is_empty() {
                        extra.push_str(&format!("\n\n<git_diff>\n{d}\n</git_diff>"));
                    }
                }
            }
        }
    }
    format!("{text}{extra}")
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
