//! UI components.

mod appearance;
mod ask;
mod automations;
mod chat;
mod composer;
mod customize;
mod files;
mod home;
pub mod mentions;
mod menubar;
mod agent_models;
mod models;
mod overlays;
mod request;
mod right;
mod settings;
pub mod setup;
mod sidebar;
mod tasks;
mod terminal;
mod transcript;
pub(crate) mod usage;

use dioxus::prelude::*;
use larrier_core::AgentKind;

use crate::icons::{Icon, I};
use crate::state::{use_ui, App, SettingsTab, View};

pub use overlays::{ContextMenu, GlobalMenu, ModalHost, Palette, TabMenu, Toasts};

#[component]
pub fn Root() -> Element {
    let app = use_context_provider(App::new);

    crate::notify::track_window();
    crate::notify::use_tray(move || app.new_agent());

    use_hook(move || {
        let stale = crate::notify::interrupted_sessions();
        if !stale.is_empty() {
            crate::notify::mark_interrupted(&stale);
            app.refresh_sessions();
            if app.config.read().settings.continue_interrupted {
                let mut a = app;
                a.ui.write().interrupted = stale;
            }
        }
        crate::highlight::warm_up();
        crate::models_sync::start();
        crate::automations::start(app);
        app.detect_agents();
        crate::agent_models::start(app);
        // After an update, say once what's new.
        {
            let last = app.config.peek().settings.last_version.clone();
            let now = crate::version::VERSION;
            if last.as_deref() != Some(now) {
                if last.as_deref().map(|prev| crate::version::newer(now, prev)).unwrap_or(false) && crate::version::notes(now).is_some() {
                    let mut a = app;
                    a.ui.write().modal = Some(crate::state::Modal::WhatsNew(now.to_string(), true));
                }
                app.update_config(|c| c.settings.last_version = Some(now.to_string()));
            }
        }
        if app.config.read().settings.auto_import {
            app.sync_imports(true);
        }
        // Sessions other apps are writing update here as they change.
        crate::live::start(app);
        // Installed copies look for a newer release now and then.
        if matches!(crate::install::current(), crate::install::Kind::Installed(_)) {
            dioxus::core::spawn_forever(async move {
                tokio::time::sleep(std::time::Duration::from_secs(8)).await;
                loop {
                    if app.config.peek().settings.auto_update {
                        app.check_updates(true);
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(12 * 3600)).await;
                }
            });
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
            // Larrier's own tooltips instead of the system's: a hovered
            // element's title moves to data-tip and shows in a bubble.
            if (!window.__larrierTips) {
              window.__larrierTips = true;
              const tip = document.createElement('div');
              tip.className = 'l-tip';
              tip.setAttribute('role', 'tooltip');
              document.body.appendChild(tip);
              let target = null, timer = null;
              const hide = () => { clearTimeout(timer); timer = null; target = null; tip.classList.remove('on'); };
              const place = (el) => {
                const r = el.getBoundingClientRect();
                const t = tip.getBoundingClientRect();
                let x = Math.max(6, Math.min(r.left + r.width / 2 - t.width / 2, innerWidth - t.width - 6));
                let y = r.bottom + 6;
                if (y + t.height > innerHeight - 6) y = r.top - t.height - 6;
                tip.style.left = x + 'px';
                tip.style.top = y + 'px';
              };
              document.addEventListener('mouseover', (e) => {
                const el = e.target && e.target.closest ? e.target.closest('[title], [data-tip]') : null;
                if (!el) { if (target) hide(); return; }
                if (el.hasAttribute('title')) {
                  const t = el.getAttribute('title');
                  el.removeAttribute('title');
                  if (t) el.setAttribute('data-tip', t); else el.removeAttribute('data-tip');
                }
                if (el === target) return;
                hide();
                if (!el.getAttribute('data-tip')) return;
                target = el;
                timer = setTimeout(() => {
                  if (target !== el || !el.isConnected) return;
                  const root = document.querySelector('.larrier-root');
                  tip.classList.toggle('dark', !!root && root.getAttribute('data-theme') === 'dark');
                  tip.classList.toggle('still', !!root && root.classList.contains('no-motion'));
                  tip.textContent = el.getAttribute('data-tip');
                  tip.style.left = '0px';
                  tip.style.top = '0px';
                  place(el);
                  tip.classList.add('on');
                }, 450);
              }, true);
              document.addEventListener('mousedown', hide, true);
              document.addEventListener('keydown', hide, true);
              document.addEventListener('scroll', hide, true);
              window.addEventListener('blur', hide);
            }
            window.addEventListener('keydown', (e) => {
              // Keys typed into a terminal belong to the terminal.
              if (e.target && e.target.closest && e.target.closest('.xterm')) return;
              const mod = e.ctrlKey || e.metaKey;
              const k = (e.key || '').toLowerCase();
              if (k === 'escape') {
                // The prompt editor handles its own Escape.
                if (!(e.target && e.target.closest && e.target.closest('.user-msg.editing'))) dioxus.send({ t: 'key', k: 'escape', shift: false });
                return;
              }
              if (e.altKey && (k === 'arrowleft' || k === 'arrowright')) { e.preventDefault(); dioxus.send({ t: 'nav', d: k === 'arrowleft' ? -1 : 1 }); return; }
              if (!mod) return;
              if (['k','n','b','j',',','l','p','r','w','e','g','i','.','/','backspace','=','+','-','0','tab'].includes(k)) {
                e.preventDefault();
                dioxus.send({ t: 'key', k, shift: e.shiftKey, alt: e.altKey });
              }
            }, true);
            // Paste images / files / long text into the composer.
            document.addEventListener('paste', (e) => {
              const t = e.target;
              if (!t || !t.closest) return;
              // Into the composer, or into a sent prompt being edited.
              const edit = !!t.closest('.user-msg.editing');
              if (!edit && !t.closest('.composer')) return;
              const cd = e.clipboardData; if (!cd) return;
              const files = [...cd.items].filter(i => i.kind === 'file').map(i => i.getAsFile()).filter(Boolean);
              if (files.length) {
                e.preventDefault();
                for (const f of files) {
                  if (f.size > 25 * 1024 * 1024) continue;
                  const r = new FileReader();
                  r.onload = () => dioxus.send({ t: 'paste-file', name: f.name || 'pasted', data: r.result, edit });
                  r.readAsDataURL(f);
                }
                return;
              }
              const text = cd.getData('text/plain') || '';
              if (text.length > 2500 || text.split('\n').length > 40) {
                e.preventDefault();
                dioxus.send({ t: 'paste-text', text });
              }
            }, true);
            // Window width for the narrow layout.
            let sizeT = null;
            const sendSize = () => dioxus.send({ t: 'size', w: window.innerWidth });
            sendSize();
            window.addEventListener('resize', () => { clearTimeout(sizeT); sizeT = setTimeout(sendSize, 80); });
            // Mouse back / forward buttons.
            window.addEventListener('mouseup', (e) => {
              if (e.button === 3 || e.button === 4) { e.preventDefault(); dioxus.send({ t: 'nav', d: e.button === 3 ? -1 : 1 }); }
            });
            // Larrier's own right-click menu everywhere (never the browser's).
            window.addEventListener('contextmenu', (e) => {
              e.preventDefault();
              let t = e.target instanceof Element ? e.target : document.body;
              let later = false;
              // Right-clicking while a menu is open: that menu closes (its
              // backdrop handles that) and the click goes to what's underneath.
              if (t.closest('.menu-backdrop')) {
                const under = document.elementsFromPoint(e.clientX, e.clientY).find(el => !el.closest('.menu-backdrop, .menu'));
                if (!under) return;
                if (under.closest('[data-ctx]')) {
                  const x = e.clientX, y = e.clientY;
                  setTimeout(() => under.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: x, clientY: y, button: 2 })), 0);
                  return;
                }
                t = under; later = true;
              }
              if (t.closest('.menu')) return;
              if (t.closest('[data-ctx]')) return; // components with their own menu
              const ed = t.closest('input, textarea, [contenteditable="true"]');
              window.__larrierCtx = ed || null;
              let sel = '';
              if (ed && typeof ed.selectionStart === 'number') sel = ed.value.substring(ed.selectionStart, ed.selectionEnd);
              else sel = String(window.getSelection() || '');
              const a = t.closest('a[href]');
              const pre = t.closest('pre');
              const pathEl = t.closest('[data-path]');
              const msg = { t: 'ctx', x: e.clientX, y: e.clientY, editable: !!ed, selection: sel.slice(0, 200000),
                link: a ? a.href : null, code: pre ? pre.innerText.slice(0, 500000) : null,
                path: pathEl ? pathEl.getAttribute('data-path') : null, image: t.tagName === 'IMG' ? t.src : null };
              if (later) setTimeout(() => dioxus.send(msg), 0); else dioxus.send(msg);
            }, true);
            // Drag a chat (sidebar row or tab) onto a drop target. Pointer
            // based, so it works alongside the webview's file drop handling.
            (() => {
              let st = null;
              document.addEventListener('dragstart', (e) => { if (e.target.closest && e.target.closest('[data-drag-session]')) e.preventDefault(); }, true);
              document.addEventListener('pointerdown', (e) => {
                if (e.button !== 0 || !(e.target instanceof Element)) return;
                const src = e.target.closest('[data-drag-session]');
                if (!src || !src.getAttribute('data-drag-session')) return;
                const btn = e.target.closest('button');
                if (btn && btn !== src && src.contains(btn)) return;
                st = { id: src.getAttribute('data-drag-session'), title: src.getAttribute('data-drag-title') || '', x0: e.clientX, y0: e.clientY, active: false, zone: null, ghost: null };
              }, true);
              // Where a drop at (x, y) lands. Over a chat window the pointer's
              // place decides: near a composer attaches as context, an edge
              // opens side by side, anywhere else opens it here.
              const inside = (b, x, y, mx, my) => x >= b.left - mx && x <= b.right + mx && y >= b.top - my && y <= b.bottom + my;
              const zoneAt = (x, y) => {
                const ov = [...document.querySelectorAll('.drop-overlay')].find(o => inside(o.getBoundingClientRect(), x, y, 0, 0));
                if (!ov) {
                  const under = document.elementFromPoint(x, y);
                  const z = under && under.closest('[data-drop]');
                  return z ? { el: z, zone: z.getAttribute('data-drop'), target: z.getAttribute('data-drop-target') || '' } : null;
                }
                const r = ov.getBoundingClientRect();
                const scope = ov.parentElement || document;
                const comp = [...scope.querySelectorAll('.composer')].find(c => inside(c.getBoundingClientRect(), x, y, 28, 40));
                if (comp) {
                  const b = comp.getBoundingClientRect();
                  ov.style.setProperty('--cx', (b.left - r.left - 8) + 'px');
                  ov.style.setProperty('--cy', (b.top - r.top - 8) + 'px');
                  ov.style.setProperty('--cw', (b.width + 16) + 'px');
                  ov.style.setProperty('--ch', (b.height + 16) + 'px');
                  const col = comp.closest('[data-session]');
                  return { el: ov, zone: 'context', target: col ? col.getAttribute('data-session') : (ov.getAttribute('data-drop-target') || '') };
                }
                let zone = 'here';
                if (ov.classList.contains('can-split')) {
                  const edge = Math.min(Math.max(120, r.width * 0.26), r.width * 0.4);
                  if (x < r.left + edge) zone = 'split-left';
                  else if (x > r.right - edge) zone = 'split';
                }
                return { el: ov, zone, target: ov.getAttribute('data-drop-target') || '' };
              };
              const mark = (z) => {
                const prev = st.zone;
                if (prev && (!z || prev.el !== z.el)) {
                  prev.el.classList.remove('over');
                  prev.el.removeAttribute('data-zone');
                }
                if (z) {
                  z.el.classList.add('over');
                  z.el.setAttribute('data-zone', z.zone);
                }
                st.zone = z;
              };
              window.addEventListener('pointermove', (e) => {
                if (!st) return;
                if (!st.active) {
                  if (Math.hypot(e.clientX - st.x0, e.clientY - st.y0) < 6) return;
                  st.active = true;
                  const g = document.createElement('div');
                  g.className = 'drag-ghost'; g.textContent = st.title || 'Chat';
                  (document.querySelector('.app') || document.body).appendChild(g); st.ghost = g;
                  document.documentElement.classList.add('dragging-chat');
                  window.getSelection()?.removeAllRanges();
                  dioxus.send({ t: 'drag', id: st.id });
                }
                st.ghost.style.transform = 'translate(' + (e.clientX + 14) + 'px,' + (e.clientY + 12) + 'px)';
                mark(zoneAt(e.clientX, e.clientY));
              }, true);
              const end = (cancel) => {
                if (!st) return;
                const s = st; st = null;
                if (!s.active) return;
                s.ghost && s.ghost.remove();
                document.documentElement.classList.remove('dragging-chat');
                if (s.zone) { s.zone.el.classList.remove('over'); s.zone.el.removeAttribute('data-zone'); }
                const z = cancel ? null : s.zone;
                dioxus.send({ t: 'drop', id: s.id, zone: z ? z.zone : null, target: z ? z.target : '' });
                // Swallow the click that ends a drag.
                const swallow = (ev) => { ev.stopPropagation(); ev.preventDefault(); };
                window.addEventListener('click', swallow, { capture: true, once: true });
                setTimeout(() => window.removeEventListener('click', swallow, true), 60);
              };
              window.addEventListener('pointerup', () => end(false), true);
              window.addEventListener('pointercancel', () => end(true), true);
              window.addEventListener('blur', () => end(true));
              window.addEventListener('keydown', (e) => { if (e.key === 'Escape' && st && st.active) { e.stopPropagation(); end(true); } }, true);
            })();
            // Drag a project (its header row) into another place in the
            // sidebar: it lifts and follows the pointer, a line shows where
            // it lands, and on drop the projects slide into their new order.
            (() => {
              let pd = null;
              const groups = () => [...document.querySelectorAll('[data-project]')].filter(g => g.getAttribute('data-project'));
              document.addEventListener('pointerdown', (e) => {
                if (e.button !== 0 || !(e.target instanceof Element)) return;
                const row = e.target.closest('[data-drag-project]');
                if (!row || !row.getAttribute('data-drag-project') || e.target.closest('button')) return;
                pd = { key: row.getAttribute('data-drag-project'), x0: e.clientX, y0: e.clientY, active: false, group: row.closest('[data-project]'), line: null, before: undefined };
              }, true);
              window.addEventListener('pointermove', (e) => {
                if (!pd) return;
                if (!pd.active) {
                  if (Math.hypot(e.clientX - pd.x0, e.clientY - pd.y0) < 6 || !pd.group) return;
                  pd.active = true;
                  pd.group.classList.add('project-lifted');
                  document.documentElement.classList.add('dragging-project');
                  pd.line = document.createElement('div');
                  pd.line.className = 'project-drop-line';
                  document.body.appendChild(pd.line);
                  window.getSelection()?.removeAllRanges();
                }
                pd.group.style.transform = 'translateY(' + (e.clientY - pd.y0) + 'px)';
                // Lands before the first project whose header is below the pointer.
                const others = groups().filter(g => g !== pd.group);
                pd.before = null;
                for (const g of others) {
                  const h = (g.querySelector('[data-drag-project]') || g).getBoundingClientRect();
                  if (e.clientY < h.top + h.height / 2) { pd.before = g; break; }
                }
                const ref = pd.before || others[others.length - 1];
                if (ref) {
                  const r = ref.getBoundingClientRect();
                  const y = pd.before ? r.top - 1 : r.bottom + 1;
                  Object.assign(pd.line.style, { left: (r.left + 8) + 'px', width: Math.max(0, r.width - 16) + 'px', top: y + 'px' });
                }
              }, true);
              const done = (cancel) => {
                if (!pd) return;
                const d = pd; pd = null;
                if (!d.active) return;
                d.line && d.line.remove();
                document.documentElement.classList.remove('dragging-project');
                d.group.classList.remove('project-lifted');
                const all = groups();
                const first = new Map(all.map(g => [g.getAttribute('data-project'), g.getBoundingClientRect().top]));
                const keys = all.filter(g => g !== d.group).map(g => g.getAttribute('data-project'));
                const at = d.before ? keys.indexOf(d.before.getAttribute('data-project')) : keys.length;
                keys.splice(at < 0 ? keys.length : at, 0, d.key);
                if (!cancel) dioxus.send({ t: 'project-order', order: keys });
                // Once the sidebar shows the new order, each project slides
                // from where it was into its place.
                const want = cancel ? all.map(g => g.getAttribute('data-project')).join('|') : keys.join('|');
                let tries = 0;
                const flip = () => {
                  const now = groups();
                  if (now.map(g => g.getAttribute('data-project')).join('|') !== want && tries++ < 30) { requestAnimationFrame(flip); return; }
                  d.group.style.transition = 'none';
                  d.group.style.transform = '';
                  for (const g of now) {
                    const f = first.get(g.getAttribute('data-project'));
                    if (f == null) continue;
                    const dy = f - g.getBoundingClientRect().top;
                    if (!dy) continue;
                    g.style.transition = 'none';
                    g.style.transform = 'translateY(' + dy + 'px)';
                    g.getBoundingClientRect();
                    g.style.transition = 'transform .3s cubic-bezier(.2, .8, .2, 1)';
                    g.style.transform = '';
                    g.addEventListener('transitionend', () => { g.style.transition = ''; }, { once: true });
                  }
                };
                requestAnimationFrame(flip);
                const swallow = (ev) => { ev.stopPropagation(); ev.preventDefault(); };
                window.addEventListener('click', swallow, { capture: true, once: true });
                setTimeout(() => window.removeEventListener('click', swallow, true), 60);
              };
              window.addEventListener('pointerup', () => done(false), true);
              window.addEventListener('pointercancel', () => done(true), true);
              window.addEventListener('blur', () => done(true));
              window.addEventListener('keydown', (e) => { if (e.key === 'Escape' && pd && pd.active) { e.stopPropagation(); done(true); } }, true);
            })();
            "#,
        );
        while let Ok(v) = ev.recv::<serde_json::Value>().await {
            handle_bridge(app, v);
        }
    });

    // Interface zoom follows the setting.
    let zoom_level = app.config.read().settings.zoom;
    use_effect(use_reactive!(|zoom_level| {
        dioxus::desktop::window().set_zoom_level(zoom_level.clamp(0.5, 2.0));
    }));

    let dark = use_ui(|u| u.dark)();
    let narrow = use_ui(|u| u.win_w < NARROW)();
    let sidebar_open = use_ui(|u| u.sidebar_open)();
    let lightbox = use_ui(|u| u.lightbox.clone())();
    let cfg = app.config.read();
    let st = &cfg.settings;
    let theme = if dark { "dark" } else { "light" };
    let mut style = format!("--sidebar-w: {}px; --chat-w: {}px; --tw: {}px; {}", st.sidebar_width, st.chat_width, st.transcript_width.clamp(560, 1600), text_size_vars(st));
    if !st.accent.trim().is_empty() {
        style.push_str(&format!(" --accent-c: {};", st.accent.trim()));
    }
    if let Some(f) = st.ui_font.as_ref().filter(|f| !f.trim().is_empty()) {
        style.push_str(&format!(" --font: '{}', system-ui, sans-serif;", f.trim().replace('\'', "")));
    }
    if let Some(f) = st.mono_font.as_ref().filter(|f| !f.trim().is_empty()) {
        style.push_str(&format!(" --mono: '{}', ui-monospace, monospace;", f.trim().replace('\'', "")));
    }
    for (k, v) in &st.custom_colors {
        if let Some(var) = k.strip_prefix(&format!("{theme}.")) {
            if !v.trim().is_empty() && var.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                style.push_str(&format!(" --{var}: {};", v.trim()));
            }
        }
    }
    let mut class = String::from("app");
    if st.density == larrier_core::Density::Compact {
        class.push_str(" compact");
    }
    if st.sidebar_right {
        class.push_str(" sidebar-right");
    }
    if st.reduce_motion {
        class.push_str(" no-motion");
    }
    match st.button_corners {
        larrier_core::ButtonCorners::Solid => class.push_str(" corners-solid"),
        larrier_core::ButtonCorners::Rounded => class.push_str(" corners-rounded"),
        larrier_core::ButtonCorners::Balanced => {}
    }
    if narrow {
        class.push_str(" narrow");
    }
    let code_themes = (st.code_theme_light.clone(), st.code_theme_dark.clone());
    drop(cfg);

    let in_settings = matches!(&*app.view.read(), View::Settings(_));
    if !sidebar_open && !in_settings {
        class.push_str(" side-closed");
    }
    // Not an inline `style`: Dioxus restores inline properties a new value
    // leaves out, so removed colors (the Larrier preset, Reset) would stick.
    let root_css = format!(".app.larrier-root[data-theme] {{ {style} }}");
    rsx! {
        div { class: "{class} larrier-root", "data-theme": "{theme}",
            style { "{root_css}" }
            crate::icons::IconSprite {}
            CodeStyles { light: code_themes.0.clone(), dark: code_themes.1.clone() }
            menubar::MenuBar {}
            div { class: "app-body",
                if in_settings {
                    settings::SettingsNav {}
                }
                // Always mounted: it slides in and out, and keeps its scroll
                // and open groups while Settings is shown.
                sidebar::Sidebar { parked: in_settings }
                if narrow && sidebar_open && !in_settings {
                    div { class: "narrow-backdrop", onclick: move |_| { let mut a = app; a.ui.write().sidebar_open = false; } }
                }
                Main {}
                if !in_settings {
                    SideToggle {}
                }
            }
            Palette {}
            ModalHost {}
            ContextMenu {}
            TabMenu {}
            GlobalMenu {}
            Toasts {}
            if let Some(src) = lightbox {
                div { class: "lightbox", onclick: move |_| { let mut a = app; a.ui.write().lightbox = None; },
                    img { src: "{src}" }
                }
            }
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
                    let editing = app.editing.peek().clone();
                    let (menus, palette, modal) = {
                        let ui = app.ui.peek();
                        (ui.gctx.is_some() || ui.ctx.is_some() || ui.menubar.is_some() || ui.tab_ctx.is_some(), ui.palette_open, ui.modal.is_some())
                    };
                    if menus {
                        let mut ui = app.ui.write();
                        ui.gctx = None;
                        ui.ctx = None;
                        ui.menubar = None;
                        ui.tab_ctx = None;
                    } else if editing.msg.is_some() {
                        if editing.confirm.is_some() {
                            app.editing.write().confirm = None;
                        } else {
                            app.request_edit(None);
                        }
                    } else if palette {
                        app.ui.write().palette_open = false;
                    } else if modal {
                        app.ui.write().modal = None;
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
                ("w", false) => {
                    let v = app.view.read().clone();
                    if v.is_tab() {
                        app.close_tab(&v);
                    }
                }
                ("tab", back) => {
                    let tabs = app.window_tabs();
                    if !tabs.is_empty() {
                        let v = app.view.read().clone();
                        let i = tabs.iter().position(|t| t.view == v);
                        let n = tabs.len() as isize;
                        let next = match i {
                            Some(i) => (i as isize + if back { -1 } else { 1 }).rem_euclid(n) as usize,
                            None => 0,
                        };
                        app.navigate(tabs[next].view.clone());
                    }
                }
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
        Some("drag") => app.drag.set(v["id"].as_str().map(|s| s.to_string())),
        // Projects dragged into a new order; ones not shown keep their place
        // after them.
        Some("project-order") => {
            let shown: Vec<String> = v["order"].as_array().map(|a| a.iter().filter_map(|k| k.as_str().map(String::from)).collect()).unwrap_or_default();
            if !shown.is_empty() {
                app.update_config(|c| {
                    let rest: Vec<String> = c.settings.project_order.iter().filter(|k| !shown.contains(k)).cloned().collect();
                    c.settings.project_order = shown.into_iter().chain(rest).collect();
                });
            }
        }
        Some("drop") => {
            app.drag.set(None);
            let id = v["id"].as_str().unwrap_or("").to_string();
            if id.is_empty() {
                return;
            }
            let target = v["target"].as_str().map(|s| s.to_string()).filter(|s| !s.is_empty());
            match v["zone"].as_str() {
                Some("here") => app.open_session(&id),
                // Onto the tab bar: next to the open chat, never inside it.
                Some("split") | Some("tabstrip") => app.open_split(&id),
                Some("split-left") => app.open_split_left(&id),
                Some("context") => {
                    let short = id[id.len().saturating_sub(8)..].to_string();
                    app.ui.write().compose_inbox.push((target, format!("@agent:{short}")));
                }
                _ => {}
            }
        }
        Some("paste-file") => {
            let data = v["data"].as_str().unwrap_or("");
            let name = v["name"].as_str().unwrap_or("pasted");
            match crate::state::save_data_url(data, name) {
                Ok(part) if v["edit"].as_bool() == Some(true) && app.editing.peek().msg.is_some() => app.editing.write().attachments.push(part),
                Ok(part) => app.ui.write().attach_inbox.push(part),
                Err(e) => app.toast_err(format!("Could not attach: {e}")),
            }
        }
        Some("paste-text") => {
            let text = v["text"].as_str().unwrap_or("").to_string();
            let lines = text.lines().count();
            app.ui.write().attach_inbox.push(larrier_core::Part::File {
                path: format!("Pasted text · {lines} lines"),
                mime: Some("text/plain".into()),
                text: Some(text),
            });
        }
        Some("size") => {
            let w = v["w"].as_f64().unwrap_or(1400.0);
            let old = app.ui.peek().win_w;
            if (old - w).abs() < 0.5 {
                return;
            }
            let was_narrow = old < NARROW;
            app.ui.write().win_w = w;
            // Entering the narrow layout closes the overlay sidebar.
            if w < NARROW && !was_narrow {
                app.ui.write().sidebar_open = false;
            }
        }
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

/// Syntax-highlight + code-block styles for the chosen code themes.
#[component]
fn CodeStyles(light: Option<String>, dark: Option<String>) -> Element {
    let css = use_memo(use_reactive!(|light, dark| {
        format!("{}\n{}", crate::highlight::css_for(light.as_deref(), dark.as_deref()), crate::markdown::CSS)
    }));
    rsx! { style { "{css}" } }
}

/// A number with − / + buttons. It can also be typed (Enter or leaving the
/// field applies it) or nudged with the arrow keys; it stays within
/// `min`..=`max`.
#[component]
pub fn Stepper(value: u32, min: u32, max: u32, #[props(default = 1)] step: u32, #[props(default = "px")] unit: &'static str, onchange: EventHandler<u32>) -> Element {
    let set = move |v: i64| onchange.call(v.clamp(min as i64, max as i64) as u32);
    let (v, s) = (value as i64, step as i64);
    rsx! {
        div { class: "stepper",
            button { class: "st-btn", disabled: value <= min, title: "Smaller", "aria-label": "Decrease",
                onclick: move |_| set(v - s),
                crate::icons::Lucide { name: "minus".to_string(), size: 13, stroke: 2.0 }
            }
            label { class: "st-val",
                input {
                    r#type: "text",
                    inputmode: "numeric",
                    value: "{value}",
                    onchange: move |e| {
                        if let Ok(n) = e.value().trim().trim_end_matches(unit).trim().parse::<i64>() {
                            set(n);
                        }
                    },
                    onkeydown: move |e| match e.key() {
                        Key::ArrowUp => { e.prevent_default(); set(v + s) }
                        Key::ArrowDown => { e.prevent_default(); set(v - s) }
                        _ => {}
                    },
                }
                if !unit.is_empty() { span { class: "st-unit", "{unit}" } }
            }
            button { class: "st-btn", disabled: value >= max, title: "Larger", "aria-label": "Increase",
                onclick: move |_| set(v + s),
                crate::icons::Lucide { name: "plus".to_string(), size: 13, stroke: 2.0 }
            }
        }
    }
}

/// Sidebar width that suits a window `win_w` CSS px wide (double-click on
/// its edge).
pub fn auto_sidebar_width(win_w: f64) -> u32 {
    (win_w * 0.2).round().clamp(240.0, 320.0) as u32
}

/// Chat column width beside the side panel that suits the window: a little
/// over half of what the sidebar leaves, so both stay usable.
pub fn auto_chat_width(win_w: f64, sidebar: Option<u32>) -> u32 {
    let room = win_w - sidebar.map(|w| w as f64).unwrap_or(0.0);
    (room * 0.54).round().clamp(420.0, (room - 360.0).max(420.0)).min(1000.0) as u32
}

/// Whether a press on a resize handle is the second of a double-click
/// (the drag overlay swallows the browser's own dblclick).
pub fn second_press(last: &mut Option<std::time::Instant>) -> bool {
    let now = std::time::Instant::now();
    let double = last.map(|t| now.duration_since(t) < std::time::Duration::from_millis(350)).unwrap_or(false);
    *last = if double { None } else { Some(now) };
    double
}

/// CSS variables for the text sizes (Settings → Appearance → Text).
pub fn text_size_vars(s: &larrier_core::Settings) -> String {
    let t = &s.text_sizes;
    format!(
        "--fs: {}px; --fs-prompt: {}px; --fs-composer: {}px; --fs-steps: {}px; --fs-code: {}px; --fs-side: {}px; --fs-term: {}px;",
        s.font_size, t.prompts, t.composer, t.steps, t.code, t.sidebar, t.terminal
    )
}

/// Below this window width the sidebar floats over the content.
pub const NARROW: f64 = 760.0;

/// Open the review pane on a tool tab (Changes, Browser, Terminal, Files).
pub fn open_tool(mut app: App, tab: crate::state::RightTab) {
    app.ui.write().open_panel(tab, false);
}

/// Ctrl + / Ctrl - / Ctrl 0 interface zoom (persisted in settings).
pub fn zoom(app: App, delta: f64) {
    let cur = app.config.read().settings.zoom;
    let z = if delta == 0.0 { 1.0 } else { ((cur + delta) * 10.0).round() / 10.0 };
    app.update_config(|c| c.settings.zoom = z.clamp(0.5, 2.0));
}

#[component]
fn Main() -> Element {
    let app = crate::state::use_app();
    let view = app.view.read().clone();
    // A session and its subagent tabs are one view: switching tabs swaps the
    // transcript and keeps the header and side panel as they are.
    let chat = match &view {
        View::Agent(id) => Some((id.clone(), None)),
        View::Subagent { session, tool } => Some((session.clone(), Some(tool.clone()))),
        _ => None,
    };
    rsx! {
        div { class: "main",
            InterruptedBanner {}
            if let Some((id, sub)) = chat {
                chat::AgentView { key: "{id}", id, sub }
            } else {
                match view {
                    View::Home => rsx! { home::Home {} },
                    View::Settings(tab) => rsx! { settings::SettingsView { tab } },
                    View::Automations => rsx! { PageFrame { automations::AutomationsView {} } },
                    View::Customize => rsx! { PageFrame { customize::CustomizeView {} } },
                    View::Agent(_) | View::Subagent { .. } => rsx! {},
                }
            }
        }
    }
}

/// Offer to continue agents that were running when Larrier closed.
#[component]
fn InterruptedBanner() -> Element {
    let mut app = crate::state::use_app();
    let ids = app.ui.read().interrupted.clone();
    if ids.is_empty() {
        return rsx! {};
    }
    let n = ids.len();
    let ids2 = ids.clone();
    rsx! {
        div { style: "display: flex; align-items: center; gap: 10px; margin: 8px 12px 0; padding: 8px 12px; border-radius: 10px; background: var(--amber-bg); color: var(--fg); font-size: 12.5px; animation: rise-in .2s ease-out;",
            Icon { i: I::Alert, size: 14 }
            span { style: "flex: 1;", if n == 1 { "1 agent was interrupted when Larrier closed." } else { "{n} agents were interrupted when Larrier closed." } }
            if n == 1 {
                button { class: "ghost-btn", onclick: { let id = ids[0].clone(); move |_| app.open_session(&id) }, "Open" }
            }
            button { class: "primary-btn",
                onclick: move |_| {
                    for id in ids2.iter() {
                        app.send(Some(id.clone()), crate::notify::CONTINUE_PROMPT.to_string(), Vec::new());
                    }
                    app.ui.write().interrupted.clear();
                },
                if n == 1 { "Continue" } else { "Continue all" }
            }
            button { class: "icon-btn sm", title: "Dismiss", onclick: move |_| app.ui.write().interrupted.clear(), Icon { i: I::X, size: 12 } }
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

/// Empty space in a header that drags the window; double-click maximizes
/// or restores (like a native title bar).
#[component]
pub fn DragRegion() -> Element {
    let mut app = crate::state::use_app();
    let window = dioxus::desktop::use_window();
    rsx! {
        div { class: "drag",
            onmousedown: move |e| {
                if e.trigger_button() != Some(dioxus::html::input_data::MouseButton::Primary) {
                    return;
                }
                // The native move loop eats the second click of a double
                // click, so time the presses here.
                use std::sync::Mutex;
                static LAST: Mutex<Option<(std::time::Instant, f64, f64)>> = Mutex::new(None);
                let p = e.screen_coordinates();
                let now = std::time::Instant::now();
                let double = {
                    let mut last = LAST.lock().unwrap();
                    let d = matches!(*last, Some((t, x, y)) if now.duration_since(t).as_millis() < 450 && (x - p.x).abs() < 6.0 && (y - p.y).abs() < 6.0);
                    *last = if d { None } else { Some((now, p.x, p.y)) };
                    d
                };
                if double {
                    window.toggle_maximized();
                    app.ui.write().maximized = window.is_maximized();
                } else {
                    window.drag();
                }
            },
        }
    }
}

/// Copy button that turns into a check for a moment — no toast.
#[component]
pub fn CopyButton(text: String, #[props(default = 14)] size: u32, #[props(default = "Copy")] title: &'static str) -> Element {
    let app = crate::state::use_app();
    let mut done = use_signal(|| false);
    rsx! {
        button { class: if done() { "icon-btn copied" } else { "icon-btn" }, title: if done() { "Copied" } else { title },
            onclick: move |e| {
                e.stop_propagation();
                app.copy_text(&text);
                done.set(true);
                spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(1400)).await;
                    done.set(false);
                });
            },
            if done() { Icon { i: I::Check, size, stroke: 1.8 } } else { Icon { i: I::Copy, size, stroke: 1.5 } }
        }
    }
}

/// The sidebar's toggle: one button that stays where it is whether the
/// sidebar is open or not (headers leave room for it).
#[component]
fn SideToggle() -> Element {
    let mut app = crate::state::use_app();
    let open = app.ui.read().sidebar_open;
    let review = app
        .sessions
        .read()
        .iter()
        .filter(|s| !s.archived && s.unread && matches!(s.status, larrier_core::SessionStatus::Review | larrier_core::SessionStatus::Done))
        .count();
    rsx! {
        button { class: if open { "icon-btn pane-toggle side on" } else { "icon-btn pane-toggle side" },
            title: if open { "Hide sidebar (Ctrl+B)" } else { "Show sidebar (Ctrl+B)" },
            onclick: move |_| { let o = !app.ui.read().sidebar_open; app.ui.write().sidebar_open = o; },
            Icon { i: I::PanelLeft, size: 16, stroke: 1.5 }
            if !open && review > 0 {
                span { class: "badge", "{review}" }
            }
        }
    }
}

/// Search, in pane headers while the sidebar (which has its own) is hidden.
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
    let _ = review;
    if open {
        return rsx! {};
    }
    rsx! {
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

/// The running indicator: an accent arc that sweeps and breathes around a
/// faint ring. With Reduce Motion it only turns, slowly.
#[component]
pub fn Spinner(#[props(default = 14)] size: u32) -> Element {
    rsx! {
        svg { class: "spin", width: "{size}", height: "{size}", view_box: "0 0 16 16", role: "img", "aria-label": "Running",
            circle { class: "spin-track", cx: "8", cy: "8", r: "6", fill: "none", stroke_width: "2" }
            circle { class: "spin-arc", cx: "8", cy: "8", r: "6", fill: "none", stroke_width: "2", stroke_linecap: "round", "pathLength": "100" }
        }
    }
}

/// Children that open and close by growing and shrinking (and fading).
/// They stay mounted until the close has played out, then leave the page.
#[component]
pub fn Collapse(open: bool, children: Element) -> Element {
    let mut mounted = use_signal(|| open);
    let mut turn = use_signal(|| 0u32);
    use_effect(use_reactive!(|open| {
        let t = turn.peek().wrapping_add(1);
        turn.set(t);
        if open {
            mounted.set(true);
        } else if *mounted.peek() {
            spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(280)).await;
                if *turn.peek() == t {
                    mounted.set(false);
                }
            });
        }
    }));
    if !open && !mounted() {
        return rsx! {};
    }
    rsx! {
        div { class: if open { "collapse open" } else { "collapse" },
            div { class: "collapse-inner", {children} }
        }
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

/// A dropdown anchored to its trigger (the enclosing `.menu-anchor`).
/// Positioned with `position: fixed` against the window: it flips above or
/// below to whichever side has room, clamps to the window edges and caps its
/// height, so menus never get cut off by scroll containers or the window.
/// Arrow keys, Home / End, type-ahead, Enter and Escape work inside it.
#[component]
pub fn Dropdown(
    open: Signal<bool>,
    #[props(default = true)] up: bool,
    #[props(default = false)] right: bool,
    #[props(default)] class: Option<String>,
    children: Element,
) -> Element {
    let mut open = open;
    let id = use_hook(|| format!("dd-{}", crate::state::rand_id()));
    let is_open = open();
    let id2 = id.clone();
    use_effect(use_reactive!(|is_open| {
        if is_open {
            document::eval(&place_js(&id2, up, right));
        }
    }));
    if !is_open {
        return rsx! {};
    }
    let cls = format!("menu {} {} {}", if up { "up" } else { "down" }, if right { "right" } else { "" }, class.unwrap_or_default());
    rsx! {
        div { class: "menu-backdrop", onclick: move |_| open.set(false), oncontextmenu: move |e| { e.prevent_default(); open.set(false); } }
        div { id: "{id}", class: "{cls}", style: "visibility: hidden;", role: "menu", onclick: move |e| e.stop_propagation(), {children} }
    }
}

/// JS that positions a menu next to its anchor inside the window and wires
/// its keyboard handling.
pub fn place_js(id: &str, prefer_up: bool, align_right: bool) -> String {
    format!(
        r#"requestAnimationFrame(() => {{
  const m = document.getElementById('{id}'); if (!m) return;
  const anchor = m.parentElement; if (!anchor) return;
  const focusFirst = true;
  const a = anchor.getBoundingClientRect();
  const vw = window.innerWidth, vh = window.innerHeight, pad = 8, gap = 4;
  const cap = parseFloat(getComputedStyle(m).getPropertyValue('--menu-max')) || 440;
  m.style.position = 'fixed'; m.style.bottom = 'auto'; m.style.right = 'auto'; m.style.maxHeight = 'none';
  m.style.maxWidth = (vw - pad * 2) + 'px';
  if (m.classList.contains('select-menu')) m.style.minWidth = Math.min(a.width, vw - pad * 2) + 'px';
  const mw = Math.min(m.offsetWidth, vw - pad * 2), mh = Math.min(m.scrollHeight, cap);
  const below = vh - a.bottom - pad - gap, above = a.top - pad - gap;
  const need = Math.min(mh, 260);
  let up = {prefer_up} ? (above >= need || above > below) : (below < need && above > below);
  const room = Math.max(120, up ? above : below);
  const h = Math.min(mh, room);
  const top = up ? Math.max(pad, a.top - gap - h) : Math.min(a.bottom + gap, vh - pad - h);
  let left = {align_right} ? a.right - mw : a.left;
  left = Math.max(pad, Math.min(left, vw - mw - pad));
  m.style.left = left + 'px'; m.style.top = top + 'px'; m.style.maxHeight = Math.min(cap, room) + 'px';
  m.style.transformOrigin = (up ? 'bottom ' : 'top ') + ({align_right} ? 'right' : 'left');
  m.classList.toggle('flip-up', up);
  m.style.visibility = 'visible'; m.classList.add('in');
  {keys}
}});"#,
        keys = MENU_KEYS_JS
    )
}

/// JS that places a menu opened at a point (right-click) fully inside the
/// window: it flips left / up when it would overflow and caps its height.
/// `focus` moves keyboard focus into the menu (not for edit menus, which
/// must leave focus in the text field they act on).
pub fn place_at_js(id: &str, x: f64, y: f64, focus: bool) -> String {
    format!(
        r#"requestAnimationFrame(() => {{
  const m = document.getElementById('{id}'); if (!m) return;
  const vw = window.innerWidth, vh = window.innerHeight, pad = 8, x = {x}, y = {y};
  const focusFirst = {focus};
  m.style.position = 'fixed'; m.style.maxHeight = (vh - pad * 2) + 'px'; m.style.maxWidth = (vw - pad * 2) + 'px';
  const w = m.offsetWidth, h = Math.min(m.scrollHeight, vh - pad * 2);
  let left = x + 2, top = y + 2;
  if (left + w > vw - pad) left = x - w - 2;
  if (top + h > vh - pad) top = y - h - 2;
  left = Math.max(pad, Math.min(left, vw - w - pad));
  top = Math.max(pad, Math.min(top, vh - h - pad));
  m.style.left = left + 'px'; m.style.top = top + 'px';
  m.style.transformOrigin = (top < y ? 'bottom ' : 'top ') + (left < x ? 'right' : 'left');
  m.style.visibility = 'visible'; m.classList.add('in');
  {keys}
}});"#,
        keys = MENU_KEYS_JS
    )
}

/// Keyboard handling shared by every menu (expects `m` and `focusFirst`).
const MENU_KEYS_JS: &str = r#"const items = () => [...m.querySelectorAll('button.menu-item:not(.disabled), button.mp-agent')].filter(b => b.offsetParent !== null);
  const checked = m.querySelector('.menu-item.checked, .menu-item .check');
  if (checked) (checked.closest('.menu-item') || checked).scrollIntoView({ block: 'nearest' });
  const s = m.querySelector('.menu-search input, input[autofocus]');
  if (s) s.focus({ preventScroll: true });
  else if (focusFirst) { const f = (checked && checked.closest('.menu-item')) || items()[0]; if (f) f.focus({ preventScroll: true }); }
  if (m.__keys) return; m.__keys = true;
  let typed = '', typedT = null;
  m.addEventListener('keydown', (e) => {
    const list = items(); const i = list.indexOf(document.activeElement);
    const go = (n) => { if (!list.length) return; const t = list[(n + list.length) % list.length]; t.focus(); t.scrollIntoView({ block: 'nearest' }); };
    if (e.key === 'ArrowDown') { e.preventDefault(); go(i < 0 ? 0 : i + 1); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); go(i < 0 ? list.length - 1 : i - 1); }
    else if (e.key === 'Home' && e.target.tagName !== 'INPUT') { e.preventDefault(); go(0); }
    else if (e.key === 'End' && e.target.tagName !== 'INPUT') { e.preventDefault(); go(list.length - 1); }
    else if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); const b = m.previousElementSibling; if (b && b.classList.contains('menu-backdrop')) b.click(); }
    else if (e.key === 'Tab') { e.preventDefault(); go(e.shiftKey ? i - 1 : i + 1); }
    else if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && e.target.tagName !== 'INPUT') {
      typed += e.key.toLowerCase(); clearTimeout(typedT); typedT = setTimeout(() => typed = '', 600);
      const hit = list.findIndex((b, j) => j !== i && b.innerText.trim().toLowerCase().startsWith(typed));
      if (hit >= 0) go(hit);
    }
  });"#;

/// A styled select (replaces native `<select>` so it matches the app and
/// stays inside the window). Long lists get a filter box.
#[component]
pub fn Select(
    value: String,
    options: Vec<(String, String)>,
    onchange: EventHandler<String>,
    #[props(default)] placeholder: Option<String>,
    #[props(default = false)] wide: bool,
) -> Element {
    let mut open = use_signal(|| false);
    let mut filter = use_signal(String::new);
    let label = options
        .iter()
        .find(|(v, _)| *v == value)
        .map(|(_, l)| l.clone())
        .or(placeholder.clone())
        .unwrap_or_else(|| value.clone());
    let searchable = options.len() > 8;
    let f = filter.read().to_lowercase();
    let shown: Vec<(String, String)> = options.iter().filter(|(_, l)| f.is_empty() || l.to_lowercase().contains(&f)).cloned().collect();
    rsx! {
        div { class: if wide { "menu-anchor select wide" } else { "menu-anchor select" },
            button { class: if open() { "select-btn open" } else { "select-btn" }, "aria-haspopup": "listbox",
                onclick: move |_| { filter.set(String::new()); open.set(true); },
                span { class: "ellipsis", "{label}" }
                span { class: "chev", Icon { i: I::ChevronDown, size: 12 } }
            }
            Dropdown { open, up: false, right: true, class: "select-menu",
                if searchable {
                    div { class: "menu-search",
                        Icon { i: I::Search, size: 12 }
                        input { placeholder: "Filter…", value: "{filter}", autofocus: true, oninput: move |e| filter.set(e.value()) }
                    }
                }
                for (v, l) in shown {
                    {
                        let checked = v == value;
                        let v2 = v.clone();
                        rsx! {
                            MenuItem { key: "{v}", label: l.clone(), checked,
                                onclick: move |_| { open.set(false); onchange.call(v2.clone()); } }
                        }
                    }
                }
                if searchable && !f.is_empty() && options.iter().all(|(_, l)| !l.to_lowercase().contains(&f)) {
                    div { class: "menu-empty", "No matches" }
                }
            }
        }
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
    let cls = format!("menu-item{}{}{}", if danger { " danger" } else { "" }, if disabled { " disabled" } else { "" }, if checked { " checked" } else { "" });
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
