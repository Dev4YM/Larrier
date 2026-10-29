//! Settings: general, agents, accounts, model providers, sessions/import,
//! workspaces, shortcuts, about.

use dioxus::prelude::*;
use larrier_core::{
    agent::catalog, config::CustomAgent, config::SidebarGroup, lsf::new_id, secrets, AccountAuth, AgentKind, Protocol,
    Provider, ProviderKind, RunMode, Theme,
};

use super::{AgentGlyph, DragRegion, Kbd, Toggle};
use crate::icons::{Icon, I};
use crate::state::{store, use_app, Modal, SettingsTab, View};

fn tab_icon(t: SettingsTab) -> I {
    match t {
        SettingsTab::General => I::Settings,
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
                button { class: "nav-row", style: "width: auto; padding: 0 8px; color: var(--fg-2);",
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
    rsx! {
        h1 { "General" }
        div { class: "sub", "Appearance and how agents run." }
        div { class: "set-group-title", "Appearance" }
        div { class: "set-card",
            Row { title: "Theme".to_string(), desc: Some("Follow the system or force light / dark.".to_string()),
                div { class: "seg",
                    for (t, l) in [(Theme::System, "System"), (Theme::Light, "Light"), (Theme::Dark, "Dark")] {
                        button { key: "{l}", class: seg(s.theme == t), onclick: move |_| { app.update_config(|c| c.settings.theme = t); app.apply_theme(); }, "{l}" }
                    }
                }
            }
            Row { title: "Font size".to_string(), desc: Some("Transcript and composer text size.".to_string()),
                select { class: "input", value: "{s.font_size}",
                    onchange: move |e| { let v = e.value().parse().unwrap_or(13); app.update_config(|c| c.settings.font_size = v); },
                    for n in [12u32, 13, 14, 15, 16] { option { value: "{n}", selected: n == s.font_size, "{n}px" } }
                }
            }
            Row { title: "Sidebar grouping".to_string(), desc: Some("How agents are grouped in the sidebar.".to_string()),
                div { class: "seg",
                    for (g, l) in [(SidebarGroup::Status, "Status"), (SidebarGroup::Workspace, "Workspace"), (SidebarGroup::Agent, "Agent")] {
                        button { key: "{l}", class: seg(s.sidebar_group == g), onclick: move |_| app.update_config(|c| c.settings.sidebar_group = g), "{l}" }
                    }
                }
            }
            Row { title: "Show reasoning".to_string(), desc: Some("Display thinking blocks in transcripts.".to_string()),
                Toggle { on: s.show_reasoning, onchange: move |v| app.update_config(|c| c.settings.show_reasoning = v) }
            }
        }
        div { class: "set-group-title", "Agents" }
        div { class: "set-card",
            Row { title: "Default agent".to_string(), desc: Some("Used for new agents.".to_string()),
                select { class: "input",
                    onchange: move |e| {
                        let k = AgentKind::from_id(&e.value());
                        app.update_config(|c| c.settings.default_agent = k.clone());
                        app.draft.write().agent = k;
                    },
                    for spec in larrier_core::agent::runnable() {
                        option { key: "{spec.kind.id()}", value: "{spec.kind.id()}", selected: spec.kind == s.default_agent, "{spec.name}" }
                    }
                }
            }
            Row { title: "Autonomy".to_string(), desc: Some("How much agents may do without asking. Applied through each agent's own permission flags.".to_string()),
                div { class: "seg",
                    for m in [RunMode::Ask, RunMode::AutoEdit, RunMode::FullAuto] {
                        button { key: "{m.label()}", class: seg(s.run_mode == m), onclick: move |_| app.update_config(|c| c.settings.run_mode = m), "{m.label()}" }
                    }
                }
            }
            Row { title: "Use worktrees by default".to_string(), desc: Some("Start each new agent in its own git worktree so parallel agents never collide.".to_string()),
                Toggle { on: s.use_worktrees, onchange: move |v| { app.update_config(|c| c.settings.use_worktrees = v); app.draft.write().worktree = v; } }
            }
            Row { title: "Send with Ctrl+Enter".to_string(), desc: Some("Otherwise Enter sends and Shift+Enter adds a line.".to_string()),
                Toggle { on: s.send_with_ctrl_enter, onchange: move |v| app.update_config(|c| c.settings.send_with_ctrl_enter = v) }
            }
            Row { title: "Import sessions on startup".to_string(), desc: Some("Pull new conversations from every installed agent when Larrier opens.".to_string()),
                Toggle { on: s.auto_import, onchange: move |v| app.update_config(|c| c.settings.auto_import = v) }
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
            select { class: "input",
                onchange: move |e| protocol.set(match e.value().as_str() { "claude" => Protocol::ClaudeStream, "plain" => Protocol::PlainText, _ => Protocol::Acp }),
                option { value: "acp", "ACP" }
                option { value: "claude", "Claude stream-json" }
                option { value: "plain", "Plain text" }
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
        div { class: "sub", "Providers for Larrier's built-in Chat agent. Keys live in your OS keychain." }
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
    rsx! {
        h1 { "Sessions & Import" }
        div { class: "sub", "Every conversation is stored in the Larrier Session Format (LSF) — one portable file per session that any agent can continue." }
        div { class: "set-card",
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
        h1 { "Workspaces" }
        div { class: "sub", "Folders and repositories your agents work in." }
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
    rsx! {
        h1 { "Larrier" }
        div { class: "sub", "One window for every coding agent and AI chat." }
        div { class: "set-card",
            Row { title: "Version".to_string(), span { class: "code-inline", "{larrier_core::APP_VERSION}" } }
            Row { title: "Session format".to_string(), span { class: "code-inline", "LSF {larrier_core::lsf::LSF_VERSION}" } }
            Row { title: "Source".to_string(), a { href: "https://github.com/Dev4YM/Larrier", "github.com/Dev4YM/Larrier" } }
        }
    }
}
