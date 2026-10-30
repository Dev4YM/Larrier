//! Customize: every agent's MCP servers, skills, subagents, rules, commands,
//! hooks and plugins on one page (modeled on Cursor's Customize screen, but
//! across all installed harnesses).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use dioxus::prelude::*;
use larrier_agents::harness::{self, HarnessItem, ItemKind, Scope, Status};
use larrier_core::AgentKind;

use super::AgentGlyph;
use crate::icons::{Icon, I};
use crate::state::use_app;

const VISIBLE: usize = 6;

const CSS: &str = r#"
.cz { flex: 1; overflow-y: auto; }
.cz-inner { max-width: 920px; margin: 0 auto; padding: 28px 28px 64px; display: flex; flex-direction: column; gap: 14px; }
.cz-search { display: flex; gap: 10px; align-items: center; }
.cz-search .box { flex: 1; height: 36px; display: flex; align-items: center; gap: 8px; padding: 0 12px; border-radius: 10px; border: 1px solid var(--border); background: var(--card); color: var(--fg-3); }
.cz-search .box:focus-within { border-color: var(--border-2); }
.cz-search input { flex: 1; border: none; outline: none; background: transparent; font-size: 13px; color: var(--fg); min-width: 0; }
.cz-dark { height: 36px; padding: 0 16px; border-radius: 18px; background: var(--fg); color: var(--pane, var(--bg)); font-size: 13px; font-weight: 500; display: inline-flex; align-items: center; gap: 6px; white-space: nowrap; }
.cz-dark:hover { opacity: .9; }
.cz-filters { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.cz-pill { height: 32px; padding: 0 12px; border-radius: 16px; border: 1px solid var(--border); background: var(--card); font-size: 13px; color: var(--fg); display: inline-flex; align-items: center; gap: 6px; white-space: nowrap; }
.cz-pill:hover { border-color: var(--border-2); }
.cz-pill.on { background: var(--selected); border-color: var(--border-2); }
.cz-pill .n { color: var(--fg-3); font-size: 12px; }
.cz-sep { width: 1px; height: 20px; background: var(--border); margin: 0 2px; }
.cz-scope { position: relative; }
.cz-menu { position: absolute; top: calc(100% + 4px); left: 0; z-index: 50; min-width: 220px; max-height: 360px; overflow-y: auto; background: var(--card-2, var(--card)); border-radius: 8px; box-shadow: var(--shadow-pop, 0 8px 24px rgba(0,0,0,.15)); padding: 4px; }
.cz-group-title { font-size: 14px; font-weight: 600; margin: 10px 2px 2px; display: flex; align-items: baseline; gap: 6px; }
.cz-group-title .n { font-weight: 400; color: var(--fg-3); font-size: 13px; }
.cz-card { border: 1px solid var(--border); border-radius: 12px; background: var(--card); overflow: hidden; }
.cz-row { display: flex; align-items: center; gap: 12px; padding: 10px 14px; min-height: 58px; }
.cz-row + .cz-row { border-top: 1px solid var(--border); }
.cz-row:hover { background: var(--hover); }
.cz-ic { position: relative; width: 32px; height: 32px; border-radius: 8px; background: var(--chip, var(--hover)); display: flex; align-items: center; justify-content: center; color: var(--fg-2); flex-shrink: 0; }
.cz-ic .st { position: absolute; right: -2px; bottom: -2px; width: 10px; height: 10px; border-radius: 50%; border: 2px solid var(--card); }
.cz-ic .st.ok { background: var(--green); }
.cz-ic .st.auth { background: var(--amber); }
.cz-ic .st.err { background: var(--red); }
.cz-ic .st.off, .cz-ic .st.unk { background: var(--fg-4); }
.cz-body { flex: 1; min-width: 0; }
.cz-l1 { display: flex; align-items: center; gap: 6px; font-size: 13.5px; min-width: 0; }
.cz-l1 .nm { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.cz-badge { height: 20px; padding: 0 7px; border-radius: 10px; background: var(--chip, var(--hover)); color: var(--fg-2); font-size: 11.5px; display: inline-flex; align-items: center; flex-shrink: 0; }
.cz-l2 { font-size: 12px; color: var(--fg-2); margin-top: 2px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.cz-l2.err { color: var(--red); }
.cz-l2.auth { color: var(--amber); }
.cz-agents { display: flex; gap: 4px; flex-shrink: 0; }
.cz-act { font-size: 13px; color: var(--blue); padding: 4px 6px; border-radius: 6px; flex-shrink: 0; }
.cz-act:hover { background: var(--hover); }
.cz-more { font-size: 12.5px; color: var(--fg-3); display: inline-flex; align-items: center; gap: 4px; padding: 6px 2px; }
.cz-more:hover { color: var(--fg); }
.cz-empty { padding: 40px 0; text-align: center; color: var(--fg-3); font-size: 13px; line-height: 20px; }
"#;

fn kind_icon(k: ItemKind) -> I {
    match k {
        ItemKind::Plugin => I::Puzzle,
        ItemKind::Mcp => I::Plug,
        ItemKind::Skill => I::Sparkle,
        ItemKind::Subagent => I::Bot,
        ItemKind::Rule => I::List,
        ItemKind::Command => I::Command,
        ItemKind::Hook => I::Zap,
    }
}

fn matches_query(it: &HarnessItem, q: &str) -> bool {
    if q.is_empty() {
        return true;
    }
    let hay = format!(
        "{} {} {} {} {}",
        it.name,
        it.description.clone().unwrap_or_default(),
        it.detail.clone().unwrap_or_default(),
        it.kind.label(),
        it.agents.iter().map(|a| a.display_name()).collect::<Vec<_>>().join(" ")
    )
    .to_lowercase();
    q.split_whitespace().all(|w| hay.contains(w))
}

#[component]
pub fn CustomizeView() -> Element {
    let app = use_app();
    let mut nonce = use_signal(|| 0u32);
    let mut query = use_signal(String::new);
    // Opened from `/mcp`: on that agent's MCP servers.
    let focus = use_hook(|| app.ui.peek().customize_focus.clone());
    let mut kind_filter = use_signal(|| focus.as_ref().and_then(|f| f.0));
    let mut agent_filter = use_signal(|| focus.as_ref().and_then(|f| f.1.clone()));
    use_hook(move || {
        let mut app = app;
        app.ui.write().customize_focus = None;
    });
    let mut scope_open = use_signal(|| false);
    let expanded = use_signal(BTreeSet::<String>::new);

    let data = use_resource(move || {
        let _ = nonce();
        let ws: Vec<PathBuf> = app.config.read().workspaces.iter().map(|w| PathBuf::from(&w.path)).collect();
        async move { tokio::task::spawn_blocking(move || harness::discover(&ws)).await.unwrap_or_default() }
    });

    let mut all: Option<Vec<HarnessItem>> = data.read().clone();
    // MCP servers the agents report themselves (account connectors,
    // plugins), next to the ones their config files name.
    if let Some(items) = all.as_mut() {
        for (id, models) in app.native_models.read().iter() {
            let kind = AgentKind::from_id(id);
            for s in &models.mcp {
                let known = items.iter().any(|i| i.kind == ItemKind::Mcp && i.name.eq_ignore_ascii_case(&s.name) && i.agents.contains(&kind));
                if known {
                    continue;
                }
                items.push(HarnessItem {
                    kind: ItemKind::Mcp,
                    name: s.name.clone(),
                    agents: vec![kind.clone()],
                    scope: Scope::User,
                    source: PathBuf::new(),
                    description: Some({
                        let from = match s.source.as_deref() {
                            Some("claudeai") => "From your claude.ai account".to_string(),
                            Some("plugin") => "From a plugin".to_string(),
                            Some("managed") => "Set by your organization".to_string(),
                            Some(src @ ("user" | "project" | "local")) => format!("From {}'s {src} settings", kind.display_name()),
                            _ => format!("Reported by {}", kind.display_name()),
                        };
                        match s.tools {
                            0 => from,
                            1 => format!("{from} · 1 tool"),
                            n => format!("{from} · {n} tools"),
                        }
                    }),
                    detail: (s.tools > 0).then(|| format!("{} tools", s.tools)),
                    status: match s.status.as_str() {
                        "connected" => Status::Ok,
                        "needs-auth" => Status::NeedsAuth,
                        "failed" => Status::Error(s.error.clone().unwrap_or_else(|| "Failed to connect".into())),
                        "disabled" => Status::Disabled,
                        _ => Status::Unknown,
                    },
                });
            }
        }
    }
    let q = query.read().trim().to_lowercase();
    let agent = agent_filter();
    let kind = kind_filter();

    let (agents, by_agent): (Vec<(AgentKind, usize)>, Vec<HarnessItem>) = match &all {
        Some(items) => {
            let mut counts: Vec<(AgentKind, usize)> = Vec::new();
            for it in items {
                for a in &it.agents {
                    match counts.iter_mut().find(|(k, _)| k == a) {
                        Some(e) => e.1 += 1,
                        None => counts.push((a.clone(), 1)),
                    }
                }
            }
            counts.sort_by(|a, b| b.1.cmp(&a.1));
            let filtered = items
                .iter()
                .filter(|it| agent.as_ref().map(|a| it.agents.contains(a)).unwrap_or(true))
                .filter(|it| matches_query(it, &q))
                .cloned()
                .collect();
            (counts, filtered)
        }
        None => (Vec::new(), Vec::new()),
    };
    let count_of = |k: ItemKind| by_agent.iter().filter(|it| it.kind == k).count();
    let visible: Vec<HarnessItem> = by_agent.iter().filter(|it| kind.map(|k| it.kind == k).unwrap_or(true)).cloned().collect();
    let attention: Vec<HarnessItem> = visible.iter().filter(|it| it.status.needs_attention()).cloned().collect();
    let installed: Vec<HarnessItem> = visible.iter().filter(|it| matches!(it.status, Status::Ok | Status::Unknown)).cloned().collect();
    let disabled: Vec<HarnessItem> = visible.iter().filter(|it| it.status == Status::Disabled).cloned().collect();
    let scope_label = agent.as_ref().map(|a| a.display_name()).unwrap_or_else(|| "All agents".into());
    let total = by_agent.len();

    rsx! {
        style { {CSS} }
        div { class: "cz",
            div { class: "cz-inner",
                div { class: "cz-search",
                    div { class: "box",
                        Icon { i: I::Search, size: 15 }
                        input {
                            placeholder: "Search Plugins, Skills, MCPs, Rules, Hooks…",
                            value: "{query}",
                            oninput: move |e| query.set(e.value()),
                        }
                    }
                    button { class: "cz-dark", title: "Re-scan every agent's configuration", onclick: move |_| nonce += 1,
                        Icon { i: I::Refresh, size: 14 }
                        "Refresh"
                    }
                }
                div { class: "cz-filters",
                    div { class: "cz-scope",
                        button { class: "cz-pill", onclick: move |_| scope_open.toggle(),
                            match &agent {
                                Some(a) => rsx! { AgentGlyph { kind: a.clone() } },
                                None => rsx! { Icon { i: I::Globe, size: 14 } },
                            }
                            "{scope_label}"
                            Icon { i: I::ChevronDown, size: 12 }
                        }
                        if scope_open() {
                            div { style: "position: fixed; inset: 0; z-index: 40;", onclick: move |_| scope_open.set(false) }
                            div { class: "cz-menu",
                                button { class: "menu-item",
                                    onclick: move |_| { agent_filter.set(None); scope_open.set(false); },
                                    Icon { i: I::Globe, size: 14 }
                                    span { "All agents" }
                                    span { class: "sub", "{all.as_ref().map(|a| a.len()).unwrap_or(0)}" }
                                }
                                for (a, n) in agents.clone() {
                                    {
                                        let a2 = a.clone();
                                        rsx! {
                                            button { key: "{a.id()}", class: "menu-item",
                                                onclick: move |_| { agent_filter.set(Some(a2.clone())); scope_open.set(false); },
                                                AgentGlyph { kind: a.clone() }
                                                span { "{a.display_name()}" }
                                                span { class: "sub", "{n}" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    div { class: "cz-sep" }
                    button { class: if kind.is_none() { "cz-pill on" } else { "cz-pill" }, onclick: move |_| kind_filter.set(None),
                        "All"
                        span { class: "n", "{total}" }
                    }
                    for k in ItemKind::ALL {
                        {
                            let n = count_of(k);
                            rsx! {
                                button { key: "{k.plural()}", class: if kind == Some(k) { "cz-pill on" } else { "cz-pill" },
                                    onclick: move |_| kind_filter.set(if kind == Some(k) { None } else { Some(k) }),
                                    "{k.plural()}"
                                    if n > 0 { span { class: "n", "{n}" } }
                                }
                            }
                        }
                    }
                }
                match &all {
                    None => rsx! { div { class: "cz-empty", div { class: "spinner lg", style: "margin: 0 auto 10px;" } "Scanning agent configurations…" } },
                    Some(_) if visible.is_empty() => rsx! {
                        div { class: "cz-empty",
                            if q.is_empty() && kind.is_none() && agent.is_none() {
                                "No MCP servers, skills, subagents, rules, commands, hooks or plugins were found for your installed agents."
                            } else {
                                "Nothing matches these filters."
                            }
                        }
                    },
                    Some(_) => rsx! {
                        Group { gkey: "attention", title: "Needs Attention", items: attention.clone(), expanded }
                        Group { gkey: "installed", title: "Installed", items: installed.clone(), expanded }
                        Group { gkey: "disabled", title: "Disabled", items: disabled.clone(), expanded }
                    },
                }
            }
        }
    }
}

#[component]
fn Group(gkey: &'static str, title: &'static str, items: Vec<HarnessItem>, expanded: Signal<BTreeSet<String>>) -> Element {
    let mut expanded = expanded;
    if items.is_empty() {
        return rsx! {};
    }
    let open = expanded.read().contains(gkey);
    let n = items.len();
    let shown: Vec<HarnessItem> = if open { items.clone() } else { items.iter().take(VISIBLE).cloned().collect() };
    let hidden = n.saturating_sub(shown.len());
    rsx! {
        div { class: "cz-group-title", "{title}" span { class: "n", "{n}" } }
        div { class: "cz-card",
            for (i, it) in shown.into_iter().enumerate() {
                Row { key: "{gkey}-{i}-{it.name}", it }
            }
        }
        if hidden > 0 {
            button { class: "cz-more",
                onclick: move |_| { expanded.write().insert(gkey.to_string()); },
                "Show {hidden} more"
                Icon { i: I::ChevronDown, size: 12 }
            }
        } else if open && n > VISIBLE {
            button { class: "cz-more",
                onclick: move |_| { expanded.write().remove(gkey); },
                "Show less"
            }
        }
    }
}

#[component]
fn Row(it: HarnessItem) -> Element {
    let app = use_app();
    let (dot, l2_class, status_text) = match &it.status {
        Status::Ok => ("ok", "cz-l2", None),
        Status::NeedsAuth => ("auth", "cz-l2 auth", Some("Needs authentication".to_string())),
        Status::Error(m) => ("err", "cz-l2 err", Some(format!("Error · {m}"))),
        Status::Disabled => ("off", "cz-l2", Some("Disabled".to_string())),
        Status::Unknown => ("unk", "cz-l2", None),
    };
    let scope = match &it.scope {
        Scope::User => "User".to_string(),
        Scope::Project(p) => {
            let name = Path::new(p).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.clone());
            format!("Project · {name}")
        }
    };
    let body = it.description.clone().or_else(|| it.detail.clone()).unwrap_or_default();
    let line2 = match (status_text, body.is_empty()) {
        (Some(s), true) => s,
        (Some(s), false) => format!("{s} — {body}"),
        (None, false) => body,
        (None, true) => it.source.to_string_lossy().to_string(),
    };
    let title = format!("{}\n{}", it.source.display(), it.detail.clone().unwrap_or_default());
    // Servers an agent reports have no file of ours to open.
    let has_file = !it.source.as_os_str().is_empty();
    let src = it.source.clone();
    let src2 = it.source.clone();
    rsx! {
        div { class: "cz-row", title: "{title}",
            div { class: "cz-ic",
                Icon { i: kind_icon(it.kind), size: 16 }
                span { class: "st {dot}" }
            }
            div { class: "cz-body",
                div { class: "cz-l1",
                    span { class: "nm", "{it.name}" }
                    span { class: "cz-badge", "{it.kind.label()}" }
                    span { class: "cz-badge", "{scope}" }
                }
                div { class: "{l2_class}", "{line2}" }
            }
            div { class: "cz-agents",
                for a in it.agents.clone() {
                    AgentGlyph { key: "{a.id()}", kind: a.clone() }
                }
            }
            if has_file {
                button { class: "cz-act", title: "Open the file this comes from",
                    onclick: move |_| {
                        if let Err(e) = open::that(&src) {
                            app.toast_err(format!("Could not open {}: {e}", src.display()));
                        }
                    },
                    "Open"
                }
                button { class: "icon-btn", title: "Show in folder",
                    onclick: move |_| reveal(&src2),
                    Icon { i: I::FolderOpen, size: 14 }
                }
            }
        }
    }
}

/// Show a file in the OS file manager (selected where supported).
fn reveal(path: &Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(format!("/select,{}", path.display())).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let dir = if path.is_dir() { path.to_path_buf() } else { path.parent().map(|p| p.to_path_buf()).unwrap_or_default() };
        let _ = open::that(dir);
    }
}
