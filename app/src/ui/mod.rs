//! UI components.

mod appearance;
mod ask;
mod automations;
mod chat;
mod composer;
mod customize;
mod files;
mod home;
pub mod loaders;
pub mod mentions;
mod meters;
pub mod menubar;
mod agent_models;
mod models;
mod overlays;
mod profile;
mod request;
mod right;
mod settings;
pub mod setup;
mod sidebar;
mod tasks;
pub mod terminal;
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

    // Each chat's side panel, kept as it changes (see `App::set_view`).
    let panel = use_ui(|u| (u.right_open, u.right_tab, u.right_tabs.clone()));
    use_effect(move || {
        let (open, tab, tabs) = panel();
        if let Some(id) = app.view.read().session_id() {
            crate::keep::set_panel(id, crate::keep::Panel { open, tab, tabs });
        }
    });
    // The new agent screen's picks, kept with its message.
    use_effect(move || crate::keep::set_draft(&app.draft.read()));
    // An account with a folder of its own added or removed: its sessions
    // come in (and are watched) at once.
    let homes = use_memo(move || crate::state::account_homes(&app.config.read()));
    let known = use_hook(|| std::rc::Rc::new(std::cell::RefCell::new(homes.peek().clone())));
    use_effect(move || {
        let h = homes();
        if *known.borrow() != h {
            *known.borrow_mut() = h.clone();
            larrier_formats::set_account_homes(h);
            crate::live::restart(app);
            app.sync_imports(true);
        }
    });

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
        crate::spell::warm();
        crate::models_sync::start();
        crate::automations::start(app);
        app.detect_agents();
        crate::agent_models::start(app);
        // Agents' latest versions, for Settings → Agents to offer updates.
        crate::agent_versions::start();
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
        // Plan limits stay current on their own.
        usage::keep_fresh(app);
        // Installed and portable copies look for a newer release when they
        // start, then every ten minutes. The clock decides when, so a
        // computer that slept looks as it wakes; a look that couldn't reach
        // GitHub is tried again in a few minutes. Turned on in Settings, it
        // looks at once.
        if !matches!(crate::install::current(), crate::install::Kind::Dev) {
            dioxus::core::spawn_forever(async move {
                use std::time::{Duration, SystemTime};
                // LARRIER_UPDATE_EVERY (seconds) shortens the wait for tests.
                let every = std::env::var("LARRIER_UPDATE_EVERY").ok().and_then(|v| v.parse::<u64>().ok()).filter(|s| *s >= 10).unwrap_or(600);
                tokio::time::sleep(Duration::from_secs(8)).await;
                let mut due = SystemTime::now();
                loop {
                    let s = app.config.peek().settings.clone();
                    if s.auto_update && s.updates != larrier_core::UpdateMode::Off && SystemTime::now() >= due {
                        let asked = app.look_for_update(true).await;
                        due = SystemTime::now() + Duration::from_secs(if asked { every } else { every.min(300) });
                    }
                    tokio::time::sleep(Duration::from_secs(every.min(30))).await;
                }
            });
        }
        // Back where the window was when Larrier closed.
        app.restore_layout();
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

    // The agents' browser tools reach Larrier's browser tabs through here.
    use_hook(move || {
        crate::browser_host::set_app(app);
        crate::browser_control::start();
    });

    // Where the window is, the follow-ups waiting and each chat's browser
    // tabs are kept for the next launch as they change.
    let layout = use_memo(move || app.layout());
    use_effect(move || crate::keep::set_layout(&layout.read()));
    use_effect(move || crate::keep::set_queues(&app.queue.read()));
    // The side panel of the chat shown, as it is now (not only once left).
    let panel = use_memo(move || app.view.read().session_id().map(|id| (id.to_string(), crate::keep::Panel::of(&app.ui.read()))));
    use_effect(move || {
        if let Some((id, p)) = panel() {
            crate::keep::set_panel(&id, p);
        }
    });
    use_effect(move || {
        let _ = crate::browser_host::STATE.read();
        crate::browser_host::keep();
    });

    // Bridge: system theme + global keyboard shortcuts from the webview.
    use_future(move || async move {
        // Tooltips name shortcuts the way macOS writes them (⌘K, not Ctrl+K).
        if cfg!(target_os = "macos") {
            document::eval(r#"window.__larrierKeys = (s) => s.replace(/Ctrl\+/g, '⌘').replace(/Shift\+/g, '⇧').replace(/Alt\+/g, '⌥');"#);
        }
        let mut ev = document::eval(
            r#"
            const mq = window.matchMedia('(prefers-color-scheme: dark)');
            dioxus.send({ t: 'theme', dark: mq.matches });
            mq.addEventListener('change', (e) => dioxus.send({ t: 'theme', dark: e.matches }));
            // A typing cursor that glides (motion on): text fields hide their
            // own and this one is drawn where theirs would be, easing to
            // each new spot as text is typed or the cursor moves.
            if (!window.__larrierCaret) {
              window.__larrierCaret = true;
              const caret = document.createElement('div');
              caret.className = 'l-caret';
              document.body.appendChild(caret);
              // A copy of the field's text laid out the same way, to find
              // where its cursor is.
              const mirror = document.createElement('div');
              mirror.setAttribute('aria-hidden', 'true');
              document.body.appendChild(mirror);
              const PROPS = ['paddingTop','paddingRight','paddingBottom','paddingLeft','fontStyle','fontVariant','fontWeight','fontStretch','fontSize','lineHeight','fontFamily',
                'fontKerning','fontFeatureSettings','fontVariationSettings','textAlign','textTransform','textIndent','letterSpacing','wordSpacing','tabSize','direction','unicodeBidi','wordBreak','overflowWrap'];
              const rng = document.createRange();
              const rects = (t, a, b) => { rng.setStart(t, a); rng.setEnd(t, b); return [...rng.getClientRects()]; };
              // Letters of right-to-left scripts (Hebrew, Arabic, Syriac, Thaana, N'Ko...).
              const RTL = /[\u0590-\u05FF\u0608\u060B\u060D\u061B-\u064A\u066D-\u066F\u0671-\u06D5\u06E5\u06E6\u06EE\u06EF\u06FA-\u0710\u0712-\u072F\u074D-\u07A5\u07B1-\u07EA\u07F4\u07F5\u07FA-\u0815\u081A\u0824\u0828\u0830-\u0858\u085E-\u08BD\uFB1D-\uFDFF\uFE70-\uFEFC]|[\uD802\uD803\uD83A\uD83B][\uDC00-\uDFFF]/;
              // The cursor's box in the copy, where the field draws its own.
              const spot = (el, cs) => {
                const v = el.value, i = el.selectionStart || 0;
                // A zero-width mark at the end keeps a last empty line.
                mirror.textContent = v + '\u200b';
                let t = mirror.firstChild;
                let qs = rects(t, i, i);
                if (!qs.length) {
                  // An empty line has nothing to measure: the mark goes where the cursor is.
                  mirror.textContent = v.slice(0, i) + '\u200b' + v.slice(i);
                  t = mirror.firstChild;
                  qs = rects(t, i, i + 1);
                }
                if (qs.length < 2) return qs[0];
                const a = qs[0], b = qs[qs.length - 1];
                // Where a line wraps, the cursor starts the next one.
                if (Math.abs(a.top - b.top) > 1) return a.top > b.top ? a : b;
                // Between text running different ways there are two spots;
                // the cursor sits with the side running the paragraph's way.
                let k = i - 1;
                if (k > 0 && v.charCodeAt(k) >= 0xDC00 && v.charCodeAt(k) <= 0xDFFF) k--;
                const before = k >= 0 && v[k] !== '\n' ? rects(t, k, i)[0] : null;
                if (!before) return a;
                let pdir = cs.direction;
                if (cs.unicodeBidi === 'plaintext') {
                  // dir=auto: each paragraph goes the way of its first letter.
                  const s = v.lastIndexOf('\n', i - 1) + 1, e = v.indexOf('\n', i);
                  pdir = 'ltr';
                  for (const ch of v.slice(s, e < 0 ? v.length : e)) { if (RTL.test(ch)) { pdir = 'rtl'; break; } if (/\p{L}/u.test(ch)) break; }
                }
                const near = (x, y) => Math.abs(x - y) < 0.75;
                const end = qs.find(r => near(r.left, before.left) || near(r.left, before.right)) || a;
                const bdir = near(end.left, before.right) ? 'ltr' : 'rtl';
                return bdir === pdir ? end : (qs.find(r => r !== end) || end);
              };
              let el = null, raf = 0, moved = 0;
              // Where it was last drawn: it glides only along the same line,
              // a short way, in a field that hasn't moved or changed size.
              let last = null, snapNext = false, followUntil = 0;
              const motion = () => { const r = document.querySelector('.larrier-root'); return !!r && !r.classList.contains('no-motion'); };
              const eligible = (t) => !!t && (t.tagName === 'TEXTAREA' || (t.tagName === 'INPUT' && /^(text|search|url|email|)$/i.test(t.getAttribute('type') || ''))) && !t.closest('.xterm');
              const hide = () => { caret.style.visibility = 'hidden'; };
              const place = () => {
                raf = 0;
                if (!el || document.activeElement !== el || !motion()) { if (el) el.classList.remove('l-caret-on'); hide(); last = null; return; }
                el.classList.add('l-caret-on');
                if (el.selectionStart !== el.selectionEnd) { hide(); last = null; return; }
                const r = el.getBoundingClientRect();
                const box = Math.round(r.left) + ',' + Math.round(r.top) + ',' + Math.round(r.width) + ',' + Math.round(r.height);
                const key = el.selectionStart + ':' + el.scrollTop + ':' + el.scrollLeft + ':' + el.value;
                // Following a layout change frame by frame: nothing to redo
                // while the field stays put.
                if (last && last.el === el && last.box === box && last.key === key && !snapNext) {
                  // Unmoved, but a dialog or menu may have opened over it.
                  caret.style.visibility = covered(last.x, last.y) ? 'hidden' : 'visible';
                  return;
                }
                const cs = getComputedStyle(el);
                mirror.style.cssText = 'position:absolute;visibility:hidden;top:0;left:-99999px;overflow:hidden;border:0;box-sizing:border-box;';
                for (const p of PROPS) mirror.style[p] = cs[p];
                mirror.style.whiteSpace = el.tagName === 'TEXTAREA' ? 'pre-wrap' : 'pre';
                mirror.style.width = el.clientWidth + 'px';
                const q = spot(el, cs);
                if (!q) { hide(); last = null; return; }
                const m = mirror.getBoundingClientRect();
                // On whole screen pixels, one wide and as tall as the text,
                // like the field's own.
                const dpr = window.devicePixelRatio || 1;
                const fs = parseFloat(cs.fontSize) || 14;
                // As tall as the text, but no taller than a line set shorter
                // than it (the field's own cursor is cut to the line too).
                const full = q.height || fs * 1.3;
                const lh = parseFloat(cs.lineHeight);
                const h = lh > 0 && lh < full ? lh : full;
                const cut = (full - h) / 2;
                const x = Math.floor((r.left + el.clientLeft + q.left - m.left - el.scrollLeft) * dpr) / dpr;
                // A one-line field centres its line in its height.
                let mid = 0;
                if (el.tagName === 'INPUT') {
                  const pt = parseFloat(cs.paddingTop) || 0, pb = parseFloat(cs.paddingBottom) || 0;
                  mid = Math.max(0, (el.clientHeight - pt - pb - (m.height - pt - pb)) / 2);
                }
                const y = Math.round((r.top + el.clientTop + q.top - m.top - el.scrollTop + mid + cut) * dpr) / dpr;
                if (x < r.left - 1 || x > r.right + 1 || y < r.top - 1 || y + h > r.bottom + 2) { hide(); last = null; return; }
                const glide = !snapNext && !!last && last.el === el && last.box === box && last.y === y && Math.abs(last.x - x) <= fs * 6;
                snapNext = false;
                last = { el, box, key, x, y };
                caret.classList.toggle('snap', !glide);
                caret.style.height = h + 'px';
                caret.style.width = Math.max(1, Math.floor(dpr)) / dpr + 'px';
                // A box drawing its text from a copy behind it (objects) has
                // invisible text of its own: the copy has the color.
                const clear = cs.color === 'transparent' || /rgba\([^)]*,\s*0\)$/.test(cs.color);
                caret.style.background = clear && el.parentElement ? getComputedStyle(el.parentElement).color : cs.color;
                caret.style.transform = 'translate(' + x + 'px,' + y + 'px)';
                caret.style.visibility = covered(x, y) ? 'hidden' : 'visible';
                // Solid while it moves; blinks once still.
                caret.classList.remove('blink');
                clearTimeout(moved);
                moved = setTimeout(() => caret.classList.add('blink'), 530);
              };
              const queue = () => { if (!raf) raf = requestAnimationFrame(place); };
              // Something drawn over the field at the cursor's spot (a dialog,
              // a menu, a popover) hides it; it isn't drawn above them.
              const covered = (x, y) => {
                const hit = document.elementFromPoint(x + 1, y + 2);
                return !!hit && hit !== el && !el.contains(hit) && !hit.contains(el);
              };
              // Dialogs can open without the field noticing: look again now and then.
              setInterval(() => { if (el && document.activeElement === el) queue(); }, 250);
              const snap = () => { snapNext = true; queue(); };
              // While the layout animates (a pane sliding, the composer
              // growing) the field moves without telling: follow it each
              // frame for a moment.
              const follow = (ms) => {
                const was = followUntil > performance.now();
                followUntil = Math.max(followUntil, performance.now() + ms);
                if (was || !el) return;
                const tick = () => { if (!el || performance.now() > followUntil) return; place(); requestAnimationFrame(tick); };
                requestAnimationFrame(tick);
              };
              const ro = new ResizeObserver(() => { snap(); follow(300); });
              document.addEventListener('focusin', (e) => { if (eligible(e.target)) { if (el) ro.unobserve(el); el = e.target; ro.observe(el); snap(); } });
              document.addEventListener('focusout', (e) => { if (e.target === el) { ro.unobserve(el); el.classList.remove('l-caret-on'); el = null; last = null; hide(); } });
              document.addEventListener('selectionchange', queue);
              document.addEventListener('input', queue, true);
              document.addEventListener('scroll', snap, true);
              window.addEventListener('resize', snap);
              document.addEventListener('transitionrun', () => { if (el) follow(500); }, true);
              document.addEventListener('animationstart', () => { if (el) follow(500); }, true);
            }
            // Objects in text boxes (see mentions.rs): a token between two
            // invisible marks, drawn as a pill by a copy of the text behind
            // the box. One Backspace or Delete takes it whole, the cursor
            // never lands inside one, and copies leave the marks out.
            if (!window.__larrierTok) {
              window.__larrierTok = true;
              const O = '⁣', C = '⁤', MARKS = /[⁣⁤]/g;
              const isTok = (el) => !!el && el.tagName === 'TEXTAREA' && el.classList.contains('tok-input');
              const backOf = (el) => el.parentElement && el.parentElement.querySelector(':scope > .tok-back');
              const COPY = ['fontStyle','fontVariant','fontWeight','fontStretch','fontSize','lineHeight','fontFamily','fontKerning','fontFeatureSettings','letterSpacing','wordSpacing',
                'textAlign','textIndent','tabSize','direction','unicodeBidi','wordBreak','overflowWrap','paddingTop','paddingRight','paddingBottom','paddingLeft',
                'borderTopWidth','borderRightWidth','borderBottomWidth','borderLeftWidth'];
              // Lay the copy out exactly like the box (its width without a scrollbar).
              const sync = (el) => {
                const b = backOf(el);
                if (!b) return;
                const cs = getComputedStyle(el);
                for (const p of COPY) b.style[p] = cs[p];
                b.style.left = el.offsetLeft + 'px';
                b.style.top = el.offsetTop + 'px';
                b.style.width = (el.clientWidth + (parseFloat(cs.borderLeftWidth) || 0) + (parseFloat(cs.borderRightWidth) || 0)) + 'px';
                b.style.height = el.offsetHeight + 'px';
                b.scrollTop = el.scrollTop;
                b.scrollLeft = el.scrollLeft;
                b.dataset.synced = '1';
              };
              const later = (el) => requestAnimationFrame(() => sync(el));
              const ro = new ResizeObserver((es) => es.forEach(e => sync(e.target)));
              const init = (el) => {
                if (el.dataset.tok) return;
                el.dataset.tok = '1';
                ro.observe(el);
                const b = backOf(el);
                // The copy's text changes after the box's (when the app re-renders).
                if (b) new MutationObserver(() => sync(el)).observe(b, { childList: true, subtree: true, characterData: true });
                sync(el);
              };
              // Styled the moment it appears (before it's painted): a new box,
              // or a new copy behind one (an edit card opening).
              const scan = () => document.querySelectorAll('textarea.tok-input').forEach((el) => {
                if (!el.dataset.tok) init(el);
                const b = backOf(el);
                if (b && !b.dataset.synced) sync(el);
              });
              new MutationObserver(scan).observe(document.body, { childList: true, subtree: true });
              setInterval(scan, 700);
              document.addEventListener('focusin', (e) => { if (isTok(e.target)) { init(e.target); later(e.target); } });
              document.addEventListener('input', (e) => { if (isTok(e.target)) later(e.target); }, true);
              document.addEventListener('scroll', (e) => { if (isTok(e.target)) sync(e.target); }, true);
              // The object around position i, as [start, end), if i is inside one.
              const inside = (v, i) => {
                const o = v.lastIndexOf(O, i - 1);
                if (o < 0 || v.lastIndexOf(C, i - 1) > o) return null;
                const c = v.indexOf(C, o);
                return c >= i ? [o, c + 1] : null;
              };
              document.addEventListener('keydown', (e) => {
                const el = e.target;
                if (!isTok(el) || e.isComposing || (e.key !== 'Backspace' && e.key !== 'Delete')) return;
                const v = el.value, s = el.selectionStart, en = el.selectionEnd;
                let r = null;
                if (s !== en) {
                  const a = inside(v, s), b = inside(v, en);
                  if (a || b) r = [a ? a[0] : s, b ? b[1] : en];
                } else if (e.key === 'Backspace' && v[s - 1] === C) {
                  r = [v.lastIndexOf(O, s - 1), s];
                } else if (e.key === 'Delete' && v[s] === O) {
                  r = [s, v.indexOf(C, s) + 1];
                } else {
                  r = inside(v, s);
                }
                if (!r || r[0] < 0 || r[1] <= r[0]) return;
                e.preventDefault();
                // The space it was put in with goes too.
                if (v[r[1]] === ' ' && (r[0] === 0 || /\s/.test(v[r[0] - 1]))) r[1]++;
                el.setSelectionRange(r[0], r[1]);
                document.execCommand('delete');
              }, true);
              // The cursor steps over an object; a selection takes it whole.
              const lastAt = new WeakMap();
              document.addEventListener('selectionchange', () => {
                const el = document.activeElement;
                if (!isTok(el)) return;
                const v = el.value, s = el.selectionStart, en = el.selectionEnd;
                const prev = lastAt.has(el) ? lastAt.get(el) : s;
                if (v.indexOf(O) >= 0) {
                  if (s === en) {
                    const t = inside(v, s);
                    if (t) {
                      const to = prev <= t[0] ? t[1] : prev >= t[1] ? t[0] : (s - t[0] <= t[1] - s ? t[0] : t[1]);
                      el.setSelectionRange(to, to);
                    }
                  } else {
                    const a = inside(v, s), b = inside(v, en);
                    if (a || b) el.setSelectionRange(a ? a[0] : s, b ? b[1] : en, el.selectionDirection);
                  }
                }
                lastAt.set(el, el.selectionStart);
              });
              const marked = (el) => /[⁣⁤]/.test(el.value.slice(el.selectionStart, el.selectionEnd));
              for (const kind of ['copy', 'cut']) {
                document.addEventListener(kind, (e) => {
                  const el = document.activeElement;
                  if (!isTok(el) || !marked(el)) return;
                  e.preventDefault();
                  e.clipboardData.setData('text/plain', el.value.slice(el.selectionStart, el.selectionEnd).replace(MARKS, ''));
                  if (kind === 'cut') document.execCommand('delete');
                }, true);
              }
              document.addEventListener('paste', (e) => {
                const el = e.target;
                const t = e.clipboardData && e.clipboardData.getData('text/plain');
                if (!isTok(el) || !t || !MARKS.test(t)) return;
                MARKS.lastIndex = 0;
                e.preventDefault();
                document.execCommand('insertText', false, t.replace(MARKS, ''));
              }, true);
            }
            // The Browser tab's page area: where it is, and whether a menu or
            // dialog of the app's is over it (the page is a native view drawn
            // above the app, so it steps aside then). Sent when it changes.
            if (!window.__larrierBrowserArea) {
              window.__larrierBrowserArea = true;
              const OVER = '.menu, .ctx-menu, #g-menu, .modal, .palette, .lightbox, .overlay, .menu-backdrop + .menu';
              let last = '';
              const tick = () => {
                const a = document.getElementById('lb-area');
                let msg = { t: 'browser-area', none: true };
                if (a && a.isConnected && a.getClientRects().length) {
                  const r = a.getBoundingClientRect();
                  let covered = false;
                  for (const o of document.querySelectorAll(OVER)) {
                    const q = o.getBoundingClientRect();
                    if (q.width && q.height && q.right > r.left && q.left < r.right && q.bottom > r.top && q.top < r.bottom && getComputedStyle(o).visibility !== 'hidden') { covered = true; break; }
                  }
                  const d = window.devicePixelRatio || 1;
                  msg = { t: 'browser-area', x: Math.round(r.left * d), y: Math.round(r.top * d), w: Math.round(r.width * d), h: Math.round(r.height * d), covered };
                }
                const k = JSON.stringify(msg);
                if (k !== last) { last = k; dioxus.send(msg); }
                requestAnimationFrame(tick);
              };
              requestAnimationFrame(tick);
            }
            // Copy buttons of code blocks (in rendered Markdown): the text
            // goes to the system clipboard through the app.
            if (!window.__larrierCopy) {
              window.__larrierCopy = true;
              document.addEventListener('click', (e) => {
                const b = e.target && e.target.closest ? e.target.closest('.code-copy') : null;
                if (!b) return;
                e.preventDefault();
                e.stopPropagation();
                const c = b.closest('.code-block');
                const code = c && c.querySelector('code');
                if (!code) return;
                dioxus.send({ t: 'copy', text: code.innerText.replace(/\n$/, '') });
                const u = b.querySelector('use');
                if (u) {
                  u.setAttribute('href', '#lucide-check');
                  b.classList.add('done');
                  clearTimeout(b.__t);
                  b.__t = setTimeout(() => { u.setAttribute('href', '#lucide-copy'); b.classList.remove('done'); }, 1400);
                }
              }, true);
            }
            // Menus stay inside the window whatever loads into them after
            // they open: each open menu is re-fitted whenever its size
            // changes (more results, a longer list).
            if (!window.__larrierMenus) {
              window.__larrierMenus = true;
              const pad = 8;
              const minTop = () => { const b = document.querySelector('.menubar'); return (b ? b.getBoundingClientRect().bottom : 0) + pad; };
              const set = (m, k, v) => { if (m.style[k] !== v) m.style[k] = v; };
              const fit = (m) => {
                if (!m.isConnected) return;
                const r = m.getBoundingClientRect();
                if (!r.height) return;
                const top = minTop(), bottom = innerHeight - pad;
                const natural = m.scrollHeight;
                if (m.classList.contains('mention-menu')) {
                  // Visible room: the window under the menu bar, inside every
                  // box that scrolls or clips (New Agent's page scrolls under
                  // its drag strip).
                  let lo = top, hi = bottom;
                  for (let a = m.parentElement; a && a !== document.body; a = a.parentElement) {
                    const cs = getComputedStyle(a);
                    if (cs.overflowX === 'visible' && cs.overflowY === 'visible') continue;
                    const ar = a.getBoundingClientRect();
                    lo = Math.max(lo, ar.top + pad);
                    hi = Math.min(hi, ar.bottom - pad);
                  }
                  const wrap = m.parentElement.getBoundingClientRect();
                  const box = (m.parentElement.querySelector('.composer') || m.parentElement).getBoundingClientRect();
                  const above = box.top - 4 - lo, below = hi - (box.bottom + 4);
                  // Above the composer when the whole list fits there, else
                  // below when it fits there, else on the roomier side; the
                  // side it opened on stays while the list still fits it.
                  const want = Math.min(natural, 340);
                  const was = m.dataset.side;
                  const up = was === 'up' && above >= want ? true
                    : was === 'down' && below >= want ? false
                    : above >= want ? true : below >= want ? false : above >= below;
                  m.dataset.side = up ? 'up' : 'down';
                  const room = Math.max(48, Math.min(340, up ? above : below));
                  set(m, 'maxHeight', room + 'px');
                  if (up) { set(m, 'top', 'auto'); set(m, 'bottom', (wrap.bottom - box.top + 4) + 'px'); }
                  else { set(m, 'bottom', 'auto'); set(m, 'top', (box.bottom - wrap.top + 4) + 'px'); }
                  m.classList.toggle('flip-up', up);
                  // Anything still drawn over its far edge (a header, a drag
                  // strip) takes that much off.
                  const r2 = m.getBoundingClientRect();
                  const y = up ? r2.top + 2 : r2.bottom - 2;
                  const hit = document.elementFromPoint(r2.left + 24, y);
                  if (hit && !m.contains(hit) && !hit.closest('.l-tip')) {
                    const hr = hit.getBoundingClientRect();
                    const lost = up ? hr.bottom - r2.top : r2.bottom - hr.top;
                    if (lost > 0 && lost < room) set(m, 'maxHeight', Math.max(48, room - lost - pad) + 'px');
                  }
                  return;
                }
                if (getComputedStyle(m).position !== 'fixed') return;
                const cap = parseFloat(getComputedStyle(m).getPropertyValue('--menu-max')) || 440;
                if (m.classList.contains('flip-up')) {
                  // Grows upward from where it was anchored.
                  const end = r.bottom;
                  const h = Math.min(natural, cap, end - top);
                  set(m, 'maxHeight', Math.max(96, end - top) + 'px');
                  set(m, 'top', Math.max(top, end - h) + 'px');
                } else {
                  set(m, 'maxHeight', Math.max(96, Math.min(cap, bottom - r.top)) + 'px');
                  if (r.top < top) set(m, 'top', top + 'px');
                }
                if (r.right > innerWidth - pad) set(m, 'left', Math.max(pad, innerWidth - pad - r.width) + 'px');
              };
              const ro = new ResizeObserver((es) => { for (const e of es) fit(e.target); });
              const watch = (n) => {
                if (n.nodeType !== 1) return;
                const list = n.matches('.menu, .mention-menu') ? [n] : [...n.querySelectorAll('.menu, .mention-menu')];
                for (const m of list) { if (!m.__fitted) { m.__fitted = true; ro.observe(m); } requestAnimationFrame(() => fit(m)); }
              };
              new MutationObserver((ms) => { for (const mu of ms) { for (const n of mu.addedNodes) watch(n); if (mu.type === 'childList' && mu.target.closest) { const m = mu.target.closest('.menu, .mention-menu'); if (m) requestAnimationFrame(() => fit(m)); } } })
                .observe(document.body, { childList: true, subtree: true });
              window.addEventListener('resize', () => document.querySelectorAll('.menu, .mention-menu').forEach(fit));
            }
            // Fast typing: a field's text comes back from Larrier after each
            // key, and when that lags behind the keys (a slow machine, a
            // busy webview) an older text would land on newer typing and
            // eat characters. Text the user has already typed past isn't
            // put back (Larrier's own changes, like clearing after sending
            // or inserting an object, still are).
            if (!window.__larrierTypeGuard) {
              window.__larrierTypeGuard = true;
              const guard = (el) => {
                if (el.__lguard) return;
                el.__lguard = true;
                const d = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(el), 'value');
                if (!d || !d.set) return;
                const typed = [];
                el.addEventListener('input', () => {
                  const now = performance.now();
                  typed.push({ v: d.get.call(el), t: now });
                  while (typed.length && now - typed[0].t > 2000) typed.shift();
                }, true);
                Object.defineProperty(el, 'value', {
                  configurable: true,
                  get() { return d.get.call(el); },
                  set(v) {
                    const cur = d.get.call(el);
                    const last = typed[typed.length - 1];
                    const stale = v !== '' && v !== cur && last && last.v === cur && cur.startsWith(v) && typed.some((e) => e.v === v);
                    if (!stale) d.set.call(el, v);
                  },
                });
              };
              document.addEventListener('focusin', (e) => {
                const el = e.target;
                if (el && (el.tagName === 'TEXTAREA' || (el.tagName === 'INPUT' && (el.type === 'text' || el.type === 'search' || el.type === '')))) guard(el);
              }, true);
            }
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
                // A browser page is a window over the app: a tip that would
                // land on it shows above its button instead.
                const page = document.getElementById('lb-area');
                const p = page && page.getBoundingClientRect();
                const onPage = p && p.width > 0 && x < p.right && x + t.width > p.left && y < p.bottom && y + t.height > p.top;
                if (y + t.height > innerHeight - 6 || onPage) y = r.top - t.height - 6;
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
                  tip.textContent = window.__larrierKeys ? window.__larrierKeys(el.getAttribute('data-tip')) : el.getAttribute('data-tip');
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
              // Ctrl+Shift+V in a message box: the clipboard as plain text,
              // never made an attachment however long.
              if (e.shiftKey && k === 'v' && e.target && e.target.closest && e.target.closest('.composer, .user-msg.editing')) {
                e.preventDefault();
                window.__larrierCtx = null;
                dioxus.send({ t: 'paste-plain' });
                return;
              }
              if (['k','n','b','j',',','l','p','r','w','e','g','i','.','/','=','+','-','0','tab'].includes(k)) {
                e.preventDefault();
                dioxus.send({ t: 'key', k, shift: e.shiftKey, alt: e.altKey });
              }
            }, true);
            // A Quote button over text selected in an agent's reply: the
            // quote goes into the message box as an object.
            (() => {
              let pop = null;
              const hide = () => { if (pop) pop.style.display = 'none'; };
              const inAgent = (n) => { const el = n && (n.nodeType === 1 ? n : n.parentElement); return !!(el && el.closest && el.closest('.a-text') && !el.closest('.composer')); };
              const show = () => {
                const sel = window.getSelection();
                if (!sel || sel.isCollapsed || !sel.rangeCount || !String(sel).trim()) return hide();
                if (!inAgent(sel.anchorNode) && !inAgent(sel.focusNode)) return hide();
                const r = sel.getRangeAt(0).getBoundingClientRect();
                if (!r || (!r.width && !r.height)) return hide();
                if (!pop) {
                  pop = document.createElement('button');
                  pop.className = 'quote-pop';
                  pop.title = 'Quote it in your message';
                  pop.innerHTML = '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><use href=#lucide-message-square-quote></use></svg><span>Quote</span>';
                  // Pressing it keeps the selection.
                  pop.addEventListener('mousedown', (e) => e.preventDefault());
                  pop.addEventListener('click', (e) => {
                    e.preventDefault();
                    const t = String(window.getSelection() || '').trim();
                    if (t) dioxus.send({ t: 'quote', text: t });
                    hide();
                    window.getSelection()?.removeAllRanges();
                  });
                  document.body.appendChild(pop);
                }
                pop.style.display = 'flex';
                const w = pop.offsetWidth || 76, h = pop.offsetHeight || 28;
                const x = Math.max(8, Math.min(r.left + r.width / 2 - w / 2, window.innerWidth - w - 8));
                const y = r.top - h - 8 >= 8 ? r.top - h - 8 : r.bottom + 8;
                pop.style.left = x + 'px';
                pop.style.top = y + 'px';
              };
              document.addEventListener('mouseup', (e) => { if (pop && pop.contains(e.target)) return; setTimeout(show, 0); });
              document.addEventListener('keyup', (e) => { if (e.shiftKey || e.key === 'Shift') setTimeout(show, 0); });
              document.addEventListener('mousedown', (e) => { if (pop && !pop.contains(e.target)) hide(); });
              document.addEventListener('scroll', hide, true);
              document.addEventListener('selectionchange', () => { const s = window.getSelection(); if (!s || s.isCollapsed) hide(); });
            })();
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
              // A word right-clicked (or selected) in a text box: the menu
              // offers its corrections, the word selected once it's known to
              // be misspelled (as Chrome does).
              let spell = null;
              const wordy = /^[\p{L}\p{M}'\u2019]{2,40}$/u;
              const checked = ed && typeof ed.selectionStart === 'number' && (ed.classList.contains('tok-input') || ed.spellcheck !== false);
              if (checked && wordy.test(sel)) {
                spell = { word: sel, start: ed.selectionStart, end: ed.selectionEnd, pick: false };
              } else if (checked && !sel) {
                // The word under the pointer (the caret, if the point is no text).
                let at = ed.selectionStart;
                const cp = document.caretPositionFromPoint ? document.caretPositionFromPoint(e.clientX, e.clientY) : null;
                if (cp && cp.offsetNode === ed) at = cp.offset;
                const v = ed.value, isW = (c) => /[\p{L}\p{M}'\u2019]/u.test(c || '');
                let a = at, b = at;
                while (a > 0 && isW(v[a - 1])) a--;
                while (b < v.length && isW(v[b])) b++;
                const w = v.slice(a, b).replace(/^['\u2019]+|['\u2019]+$/g, '');
                if (wordy.test(w)) { const s = v.indexOf(w, a); spell = { word: w, start: s, end: s + w.length, pick: true }; }
              }
              const msg = { t: 'ctx', x: e.clientX, y: e.clientY, editable: !!ed, selection: sel.slice(0, 200000),
                link: a ? a.href : null, code: pre ? pre.innerText.slice(0, 500000) : null,
                path: pathEl ? pathEl.getAttribute('data-path') : null, image: t.tagName === 'IMG' ? t.src : null, spell };
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
                const page = st && st.id.startsWith('page:');
                const comp = [...scope.querySelectorAll('.composer')].find(c => inside(c.getBoundingClientRect(), x, y, 28, 40)) || (page ? scope.querySelector('.composer') : null);
                if (page && !comp) return null;
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
                        app.stop_reply(&id);
                    }
                }
                _ => {}
            }
        }
        Some("nav") => app.go(v["d"].as_i64().unwrap_or(0) as isize),
        Some("browser-area") => {
            let area = (!v["none"].as_bool().unwrap_or(false)).then(|| crate::browser_host::Area {
                x: v["x"].as_f64().unwrap_or(0.0),
                y: v["y"].as_f64().unwrap_or(0.0),
                w: v["w"].as_f64().unwrap_or(0.0),
                h: v["h"].as_f64().unwrap_or(0.0),
            });
            crate::browser_host::set_area(area, v["covered"].as_bool().unwrap_or(false));
        }
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
            // A browser page tab (or the Browser itself: the page it shows)
            // goes to the chat's box as context.
            if let Some(tab) = id.strip_prefix("page:") {
                if v["zone"].as_str() == Some("context") {
                    crate::browser_host::to_chat(tab, target);
                }
                return;
            }
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
        // A code block's Copy button: through the system clipboard (the
        // webview's own clipboard API isn't allowed to write).
        Some("copy") => {
            if let Some(t) = v["text"].as_str() {
                app.copy_text(t);
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
            // Said once a run: how to paste it as text instead.
            static TOLD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !TOLD.swap(true, std::sync::atomic::Ordering::Relaxed) {
                app.toast("Long text was attached as a file. Ctrl+Shift+V pastes it as text.");
            }
        }
        Some("paste-plain") => menubar::paste_into_focused(),
        // The Quote button over agent text: an object in the chat's box.
        Some("quote") => {
            if let (Some(t), Some(id)) = (v["text"].as_str(), app.current_id()) {
                app.ui.write().chip_inbox.push((Some(id), mentions::MentionChip::quote(t), None));
            }
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
            let g = crate::state::GlobalCtx {
                x: v["x"].as_f64().unwrap_or(0.0),
                y: v["y"].as_f64().unwrap_or(0.0),
                editable: v["editable"].as_bool().unwrap_or(false),
                selection: s("selection").unwrap_or_default(),
                link: s("link"),
                code: s("code"),
                path: s("path"),
                image: s("image"),
                spell: None,
            };
            let word = v["spell"]["word"].as_str().map(|w| (w.to_string(), v["spell"]["start"].as_u64().unwrap_or(0) as usize, v["spell"]["end"].as_u64().unwrap_or(0) as usize));
            let pick = v["spell"]["pick"].as_bool().unwrap_or(false);
            {
                let mut ui = app.ui.write();
                ui.ctx = None;
                ui.menubar = None;
            }
            match word {
                // The checker answers in a moment; the menu opens with its answer.
                Some((w, start, end)) => {
                    spawn(async move {
                        let w2 = w.clone();
                        let found = tokio::task::spawn_blocking(move || crate::spell::suggest(&w2)).await.ok().flatten();
                        // A misspelled word right-clicked is selected, so it's
                        // clear what a correction replaces.
                        if pick && found.is_some() {
                            document::eval(&format!("const el = window.__larrierCtx; if (el) el.setSelectionRange({start}, {end});"));
                        }
                        let mut g = g;
                        g.spell = found.map(|suggestions| crate::state::Misspelling { word: w, start, end, suggestions });
                        app.ui.write().gctx = Some(g);
                    });
                }
                None => app.ui.write().gctx = Some(g),
            }
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
    TypeScale::of(s).vars()
}

/// Every kind of text's size and line height, set from the one text size
/// and the density. Line heights are whole pixels, so rows, carets and the
/// text drawn behind text boxes line up exactly at every size.
#[derive(Debug, PartialEq)]
pub struct TypeScale {
    /// (CSS variable, px)
    pub vars: Vec<(&'static str, f64)>,
}

impl TypeScale {
    pub fn of(s: &larrier_core::Settings) -> TypeScale {
        let b = s.text_size.px() as f64;
        let compact = s.density == larrier_core::Density::Compact;
        let lh = |px: f64, ratio: f64, min: f64| (px * ratio).round().max(min);
        let (steps, code, note) = (b - 1.0, b - 2.0, b - 2.0);
        TypeScale {
            vars: vec![
                // The agent's replies; headings and inline code scale with them.
                ("fs", b),
                ("lh", lh(b, if compact { 1.5 } else { 1.7 }, 0.0)),
                // Your messages, and what you type.
                ("fs-prompt", b),
                ("lh-prompt", lh(b, if compact { 1.43 } else { 1.57 }, 0.0)),
                ("fs-composer", b),
                ("lh-composer", lh(b, 1.43, 0.0)),
                // Tool steps, and the heads of tool cards a size below them.
                ("fs-steps", steps),
                ("lh-steps", lh(steps, 1.25, 16.0)),
                ("fs-tool", steps - 1.0),
                ("lh-tool", lh(steps - 1.0, 1.33, 16.0)),
                // Code blocks, tool output and diffs.
                ("fs-code", code),
                ("lh-code", lh(code, 1.6, 0.0)),
                ("fs-out", code - 1.0),
                ("lh-out", lh(code - 1.0, 1.55, 0.0)),
                ("fs-diff", code - 0.5),
                ("lh-diff", lh(code - 0.5, 1.65, 0.0)),
                // Thinking and other quiet text in the chat.
                ("fs-note", note),
                ("lh-note", lh(note, 1.55, 0.0)),
                // Sidebar rows (and their times), and the terminal.
                ("fs-side", b - 1.0),
                ("fs-side-2", b - 2.0),
                ("fs-term", b - 1.0),
            ],
        }
    }

    #[cfg(test)]
    pub fn get(&self, var: &str) -> f64 {
        self.vars.iter().find(|(k, _)| *k == var).map_or(0.0, |(_, v)| *v)
    }

    fn vars(&self) -> String {
        self.vars.iter().map(|(k, v)| format!("--{k}: {v}px;")).collect::<Vec<_>>().join(" ")
    }
}

/// A widget's state kept with its chat (a card left open, a group folded):
/// as it was when the chat is shown again, also after a restart. The widget
/// is one per chat (keyed by it), so its state is read once.
pub fn use_kept<T>(chat: &str, name: &'static str, default: impl FnOnce() -> T) -> Signal<T>
where
    T: Clone + PartialEq + serde::Serialize + serde::de::DeserializeOwned + 'static,
{
    let chat = use_hook(|| chat.to_string());
    let v = use_signal(|| crate::keep::value::<T>(&chat, name).unwrap_or_else(default));
    use_effect(move || {
        let now = v.read().clone();
        if !chat.is_empty() {
            crate::keep::set_value(&chat, name, &now);
        }
    });
    v
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
    // macOS has its traffic lights and Linux the system's frame.
    if !crate::platform::DRAWS_CAPTION {
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
            button { class: "win-btn close", title: "Close", onclick: move |_| { app.install_on_exit(); w3.close() },
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
                span { {crate::platform::key(k)} }
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
  m.style.left = left + 'px'; m.style.maxHeight = Math.min(cap, room) + 'px';
  if (up) {{ m.style.top = 'auto'; m.style.bottom = (vh - (a.top - gap)) + 'px'; }} else {{ m.style.top = top + 'px'; }}
  m.style.transformOrigin = (up ? 'bottom ' : 'top ') + ({align_right} ? 'right' : 'left');
  m.classList.toggle('flip-up', up);
  // It scrolls only when its content is clearly taller than it: checked once
  // it has settled after opening, and when its content changes.
  m.style.overflowY = 'hidden';
  const fit = () => {{ if (m.isConnected) m.style.overflowY = m.scrollHeight > m.clientHeight + 8 ? 'auto' : 'hidden'; }};
  setTimeout(() => {{ fit(); new MutationObserver(() => requestAnimationFrame(fit)).observe(m, {{ childList: true, subtree: true, characterData: true }}); }}, 320);
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

#[cfg(test)]
mod tests {
    use super::TypeScale;
    use larrier_core::{Density, Settings, TextSize};

    fn scale(t: TextSize, d: Density) -> TypeScale {
        TypeScale::of(&Settings { text_size: t, density: d, ..Settings::default() })
    }

    #[test]
    fn default_text_keeps_its_sizes() {
        let s = scale(TextSize::Default, Density::Comfortable);
        let got: Vec<f64> = ["fs", "lh", "fs-prompt", "lh-prompt", "fs-composer", "lh-composer", "fs-steps", "lh-steps", "fs-tool", "lh-tool", "fs-code", "fs-side", "fs-term"].iter().map(|v| s.get(v)).collect();
        assert_eq!(got, [14.0, 24.0, 14.0, 22.0, 14.0, 20.0, 13.0, 16.0, 12.0, 16.0, 12.0, 13.0, 13.0]);
    }

    #[test]
    fn every_step_grows_together_on_whole_pixels() {
        for d in [Density::Comfortable, Density::Compact] {
            let steps: Vec<TypeScale> = TextSize::ALL.iter().map(|t| scale(*t, d)).collect();
            for s in &steps {
                for (k, v) in &s.vars {
                    if let Some(kind) = k.strip_prefix("lh") {
                        assert_eq!(v.fract(), 0.0, "{k} = {v}");
                        let fs = s.get(&format!("fs{kind}"));
                        assert!(*v > fs, "{k} {v} isn't taller than its text {fs}");
                    }
                }
            }
            for w in steps.windows(2) {
                for (k, v) in &w[1].vars {
                    assert!(*v >= w[0].get(k), "{k} shrinks from {} to {v}", w[0].get(k));
                }
            }
        }
    }
}
