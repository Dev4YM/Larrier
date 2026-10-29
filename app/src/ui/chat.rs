//! The agent view: title chip header, transcript + composer column, the
//! "On <repo>" tool list, and the side panel (Changes / Browser / Terminal /
//! Files) — laid out like Cursor's Agents Window.

use std::path::Path;

use dioxus::prelude::*;
use larrier_agents::git;
use larrier_core::Environment;
use larrier_formats::ExportFormat;

use super::composer::Composer;
use super::right::RightPane;
use super::transcript::Transcript;
use super::{open_tool, Dropdown, DragRegion, MenuItem, SidebarToggle};
use crate::icons::{Icon, I};
use crate::state::{use_app, Modal, RightTab};

#[component]
pub fn AgentView(id: String) -> Element {
    let mut app = use_app();
    let right_open = app.ui.read().right_open;
    let exists = app.open.read().contains_key(&id);
    let mut dragging = use_signal(|| false);
    if !exists {
        return rsx! { div { class: "empty-state", "Loading…" } };
    }
    let ws_name = app.open.read().get(&id).and_then(|s| s.workspace.as_ref().map(|w| w.name.clone()));
    rsx! {
        div { class: "agent-body",
            div {
                class: "chat-col",
                style: if right_open { "flex: 0 0 var(--chat-w, 760px); max-width: 70%;" } else { "" },
                ChatHeader { id: id.clone() }
                Transcript { id: id.clone() }
                AboveComposer { id: id.clone() }
                Composer { session_id: Some(id.clone()) }
                if right_open {
                    div {
                        class: if dragging() { "resizer dragging" } else { "resizer" },
                        onmousedown: move |_| dragging.set(true),
                    }
                }
            }
            if right_open {
                RightPane { id: id.clone() }
            } else if let Some(name) = ws_name {
                div { class: "tools-col",
                    div { style: "height: 38px;" }
                    div { class: "h", "On {name}" }
                    ToolLink { tab: RightTab::Changes, icon: I::Diff, label: "Changes" }
                    ToolLink { tab: RightTab::Browser, icon: I::Globe, label: "Browser" }
                    ToolLink { tab: RightTab::Terminal, icon: I::Terminal, label: "Terminal" }
                    ToolLink { tab: RightTab::Files, icon: I::Files, label: "Files" }
                }
            }
            if dragging() {
                div {
                    style: "position: fixed; inset: 0; z-index: 300; cursor: col-resize;",
                    onmousemove: move |e| {
                        let side = if app.ui.read().sidebar_open { app.config.read().settings.sidebar_width as f64 } else { 0.0 };
                        let w = (e.client_coordinates().x - side).clamp(420.0, 1400.0) as u32;
                        app.config.write().settings.chat_width = w;
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

#[component]
fn ToolLink(tab: RightTab, icon: I, label: &'static str) -> Element {
    let app = use_app();
    rsx! {
        button { class: "tool-link", onclick: move |_| open_tool(app, tab),
            span { class: "ic", Icon { i: icon, size: 16, stroke: 1.5 } }
            "{label}"
        }
    }
}

#[component]
fn ChatHeader(id: String) -> Element {
    let mut app = use_app();
    let session = app.open.read().get(&id).cloned();
    let mut editing = use_signal(|| false);
    let mut draft_title = use_signal(String::new);
    let mut menu = use_signal(|| false);
    let mut export_menu = use_signal(|| false);
    let Some(s) = session else { return rsx! {} };
    let right_open = app.ui.read().right_open;
    let (env_icon, env_tip) = match &s.environment {
        Environment::Worktree { branch, .. } => (I::Layers, format!("Worktree · {branch}")),
        Environment::Ssh { host, .. } => (I::Server, format!("SSH · {host}")),
        Environment::Local => (I::Laptop, "This PC".to_string()),
    };
    let title = s.title.clone();
    let running = app.is_running(&id);
    let (id_a, id_b, id_c, id_d, id_e, id_f, id_g, id_h, id_i) =
        (id.clone(), id.clone(), id.clone(), id.clone(), id.clone(), id.clone(), id.clone(), id.clone(), id.clone());

    rsx! {
        div { class: "main-head",
            SidebarToggle {}
            if editing() {
                input {
                    class: "chat-title-input",
                    value: "{draft_title}",
                    autofocus: true,
                    oninput: move |e| draft_title.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter {
                            app.rename(&id_a, draft_title());
                            editing.set(false);
                        } else if e.key() == Key::Escape {
                            editing.set(false);
                        }
                    },
                    onblur: move |_| {
                        app.rename(&id_b, draft_title());
                        editing.set(false);
                    },
                }
            } else {
                div { class: "title-chip", title: "Double-click to rename",
                    ondoubleclick: move |_| {
                        draft_title.set(title.clone());
                        editing.set(true);
                    },
                    span { class: "t", dir: "auto", "{s.title}" }
                    span { class: "ic", title: "{env_tip}", Icon { i: env_icon, size: 14, stroke: 1.5 } }
                }
            }
            if running {
                span { class: "sunburst", style: "margin-left: 6px;", Icon { i: I::Loader, size: 14 } }
            }
            DragRegion {}
            if s.cwd().is_some() {
                button { class: "head-link", title: "Open the folder in Cursor / VS Code", onclick: move |_| app.open_in_ide(&id_c),
                    "IDE" Icon { i: I::ArrowUpRight, size: 12 }
                }
            }
            div { class: "menu-anchor",
                button { class: "icon-btn", title: "More", onclick: move |_| menu.set(true), Icon { i: I::More, size: 16 } }
                Dropdown { open: menu, up: false, right: true,
                    MenuItem { label: "Rename".to_string(), icon: Some(I::Pencil), onclick: move |_| { menu.set(false); app.ui.write().modal = Some(Modal::Rename(id_d.clone())); } }
                    MenuItem { label: "Fork".to_string(), icon: Some(I::Fork), onclick: move |_| { menu.set(false); app.fork(&id_e, None); } }
                    MenuItem { label: "Open in Terminal".to_string(), icon: Some(I::Terminal), onclick: move |_| { menu.set(false); app.open_in_terminal(&id_f); } }
                    MenuItem { label: "Export…".to_string(), icon: Some(I::Upload), onclick: move |_| { menu.set(false); export_menu.set(true); } }
                    MenuItem { label: if s.pinned { "Unpin".to_string() } else { "Pin".to_string() }, icon: Some(I::Pin), onclick: move |_| { menu.set(false); app.toggle_pin(&id_g); } }
                    MenuItem { label: if s.archived { "Unarchive".to_string() } else { "Archive".to_string() }, icon: Some(I::Archive), onclick: move |_| { menu.set(false); app.toggle_archive(&id_i); } }
                    div { class: "menu-sep" }
                    MenuItem { label: "Delete".to_string(), icon: Some(I::Trash), danger: true, onclick: move |_| { menu.set(false); app.ui.write().modal = Some(Modal::ConfirmDelete(id_h.clone())); } }
                }
                Dropdown { open: export_menu, up: false, right: true,
                    div { class: "menu-label", "Export as" }
                    for f in ExportFormat::ALL.iter().copied() {
                        MenuItem {
                            key: "{f.label()}",
                            label: f.label().to_string(),
                            onclick: {
                                let id = id.clone();
                                move |_| {
                                    export_menu.set(false);
                                    app.export(&id, f);
                                }
                            },
                        }
                    }
                }
            }
            if !right_open {
                button { class: "icon-btn", title: "Show side panel (Ctrl+Shift+J)",
                    onclick: move |_| {
                        app.ui.write().right_open = true;
                        app.update_config(|c| c.settings.right_panel_open = true);
                    },
                    Icon { i: I::PanelRight, size: 16, stroke: 1.5 }
                }
            }
        }
    }
}

/// "Changes +N -M" and "Commit & Push ▾" above the composer.
#[component]
fn AboveComposer(id: String) -> Element {
    let app = use_app();
    let mut menu = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let session = app.open.read().get(&id).cloned();
    let running = app.is_running(&id);
    let cwd = session.as_ref().and_then(|s| s.cwd());
    let updated = session.as_ref().map(|s| s.updated_at);
    let mut nonce = use_signal(|| 0u32);
    let cwd2 = cwd.clone();
    let stats = use_resource(use_reactive!(|cwd2, updated| {
        let _ = (updated, nonce());
        async move {
            let c = cwd2?;
            tokio::task::spawn_blocking(move || {
                let p = Path::new(&c);
                if !git::is_repo(p) {
                    return None;
                }
                let st = git::status(p).ok()?;
                let (a, r) = st.totals();
                Some((st.files.len(), a, r))
            })
            .await
            .ok()
            .flatten()
        }
    }));
    let Some(s) = session else { return rsx! {} };
    let git_stats = stats.read().clone().flatten();
    let (files, added, removed) = match git_stats {
        Some(g) => g,
        None => {
            let (a, r, f) = s.diff_stats();
            (f, a, r)
        }
    };
    if files == 0 || running {
        return rsx! {};
    }
    let in_git = git_stats.is_some();
    let title = s.title.clone();
    let summary = s.last_assistant_text().unwrap_or_default();
    let run = move |what: &'static str| {
        let Some(c) = cwd.clone() else { return };
        let (t, body) = (title.clone(), summary.clone());
        busy.set(true);
        menu.set(false);
        spawn(async move {
            let r = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
                let p = Path::new(&c);
                match what {
                    "commit" => git::commit(p, &t, true),
                    "push" => {
                        git::commit(p, &t, true)?;
                        git::push(p)
                    }
                    _ => {
                        let branch = git::current_branch(p).unwrap_or_default();
                        if branch == git::default_branch(p) {
                            git::checkout_new_branch(p, &format!("larrier/{}", git::branch_slug(&t)))?;
                        }
                        git::commit(p, &t, true)?;
                        git::push(p)?;
                        git::create_pr(p, &t, &format!("{body}\n\n— Created with Larrier"), false)
                    }
                }
            })
            .await;
            busy.set(false);
            match r {
                Ok(Ok(out)) => app.toast(out.lines().last().unwrap_or("Done").to_string()),
                Ok(Err(e)) => app.toast_err(e.to_string()),
                Err(e) => app.toast_err(e.to_string()),
            }
            nonce += 1;
        });
    };
    let mut run2 = run.clone();
    let mut run3 = run.clone();
    let mut run4 = run.clone();
    let mut run = run;
    rsx! {
        div { class: "composer-wrap", style: "padding-bottom: 8px;",
            div { class: "row", style: "gap: 6px;",
                button { class: "pill-btn", onclick: move |_| open_tool(app, RightTab::Changes),
                    "Changes"
                    span { class: "add num", "+{added}" }
                    span { class: "del num", "-{removed}" }
                }
                if in_git {
                    div { class: "menu-anchor",
                        div { class: "pill-btn", style: "padding: 0; gap: 0;",
                            button { style: "padding: 0 10px; height: 100%;", disabled: busy(), onclick: move |_| run("push"),
                                if busy() { "Working…" } else { "Commit & Push" }
                            }
                            button { style: "padding: 0 8px 0 2px; height: 100%; display: flex; align-items: center; color: var(--fg-3);", onclick: move |_| menu.set(true),
                                Icon { i: I::ChevronDown, size: 12 }
                            }
                        }
                        Dropdown { open: menu, up: true,
                            MenuItem { label: "Commit".to_string(), icon: Some(I::Commit), onclick: move |_| run2("commit") }
                            MenuItem { label: "Commit & Push".to_string(), icon: Some(I::Upload), onclick: move |_| run3("push") }
                            MenuItem { label: "Create Pull Request".to_string(), icon: Some(I::PullRequest), onclick: move |_| run4("pr") }
                        }
                    }
                }
            }
        }
    }
}
