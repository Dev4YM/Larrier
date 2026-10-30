//! Command palette, context menu, modals and toasts.

use dioxus::prelude::*;
use larrier_core::{lsf::new_id, Account, AccountAuth, AgentKind};
use larrier_formats::ExportFormat;

use super::{AgentGlyph, Kbd};
use crate::icons::{Icon, I};
use crate::state::{rel_time, store, use_app, App, Modal, RightTab, SettingsTab};

#[derive(Clone)]
enum PaletteAction {
    NewAgentWith(String),
    Open(String),
    Act(&'static str),
}

const ACTIONS: &[(&str, &str, I, &[&str])] = &[
    ("new", "New Agent", I::Plus, &["Ctrl", "N"]),
    ("settings", "Open Settings", I::Settings, &["Ctrl", ","]),
    ("sidebar", "Toggle Sidebar", I::PanelLeft, &["Ctrl", "B"]),
    ("panel", "Toggle Review Panel", I::PanelRight, &["Ctrl", "J"]),
    ("changes", "Show Changes", I::GitBranch, &[]),
    ("browser", "Show Browser", I::Globe, &[]),
    ("terminal", "Show Terminal", I::Terminal, &[]),
    ("files", "Search Files", I::File, &["Ctrl", "Shift", "E"]),
    ("import", "Import Sessions from All Agents", I::Download, &["Ctrl", "Shift", "I"]),
    ("import-file", "Import File (LSF, ChatGPT, Claude.ai)…", I::Upload, &[]),
    ("theme", "Toggle Light / Dark Theme", I::Moon, &[]),
    ("agents", "Manage Agents", I::Bot, &[]),
    ("accounts", "Manage Accounts", I::User, &[]),
    ("models", "Models & API Keys", I::Key, &[]),
    ("workspace", "Open Folder as Workspace…", I::FolderOpen, &[]),
    ("detect", "Re-detect Installed Agents", I::Refresh, &[]),
];

fn run_action(mut app: App, a: &str) {
    match a {
        "new" => app.new_agent(),
        "settings" => app.open_settings(SettingsTab::General),
        "sidebar" => {
            let v = !app.ui.read().sidebar_open;
            app.ui.write().sidebar_open = v;
        }
        "panel" => {
            let v = !app.ui.read().right_open;
            app.ui.write().right_open = v;
        }
        "changes" | "browser" | "terminal" | "files" => {
            app.ui.write().open_panel(
                match a {
                    "changes" => RightTab::Changes,
                    "browser" => RightTab::Browser,
                    "terminal" => RightTab::Terminal,
                    _ => RightTab::Files,
                },
                false,
            );
        }
        "import" => app.sync_imports(false),
        "import-file" => app.import_files(),
        "theme" => {
            let dark = app.ui.read().dark;
            app.update_config(|c| c.settings.theme = if dark { larrier_core::Theme::Light } else { larrier_core::Theme::Dark });
            app.apply_theme();
        }
        "agents" => app.open_settings(SettingsTab::Agents),
        "accounts" => app.open_settings(SettingsTab::Accounts),
        "models" => app.open_settings(SettingsTab::Models),
        "workspace" => app.pick_workspace(true),
        "detect" => {
            app.detect_agents();
            app.toast("Detecting agents…");
        }
        _ => {}
    }
}

#[component]
pub fn Palette() -> Element {
    let mut app = use_app();
    let open = app.ui.read().palette_open;
    let mut hl = use_signal(|| 0usize);
    if !open {
        return rsx! {};
    }
    let q = app.ui.read().palette_query.clone();
    let ql = q.to_lowercase();

    let actions: Vec<_> = ACTIONS.iter().filter(|(_, l, _, _)| ql.is_empty() || l.to_lowercase().contains(&ql)).collect();
    let sessions: Vec<_> = app
        .sessions
        .read()
        .iter()
        .filter(|s| ql.is_empty() || s.title.to_lowercase().contains(&ql) || s.preview.to_lowercase().contains(&ql))
        .take(if ql.is_empty() { 6 } else { 12 })
        .cloned()
        .collect();
    let mut flat: Vec<PaletteAction> = Vec::new();
    for a in &actions {
        flat.push(PaletteAction::Act(a.0));
    }
    for s in &sessions {
        flat.push(PaletteAction::Open(s.id.clone()));
    }
    if !q.trim().is_empty() {
        flat.push(PaletteAction::NewAgentWith(q.clone()));
    }
    let count = flat.len();
    let cur = hl().min(count.saturating_sub(1));
    let flat2 = flat.clone();
    let mut exec = move |a: PaletteAction| {
        app.ui.write().palette_open = false;
        match a {
            PaletteAction::Act(x) => run_action(app, x),
            PaletteAction::Open(id) => app.open_session(&id),
            PaletteAction::NewAgentWith(text) => app.send(None, text, Vec::new()),
        }
    };
    let mut idx = 0usize;
    let mut next = || {
        let i = idx;
        idx += 1;
        i
    };

    rsx! {
        div { class: "overlay", onclick: move |_| app.ui.write().palette_open = false,
            div { class: "palette", onclick: move |e| e.stop_propagation(),
                div { class: "palette-input",
                    span { class: "filter-chip", "Agents & Actions" }
                    input {
                        autofocus: true,
                        placeholder: "Search agents, run actions, or describe a new task…",
                        value: "{q}",
                        oninput: move |e| { app.ui.write().palette_query = e.value(); hl.set(0); },
                        onkeydown: move |e| match e.key() {
                            Key::ArrowDown => { e.prevent_default(); hl.set((cur + 1).min(count.saturating_sub(1))); }
                            Key::ArrowUp => { e.prevent_default(); hl.set(cur.saturating_sub(1)); }
                            Key::Enter => {
                                if let Some(a) = flat2.get(cur).cloned() { exec(a); }
                            }
                            _ => {}
                        },
                    }
                }
                div { class: "palette-list",
                    if !actions.is_empty() {
                        div { class: "menu-label", "Actions" }
                    }
                    for (key, label, icon, keys) in actions.iter().map(|a| **a) {
                        {
                            let i = next();
                            rsx! {
                                button { key: "{key}", class: if i == cur { "menu-item hl" } else { "menu-item" },
                                    onmouseenter: move |_| hl.set(i),
                                    onclick: move |_| exec(PaletteAction::Act(key)),
                                    span { class: "muted", Icon { i: icon, size: 15 } }
                                    span { class: "grow ellipsis", "{label}" }
                                    if !keys.is_empty() { Kbd { keys: keys.to_vec() } }
                                }
                            }
                        }
                    }
                    if !sessions.is_empty() {
                        div { class: "menu-label", "Agents" }
                    }
                    for s in sessions {
                        {
                            let i = next();
                            let id = s.id.clone();
                            rsx! {
                                button { key: "{s.id}", class: if i == cur { "menu-item hl" } else { "menu-item" },
                                    onmouseenter: move |_| hl.set(i),
                                    onclick: move |_| exec(PaletteAction::Open(id.clone())),
                                    AgentGlyph { kind: s.agent.clone() }
                                    span { class: "grow ellipsis", "{s.title}" }
                                    span { class: "sub", "{rel_time(s.updated_at)}" }
                                }
                            }
                        }
                    }
                    if !q.trim().is_empty() {
                        div { class: "menu-label", "Suggestions" }
                        {
                            let i = next();
                            let text = q.clone();
                            rsx! {
                                button { class: if i == cur { "menu-item hl" } else { "menu-item" },
                                    onmouseenter: move |_| hl.set(i),
                                    onclick: move |_| exec(PaletteAction::NewAgentWith(text.clone())),
                                    span { class: "muted", Icon { i: I::Plus, size: 15 } }
                                    span { class: "grow ellipsis", "New agent: “{q}”" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Right-click menu of a chat tab.
#[component]
pub fn TabMenu() -> Element {
    let mut app = use_app();
    let tab_pos = app.ui.read().tab_ctx.as_ref().map(|c| (c.x, c.y));
    use_effect(use_reactive!(|tab_pos| {
        if let Some((x, y)) = tab_pos {
            document::eval(&super::place_at_js("tab-menu", x, y, true));
        }
    }));
    let Some(c) = app.ui.read().tab_ctx.clone() else { return rsx! {} };
    let tabs = app.window_tabs();
    let preview = tabs.iter().find(|t| t.view == c.view).map(|t| t.preview).unwrap_or(false);
    let n = tabs.len();
    let is_last = tabs.last().map(|t| t.view == c.view).unwrap_or(true);
    let session = c.view.session_id().map(|s| s.to_string());
    let is_agent = matches!(c.view, crate::state::View::Agent(_));
    let active = *app.view.read() == c.view;
    let close = move || app.ui.write().tab_ctx = None;
    let item = |label: &'static str, icon: I, kb: &'static str, enabled: bool, f: Box<dyn Fn()>| {
        let mut close = close;
        rsx! {
            button { class: if enabled { "menu-item" } else { "menu-item disabled" },
                onclick: move |_| { if enabled { close(); f(); } },
                span { class: "muted", Icon { i: icon, size: 14 } }
                span { "{label}" }
                if !kb.is_empty() { span { class: "kb", "{kb}" } }
            }
        }
    };
    let (v1, v2, v3, v4, v5) = (c.view.clone(), c.view.clone(), c.view.clone(), c.view.clone(), c.view.clone());
    let (s1, s2, s3) = (session.clone().unwrap_or_default(), session.clone().unwrap_or_default(), session.clone().unwrap_or_default());
    rsx! {
        div { class: "menu-backdrop", style: "z-index: 399;",
            onclick: move |_| app.ui.write().tab_ctx = None,
            oncontextmenu: move |e| { e.prevent_default(); app.ui.write().tab_ctx = None; },
        }
        div { id: "tab-menu", class: "menu ctx-menu", style: "left: {c.x}px; top: {c.y}px; visibility: hidden;",
            if !is_agent {
                {item("Close", I::X, "Ctrl+W", true, Box::new(move || app.close_tab(&v1)))}
            }
            {item(if is_agent { "Close Subagent Tabs" } else { "Close Others" }, I::X, "", n > if is_agent { 1 } else { 2 }, Box::new(move || app.close_tabs(&v2, "others")))}
            if !is_agent {
                {item("Close to the Right", I::X, "", !is_last, Box::new(move || app.close_tabs(&v3, "right")))}
                {item("Close All", I::X, "", true, Box::new(move || app.close_tabs(&v4, "all")))}
            }
            div { class: "menu-sep" }
            if preview {
                {item("Keep Open", I::Pin, "Double-click", true, Box::new(move || app.keep_tab(&v5)))}
            }
            if is_agent && !active {
                {item("Open Side by Side", I::Columns, "", true, Box::new(move || app.open_split(&s1)))}
            }
            if is_agent {
                {item("Rename…", I::Pencil, "", true, Box::new(move || { let mut a = app; a.ui.write().modal = Some(Modal::Rename(s2.clone())); }))}
                {item("Fork", I::Fork, "", true, Box::new(move || app.fork(&s3, None)))}
            }
        }
    }
}

/// Right-click menu for a session row.
#[component]
pub fn ContextMenu() -> Element {
    let mut app = use_app();
    let ctx = app.ui.read().ctx.clone();
    let mut sub = use_signal(|| None::<&'static str>);
    let pos = ctx.as_ref().map(|c| (c.x, c.y));
    // Re-place whenever it opens, moves or a submenu grows it.
    use_effect(use_reactive!(|pos| {
        let _ = sub();
        if let Some((x, y)) = pos {
            document::eval(&super::place_at_js("ctx-menu", x, y, true));
        }
    }));
    let Some(ctx) = ctx else { return rsx! {} };
    let Some(s) = app.sessions.read().iter().find(|s| s.id == ctx.session_id).cloned() else { return rsx! {} };
    let id = s.id.clone();
    let x = ctx.x.min(10_000.0);
    let y = ctx.y;
    let close = move || {
        app.ui.write().ctx = None;
        sub.set(None);
    };
    let item = |label: &'static str, icon: I, f: Box<dyn Fn()>| {
        let mut close = close;
        rsx! {
            button { class: "menu-item", onclick: move |_| { close(); f(); },
                span { class: "muted", Icon { i: icon, size: 14 } }
                span { "{label}" }
            }
        }
    };
    let (i1, i2, i3, i4, i5, i6, i7) = (id.clone(), id.clone(), id.clone(), id.clone(), id.clone(), id.clone(), id.clone());
    rsx! {
        div { class: "menu-backdrop", onclick: move |_| { app.ui.write().ctx = None; sub.set(None); },
            oncontextmenu: move |e| { e.prevent_default(); app.ui.write().ctx = None; } }
        div { id: "ctx-menu", class: "menu", style: "position: fixed; left: {x}px; top: {y}px; visibility: hidden;",
            {item("Open", I::Message, Box::new(move || app.open_session(&i1)))}
            {item("Open Side by Side", I::Columns, { let i = id.clone(); Box::new(move || app.open_split(&i)) })}
            if app.current_id().is_some_and(|c| c != id) {
                {item("Open Side by Side", I::Columns, { let i = id.clone(); Box::new(move || app.open_split(&i)) })}
            }
            {item("Add as Context", I::Link, { let i = id.clone(); Box::new(move || {
                let mut a = app;
                let short = i[i.len().saturating_sub(8)..].to_string();
                let target = a.current_id();
                a.ui.write().compose_inbox.push((target, format!("@agent:{short}")));
            }) })}
            div { class: "menu-sep" }
            {item("Rename", I::Pencil, Box::new(move || { let mut a = app; a.ui.write().modal = Some(Modal::Rename(i2.clone())); }))}
            {item(if s.pinned { "Unpin" } else { "Pin" }, I::Pin, Box::new(move || app.toggle_pin(&i3)))}
            {item(if s.unread { "Mark as Read" } else { "Mark as Unread" }, I::Circle, { let u = !s.unread; Box::new(move || app.set_unread(&i4, u)) })}
            {item("Fork", I::Fork, Box::new(move || app.fork(&i5, None)))}
            button { class: "menu-item", onclick: move |_| sub.set(Some("port")),
                span { class: "muted", Icon { i: I::Swap, size: 14 } }
                span { "Continue with…" }
                span { class: "sub", Icon { i: I::ChevronRight, size: 12 } }
            }
            button { class: "menu-item", onclick: move |_| sub.set(Some("export")),
                span { class: "muted", Icon { i: I::Upload, size: 14 } }
                span { "Export as…" }
                span { class: "sub", Icon { i: I::ChevronRight, size: 12 } }
            }
            if sub() == Some("port") {
                div { class: "menu-sep" }
                for spec in larrier_core::agent::runnable() {
                    {
                        let k = spec.kind.clone();
                        let sid = id.clone();
                        let mut close = close;
                        rsx! {
                            button { key: "{spec.kind.id()}", class: "menu-item", style: "padding-left: 22px;",
                                onclick: move |_| { close(); app.ensure_loaded(&sid); app.port(&sid, k.clone()); },
                                AgentGlyph { kind: spec.kind.clone() }
                                span { "{spec.name}" }
                                if spec.kind == s.agent { span { class: "check", Icon { i: I::Check, size: 13 } } }
                            }
                        }
                    }
                }
            }
            if sub() == Some("export") {
                div { class: "menu-sep" }
                for f in ExportFormat::ALL.iter().copied() {
                    {
                        let sid = id.clone();
                        let mut close = close;
                        rsx! {
                            button { key: "{f.label()}", class: "menu-item", style: "padding-left: 22px;",
                                onclick: move |_| { close(); app.export(&sid, f); },
                                span { "{f.label()}" }
                            }
                        }
                    }
                }
            }
            {item("Open in Terminal", I::Terminal, Box::new(move || { app.ensure_loaded(&i6); app.open_in_terminal(&i6); }))}
            div { class: "menu-sep" }
            {item(if s.archived { "Unarchive" } else { "Archive" }, I::Archive, Box::new(move || app.toggle_archive(&i7)))}
            button { class: "menu-item danger",
                onclick: {
                    let sid = id.clone();
                    let mut close = close;
                    move |_| { close(); app.ui.write().modal = Some(Modal::ConfirmDelete(sid.clone())); }
                },
                Icon { i: I::Trash, size: 14 }
                span { "Delete" }
            }
        }
    }
}

#[component]
pub fn Toasts() -> Element {
    let app = use_app();
    let toasts = app.toasts.read().clone();
    rsx! {
        div { class: "toasts",
            for t in toasts {
                div { key: "{t.id}", class: if t.error { "toast error" } else { "toast" },
                    span { class: "ic", if t.error { Icon { i: I::Alert, size: 14 } } else { Icon { i: I::CircleCheck, size: 14 } } }
                    span { "{t.text}" }
                }
            }
        }
    }
}

#[component]
pub fn ModalHost() -> Element {
    let mut app = use_app();
    let modal = app.ui.read().modal.clone();
    let Some(modal) = modal else { return rsx! {} };
    let close = move |_| app.ui.write().modal = None;
    rsx! {
        div { class: "overlay", onclick: close,
            div { class: "modal", onclick: move |e| e.stop_propagation(),
                match modal {
                    Modal::Rename(id) => rsx! { RenameModal { id } },
                    Modal::ConfirmDelete(id) => rsx! { DeleteModal { id } },
                    Modal::Export(id) => rsx! { DeleteModal { id } },
                    Modal::AddAccount(kind) => rsx! { AddAccountModal { kind } },
                    Modal::AddWorkspace => rsx! { div {} },
                    Modal::WhatsNew(version, updated) => rsx! { WhatsNewModal { version, updated } },
                }
            }
        }
    }
}

#[component]
fn RenameModal(id: String) -> Element {
    let mut app = use_app();
    let current = app.sessions.read().iter().find(|s| s.id == id).map(|s| s.title.clone()).unwrap_or_default();
    let mut value = use_signal(|| current);
    let id2 = id.clone();
    rsx! {
        div { class: "modal-head", "Rename agent" }
        div { class: "modal-body",
            input { class: "input full", autofocus: true, value: "{value}",
                oninput: move |e| value.set(e.value()),
                onkeydown: move |e| if e.key() == Key::Enter { app.rename(&id, value()); app.ui.write().modal = None; },
            }
        }
        div { class: "modal-foot",
            button { class: "ghost-btn", onclick: move |_| app.ui.write().modal = None, "Cancel" }
            button { class: "primary-btn", onclick: move |_| { app.rename(&id2, value()); app.ui.write().modal = None; }, "Rename" }
        }
    }
}

#[component]
fn DeleteModal(id: String) -> Element {
    let mut app = use_app();
    let title = app.sessions.read().iter().find(|s| s.id == id).map(|s| s.title.clone()).unwrap_or_default();
    rsx! {
        div { class: "modal-head", "Delete “{title}”?" }
        div { class: "modal-body",
            "This removes the session from Larrier, and it won't be imported again. The agent's own session files are left untouched. Worktrees Larrier created for this agent are removed."
        }
        div { class: "modal-foot",
            button { class: "ghost-btn", onclick: move |_| app.ui.write().modal = None, "Cancel" }
            button { class: "primary-btn", style: "background: var(--red); color: #fff;",
                onclick: move |_| { app.ui.write().modal = None; app.delete(&id); },
                "Delete"
            }
        }
    }
}

/// A release's notes from the built-in changelog.
#[component]
fn WhatsNewModal(version: String, updated: bool) -> Element {
    let mut app = use_app();
    let notes = crate::version::notes(&version).unwrap_or_else(|| "No notes for this version.".into());
    let html = crate::markdown::to_html(&notes);
    let date = crate::version::released_on(&version).unwrap_or_default();
    rsx! {
        div { class: "modal-head",
            if updated { "Larrier is now {version}" } else { "What's new in Larrier {version}" }
            if !date.is_empty() { span { class: "faint", style: "font-weight: 400; margin-left: 8px; font-size: 12px;", "{date}" } }
        }
        div { class: "modal-body whats-new md", dangerous_inner_html: "{html}" }
        div { class: "modal-foot",
            button { class: "primary-btn", onclick: move |_| app.ui.write().modal = None, "Got it" }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum AuthKind {
    System,
    Dir,
    Key,
}

#[component]
fn AddAccountModal(kind: AgentKind) -> Element {
    let mut app = use_app();
    let spec = kind.spec();
    let mut label = use_signal(|| String::new());
    let mut auth = use_signal(|| if spec.home_env.is_some() { AuthKind::Dir } else if !spec.key_env.is_empty() { AuthKind::Key } else { AuthKind::System });
    let mut key = use_signal(String::new);
    let mut env_var = use_signal(|| spec.key_env.first().map(|s| s.to_string()).unwrap_or_default());
    let k2 = kind.clone();
    let save = move |_| {
        let name = if label().trim().is_empty() { format!("{} account", k2.display_name()) } else { label() };
        let id = new_id();
        let a = match auth() {
            AuthKind::System => AccountAuth::SystemLogin,
            AuthKind::Dir => {
                let dir = store().accounts_dir().join(&id);
                let _ = std::fs::create_dir_all(&dir);
                AccountAuth::ConfigDir { path: dir.to_string_lossy().to_string() }
            }
            AuthKind::Key => {
                let r = format!("account:{id}");
                if let Err(e) = larrier_core::secrets::set(&r, key().trim()) {
                    app.toast_err(format!("Could not store key: {e}"));
                    return;
                }
                AccountAuth::ApiKey { env_var: env_var(), secret_ref: r }
            }
        };
        let mut acc = Account::new(k2.clone(), name, a.clone());
        acc.id = id;
        let first = app.config.read().accounts_for(&k2).is_empty();
        acc.is_default = first;
        app.update_config(|c| c.accounts.push(acc.clone()));
        app.ui.write().modal = None;
        if let AccountAuth::ConfigDir { path } = &a {
            // Open the agent's login flow inside the isolated config dir.
            if let (Some(var), Some(login)) = (k2.spec().home_env, k2.spec().login_command) {
                let cmd = if cfg!(windows) { format!("set {var}={path}&& {login}") } else { format!("{var}='{path}' {login}") };
                let _ = crate::state::open_terminal(&path, &cmd);
                app.toast("Finish signing in in the terminal window");
            }
        } else {
            app.toast("Account added");
        }
    };
    rsx! {
        div { class: "modal-head", AgentGlyph { kind: kind.clone(), class: "md" } span { style: "margin-left: 8px", "Add {spec.name} account" } }
        div { class: "modal-body",
            div { class: "kv",
                span { class: "k", "Name" }
                input { class: "input full", placeholder: "Work, Personal, Team…", value: "{label}", oninput: move |e| label.set(e.value()) }
                span { class: "k", "Sign-in" }
                div { class: "seg",
                    if spec.home_env.is_some() {
                        button { class: if auth() == AuthKind::Dir { "on" } else { "" }, onclick: move |_| auth.set(AuthKind::Dir), "Separate login" }
                    }
                    if !spec.key_env.is_empty() {
                        button { class: if auth() == AuthKind::Key { "on" } else { "" }, onclick: move |_| auth.set(AuthKind::Key), "API key" }
                    }
                    button { class: if auth() == AuthKind::System { "on" } else { "" }, onclick: move |_| auth.set(AuthKind::System), "Current login" }
                }
                if auth() == AuthKind::Key {
                    span { class: "k", "Variable" }
                    super::Select {
                        value: env_var(),
                        options: spec.key_env.iter().map(|v| (v.to_string(), v.to_string())).collect::<Vec<_>>(),
                        onchange: move |v: String| env_var.set(v),
                    }
                    span { class: "k", "API key" }
                    input { class: "input full", r#type: "password", placeholder: "sk-…", value: "{key}", oninput: move |e| key.set(e.value()) }
                }
            }
            match auth() {
                AuthKind::Dir => rsx! { div { "Larrier creates an isolated config folder for this account and opens {spec.name}'s login in a terminal. Each account keeps its own credentials and history." } },
                AuthKind::Key => rsx! { div { "The key is stored in your OS keychain and passed to {spec.name} as an environment variable only when this account runs." } },
                AuthKind::System => rsx! { div { "Uses whatever {spec.name} is currently signed into on this machine." } },
            }
        }
        div { class: "modal-foot",
            button { class: "ghost-btn", onclick: move |_| app.ui.write().modal = None, "Cancel" }
            button { class: "primary-btn", disabled: auth() == AuthKind::Key && key().trim().is_empty(), onclick: save, "Add account" }
        }
    }
}

/// Larrier's own right-click menu for everything that isn't a session row.
#[component]
pub fn GlobalMenu() -> Element {
    let mut app = use_app();
    let g_pos = app.ui.read().gctx.as_ref().map(|c| (c.x, c.y));
    use_effect(use_reactive!(|g_pos| {
        if let Some((x, y)) = g_pos {
            document::eval(&super::place_at_js("g-menu", x, y, false));
        }
    }));
    let ctx = app.ui.read().gctx.clone();
    let Some(c) = ctx else { return rsx! {} };
    let in_agent = app.current_id().is_some();
    let has_sel = !c.selection.trim().is_empty();

    // Rough menu height so the menu stays inside the window.
    let mut rows = 0usize;
    let mut seps = 0usize;
    if c.editable {
        rows += 6;
        seps += 2;
    } else {
        if has_sel {
            rows += 4;
            seps += 1;
        }
        if c.link.is_some() {
            rows += 2;
            seps += 1;
        }
        if c.code.is_some() {
            rows += 1;
            seps += 1;
        }
        if c.path.is_some() {
            rows += 3;
            seps += 1;
        }
        if c.image.is_some() {
            rows += 2;
            seps += 1;
        }
        rows += 7;
        seps += 2;
    }
    let _ = (rows, seps);
    let (x, y) = (c.x, c.y);

    let close = move || app.ui.write().gctx = None;
    let item = move |label: &'static str, icon: I, kb: &'static str, enabled: bool, f: Box<dyn Fn()>| {
        let mut close = close;
        rsx! {
            button {
                class: if enabled { "menu-item" } else { "menu-item disabled" },
                onclick: move |_| {
                    if enabled {
                        close();
                        f();
                    }
                },
                span { class: "muted", Icon { i: icon, size: 14 } }
                span { "{label}" }
                if !kb.is_empty() { span { class: "kb", "{kb}" } }
            }
        }
    };
    let sel = c.selection.clone();
    let (s1, s2, s3, s4) = (sel.clone(), sel.clone(), sel.clone(), sel.clone());
    let link = c.link.clone().unwrap_or_default();
    let (l1, l2) = (link.clone(), link.clone());
    let code = c.code.clone().unwrap_or_default();
    let path = c.path.clone().unwrap_or_default();
    let (p1, p2, p3) = (path.clone(), path.clone(), path.clone());
    let image = c.image.clone().unwrap_or_default();
    let (i1, i2) = (image.clone(), image.clone());
    let dark = app.ui.read().dark;

    rsx! {
        div { class: "menu-backdrop", style: "z-index: 399;",
            onclick: move |_| app.ui.write().gctx = None,
            oncontextmenu: move |e| { e.prevent_default(); app.ui.write().gctx = None; },
        }
        div { id: "g-menu", class: "menu ctx-menu", style: "left: {x}px; top: {y}px; visibility: hidden;",
            if c.editable {
                {item("Undo", I::Undo, "Ctrl+Z", true, Box::new(|| super::menubar::edit_command("undo")))}
                {item("Redo", I::Refresh, "Ctrl+Y", true, Box::new(|| super::menubar::edit_command("redo")))}
                div { class: "menu-sep" }
                {item("Cut", I::Scissors, "Ctrl+X", has_sel, Box::new(move || { app.copy_text(&s1); super::menubar::edit_command("delete"); }))}
                {item("Copy", I::Copy, "Ctrl+C", has_sel, Box::new(move || app.copy_text(&s2)))}
                {item("Paste", I::ClipboardPaste, "Ctrl+V", true, Box::new(super::menubar::paste_into_focused))}
                div { class: "menu-sep" }
                {item("Select All", I::TextSelect, "Ctrl+A", true, Box::new(|| super::menubar::edit_command("selectAll")))}
            } else {
                if has_sel {
                    {item("Copy", I::Copy, "Ctrl+C", true, Box::new(move || app.copy_text(&s3)))}
                    if in_agent {
                        {item("Quote in Follow-up", I::Quote, "", true, Box::new(move || {
                            let mut app = app;
                            let mut ui = app.ui.write();
                            ui.quote = Some(s4.trim().to_string());
                            ui.focus_composer += 1;
                        }))}
                    }
                    {item("Ask a New Agent", I::SquarePen, "", true, Box::new({
                        let q = sel.clone();
                        move || {
                            let mut app = app;
                            app.ui.write().quote = Some(q.trim().to_string());
                            app.new_agent();
                        }
                    }))}
                    {item("Search the Web", I::Globe, "", true, Box::new({
                        let q = sel.clone();
                        move || {
                            let enc: String = q.trim().chars().take(300).map(|ch| if ch.is_ascii_alphanumeric() { ch.to_string() } else { format!("%{:02X}", ch as u32 & 0xff) }).collect();
                            let _ = open::that(format!("https://www.google.com/search?q={enc}"));
                        }
                    }))}
                    div { class: "menu-sep" }
                }
                if c.link.is_some() {
                    {item("Open Link", I::External, "", true, Box::new(move || { let _ = open::that(&l1); }))}
                    {item("Copy Link Address", I::Copy, "", true, Box::new(move || app.copy_text(&l2)))}
                    div { class: "menu-sep" }
                }
                if c.code.is_some() {
                    {item("Copy Code", I::Copy, "", true, Box::new(move || { app.copy_text(&code); }))}
                    div { class: "menu-sep" }
                }
                if c.path.is_some() {
                    {item("Open File", I::File, "", true, Box::new(move || {
                        let cwd = app.current_session().and_then(|s| s.cwd()).unwrap_or_default();
                        let full = if std::path::Path::new(&p1).is_absolute() { p1.clone() } else { std::path::Path::new(&cwd).join(&p1).to_string_lossy().to_string() };
                        let _ = open::that(full);
                    }))}
                    {item("Reveal in File Explorer", I::FolderOpen, "", true, Box::new(move || {
                        let cwd = app.current_session().and_then(|s| s.cwd()).unwrap_or_default();
                        let full = if std::path::Path::new(&p2).is_absolute() { std::path::PathBuf::from(&p2) } else { std::path::Path::new(&cwd).join(&p2) };
                        let _ = open::that(full.parent().map(|p| p.to_path_buf()).unwrap_or(full));
                    }))}
                    {item("Copy Path", I::Copy, "", true, Box::new(move || app.copy_text(&p3)))}
                    div { class: "menu-sep" }
                }
                if c.image.is_some() {
                    {item("Open Image", I::Image, "", true, Box::new(move || { let _ = open::that(&i1); }))}
                    {item("Copy Image Address", I::Copy, "", true, Box::new(move || app.copy_text(&i2)))}
                    div { class: "menu-sep" }
                }
                {item("Back", I::ArrowLeft, "Alt+←", app.can_go(-1), Box::new(move || app.go(-1)))}
                {item("Forward", I::ArrowRight, "Alt+→", app.can_go(1), Box::new(move || app.go(1)))}
                div { class: "menu-sep" }
                {item("New Agent", I::SquarePen, "Ctrl+N", true, Box::new(move || app.new_agent()))}
                {item("Command Palette", I::Command, "Ctrl+K", true, Box::new(move || {
                    let mut app = app;
                    let mut ui = app.ui.write();
                    ui.palette_open = true;
                    ui.palette_query.clear();
                }))}
                {item("Toggle Sidebar", I::PanelLeft, "Ctrl+B", true, Box::new(move || {
                    let mut app = app;
                    let v = !app.ui.read().sidebar_open;
                    app.ui.write().sidebar_open = v;
                }))}
                {item(if dark { "Light Theme" } else { "Dark Theme" }, if dark { I::Sun } else { I::Moon }, "", true, Box::new(move || {
                    app.update_config(|cfg| cfg.settings.theme = if dark { larrier_core::Theme::Light } else { larrier_core::Theme::Dark });
                    app.apply_theme();
                }))}
                div { class: "menu-sep" }
                {item("Settings", I::Settings, "Ctrl+,", true, Box::new(move || app.open_settings(SettingsTab::General)))}
            }
        }
    }
}
