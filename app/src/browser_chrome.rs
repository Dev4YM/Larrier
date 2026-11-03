//! The browser's engine when Google Chrome is installed. Larrier runs its
//! own Chrome (a profile of its own, a DevTools port for the agents) and
//! seats each of its windows in the Browser tab: inside a frame window over
//! the tab's area, its title bar and toolbar outside the frame, so what
//! shows is the page. Only windows of the Chrome Larrier started are ever
//! touched.

#![cfg(windows)]

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use larrier_agents::browser::{chrome_running, Cdp};
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;
use windows::core::{w, BOOL};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetStockObject, HBRUSH, WHITE_BRUSH};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, GetFocus, SetFocus, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, EnumChildWindows, EnumWindows, GetClassNameW, GetCursorPos, IsChild, WindowFromPoint, WA_CLICKACTIVE, WA_INACTIVE, WM_ACTIVATE, WM_SETFOCUS, GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible,
    RegisterClassW, SetParent, HWND_TOP, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, GWL_STYLE, SET_WINDOW_POS_FLAGS, SWP_ASYNCWINDOWPOS, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOSIZE,
    SWP_NOZORDER, SWP_SHOWWINDOW,
    WM_DESTROY, WNDCLASSW, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_APPWINDOW, WS_EX_NOPARENTNOTIFY, WS_EX_TOOLWINDOW,
    WS_THICKFRAME, WS_VISIBLE,
};

use crate::browser_host::{Area, Ev};

/// Larrier's Chrome: its profile folder.
pub fn profile() -> PathBuf {
    larrier_agents::browser::chrome_dir()
}

/// Google Chrome on this PC.
pub fn chrome_exe() -> Option<PathBuf> {
    static FOUND: OnceLock<Option<PathBuf>> = OnceLock::new();
    FOUND
        .get_or_init(|| {
            ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"]
                .iter()
                .filter_map(|v| std::env::var_os(v).map(PathBuf::from))
                .map(|b| b.join(r"Google\Chrome\Application\chrome.exe"))
                .find(|p| p.is_file())
        })
        .clone()
}

/// What the UI asks of the Chrome worker.
pub enum Cmd {
    NewTab { id: u64, url: String },
    Close { id: u64 },
    Navigate { id: u64, url: String },
    History { id: u64, back: bool },
    Reload { id: u64 },
    Stop { id: u64 },
    DevTools { id: u64 },
    /// Seat the window of a page Chrome opened in a window of its own (a
    /// popup, Ctrl+N) as tab `id`.
    Adopt { id: u64, target: String },
    Script { id: u64, js: String },
    /// Show tab `id` filling the frame (now `size`, when it changed), the
    /// others out of sight.
    Show { id: Option<u64>, size: Option<(i32, i32)> },
    /// A picture of tab `id`, or of part of it (x, y, width, height in CSS
    /// pixels): base64 JPEG.
    Capture { id: u64, clip: Option<(f64, f64, f64, f64)>, reply: Sender<Option<String>> },
    /// Run `js` in tab `id`'s page: its value.
    Eval { id: u64, js: String, reply: Sender<Option<Value>> },
    Quit { reply: Sender<()> },
}

struct Setup {
    main: isize,
    events: UnboundedSender<Ev>,
    annotate_js: String,
}

struct Shared {
    /// The worker, while Chrome runs.
    tx: Option<Sender<Cmd>>,
    /// The frame window (the UI thread's), as an isize.
    frame: isize,
    /// Where the frame goes (x, y, width, height), when a page is shown.
    rect: Option<(i32, i32, i32, i32)>,
    size: (i32, i32),
    /// A page is seated: the frame may show (until then, nothing to see).
    ready: bool,
}

static SETUP: OnceLock<Setup> = OnceLock::new();
static SHARED: Mutex<Option<Shared>> = Mutex::new(None);
/// Tab → DevTools target, for the agents' tools.
static TARGETS: Mutex<Option<HashMap<u64, String>>> = Mutex::new(None);

/// Out of the frame's sight (the page keeps rendering there).
const AWAY: i32 = -40_000;

/// Called by the browser host before its first command: the app's window,
/// where pages' news goes, and the annotate script.
pub fn setup(main: isize, events: UnboundedSender<Ev>, annotate_js: String) {
    let _ = SETUP.set(Setup { main, events, annotate_js });
}

pub fn target_of(tab: u64) -> Option<String> {
    TARGETS.lock().ok()?.as_ref()?.get(&tab).cloned()
}

fn set_target(tab: u64, target: Option<&str>) {
    if let Ok(mut t) = TARGETS.lock() {
        let map = t.get_or_insert_with(HashMap::new);
        match target {
            Some(id) => map.insert(tab, id.to_string()),
            None => map.remove(&tab),
        };
    }
}

/// Send the worker a command. A new tab starts Chrome when it isn't running;
/// the frame is made the first time (on the UI thread, whose window it is).
pub fn send(cmd: Cmd) {
    let Some(setup) = SETUP.get() else { return };
    let mut g = SHARED.lock().unwrap();
    if g.is_none() {
        let Some(frame) = make_frame(setup.main) else { return };
        *g = Some(Shared { tx: None, frame, rect: None, size: (1200, 800), ready: false });
    }
    let s = g.as_mut().unwrap();
    let mut cmd = cmd;
    for _ in 0..2 {
        if s.tx.is_none() {
            // Chrome starts with a tab; anything else is for a Chrome gone.
            if !matches!(cmd, Cmd::NewTab { .. }) {
                return;
            }
            let (tx, rx) = mpsc::channel();
            let (events, js, frame) = (setup.events.clone(), setup.annotate_js.clone(), s.frame);
            if std::thread::Builder::new().name("chrome".into()).spawn(move || worker(rx, events, frame, js)).is_err() {
                return;
            }
            s.tx = Some(tx);
        }
        match s.tx.as_ref().unwrap().send(cmd) {
            Ok(()) => return,
            // The worker ended (Chrome closed): start again.
            Err(mpsc::SendError(c)) => {
                s.tx = None;
                cmd = c;
            }
        }
    }
}

/// Put the frame over the area (window pixels) showing tab `active`, or out
/// of sight (`None`).
pub fn place(area: Option<Area>, active: Option<u64>) {
    let mut g = SHARED.lock().unwrap();
    let Some(s) = g.as_mut() else { return };
    s.rect = area.filter(|_| active.is_some()).map(|a| (a.x.round() as i32, a.y.round() as i32, a.w.round().max(1.0) as i32, a.h.round().max(1.0) as i32));
    let size = s.rect.map(|r| (r.2, r.3)).filter(|z| *z != s.size);
    if let Some(z) = size {
        s.size = z;
    }
    put_frame(s, SET_WINDOW_POS_FLAGS(0));
    if let Some(tx) = &s.tx {
        let _ = tx.send(Cmd::Show { id: active, size });
    }
}

/// The frame where it goes, or out of sight (at its size, so pages don't
/// lay out again).
fn put_frame(s: &Shared, flags: SET_WINDOW_POS_FLAGS) {
    let frame = HWND(s.frame as *mut _);
    unsafe {
        let _ = match s.rect.filter(|_| s.ready) {
            // Above the app's own page (its webview window is a sibling).
            Some((x, y, w, h)) => SetWindowPos(frame, Some(HWND_TOP), x, y, w, h, SWP_NOACTIVATE | flags),
            None => SetWindowPos(frame, None, AWAY, 0, 0, 0, SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOSIZE | flags),
        };
    }
}

/// Whether a page is seated (the frame shows), from the worker: the frame
/// is the UI thread's, so it's moved without waiting on it.
fn set_ready(ready: bool) {
    if let Some(s) = SHARED.lock().unwrap().as_mut() {
        s.ready = ready;
        if !ready {
            s.tx = None;
        }
        put_frame(s, SWP_ASYNCWINDOWPOS);
    }
}

/// A picture of tab `id` (or part of it), base64 JPEG.
pub fn capture(id: u64, clip: Option<(f64, f64, f64, f64)>) -> Option<String> {
    let tx = SHARED.lock().unwrap().as_ref()?.tx.clone()?;
    let (rtx, rrx) = mpsc::channel();
    tx.send(Cmd::Capture { id, clip, reply: rtx }).ok()?;
    rrx.recv_timeout(Duration::from_secs(4)).ok().flatten()
}

/// Run `js` in tab `id`'s page: its value.
pub fn eval(id: u64, js: &str) -> Option<Value> {
    let tx = SHARED.lock().unwrap().as_ref()?.tx.clone()?;
    let (rtx, rrx) = mpsc::channel();
    tx.send(Cmd::Eval { id, js: js.to_string(), reply: rtx }).ok()?;
    rrx.recv_timeout(Duration::from_secs(5)).ok().flatten()
}

/// The frame: a plain child window of the app's, over the Browser tab's
/// area, that Chrome's windows are seated in.
fn make_frame(main: isize) -> Option<isize> {
    unsafe {
        let inst: HINSTANCE = GetModuleHandleW(None).ok()?.into();
        static CLASS: OnceLock<u16> = OnceLock::new();
        CLASS.get_or_init(|| {
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(frame_proc),
                hInstance: inst,
                lpszClassName: w!("LarrierBrowserFrame"),
                hbrBackground: HBRUSH(GetStockObject(WHITE_BRUSH).0),
                ..Default::default()
            })
        });
        let frame = CreateWindowExW(
            WS_EX_NOPARENTNOTIFY,
            w!("LarrierBrowserFrame"),
            w!(""),
            WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
            AWAY,
            0,
            1200,
            800,
            Some(HWND(main as *mut _)),
            None,
            Some(inst),
            None,
        )
        .ok()?;
        FRAME.store(frame.0 as isize, Ordering::Relaxed);
        let _ = SetWindowSubclass(HWND(main as *mut _), Some(main_focus_proc), 0x4C52_4246, 0);
        Some(frame.0 as isize)
    }
}

/// The frame, for the focus guard (which runs inside window messages, so
/// it doesn't take `SHARED`'s lock).
static FRAME: AtomicIsize = AtomicIsize::new(0);
/// The page's window that had keyboard focus when the app was left.
static PAGE_FOCUS: AtomicIsize = AtomicIsize::new(0);
/// The app was just activated by a click.
static CLICKED: AtomicBool = AtomicBool::new(false);

/// Keyboard focus between the app and a seated page. Activating the app's
/// window hands its focus to the app's own webview (wry moves it there on
/// WM_SETFOCUS), which then took the typing meant for a page field and
/// put it in the last box used in the app. When the window is activated by
/// a click on the page, or comes back while the page had focus, the page
/// gets the focus instead.
unsafe extern "system" fn main_focus_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
    match msg {
        WM_ACTIVATE => {
            let state = (wp.0 & 0xFFFF) as u32;
            if state == WA_INACTIVE {
                let f = unsafe { GetFocus() };
                PAGE_FOCUS.store(if in_frame(f) { f.0 as isize } else { 0 }, Ordering::Relaxed);
            } else {
                CLICKED.store(state == WA_CLICKACTIVE, Ordering::Relaxed);
            }
        }
        WM_SETFOCUS => {
            if let Some(page) = page_to_focus() {
                unsafe {
                    let _ = SetFocus(Some(page));
                }
                return LRESULT(0);
            }
        }
        _ => {}
    }
    unsafe { DefSubclassProc(h, msg, wp, lp) }
}

/// A window of a seated page (inside the frame).
fn in_frame(h: HWND) -> bool {
    let frame = FRAME.load(Ordering::Relaxed);
    !h.is_invalid() && frame != 0 && unsafe { IsChild(HWND(frame as *mut _), h) }.as_bool()
}

/// Where focus goes as the app's window takes it: the page clicked, or the
/// page that had it, while a page shows. Otherwise the app's webview.
fn page_to_focus() -> Option<HWND> {
    let frame = HWND(FRAME.load(Ordering::Relaxed) as *mut _);
    if frame.is_invalid() {
        return None;
    }
    let clicked = CLICKED.swap(false, Ordering::Relaxed)
        || [VK_LBUTTON, VK_RBUTTON, VK_MBUTTON].iter().any(|k| unsafe { GetAsyncKeyState(k.0 as i32) } as u16 & 0x8000 != 0);
    let mut at = POINT::default();
    let r = rect(frame);
    let shown = r.left > AWAY / 2 && r.right > r.left && unsafe { IsWindowVisible(frame) }.as_bool();
    if !shown || unsafe { GetCursorPos(&mut at) }.is_err() {
        return None;
    }
    if clicked {
        let over = at.x >= r.left && at.x < r.right && at.y >= r.top && at.y < r.bottom;
        let target = unsafe { WindowFromPoint(at) };
        return (over && in_frame(target)).then_some(target);
    }
    // Kept until the app is left again: activating can set focus more than once.
    let had = HWND(PAGE_FOCUS.load(Ordering::Relaxed) as *mut _);
    (in_frame(had) && unsafe { IsWindowVisible(had) }.as_bool()).then_some(had)
}

unsafe extern "system" fn frame_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_DESTROY {
        // The app is closing: Chrome closes properly with it (its cookies
        // and the rest saved), not cut off.
        let _ = std::panic::catch_unwind(|| {
            let tx = SHARED.lock().ok().and_then(|g| g.as_ref().and_then(|s| s.tx.clone()));
            if let Some(tx) = tx {
                let (rtx, rrx) = mpsc::channel();
                if tx.send(Cmd::Quit { reply: rtx }).is_ok() {
                    let _ = rrx.recv_timeout(Duration::from_millis(400));
                }
            }
        });
    }
    unsafe { DefWindowProcW(h, msg, wp, lp) }
}

// ------------------------------------------------------------- the worker

struct Tab {
    target: String,
    session: String,
    window: Option<isize>,
    /// Chrome's own id for its window.
    window_id: Option<i64>,
    /// Where the page sits in its window, as last placed.
    insets: Option<(i32, i32, i32, i32)>,
    url: String,
    title: String,
}

struct Worker {
    cdp: Cdp,
    chrome: std::process::Child,
    frame: isize,
    tabs: HashMap<u64, Tab>,
    shown: Option<u64>,
    size: (i32, i32),
    events: UnboundedSender<Ev>,
    annotate_js: String,
    /// Targets Larrier made or let be, so ones it didn't are noticed.
    known: HashSet<String>,
    /// Pages opened inside a seated window (a link opening a tab, Ctrl+T),
    /// with when and where: once their address is known they leave it for
    /// a tab of their own.
    strays: HashMap<String, (Instant, String, Option<u64>)>,
    /// When the tabs' titles and addresses were last read whole.
    synced: Instant,
}

fn worker(rx: Receiver<Cmd>, events: UnboundedSender<Ev>, frame: isize, annotate_js: String) {
    let stopped = || set_ready(false);
    // The first command opens the first tab: Chrome starts with it.
    let (id0, url0) = match rx.recv() {
        Ok(Cmd::NewTab { id, url }) => (id, url),
        _ => return stopped(),
    };
    let size = SHARED.lock().unwrap().as_ref().map(|s| s.size).unwrap_or((1200, 800));
    let t0 = Instant::now();
    let (chrome, cdp) = match start(&url0) {
        Ok(v) => v,
        Err(error) => {
            let _ = events.send(Ev::Failed { id: id0, error });
            return stopped();
        }
    };
    let mut w = Worker { cdp, chrome, frame, tabs: HashMap::new(), shown: None, size, events, annotate_js, known: HashSet::new(), strays: HashMap::new(), synced: Instant::now() };
    let _ = w.cdp.call("Target.setDiscoverTargets", json!({"discover": true}), None);
    // Its first window, with the first page.
    let page = page_targets(&w.cdp).into_iter().next();
    let window = chrome_windows(w.chrome.id()).into_iter().next();
    match page {
        Some(t) => w.adopt(id0, &t, window),
        None => {
            let _ = w.events.send(Ev::Failed { id: id0, error: "Chrome didn't open a window.".into() });
        }
    }
    w.show(Some(id0));
    set_ready(true);
    tracing::info!(ms = t0.elapsed().as_millis() as u64, "Chrome started and its first page seated");
    loop {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(Cmd::Quit { reply }) => {
                w.cdp.send("Browser.close", json!({}), None);
                std::thread::sleep(Duration::from_millis(60));
                let _ = reply.send(());
                return;
            }
            Ok(cmd) => w.command(cmd),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        w.pump();
        // Chrome closed (or crashed): its tabs go with it.
        if !matches!(w.chrome.try_wait(), Ok(None)) {
            for id in w.tabs.keys() {
                set_target(*id, None);
                let _ = w.events.send(Ev::Gone { id: *id });
            }
            return stopped();
        }
    }
    let _ = w.cdp.call_for("Browser.close", json!({}), None, Duration::from_secs(2));
    stopped();
}

/// Start Larrier's Chrome on `url`: the process and a DevTools connection.
fn start(url: &str) -> Result<(std::process::Child, Cdp), String> {
    let exe = chrome_exe().ok_or("Google Chrome isn't installed.")?;
    let dir = profile();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // A Chrome of Larrier's left from an earlier run that didn't close it:
    // closed first, so the one running is this run's own.
    if let Some(ws) = chrome_running() {
        if let Ok(old) = Cdp::connect(&ws) {
            let _ = old.call_for("Browser.close", json!({}), None, Duration::from_secs(3));
        }
        let until = Instant::now() + Duration::from_secs(6);
        while chrome_running().is_some() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(150));
        }
    }
    let _ = std::fs::remove_file(dir.join("DevToolsActivePort"));
    let mut chrome = std::process::Command::new(exe)
        .arg(format!("--user-data-dir={}", dir.display()))
        .args([
            "--remote-debugging-port=0",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-sync",
            "--hide-crash-restore-bubble",
            // It's the user's own browsing too: pages aren't told it's
            // automated (sites refuse automated browsers).
            "--disable-blink-features=AutomationControlled",
            // Pages out of sight (other tabs, a closed panel) keep running:
            // agents work in them.
            "--disable-features=CalculateNativeWinOcclusion",
            "--disable-backgrounding-occluded-windows",
            "--disable-renderer-backgrounding",
            "--window-position=-32000,-32000",
            "--window-size=1200,800",
            "--new-window",
        ])
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Couldn't start Chrome: {e}"))?;
    let began = Instant::now();
    let until = began + Duration::from_secs(25);
    while Instant::now() < until {
        if !matches!(chrome.try_wait(), Ok(None)) {
            return Err("Chrome closed as it started.".into());
        }
        if let Some(ws) = chrome_running() {
            let port_at = began.elapsed();
            // Its window, once it has one.
            while Instant::now() < until && chrome_windows(chrome.id()).is_empty() {
                std::thread::sleep(Duration::from_millis(50));
            }
            tracing::info!(?port_at, window_at = ?began.elapsed(), "Chrome is up");
            return Cdp::connect(&ws).map(|c| (chrome, c));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = chrome.kill();
    Err("Chrome didn't start.".into())
}

fn page_targets(cdp: &Cdp) -> Vec<String> {
    let r = cdp.call("Target.getTargets", json!({}), None).unwrap_or(Value::Null);
    r["targetInfos"].as_array().into_iter().flatten().filter(|t| t["type"] == "page").filter_map(|t| t["targetId"].as_str().map(String::from)).collect()
}

/// Larrier's Chrome's own browser windows not seated yet (never anyone
/// else's; not its bubbles and menus, which have no resizable frame).
fn chrome_windows(pid: u32) -> Vec<isize> {
    struct Find {
        pid: u32,
        out: Vec<isize>,
    }
    unsafe extern "system" fn each(h: HWND, l: LPARAM) -> BOOL {
        let f = unsafe { &mut *(l.0 as *mut Find) };
        let mut p = 0u32;
        unsafe { GetWindowThreadProcessId(h, Some(&mut p)) };
        let framed = unsafe { GetWindowLongPtrW(h, GWL_STYLE) } & WS_THICKFRAME.0 as isize != 0;
        if p == f.pid && framed && unsafe { IsWindowVisible(h) }.as_bool() && class(h) == "Chrome_WidgetWin_1" {
            f.out.push(h.0 as isize);
        }
        BOOL(1)
    }
    let mut f = Find { pid, out: Vec::new() };
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut f as *mut Find as isize));
    }
    f.out
}

fn class(h: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn rect(h: HWND) -> RECT {
    let mut r = RECT::default();
    unsafe {
        let _ = GetWindowRect(h, &mut r);
    }
    r
}

/// How far the page area (the page, and DevTools when docked) sits inside a
/// Chrome window: left, top, right, bottom.
fn insets(win: HWND) -> Option<(i32, i32, i32, i32)> {
    struct Find(Option<RECT>);
    unsafe extern "system" fn each(h: HWND, l: LPARAM) -> BOOL {
        let f = unsafe { &mut *(l.0 as *mut Find) };
        if class(h) == "Chrome_RenderWidgetHostHWND" && unsafe { IsWindowVisible(h) }.as_bool() {
            let r = rect(h);
            f.0 = Some(match f.0 {
                Some(u) => RECT { left: u.left.min(r.left), top: u.top.min(r.top), right: u.right.max(r.right), bottom: u.bottom.max(r.bottom) },
                None => r,
            });
        }
        BOOL(1)
    }
    let mut f = Find(None);
    unsafe {
        let _ = EnumChildWindows(Some(win), Some(each), LPARAM(&mut f as *mut Find as isize));
    }
    let (o, i) = (rect(win), f.0?);
    (i.right > i.left).then_some((i.left - o.left, i.top - o.top, o.right - i.right, o.bottom - i.bottom))
}

/// An address that is no page of its own yet.
fn unsettled(url: &str) -> bool {
    url.is_empty() || url == "about:blank"
}

impl Worker {
    fn session(&self, id: u64) -> Option<String> {
        self.tabs.get(&id).map(|t| t.session.clone())
    }

    fn call(&self, id: u64, method: &str, params: Value) -> Option<Value> {
        let s = self.session(id)?;
        self.cdp.call(method, params, Some(&s)).ok()
    }

    /// Make target `target` Larrier tab `id`: attach, tell its pages which
    /// tab they are in, and seat its window in the frame.
    fn adopt(&mut self, id: u64, target: &str, window: Option<isize>) {
        self.known.insert(target.to_string());
        let Ok(r) = self.cdp.call("Target.attachToTarget", json!({"targetId": target, "flatten": true}), None) else {
            let _ = self.events.send(Ev::Failed { id, error: "Chrome didn't open the tab.".into() });
            return;
        };
        let Some(session) = r["sessionId"].as_str().map(String::from) else { return };
        let mark = format!("Object.defineProperty(window, '__larrierTab', {{ value: {id}, configurable: true }});");
        for (m, p) in [
            ("Page.enable", json!({})),
            // Its binding's calls are reported only with Runtime on.
            ("Runtime.enable", json!({})),
            ("Runtime.addBinding", json!({"name": "__larrierPost"})),
            ("Page.addScriptToEvaluateOnNewDocument", json!({"source": mark})),
            ("Page.addScriptToEvaluateOnNewDocument", json!({"source": self.annotate_js})),
            ("Runtime.evaluate", json!({"expression": mark})),
            ("Runtime.evaluate", json!({"expression": self.annotate_js})),
        ] {
            let _ = self.cdp.call(m, p, Some(&session));
        }
        set_target(id, Some(target));
        let window_id = self.cdp.call("Browser.getWindowForTarget", json!({"targetId": target}), None).ok().and_then(|v| v["windowId"].as_i64());
        self.tabs.insert(id, Tab { target: target.to_string(), session, window, window_id, insets: None, url: String::new(), title: String::new() });
        if let Some(h) = window {
            self.seat(h);
        }
        // Where it is and what it's called.
        if let Ok(info) = self.cdp.call("Target.getTargetInfo", json!({"targetId": target}), None) {
            self.news(id, &info["targetInfo"]);
        }
    }

    /// A tab's address or title changed: tell the UI.
    fn news(&mut self, id: u64, info: &Value) {
        let Some(t) = self.tabs.get_mut(&id) else { return };
        let (url, title) = (info["url"].as_str().unwrap_or(""), info["title"].as_str().unwrap_or(""));
        if url != t.url {
            t.url = url.to_string();
            let _ = self.events.send(Ev::Url { id, url: url.to_string() });
        }
        if title != t.title {
            t.title = title.to_string();
            let _ = self.events.send(Ev::Title { id, title: title.to_string() });
        }
    }

    /// Put a Chrome window in the frame: a child, out of sight until shown.
    fn seat(&self, h: isize) {
        let hw = HWND(h as *mut _);
        unsafe {
            let _ = SetParent(hw, Some(HWND(self.frame as *mut _)));
            SetWindowLongPtrW(hw, GWL_STYLE, (WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS | WS_CLIPCHILDREN).0 as isize);
            let ex = GetWindowLongPtrW(hw, GWL_EXSTYLE);
            SetWindowLongPtrW(hw, GWL_EXSTYLE, (ex & !(WS_EX_APPWINDOW.0 as isize)) | (WS_EX_TOOLWINDOW.0 | WS_EX_NOPARENTNOTIFY.0) as isize);
            let _ = SetWindowPos(hw, None, AWAY, 0, self.size.0, self.size.1, SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED | SWP_SHOWWINDOW);
        }
    }

    /// Where the page sits in tab `id`'s window: measured, or worked out
    /// from the page's own sizes.
    fn measure(&self, id: u64, win: HWND) -> (i32, i32, i32, i32) {
        if let Some(i) = insets(win) {
            return i;
        }
        let v = self.call(id, "Runtime.evaluate", json!({"expression": "[outerWidth, outerHeight, innerWidth, innerHeight, devicePixelRatio]", "returnByValue": true}));
        let n: Vec<f64> = v.and_then(|v| v["result"]["value"].as_array().cloned()).unwrap_or_default().iter().filter_map(|x| x.as_f64()).collect();
        match n[..] {
            [ow, oh, iw, ih, d] if ow > iw => {
                let side = ((ow - iw) * d / 2.0).round() as i32;
                (side, ((oh - ih) * d).round() as i32 - side, side, side)
            }
            _ => (8, 33, 7, 7),
        }
    }

    /// Lay the windows out: the one shown with its page filling the frame
    /// (the rest of the window outside it), the others out of sight at the
    /// same size (their pages keep the shown one's size).
    fn show(&mut self, id: Option<u64>) {
        if id.is_some() {
            self.shown = id;
        }
        let (fw, fh) = self.size;
        let ids: Vec<u64> = self.tabs.keys().copied().collect();
        for tid in ids {
            let Some(h) = self.tabs[&tid].window else { continue };
            let hw = HWND(h as *mut _);
            let shown = Some(tid) == self.shown;
            // Twice when shown: the insets are measured at the window's size.
            for _ in 0..if shown { 2 } else { 1 } {
                let (l, t, r, b) = if shown { self.measure(tid, hw) } else { self.tabs[&tid].insets.unwrap_or((8, 33, 7, 7)) };
                let x = if shown { -l } else { AWAY };
                unsafe {
                    let _ = SetWindowPos(hw, None, x, -t, fw + l + r, fh + t + b, SWP_NOZORDER | SWP_NOACTIVATE);
                }
                if let Some(tab) = self.tabs.get_mut(&tid) {
                    tab.insets = Some((l, t, r, b));
                }
            }
        }
    }

    fn command(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::NewTab { id, url } => {
                let before: HashSet<isize> = chrome_windows(self.chrome.id()).into_iter().collect();
                // Opened out of sight, seated before it's ever seen.
                let (w, h) = (self.size.0 + 16, self.size.1 + 120);
                let made = self.cdp.call("Target.createTarget", json!({"url": url, "newWindow": true, "background": true, "left": -32000, "top": -32000, "width": w, "height": h}), None);
                let Some(target) = made.ok().and_then(|r| r["targetId"].as_str().map(String::from)) else {
                    let _ = self.events.send(Ev::Failed { id, error: "Chrome couldn't open a tab.".into() });
                    return;
                };
                self.known.insert(target.clone());
                // Its window: the one that wasn't there before.
                let until = Instant::now() + Duration::from_secs(5);
                let mut win = None;
                while win.is_none() && Instant::now() < until {
                    win = chrome_windows(self.chrome.id()).into_iter().find(|h| !before.contains(h));
                    if win.is_none() {
                        std::thread::sleep(Duration::from_millis(15));
                    }
                }
                self.adopt(id, &target, win);
                self.show(Some(id));
            }
            Cmd::Close { id } => {
                if let Some(t) = self.tabs.remove(&id) {
                    let _ = self.cdp.call("Target.closeTarget", json!({"targetId": t.target}), None);
                }
                set_target(id, None);
            }
            Cmd::Navigate { id, url } => {
                self.call(id, "Page.navigate", json!({"url": url}));
            }
            Cmd::History { id, back } => {
                if let Some(h) = self.call(id, "Page.getNavigationHistory", json!({})) {
                    let i = h["currentIndex"].as_i64().unwrap_or(0) + if back { -1 } else { 1 };
                    if let Some(e) = h["entries"].as_array().and_then(|e| e.get(i.max(0) as usize)).filter(|_| i >= 0) {
                        self.call(id, "Page.navigateToHistoryEntry", json!({"entryId": e["id"]}));
                    }
                }
            }
            Cmd::Reload { id } => {
                self.call(id, "Page.reload", json!({}));
            }
            Cmd::Stop { id } => {
                self.call(id, "Page.stopLoading", json!({}));
            }
            Cmd::Script { id, js } => {
                self.call(id, "Runtime.evaluate", json!({"expression": js, "userGesture": true}));
            }
            Cmd::DevTools { id } => {
                // Chrome's own DevTools for the page, in a tab beside it.
                let at = chrome_running().and_then(|ws| ws.split("//").nth(1).and_then(|r| r.split('/').next()).map(String::from));
                if let (Some(t), Some(at)) = (self.tabs.get(&id), at) {
                    let url = format!("devtools://devtools/bundled/inspector.html?ws={at}/devtools/page/{}", t.target);
                    let _ = self.events.send(Ev::Open { url, from: Some(id) });
                }
            }
            Cmd::Adopt { id, target } => {
                // Its window: the one of Chrome's not seated yet.
                let seated: HashSet<isize> = self.tabs.values().filter_map(|t| t.window).collect();
                let until = Instant::now() + Duration::from_secs(3);
                let mut win = None;
                while win.is_none() && Instant::now() < until {
                    win = chrome_windows(self.chrome.id()).into_iter().find(|h| !seated.contains(h));
                    if win.is_none() {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                }
                self.adopt(id, &target, win);
                self.show(Some(id));
            }
            Cmd::Show { id, size } => {
                if let Some((w, h)) = size {
                    self.size = (w.max(1), h.max(1));
                }
                self.shown = id;
                self.show(None);
            }
            Cmd::Capture { id, clip, reply } => {
                let mut params = json!({"format": "jpeg", "quality": 85});
                if let Some((x, y, w, h)) = clip {
                    params["clip"] = json!({"x": x.max(0.0), "y": y.max(0.0), "width": w.max(1.0), "height": h.max(1.0), "scale": 1});
                }
                let shot = self.session(id).and_then(|s| self.cdp.call_for("Page.captureScreenshot", params, Some(&s), Duration::from_secs(3)).ok());
                let _ = reply.send(shot.and_then(|v| v["data"].as_str().map(String::from)));
            }
            Cmd::Eval { id, js, reply } => {
                let v = self.session(id).and_then(|s| self.cdp.call_for("Runtime.evaluate", json!({"expression": js, "returnByValue": true}), Some(&s), Duration::from_secs(4)).ok());
                let _ = reply.send(v.map(|v| v["result"]["value"].clone()));
            }
            Cmd::Quit { .. } => {}
        }
    }

    /// Chrome's news: loads, titles, addresses, annotate messages, tabs
    /// closed from the page, and pages Larrier didn't open (a link opening
    /// a tab) becoming tabs of their own.
    fn pump(&mut self) {
        let by_session: HashMap<String, u64> = self.tabs.iter().map(|(id, t)| (t.session.clone(), *id)).collect();
        let by_target: HashMap<String, u64> = self.tabs.iter().map(|(id, t)| (t.target.clone(), *id)).collect();
        for e in self.cdp.drain() {
            let p = &e.params;
            let tab = e.session.as_ref().and_then(|s| by_session.get(s)).copied();
            let main_frame = |id: u64| p["frameId"].as_str() == self.tabs.get(&id).map(|t| t.target.as_str());
            match (e.method.as_str(), tab) {
                ("Page.frameStartedLoading", Some(id)) if main_frame(id) => {
                    let _ = self.events.send(Ev::Load { id, started: true, url: String::new() });
                }
                ("Page.frameStoppedLoading", Some(id)) if main_frame(id) => {
                    // Its title as the page set it (Chrome names it by its
                    // address until then).
                    if let Some(target) = self.tabs.get(&id).map(|t| t.target.clone()) {
                        if let Ok(info) = self.cdp.call("Target.getTargetInfo", json!({"targetId": target}), None) {
                            self.news(id, &info["targetInfo"]);
                        }
                    }
                    let url = self.tabs.get(&id).map(|t| t.url.clone()).unwrap_or_default();
                    let _ = self.events.send(Ev::Load { id, started: false, url });
                    // Chrome's "can't be reached" page: Larrier shows its own
                    // instead (a certificate warning stays Chrome's: it can
                    // be passed from there).
                    let probe = "location.href.startsWith('chrome-error://') ? ((document.querySelector('.error-code') || {}).textContent || 'ERR_FAILED').trim() : ''";
                    let code = self
                        .call(id, "Runtime.evaluate", json!({"expression": probe, "returnByValue": true}))
                        .and_then(|v| v["result"]["value"].as_str().map(String::from))
                        .filter(|c| !c.is_empty() && !c.contains("CERT"));
                    let _ = self.events.send(Ev::PageError { id, code });
                    if let Some(h) = self.call(id, "Page.getNavigationHistory", json!({})) {
                        let i = h["currentIndex"].as_u64().unwrap_or(0) as usize;
                        let n = h["entries"].as_array().map(|e| e.len()).unwrap_or(0);
                        let _ = self.events.send(Ev::Nav { id, back: i > 0, forward: i + 1 < n });
                    }
                }
                // A real page took the tab: whatever couldn't load before is gone.
                ("Page.frameNavigated", Some(id)) if p["frame"]["parentId"].is_null() && !p["frame"]["url"].as_str().unwrap_or("").starts_with("chrome-error://") => {
                    let _ = self.events.send(Ev::PageError { id, code: None });
                }
                ("Runtime.bindingCalled", Some(id)) if p["name"] == "__larrierPost" => {
                    let _ = self.events.send(Ev::Annotate { id, body: p["payload"].as_str().unwrap_or("").to_string() });
                }
                ("Target.targetInfoChanged", _) => {
                    let i = &p["targetInfo"];
                    let t = i["targetId"].as_str().unwrap_or("");
                    if let Some(id) = by_target.get(t).copied() {
                        self.news(id, i);
                    } else if let Some(s) = self.strays.get_mut(t) {
                        s.1 = i["url"].as_str().unwrap_or("").to_string();
                    }
                }
                ("Target.targetCreated", _) => {
                    let i = &p["targetInfo"];
                    let Some(t) = i["targetId"].as_str().filter(|_| i["type"] == "page") else { continue };
                    if self.known.contains(t) {
                        continue;
                    }
                    // In one of the seated windows (a tab of its own soon),
                    // or a window of its own (a sign-in popup: left to be).
                    let window = self.cdp.call("Browser.getWindowForTarget", json!({"targetId": t}), None).ok().and_then(|v| v["windowId"].as_i64());
                    let from = window.and_then(|_| self.tabs.iter().find(|(_, x)| x.window_id == window).map(|(id, _)| *id));
                    let url = i["url"].as_str().unwrap_or("").to_string();
                    if let Some(from) = from {
                        self.strays.insert(t.to_string(), (Instant::now(), url, Some(from)));
                    } else {
                        // A window of its own (a sign-in popup, Ctrl+N): it
                        // becomes a tab as it is, keeping what opened it.
                        self.known.insert(t.to_string());
                        let prerender = i["subtype"].as_str().is_some_and(|s| !s.is_empty());
                        if !prerender && !url.starts_with("devtools://") {
                            let opener = i["openerId"].as_str().and_then(|o| by_target.get(o)).copied().or(self.shown);
                            let _ = self.events.send(Ev::Popup { target: t.to_string(), url, from: opener });
                        }
                    }
                }
                ("Target.targetDestroyed", _) => {
                    let t = p["targetId"].as_str().unwrap_or("");
                    self.strays.remove(t);
                    if let Some(id) = by_target.get(t).copied() {
                        if self.tabs.remove(&id).is_some() {
                            set_target(id, None);
                            let _ = self.events.send(Ev::Gone { id });
                        }
                    }
                }
                _ => {}
            }
        }
        // Strays whose address is known (or that waited long enough) leave.
        let ready: Vec<(String, String, Option<u64>)> =
            self.strays.iter().filter(|(_, (since, url, _))| !unsettled(url) || since.elapsed() > Duration::from_millis(1500)).map(|(t, (_, u, f))| (t.clone(), u.clone(), *f)).collect();
        for (t, url, from) in ready {
            self.strays.remove(&t);
            self.known.insert(t.clone());
            // Docked DevTools is part of its window.
            if url.starts_with("devtools://") {
                continue;
            }
            let _ = self.cdp.call("Target.closeTarget", json!({"targetId": t}), None);
            let _ = self.events.send(Ev::Open { url: if unsettled(&url) { "chrome://newtab/".into() } else { url }, from });
        }
        // Every tab's title and address, read whole now and then: a page
        // that loaded before Larrier attached sends no news of its own.
        if self.synced.elapsed() > Duration::from_millis(1500) {
            self.synced = Instant::now();
            let infos = self.cdp.call_for("Target.getTargets", json!({}), None, Duration::from_secs(2)).ok();
            for i in infos.as_ref().and_then(|v| v["targetInfos"].as_array()).into_iter().flatten() {
                if let Some(id) = i["targetId"].as_str().and_then(|t| by_target.get(t)).copied() {
                    self.news(id, i);
                }
            }
        }
        // The page moved in its window (an infobar, DevTools docked): follow.
        if let Some(id) = self.shown {
            if let Some((h, was)) = self.tabs.get(&id).and_then(|t| Some((t.window?, t.insets))) {
                if insets(HWND(h as *mut _)).is_some_and(|now| Some(now) != was) {
                    self.show(None);
                }
            }
        }
    }
}
