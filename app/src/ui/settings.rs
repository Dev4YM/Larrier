//! Settings: general, agents, accounts, model providers, sessions/import,
//! workspaces, shortcuts, about.

use dioxus::prelude::*;
use larrier_core::{
    agent::catalog, config::CustomAgent, lsf::new_id, secrets, AccountAuth, AgentKind,
    Protocol, Provider, ProviderKind, RunMode,
};

use super::{AgentGlyph, DragRegion, Kbd, Toggle};
use crate::icons::{Icon, I};
use crate::state::{store, use_app, Modal, SettingsTab, View};

fn tab_icon(t: SettingsTab) -> I {
    match t {
        SettingsTab::General => I::Settings,
        SettingsTab::Appearance => I::Sun,
        SettingsTab::Usage => I::Cpu,
        SettingsTab::SourceControl => I::GitBranch,
        SettingsTab::Agents => I::Bot,
        SettingsTab::Accounts => I::User,
        SettingsTab::Models => I::Box,
        SettingsTab::Sessions => I::History,
        SettingsTab::Workspaces => I::Folder,
        SettingsTab::Shortcuts => I::Command,
        SettingsTab::About => I::Info,
    }
}

/// Settings navigation — takes the sidebar's place, like Cursor.
#[component]
pub fn SettingsNav() -> Element {
    let mut app = use_app();
    let tab = match &*app.view.read() {
        View::Settings(t) => *t,
        _ => SettingsTab::General,
    };
    let mut q = use_signal(String::new);
    let ql = q.read().to_lowercase();
    let name = crate::state::user_name();
    let initial = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
    rsx! {
        aside { class: "sidebar",
            div { class: "side-top",
                button { class: "nav-row back-row",
                    onclick: move |_| {
                        if app.can_go(-1) { app.go(-1) } else { app.new_agent() }
                    },
                    Icon { i: I::ArrowLeft, size: 15, stroke: 1.5 }
                    "Back"
                }
            }
            div { class: "side-search", style: "margin: 4px 10px 10px;",
                Icon { i: I::Search, size: 13 }
                input { placeholder: "Search Settings", value: "{q}", oninput: move |e| q.set(e.value()) }
            }
            div { class: "side-scroll",
                div { class: "side-nav",
                    for t in SettingsTab::ALL.into_iter().filter(|t| ql.is_empty() || t.label().to_lowercase().contains(&ql)) {
                        button {
                            key: "{t.label()}",
                            class: if t == tab { "nav-row on" } else { "nav-row" },
                            onclick: move |_| app.view.set(View::Settings(t)),
                            span { class: "ic", Icon { i: tab_icon(t), size: 16, stroke: 1.5 } }
                            "{t.label()}"
                        }
                    }
                    button { class: "nav-row", onclick: move |_| { let _ = open::that("https://github.com/Dev4YM/Larrier#readme"); },
                        span { class: "ic", Icon { i: I::File, size: 16, stroke: 1.5 } }
                        "Docs"
                        span { class: "faint", Icon { i: I::ArrowUpRight, size: 11 } }
                    }
                }
            }
            div { class: "side-profile",
                div { class: "avatar", "{initial}" }
                span { class: "nm", "{name}" }
            }
        }
    }
}

#[component]
pub fn SettingsView(tab: SettingsTab) -> Element {
    rsx! {
        div { class: "main-head",
            DragRegion {}
        }
        div { class: "settings",
            div { class: "settings-body",
                div { class: "settings-inner",
                    match tab {
                        SettingsTab::General => rsx! { General {} },
                        SettingsTab::Appearance => rsx! { Appearance {} },
                        SettingsTab::Usage => rsx! { super::usage::UsageView {} },
                        SettingsTab::SourceControl => rsx! { SourceControl {} },
                        SettingsTab::Agents => rsx! { Agents {} },
                        SettingsTab::Accounts => rsx! { Accounts {} },
                        SettingsTab::Models => rsx! { Models {} },
                        SettingsTab::Sessions => rsx! { Sessions {} },
                        SettingsTab::Workspaces => rsx! { Workspaces {} },
                        SettingsTab::Shortcuts => rsx! { Shortcuts {} },
                        SettingsTab::About => rsx! { About {} },
                    }
                }
            }
        }
    }
}

#[component]
fn Row(title: String, #[props(default)] desc: Option<String>, children: Element) -> Element {
    rsx! {
        div { class: "set-row",
            div { class: "lbl",
                div { class: "t", "{title}" }
                if let Some(d) = desc { div { class: "d", "{d}" } }
            }
            {children}
        }
    }
}

// ------------------------------------------------------------------ general

#[component]
fn General() -> Element {
    let mut app = use_app();
    let s = app.config.read().settings.clone();
    let seg = |on: bool| if on { "on" } else { "" };
    let agent_opts: Vec<(String, String)> = larrier_core::agent::runnable().map(|sp| (sp.kind.id(), sp.name.to_string())).collect();
    rsx! {
        h1 { "General" }
        div { class: "set-group-title", "Startup" }
        div { class: "set-card",
            Row { title: "Tips".to_string(), desc: Some("Show rotating tips on the new agent screen.".to_string()),
                Toggle { on: s.show_tips, onchange: move |v| app.update_config(|c| c.settings.show_tips = v) }
            }
            Row { title: "Window Restoration".to_string(), desc: Some("Reopen Larrier with the same size and position.".to_string()),
                Toggle { on: s.restore_window, onchange: move |v| app.update_config(|c| c.settings.restore_window = v) }
            }
            Row { title: "Continue Interrupted Agents".to_string(), desc: Some("Offer to resume agents that were running when Larrier closed.".to_string()),
                Toggle { on: s.continue_interrupted, onchange: move |v| app.update_config(|c| c.settings.continue_interrupted = v) }
            }
            Row { title: "Import Sessions on Startup".to_string(), desc: Some("Pull new conversations from every installed agent when Larrier opens.".to_string()),
                Toggle { on: s.auto_import, onchange: move |v| app.update_config(|c| c.settings.auto_import = v) }
            }
        }
        div { class: "set-group-title", "Notifications" }
        div { class: "set-card",
            Row { title: "System Notifications".to_string(), desc: Some("Show a system notification when an agent finishes or needs attention.".to_string()),
                Toggle { on: s.notify_on_finish, onchange: move |v| app.update_config(|c| c.settings.notify_on_finish = v) }
            }
            Row { title: "System Tray Icon".to_string(), desc: Some("Show Larrier in the system tray.".to_string()),
                Toggle { on: s.tray_icon, onchange: move |v| app.update_config(|c| c.settings.tray_icon = v) }
            }
            Row { title: "Completion Sound".to_string(), desc: Some("Play a sound when agents finish or need attention.".to_string()),
                div { class: "row",
                    button { class: "ghost-btn", onclick: move |_| crate::notify::preview_sound(), "Preview" }
                    Toggle { on: s.completion_sound, onchange: move |v| app.update_config(|c| c.settings.completion_sound = v) }
                }
            }
        }
        div { class: "set-group-title", "Agents" }
        div { class: "set-card",
            Row { title: "Default Agent".to_string(), desc: Some("Used for new agents.".to_string()),
                super::Select {
                    value: s.default_agent.id(),
                    options: agent_opts,
                    onchange: move |v: String| {
                        let k = AgentKind::from_id(&v);
                        app.update_config(|c| c.settings.default_agent = k.clone());
                        app.draft.write().agent = k;
                    },
                }
            }
            Row { title: "Autonomy".to_string(), desc: Some("How much agents may do without asking, applied through each agent's own permission flags.".to_string()),
                div { class: "seg",
                    for m in [RunMode::Ask, RunMode::AutoEdit, RunMode::FullAuto] {
                        button { key: "{m.label()}", class: seg(s.run_mode == m), onclick: move |_| app.update_config(|c| c.settings.run_mode = m), "{m.label()}" }
                    }
                }
            }
            Row { title: "Queue Follow-ups".to_string(), desc: Some("Messages sent while an agent is working are queued and sent when it finishes.".to_string()),
                Toggle { on: s.queue_followups, onchange: move |v| app.update_config(|c| c.settings.queue_followups = v) }
            }
            Row { title: "Send with Ctrl+Enter".to_string(), desc: Some("Otherwise Enter sends and Shift+Enter adds a line.".to_string()),
                Toggle { on: s.send_with_ctrl_enter, onchange: move |v| app.update_config(|c| c.settings.send_with_ctrl_enter = v) }
            }
            Row { title: "Show Reasoning".to_string(), desc: Some("Display thinking blocks inside \u{201c}Worked for\u{201d}.".to_string()),
                Toggle { on: s.show_reasoning, onchange: move |v| app.update_config(|c| c.settings.show_reasoning = v) }
            }
        }
    }
}

// ------------------------------------------------------------------ appearance

#[component]
fn Appearance() -> Element {
    rsx! { super::appearance::AppearancePage {} }
}

// ------------------------------------------------------------------ source control

#[component]
fn SourceControl() -> Element {
    let mut app = use_app();
    let s = app.config.read().settings.clone();
    let gh = use_resource(|| async {
        tokio::task::spawn_blocking(|| larrier_agents::detect::which_program("gh").is_some()).await.unwrap_or(false)
    });
    let git = use_resource(|| async {
        tokio::task::spawn_blocking(|| larrier_agents::detect::which_program("git").is_some()).await.unwrap_or(false)
    });
    let dir = s.worktrees_dir.clone().unwrap_or_else(|| store().worktrees_dir().to_string_lossy().to_string());
    rsx! {
        h1 { "Source Control" }
        div { class: "set-card",
            Row { title: "Git".to_string(), desc: Some("Used for changes, commits, branches and worktrees.".to_string()),
                match git.read().clone() {
                    Some(true) => rsx! { span { class: "status-pill ok", Icon { i: I::Check, size: 11 } "Found" } },
                    Some(false) => rsx! { span { class: "status-pill missing", "Not found" } },
                    None => rsx! { div { class: "spinner" } },
                }
            }
            Row { title: "GitHub CLI".to_string(), desc: Some("Needed for pull requests (gh auth login).".to_string()),
                match gh.read().clone() {
                    Some(true) => rsx! { span { class: "status-pill ok", Icon { i: I::Check, size: 11 } "Found" } },
                    Some(false) => rsx! { a { href: "https://cli.github.com", "Install GitHub CLI" } },
                    None => rsx! { div { class: "spinner" } },
                }
            }
        }
        div { class: "set-group-title", "Worktrees" }
        div { class: "set-card",
            Row { title: "Use Worktrees by Default".to_string(), desc: Some("Start each new agent in its own git worktree so parallel agents never collide.".to_string()),
                Toggle { on: s.use_worktrees, onchange: move |v| { app.update_config(|c| c.settings.use_worktrees = v); app.draft.write().worktree = v; } }
            }
            Row { title: "Worktrees Folder".to_string(), desc: Some(dir.clone()),
                button { class: "ghost-btn",
                    onclick: move |_| {
                        spawn(async move {
                            if let Some(d) = rfd::AsyncFileDialog::new().pick_folder().await {
                                let p = d.path().to_string_lossy().to_string();
                                app.update_config(|c| c.settings.worktrees_dir = Some(p));
                            }
                        });
                    },
                    Icon { i: I::FolderOpen, size: 13 } "Change…"
                }
            }
        }
    }
}

// ------------------------------------------------------------------ agents

#[component]
fn Agents() -> Element {
    let mut app = use_app();
    let detected = app.detected.read().clone();
    let mut open = use_signal(|| None::<String>);
    let customs = app.config.read().custom_agents.clone();
    rsx! {
        h1 { "Agents" }
        div { class: "sub", "Every coding agent Larrier can drive, import from, and port sessions to." }
        div { class: "set-group-title", "Coding agents"
            button { class: "icon-btn sm", title: "Re-detect", onclick: move |_| { app.detect_agents(); app.toast("Detecting agents…"); }, Icon { i: I::Refresh, size: 12 } }
        }
        div { class: "set-card",
            for spec in catalog().iter().filter(|s| !s.binaries.is_empty()) {
                {
                    let d = detected.iter().find(|d| d.kind == spec.kind).cloned();
                    let id = spec.kind.id();
                    let is_open = open().as_deref() == Some(id.as_str());
                    let id2 = id.clone();
                    let ac = app.config.read().agent(&spec.kind);
                    let k = spec.kind.clone();
                    let (k1, k2, k3, k4, k5, k6) = (k.clone(), k.clone(), k.clone(), k.clone(), k.clone(), k.clone());
                    rsx! {
                        div { key: "{id}",
                            div { class: "set-row", style: "cursor: pointer;",
                                onclick: move |_| open.set(if is_open { None } else { Some(id2.clone()) }),
                                AgentGlyph { kind: spec.kind.clone(), class: "lg" }
                                div { class: "lbl",
                                    div { class: "t", "{spec.name} " span { class: "faint", style: "font-weight: 400", "· {spec.vendor}" } }
                                    div { class: "d", "{spec.tagline}" }
                                }
                                match &d {
                                    Some(d) if d.installed() => rsx! { span { class: "status-pill ok", Icon { i: I::Check, size: 11 } {d.version.clone().map(|v| format!("v{v}")).unwrap_or("Installed".into())} } },
                                    Some(_) => rsx! { span { class: "status-pill missing", "Not installed" } },
                                    None => rsx! { div { class: "spinner" } },
                                }
                                span { class: "faint", if is_open { Icon { i: I::ChevronDown, size: 13 } } else { Icon { i: I::ChevronRight, size: 13 } } }
                            }
                            if is_open {
                                div { class: "agent-detail",
                                    div { class: "kv",
                                        span { class: "k", "Executable" }
                                        input { class: "input full",
                                            placeholder: d.as_ref().and_then(|d| d.path.clone()).map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|| spec.binaries.join(" / ")),
                                            value: "{ac.binary.clone().unwrap_or_default()}",
                                            onchange: move |e| { let v = e.value(); app.update_config(|c| c.agent_mut(&k1).binary = if v.trim().is_empty() { None } else { Some(v) }); app.detect_agents(); },
                                        }
                                        span { class: "k", "Default model" }
                                        input { class: "input full", placeholder: "agent default",
                                            value: "{ac.default_model.clone().unwrap_or_default()}",
                                            onchange: move |e| { let v = e.value(); app.update_config(|c| c.agent_mut(&k2).default_model = if v.trim().is_empty() { None } else { Some(v) }); },
                                        }
                                        // The agent's own mode groups (Claude Code modes, Codex
                                        // plan + permissions, …): default for new agents.
                                        for (gi, g) in spec.mode_groups.iter().enumerate() {
                                            {
                                                let kk = spec.kind.clone();
                                                let current = if gi == 0 { ac.default_mode.clone() } else { ac.default_permission.clone() };
                                                let auto_label = format!("{} (from Autonomy)", g.default_for(app.config.read().settings.run_mode).label);
                                                let options: Vec<(String, String)> = std::iter::once((String::new(), auto_label))
                                                    .chain(g.options.iter().map(|o| (o.id.to_string(), o.label.to_string())))
                                                    .collect();
                                                rsx! {
                                                    span { key: "k{gi}", class: "k", "Default {g.label.to_lowercase()}" }
                                                    super::Select { key: "s{gi}", value: current.unwrap_or_default(), options, wide: true,
                                                        onchange: move |v: String| {
                                                            let v = if v.is_empty() { None } else { Some(v) };
                                                            let k = kk.clone();
                                                            app.update_config(move |c| if gi == 0 { c.agent_mut(&k).default_mode = v } else { c.agent_mut(&k).default_permission = v });
                                                        } }
                                                }
                                            }
                                        }
                                        span { class: "k", "Extra models" }
                                        input { class: "input full", placeholder: "comma separated model ids",
                                            value: "{ac.extra_models.join(\", \")}",
                                            onchange: move |e| { let v: Vec<String> = e.value().split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(); app.update_config(|c| c.agent_mut(&k3).extra_models = v); },
                                        }
                                        span { class: "k", "Extra arguments" }
                                        input { class: "input full", placeholder: "e.g. --max-turns 40",
                                            value: "{ac.extra_args.join(\" \")}",
                                            onchange: move |e| { let v: Vec<String> = e.value().split_whitespace().map(|s| s.to_string()).collect(); app.update_config(|c| c.agent_mut(&k4).extra_args = v); },
                                        }
                                        span { class: "k", "Protocol" }
                                        div { class: "row",
                                            span { class: "code-inline", "{protocol_label(spec.protocol)}" }
                                            if let Some(acp) = spec.acp_command {
                                                span { class: "faint", "· ACP:" }
                                                span { class: "code-inline", "{acp.join(\" \")}" }
                                                span { class: "faint", "prefer" }
                                                Toggle { on: ac.prefer_acp, onchange: move |v| app.update_config(|c| c.agent_mut(&k5).prefer_acp = v) }
                                            }
                                        }
                                        span { class: "k", "Import sessions" }
                                        div { class: "row",
                                            Toggle { on: ac.import && spec.can_import, onchange: move |v| app.update_config(|c| c.agent_mut(&k6).import = v) }
                                            span { class: "code-inline", "{spec.session_location}" }
                                        }
                                        span { class: "k", "Install" }
                                        div { class: "col", style: "gap: 4px;",
                                            for (lbl, cmd) in spec.install.iter() {
                                                div { class: "row",
                                                    span { class: "faint", style: "width: 90px; font-size: 11px;", "{lbl}" }
                                                    span { class: "code-inline grow", "{cmd}" }
                                                    button { class: "icon-btn sm", title: "Run in terminal",
                                                        onclick: { let c = cmd.to_string(); move |_| { let home = dirs::home_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or(".".into()); let _ = crate::state::open_terminal(&home, &c); } },
                                                        Icon { i: I::Play, size: 11 }
                                                    }
                                                }
                                            }
                                        }
                                        if let Some(login) = spec.login_command {
                                            span { class: "k", "Sign in" }
                                            div { class: "row",
                                                span { class: "code-inline", "{login}" }
                                                button { class: "ghost-btn", onclick: { let l = login.to_string(); move |_| { let home = dirs::home_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or(".".into()); let _ = crate::state::open_terminal(&home, &l); } }, "Open login" }
                                            }
                                        }
                                    }
                                    div { class: "row",
                                        a { href: "{spec.homepage}", "Documentation" }
                                        div { class: "grow" }
                                        button { class: "ghost-btn", onclick: { let k = spec.kind.clone(); move |_| app.ui.write().modal = Some(Modal::AddAccount(k.clone())) }, Icon { i: I::Plus, size: 12 } "Add account" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        div { class: "set-group-title", "Custom agents" }
        div { class: "set-card",
            for c in customs {
                div { key: "{c.id}", class: "set-row",
                    AgentGlyph { kind: AgentKind::Custom(c.id.clone()), class: "lg" }
                    div { class: "lbl",
                        div { class: "t", "{c.name}" }
                        div { class: "d", span { class: "code-inline", "{c.command} {c.args.join(\" \")}" } " · {protocol_label(c.protocol)}" }
                    }
                    button { class: "icon-btn", title: "Remove",
                        onclick: { let id = c.id.clone(); move |_| app.update_config(|cfg| cfg.custom_agents.retain(|x| x.id != id)) },
                        Icon { i: I::Trash, size: 13 }
                    }
                }
            }
            AddCustomAgent {}
        }
    }
}

fn protocol_label(p: Protocol) -> &'static str {
    match p {
        Protocol::ClaudeStream => "stream-json (Claude)",
        Protocol::CodexExec => "codex exec --json",
        Protocol::CursorStream => "stream-json (Cursor)",
        Protocol::GeminiStream => "stream-json (Gemini)",
        Protocol::OpenCodeRun => "opencode run --format json",
        Protocol::PiJson => "pi --mode json",
        Protocol::AmpStream => "amp --stream-json",
        Protocol::QwenStream => "stream-json (Qwen)",
        Protocol::HermesStream => "hermes --format stream-json",
        Protocol::DroidStream => "droid exec -o stream-json",
        Protocol::Acp => "Agent Client Protocol",
        Protocol::PlainText => "plain text",
        Protocol::HttpChat => "HTTP chat API",
        Protocol::None => "import only",
    }
}

#[component]
fn AddCustomAgent() -> Element {
    let app = use_app();
    let mut name = use_signal(String::new);
    let mut command = use_signal(String::new);
    let mut protocol = use_signal(|| Protocol::Acp);
    rsx! {
        div { class: "set-row",
            input { class: "input", style: "width: 150px", placeholder: "Name", value: "{name}", oninput: move |e| name.set(e.value()) }
            input { class: "input grow", placeholder: "Command, e.g. npx -y my-acp-agent", value: "{command}", oninput: move |e| command.set(e.value()) }
            super::Select {
                value: match protocol() { Protocol::ClaudeStream => "claude", Protocol::PlainText => "plain", _ => "acp" }.to_string(),
                options: vec![("acp".to_string(), "ACP".to_string()), ("claude".to_string(), "Claude stream-json".to_string()), ("plain".to_string(), "Plain text".to_string())],
                onchange: move |v: String| protocol.set(match v.as_str() { "claude" => Protocol::ClaudeStream, "plain" => Protocol::PlainText, _ => Protocol::Acp }),
            }
            button { class: "primary-btn", disabled: name().trim().is_empty() || command().trim().is_empty(),
                onclick: move |_| {
                    let mut parts = command().split_whitespace().map(|s| s.to_string()).collect::<Vec<_>>();
                    if parts.is_empty() { return; }
                    let cmd = parts.remove(0);
                    let slug: String = name().to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();
                    let ca = CustomAgent { id: slug, name: name(), command: cmd, args: parts, env: Vec::new(), protocol: protocol(), models: Vec::new() };
                    app.update_config(|c| c.custom_agents.push(ca.clone()));
                    name.set(String::new());
                    command.set(String::new());
                },
                "Add"
            }
        }
    }
}

// ------------------------------------------------------------------ accounts

#[component]
fn Accounts() -> Element {
    let mut app = use_app();
    let cfg = app.config.read().clone();
    rsx! {
        h1 { "Accounts" }
        div { class: "sub", "Connect several logins or API keys per agent and choose which one each agent runs as." }
        for spec in catalog().iter().filter(|s| s.can_run && !s.binaries.is_empty()) {
            {
                let accounts: Vec<_> = cfg.accounts_for(&spec.kind).into_iter().cloned().collect();
                let k = spec.kind.clone();
                rsx! {
                    div { key: "{spec.kind.id()}", class: "set-group-title",
                        AgentGlyph { kind: spec.kind.clone() }
                        "{spec.name}"
                        div { class: "grow" }
                        button { class: "ghost-btn", style: "height: 24px; font-size: 11.5px;", onclick: move |_| app.ui.write().modal = Some(Modal::AddAccount(k.clone())),
                            Icon { i: I::Plus, size: 11 } "Add"
                        }
                    }
                    div { class: "set-card",
                        if accounts.is_empty() {
                            div { class: "set-row",
                                div { class: "lbl",
                                    div { class: "t", "Current login" }
                                    div { class: "d", "Runs with whatever {spec.name} is signed into on this machine." }
                                }
                                span { class: "status-pill missing", "default" }
                            }
                        }
                        for a in accounts {
                            div { key: "{a.id}", class: "set-row",
                                Icon { i: match a.auth { AccountAuth::ApiKey { .. } => I::Key, AccountAuth::ConfigDir { .. } => I::Lock, _ => I::User }, size: 15 }
                                div { class: "lbl",
                                    div { class: "t", "{a.label}" }
                                    div { class: "d",
                                        "{a.auth.label()}"
                                        match &a.auth {
                                            AccountAuth::ApiKey { env_var, secret_ref } => rsx! { " · {env_var} = " span { class: "code-inline", {secrets::get(secret_ref).map(|v| secrets::mask(&v)).unwrap_or("missing".into())} } },
                                            AccountAuth::ConfigDir { path } => rsx! { " · " span { class: "code-inline", "{path}" } },
                                            _ => rsx! {},
                                        }
                                    }
                                }
                                if a.is_default {
                                    span { class: "status-pill ok", "Default" }
                                } else {
                                    button { class: "ghost-btn", style: "height: 24px;",
                                        onclick: { let id = a.id.clone(); let k = a.agent.clone(); move |_| app.update_config(|c| for x in c.accounts.iter_mut().filter(|x| x.agent == k) { x.is_default = x.id == id; }) },
                                        "Make default"
                                    }
                                }
                                if let AccountAuth::ConfigDir { path } = &a.auth {
                                    if let (Some(var), Some(login)) = (spec.home_env, spec.login_command) {
                                        button { class: "ghost-btn", style: "height: 24px;",
                                            onclick: { let p = path.clone(); move |_| { let cmd = if cfg!(windows) { format!("set {var}={p}&& {login}") } else { format!("{var}='{p}' {login}") }; let _ = crate::state::open_terminal(&p, &cmd); } },
                                            "Sign in"
                                        }
                                    }
                                }
                                button { class: "icon-btn", title: "Remove account",
                                    onclick: {
                                        let a2 = a.clone();
                                        move |_| {
                                            if let AccountAuth::ApiKey { secret_ref, .. } = &a2.auth { secrets::delete(secret_ref); }
                                            app.update_config(|c| c.accounts.retain(|x| x.id != a2.id));
                                        }
                                    },
                                    Icon { i: I::Trash, size: 13 }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ models

#[component]
fn Models() -> Element {
    let app = use_app();
    let providers = app.config.read().providers.clone();
    rsx! {
        h1 { "Models & API Keys" }
        div { class: "sub", "Every model Larrier knows, with context windows and prices, plus providers for the built-in Chat agent. Keys live in your OS keychain." }
        super::models::ModelRegistry {}
        div { class: "set-group-title", style: "margin-top: 22px;", "Chat providers" }
        for p in providers {
            ProviderCard { key: "{p.id}", p }
        }
        div { class: "set-group-title", "Add provider" }
        div { class: "set-card",
            div { class: "set-row",
                div { class: "lbl", div { class: "t", "OpenAI-compatible endpoint" } div { class: "d", "Groq, Together, DeepSeek, Mistral, LM Studio, vLLM, Nous Portal…" } }
                button { class: "ghost-btn",
                    onclick: move |_| app.update_config(|c| c.providers.push(Provider {
                        id: format!("custom-{}", &new_id()[30..]),
                        name: "Custom provider".into(),
                        kind: ProviderKind::OpenAiCompatible,
                        base_url: "https://api.example.com/v1".into(),
                        secret_ref: None,
                        models: Vec::new(),
                        enabled: false,
                        default_model: None,
                    })),
                    Icon { i: I::Plus, size: 12 } "Add"
                }
            }
        }
    }
}

#[component]
fn ProviderCard(p: Provider) -> Element {
    let app = use_app();
    let has_key = p.secret_ref.as_deref().and_then(secrets::get).map(|k| secrets::mask(&k));
    let mut key = use_signal(String::new);
    let (i1, i2, i3, i4, i5, i6) = (p.id.clone(), p.id.clone(), p.id.clone(), p.id.clone(), p.id.clone(), p.id.clone());
    let custom = p.kind == ProviderKind::OpenAiCompatible;
    rsx! {
        div { class: "set-group-title", "{p.name}" span { class: "faint", style: "font-weight: 400", "{p.kind.label()}" } }
        div { class: "set-card",
            Row { title: "Enabled".to_string(), desc: Some("Show this provider's models in the Chat agent.".to_string()),
                Toggle { on: p.enabled, onchange: move |v| app.update_config(|c| if let Some(x) = c.providers.iter_mut().find(|x| x.id == i1) { x.enabled = v; }) }
            }
            if custom {
                Row { title: "Name".to_string(),
                    input { class: "input wide", value: "{p.name}", onchange: move |e| { let v = e.value(); app.update_config(|c| if let Some(x) = c.providers.iter_mut().find(|x| x.id == i6) { x.name = v; }) } }
                }
            }
            Row { title: "Base URL".to_string(),
                input { class: "input wide", value: "{p.base_url}",
                    onchange: move |e| { let v = e.value(); app.update_config(|c| if let Some(x) = c.providers.iter_mut().find(|x| x.id == i2) { x.base_url = v; }) },
                }
            }
            if p.kind.needs_key() {
                Row { title: "API key".to_string(), desc: has_key.clone().map(|m| format!("Saved: {m}")).or(Some("Not set".to_string())),
                    div { class: "row",
                        input { class: "input", style: "width: 220px", r#type: "password", placeholder: "Paste key", value: "{key}", oninput: move |e| key.set(e.value()) }
                        button { class: "primary-btn", disabled: key().trim().is_empty(),
                            onclick: move |_| {
                                let r = format!("provider:{}", i3);
                                match secrets::set(&r, key().trim()) {
                                    Ok(()) => {
                                        app.update_config(|c| if let Some(x) = c.providers.iter_mut().find(|x| x.id == i3) { x.secret_ref = Some(r.clone()); x.enabled = true; });
                                        key.set(String::new());
                                        app.toast("Key saved");
                                    }
                                    Err(e) => app.toast_err(e.to_string()),
                                }
                            },
                            "Save"
                        }
                    }
                }
            }
            Row { title: "Models".to_string(), desc: Some("Comma separated.".to_string()),
                input { class: "input wide", value: "{p.models.join(\", \")}",
                    onchange: move |e| { let v: Vec<String> = e.value().split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(); app.update_config(|c| if let Some(x) = c.providers.iter_mut().find(|x| x.id == i4) { x.models = v; }) },
                }
            }
            if custom {
                Row { title: "Remove provider".to_string(),
                    button { class: "ghost-btn danger", onclick: move |_| app.update_config(|c| c.providers.retain(|x| x.id != i5)), "Remove" }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ sessions

#[component]
fn Sessions() -> Element {
    let app = use_app();
    let list = app.sessions.read().clone();
    let total = list.len();
    let mut by_agent: Vec<(AgentKind, usize)> = Vec::new();
    for s in &list {
        if let Some(e) = by_agent.iter_mut().find(|e| e.0 == s.origin) {
            e.1 += 1;
        } else {
            by_agent.push((s.origin.clone(), 1));
        }
    }
    by_agent.sort_by(|a, b| b.1.cmp(&a.1));
    let root = store().root().to_string_lossy().to_string();
    let sync = app.sync_status.read().clone();
    let live_on = app.config.read().settings.live_sync;
    // Re-read the watcher status shortly after toggling (it starts in the background).
    let mut tick = use_signal(|| 0u32);
    let live_status = {
        let _ = tick();
        crate::live::status(live_on)
    };
    rsx! {
        h1 { "Sessions & Import" }
        div { class: "sub", "Every conversation is stored in the Larrier Session Format (LSF) — one portable file per session that any agent can continue." }
        div { class: "set-card",
            Row { title: "Live sync".to_string(), desc: Some(format!("Sessions you run in a terminal, an IDE or another app update here as they change. {live_status}")),
                Toggle { on: live_on, onchange: move |v: bool| {
                    app.update_config(|c| c.settings.live_sync = v);
                    crate::live::restart(app);
                    spawn(async move {
                        for _ in 0..6 {
                            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                            tick += 1;
                        }
                    });
                } }
            }
            Row { title: "Import from installed agents".to_string(), desc: Some(if sync.is_empty() { "Claude Code, Codex, Cursor, OpenCode, Gemini CLI, Pi, Hermes and more.".to_string() } else { sync.clone() }),
                button { class: "primary-btn", onclick: move |_| app.sync_imports(false), Icon { i: I::Download, size: 13 } "Import now" }
            }
            Row { title: "Import a file".to_string(), desc: Some("LSF files, native agent sessions, ChatGPT and Claude.ai data exports (conversations.json).".to_string()),
                button { class: "ghost-btn", onclick: move |_| app.import_files(), Icon { i: I::Upload, size: 13 } "Choose file…" }
            }
            Row { title: "Storage".to_string(), desc: Some(root.clone()),
                button { class: "ghost-btn", onclick: { let r = root.clone(); move |_| { let _ = open::that(&r); } }, Icon { i: I::FolderOpen, size: 13 } "Open folder" }
            }
            Row { title: "Rebuild index".to_string(), desc: Some("Re-scan session files if the sidebar looks out of date.".to_string()),
                button { class: "ghost-btn", onclick: move |_| { store().rebuild_index(); app.refresh_sessions(); app.toast("Index rebuilt"); }, Icon { i: I::Refresh, size: 13 } "Rebuild" }
            }
        }
        div { class: "set-group-title", "{total} sessions" }
        div { class: "set-card",
            for (k, n) in by_agent {
                div { key: "{k.id()}", class: "set-row",
                    AgentGlyph { kind: k.clone(), class: "md" }
                    div { class: "lbl", div { class: "t", "{k.display_name()}" } }
                    span { class: "faint", "{n}" }
                }
            }
        }
        div { class: "set-group-title", "Larrier Session Format" }
        div { class: "set-card",
            div { class: "set-row top",
                div { class: "lbl",
                    div { class: "d", style: "line-height: 1.6",
                        "A session is a JSON Lines file: a header line (id, title, agent, model, workspace, environment, native ids per agent) followed by one line per message. Messages carry typed parts — text, reasoning, tool calls with inputs, outputs and file diffs, plans, images, files and notices — plus usage and cost. "
                        "When you continue a session with a different agent, Larrier writes a native resumable session for agents that support it (Claude Code, Codex, Gemini CLI, Pi) and hands the transcript over as context for the rest."
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ workspaces

#[component]
fn Workspaces() -> Element {
    let app = use_app();
    let list = app.config.read().workspaces.clone();
    rsx! {
        h1 { "Projects" }
        div { class: "sub", "Folders your agents work in. Chats without one are in Home." }
        div { class: "set-card",
            for w in list {
                div { key: "{w.id}", class: "set-row",
                    Icon { i: I::Folder, size: 15 }
                    div { class: "lbl", div { class: "t", "{w.name}" } div { class: "d", "{w.path}" } }
                    button { class: "icon-btn", title: "Remove",
                        onclick: { let id = w.id.clone(); move |_| app.update_config(|c| c.workspaces.retain(|x| x.id != id)) },
                        Icon { i: I::Trash, size: 13 }
                    }
                }
            }
            div { class: "set-row",
                div { class: "lbl", div { class: "t", "Add a folder" } }
                button { class: "primary-btn", onclick: move |_| app.pick_workspace(false), Icon { i: I::FolderOpen, size: 13 } "Open folder…" }
            }
        }
    }
}

// ------------------------------------------------------------------ shortcuts / about

#[component]
fn Shortcuts() -> Element {
    let rows: &[(&str, &[&str])] = &[
        ("New agent", &["Ctrl", "N"]),
        ("Command palette", &["Ctrl", "K"]),
        ("Command palette", &["Ctrl", "Shift", "P"]),
        ("Toggle sidebar", &["Ctrl", "B"]),
        ("Toggle review panel", &["Ctrl", "J"]),
        ("Focus composer", &["Ctrl", "L"]),
        ("Stop agent", &["Ctrl", "."]),
        ("Settings", &["Ctrl", ","]),
        ("Import sessions", &["Ctrl", "Shift", "I"]),
        ("Files", &["Ctrl", "Shift", "E"]),
        ("Send", &["Enter"]),
        ("New line", &["Shift", "Enter"]),
        ("Close overlay", &["Esc"]),
    ];
    rsx! {
        h1 { "Keyboard Shortcuts" }
        div { class: "sub", "Use Cmd instead of Ctrl on macOS." }
        div { class: "set-card",
            for (i, (label, keys)) in rows.iter().enumerate() {
                div { key: "{i}", class: "set-row", style: "min-height: 40px;",
                    div { class: "lbl", div { class: "t", "{label}" } }
                    Kbd { keys: keys.to_vec() }
                }
            }
        }
    }
}

#[component]
fn About() -> Element {
    let app = use_app();
    let kind = crate::install::current();
    let auto = app.config.read().settings.auto_update;
    let upd = app.update.read().clone();
    let (where_t, where_d) = match kind {
        crate::install::Kind::Installed(crate::install::Scope::Machine) => ("Installed for everyone", crate::install::layout(crate::install::Scope::Machine).bin_dir.display().to_string()),
        crate::install::Kind::Installed(crate::install::Scope::User) => ("Installed for you", crate::install::layout(crate::install::Scope::User).bin_dir.display().to_string()),
        crate::install::Kind::Portable => ("Not installed", "Running this copy as it is. Run it again to install it.".to_string()),
        crate::install::Kind::Dev => ("Development build", "Built from source: updates come from your checkout.".to_string()),
    };
    let installed = matches!(kind, crate::install::Kind::Installed(_));
    let can_update = !matches!(kind, crate::install::Kind::Dev);
    let status = match &upd {
        crate::state::UpdateState::Idle => "Updates come from Larrier's releases on GitHub.".to_string(),
        crate::state::UpdateState::Checking => "Checking…".to_string(),
        crate::state::UpdateState::UpToDate => format!("Larrier {} is the newest release.", larrier_core::APP_VERSION),
        crate::state::UpdateState::Available(r) => format!("Larrier {} is available.", r.version),
        crate::state::UpdateState::Downloading { version, got, total } => {
            let pct = if *total > 0 { got * 100 / total } else { 0 };
            format!("Downloading Larrier {version}… {pct}%")
        }
        crate::state::UpdateState::Ready { version, .. } => format!("Larrier {version} is ready: restart to finish."),
        crate::state::UpdateState::Failed(e) => e.clone(),
    };
    rsx! {
        h1 { "Larrier" }
        div { class: "sub", "One window for every coding agent and AI chat." }
        div { class: "set-card",
            Row { title: "Version".to_string(), span { class: "code-inline", "{larrier_core::APP_VERSION}" } }
            Row { title: where_t.to_string(), desc: Some(where_d) }
            Row { title: "Updates".to_string(), desc: Some(status),
                match &upd {
                    _ if !can_update => rsx! {},
                    crate::state::UpdateState::Available(r) => {
                        let page = r.page.clone();
                        rsx! {
                            if !page.is_empty() {
                                button { class: "ghost-btn", onclick: move |_| { let _ = open::that(&page); }, "What's new" }
                            }
                            button { class: "primary-btn", onclick: move |_| app.download_update(), "Download" }
                        }
                    }
                    crate::state::UpdateState::Ready { .. } => rsx! {
                        button { class: "primary-btn", onclick: move |_| app.restart_to_update(), "Restart to update" }
                    },
                    crate::state::UpdateState::Checking | crate::state::UpdateState::Downloading { .. } => rsx! { super::Spinner { size: 14 } },
                    _ => rsx! {
                        button { class: "ghost-btn", onclick: move |_| app.check_updates(false), "Check for updates" }
                    },
                }
            }
            if can_update {
                Row { title: "Check automatically".to_string(), desc: Some("Look for a new release when Larrier starts, then twice a day.".to_string()),
                    super::Toggle { on: auto, onchange: move |v: bool| app.update_config(|c| c.settings.auto_update = v) }
                }
            }
            Row { title: "Session format".to_string(), span { class: "code-inline", "LSF {larrier_core::lsf::LSF_VERSION}" } }
            Row { title: "Source".to_string(), a { href: "https://github.com/Dev4YM/Larrier", "github.com/Dev4YM/Larrier" } }
            if installed {
                Row { title: "Uninstall".to_string(), desc: Some("Remove Larrier from this computer. You choose whether your sessions go too.".to_string()),
                    button { class: "ghost-btn",
                        onclick: move |_| {
                            if let Ok(me) = std::env::current_exe() {
                                if crate::install::launch(&me, &["--uninstall"]).is_ok() {
                                    std::process::exit(0);
                                }
                            }
                        },
                        "Uninstall…"
                    }
                }
            }
        }
    }
}
