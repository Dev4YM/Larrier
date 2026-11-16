//! Command palette, context menu, modals and toasts.

use dioxus::prelude::*;
use larrier_core::{lsf::new_id, Account, AccountAuth, AgentKind, ApiAccount, ApiKind};
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
    ("harnesses", "Harnesses, Accounts & API Keys", I::Bot, &[]),
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
        "agents" | "accounts" | "models" | "harnesses" => app.open_models(None),
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
                if !kb.is_empty() { span { class: "kb", {crate::platform::keys(kb)} } }
            }
        }
    };
    let (v1, v2, v3, v4, v5) = (c.view.clone(), c.view.clone(), c.view.clone(), c.view.clone(), c.view.clone());
    let (s1, s2, s3) = (session.clone().unwrap_or_default(), session.clone().unwrap_or_default(), session.clone().unwrap_or_default());
    rsx! {
        div { class: "menu-backdrop ctx-block", style: "z-index: 1499;",
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
    let pos = ctx.as_ref().map(|c| (c.x, c.y));
    // Placed whenever it opens or moves.
    use_effect(use_reactive!(|pos| {
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
        div { class: "menu-backdrop ctx-block", onclick: move |_| app.ui.write().ctx = None,
            oncontextmenu: move |e| { e.prevent_default(); app.ui.write().ctx = None; } }
        div { id: "ctx-menu", class: "menu", style: "position: fixed; left: {x}px; top: {y}px; visibility: hidden;",
            {item("Open", I::Message, Box::new(move || app.open_session(&i1)))}
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
            super::SubMenu { label: "Continue with", icon: I::Swap,
                for spec in larrier_core::agent::runnable() {
                    {
                        let k = spec.kind.clone();
                        let sid = id.clone();
                        let mut close = close;
                        rsx! {
                            button { key: "{spec.kind.id()}", class: if spec.kind == s.agent { "menu-item checked" } else { "menu-item" },
                                onclick: move |_| { close(); app.ensure_loaded(&sid); app.port(&sid, k.clone()); },
                                AgentGlyph { kind: spec.kind.clone() }
                                span { class: "ellipsis", "{spec.name}" }
                                if spec.kind == s.agent { span { class: "check", Icon { i: I::Check, size: 13 } } }
                            }
                        }
                    }
                }
            }
            super::SubMenu { label: "Export as", icon: I::Upload,
                for f in ExportFormat::ALL.iter().copied() {
                    {
                        let sid = id.clone();
                        let mut close = close;
                        rsx! {
                            button { key: "{f.label()}", class: "menu-item",
                                onclick: move |_| { close(); app.export(&sid, f); },
                                span { class: "ellipsis", "{f.label()}" }
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
    let wide = matches!(modal, Modal::Project(..));
    let close = move |_| app.ui.write().modal = None;
    rsx! {
        div { class: "overlay", onclick: close,
            div { class: if wide { "modal pj-modal" } else { "modal" }, onclick: move |e| e.stop_propagation(),
                match modal {
                    Modal::Rename(id) => rsx! { RenameModal { id } },
                    Modal::ConfirmDelete(id) => rsx! { DeleteModal { id } },
                    Modal::Export(id) => rsx! { DeleteModal { id } },
                    Modal::AddAccount(kind) => rsx! { AddAccountModal { kind } },
                    Modal::EditAccount(id) => rsx! { EditAccountModal { id } },
                    Modal::AddCustomAgent => rsx! {
                        div { class: "modal-head", "Add a custom agent" }
                        div { class: "modal-body",
                            "Any program that speaks the Agent Client Protocol or one of the stream protocols."
                            super::settings::AddCustomAgent {}
                        }
                    },
                    Modal::AddWorkspace => rsx! { div {} },
                    Modal::Project(id, path) => rsx! { super::projects::ProjectModal { id, path } },
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
            if !date.is_empty() { span { class: "faint", style: "font-weight: 400; margin-left: 8px; font-size: var(--ui-sm);", "{date}" } }
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
    /// A folder the agent already signs in from (another account's).
    Existing,
    /// A key the agent reads from one of its variables.
    Key,
    /// An API Larrier points the harness at (Larrier Chat speaks it itself).
    Api,
}

/// What marks an agent's own folder: its sign-in file, and where its
/// sessions are.
fn folder_marks(kind: &AgentKind) -> Option<(&'static str, &'static str, &'static str)> {
    match kind {
        AgentKind::ClaudeCode => Some((".claude-", ".credentials.json", "projects")),
        AgentKind::Codex => Some((".codex-", "auth.json", "sessions")),
        _ => None,
    }
}

/// Whether `path` holds the agent's sign-in (`None`: not a folder).
fn signed_in(kind: &AgentKind, path: &str) -> Option<bool> {
    let p = std::path::Path::new(path.trim());
    if path.trim().is_empty() || !p.is_dir() {
        return None;
    }
    Some(folder_marks(kind).map(|(_, sign, _)| p.join(sign).is_file()).unwrap_or(false))
}

/// The agent's other folders in the home folder (`~/.claude-work`) that no
/// account uses yet: (path, a name for it).
fn found_folders(kind: &AgentKind, used: &[String]) -> Vec<(String, String)> {
    let Some((prefix, sign, data)) = folder_marks(kind) else { return Vec::new() };
    let Some(home) = dirs::home_dir() else { return Vec::new() };
    let Ok(rd) = std::fs::read_dir(&home) else { return Vec::new() };
    let mut out: Vec<(String, String)> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let rest = name.strip_prefix(prefix)?.to_string();
            let p = e.path();
            if rest.is_empty() || !p.is_dir() || !(p.join(sign).is_file() || p.join(data).is_dir()) {
                return None;
            }
            let path = p.to_string_lossy().to_string();
            (!used.iter().any(|u| same_folder(u, &path))).then_some((path, rest))
        })
        .collect();
    out.sort();
    out
}

fn same_folder(a: &str, b: &str) -> bool {
    let canon = |s: &str| std::fs::canonicalize(s).unwrap_or_else(|_| std::path::PathBuf::from(s));
    a.eq_ignore_ascii_case(b) || canon(a) == canon(b)
}

#[component]
fn AddAccountModal(kind: AgentKind) -> Element {
    let mut app = use_app();
    let spec = kind.spec();
    let mut label = use_signal(|| String::new());
    let chat = kind == AgentKind::Chat;
    let kinds = larrier_agents::request::api_kinds(&kind);
    let can_api = !kinds.is_empty();
    let mut auth = use_signal(|| {
        if chat {
            AuthKind::Api
        } else if spec.home_env.is_some() {
            AuthKind::Dir
        } else if can_api {
            AuthKind::Api
        } else if !spec.key_env.is_empty() {
            AuthKind::Key
        } else {
            AuthKind::System
        }
    });
    let first_api = kinds.first().copied().unwrap_or(ApiKind::OpenAiCompatible);
    let mut api_kind = use_signal(|| first_api);
    let mut url = use_signal(|| first_api.default_url().to_string());
    let mut models = use_signal(|| first_api.suggested_models().join(", "));
    let mut key = use_signal(String::new);
    let mut env_var = use_signal(|| spec.key_env.first().map(|s| s.to_string()).unwrap_or_default());
    // An existing folder: typed, picked, or one of those found at home.
    let mut folder = use_signal(String::new);
    let mut folder_name = use_signal(String::new);
    let found = use_hook({
        let kind = kind.clone();
        move || {
            let used: Vec<String> = app.config.peek().accounts.iter().filter_map(|a| if let AccountAuth::ConfigDir { path } = &a.auth { Some(path.clone()) } else { None }).collect();
            found_folders(&kind, &used)
        }
    });
    let has_folders = folder_marks(&kind).is_some();
    let k2 = kind.clone();
    let save = move |_| {
        let fallback = match auth() {
            AuthKind::Existing if !folder_name().is_empty() => folder_name(),
            AuthKind::Api => api_kind().label().to_string(),
            AuthKind::Key => format!("{} API key", k2.display_name()),
            _ => format!("{} account", k2.display_name()),
        };
        let name = if label().trim().is_empty() { fallback } else { label() };
        let id = new_id();
        let mut needs_login = matches!(auth(), AuthKind::Dir);
        let a = match auth() {
            AuthKind::System => AccountAuth::SystemLogin,
            AuthKind::Dir => {
                let dir = store().accounts_dir().join(&id);
                let _ = std::fs::create_dir_all(&dir);
                AccountAuth::ConfigDir { path: dir.to_string_lossy().to_string() }
            }
            AuthKind::Existing => {
                let path = folder().trim().to_string();
                match signed_in(&k2, &path) {
                    None => {
                        app.toast_err("That folder doesn't exist");
                        return;
                    }
                    Some(ok) => needs_login = !ok,
                }
                AccountAuth::ConfigDir { path }
            }
            AuthKind::Key => {
                let r = format!("account:{id}");
                if let Err(e) = larrier_core::secrets::set(&r, key().trim()) {
                    app.toast_err(format!("Could not store key: {e}"));
                    return;
                }
                let var = env_var();
                AccountAuth::Api(ApiAccount { api: ApiKind::for_variable(&var), base_url: None, secret_ref: Some(r), env_var: Some(var), models: Vec::new() })
            }
            AuthKind::Api => {
                let api = api_kind();
                let mut secret_ref = None;
                if api.needs_key() {
                    let r = format!("account:{id}");
                    if let Err(e) = larrier_core::secrets::set(&r, key().trim()) {
                        app.toast_err(format!("Could not store key: {e}"));
                        return;
                    }
                    secret_ref = Some(r);
                }
                let u = url().trim().trim_end_matches('/').to_string();
                let base_url = (!u.is_empty() && u != api.default_url()).then_some(u);
                let list = if k2 == AgentKind::Chat {
                    models().split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
                } else {
                    Vec::new()
                };
                AccountAuth::Api(ApiAccount { api, base_url, secret_ref, env_var: None, models: list })
            }
        };
        let mut acc = Account::new(k2.clone(), name, a.clone());
        acc.id = id;
        let first = app.config.read().accounts_for(&k2).is_empty();
        // An agent's first added account keeps it on what it ran with:
        // that stays, as an account of its own, the default; the new one
        // can be made default. (Larrier Chat had nothing to run on.)
        let keep_current = first && k2 != AgentKind::Chat && auth() != AuthKind::System;
        acc.is_default = first && !keep_current;
        app.update_config(|c| {
            if keep_current {
                let mut cur = Account::new(k2.clone(), "Current login".to_string(), AccountAuth::SystemLogin);
                cur.is_default = true;
                c.accounts.push(cur);
            }
            c.accounts.push(acc.clone());
        });
        app.ui.write().modal = None;
        match &a {
            // Sign in inside the account's folder.
            AccountAuth::ConfigDir { .. } if needs_login => app.sign_in(&acc),
            AccountAuth::ConfigDir { .. } => app.toast("Account added · bringing in its sessions"),
            _ if keep_current => app.toast("Account added · make it the default to run new chats on it"),
            _ => app.toast("Account added"),
        }
    };
    rsx! {
        div { class: "modal-head", AgentGlyph { kind: kind.clone(), class: "md" } span { style: "margin-left: 8px", "Add {spec.name} account" } }
        div { class: "modal-body",
            div { class: "kv",
                span { class: "k", "Name" }
                input { class: "input full", placeholder: "Work, Personal, Team…", value: "{label}", oninput: move |e| label.set(e.value()) }
                if !chat {
                    span { class: "k", "Runs as" }
                    div { class: "seg",
                        if spec.home_env.is_some() {
                            button { class: if auth() == AuthKind::Dir { "on" } else { "" }, onclick: move |_| auth.set(AuthKind::Dir), "Separate sign-in" }
                        }
                        if spec.home_env.is_some() && has_folders {
                            button { class: if auth() == AuthKind::Existing { "on" } else { "" }, onclick: move |_| auth.set(AuthKind::Existing), "Existing folder" }
                        }
                        if can_api {
                            button { class: if auth() == AuthKind::Api { "on" } else { "" }, onclick: move |_| auth.set(AuthKind::Api), "API" }
                        } else if !spec.key_env.is_empty() {
                            button { class: if auth() == AuthKind::Key { "on" } else { "" }, onclick: move |_| auth.set(AuthKind::Key), "API key" }
                        }
                        button { class: if auth() == AuthKind::System { "on" } else { "" }, onclick: move |_| auth.set(AuthKind::System), "Current sign-in" }
                    }
                }
                if auth() == AuthKind::Api {
                    if kinds.len() > 1 {
                        span { class: "k", "API" }
                        super::Select {
                            value: api_kind().id().to_string(),
                            options: kinds.iter().map(|k| (k.id().to_string(), k.label().to_string())).collect::<Vec<_>>(),
                            onchange: move |v: String| {
                                if let Some(k) = ApiKind::from_id(&v) {
                                    // Follow the API's own address and models unless typed over.
                                    let old = api_kind();
                                    if url.read().trim().is_empty() || url.read().as_str() == old.default_url() {
                                        url.set(k.default_url().to_string());
                                    }
                                    if models.read().trim().is_empty() || *models.read() == old.suggested_models().join(", ") {
                                        models.set(k.suggested_models().join(", "));
                                    }
                                    api_kind.set(k);
                                }
                            },
                        }
                    }
                    span { class: "k", "Address" }
                    input { class: "input full", placeholder: if api_kind().default_url().is_empty() { "https://…/v1" } else { api_kind().default_url() }, value: "{url}", spellcheck: "false", oninput: move |e| url.set(e.value()) }
                    if api_kind().needs_key() {
                        span { class: "k", "API key" }
                        input { class: "input full", r#type: "password", placeholder: "Paste the key", value: "{key}", oninput: move |e| key.set(e.value()) }
                    }
                    if chat {
                        span { class: "k", "Models" }
                        input { class: "input full", placeholder: "Model ids, comma separated", value: "{models}", spellcheck: "false", oninput: move |e| models.set(e.value()) }
                    }
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
                if auth() == AuthKind::Existing {
                    span { class: "k", "Folder" }
                    div { class: "folder-pick",
                        input { class: "input full", placeholder: "C:\\Users\\you\\.claude-work", value: "{folder}", spellcheck: "false",
                            oninput: move |e| { folder.set(e.value()); folder_name.set(String::new()); },
                        }
                        button { class: "ghost-btn", title: "Choose the folder",
                            onclick: move |_| {
                                spawn(async move {
                                    if let Some(d) = rfd::AsyncFileDialog::new().pick_folder().await {
                                        folder.set(d.path().to_string_lossy().to_string());
                                        folder_name.set(String::new());
                                    }
                                });
                            },
                            "Browse…"
                        }
                    }
                    if !found.is_empty() {
                        span { class: "k", "Found" }
                        div { class: "found-folders",
                            for (path, name) in found.clone() {
                                {
                                    let short = path.rsplit(['\\', '/']).next().unwrap_or(&path).to_string();
                                    let on = same_folder(&folder(), &path);
                                    rsx! {
                                        button { key: "{path}", class: if on { "pill-btn on" } else { "pill-btn" }, title: "{path}",
                                            onclick: move |_| { folder.set(path.clone()); folder_name.set(name.clone()); },
                                            Icon { i: I::Folder, size: 13 }
                                            "~/{short}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            match auth() {
                AuthKind::Dir => rsx! { div { "Larrier creates an isolated config folder for this account and opens {spec.name}'s login in a terminal. Each account keeps its own credentials and history." } },
                AuthKind::Existing => rsx! {
                    div {
                        "Uses a {spec.name} folder you already have, with its sign-in, settings and sessions. {spec.name} runs with it for this account, and its sessions show up in Larrier; Larrier itself changes nothing in it."
                        match signed_in(&kind, &folder()) {
                            Some(true) => rsx! { div { class: "folder-state ok", "Signed in · its sessions come in once it's added" } },
                            Some(false) => rsx! { div { class: "folder-state", "No sign-in in this folder yet: {spec.name}'s login opens in a terminal after adding" } },
                            None if !folder().trim().is_empty() => rsx! { div { class: "folder-state bad", "That folder doesn't exist" } },
                            None => rsx! {},
                        }
                    }
                },
                AuthKind::Key => rsx! { div { "The key is stored in your OS keychain and passed to {spec.name} as an environment variable only when this account runs." } },
                AuthKind::Api if chat => rsx! { div { "Larrier Chat sends your messages to this API with this key. The key stays in your OS keychain." } },
                AuthKind::Api => rsx! { div { "When a chat runs on this account, Larrier points {spec.name} at this API for that run and hands it the key from your OS keychain. {spec.name}'s own settings aren't changed." } },
                AuthKind::System => rsx! { div { "Uses whatever {spec.name} is currently signed into on this computer." } },
            }
        }
        div { class: "modal-foot",
            button { class: "ghost-btn", onclick: move |_| app.ui.write().modal = None, "Cancel" }
            button { class: "primary-btn",
                disabled: (auth() == AuthKind::Key && key().trim().is_empty())
                    || (auth() == AuthKind::Existing && signed_in(&kind, &folder()).is_none())
                    || (auth() == AuthKind::Api && ((api_kind().needs_key() && key().trim().is_empty()) || (url().trim().is_empty() && api_kind().default_url().is_empty()) || (chat && models().trim().is_empty()))),
                onclick: save, "Add account" }
        }
    }
}

/// An API account's name, API, address, key and models, edited in place.
#[component]
fn EditAccountModal(id: String) -> Element {
    let mut app = use_app();
    let acc = app.config.read().account(&id).cloned();
    let Some(a) = acc else {
        return rsx! { div { class: "modal-body", "That account is gone." } };
    };
    let AccountAuth::Api(api) = a.auth.clone() else {
        return rsx! { div { class: "modal-body", "Only API accounts have settings to edit." } };
    };
    rsx! {
        div { class: "modal-head", AgentGlyph { kind: a.agent.clone(), class: "md" } span { "Edit {a.label}" } }
        div { class: "modal-body",
            super::settings::ApiEditor { a: a.clone(), api }
        }
        div { class: "modal-foot",
            button { class: "primary-btn", onclick: move |_| app.ui.write().modal = None, "Done" }
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
                if !kb.is_empty() { span { class: "kb", {crate::platform::keys(kb)} } }
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
    let (i1, i2, i3, i4) = (image.clone(), image.clone(), image.clone(), image.clone());
    let dark = app.ui.read().dark;

    rsx! {
        div { class: "menu-backdrop ctx-block", style: "z-index: 1499;",
            onclick: move |_| app.ui.write().gctx = None,
            oncontextmenu: move |e| { e.prevent_default(); app.ui.write().gctx = None; },
        }
        div { id: "g-menu", class: "menu ctx-menu", style: "left: {x}px; top: {y}px; visibility: hidden;",
            if let Some(sp) = c.spell.clone() {
                // Corrections for the misspelled word, as Chrome offers them.
                if sp.suggestions.is_empty() {
                    button { class: "menu-item disabled", span { class: "muted", Icon { i: I::Spell, size: 14 } } span { "No spelling suggestions" } }
                }
                for (i, w) in sp.suggestions.iter().cloned().enumerate() {
                    button { key: "sp{i}", class: "menu-item spell-fix",
                        onclick: move |_| {
                            app.ui.write().gctx = None;
                            document::eval(&format!(
                                "const el = window.__larrierCtx; if (el) {{ el.focus(); el.setSelectionRange({}, {}); document.execCommand('insertText', false, {}); }}",
                                sp.start, sp.end, serde_json::to_string(&w).unwrap_or_default()
                            ));
                        },
                        span { class: "muted", if i == 0 { Icon { i: I::Spell, size: 14 } } }
                        span { class: "fix", "{w}" }
                    }
                }
                {item("Add to Dictionary", I::BookPlus, "", true, Box::new({
                    let w = sp.word.clone();
                    move || {
                        if crate::spell::add(&w) {
                            // The boxes draw their text again, without the underline.
                            *crate::spell::WORDS.write() += 1;
                        }
                    }
                }))}
                div { class: "menu-sep" }
            }
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
                        {item("Quote", I::Quote, "", true, Box::new(move || {
                            let mut app = app;
                            let id = app.current_id();
                            app.ui.write().chip_inbox.push((id, super::mentions::MentionChip::quote(&s4), None));
                        }))}
                    }
                    {item("Ask a New Agent", I::SquarePen, "", true, Box::new({
                        let q = sel.clone();
                        move || {
                            let mut app = app;
                            app.ui.write().chip_inbox.push((None, super::mentions::MentionChip::quote(&q), None));
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
                    {item("Copy Image", I::Copy, "", true, Box::new(move || app.copy_image(i1.clone())))}
                    {item("Save Image As…", I::Download, "", true, Box::new(move || app.save_image(i3.clone())))}
                    {item("Open Image", I::Image, "", true, Box::new(move || app.open_image(i4.clone())))}
                    // An inline image's address is the whole picture, not a link.
                    if !image.starts_with("data:") {
                        {item("Copy Image Address", I::Copy, "", true, Box::new(move || app.copy_text(&i2)))}
                    }
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
