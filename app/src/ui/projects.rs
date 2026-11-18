//! Projects: a name and a look (an icon, an emoji or an image, on a color),
//! the folders it covers (its chats are the ones started in them), the
//! agents it works with and where its new chats run; and the dialog that
//! creates or edits one. Home, the project of chats without a folder, is
//! edited the same way, without folders.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use base64::Engine;
use dioxus::html::input_data::MouseButton;
use dioxus::prelude::*;
use lary_core::config::HOME_PROJECT;
use lary_core::{AgentKind, Workspace};

use super::{AgentGlyph, Dropdown, MenuItem};
use crate::icons::{Icon, Lucide, I};
use crate::state::{store, use_app};

/// Icons that suit a project, shown first.
const COMMON: &[&str] = &[
    "folder", "code", "terminal", "globe", "layout-dashboard", "server", "database", "cpu", "smartphone", "monitor", "box", "package", "rocket", "sparkles", "bot", "brain",
    "flask-conical", "atom", "palette", "pen-tool", "image", "film", "music", "camera", "book-open", "notebook", "graduation-cap", "briefcase", "building-2", "store", "shopping-cart",
    "credit-card", "wallet", "chart-line", "chart-pie", "calendar", "mail", "message-circle", "users", "user", "heart", "star", "shield", "lock", "key", "zap", "flame", "leaf",
    "tree-pine", "sun", "moon", "cloud", "map", "compass", "plane", "car", "house", "gamepad-2", "puzzle", "trophy", "target", "flag", "bug", "wrench", "hammer", "cog", "layers",
    "git-branch", "github", "gitlab", "figma", "chrome", "coffee", "pizza", "dog", "cat",
];

/// Every Lucide icon bundled with Lary: the common ones first, then the
/// rest by name.
static ALL_ICONS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    let all: Vec<&'static str> = crate::icons::SPRITE.split("id=\"lucide-").skip(1).filter_map(|s| s.split('"').next()).collect();
    let mut out: Vec<&'static str> = COMMON.iter().copied().filter(|c| all.contains(c)).collect();
    out.extend(all.into_iter().filter(|n| !COMMON.contains(n)));
    out
});

/// Background colors to pick from (then any color, with the picker).
pub const COLORS: &[&str] = &["#ffffff", "#3e6fd8", "#a56cc1", "#d17aa0", "#a9684f", "#b84a46", "#e57a2e", "#e8a73a", "#a8a845", "#6daa4e", "#4f9e6e"];

/// Icons shown before more are asked for (the whole set is searched).
const ICON_PAGE: usize = 240;

/// What a project shows: a Lucide icon, an emoji or an image file
/// (`Workspace::icon` is the name, `emoji:…` or `image:…`).
#[derive(Clone, PartialEq)]
pub enum Look {
    Icon(String),
    Emoji(String),
    Image(String),
}

pub fn look_of(icon: Option<&str>, fallback: &str) -> Look {
    match icon {
        Some(s) if s.starts_with("emoji:") => Look::Emoji(s["emoji:".len()..].to_string()),
        Some(s) if s.starts_with("image:") => Look::Image(s["image:".len()..].to_string()),
        Some(s) if !s.is_empty() => Look::Icon(s.to_string()),
        _ => Look::Icon(fallback.to_string()),
    }
}

/// Ink that reads on `hex`: dark on light colors, white on the rest.
fn ink_on(hex: &str) -> &'static str {
    match rgb(hex) {
        Some((r, g, b)) if 0.299 * r as f64 + 0.587 * g as f64 + 0.114 * b as f64 > 160.0 => "#1d1d1f",
        _ => "#ffffff",
    }
}

fn rgb(hex: &str) -> Option<(u8, u8, u8)> {
    let h = hex.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(h, 16).ok()?;
    Some(((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

/// An image file as a data URL (read once, then kept).
pub fn image_url(path: &str) -> String {
    static CACHE: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
    if let Some(u) = CACHE.lock().unwrap().get(path) {
        return u.clone();
    }
    let mime = match path.rsplit('.').next().map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("svg") => "image/svg+xml",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        _ => "image/png",
    };
    let url = std::fs::read(path).map(|b| format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(b))).unwrap_or_default();
    CACHE.lock().unwrap().insert(path.to_string(), url.clone());
    url
}

/// A project's mark. A plain icon until it has a color, an emoji or an
/// image; then a rounded tile in its color (`tile` always draws one).
#[component]
pub fn ProjectMark(
    #[props(default)] icon: Option<String>,
    #[props(default)] color: Option<String>,
    #[props(default = 16)] size: u32,
    #[props(default = false)] open: bool,
    #[props(default = false)] home: bool,
    #[props(default = false)] tile: bool,
    /// The icon with none picked (a chat's: a speech bubble).
    #[props(default)]
    fallback: Option<&'static str>,
) -> Element {
    let fallback = fallback.unwrap_or(if home { "house" } else if open { "folder-open" } else { "folder" });
    let look = look_of(icon.as_deref(), fallback);
    if !tile && color.is_none() && matches!(look, Look::Icon(_)) {
        let Look::Icon(name) = look else { return rsx! {} };
        return rsx! { span { class: "pj-mark", Lucide { name, size, stroke: 1.5 } } };
    }
    let bg = color.clone().unwrap_or_else(|| "var(--chip)".into());
    let ink = color.as_deref().map(ink_on).unwrap_or("var(--fg-2)");
    let radius = (size as f32 * 0.26).round();
    let style = format!("width: {size}px; height: {size}px; border-radius: {radius}px; background: {bg}; color: {ink};");
    rsx! {
        span { class: "pj-tile", style: "{style}",
            match look {
                Look::Icon(name) => rsx! { Lucide { name, size: (size as f32 * 0.6).round() as u32, stroke: 1.6 } },
                Look::Emoji(e) => rsx! { span { class: "pj-emoji", style: "font-size: {(size as f32 * 0.66).round()}px;", "{e}" } },
                Look::Image(p) => rsx! { img { src: "{image_url(&p)}", draggable: false } },
            }
        }
    }
}

/// Create a project (`id` None, its folder `path` when known), edit one, or
/// edit Home (`id` [`HOME_PROJECT`]).
#[component]
pub fn ProjectModal(id: Option<String>, path: Option<String>) -> Element {
    let mut app = use_app();
    let home = id.as_deref() == Some(HOME_PROJECT);
    let existing = if home { Some(app.config.read().home_project()) } else { id.as_ref().and_then(|id| app.config.read().workspaces.iter().find(|w| &w.id == id).cloned()) };
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
    let mut picker = use_signal(|| false);
    let mut folders_menu = use_signal(|| false);
    let mut agents_menu = use_signal(|| false);
    let mut chats_menu = use_signal(|| false);
    let cur = w.read().clone();
    let draft_agent = app.draft.read().agent.clone();
    let agents: Vec<(AgentKind, String)> = super::composer::pickable_agents(&app, &draft_agent).into_iter().map(|(k, name, _)| (k, name)).collect();
    let name_of = |p: &str| std::path::Path::new(p).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| p.to_string());

    // A folder from the system's picker: the first is the main one (and
    // names the project when it has no name yet).
    let add_folder = move || {
        spawn(async move {
            let Some(dir) = rfd::AsyncFileDialog::new().pick_folder().await else { return };
            let p = dir.path().to_string_lossy().to_string();
            let mut v = w.write();
            let same = |a: &str| lary_core::config::normalize_path(a) == lary_core::config::normalize_path(&p);
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
        if !home && v.path.is_empty() {
            return error.set("Choose the project's folder".into());
        }
        if v.name.is_empty() {
            return error.set("Give the project a name".into());
        }
        app.update_config(move |c| {
            if home {
                v.id = HOME_PROJECT.into();
                v.path.clear();
                v.folders.clear();
                c.home_project = Some(v);
                return;
            }
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

    let chats_label = match cur.worktree {
        None => "As in Settings",
        Some(false) => "In the folder",
        Some(true) => "New worktree",
    };
    let agents_label = match cur.agents.len() {
        0 => "Any agent".to_string(),
        1 => cur.agents[0].display_name(),
        n => format!("{} +{}", cur.agents[0].display_name(), n - 1),
    };
    rsx! {
        div { class: "pjd-head",
            span { class: "t", if home { "Home" } else if is_new { "New project" } else { "Edit project" } }
            button { class: "icon-btn", title: "Close", onclick: move |_| app.ui.write().modal = None, Icon { i: I::X, size: 15 } }
        }
        div { class: "pjd-hero",
            div { class: "menu-anchor",
                button { class: if picker() { "pjd-avatar open" } else { "pjd-avatar" }, title: "Icon, emoji or image, and its color", onclick: move |_| picker.set(true),
                    ProjectMark { icon: cur.icon.clone(), color: cur.color.clone(), size: 72, tile: true, home }
                }
                Dropdown { open: picker, up: false, class: "pjd-picker center".to_string(),
                    LookPicker { w }
                }
            }
            input { class: "pjd-name", autofocus: true, placeholder: if home { "Home" } else { "Project name" }, value: "{cur.name}", spellcheck: "false",
                oninput: move |e| w.write().name = e.value() }
        }
        div { class: "pjd-rows",
            if !home {
                div { class: "pjd-row",
                    span { class: "lbl", "Folders" }
                    if cur.path.is_empty() {
                        button { class: "pjd-pick", onclick: move |_| add_folder(), Icon { i: I::FolderPlus, size: 14, stroke: 1.5 } "Choose a folder…" }
                    } else {
                        div { class: "menu-anchor",
                            button { class: if folders_menu() { "pjd-pick open" } else { "pjd-pick" }, title: "{cur.path}", onclick: move |_| folders_menu.set(true),
                                Icon { i: I::Folder, size: 14, stroke: 1.5 }
                                span { class: "ellipsis", "{name_of(&cur.path)}" }
                                if !cur.folders.is_empty() { span { class: "more", "+{cur.folders.len()}" } }
                                Icon { i: I::ChevronDown, size: 12 }
                            }
                            Dropdown { open: folders_menu, up: false, right: true, class: "pjd-folders".to_string(),
                                div { class: "menu-label", "Chats start in the main folder; Claude Code and Codex can work in the others too" }
                                div { class: "pjd-folder", title: "{cur.path}",
                                    Icon { i: I::Folder, size: 14, stroke: 1.5 }
                                    span { class: "ellipsis", "{lary_core::lsf::short_path(&cur.path)}" }
                                    span { class: "pj-tag", "Main" }
                                    if !cur.folders.is_empty() {
                                        button { class: "icon-btn sm", title: "Remove (the next one becomes the main folder)",
                                            onclick: move |_| { let mut v = w.write(); let next = v.folders.remove(0); v.path = next; },
                                            Icon { i: I::X, size: 12 }
                                        }
                                    }
                                }
                                for (i, f) in cur.folders.iter().cloned().enumerate() {
                                    div { key: "{f}", class: "pjd-folder", title: "{f}",
                                        Icon { i: I::Folder, size: 14, stroke: 1.5 }
                                        span { class: "ellipsis", "{lary_core::lsf::short_path(&f)}" }
                                        button { class: "pjd-make-main", title: "Make it the main folder",
                                            onclick: move |_| { let mut v = w.write(); let f = v.folders.remove(i); let old = std::mem::replace(&mut v.path, f); v.folders.insert(0, old); },
                                            "Main"
                                        }
                                        button { class: "icon-btn sm", title: "Remove", onclick: move |_| { w.write().folders.remove(i); }, Icon { i: I::X, size: 12 } }
                                    }
                                }
                                div { class: "menu-sep" }
                                MenuItem { label: "Add a folder…".to_string(), icon: Some(I::FolderPlus), onclick: move |_| add_folder() }
                            }
                        }
                    }
                }
            }
            div { class: "pjd-row",
                span { class: "lbl", "Agents" }
                div { class: "menu-anchor",
                    button { class: if agents_menu() { "pjd-pick open" } else { "pjd-pick" }, onclick: move |_| agents_menu.set(true),
                        if cur.agents.is_empty() {
                            Icon { i: I::Bot, size: 14, stroke: 1.5 }
                        } else {
                            span { class: "pjd-glyphs", for k in cur.agents.iter().take(3).cloned() { AgentGlyph { key: "{k.id()}", kind: k } } }
                        }
                        span { class: "ellipsis", "{agents_label}" }
                        Icon { i: I::ChevronDown, size: 12 }
                    }
                    Dropdown { open: agents_menu, up: false, right: true, class: "pjd-agents".to_string(),
                        div { class: "menu-label", "Its new chats start with the first one" }
                        for (k, name) in agents {
                            {
                                let at = cur.agents.iter().position(|a| *a == k);
                                let k2 = k.clone();
                                rsx! {
                                    button { key: "{k.id()}", class: if at.is_some() { "menu-item checked" } else { "menu-item" },
                                        onclick: move |e| {
                                            e.stop_propagation();
                                            let mut v = w.write();
                                            match v.agents.iter().position(|a| *a == k2) {
                                                Some(i) => { v.agents.remove(i); }
                                                None => v.agents.push(k2.clone()),
                                            }
                                        },
                                        AgentGlyph { kind: k.clone() }
                                        span { class: "ellipsis", "{name}" }
                                        if at == Some(0) { span { class: "sub", "Default" } }
                                        if at.is_some() { span { class: "check", Icon { i: I::Check, size: 13 } } }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            div { class: "pjd-row",
                span { class: "lbl", "New chats" }
                div { class: "menu-anchor",
                    button { class: if chats_menu() { "pjd-pick open" } else { "pjd-pick" }, onclick: move |_| chats_menu.set(true),
                        Icon { i: if cur.worktree == Some(true) { I::Layers } else { I::Laptop }, size: 14, stroke: 1.5 }
                        span { "{chats_label}" }
                        Icon { i: I::ChevronDown, size: 12 }
                    }
                    Dropdown { open: chats_menu, up: false, right: true,
                        MenuItem { label: "As in Settings".to_string(), checked: cur.worktree.is_none(), onclick: move |_| { chats_menu.set(false); w.write().worktree = None; } }
                        MenuItem { label: "In the folder".to_string(), icon: Some(I::Laptop), checked: cur.worktree == Some(false), onclick: move |_| { chats_menu.set(false); w.write().worktree = Some(false); } }
                        MenuItem { label: "New worktree".to_string(), icon: Some(I::Layers), checked: cur.worktree == Some(true), onclick: move |_| { chats_menu.set(false); w.write().worktree = Some(true); } }
                    }
                }
            }
        }
        div { class: "modal-foot",
            if !is_new && !home {
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
            button { class: "primary-btn", onclick: save, if is_new { "Create project" } else { "Save" } }
        }
    }
}

/// Edit a chat: its icon and colour, its name, and whether it's pinned or
/// archived. Laid out as a project's dialog.
#[component]
pub fn ChatModal(id: String) -> Element {
    let mut app = use_app();
    let s = app.sessions.read().iter().find(|s| s.id == id).cloned();
    let Some(s) = s else {
        return rsx! { div { class: "modal-body", "That chat is gone." } };
    };
    // Its look, carried the way the picker takes a project's.
    let mut w = use_signal(|| {
        let mut w = Workspace::new("");
        w.id = s.id.clone();
        w.name = s.title.clone();
        w.icon = s.icon.clone();
        w.color = s.color.clone();
        w
    });
    let mut pinned = use_signal(|| s.pinned);
    let mut archived = use_signal(|| s.archived);
    let mut picker = use_signal(|| false);
    let cur = w.read().clone();
    let id2 = id.clone();
    let save = move |_| {
        let v = w.read().clone();
        app.edit_chat(&id2, v.name, v.icon, v.color, pinned(), archived());
        app.ui.write().modal = None;
    };
    rsx! {
        div { class: "pjd-head",
            span { class: "t", "Edit chat" }
            button { class: "icon-btn", title: "Close", onclick: move |_| app.ui.write().modal = None, Icon { i: I::X, size: 15 } }
        }
        div { class: "pjd-hero",
            div { class: "menu-anchor",
                button { class: if picker() { "pjd-avatar open" } else { "pjd-avatar" }, title: "Icon, emoji or image, and its color", onclick: move |_| picker.set(true),
                    ProjectMark { icon: cur.icon.clone(), color: cur.color.clone(), size: 72, tile: true, fallback: Some("message-square") }
                }
                Dropdown { open: picker, up: false, class: "pjd-picker center".to_string(),
                    LookPicker { w, fallback: Some("message-square") }
                }
            }
            input { class: "pjd-name", autofocus: true, placeholder: "Chat name", value: "{cur.name}", spellcheck: "false",
                oninput: move |e| w.write().name = e.value() }
        }
        div { class: "pjd-rows",
            div { class: "pjd-row",
                span { class: "lbl", "Pinned" }
                super::Toggle { on: pinned(), onchange: move |v: bool| pinned.set(v) }
            }
            div { class: "pjd-row",
                span { class: "lbl", "Archived" }
                super::Toggle { on: archived(), onchange: move |v: bool| archived.set(v) }
            }
        }
        div { class: "modal-foot",
            button { class: "ghost-btn danger pj-remove", onclick: { let i = id.clone(); move |_| app.ui.write().modal = Some(crate::state::Modal::ConfirmDelete(i.clone())) }, "Delete chat" }
            button { class: "primary-btn", disabled: cur.name.trim().is_empty(), onclick: save, "Save" }
        }
    }
}

/// The look picker: Icons (every bundled one, searchable), Emojis
/// (searchable) or a Custom image; then the color behind it. A chat's look
/// goes through it too (in a `Workspace` carrying its icon and colour).
#[component]
pub fn LookPicker(w: Signal<Workspace>, #[props(default)] fallback: Option<&'static str>) -> Element {
    let mut w = w;
    let cur = w.read().clone();
    // The icon it shows with none picked: Home's house, a project's folder.
    let fallback: &'static str = fallback.unwrap_or(if cur.id == lary_core::config::HOME_PROJECT { "house" } else { "folder" });
    let start = match look_of(cur.icon.as_deref(), fallback) {
        Look::Emoji(_) => 1,
        Look::Image(_) => 2,
        Look::Icon(_) => 0,
    };
    let mut tab = use_signal(|| start);
    let mut query = use_signal(String::new);
    let mut shown = use_signal(|| ICON_PAGE);
    let mut custom = use_signal(|| false);
    let q = query.read().trim().to_lowercase();
    let current = cur.icon.clone().unwrap_or_default();
    let mut set_tab = move |t: u8| {
        tab.set(t);
        query.set(String::new());
        shown.set(ICON_PAGE);
    };
    let upload = move |_| {
        spawn(async move {
            let Some(f) = rfd::AsyncFileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "webp", "gif", "svg"]).pick_file().await else { return };
            let ext = f.path().extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_else(|| "png".into());
            let dir = store().root().join("project-images");
            if std::fs::create_dir_all(&dir).is_err() {
                return;
            }
            let id = w.read().id.clone();
            let dest = dir.join(format!("{id}-{}.{ext}", chrono::Utc::now().timestamp_millis()));
            if std::fs::write(&dest, f.read().await).is_ok() {
                w.write().icon = Some(format!("image:{}", dest.to_string_lossy()));
            }
        });
    };
    rsx! {
        div { class: "pjd-tabs",
            button { class: if tab() == 0 { "on" } else { "" }, onclick: move |_| set_tab(0), "Icons" }
            button { class: if tab() == 1 { "on" } else { "" }, onclick: move |_| set_tab(1), "Emojis" }
            button { class: if tab() == 2 { "on" } else { "" }, onclick: move |_| set_tab(2), "Custom" }
        }
        if tab() != 2 {
            div { class: "menu-search pjd-search",
                Icon { i: I::Search, size: 13 }
                input {
                    placeholder: if tab() == 0 { format!("Search {} icons", ALL_ICONS.len()) } else { "Search emoji".to_string() },
                    value: "{query}",
                    spellcheck: "false",
                    oninput: move |e| { query.set(e.value()); shown.set(ICON_PAGE); },
                }
            }
        }
        div { class: "pjd-grid-wrap",
            match tab() {
                0 => {
                    let words: Vec<String> = q.split_whitespace().map(String::from).collect();
                    let all: Vec<&'static str> = ALL_ICONS.iter().copied().filter(|n| words.iter().all(|w| n.contains(w.as_str()))).collect();
                    let total = all.len();
                    rsx! {
                        div { class: "pjd-grid",
                            for name in all.into_iter().take(shown()) {
                                button { key: "{name}", class: if current == name || (current.is_empty() && name == fallback) { "pjd-cell on" } else { "pjd-cell" }, title: "{name}",
                                    onclick: move |_| w.write().icon = (name != fallback).then(|| name.to_string()),
                                    Lucide { name: name.to_string(), size: 18, stroke: 1.5 }
                                }
                            }
                        }
                        if total == 0 { div { class: "menu-empty", "No icons match" } }
                        if total > shown() {
                            button { class: "pjd-more", onclick: move |_| shown.set(shown() + ICON_PAGE * 2), "Show more ({total - shown()})" }
                        }
                    }
                }
                1 => {
                    let list: Vec<(&'static str, &'static str, u8)> = super::emoji::EMOJI.iter().copied().filter(|(_, n, _)| q.is_empty() || n.contains(q.as_str())).collect();
                    rsx! {
                        if q.is_empty() {
                            for (ci, cat) in super::emoji::CATEGORIES.iter().enumerate() {
                                div { key: "{cat}", class: "menu-label", "{cat}" }
                                div { class: "pjd-grid emoji",
                                    for (e, n, _) in list.iter().copied().filter(|(_, _, c)| *c as usize == ci) {
                                        EmojiCell { key: "{e}", e, name: n, on: current == format!("emoji:{e}"), w }
                                    }
                                }
                            }
                        } else {
                            div { class: "pjd-grid emoji",
                                for (e, n, _) in list.iter().copied() {
                                    EmojiCell { key: "{e}", e, name: n, on: current == format!("emoji:{e}"), w }
                                }
                            }
                            if list.is_empty() { div { class: "menu-empty", "No emoji match" } }
                        }
                    }
                }
                _ => rsx! {
                    div { class: "pjd-custom",
                        if let Look::Image(_) = look_of(cur.icon.as_deref(), fallback) {
                            ProjectMark { icon: cur.icon.clone(), color: cur.color.clone(), size: 64, tile: true }
                            div { class: "row",
                                button { class: "ghost-btn", onclick: upload, Icon { i: I::Upload, size: 13 } "Replace…" }
                                button { class: "ghost-btn", onclick: move |_| w.write().icon = None, "Remove" }
                            }
                        } else {
                            button { class: "pjd-upload", onclick: upload,
                                Icon { i: I::Upload, size: 18, stroke: 1.5 }
                                span { "Upload an image" }
                            }
                        }
                        span { class: "pj-hint", "PNG, JPG, WebP, GIF or SVG. A transparent image shows the color below behind it." }
                    }
                },
            }
        }
        div { class: "pjd-colors",
            button { class: if cur.color.is_none() { "pj-swatch none on" } else { "pj-swatch none" }, title: "No color", onclick: move |_| w.write().color = None }
            for c in COLORS.iter().copied() {
                button { key: "{c}", class: if cur.color.as_deref() == Some(c) { "pj-swatch on" } else { "pj-swatch" }, style: "--sw: {c};", title: "{c}",
                    onclick: move |_| w.write().color = Some(c.to_string()) }
            }
            {
                let other = cur.color.as_deref().is_some_and(|c| !COLORS.contains(&c));
                rsx! {
                    button { class: if custom() || other { "pj-swatch any on" } else { "pj-swatch any" }, style: if other { format!("background: {};", cur.color.clone().unwrap_or_default()) } else { String::new() }, title: "Any color",
                        onclick: move |_| custom.set(!custom()),
                        Icon { i: I::Plus, size: 12 }
                    }
                }
            }
        }
        if custom() {
            ColorPicker { value: cur.color.clone(), onchange: move |hex: String| w.write().color = Some(hex) }
        }
    }
}

#[component]
fn EmojiCell(e: &'static str, name: &'static str, on: bool, w: Signal<Workspace>) -> Element {
    let mut w = w;
    rsx! {
        button { class: if on { "pjd-cell emoji on" } else { "pjd-cell emoji" }, title: "{name}",
            onclick: move |_| w.write().icon = Some(format!("emoji:{e}")),
            "{e}"
        }
    }
}

/// Any color: saturation and brightness on a square, hue on a strip, and
/// its hex.
#[component]
fn ColorPicker(value: Option<String>, onchange: EventHandler<String>) -> Element {
    const W: f64 = 248.0;
    const H: f64 = 140.0;
    let mut hsv = use_signal(|| value.as_deref().and_then(hex_to_hsv).unwrap_or((20.0, 0.53, 0.66)));
    let mut drag = use_signal(|| 0u8);
    let mut hex = use_signal(|| value.clone().unwrap_or_else(|| "#a9684f".into()));
    let (h, s, v) = hsv();
    let mut set = move |nh: f64, ns: f64, nv: f64| {
        hsv.set((nh, ns, nv));
        let x = hsv_to_hex(nh, ns, nv);
        hex.set(x.clone());
        onchange.call(x);
    };
    let held = |e: &MouseEvent| e.held_buttons().contains(MouseButton::Primary);
    rsx! {
        div { class: "pjd-cp",
            div { class: "pjd-cp-sv", style: "width: {W}px; height: {H}px; background: linear-gradient(to top, #000, transparent), linear-gradient(to right, #fff, transparent), hsl({h}, 100%, 50%);",
                onmousedown: move |e| { drag.set(1); let p = e.element_coordinates(); set(h, (p.x / W).clamp(0.0, 1.0), 1.0 - (p.y / H).clamp(0.0, 1.0)); },
                onmousemove: move |e| if drag() == 1 && held(&e) { let p = e.element_coordinates(); set(h, (p.x / W).clamp(0.0, 1.0), 1.0 - (p.y / H).clamp(0.0, 1.0)); },
                onmouseup: move |_| drag.set(0),
                onmouseleave: move |_| drag.set(0),
                span { class: "pjd-cp-dot", style: "left: {s * W}px; top: {(1.0 - v) * H}px; background: {hex};" }
            }
            div { class: "pjd-cp-hue", style: "width: {W}px;",
                onmousedown: move |e| { drag.set(2); let p = e.element_coordinates(); set((p.x / W).clamp(0.0, 1.0) * 360.0, s, v); },
                onmousemove: move |e| if drag() == 2 && held(&e) { let p = e.element_coordinates(); set((p.x / W).clamp(0.0, 1.0) * 360.0, s, v); },
                onmouseup: move |_| drag.set(0),
                onmouseleave: move |_| drag.set(0),
                span { class: "pjd-cp-knob", style: "left: {h / 360.0 * W}px; background: hsl({h}, 100%, 50%);" }
            }
            div { class: "pjd-cp-hex",
                span { "Hex" }
                input { class: "input", value: "{hex}", spellcheck: "false",
                    oninput: move |e| {
                        let t = e.value();
                        hex.set(t.clone());
                        let t = if t.starts_with('#') { t } else { format!("#{t}") };
                        if let Some(x) = hex_to_hsv(&t) {
                            hsv.set(x);
                            onchange.call(t.to_lowercase());
                        }
                    } }
            }
        }
    }
}

fn hsv_to_hex(h: f64, s: f64, v: f64) -> String {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let to = |n: f64| ((n + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    format!("#{:02x}{:02x}{:02x}", to(r), to(g), to(b))
}

fn hex_to_hsv(hex: &str) -> Option<(f64, f64, f64)> {
    let (r, g, b) = rgb(hex)?;
    let (r, g, b) = (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    Some((if h < 0.0 { h + 360.0 } else { h }, if max == 0.0 { 0.0 } else { d / max }, max))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_go_round_through_hsv() {
        for hex in ["#a9684f", "#3e6fd8", "#ffffff", "#000000", "#6daa4e"] {
            let (h, s, v) = hex_to_hsv(hex).unwrap();
            assert_eq!(hsv_to_hex(h, s, v), hex);
        }
        assert_eq!(ink_on("#ffffff"), "#1d1d1f");
        assert_eq!(ink_on("#2b2b2b"), "#ffffff");
    }

    #[test]
    fn a_look_is_an_icon_an_emoji_or_an_image() {
        assert!(look_of(None, "folder") == Look::Icon("folder".into()));
        assert!(look_of(Some("rocket"), "folder") == Look::Icon("rocket".into()));
        assert!(look_of(Some("emoji:🚀"), "folder") == Look::Emoji("🚀".into()));
        assert!(look_of(Some("image:C:/a.png"), "folder") == Look::Image("C:/a.png".into()));
        assert!(ALL_ICONS.len() > 1000 && ALL_ICONS.contains(&"rocket"));
    }
}
