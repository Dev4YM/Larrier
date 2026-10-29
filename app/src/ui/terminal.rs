//! Real terminals for the side panel: a PTY per terminal (portable-pty,
//! ConPTY on Windows) rendered with xterm.js, like Cursor's terminal tab.
//!
//! Terminals live in a global registry keyed by session id, so they keep
//! running when the panel, tab or session changes. Each terminal keeps the
//! last ~512 KB of raw output; when a view (re)mounts, the buffer is replayed
//! into a fresh xterm instance and live output streams in after it.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use base64::Engine;
use dioxus::prelude::*;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde_json::{json, Value};

use crate::icons::{Icon, I};
use crate::state::use_app;

const SCROLLBACK_BYTES: usize = 512 * 1024;

const XTERM_JS: &str = include_str!("../../assets/xterm/xterm.js");
const XTERM_CSS: &str = include_str!("../../assets/xterm/xterm.css");
const FIT_JS: &str = include_str!("../../assets/xterm/addon-fit.js");

// ----------------------------------------------------------------- registry

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

struct Inner {
    title: String,
    cwd: Option<String>,
    pid: Option<u32>,
    alive: bool,
    exit_code: Option<u32>,
    /// Raw output; `base` is the absolute offset of `data[0]`.
    data: Vec<u8>,
    base: u64,
    writer: Option<Box<dyn Write + Send>>,
    master: Option<Box<dyn MasterPty + Send>>,
    killer: Option<Box<dyn ChildKiller + Send + Sync>>,
    size: PtySize,
    /// Bumped on every (re)start so a stale waiter can't mark a new shell dead.
    generation: u64,
    /// Mounted xterm views (they answer terminal queries themselves).
    viewers: u32,
    /// Terminal queries waiting for a live view to answer them.
    queries: Vec<Vec<u8>>,
}

/// Queries a shell sends to its terminal and the reply we give when no
/// xterm view is attached (ConPTY blocks until its cursor query is answered).
const QUERIES: &[(&[u8], &[u8])] = &[
    (b"\x1b[6n", b"\x1b[1;1R"),
    (b"\x1b[5n", b"\x1b[0n"),
    (b"\x1b[>0c", b"\x1b[>0;10;1c"),
    (b"\x1b[>c", b"\x1b[>0;10;1c"),
    (b"\x1b[0c", b"\x1b[?1;2c"),
    (b"\x1b[c", b"\x1b[?1;2c"),
];

/// Remove terminal queries from `bytes`, returning the cleaned bytes and the
/// queries found (in order). Queries never enter the scrollback, so replaying
/// it can't make xterm answer them a second time.
fn split_queries(bytes: &[u8]) -> (Vec<u8>, Vec<&'static [u8]>) {
    let mut out = Vec::with_capacity(bytes.len());
    let mut found = Vec::new();
    let mut i = 0;
    'outer: while i < bytes.len() {
        if bytes[i] == 0x1b {
            for (q, _) in QUERIES {
                if bytes[i..].starts_with(q) {
                    found.push(*q);
                    i += q.len();
                    continue 'outer;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    (out, found)
}

/// One terminal: a shell running in a PTY plus its scrollback.
pub struct Term {
    pub id: u64,
    inner: Mutex<Inner>,
    notify: tokio::sync::Notify,
}

impl Term {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn push(&self, bytes: &[u8]) {
        {
            let mut g = self.lock();
            let (clean, queries) = split_queries(bytes);
            for q in queries {
                if g.viewers > 0 {
                    g.queries.push(q.to_vec());
                } else if let Some((_, reply)) = QUERIES.iter().find(|(k, _)| *k == q) {
                    if let Some(w) = g.writer.as_mut() {
                        let _ = w.write_all(reply);
                        let _ = w.flush();
                    }
                }
            }
            let bytes = clean.as_slice();
            g.data.extend_from_slice(bytes);
            if g.data.len() > SCROLLBACK_BYTES {
                let cut = g.data.len() - SCROLLBACK_BYTES;
                g.data.drain(..cut);
                g.base += cut as u64;
            }
        }
        self.notify.notify_one();
    }

    /// Output after absolute offset `since` (everything retained if `since`
    /// fell out of the scrollback) and the new end offset.
    fn read_since(&self, since: u64) -> (Vec<u8>, u64) {
        let g = self.lock();
        let end = g.base + g.data.len() as u64;
        if since >= end {
            return (Vec::new(), end);
        }
        let start = since.saturating_sub(g.base) as usize;
        (g.data[start..].to_vec(), end)
    }

    fn write(&self, bytes: &[u8]) {
        let mut g = self.lock();
        if let Some(w) = g.writer.as_mut() {
            let _ = w.write_all(bytes);
            let _ = w.flush();
        }
    }

    fn resize(&self, cols: u16, rows: u16) {
        let mut g = self.lock();
        let size = PtySize { rows: rows.max(2), cols: cols.max(10), pixel_width: 0, pixel_height: 0 };
        if g.size == size {
            return;
        }
        g.size = size;
        if let Some(m) = g.master.as_ref() {
            let _ = m.resize(size);
        }
    }

    fn kill(&self) {
        let mut g = self.lock();
        if let Some(k) = g.killer.as_mut() {
            let _ = k.kill();
        }
        g.writer = None;
        g.master = None;
        g.killer = None;
        g.alive = false;
    }

    fn snapshot(&self) -> (u64, String, bool) {
        let g = self.lock();
        (self.id, g.title.clone(), g.alive)
    }
}

#[derive(Default)]
struct SessionTerms {
    list: Vec<Arc<Term>>,
    selected: Option<u64>,
}

fn registry() -> MutexGuard<'static, HashMap<String, SessionTerms>> {
    static REG: OnceLock<Mutex<HashMap<String, SessionTerms>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap_or_else(|e| e.into_inner())
}

fn find(id: u64) -> Option<Arc<Term>> {
    registry().values().flat_map(|s| s.list.iter()).find(|t| t.id == id).cloned()
}

/// Number of terminals open for a session.
pub fn count(session_id: &str) -> usize {
    registry().get(session_id).map(|s| s.list.len()).unwrap_or(0)
}

fn list(session_id: &str) -> (Vec<(u64, String, bool)>, Option<u64>) {
    let reg = registry();
    match reg.get(session_id) {
        Some(s) => (s.list.iter().map(|t| t.snapshot()).collect(), s.selected),
        None => (Vec::new(), None),
    }
}

fn select(session_id: &str, id: u64) {
    if let Some(s) = registry().get_mut(session_id) {
        s.selected = Some(id);
    }
}

fn remove(session_id: &str, id: u64) {
    let removed = {
        let mut reg = registry();
        let Some(s) = reg.get_mut(session_id) else { return };
        let pos = s.list.iter().position(|t| t.id == id);
        let t = pos.map(|p| s.list.remove(p));
        if s.selected == Some(id) {
            s.selected = s.list.last().map(|t| t.id);
        }
        t
    };
    if let Some(t) = removed {
        t.kill();
    }
}

/// The shell to run: pwsh → Windows PowerShell → cmd on Windows, `$SHELL -l`
/// elsewhere.
fn shell() -> (String, Vec<String>, String) {
    #[cfg(windows)]
    {
        if let Some(p) = larrier_agents::detect::which_program("pwsh") {
            return (p.to_string_lossy().to_string(), vec!["-NoLogo".into()], "pwsh".into());
        }
        if let Some(p) = larrier_agents::detect::which_program("powershell") {
            return (p.to_string_lossy().to_string(), vec!["-NoLogo".into()], "powershell".into());
        }
        let cmd = std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".into());
        (cmd, Vec::new(), "cmd".into())
    }
    #[cfg(not(windows))]
    {
        let sh = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/bash".into());
        let name = std::path::Path::new(&sh).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| "sh".into());
        (sh, vec!["-l".into()], name)
    }
}

/// Spawn (or respawn) the shell for `t` in a fresh PTY.
fn start(t: &Arc<Term>) -> anyhow::Result<()> {
    let (size, cwd) = {
        let g = t.lock();
        (g.size, g.cwd.clone())
    };
    let pair = native_pty_system().openpty(size)?;
    let (prog, args, _) = shell();
    let mut cmd = CommandBuilder::new(&prog);
    cmd.args(&args);
    let dir = cwd
        .filter(|d| std::path::Path::new(d).is_dir())
        .or_else(|| dirs::home_dir().map(|h| h.to_string_lossy().to_string()))
        .unwrap_or_else(|| ".".into());
    cmd.cwd(dir);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    let mut child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader()?;
    let writer = pair.master.take_writer()?;
    let killer = child.clone_killer();
    let generation = {
        let mut g = t.lock();
        g.generation += 1;
        g.pid = child.process_id();
        g.alive = true;
        g.exit_code = None;
        g.writer = Some(writer);
        g.master = Some(pair.master);
        g.killer = Some(killer);
        g.generation
    };

    let rt = t.clone();
    std::thread::Builder::new().name(format!("pty-read-{}", t.id)).spawn(move || {
        let mut buf = [0u8; 16 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => rt.push(&buf[..n]),
            }
        }
    })?;

    let wt = t.clone();
    std::thread::Builder::new().name(format!("pty-wait-{}", t.id)).spawn(move || {
        let code = child.wait().map(|s| s.exit_code()).unwrap_or(1);
        let current = {
            let mut g = wt.lock();
            if g.generation != generation {
                false
            } else {
                g.alive = false;
                g.exit_code = Some(code);
                g.writer = None;
                g.killer = None;
                true
            }
        };
        if current {
            // Let the reader drain the shell's last output first.
            std::thread::sleep(Duration::from_millis(80));
            wt.push(format!("\r\n\x1b[2m[process exited with code {code}] — press Enter to restart\x1b[0m\r\n").as_bytes());
            // Closing ConPTY releases the reader thread.
            wt.lock().master = None;
        }
    })?;
    Ok(())
}

/// Open a new terminal for a session, select it and start its shell.
fn open_terminal(session_id: &str, cwd: Option<String>, title: Option<String>) -> Arc<Term> {
    let (_, _, shell_name) = shell();
    let t = Arc::new(Term {
        id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        inner: Mutex::new(Inner {
            title: title.unwrap_or(shell_name),
            cwd,
            pid: None,
            alive: false,
            exit_code: None,
            data: Vec::new(),
            base: 0,
            writer: None,
            master: None,
            killer: None,
            size: PtySize { rows: 24, cols: 100, pixel_width: 0, pixel_height: 0 },
            generation: 0,
            viewers: 0,
            queries: Vec::new(),
        }),
        notify: tokio::sync::Notify::new(),
    });
    if let Err(e) = start(&t) {
        t.push(format!("\x1b[31mCould not start a shell: {e}\x1b[0m\r\n").as_bytes());
    }
    let mut reg = registry();
    let s = reg.entry(session_id.to_string()).or_default();
    s.list.push(t.clone());
    s.selected = Some(t.id);
    t
}

/// Open a new terminal for `session_id` and run `command` in it.
pub fn run_command(session_id: &str, cwd: Option<String>, command: &str) {
    let title: String = command.chars().take(40).collect();
    let t = open_terminal(session_id, cwd, Some(title));
    t.write(format!("{command}\r").as_bytes());
}

/// Typed input from xterm; Enter on an exited shell restarts it.
fn input(t: &Arc<Term>, data: &str) {
    let alive = t.lock().alive;
    if alive {
        t.write(data.as_bytes());
    } else if data.contains('\r') {
        t.push(b"\r\n");
        if let Err(e) = start(t) {
            t.push(format!("\x1b[31mCould not restart the shell: {e}\x1b[0m\r\n").as_bytes());
        }
    }
}

// ----------------------------------------------------------------- UI

const CSS: &str = r#"
.term-wrap { flex: 1; min-height: 0; display: flex; flex-direction: column; background: var(--pane); }
.term-head { height: 36px; flex-shrink: 0; display: flex; align-items: center; gap: 2px; padding: 0 6px 0 14px; border-bottom: 1px solid var(--border); font-size: 13px; color: var(--fg); }
.term-head .term-title { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.term-body { flex: 1; min-height: 0; display: flex; }
.term-main { flex: 1; min-width: 0; position: relative; }
.term-x { position: absolute; top: 6px; right: 4px; bottom: 2px; left: 10px; }
.term-x .xterm, .term-x .xterm-viewport, .term-x .xterm-screen { background: transparent !important; }
.term-x .xterm-viewport::-webkit-scrollbar { width: 10px; }
.term-x .xterm-viewport::-webkit-scrollbar-thumb { background: var(--scroll); border-radius: 10px; border: 3px solid transparent; background-clip: content-box; }
.term-list { width: 200px; flex-shrink: 0; border-left: 1px solid var(--border); padding: 8px 6px; display: flex; flex-direction: column; gap: 2px; overflow-y: auto; }
.term-list .term-h { font-size: 12px; color: var(--fg-3); padding: 2px 8px 6px; }
.term-item { height: 30px; display: flex; align-items: center; gap: 8px; padding: 0 6px 0 8px; border-radius: 6px; font-size: 13px; color: var(--fg-2); cursor: pointer; width: 100%; text-align: left; }
.term-item:hover { background: var(--hover); color: var(--fg); }
.term-item.on { background: var(--selected); color: var(--fg); }
.term-item.dead { opacity: .6; }
.term-item .term-name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.term-item .term-kill { display: none; }
.term-item:hover .term-kill { display: flex; }
.term-empty { position: absolute; inset: 0; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 10px; color: var(--fg-3); font-size: 12px; }
"#;

#[component]
pub fn TerminalPanel(session_id: String, cwd: Option<String>) -> Element {
    let app = use_app();
    let dark = app.ui.read().dark;
    let mut show_list = use_signal(|| true);
    let mut tick = use_signal(|| 0u64);

    // First visit to a session's terminal tab starts a shell.
    let sid0 = session_id.clone();
    let cwd0 = cwd.clone();
    use_hook(move || {
        if count(&sid0) == 0 {
            open_terminal(&sid0, cwd0, None);
        }
    });

    // Titles / exit states change in the background: refresh the list.
    let sid_poll = session_id.clone();
    use_future(move || {
        let sid = sid_poll.clone();
        async move {
            let mut last = list(&sid);
            loop {
                tokio::time::sleep(Duration::from_millis(600)).await;
                let now = list(&sid);
                if now != last {
                    last = now;
                    tick += 1;
                }
            }
        }
    });

    let _ = tick();
    let (terms, selected) = list(&session_id);
    let current = selected.and_then(|id| terms.iter().find(|t| t.0 == id).cloned()).or_else(|| terms.last().cloned());
    let current_id = current.as_ref().map(|c| c.0);
    let n = terms.len();
    let heading = if n == 1 { "1 Terminal".to_string() } else { format!("{n} Terminals") };
    let (sid_new, cwd_new, sid_kill, sid_new2, cwd_new2) = (session_id.clone(), cwd.clone(), session_id.clone(), session_id.clone(), cwd.clone());

    rsx! {
        style { {CSS} }
        div { class: "term-wrap", "data-ctx": "terminal",
            div { class: "term-head",
                span { class: "term-title", {current.as_ref().map(|c| c.1.clone()).unwrap_or_else(|| "Terminal".into())} }
                button { class: "icon-btn", title: "New terminal",
                    onclick: move |_| {
                        open_terminal(&sid_new, cwd_new.clone(), None);
                        tick += 1;
                    },
                    Icon { i: I::Plus, size: 15, stroke: 1.5 }
                }
                if let Some(c) = current.clone() {
                    button { class: "icon-btn", title: "Kill terminal",
                        onclick: move |_| {
                            remove(&sid_kill, c.0);
                            tick += 1;
                        },
                        Icon { i: I::Trash, size: 14, stroke: 1.5 }
                    }
                }
                button {
                    class: if show_list() { "icon-btn active" } else { "icon-btn" },
                    title: if show_list() { "Hide terminal list" } else { "Show terminal list" },
                    onclick: move |_| show_list.toggle(),
                    Icon { i: I::PanelRight, size: 15, stroke: 1.5 }
                }
            }
            div { class: "term-body",
                div { class: "term-main",
                    match current_id {
                        Some(cid) => rsx! { XTerm { key: "{cid}", term_id: cid, dark } },
                        None => rsx! {
                            div { class: "term-empty",
                                Icon { i: I::Terminal, size: 22, stroke: 1.5 }
                                "No terminals"
                                button { class: "ghost-btn",
                                    onclick: move |_| {
                                        open_terminal(&sid_new2, cwd_new2.clone(), None);
                                        tick += 1;
                                    },
                                    Icon { i: I::Plus, size: 12 }
                                    "New terminal"
                                }
                            }
                        },
                    }
                }
                if show_list() && n > 0 {
                    div { class: "term-list",
                        div { class: "term-h", "{heading}" }
                        for (id, title, alive) in terms.clone() {
                            {
                                let (s1, s2) = (session_id.clone(), session_id.clone());
                                let on = current_id == Some(id);
                                let cls = format!("term-item{}{}", if on { " on" } else { "" }, if alive { "" } else { " dead" });
                                rsx! {
                                    div { key: "{id}", class: "{cls}", role: "button",
                                        onclick: move |_| {
                                            select(&s1, id);
                                            tick += 1;
                                        },
                                        Icon { i: I::Terminal, size: 14, stroke: 1.5 }
                                        span { class: "term-name", "{title}" }
                                        button { class: "icon-btn sm term-kill", title: "Kill",
                                            onclick: move |e| {
                                                e.stop_propagation();
                                                remove(&s2, id);
                                                tick += 1;
                                            },
                                            Icon { i: I::Trash, size: 12, stroke: 1.5 }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Load xterm.js, its CSS and the fit addon into the page (once per process;
/// the page-side flag also guards against double loading).
fn ensure_assets() {
    static DONE: OnceLock<()> = OnceLock::new();
    if DONE.set(()).is_err() {
        return;
    }
    let css = serde_json::to_string(XTERM_CSS).unwrap_or_default();
    let js = format!(
        "if (!window.__larrierXterm) {{\n{XTERM_JS}\n;\n{FIT_JS}\n;\nconst st = document.createElement('style'); st.textContent = {css}; document.head.appendChild(st);\nwindow.__larrierXterm = true;\n}}"
    );
    document::eval(&js);
}

/// JS that creates the xterm instance for terminal `id` and bridges it.
fn mount_js(id: u64, dark: bool) -> String {
    format!(
        r#"
const id = {id};
const domId = 'xterm-' + id;
let el = null;
for (let i = 0; i < 300 && !(el = document.getElementById(domId)); i++) await new Promise(r => setTimeout(r, 15));
for (let i = 0; i < 300 && !window.Terminal; i++) await new Promise(r => setTimeout(r, 15));
if (!el || !window.Terminal) return;
window.__lterms = window.__lterms || {{}};
if (window.__lterms[id]) {{ try {{ window.__lterms[id].dispose(); }} catch (e) {{}} }}
const cs = () => getComputedStyle(el);
const v = (n, d) => (cs().getPropertyValue(n) || '').trim() || d;
const palette = (dark) => dark ? {{
  black: '#1b1a17', red: '#ee6a8a', green: '#4cb88a', yellow: '#d9a149', blue: '#7ba7d8', magenta: '#c792ea', cyan: '#5ec4d4', white: '#d4d2cb',
  brightBlack: '#6e6c66', brightRed: '#f48aa3', brightGreen: '#6fd0a4', brightYellow: '#e8bb6e', brightBlue: '#9cc0e8', brightMagenta: '#d9b0f2', brightCyan: '#86d8e4', brightWhite: '#f4f3ee'
}} : {{
  black: '#26251e', red: '#cf2d56', green: '#1f8a65', yellow: '#a16207', blue: '#3a6a9f', magenta: '#8b3fb5', cyan: '#0e7490', white: '#7a7970',
  brightBlack: '#8e8d85', brightRed: '#e0456b', brightGreen: '#2aa37a', brightYellow: '#b7791f', brightBlue: '#4a7fbf', brightMagenta: '#a35acb', brightCyan: '#1597ad', brightWhite: '#26251e'
}};
const theme = (dark) => Object.assign({{
  background: v('--pane', dark ? '#1b1a17' : '#f7f7f4'),
  foreground: v('--fg', dark ? '#e9e7e0' : '#26251e'),
  cursor: v('--fg', dark ? '#e9e7e0' : '#26251e'),
  cursorAccent: v('--pane', dark ? '#1b1a17' : '#f7f7f4'),
  selectionBackground: dark ? 'rgba(233,231,224,0.22)' : 'rgba(38,37,30,0.16)',
}}, palette(dark));
const term = new Terminal({{
  fontFamily: v('--mono', 'Cascadia Mono, Consolas, monospace'),
  fontSize: 12.5,
  lineHeight: 1.15,
  cursorBlink: true,
  scrollback: 5000,
  allowProposedApi: true,
  theme: theme({dark}),
}});
const fit = new FitAddon.FitAddon();
term.loadAddon(fit);
term.open(el);
let last = '';
const doFit = () => {{
  try {{
    fit.fit();
    const k = term.cols + 'x' + term.rows;
    if (k !== last) {{ last = k; dioxus.send({{ t: 'resize', cols: term.cols, rows: term.rows }}); }}
  }} catch (e) {{}}
}};
requestAnimationFrame(doFit);
const ro = new ResizeObserver(() => requestAnimationFrame(doFit));
ro.observe(el);
term.onData(d => dioxus.send({{ t: 'in', data: d }}));
const copy = () => {{ const s = term.getSelection(); if (s) {{ dioxus.send({{ t: 'copy', text: s }}); term.clearSelection(); return true; }} return false; }};
const paste = () => dioxus.send({{ t: 'paste' }});
term.attachCustomKeyEventHandler(e => {{
  if (e.type !== 'keydown') return true;
  const k = (e.key || '').toLowerCase();
  if (e.ctrlKey && e.shiftKey && k === 'c') {{ copy(); return false; }}
  if (e.ctrlKey && e.shiftKey && k === 'v') {{ paste(); return false; }}
  if (e.ctrlKey && !e.shiftKey && k === 'c' && term.hasSelection()) {{ copy(); return false; }}
  if (e.ctrlKey && !e.shiftKey && k === 'v') {{ paste(); return false; }}
  return true;
}});
const onCtx = (e) => {{ e.preventDefault(); e.stopPropagation(); if (!copy()) paste(); }};
el.addEventListener('contextmenu', onCtx);
const dispose = () => {{ try {{ ro.disconnect(); el.removeEventListener('contextmenu', onCtx); term.dispose(); }} catch (e) {{}} delete window.__lterms[id]; }};
window.__lterms[id] = {{ dispose }};
term.focus();
while (true) {{
  let m;
  try {{ m = await dioxus.recv(); }} catch (e) {{ break; }}
  if (!m) continue;
  if (m.t === 'out') {{
    const bin = atob(m.d);
    const u = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) u[i] = bin.charCodeAt(i);
    term.write(u);
  }} else if (m.t === 'paste') {{
    if (m.text) term.paste(m.text);
  }} else if (m.t === 'theme') {{
    term.options.theme = theme(m.dark);
  }} else if (m.t === 'focus') {{
    term.focus();
  }} else if (m.t === 'dispose') {{
    dispose();
    break;
  }}
}}
"#
    )
}

/// Counts a mounted view for the lifetime of its streaming task.
struct ViewerGuard(Arc<Term>);

impl ViewerGuard {
    fn new(t: Arc<Term>) -> Self {
        t.lock().viewers += 1;
        ViewerGuard(t)
    }
}

impl Drop for ViewerGuard {
    fn drop(&mut self) {
        let mut g = self.0.lock();
        g.viewers = g.viewers.saturating_sub(1);
    }
}

/// One xterm view bound to a registry terminal.
#[component]
fn XTerm(term_id: u64, dark: bool) -> Element {
    let app = use_app();
    let mut bridge: Signal<Option<document::Eval>> = use_signal(|| None);
    let dark_at_mount = use_hook(|| dark);

    use_future(move || async move {
        let Some(t) = find(term_id) else { return };
        let _viewer = ViewerGuard::new(t.clone());
        ensure_assets();
        let ev = document::eval(&mount_js(term_id, dark_at_mount));
        bridge.set(Some(ev));
        let mut rx = ev;
        let mut since = 0u64;
        let b64 = base64::engine::general_purpose::STANDARD;
        loop {
            let (chunk, end) = t.read_since(since);
            let queries: Vec<Vec<u8>> = std::mem::take(&mut t.lock().queries);
            let chunk = if queries.is_empty() { chunk } else { [chunk, queries.concat()].concat() };
            if !chunk.is_empty() {
                // Keep each message reasonably small for the webview bridge.
                for part in chunk.chunks(256 * 1024) {
                    if ev.send(json!({ "t": "out", "d": b64.encode(part) })).is_err() {
                        return;
                    }
                }
            }
            since = end;
            tokio::select! {
                m = rx.recv::<Value>() => match m {
                    Ok(v) => match v["t"].as_str() {
                        Some("in") => input(&t, v["data"].as_str().unwrap_or("")),
                        Some("resize") => t.resize(v["cols"].as_u64().unwrap_or(80) as u16, v["rows"].as_u64().unwrap_or(24) as u16),
                        Some("copy") => {
                            if let Some(s) = v["text"].as_str() {
                                app.copy_text(s);
                            }
                        }
                        Some("paste") => {
                            let text = arboard::Clipboard::new().and_then(|mut c| c.get_text()).unwrap_or_default();
                            if !text.is_empty() {
                                let _ = ev.send(json!({ "t": "paste", "text": text }));
                            }
                        }
                        _ => {}
                    },
                    Err(_) => return,
                },
                _ = t.notify.notified() => {
                    // Batch bursts of output into one frame.
                    tokio::time::sleep(Duration::from_millis(16)).await;
                }
                _ = tokio::time::sleep(Duration::from_millis(400)) => {}
            }
        }
    });

    // Follow the app theme.
    use_effect(use_reactive!(|dark| {
        if let Some(ev) = *bridge.peek() {
            let _ = ev.send(json!({ "t": "theme", "dark": dark }));
        }
    }));

    use_drop(move || {
        document::eval(&format!("window.__lterms && window.__lterms[{term_id}] && window.__lterms[{term_id}].dispose()"));
    });

    rsx! {
        div { id: "xterm-{term_id}", class: "term-x" }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_are_stripped() {
        let (clean, q) = split_queries(b"ab\x1b[6ncd\x1b[0m\x1b[c");
        assert_eq!(clean, b"abcd\x1b[0m");
        assert_eq!(q.len(), 2);
    }

    #[test]
    fn scrollback_is_bounded_and_replays() {
        let t = Term {
            id: 1,
            inner: Mutex::new(Inner {
                title: "t".into(),
                cwd: None,
                pid: None,
                alive: false,
                exit_code: None,
                data: Vec::new(),
                base: 0,
                writer: None,
                master: None,
                killer: None,
                size: PtySize::default(),
                generation: 0,
                viewers: 0,
                queries: Vec::new(),
            }),
            notify: tokio::sync::Notify::new(),
        };
        t.push(b"hello ");
        let (a, end) = t.read_since(0);
        assert_eq!(a, b"hello ");
        t.push(b"world");
        let (b, end2) = t.read_since(end);
        assert_eq!(b, b"world");
        assert_eq!(end2, 11);
        // Overflow keeps only the newest bytes; a stale reader gets them all.
        t.push(&vec![b'x'; SCROLLBACK_BYTES + 10]);
        let (c, end3) = t.read_since(0);
        assert_eq!(c.len(), SCROLLBACK_BYTES);
        assert_eq!(end3, 11 + SCROLLBACK_BYTES as u64 + 10);
    }

    /// Spawns the real shell in a PTY and waits for its prompt.
    #[test]
    #[ignore]
    fn spawns_shell_and_reads_prompt() {
        let t = open_terminal("test-session", None, None);
        t.write(b"echo larrier-pty-ok\r");
        let mut seen = String::new();
        for _ in 0..100 {
            std::thread::sleep(Duration::from_millis(100));
            let (b, _) = t.read_since(0);
            seen = String::from_utf8_lossy(&b).to_string();
            if seen.contains("larrier-pty-ok") && seen.matches("larrier-pty-ok").count() >= 2 {
                break;
            }
        }
        assert!(seen.contains("larrier-pty-ok"), "output: {seen}");
        assert_eq!(count("test-session"), 1);
        remove("test-session", t.id);
        assert_eq!(count("test-session"), 0);
    }
}
