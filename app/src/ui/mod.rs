//! UI components.

mod automations;
mod chat;
mod composer;
mod customize;
mod home;
mod menubar;
mod overlays;
mod right;
mod settings;
mod sidebar;
mod transcript;

use dioxus::prelude::*;
use larrier_core::AgentKind;

use crate::icons::{Icon, I};
use crate::state::{App, SettingsTab, View};

pub use overlays::{ContextMenu, GlobalMenu, ModalHost, Palette, Toasts};

#[component]
pub fn Root() -> Element {
    let app = use_context_provider(App::new);

    use_hook(move || {
        crate::automations::start(app);
        app.detect_agents();
        if app.config.read().settings.auto_import {
            app.sync_imports(true);
        }
        // `larrier-desktop --open <session-id>` opens a session directly.
        let args: Vec<String> = std::env::args().collect();
        if let Some(id) = args.iter().position(|a| a == "--open").and_then(|i| args.get(i + 1)) {
            app.open_session(id);
        }
        // `--prompt <text> [--agent <id>] [--cwd <dir>]` starts a new agent.
        let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
        if let Some(prompt) = arg("--prompt") {
            let mut app = app;
            if let Some(a) = arg("--agent") {
                app.draft.write().agent = larrier_core::AgentKind::from_id(&a);
            }
            if let Some(dir) = arg("--cwd") {
                app.draft.write().workspace = Some(dir);
            }
            if let Some(m) = arg("--model") {
                app.draft.write().model = Some(m);
            }
            app.send(None, prompt, Vec::new());
        }
        match arg("--page").as_deref() {
            Some("automations") => app.navigate(View::Automations),
            Some("customize") => app.navigate(View::Customize),
            _ => {}
        }
        if let Some(tab) = args.iter().position(|a| a == "--settings").map(|i| args.get(i + 1).cloned().unwrap_or_default()) {
            let t = SettingsTab::ALL.into_iter().find(|t| t.label().to_lowercase().starts_with(&tab.to_lowercase())).unwrap_or(SettingsTab::General);
            app.open_settings(t);
        }
    });

    // Bridge: system theme + global keyboard shortcuts from the webview.
    use_future(move || async move {
        let mut ev = document::eval(
            r#"
            const mq = window.matchMedia('(prefers-color-scheme: dark)');
            dioxus.send({ t: 'theme', dark: mq.matches });
            mq.addEventListener('change', (e) => dioxus.send({ t: 'theme', dark: e.matches }));
            window.addEventListener('keydown', (e) => {
              const mod = e.ctrlKey || e.metaKey;
              const k = (e.key || '').toLowerCase();
              if (k === 'escape') { dioxus.send({ t: 'key', k: 'escape', shift: false }); return; }
              if (e.altKey && (k === 'arrowleft' || k === 'arrowright')) { e.preventDefault(); dioxus.send({ t: 'nav', d: k === 'arrowleft' ? -1 : 1 }); return; }
              if (!mod) return;
              if (['k','n','b','j',',','l','p','r','w','e','g','i','.','/','backspace','=','+','-','0'].includes(k)) {
                e.preventDefault();
                dioxus.send({ t: 'key', k, shift: e.shiftKey, alt: e.altKey });
              }
            }, true);
            // Mouse back / forward buttons.
            window.addEventListener('mouseup', (e) => {
              if (e.button === 3 || e.button === 4) { e.preventDefault(); dioxus.send({ t: 'nav', d: e.button === 3 ? -1 : 1 }); }
            });
            // Larrier's own right-click menu everywhere (never the browser's).
            window.addEventListener('contextmenu', (e) => {
              e.preventDefault();
              const t = e.target instanceof Element ? e.target : document.body;
              if (t.closest('[data-ctx]')) return; // components with their own menu
              const ed = t.closest('input, textarea, [contenteditable="true"]');
              window.__larrierCtx = ed || null;
              let sel = '';
              if (ed && typeof ed.selectionStart === 'number') sel = ed.value.substring(ed.selectionStart, ed.selectionEnd);
              else sel = String(window.getSelection() || '');
              const a = t.closest('a[href]');
              const pre = t.closest('pre');
              const pathEl = t.closest('[data-path]');
              dioxus.send({ t: 'ctx', x: e.clientX, y: e.clientY, editable: !!ed, selection: sel.slice(0, 200000),
                link: a ? a.href : null, code: pre ? pre.innerText.slice(0, 500000) : null,
                path: pathEl ? pathEl.getAttribute('data-path') : null, image: t.tagName === 'IMG' ? t.src : null });
            }, true);
            "#,
        );
        while let Ok(v) = ev.recv::<serde_json::Value>().await {
            handle_bridge(app, v);
        }
    });

    let ui = app.ui.read();
    let cfg = app.config.read();
    let theme = if ui.dark { "dark" } else { "light" };
    let style = format!(
        "--sidebar-w: {}px; --chat-w: {}px; --fs: {}px;",
        cfg.settings.sidebar_width, cfg.settings.chat_width, cfg.settings.font_size
    );
    let sidebar_open = ui.sidebar_open;
    drop(ui);
    drop(cfg);

    let in_settings = matches!(&*app.view.read(), View::Settings(_));
    rsx! {
        div { class: "app", "data-theme": "{theme}", style: "{style}",
            crate::icons::IconSprite {}
            menubar::MenuBar {}
            div { class: "app-body",
                if in_settings {
                    settings::SettingsNav {}
                } else if sidebar_open {
                    sidebar::Sidebar {}
                }
                Main {}
            }
            Palette {}
            ModalHost {}
            ContextMenu {}
            GlobalMenu {}
            Toasts {}
        }
    }
}

fn handle_bridge(mut app: App, v: serde_json::Value) {
    match v["t"].as_str() {
        Some("theme") => {
            app.ui.write().system_dark = v["dark"].as_bool().unwrap_or(false);
            app.apply_theme();
        }
        Some("key") => {
            let k = v["k"].as_str().unwrap_or("");
            let shift = v["shift"].as_bool().unwrap_or(false);
            match (k, shift) {
                ("escape", _) => {
                    let mut ui = app.ui.write();
                    if ui.gctx.is_some() || ui.ctx.is_some() || ui.menubar.is_some() {
                        ui.gctx = None;
                        ui.ctx = None;
                        ui.menubar = None;
                    } else if ui.palette_open {
                        ui.palette_open = false;
                    } else if ui.modal.is_some() {
                        ui.modal = None;
                    }
                }
                ("k", _) | ("p", true) => {
                    let mut ui = app.ui.write();
                    ui.palette_open = !ui.palette_open;
                    ui.palette_query.clear();
                }
                ("n", _) => app.new_agent(),
                ("b", false) => {
                    let mut ui = app.ui.write();
                    ui.sidebar_open = !ui.sidebar_open;
                }
                ("b", true) => open_tool(app, crate::state::RightTab::Browser),
                ("j", true) => {
                    let open = !app.ui.read().right_open;
                    app.ui.write().right_open = open;
                    app.update_config(|c| c.settings.right_panel_open = open);
                }
                ("j", false) => open_tool(app, crate::state::RightTab::Terminal),
                ("e", false) => open_tool(app, crate::state::RightTab::Changes),
                ("g", _) => open_tool(app, crate::state::RightTab::Files),
                ("=", _) | ("+", _) => zoom(app, 0.1),
                ("-", _) => zoom(app, -0.1),
                ("0", _) => zoom(app, 0.0),
                (",", _) => app.open_settings(SettingsTab::General),
                ("l", _) => app.ui.write().focus_composer += 1,
                ("i", true) => app.sync_imports(false),
                (".", _) => {
                    if let Some(id) = app.current_id() {
                        app.stop(&id);
                    }
                }
                _ => {}
            }
        }
        Some("nav") => app.go(v["d"].as_i64().unwrap_or(0) as isize),
        Some("ctx") => {
            let s = |k: &str| v[k].as_str().map(|x| x.to_string()).filter(|x| !x.is_empty());
            let mut ui = app.ui.write();
            ui.ctx = None;
            ui.menubar = None;
            ui.gctx = Some(crate::state::GlobalCtx {
                x: v["x"].as_f64().unwrap_or(0.0),
                y: v["y"].as_f64().unwrap_or(0.0),
                editable: v["editable"].as_bool().unwrap_or(false),
                selection: s("selection").unwrap_or_default(),
                link: s("link"),
                code: s("code"),
                path: s("path"),
                image: s("image"),
            });
        }
        _ => {}
    }
}

/// Open the review pane on a tool tab (Changes, Browser, Terminal, Files).
pub fn open_tool(mut app: App, tab: crate::state::RightTab) {
    let mut ui = app.ui.write();
    ui.right_open = true;
    ui.right_tab = tab;
}

thread_local! {
    static ZOOM: std::cell::Cell<f64> = const { std::cell::Cell::new(1.0) };
}

/// Ctrl + / Ctrl - / Ctrl 0 page zoom.
pub fn zoom(_app: App, delta: f64) {
    let z = ZOOM.with(|z| {
        let v = if delta == 0.0 { 1.0 } else { (z.get() + delta).clamp(0.6, 2.0) };
        z.set(v);
        v
    });
    dioxus::desktop::window().set_zoom_level(z);
}

#[component]
fn Main() -> Element {
    let app = crate::state::use_app();
    let view = app.view.read().clone();
    rsx! {
        div { class: "main",
            match view {
                View::Home => rsx! { home::Home {} },
                View::Agent(id) => rsx! { chat::AgentView { key: "{id}", id: id.clone() } },
                View::Settings(tab) => rsx! { settings::SettingsView { tab } },
                View::Automations => rsx! { PageFrame { automations::AutomationsView {} } },
                View::Customize => rsx! { PageFrame { customize::CustomizeView {} } },
            }
        }
    }
}

/// Header + scroll area for full pages (Automations, Customize).
#[component]
fn PageFrame(children: Element) -> Element {
    rsx! {
        div { class: "main-head",
            SidebarToggle {}
            DragRegion {}
        }
        div { style: "flex: 1; min-height: 0; overflow-y: auto;", {children} }
    }
}

// ------------------------------------------------------------------ shared bits

/// Minimize / maximize / close for the frameless window.
#[component]
pub fn WindowControls() -> Element {
    let mut app = crate::state::use_app();
    let window = dioxus::desktop::use_window();
    let maximized = app.ui.read().maximized;
    let w1 = window.clone();
    let w2 = window.clone();
    let w3 = window.clone();
    if cfg!(target_os = "macos") {
        return rsx! {};
    }
    rsx! {
        div { class: "win-controls",
            button { class: "win-btn", title: "Minimize", onclick: move |_| w1.set_minimized(true),
                Icon { i: I::Minimize, size: 14, stroke: 1.2 }
            }
            button { class: "win-btn", title: if maximized { "Restore" } else { "Maximize" },
                onclick: move |_| {
                    w2.toggle_maximized();
                    let m = w2.is_maximized();
                    app.ui.write().maximized = m;
                },
                if maximized {
                    Icon { i: I::Restore, size: 13, stroke: 1.2 }
                } else {
                    Icon { i: I::Maximize, size: 13, stroke: 1.2 }
                }
            }
            button { class: "win-btn close", title: "Close", onclick: move |_| w3.close(),
                Icon { i: I::X, size: 15, stroke: 1.2 }
            }
        }
    }
}

/// Empty space in a header that drags the window (double-click maximizes).
#[component]
pub fn DragRegion() -> Element {
    let mut app = crate::state::use_app();
    let window = dioxus::desktop::use_window();
    let w2 = window.clone();
    rsx! {
        div { class: "drag",
            onmousedown: move |e| {
                if e.trigger_button() == Some(dioxus::html::input_data::MouseButton::Primary) {
                    window.drag();
                }
            },
            ondoubleclick: move |_| {
                w2.toggle_maximized();
                app.ui.write().maximized = w2.is_maximized();
            },
        }
    }
}

/// Sidebar toggle shown in pane headers when the sidebar is hidden.
#[component]
pub fn SidebarToggle() -> Element {
    let mut app = crate::state::use_app();
    let open = app.ui.read().sidebar_open;
    let review = app
        .sessions
        .read()
        .iter()
        .filter(|s| !s.archived && s.unread && matches!(s.status, larrier_core::SessionStatus::Review | larrier_core::SessionStatus::Done))
        .count();
    if open {
        return rsx! {};
    }
    rsx! {
        button { class: "icon-btn", title: "Show sidebar (Ctrl+B)",
            onclick: move |_| app.ui.write().sidebar_open = true,
            Icon { i: I::PanelLeft, size: 16, stroke: 1.5 }
            if review > 0 {
                span { class: "badge", "{review}" }
            }
        }
        button { class: "icon-btn", title: "Search (Ctrl+K)",
            onclick: move |_| {
                let mut ui = app.ui.write();
                ui.palette_open = true;
                ui.palette_query.clear();
            },
            Icon { i: I::Search, size: 15, stroke: 1.5 }
        }
    }
}

/// Agent brand glyph tinted with the brand color (falls back to the text
/// color when the brand color would vanish on the current theme).
#[component]
pub fn AgentGlyph(kind: AgentKind, #[props(default = "")] class: &'static str) -> Element {
    let app = crate::state::use_app();
    let spec = kind.spec();
    let glyph = match &kind {
        AgentKind::Custom(id) => id.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or("◆".into()),
        _ => spec.glyph.to_string(),
    };
    let color = glyph_color(spec.color, app.ui.read().dark);
    let size = match class {
        "lg" => 22,
        "md" => 18,
        _ => 14,
    };
    if let Some(svg) = crate::logos::agent_logo(&kind) {
        return rsx! {
            span { class: "agent-glyph {class}", title: "{kind.display_name()}",
                crate::logos::Logo { svg, size }
            }
        };
    }
    rsx! {
        span { class: "agent-glyph {class}", style: "color: {color};", title: "{kind.display_name()}", "{glyph}" }
    }
}

/// Brand color usable on the current background.
pub fn glyph_color(hex: &str, dark: bool) -> String {
    let h = hex.trim_start_matches('#');
    let v = |i: usize| u8::from_str_radix(h.get(i..i + 2).unwrap_or("80"), 16).unwrap_or(128) as f32 / 255.0;
    let lum = 0.2126 * v(0) + 0.7152 * v(2) + 0.0722 * v(4);
    if (dark && lum < 0.25) || (!dark && lum > 0.85) {
        "var(--fg)".to_string()
    } else {
        hex.to_string()
    }
}

#[component]
pub fn Kbd(keys: Vec<&'static str>) -> Element {
    rsx! {
        span { class: "kbd",
            for k in keys {
                span { "{k}" }
            }
        }
    }
}

#[component]
pub fn Toggle(on: bool, onchange: EventHandler<bool>) -> Element {
    rsx! {
        button { class: if on { "toggle on" } else { "toggle" }, onclick: move |_| onchange.call(!on) }
    }
}

/// A dropdown anchored to its trigger.
#[component]
pub fn Dropdown(
    open: Signal<bool>,
    #[props(default = true)] up: bool,
    #[props(default = false)] right: bool,
    children: Element,
) -> Element {
    let mut open = open;
    if !open() {
        return rsx! {};
    }
    let cls = format!("menu {} {}", if up { "up" } else { "down" }, if right { "right" } else { "" });
    rsx! {
        div { class: "menu-backdrop", onclick: move |_| open.set(false) }
        div { class: "{cls}", onclick: move |e| e.stop_propagation(), {children} }
    }
}

#[component]
pub fn MenuItem(
    label: String,
    #[props(default)] sub: Option<String>,
    #[props(default)] icon: Option<I>,
    #[props(default = false)] checked: bool,
    #[props(default = false)] danger: bool,
    #[props(default = false)] disabled: bool,
    onclick: EventHandler<()>,
    #[props(default)] lead: Option<Element>,
) -> Element {
    let cls = format!("menu-item{}{}", if danger { " danger" } else { "" }, if disabled { " disabled" } else { "" });
    rsx! {
        button { class: "{cls}",
            onclick: move |e| {
                e.stop_propagation();
                if !disabled {
                    onclick.call(());
                }
            },
            if let Some(l) = lead { {l} }
            if let Some(i) = icon {
                span { class: "muted", Icon { i, size: 14 } }
            }
            span { class: "ellipsis", "{label}" }
            if let Some(s) = sub {
                span { class: "sub", "{s}" }
            }
            if checked {
                span { class: "check", Icon { i: I::Check, size: 13 } }
            }
        }
    }
}
