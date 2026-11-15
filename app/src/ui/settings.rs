//! Settings: general, appearance, usage, harnesses (with their accounts and
//! models), source control, sessions/import, workspaces, shortcuts, about.

use dioxus::prelude::*;
use larrier_core::{
    agent::catalog, config::CustomAgent, secrets, Account, AccountAuth, AgentKind, Protocol, RunMode,
};

use super::{AgentGlyph, DragRegion, Kbd, Toggle};
use crate::icons::{Icon, I};
use crate::state::{store, use_app, Modal, SettingsTab, View};

fn tab_icon(t: SettingsTab) -> I {
    match t {
        SettingsTab::Profile => I::CircleUser,
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

/// Settings navigation — takes the sidebar's place while Settings is open.
#[component]
pub fn SettingsNav() -> Element {
    let app = use_app();
    let tab = match &*app.view.read() {
        View::Settings(t) => *t,
        _ => SettingsTab::General,
    };
    let mut q = crate::keep::use_kept(crate::keep::WINDOW, "settings.search", String::new);
    let ql = q.read().to_lowercase();
    let outdated = crate::agent_versions::outdated(&app);
    let name = crate::state::display_name(&app.config.read());
    rsx! {
        aside { class: "sidebar",
            div { class: "side-top",
                button { class: "nav-row back-row",
                    onclick: move |_| {
                        app.leave_settings();
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
                            onclick: move |_| app.set_view(View::Settings(t)),
                            span { class: "ic", Icon { i: tab_icon(t), size: 16, stroke: 1.5 } }
                            "{t.label()}"
                            if t == SettingsTab::Agents && outdated > 0 {
                                span { class: "nav-badge", title: "Agents with a newer version", "{outdated}" }
                            }
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
                button { class: "side-me", title: "Your profile", onclick: move |_| app.set_view(View::Settings(SettingsTab::Profile)),
                    super::profile::Avatar { size: 30 }
                    span { class: "nm", "{name}" }
                }
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
                        SettingsTab::Profile => rsx! { super::profile::ProfileView {} },
                        SettingsTab::SourceControl => rsx! { SourceControl {} },
                        SettingsTab::Agents | SettingsTab::Accounts | SettingsTab::Models => rsx! { Harnesses {} },
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
            Row { title: "While an Agent Works, Enter".to_string(),
                desc: Some(format!("{}. {} does the other of steer and queue (it never stops the agent). Agents that can't read messages mid-turn get them queued.", s.busy_send.description(), crate::platform::keys("Ctrl+Enter"))),
                super::Select {
                    value: format!("{:?}", s.busy_send),
                    options: larrier_core::BusySend::ALL.iter().map(|b| (format!("{b:?}"), b.label().to_string())).collect(),
                    onchange: move |v: String| {
                        if let Some(b) = larrier_core::BusySend::ALL.into_iter().find(|b| format!("{b:?}") == v) {
                            app.update_config(move |c| c.settings.busy_send = b);
                        }
                    },
                }
            }
            if larrier_agents::browser::SUPPORTED {
                Row { title: "Browser for Agents".to_string(), desc: Some("Claude Code, Codex and agents connected over ACP can open pages, click, type and take screenshots in Larrier's browser, which you watch in the Browser tab.".to_string()),
                    Toggle { on: s.browser_tools, onchange: move |v| app.update_config(|c| c.settings.browser_tools = v) }
                }
            } else {
                Row { title: "Browser for Agents".to_string(), desc: Some("Windows only for now: agents drive Larrier's browser through its DevTools port, which this system's web engine doesn't have. The Browser tab is yours to browse in.".to_string()),
                    span { class: "faint", "Not available" }
                }
            }
            Row { title: format!("Send with {}", crate::platform::keys("Ctrl+Enter")), desc: Some("Otherwise Enter sends and Shift+Enter adds a line.".to_string()),
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
                    None => rsx! { super::loaders::Orbit { size: 13 } },
                }
            }
            Row { title: "GitHub CLI".to_string(), desc: Some("Needed for pull requests (gh auth login).".to_string()),
                match gh.read().clone() {
                    Some(true) => rsx! { span { class: "status-pill ok", Icon { i: I::Check, size: 11 } "Found" } },
                    Some(false) => rsx! { a { href: "https://cli.github.com", "Install GitHub CLI" } },
                    None => rsx! { super::loaders::Orbit { size: 13 } },
                }
            }
        }
        div { class: "set-group-title", "Worktrees" }
        div { class: "set-card",
            Row { title: "Use Worktrees by Default".to_string(), desc: Some("Start each new agent in its own git worktree so parallel agents never collide.".to_string()),
                Toggle { on: s.use_worktrees, onchange: move |v| { app.update_config(|c| c.settings.use_worktrees = v); let mut d = app.draft.write(); d.worktree = v; d.existing_worktree = None; } }
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

pub fn protocol_label(p: Protocol) -> &'static str {
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
pub fn AddCustomAgent() -> Element {
    let app = use_app();
    let mut name = crate::keep::use_kept(crate::keep::WINDOW, "settings.custom.name", String::new);
    let mut command = crate::keep::use_kept(crate::keep::WINDOW, "settings.custom.command", String::new);
    let mut protocol = crate::keep::use_kept(crate::keep::WINDOW, "settings.custom.protocol", || Protocol::Acp);
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

// ------------------------------------------------------------------ harnesses

/// Settings → Harnesses: everything that runs a chat, or one harness's page
/// (`ui.models_agent`).
#[component]
fn Harnesses() -> Element {
    let app = use_app();
    let sel = app.ui.read().models_agent.clone();
    match sel {
        Some(k) => rsx! { HarnessPage { key: "{k.id()}", kind: k } },
        None => rsx! { HarnessList {} },
    }
}

/// What a harness runs as, in a few words.
fn runs_as(cfg: &larrier_core::Config, kind: &AgentKind) -> String {
    let list = cfg.accounts_for(kind);
    match (list.len(), cfg.default_account(kind)) {
        (0, _) if *kind == AgentKind::Chat => "No API account yet".into(),
        (0, _) => "Current sign-in".into(),
        (1, Some(a)) => a.label.clone(),
        (n, Some(a)) => format!("{} · {n} accounts", a.label),
        _ => String::new(),
    }
}

#[component]
fn HarnessList() -> Element {
    let mut app = use_app();
    let detected = app.detected.read().clone();
    let customs = app.config.read().custom_agents.clone();
    let installed = |k: &AgentKind| detected.iter().any(|d| &d.kind == k && d.installed());
    let agents: Vec<AgentKind> = catalog().iter().filter(|s| !s.binaries.is_empty()).map(|s| s.kind.clone()).collect();
    let (have, more): (Vec<AgentKind>, Vec<AgentKind>) = agents.into_iter().partition(|k| installed(k));
    let mut show_more = crate::keep::use_kept(crate::keep::WINDOW, "harnesses.more", || false);
    rsx! {
        h1 { "Harnesses" }
        div { class: "sub", "What runs your chats: Larrier Chat and the coding agents on this computer, each with the accounts it runs as." }
        div { class: "set-card hn-list",
            HarnessRow { kind: AgentKind::Chat }
            for k in have {
                HarnessRow { key: "{k.id()}", kind: k }
            }
            for c in customs {
                HarnessRow { key: "c-{c.id}", kind: AgentKind::Custom(c.id.clone()) }
            }
        }
        div { class: "hn-foot",
            button { class: "ghost-btn sm", onclick: move |_| app.ui.write().modal = Some(Modal::AddCustomAgent),
                Icon { i: I::Plus, size: 12 } "Add a custom agent"
            }
            button { class: "ghost-btn sm", title: "Find agents again and look up their latest versions",
                onclick: move |_| { app.detect_agents(); crate::agent_versions::check_now(); app.toast("Finding agents and their latest versions…"); },
                Icon { i: I::Refresh, size: 12 } "Find agents again"
            }
        }
        if !more.is_empty() {
            button { class: "hn-more", onclick: move |_| show_more.set(!show_more()),
                span { class: "chev", if show_more() { Icon { i: I::ChevronDown, size: 12 } } else { Icon { i: I::ChevronRight, size: 12 } } }
                "Not installed ({more.len()})"
            }
            if show_more() {
                div { class: "set-card hn-list",
                    for k in more {
                        HarnessRow { key: "{k.id()}", kind: k }
                    }
                }
            }
        }
    }
}

/// A harness in the list: what it runs as and how, and its version.
#[component]
fn HarnessRow(kind: AgentKind) -> Element {
    let mut app = use_app();
    let spec = kind.spec();
    let chat = kind == AgentKind::Chat;
    let custom = matches!(kind, AgentKind::Custom(_));
    let d = app.detected.read().iter().find(|d| d.kind == kind).cloned();
    let installed = chat || custom || d.as_ref().is_some_and(|d| d.installed());
    let ac = app.config.read().agent(&kind);
    let on = chat || custom || ac.enabled;
    let newer = if chat || custom { None } else { crate::agent_versions::update_to(&app, &kind) };
    let (name, meta) = match &kind {
        AgentKind::Custom(id) => {
            let c = app.config.read().custom_agent(id).cloned();
            (c.as_ref().map(|c| c.name.clone()).unwrap_or_else(|| id.clone()), c.map(|c| format!("{} · {}", c.command, protocol_label(c.protocol))).unwrap_or_default())
        }
        _ if !installed => (spec.name.to_string(), spec.vendor.to_string()),
        _ => {
            let how = super::harness_run::current_label(&app.config.read(), &kind);
            (spec.name.to_string(), [runs_as(&app.config.read(), &kind), how].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · "))
        }
    };
    let status = if chat || custom {
        String::new()
    } else if !installed {
        if d.is_some() { "Not installed".into() } else { String::new() }
    } else if !on {
        "Off".into()
    } else {
        d.as_ref().and_then(|d| d.version.clone()).map(|v| format!("v{v}")).unwrap_or_default()
    };
    let k1 = kind.clone();
    rsx! {
        button { class: if on || !installed { "hn-row" } else { "hn-row off" },
            onclick: move |_| app.ui.write().models_agent = Some(k1.clone()),
            AgentGlyph { kind: kind.clone(), class: "lg" }
            div { class: "lbl",
                div { class: "t", "{name}" }
                div { class: "d", "{meta}" }
            }
            if let Some(v) = newer {
                span { class: "hn-tag new", "v{v} available" }
            } else if !status.is_empty() {
                span { class: "hn-tag", "{status}" }
            }
            span { class: "chev", Icon { i: I::ChevronRight, size: 13 } }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum HarnessTab {
    Accounts,
    Models,
    Running,
    Version,
}

/// One harness, in tabs: its accounts, its models, how it runs, its version.
#[component]
fn HarnessPage(kind: AgentKind) -> Element {
    let mut app = use_app();
    let spec = kind.spec();
    let chat = kind == AgentKind::Chat;
    let custom = matches!(kind, AgentKind::Custom(_));
    let d = app.detected.read().iter().find(|d| d.kind == kind).cloned();
    let installed = chat || custom || d.as_ref().is_some_and(|d| d.installed());
    let ac = app.config.read().agent(&kind);
    let mut tab = use_signal(|| if installed || chat { HarnessTab::Accounts } else { HarnessTab::Version });
    let newer = if chat || custom { None } else { crate::agent_versions::update_to(&app, &kind) };
    let job = crate::agent_versions::JOBS.read().get(&kind.id()).cloned();
    let running = job.as_ref().is_some_and(|j| j.ok.is_none());
    let name = match &kind {
        AgentKind::Custom(id) => app.config.read().custom_agent(id).map(|c| c.name.clone()).unwrap_or_else(|| id.clone()),
        _ => spec.name.to_string(),
    };
    let line = if chat {
        "Larrier's own chat: talks to model APIs itself, on your API accounts.".to_string()
    } else if custom {
        "A custom agent: a command of your own.".to_string()
    } else {
        match d.as_ref() {
            Some(d) if d.installed() => format!("{} · {}", spec.vendor, d.version.clone().map(|v| format!("v{v}")).unwrap_or_else(|| "installed".into())),
            Some(_) => format!("{} · not installed", spec.vendor),
            None => spec.vendor.to_string(),
        }
    };
    let tabs: Vec<(HarnessTab, &str)> = if chat {
        vec![]
    } else if custom {
        vec![(HarnessTab::Running, "How it runs")]
    } else {
        vec![(HarnessTab::Accounts, "Accounts"), (HarnessTab::Models, "Models"), (HarnessTab::Running, "How it runs"), (HarnessTab::Version, "Version")]
    };
    let cur = if custom { HarnessTab::Running } else { tab() };
    let (k1, k2, k3) = (kind.clone(), kind.clone(), kind.clone());
    rsx! {
        button { class: "hn-back", onclick: move |_| app.ui.write().models_agent = None,
            Icon { i: I::ArrowLeft, size: 13 }
            "Harnesses"
        }
        div { class: "hn-head",
            AgentGlyph { kind: kind.clone(), class: "lg" }
            div { class: "lbl",
                h1 { "{name}" }
                div { class: "sub", "{line}" }
            }
            if let Some(v) = newer.clone() {
                button { class: "primary-btn", disabled: running, onclick: move |_| crate::agent_versions::run(app, k1.clone(), false),
                    Icon { i: I::ArrowUp, size: 12 } "Update to v{v}"
                }
            }
            if installed && !chat && !custom {
                div { class: "hn-on", title: if ac.enabled { "On: offered in pickers, its sessions sync" } else { "Off: hidden from pickers and the sidebar, its sessions don't sync" },
                    span { if ac.enabled { "On" } else { "Off" } }
                    Toggle { on: ac.enabled, onchange: move |v| app.update_config(|c| c.agent_mut(&k2).enabled = v) }
                }
            }
        }
        if tabs.len() > 1 {
            div { class: "hn-tabs", role: "tablist",
                for (t, label) in tabs {
                    button { key: "{label}", class: if cur == t { "on" } else { "" }, role: "tab", onclick: move |_| tab.set(t), "{label}" }
                }
            }
        }
        match cur {
            HarnessTab::Accounts => rsx! {
                div { class: "hn-sec-head",
                    div { class: "hn-line",
                        if chat { "Each account is one API: a provider's own, or any endpoint that speaks it, with the models it offers. Keys stay in your OS keychain." }
                        else { "The default runs new chats; a chat can switch in its model picker. Keys stay in your OS keychain." }
                    }
                    if installed {
                        button { class: "ghost-btn sm", onclick: move |_| app.ui.write().modal = Some(Modal::AddAccount(k3.clone())),
                            Icon { i: I::Plus, size: 12 } "Add account"
                        }
                    }
                }
                AgentAccounts { kind: kind.clone() }
            },
            HarnessTab::Models => rsx! { super::agent_models::AgentModelsPanel { kind: kind.clone() } },
            HarnessTab::Running => rsx! { super::harness_run::RunPanel { kind: kind.clone() } },
            HarnessTab::Version => rsx! { VersionPanel { kind: kind.clone() } },
        }
    }
}

/// A harness's version: installed, latest, and updating or installing it.
#[component]
fn VersionPanel(kind: AgentKind) -> Element {
    let app = use_app();
    let spec = kind.spec();
    let id = kind.id();
    let d = app.detected.read().iter().find(|d| d.kind == kind).cloned();
    let installed = d.as_ref().is_some_and(|d| d.installed());
    let job = crate::agent_versions::JOBS.read().get(&id).cloned();
    let running = job.as_ref().is_some_and(|j| j.ok.is_none());
    let newer = crate::agent_versions::update_to(&app, &kind);
    let latest = crate::agent_versions::LATEST.read().get(&id).cloned();
    let checking = crate::agent_versions::CHECKING();
    let versions = larrier_agents::updates::of(&kind);
    let update_cmd = if installed { crate::agent_versions::step(&app, &kind, false) } else { None };
    let install_cmd = if installed { None } else { larrier_agents::updates::install_step(&kind) };
    let (k1, k2) = (kind.clone(), kind.clone());
    rsx! {
        div { class: "set-card ad-card",
            div { class: "ad-row",
                span { class: "k", "Installed" }
                div { class: "v",
                    span { class: "plain strong",
                        match d.as_ref() {
                            Some(d) if d.installed() => d.version.clone().map(|v| format!("v{v}")).unwrap_or("Installed (its version isn't known)".into()),
                            Some(_) => "Not installed".to_string(),
                            None => "Looking…".to_string(),
                        }
                    }
                    if let Some(p) = d.as_ref().and_then(|d| d.path.clone()) {
                        span { class: "h", title: "{p.to_string_lossy()}", {short_path(&p.to_string_lossy())} }
                    }
                }
            }
            if let Some(vs) = versions {
                div { class: "ad-row",
                    span { class: "k", "Latest" }
                    div { class: "v",
                        div { class: "row",
                            span { class: "plain",
                                match (&latest, checking) {
                                    (Some(v), _) => format!("v{v}"),
                                    (None, true) => "Looking it up…".to_string(),
                                    (None, false) => "Not looked up yet".to_string(),
                                }
                            }
                            if newer.is_some() { span { class: "hn-tag new", "Newer" } }
                            else if latest.is_some() && installed && d.as_ref().is_some_and(|d| d.version.is_some()) { span { class: "hn-tag ok", "Up to date" } }
                            div { class: "grow" }
                            button { class: "ghost-btn sm", disabled: checking, onclick: move |_| crate::agent_versions::check_now(), "Check now" }
                        }
                        div { class: "h", "From {larrier_agents::updates::source_label(&vs.source)}" }
                    }
                }
            }
            if let Some(c) = update_cmd.clone() {
                div { class: "ad-row",
                    span { class: "k", "Update" }
                    div { class: "v",
                        div { class: "row",
                            span { class: "cmd mono", title: "{c.shown}", "{c.shown}" }
                            button { class: if newer.is_some() { "primary-btn" } else { "ghost-btn" }, disabled: running,
                                onclick: move |_| crate::agent_versions::run(app, k1.clone(), false),
                                if let Some(v) = &newer { "Update to v{v}" } else { "Update" }
                            }
                        }
                        div { class: "h", "As {spec.vendor} says to update it; Larrier runs it and shows what it prints." }
                    }
                }
            }
            if let Some(c) = install_cmd.clone().filter(|_| d.is_some()) {
                div { class: "ad-row",
                    span { class: "k", "Install" }
                    div { class: "v",
                        div { class: "row",
                            span { class: "cmd mono", title: "{c.shown}", "{c.shown}" }
                            button { class: "primary-btn", disabled: running, onclick: move |_| crate::agent_versions::run(app, k2.clone(), true), "Install" }
                        }
                        div { class: "h", "Its official installer; Larrier runs it and shows what it prints." }
                    }
                }
            }
            if let Some(j) = job.clone() {
                div { class: "ad-row",
                    span { class: "k", if j.installing { "Installing" } else { "Updating" } }
                    div { class: "v",
                        div { class: "plain",
                            match j.ok {
                                None => rsx! { span { class: "row", super::loaders::Orbit { size: 12 } span { class: "mono", "{j.shown}" } } },
                                Some(true) => rsx! { span { class: "ok-c", "Done: " span { class: "mono", "{j.shown}" } } },
                                Some(false) => rsx! { span { class: "err-c", "Didn't go through: " span { class: "mono", "{j.shown}" } } },
                            }
                        }
                        div { class: "ad-log mono",
                            if j.log.is_empty() { div { class: "faint", "Nothing printed yet" } }
                            for (i, l) in j.log.iter().enumerate().rev() {
                                div { key: "{i}", "{l}" }
                            }
                        }
                    }
                }
            }
        }
        div { class: "ad-foot",
            a { href: "{spec.homepage}", "Documentation" Icon { i: I::ArrowUpRight, size: 11 } }
        }
    }
}

/// A path as people read it: the home folder as `~`, and the middle left
/// out when it's long (the full path is in its tooltip).
pub fn short_path(p: &str) -> String {
    let home = dirs::home_dir().map(|h| h.to_string_lossy().to_string()).unwrap_or_default();
    let p = if !home.is_empty() && p.to_lowercase().starts_with(&home.to_lowercase()) { format!("~{}", &p[home.len()..]) } else { p.to_string() };
    let chars: Vec<char> = p.chars().collect();
    if chars.len() <= 56 {
        return p;
    }
    let head: String = chars[..22].iter().collect();
    let tail: String = chars[chars.len() - 30..].iter().collect();
    format!("{head}…{tail}")
}

// ------------------------------------------------------------------ accounts

/// A harness's accounts, the same wherever they show: what each runs as,
/// whether it is signed in or has its key, which is the default, and how
/// many chats use it. One action shows when something is needed; the rest
/// are in its "…" menu.
#[component]
pub fn AgentAccounts(kind: AgentKind) -> Element {
    let app = use_app();
    let chat = kind == AgentKind::Chat;
    let accounts: Vec<Account> = app.config.read().accounts_for(&kind).into_iter().cloned().collect();
    // With no accounts, an agent runs on whatever it's signed into.
    let implicit = (accounts.is_empty() && !chat).then(|| Account::new(kind.clone(), "Current sign-in".to_string(), AccountAuth::SystemLogin));
    let rows: Vec<(Account, bool)> = match implicit {
        Some(a) => vec![(a, true)],
        None => accounts.into_iter().map(|a| (a, false)).collect(),
    };
    rsx! {
        div { class: "set-card",
            if rows.is_empty() {
                div { class: "hn-empty", "No API account yet. Add one for Anthropic, OpenAI, Google Gemini, OpenRouter, Ollama or any OpenAI-compatible endpoint." }
            }
            for (a, only) in rows {
                AccountRow { key: "{a.id}", a, only }
            }
        }
    }
}

/// Apply a change to one saved account.
pub fn edit_account(app: crate::state::App, id: String, f: impl FnOnce(&mut Account) + 'static) {
    app.update_config(move |c| {
        if let Some(a) = c.accounts.iter_mut().find(|a| a.id == id) {
            f(a);
        }
    });
}

#[component]
fn AccountRow(a: Account, only: bool) -> Element {
    let mut app = use_app();
    let spec = a.agent.spec();
    let mut menu = use_signal(|| false);
    let mut checked = use_signal(|| None::<Result<String, String>>);
    let mut checking = use_signal(|| false);
    let (signed, email) = crate::state::signed_in_as(&a);
    // Only Claude Code and Codex say whether they're signed in.
    let knows_sign_in = matches!(a.agent, AgentKind::ClaudeCode | AgentKind::Codex);
    let can_sign_in = spec.login_command.is_some() && a.auth.is_sign_in();
    let api = match &a.auth {
        AccountAuth::Api(x) => Some(x.clone()),
        _ => None,
    };
    let key = a.auth.secret_ref().and_then(secrets::get).filter(|k| !k.trim().is_empty());
    let needs_key = api.as_ref().is_some_and(|x| x.api.needs_key()) && key.is_none();
    let used = app.sessions.read().iter().filter(|s| s.account_id.as_deref() == Some(a.id.as_str())).count();
    let what = match &a.auth {
        AccountAuth::SystemLogin => format!("Whatever {} is signed into on this computer", spec.name),
        AccountAuth::ConfigDir { path } => format!("Separate sign-in · {}", short_path(path)),
        AccountAuth::Api(x) => {
            let mut s = x.api.label().to_string();
            match &x.env_var {
                Some(v) => s.push_str(&format!(" · as {v}")),
                None => {
                    if let Some(u) = x.base_url.clone().filter(|u| !u.trim().is_empty() && u != x.api.default_url()) {
                        s.push_str(&format!(" · {}", u.trim_start_matches("https://").trim_start_matches("http://")));
                    }
                }
            }
            if a.agent == AgentKind::Chat {
                s.push_str(&match x.models.len() {
                    0 => " · no models".to_string(),
                    1 => " · 1 model".to_string(),
                    n => format!(" · {n} models"),
                });
            }
            s
        }
        AccountAuth::ApiKey { env_var, .. } => format!("API key · as {env_var}"),
        AccountAuth::Env { vars } => format!("{} variables", vars.len()),
    };
    // Its state, in a word or two.
    let (state, good) = if checking() {
        ("Checking…".to_string(), true)
    } else if let Some(r) = checked() {
        match r {
            Ok(m) => (m, true),
            Err(e) => (format!("Check failed: {e}"), false),
        }
    } else {
        match &a.auth {
            AccountAuth::SystemLogin | AccountAuth::ConfigDir { .. } if knows_sign_in => match (signed, email) {
                (true, Some(e)) => (e, true),
                (true, None) => ("Signed in".to_string(), true),
                (false, _) => ("Not signed in".to_string(), false),
            },
            AccountAuth::Api(x) if !x.api.needs_key() => (String::new(), true),
            AccountAuth::Api(_) | AccountAuth::ApiKey { .. } => match &key {
                Some(k) => (format!("Key {}", secrets::mask(k)), true),
                None => ("No key".to_string(), false),
            },
            _ => (String::new(), true),
        }
    };
    let can_check = api.as_ref().is_some_and(|x| x.api != larrier_core::ApiKind::Service && !x.url().is_empty());
    let is_default = only || a.is_default;
    let icon = match a.auth {
        AccountAuth::Api(_) | AccountAuth::ApiKey { .. } => I::Key,
        AccountAuth::ConfigDir { .. } => I::Lock,
        AccountAuth::Env { .. } => I::Terminal,
        AccountAuth::SystemLogin => I::User,
    };
    let x_check = api.clone();
    let key_check = key.clone();
    let (id1, id2, id3, id4) = (a.id.clone(), a.id.clone(), a.id.clone(), a.id.clone());
    let (a_sign, a_sign2, a_del) = (a.clone(), a.clone(), a.clone());
    let k_def = a.agent.clone();
    rsx! {
        div { class: "hn-acct",
            span { class: "ic", Icon { i: icon, size: 15 } }
            div { class: "lbl",
                div { class: "t",
                    span { class: "ellipsis", "{a.label}" }
                    if is_default { span { class: "hn-default", "Default" } }
                }
                div { class: "d",
                    span { "{what}" }
                    if !state.is_empty() { span { class: if good { "st ok" } else { "st off" }, title: "{state}", "{state}" } }
                    if used > 0 { span { class: "st", if used == 1 { "1 chat" } else { "{used} chats" } } }
                }
            }
            // The one thing it needs, when it needs one.
            if needs_key {
                button { class: "ghost-btn sm", onclick: move |_| app.ui.write().modal = Some(Modal::EditAccount(id1.clone())), "Add key" }
            } else if can_sign_in && knows_sign_in && !signed {
                button { class: "ghost-btn sm", onclick: move |_| app.sign_in(&a_sign), "Sign in" }
            }
            div { class: "menu-anchor",
                button { class: if menu() { "icon-btn sm active" } else { "icon-btn sm" }, title: "More", onclick: move |_| menu.set(true),
                    Icon { i: I::More, size: 14 }
                }
                super::Dropdown { open: menu, up: false, right: true,
                    if !is_default {
                        super::MenuItem { label: "Make default".to_string(), icon: Some(I::Check),
                            onclick: move |_| { menu.set(false); let id = id2.clone(); let k = k_def.clone(); app.update_config(move |c| for x in c.accounts.iter_mut().filter(|x| x.agent == k) { x.is_default = x.id == id; }); } }
                    }
                    if can_check {
                        super::MenuItem { label: "Check".to_string(), sub: Some("asks the API".to_string()), icon: Some(I::Refresh),
                            onclick: move |_| {
                                menu.set(false);
                                let Some(x) = x_check.clone() else { return };
                                let k = key_check.clone();
                                checking.set(true);
                                checked.set(None);
                                spawn(async move {
                                    let r = larrier_agents::drivers::chat::check_api(x.api, &x.url(), k.as_deref()).await;
                                    checking.set(false);
                                    checked.set(Some(r.map(|list| match list.len() {
                                        0 => "Works".to_string(),
                                        n => format!("Works · {n} models"),
                                    })));
                                });
                            } }
                    }
                    if api.is_some() && !only {
                        super::MenuItem { label: "Edit…".to_string(), icon: Some(I::Pencil),
                            onclick: move |_| { menu.set(false); app.ui.write().modal = Some(Modal::EditAccount(id3.clone())); } }
                    }
                    if can_sign_in {
                        super::MenuItem { label: if knows_sign_in && signed { "Sign in again".to_string() } else { "Sign in".to_string() }, icon: Some(I::User),
                            onclick: move |_| { menu.set(false); app.sign_in(&a_sign2); } }
                    }
                    if !only {
                        div { class: "menu-sep" }
                        super::MenuItem { label: "Remove".to_string(), icon: Some(I::Trash), danger: true,
                            onclick: move |_| {
                                menu.set(false);
                                if let Some(r) = a_del.auth.secret_ref() { secrets::delete(r); }
                                let id = id4.clone();
                                app.update_config(move |c| {
                                    c.accounts.retain(|x| x.id != id);
                                    // Models added for it run on the chat's account now.
                                    for ag in c.agents.iter_mut() {
                                        for m in ag.custom_models.iter_mut().filter(|m| m.account.as_deref() == Some(id.as_str())) { m.account = None; }
                                    }
                                });
                            } }
                    }
                }
            }
        }
    }
}

/// Everything an API account holds (in its Edit dialog): its name, which API,
/// its address, its key and (for Larrier Chat) the models it offers.
#[component]
pub fn ApiEditor(a: Account, api: larrier_core::ApiAccount) -> Element {
    let app = use_app();
    let spec = a.agent.spec();
    let chat = a.agent == AgentKind::Chat;
    let mut key = use_signal(String::new);
    let kinds = larrier_agents::request::api_kinds(&a.agent);
    let saved = api.secret_ref.as_deref().and_then(secrets::get).filter(|k| !k.trim().is_empty()).map(|k| secrets::mask(&k));
    let [i1, i2, i3, i4, i5] = [a.id.clone(), a.id.clone(), a.id.clone(), a.id.clone(), a.id.clone()];
    let r_default = api.secret_ref.clone().unwrap_or_else(|| format!("account:{}", a.id));
    rsx! {
        div { class: "hn-edit kv",
            span { class: "k", "Name" }
            input { class: "input full", value: "{a.label}",
                onchange: move |e| { let v = e.value(); if !v.trim().is_empty() { edit_account(app, i1.clone(), move |x| x.label = v); } } }
            if let Some(var) = api.env_var.clone() {
                span { class: "k", "Variable" }
                super::Select {
                    value: var,
                    options: spec.key_env.iter().map(|v| (v.to_string(), v.to_string())).collect::<Vec<_>>(),
                    onchange: move |v: String| edit_account(app, i2.clone(), move |x| if let AccountAuth::Api(p) = &mut x.auth { p.api = larrier_core::ApiKind::for_variable(&v); p.env_var = Some(v); }),
                }
            } else {
                if kinds.len() > 1 {
                    span { class: "k", "API" }
                    super::Select {
                        value: api.api.id().to_string(),
                        options: kinds.iter().map(|k| (k.id().to_string(), k.label().to_string())).collect::<Vec<_>>(),
                        onchange: move |v: String| {
                            if let Some(k) = larrier_core::ApiKind::from_id(&v) {
                                edit_account(app, i2.clone(), move |x| if let AccountAuth::Api(p) = &mut x.auth { p.api = k; });
                            }
                        },
                    }
                }
                span { class: "k", "Address" }
                input { class: "input full", value: "{api.base_url.clone().unwrap_or_default()}", placeholder: if api.api.default_url().is_empty() { "https://…/v1" } else { api.api.default_url() }, spellcheck: "false",
                    onchange: move |e| { let v = e.value().trim().to_string(); edit_account(app, i3.clone(), move |x| if let AccountAuth::Api(p) = &mut x.auth { p.base_url = (!v.is_empty()).then_some(v); }); } }
            }
            if api.api.needs_key() {
                span { class: "k", "Key" }
                div { class: "row", style: "gap: 6px;",
                    input { class: "input grow", r#type: "password", placeholder: if saved.is_some() { "Paste a new key to replace it" } else { "Paste the key" }, value: "{key}", oninput: move |e| key.set(e.value()) }
                    button { class: "ghost-btn", disabled: key().trim().is_empty(),
                        onclick: move |_| {
                            let r = r_default.clone();
                            match secrets::set(&r, key().trim()) {
                                Ok(()) => {
                                    edit_account(app, i4.clone(), move |x| if let AccountAuth::Api(p) = &mut x.auth { p.secret_ref = Some(r); });
                                    key.set(String::new());
                                    app.toast("Key saved to your keychain");
                                }
                                Err(e) => app.toast_err(format!("Couldn't save the key: {e}")),
                            }
                        },
                        "Save key"
                    }
                }
            }
            if chat {
                span { class: "k", "Models" }
                input { class: "input full", value: "{api.models.join(\", \")}", placeholder: "Model ids, comma separated", spellcheck: "false",
                    onchange: move |e| {
                        let v: Vec<String> = e.value().split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                        edit_account(app, i5.clone(), move |x| if let AccountAuth::Api(p) = &mut x.auth { p.models = v; });
                    } }
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
    let mut app = use_app();
    let mut list = app.config.read().workspaces.clone();
    list.sort_by_key(|w| (!w.pinned, w.name.to_lowercase()));
    let home = app.config.read().home_project();
    rsx! {
        h1 { "Projects" }
        div { class: "sub", "Each has a name, an icon and a color, its folders and the agents it works with. Chats started in its folders are its chats; chats without one are in Home." }
        div { class: "set-card",
            for w in list {
                div { key: "{w.id}", class: "set-row pj-row",
                    super::projects::ProjectMark { icon: w.icon.clone(), color: w.color.clone(), size: 16 }
                    div { class: "lbl",
                        div { class: "t", "{w.name}" }
                        div { class: "d",
                            "{larrier_core::lsf::short_path(&w.path)}"
                            if !w.folders.is_empty() { " · {w.folders.len() + 1} folders" }
                        }
                    }
                    button { class: if w.pinned { "icon-btn on" } else { "icon-btn" }, title: if w.pinned { "Unpin" } else { "Pin to the top of the sidebar" },
                        onclick: { let p = w.path.clone(); move |_| app.toggle_project_pin(&p) },
                        Icon { i: if w.pinned { I::PinOff } else { I::Pin }, size: 13 }
                    }
                    button { class: "ghost-btn", onclick: { let id = w.id.clone(); move |_| app.ui.write().modal = Some(crate::state::Modal::Project(Some(id.clone()), None)) }, "Edit" }
                }
            }
            div { class: "set-row pj-row",
                super::projects::ProjectMark { icon: home.icon.clone(), color: home.color.clone(), size: 16, home: true }
                div { class: "lbl", div { class: "t", "{home.name}" } div { class: "d", "Chats without a project folder" } }
                button { class: if home.pinned { "icon-btn on" } else { "icon-btn" }, title: if home.pinned { "Unpin" } else { "Pin to the top of the sidebar" },
                    onclick: move |_| app.toggle_home_pin(),
                    Icon { i: if home.pinned { I::PinOff } else { I::Pin }, size: 13 }
                }
                button { class: "ghost-btn", onclick: move |_| app.edit_home(), "Edit" }
            }
            div { class: "set-row",
                div { class: "lbl", div { class: "t", "New project" } div { class: "d", "Pick its folder, then name it and give it a look." } }
                button { class: "primary-btn", onclick: move |_| app.edit_project(None), Icon { i: I::FolderPlus, size: 13 } "New project" }
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
        ("Stop the reply", &["Ctrl", "."]),
        ("Settings", &["Ctrl", ","]),
        ("Import sessions", &["Ctrl", "Shift", "I"]),
        ("Files", &["Ctrl", "Shift", "E"]),
        ("Send", &["Enter"]),
        ("New line", &["Shift", "Enter"]),
        ("Paste long text as text, not a file", &["Ctrl", "Shift", "V"]),
        ("Close overlay", &["Esc"]),
    ];
    rsx! {
        h1 { "Keyboard Shortcuts" }
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
    let mut app = use_app();
    let kind = crate::install::current();
    let mode = if app.config.read().settings.auto_update { app.config.read().settings.updates } else { larrier_core::UpdateMode::Off };
    let channel = app.config.read().settings.update_channel;
    let built = match (crate::version::COMMIT, crate::version::COMMIT_DATE) {
        ("" | "unknown", _) => None,
        (c, "") => Some(format!("Commit {c}")),
        (c, d) => Some(format!("Commit {c} · {d}")),
    };
    let upd = app.update.read().clone();
    let (where_t, where_d) = match kind {
        crate::install::Kind::Installed(crate::install::Scope::Machine) => ("Installed for everyone", crate::install::layout(crate::install::Scope::Machine).bin_dir.display().to_string()),
        crate::install::Kind::Installed(crate::install::Scope::User) => ("Installed for you", crate::install::layout(crate::install::Scope::User).bin_dir.display().to_string()),
        crate::install::Kind::Portable => ("Not installed", "Running this copy as it is. Run it again to install it.".to_string()),
        crate::install::Kind::App => {
            let exe = std::env::current_exe().unwrap_or_default();
            ("Installed", crate::install::bundle_of(&exe).unwrap_or(exe).display().to_string())
        }
        crate::install::Kind::Dev => ("Development build", "Built from source: updates come from your checkout.".to_string()),
    };
    let installed = matches!(kind, crate::install::Kind::Installed(_));
    let can_update = !matches!(kind, crate::install::Kind::Dev);
    let status = match &upd {
        crate::state::UpdateState::Idle => "Updates come from Larrier's releases on GitHub.".to_string(),
        crate::state::UpdateState::Checking => "Checking…".to_string(),
        crate::state::UpdateState::UpToDate => format!("Larrier {} is the newest {}.", crate::version::VERSION, if channel == larrier_core::UpdateChannel::Preview { "release or preview" } else { "release" }),
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
            Row { title: "Version".to_string(), desc: built,
                div { class: "row", style: "gap: 8px;",
                    span { class: "code-inline", "{crate::version::VERSION}" }
                    if crate::version::is_prerelease() { span { class: "status-pill", "Preview" } }
                    if crate::version::notes(crate::version::VERSION).is_some() {
                        button { class: "ghost-btn", onclick: move |_| app.ui.write().modal = Some(crate::state::Modal::WhatsNew(crate::version::VERSION.to_string(), false)), "Release notes" }
                    }
                }
            }
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
                    crate::state::UpdateState::Checking | crate::state::UpdateState::Downloading { .. } => rsx! { super::loaders::Orbit { size: 14 } },
                    _ => rsx! {
                        button { class: "ghost-btn", onclick: move |_| app.check_updates(false), "Check for updates" }
                    },
                }
            }
            if can_update {
                Row { title: "Update channel".to_string(), desc: Some("Stable gets releases. Preview also gets previews of the next release as soon as they're out.".to_string()),
                    super::Select { value: if channel == larrier_core::UpdateChannel::Preview { "preview".to_string() } else { "stable".to_string() },
                        options: vec![("stable".to_string(), "Stable".to_string()), ("preview".to_string(), "Preview".to_string())],
                        onchange: move |v: String| {
                            let c = if v == "preview" { larrier_core::UpdateChannel::Preview } else { larrier_core::UpdateChannel::Stable };
                            app.update_config(|cfg| cfg.settings.update_channel = c);
                            app.check_updates(false);
                        },
                    }
                }
                Row { title: "Updates".to_string(), desc: Some(format!("{} Larrier looks when it starts, then every 10 minutes.", mode.description())),
                    super::Select {
                        value: format!("{mode:?}"),
                        options: larrier_core::UpdateMode::ALL.iter().map(|m| (format!("{m:?}"), m.label().to_string())).collect(),
                        wide: true,
                        onchange: move |v: String| {
                            if let Some(m) = larrier_core::UpdateMode::ALL.into_iter().find(|m| format!("{m:?}") == v) {
                                app.update_config(move |c| {
                                    c.settings.updates = m;
                                    c.settings.auto_update = m != larrier_core::UpdateMode::Off;
                                });
                                // Picked a mode that looks: look now (and
                                // download, if it says to).
                                if m != larrier_core::UpdateMode::Off {
                                    app.check_updates(true);
                                }
                            }
                        },
                    }
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
