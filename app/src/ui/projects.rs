//! Projects: a name, an icon and a color, the folders it covers (its chats
//! are the ones started in them), the agents it works with and where its
//! new chats run; and the dialog that creates or edits one.

use dioxus::prelude::*;
use larrier_core::{AgentKind, Workspace};

use super::AgentGlyph;
use crate::icons::{Icon, Lucide, I};
use crate::state::use_app;

/// The icons a project can take (Lucide names).
pub const ICONS: &[&str] = &[
    "folder", "code", "terminal", "globe", "layout-dashboard", "server", "database", "cpu", "smartphone", "box", "rocket", "sparkles", "bot", "brain", "flask-conical", "palette",
    "pen-tool", "book-open", "graduation-cap", "briefcase", "building-2", "shopping-cart", "gamepad-2", "music", "camera", "heart", "shield", "zap", "leaf",
];

/// Project colors, one per hue, readable on light and dark.
pub const COLORS: &[&str] = &["#64748b", "#ef4444", "#f97316", "#d97706", "#16a34a", "#0d9488", "#2563eb", "#7c3aed", "#db2777"];

/// A project's mark: its icon in its color (a folder when it has neither).
#[component]
pub fn ProjectMark(#[props(default)] icon: Option<String>, #[props(default)] color: Option<String>, #[props(default = 16)] size: u32, #[props(default = false)] open: bool) -> Element {
    let name = icon.unwrap_or_else(|| if open { "folder-open".into() } else { "folder".into() });
    let style = color.map(|c| format!("color: {c};")).unwrap_or_default();
    rsx! {
        span { class: "pj-mark", style: "{style}", Lucide { name, size, stroke: 1.5 } }
    }
}

/// Create a project (`id` None, its folder `path` when known) or edit one.
#[component]
pub fn ProjectModal(id: Option<String>, path: Option<String>) -> Element {
    let mut app = use_app();
    let existing = id.as_ref().and_then(|id| app.config.read().workspaces.iter().find(|w| &w.id == id).cloned());
    let is_new = existing.is_none();
    let mut w = use_signal(|| {
        existing.clone().unwrap_or_else(|| match &path {
            Some(p) => Workspace::new(p),
            None => {
                let mut w = Workspace::new("");
                w.name = String::new();
                w
            }
        })
    });
    let mut error = use_signal(String::new);
    let mut icon_menu = use_signal(|| false);
    let cur = w.read().clone();
    let draft_agent = app.draft.read().agent.clone();
    let agents: Vec<(AgentKind, String)> = super::composer::pickable_agents(&app, &draft_agent).into_iter().map(|(k, name, _)| (k, name)).collect();

    // A folder from the system's picker: the first is the main one (and
    // names the project when it has no name yet).
    let add_folder = move |_| {
        spawn(async move {
            let Some(dir) = rfd::AsyncFileDialog::new().pick_folder().await else { return };
            let p = dir.path().to_string_lossy().to_string();
            let mut v = w.write();
            let same = |a: &str| larrier_core::config::normalize_path(a) == larrier_core::config::normalize_path(&p);
            if same(&v.path) || v.folders.iter().any(|f| same(f)) {
                return;
            }
            if v.path.is_empty() {
                if v.name.trim().is_empty() {
                    v.name = std::path::Path::new(&p).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
                }
                v.path = p;
            } else {
                v.folders.push(p);
            }
        });
    };

    let save = move |_| {
        let mut v = w.read().clone();
        v.name = v.name.trim().to_string();
        if v.path.is_empty() {
            return error.set("Add the project's folder".into());
        }
        if v.name.is_empty() {
            return error.set("Give the project a name".into());
        }
        app.update_config(move |c| {
            // A folder that already was a project (chats ran there) becomes
            // this one, keeping its place.
            let at = c.workspaces.iter().position(|x| x.id == v.id).or_else(|| c.workspace_by_path(&v.path).map(|x| x.id.clone()).and_then(|id| c.workspaces.iter().position(|x| x.id == id)));
            match at {
                Some(i) => {
                    v.id = c.workspaces[i].id.clone();
                    v.added_at = c.workspaces[i].added_at;
                    c.workspaces[i] = v;
                }
                None => c.workspaces.push(v),
            }
        });
        app.toast(if is_new { "Project created" } else { "Project saved" });
        app.ui.write().modal = None;
    };

    rsx! {
        div { class: "modal-head", if is_new { "New project" } else { "Edit project" } }
        div { class: "kv pj-form",
            span { class: "k", "Name" }
            input { class: "input full", autofocus: true, placeholder: "My app", value: "{cur.name}", oninput: move |e| w.write().name = e.value() }

            span { class: "k", "Look" }
            div { class: "pj-look",
                div { class: "menu-anchor",
                    button { class: "pj-icon-btn", title: "Icon", onclick: move |_| icon_menu.set(true),
                        ProjectMark { icon: cur.icon.clone(), color: cur.color.clone(), size: 18 }
                    }
                    super::Dropdown { open: icon_menu, up: false, class: "pj-icons".to_string(),
                        div { class: "pj-icon-grid",
                            for name in ICONS.iter().copied() {
                                button {
                                    key: "{name}",
                                    class: if cur.icon.as_deref().unwrap_or("folder") == name { "pj-icon on" } else { "pj-icon" },
                                    title: "{name}",
                                    onclick: move |_| { icon_menu.set(false); w.write().icon = (name != "folder").then(|| name.to_string()); },
                                    Lucide { name: name.to_string(), size: 16, stroke: 1.5 }
                                }
                            }
                        }
                    }
                }
                div { class: "pj-swatches",
                    button { class: if cur.color.is_none() { "pj-swatch none on" } else { "pj-swatch none" }, title: "No color", onclick: move |_| w.write().color = None }
                    for c in COLORS.iter().copied() {
                        button { key: "{c}", class: if cur.color.as_deref() == Some(c) { "pj-swatch on" } else { "pj-swatch" }, style: "--sw: {c};", title: "{c}",
                            onclick: move |_| w.write().color = Some(c.to_string()) }
                    }
                }
            }

            span { class: "k top", "Folders" }
            div { class: "pj-folders",
                if !cur.path.is_empty() {
                    div { class: "pj-folder", title: "{cur.path}",
                        Icon { i: I::Folder, size: 14, stroke: 1.5 }
                        span { class: "ellipsis", "{larrier_core::lsf::short_path(&cur.path)}" }
                        span { class: "pj-tag", "Main" }
                        if !cur.folders.is_empty() {
                            button { class: "icon-btn sm", title: "Remove (the next folder becomes the main one)",
                                onclick: move |_| { let mut v = w.write(); let next = v.folders.remove(0); v.path = next; },
                                Icon { i: I::X, size: 12 }
                            }
                        }
                    }
                }
                for (i, f) in cur.folders.iter().cloned().enumerate() {
                    div { key: "{f}", class: "pj-folder", title: "{f}",
                        Icon { i: I::Folder, size: 14, stroke: 1.5 }
                        span { class: "ellipsis", "{larrier_core::lsf::short_path(&f)}" }
                        button { class: "icon-btn sm", title: "Remove", onclick: move |_| { w.write().folders.remove(i); }, Icon { i: I::X, size: 12 } }
                    }
                }
                button { class: "ghost-btn pj-add", onclick: add_folder,
                    Icon { i: I::FolderPlus, size: 14, stroke: 1.5 }
                    if cur.path.is_empty() { "Choose its folder…" } else { "Add a folder…" }
                }
                span { class: "pj-hint", "Chats start in the main folder; Claude Code and Codex can work in the others too." }
            }

            span { class: "k top", "Agents" }
            div { class: "pj-agents-wrap",
                div { class: "pj-agents",
                    for (k, name) in agents {
                        {
                            let at = cur.agents.iter().position(|a| *a == k);
                            let k2 = k.clone();
                            rsx! {
                                button { key: "{k.id()}", class: if at.is_some() { "pj-agent on" } else { "pj-agent" },
                                    onclick: move |_| {
                                        let mut v = w.write();
                                        match v.agents.iter().position(|a| *a == k2) {
                                            Some(i) => { v.agents.remove(i); }
                                            None => v.agents.push(k2.clone()),
                                        }
                                    },
                                    AgentGlyph { kind: k.clone() }
                                    span { "{name}" }
                                    if at == Some(0) { span { class: "pj-default", "Default" } }
                                }
                            }
                        }
                    }
                }
                span { class: "pj-hint", if cur.agents.is_empty() { "Any agent. Pick some to start its chats with the first." } else { "Its new chats start with the first one picked." } }
            }

            span { class: "k", "New chats" }
            div { class: "seg pj-seg",
                button { class: if cur.worktree.is_none() { "on" } else { "" }, onclick: move |_| w.write().worktree = None, "As in Settings" }
                button { class: if cur.worktree == Some(false) { "on" } else { "" }, onclick: move |_| w.write().worktree = Some(false), "In the folder" }
                button { class: if cur.worktree == Some(true) { "on" } else { "" }, onclick: move |_| w.write().worktree = Some(true), "New worktree" }
            }
        }
        div { class: "modal-foot",
            if !is_new {
                button { class: "ghost-btn danger pj-remove", title: "Its chats stay, listed by their folder",
                    onclick: move |_| {
                        let id = w.read().id.clone();
                        app.update_config(move |c| c.workspaces.retain(|x| x.id != id));
                        app.toast("Project removed; its chats stay");
                        app.ui.write().modal = None;
                    },
                    "Remove project"
                }
            }
            if !error.read().is_empty() { span { class: "pj-err", "{error}" } }
            button { class: "ghost-btn", onclick: move |_| app.ui.write().modal = None, "Cancel" }
            button { class: "primary-btn", onclick: save, if is_new { "Create" } else { "Save" } }
        }
    }
}
