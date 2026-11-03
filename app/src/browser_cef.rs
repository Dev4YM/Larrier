//! Larrier's browser engine: Chromium, run by Larrier itself through the
//! Chromium Embedded Framework, the way an Electron app runs its views. Each
//! page tab is a Chromium browser made as a child window of a frame laid
//! over the Browser tab's area. Larrier does what a browser's own chrome
//! would: it shows its own page for an address that can't be reached,
//! opens pop-ups and new windows as tabs, and DevTools in a tab; agents
//! drive the very same pages over the engine's DevTools port.
//!
//! The engine's runtime (libcef.dll and its files, about 410 MB) isn't part
//! of Larrier's program: it's fetched once per engine version into the
//! user's `%LOCALAPPDATA%\Larrier\engine\<version>` and loaded on first use
//! (libcef.dll is delay-loaded). Its helper processes are this program,
//! started with `--type=…` (see [`subprocess`]).

#![cfg(windows)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use cef::*;
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;
use windows::core::{w, HSTRING};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetStockObject, HBRUSH, WHITE_BRUSH};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, SetDllDirectoryW};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, GetFocus, SetFocus, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, PostMessageW, GetCursorPos, GetWindowThreadProcessId, GetWindowRect, IsChild, IsWindowVisible, RegisterClassW, SetWindowPos, WindowFromPoint, HWND_TOP,
    SET_WINDOW_POS_FLAGS, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, WA_CLICKACTIVE, WA_INACTIVE, WM_ACTIVATE, WM_CLOSE, WM_DESTROY, WM_SETFOCUS,
    WNDCLASSW, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_NOPARENTNOTIFY, WS_VISIBLE,
};

use crate::browser_host::{Area, Ev};

// ------------------------------------------------------------- the engine

/// The engine this build runs on: the runtime version it needs.
pub fn version() -> &'static str {
    static V: OnceLock<String> = OnceLock::new();
    V.get_or_init(|| {
        let raw = String::from_utf8_lossy(sys::CEF_VERSION).trim_end_matches('\0').to_string();
        raw.split('+').next().unwrap_or(&raw).to_string()
    })
}

/// Where engine runtimes are kept: the user's own Larrier folder, which
/// every install can write to (one for everyone can't write beside the
/// program).
pub fn root() -> PathBuf {
    crate::install::layout(crate::install::Scope::User).shared_dir.join("engine")
}

/// Where this build's engine runtime is: `<root>/<version>`
/// (`LARRIER_CEF_DIR` points elsewhere, for development).
pub fn dir() -> PathBuf {
    if let Some(d) = std::env::var_os("LARRIER_CEF_DIR") {
        return d.into();
    }
    root().join(version())
}

/// Remove runtimes of older engines, and unfinished downloads (at
/// startup, before the engine runs).
pub fn tidy() {
    let Ok(list) = std::fs::read_dir(root()) else { return };
    let ours = crate::version::Version::parse(version());
    for e in list.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let older = match (crate::version::Version::parse(&name), &ours) {
            (Some(v), Some(o)) => v < *o,
            _ => false,
        };
        if older || name.ends_with(".unpacking") || name.ends_with(".zip.part") {
            let p = e.path();
            let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
        }
    }
}

/// The engine's runtime is here.
pub fn present() -> bool {
    dir().join("libcef.dll").is_file()
}

/// Its profile: cookies, sign-ins, cache, and the DevTools port file the
/// agents' tools read.
pub fn profile() -> PathBuf {
    larrier_agents::browser::engine_dir()
}

/// Let libcef.dll (delay-loaded) be found where the runtime is.
fn load_from_dir() {
    let d = dir();
    unsafe {
        let _ = SetDllDirectoryW(&HSTRING::from(d.as_os_str()));
    }
}

/// One of the engine's helper processes (renderer, GPU, …): this program
/// started with `--type=…`. Its exit code.
pub fn subprocess() -> i32 {
    if !present() {
        return 1;
    }
    load_from_dir();
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    let args = args::Args::new();
    execute_process(Some(args.as_main_args()), None, std::ptr::null_mut())
}

pub fn is_subprocess(args: &[String]) -> bool {
    args.iter().any(|a| a.starts_with("--type="))
}

wrap_app! {
    struct EngineApp;

    impl App {
        fn on_before_command_line_processing(&self, _process_type: Option<&CefString>, command_line: Option<&mut CommandLine>) {
            let Some(cl) = command_line else { return };
            // Pages out of sight (other tabs, a closed panel) keep running:
            // agents work in them.
            cl.append_switch_with_value(Some(&CefString::from("disable-features")), Some(&CefString::from("CalculateNativeWinOcclusion")));
            cl.append_switch(Some(&CefString::from("disable-renderer-backgrounding")));
            cl.append_switch(Some(&CefString::from("disable-background-timer-throttling")));
            cl.append_switch(Some(&CefString::from("disable-backgrounding-occluded-windows")));
            // Larrier's own browser: not the extensions other programs
            // register for Google Chrome on this computer.
            cl.append_switch(Some(&CefString::from("disable-extensions")));
        }
    }
}

static STARTED: OnceLock<bool> = OnceLock::new();

/// Start the engine (on the UI thread, once): whether it runs.
fn start() -> bool {
    *STARTED.get_or_init(|| match init() {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!("browser engine: {e}");
            emit(Ev::EngineFailed { error: e });
            false
        }
    })
}

fn init() -> Result<(), String> {
    if !present() {
        return Err("The browser engine isn't installed yet.".into());
    }
    load_from_dir();
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    let args = args::Args::new();
    let profile = profile();
    std::fs::create_dir_all(&profile).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(profile.join("DevToolsActivePort"));
    let port = free_port();
    let d = dir();
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let s = |p: &std::path::Path| CefString::from(p.to_string_lossy().as_ref());
    let settings = Settings {
        no_sandbox: 1,
        multi_threaded_message_loop: 1,
        browser_subprocess_path: s(&exe),
        root_cache_path: s(&profile),
        cache_path: s(&profile),
        persist_session_cookies: 1,
        resources_dir_path: s(&d),
        locales_dir_path: s(&d.join("locales")),
        log_file: s(&profile.join("engine.log")),
        log_severity: LogSeverity::WARNING,
        remote_debugging_port: port as i32,
        background_color: 0xFFFF_FFFF,
        ..Default::default()
    };
    let mut app = EngineApp::new();
    if initialize(Some(args.as_main_args()), Some(&settings), Some(&mut app), std::ptr::null_mut()) != 1 {
        return Err("The browser engine didn't start.".into());
    }
    // The agents' tools find the pages through the DevTools port.
    std::thread::spawn(move || announce(port, profile));
    Ok(())
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()).map(|a| a.port()).unwrap_or(9229)
}

/// Write the DevTools port file once the engine answers on its port.
fn announce(port: u16, profile: PathBuf) {
    let until = Instant::now() + Duration::from_secs(20);
    while Instant::now() < until {
        if let Ok(mut s) = std::net::TcpStream::connect(("127.0.0.1", port)) {
            use std::io::{Read, Write};
            let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
            let _ = s.write_all(format!("GET /json/version HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n").as_bytes());
            let mut body = String::new();
            let _ = s.read_to_string(&mut body);
            if let Some(v) = body.split("\r\n\r\n").nth(1).and_then(|b| serde_json::from_str::<Value>(b.trim()).ok()) {
                if let Some(ws) = v["webSocketDebuggerUrl"].as_str() {
                    let path = ws.splitn(4, '/').nth(3).map(|p| format!("/{p}")).unwrap_or_default();
                    let _ = std::fs::write(profile.join("DevToolsActivePort"), format!("{port}\n{path}"));
                    return;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

// ------------------------------------------------------------- fetching it

/// The engine's runtime for this version, published as a release of its
/// own (it changes far less often than Larrier).
fn source() -> (String, String) {
    let v = version();
    let base = format!("https://github.com/{}/releases/download/engine-{v}/Larrier-engine-{v}-windows-x64.zip", crate::update::repo());
    (base.clone(), format!("{base}.sha256"))
}

static FETCHING: AtomicBool = AtomicBool::new(false);

/// Fetch the runtime if it isn't here (in the background, once at a time),
/// reporting progress, then start what waited for it.
pub fn ensure() {
    if present() || FETCHING.swap(true, Ordering::SeqCst) {
        return;
    }
    dioxus::core::spawn_forever(async {
        let r = fetch(|done, total| emit(Ev::EngineProgress { done, total })).await;
        FETCHING.store(false, Ordering::SeqCst);
        match r {
            Ok(()) => emit(Ev::EngineReady),
            Err(e) => emit(Ev::EngineFailed { error: format!("Couldn't get the browser engine: {e}") }),
        }
    });
}

async fn fetch(progress: impl Fn(u64, u64)) -> Result<(), String> {
    use sha2::Digest;
    let (url, sha_url) = source();
    let client = reqwest::Client::builder().user_agent(concat!("Larrier/", env!("CARGO_PKG_VERSION"))).build().map_err(|e| e.to_string())?;
    let want = client.get(&sha_url).send().await.map_err(|e| e.to_string())?.error_for_status().map_err(|e| e.to_string())?.text().await.map_err(|e| e.to_string())?;
    let want = want.split_whitespace().next().unwrap_or("").to_lowercase();
    let mut resp = client.get(&url).send().await.map_err(|e| e.to_string())?.error_for_status().map_err(|e| e.to_string())?;
    let total = resp.content_length().unwrap_or(0);
    let target = dir();
    let parent = target.parent().map(|p| p.to_path_buf()).ok_or("no folder for the engine")?;
    std::fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
    let zip = parent.join(format!("{}.zip.part", version()));
    let mut file = std::fs::File::create(&zip).map_err(|e| e.to_string())?;
    let mut hash = sha2::Sha256::new();
    let mut done = 0u64;
    let mut last = Instant::now();
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        use std::io::Write;
        file.write_all(&chunk).map_err(|e| e.to_string())?;
        hash.update(&chunk);
        done += chunk.len() as u64;
        if last.elapsed() > Duration::from_millis(150) {
            last = Instant::now();
            progress(done, total);
        }
    }
    drop(file);
    let got = format!("{:x}", hash.finalize());
    if got != want {
        let _ = std::fs::remove_file(&zip);
        return Err("the download doesn't match its published checksum".into());
    }
    progress(total, total);
    // Unpacked beside its final place, then moved in whole.
    let tmp = parent.join(format!("{}.unpacking", version()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let (z, t) = (zip.clone(), tmp.clone());
    let ok = tokio::task::spawn_blocking(move || {
        let mut c = std::process::Command::new("tar");
        c.arg("-xf").arg(&z).arg("-C").arg(&t);
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000);
        }
        c.status().map(|s| s.success()).unwrap_or(false)
    })
    .await
    .unwrap_or(false);
    let _ = std::fs::remove_file(&zip);
    if !ok || !tmp.join("libcef.dll").is_file() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err("the engine couldn't be unpacked".into());
    }
    let _ = std::fs::remove_dir_all(&target);
    std::fs::rename(&tmp, &target).map_err(|e| e.to_string())?;
    Ok(())
}

// ------------------------------------------------------------- the pages

#[derive(Default)]
struct Page {
    browser: Option<Browser>,
    hwnd: isize,
    /// Its DevTools target (for the agents' tools), once asked.
    target: Option<String>,
    /// Keeps its DevTools messages coming.
    observer: Option<Registration>,
}

struct Shared {
    frame: isize,
    /// Where the frame goes (x, y, width, height), when a page shows.
    rect: Option<(i32, i32, i32, i32)>,
    size: (i32, i32),
    shown: Option<u64>,
}

static PAGES: Mutex<Option<HashMap<u64, Page>>> = Mutex::new(None);
static SHARED: Mutex<Option<Shared>> = Mutex::new(None);
static EVENTS: OnceLock<UnboundedSender<Ev>> = OnceLock::new();
static ANNOTATE: OnceLock<String> = OnceLock::new();
static MAIN: AtomicIsize = AtomicIsize::new(0);
/// Commands that came before the engine was there.
static QUEUED: Mutex<Vec<Cmd>> = Mutex::new(Vec::new());
/// Tabs the engine opens itself (pop-ups, DevTools): ids of their own.
static NEXT_ID: AtomicU64 = AtomicU64::new(1 << 40);

fn pages<R>(f: impl FnOnce(&mut HashMap<u64, Page>) -> R) -> R {
    let mut g = PAGES.lock().unwrap();
    f(g.get_or_insert_with(HashMap::new))
}

fn emit(ev: Ev) {
    if let Some(tx) = EVENTS.get() {
        let _ = tx.send(ev);
    }
}

/// Off to the side: still rendering, out of sight.
const AWAY: i32 = -40_000;

/// Called by the browser host first: the app's window, where pages' news
/// goes, and the annotate script.
pub fn setup(main: isize, events: UnboundedSender<Ev>, annotate_js: String) {
    MAIN.store(main, Ordering::Relaxed);
    let _ = EVENTS.set(events);
    let _ = ANNOTATE.set(annotate_js);
}

/// What the UI asks of the engine.
pub enum Cmd {
    NewTab { id: u64, url: String },
    Close { id: u64 },
    Navigate { id: u64, url: String },
    History { id: u64, back: bool },
    Reload { id: u64 },
    Stop { id: u64 },
    DevTools { id: u64 },
    Script { id: u64, js: String },
}

/// Do `cmd` (from the UI thread): the engine starts with the first, once
/// its runtime is here; until then commands wait.
pub fn send(cmd: Cmd) {
    if !present() {
        QUEUED.lock().unwrap().push(cmd);
        ensure();
        return;
    }
    if !ensure_frame() || !start() {
        return;
    }
    on_ui(move || run(cmd));
}

/// The runtime arrived: run what waited for it (UI thread).
pub fn flush() {
    let cmds: Vec<Cmd> = std::mem::take(&mut *QUEUED.lock().unwrap());
    for c in cmds {
        send(c);
    }
}

type Job = Box<dyn FnOnce() + Send>;

wrap_task! {
    struct RunJob {
        job: Arc<Mutex<Option<Job>>>,
    }

    impl Task {
        fn execute(&self) {
            if let Some(job) = self.job.lock().unwrap().take() {
                job();
            }
        }
    }
}

/// Run `f` on the engine's UI thread (where browsers are made and driven).
fn on_ui(f: impl FnOnce() + Send + 'static) {
    let mut t = RunJob::new(Arc::new(Mutex::new(Some(Box::new(f) as Job))));
    post_task(ThreadId::UI, Some(&mut t));
}

fn browser(id: u64) -> Option<Browser> {
    pages(|p| p.get(&id).and_then(|x| x.browser.clone()))
}

fn child_info() -> WindowInfo {
    let frame = SHARED.lock().unwrap().as_ref().map(|s| (s.frame, s.size)).unwrap_or((0, (1200, 800)));
    let parent = sys::HWND(frame.0 as *mut _);
    WindowInfo { runtime_style: RuntimeStyle::ALLOY, ..Default::default() }.set_as_child(parent, &Rect { x: AWAY, y: 0, width: frame.1 .0, height: frame.1 .1 })
}

fn run(cmd: Cmd) {
    match cmd {
        Cmd::NewTab { id, url } => {
            pages(|p| {
                p.entry(id).or_default();
            });
            let mut client = PageClient::new(id);
            browser_host_create_browser(Some(&child_info()), Some(&mut client), Some(&CefString::from(url.as_str())), Some(&BrowserSettings::default()), None, None);
        }
        Cmd::Close { id } => {
            if let Some(b) = pages(|p| p.remove(&id)).and_then(|x| x.browser) {
                if let Some(h) = b.host() {
                    h.close_browser(1);
                }
            }
        }
        Cmd::Navigate { id, url } => {
            if let Some(f) = browser(id).and_then(|b| b.main_frame()) {
                f.load_url(Some(&CefString::from(url.as_str())));
            }
        }
        Cmd::History { id, back } => {
            if let Some(b) = browser(id) {
                if back {
                    b.go_back();
                } else {
                    b.go_forward();
                }
            }
        }
        Cmd::Reload { id } => {
            if let Some(b) = browser(id) {
                b.reload();
            }
        }
        Cmd::Stop { id } => {
            if let Some(b) = browser(id) {
                b.stop_load();
            }
        }
        Cmd::DevTools { id } => {
            // Chromium's DevTools for the page, in a tab beside it.
            let Some(host) = browser(id).and_then(|b| b.host()) else { return };
            let tools = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            pages(|p| {
                p.entry(tools).or_default();
            });
            emit(Ev::Popup { id: tools, url: "devtools://devtools".into(), from: Some(id), background: false });
            let mut client = PageClient::new(tools);
            host.show_dev_tools(Some(&child_info()), Some(&mut client), Some(&BrowserSettings::default()), None);
        }
        Cmd::Script { id, js } => {
            if let Some(f) = browser(id).and_then(|b| b.main_frame()) {
                f.execute_java_script(Some(&CefString::from(js.as_str())), Some(&CefString::from("")), 0);
            }
        }
    }
}

/// A browser was made for tab `id`: keep it, wire its DevTools messages,
/// mark its pages with their tab and the annotate script, and lay it out.
fn created(id: u64, browser: Option<Browser>) {
    let Some(b) = browser else { return };
    let Some(host) = b.host() else { return };
    let hwnd = host.window_handle().0 as isize;
    let mut tools = PageTools::new(id);
    let observer = host.add_dev_tools_message_observer(Some(&mut tools));
    let gone = pages(|p| match p.get_mut(&id) {
        Some(page) => {
            page.browser = Some(b.clone());
            page.hwnd = hwnd;
            page.observer = observer;
            false
        }
        None => true,
    });
    if gone {
        // Its tab closed while it was being made.
        host.close_browser(1);
        return;
    }
    lay_out();
}

fn mark_js(id: u64) -> String {
    format!("Object.defineProperty(window, '__larrierTab', {{ value: {id}, configurable: true }});")
}

/// What reaches Larrier from a page's annotate script: a console message
/// starting with this (taken, never shown).
const POST: &str = "\u{1}larrier-post:";

/// Larrier's scripts for a page of tab `id`, run as it starts: its tab, the
/// annotate script, and the way that reports back (the console).
fn page_js(id: u64) -> String {
    let post = "if (!window.__larrierPost) Object.defineProperty(window, '__larrierPost', { value: (s) => console.debug('\\u0001larrier-post:' + s), configurable: true });";
    format!("{}\n{post}\n{}", mark_js(id), ANNOTATE.get().cloned().unwrap_or_default())
}

/// Tab `id`'s browser closed by itself (the page closed it).
fn closed(id: u64) {
    let (was, left) = pages(|p| (p.remove(&id).is_some(), p.len()));
    if CLOSING.load(Ordering::Relaxed) != OPEN {
        // The app is closing: its tabs stay kept for next time.
        if left == 0 {
            go_on_closing();
        }
        return;
    }
    if was {
        emit(Ev::Gone { id });
    }
}

// ------------------------------------------------------------- handlers

wrap_client! {
    struct PageClient {
        tab: u64,
    }

    impl Client {
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(PageLife::new(self.tab))
        }

        fn load_handler(&self) -> Option<LoadHandler> {
            Some(PageLoad::new(self.tab))
        }

        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(PageDisplay::new(self.tab))
        }
    }
}

wrap_life_span_handler! {
    struct PageLife {
        tab: u64,
    }

    impl LifeSpanHandler {
        fn on_before_popup(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _popup_id: ::std::os::raw::c_int,
            target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            target_disposition: WindowOpenDisposition,
            _user_gesture: ::std::os::raw::c_int,
            _popup_features: Option<&PopupFeatures>,
            window_info: Option<&mut WindowInfo>,
            client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            // A link to a new tab or window, a pop-up (a sign-in window):
            // a tab of Larrier's, still reaching the page that opened it.
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            if let Some(w) = window_info {
                *w = child_info();
            }
            if let Some(c) = client {
                *c = Some(PageClient::new(id));
            }
            pages(|p| {
                p.entry(id).or_default();
            });
            let url = target_url.map(|u| u.to_string()).unwrap_or_default();
            let background = target_disposition.get_raw() == WindowOpenDisposition::NEW_BACKGROUND_TAB.get_raw();
            emit(Ev::Popup { id, url, from: Some(self.tab), background });
            0
        }

        fn on_after_created(&self, browser: Option<&mut Browser>) {
            created(self.tab, browser.cloned());
        }

        fn do_close(&self, browser: Option<&mut Browser>) -> ::std::os::raw::c_int {
            // The page's window sits inside Larrier's: close just it (the
            // default would ask Larrier's own window to close).
            if let Some(h) = browser.and_then(|b| b.host()) {
                let hwnd = h.window_handle().0 as isize;
                on_ui(move || unsafe {
                    let _ = DestroyWindow(HWND(hwnd as *mut _));
                });
            }
            1
        }

        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            closed(self.tab);
        }
    }
}

fn is_main(frame: &Option<&mut Frame>) -> bool {
    frame.as_ref().is_some_and(|f| f.is_main() != 0)
}

wrap_load_handler! {
    struct PageLoad {
        tab: u64,
    }

    impl LoadHandler {
        fn on_loading_state_change(&self, browser: Option<&mut Browser>, is_loading: ::std::os::raw::c_int, can_go_back: ::std::os::raw::c_int, can_go_forward: ::std::os::raw::c_int) {
            let url = browser.and_then(|b| b.main_frame()).map(|f| CefString::from(&f.url()).to_string()).unwrap_or_default();
            emit(Ev::Load { id: self.tab, started: is_loading != 0, url });
            emit(Ev::Nav { id: self.tab, back: can_go_back != 0, forward: can_go_forward != 0 });
        }

        fn on_load_start(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, _transition_type: TransitionType) {
            if is_main(&frame) {
                emit(Ev::PageError { id: self.tab, code: None });
                // Larrier's scripts, as the page starts.
                if let Some(f) = frame {
                    f.execute_java_script(Some(&CefString::from(page_js(self.tab).as_str())), Some(&CefString::from("")), 0);
                }
            }
        }

        fn on_load_error(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, error_code: Errorcode, _error_text: Option<&CefString>, _failed_url: Option<&CefString>) {
            // A load stopped by hand or replaced by another isn't an error.
            if !is_main(&frame) || error_code.get_raw() == Errorcode::ABORTED.get_raw() {
                return;
            }
            let name = format!("{:?}", error_code.as_ref());
            emit(Ev::PageError { id: self.tab, code: Some(name) });
        }
    }
}

wrap_display_handler! {
    struct PageDisplay {
        tab: u64,
    }

    impl DisplayHandler {
        fn on_address_change(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, url: Option<&CefString>) {
            if is_main(&frame) {
                emit(Ev::Url { id: self.tab, url: url.map(|u| u.to_string()).unwrap_or_default() });
            }
        }

        fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
            emit(Ev::Title { id: self.tab, title: title.map(|t| t.to_string()).unwrap_or_default() });
        }

        fn on_console_message(&self, _browser: Option<&mut Browser>, _level: LogSeverity, message: Option<&CefString>, _source: Option<&CefString>, _line: ::std::os::raw::c_int) -> ::std::os::raw::c_int {
            // The annotate script reporting back: taken, never shown.
            let text = message.map(|m| m.to_string()).unwrap_or_default();
            match text.strip_prefix(POST) {
                Some(body) => {
                    emit(Ev::Annotate { id: self.tab, body: body.to_string() });
                    1
                }
                None => 0,
            }
        }
    }
}

// ------------------------------------------------------------- DevTools channel

/// Replies awaited from pages' DevTools channels, by message id.
static WAITING: Mutex<Option<HashMap<i32, mpsc::Sender<Option<Value>>>>> = Mutex::new(None);
static NEXT_MSG: AtomicI32 = AtomicI32::new(1);

fn next_msg() -> i32 {
    NEXT_MSG.fetch_add(1, Ordering::Relaxed)
}

/// Send a DevTools message on a page's own channel (engine UI thread):
/// whether it was taken.
fn tell(host: &BrowserHost, id: i32, method: &str, params: Value) -> bool {
    let msg = json!({"id": id, "method": method, "params": params}).to_string();
    host.send_dev_tools_message(Some(msg.as_bytes())) != 0
}

/// Call a DevTools method on tab `tab`'s page and wait for its result.
fn call(tab: u64, method: &str, params: Value, wait: Duration) -> Option<Value> {
    if STARTED.get() != Some(&true) {
        return None;
    }
    let host = browser(tab)?.host()?;
    let id = next_msg();
    let (tx, rx) = mpsc::channel();
    WAITING.lock().unwrap().get_or_insert_with(HashMap::new).insert(id, tx);
    let method = method.to_string();
    on_ui(move || {
        tell(&host, id, &method, params);
    });
    let r = rx.recv_timeout(wait).ok().flatten();
    WAITING.lock().unwrap().get_or_insert_with(HashMap::new).remove(&id);
    r
}

wrap_dev_tools_message_observer! {
    struct PageTools {
        tab: u64,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_method_result(&self, _browser: Option<&mut Browser>, message_id: ::std::os::raw::c_int, success: ::std::os::raw::c_int, result: Option<&[u8]>) {
            if success == 0 {
                tracing::debug!(tab = self.tab, message_id, "browser engine: DevTools call failed: {}", result.map(|r| String::from_utf8_lossy(r).to_string()).unwrap_or_default());
            }
            let tx = WAITING.lock().unwrap().get_or_insert_with(HashMap::new).remove(&message_id);
            if let Some(tx) = tx {
                let v = result.and_then(|r| serde_json::from_slice::<Value>(r).ok()).filter(|_| success != 0);
                let _ = tx.send(v);
            }
        }
    }
}

/// A picture of tab `id` (or part of it), base64 JPEG.
pub fn capture(id: u64, clip: Option<(f64, f64, f64, f64)>) -> Option<String> {
    let mut params = json!({"format": "jpeg", "quality": 85});
    if let Some((x, y, w, h)) = clip {
        params["clip"] = json!({"x": x.max(0.0), "y": y.max(0.0), "width": w.max(1.0), "height": h.max(1.0), "scale": 1});
    }
    call(id, "Page.captureScreenshot", params, Duration::from_secs(4)).and_then(|v| v["data"].as_str().map(String::from))
}

/// Run `js` in tab `id`'s page: its value.
pub fn eval(id: u64, js: &str) -> Option<Value> {
    call(id, "Runtime.evaluate", json!({"expression": js, "returnByValue": true}), Duration::from_secs(5)).map(|v| v["result"]["value"].clone())
}

/// Tab `tab`'s DevTools target, for the agents' tools.
pub fn target_of(tab: u64) -> Option<String> {
    if let Some(t) = pages(|p| p.get(&tab).and_then(|x| x.target.clone())) {
        return Some(t);
    }
    let t = call(tab, "Target.getTargetInfo", json!({}), Duration::from_secs(3))?["targetInfo"]["targetId"].as_str()?.to_string();
    pages(|p| p.get_mut(&tab).map(|x| x.target = Some(t.clone())));
    Some(t)
}

// ------------------------------------------------------------- the frame

/// The frame: a plain child window of the app's, over the Browser tab's
/// area, that the pages are children of. Made on the UI thread.
fn ensure_frame() -> bool {
    let mut g = SHARED.lock().unwrap();
    if g.is_some() {
        return true;
    }
    let main = MAIN.load(Ordering::Relaxed);
    let Some(frame) = make_frame(main) else { return false };
    *g = Some(Shared { frame, rect: None, size: (1200, 800), shown: None });
    true
}

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

unsafe extern "system" fn frame_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_DESTROY && STARTED.get() == Some(&true) {
        // The app's window is going (its pages already closed): the engine
        // stops.
        let _ = std::panic::catch_unwind(shutdown);
    }
    unsafe { DefWindowProcW(h, msg, wp, lp) }
}

/// Closing the app: the pages close first, while the app's window still
/// runs (they save their cookies and the rest, and their windows go with
/// no thread waiting on another), then the window closes.
static CLOSING: AtomicU8 = AtomicU8::new(OPEN);
const OPEN: u8 = 0;
const PAGES_CLOSING: u8 = 1;
const GO: u8 = 2;

/// On the app's window being asked to close: whether to hold the close
/// while the pages close (it's asked again once they have).
fn hold_close() -> bool {
    match CLOSING.load(Ordering::Relaxed) {
        GO => false,
        PAGES_CLOSING => true,
        _ => {
            let all: Vec<Browser> = pages(|p| p.values().filter_map(|x| x.browser.clone()).collect());
            if all.is_empty() {
                return false;
            }
            CLOSING.store(PAGES_CLOSING, Ordering::Relaxed);
            on_ui(move || {
                for b in all {
                    if let Some(h) = b.host() {
                        h.close_browser(1);
                    }
                }
            });
            // A page that doesn't close doesn't keep the app open.
            std::thread::spawn(|| {
                std::thread::sleep(Duration::from_secs(2));
                go_on_closing();
            });
            true
        }
    }
}

fn go_on_closing() {
    if CLOSING.compare_exchange(PAGES_CLOSING, GO, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
        unsafe {
            let _ = PostMessageW(Some(HWND(MAIN.load(Ordering::Relaxed) as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
}

/// Put the frame over the area (window pixels) showing tab `active`, or out
/// of sight (`None`).
pub fn place(area: Option<Area>, active: Option<u64>) {
    {
        let mut g = SHARED.lock().unwrap();
        let Some(s) = g.as_mut() else { return };
        s.rect = area.filter(|_| active.is_some()).map(|a| (a.x.round() as i32, a.y.round() as i32, a.w.round().max(1.0) as i32, a.h.round().max(1.0) as i32));
        if let Some((_, _, w, h)) = s.rect {
            s.size = (w, h);
        }
        if active.is_some() {
            s.shown = active;
        }
    }
    lay_out();
}

/// The shown tab's page fills the frame; the others wait out of sight at the
/// same size (their pages keep laying out as if shown). The frame sits over
/// the area while the shown tab has a page. Called from the app's thread and
/// the engine's: windows are moved with no lock held (moving one can wait on
/// the thread that owns it), and without waiting on another thread's.
fn lay_out() {
    let Some((frame, rect, shown, size)) = SHARED.lock().unwrap().as_ref().map(|s| (s.frame, s.rect, s.shown, s.size)) else { return };
    let list: Vec<(u64, isize)> = pages(|p| p.iter().filter(|(_, x)| x.hwnd != 0).map(|(id, x)| (*id, x.hwnd)).collect());
    for &(id, h) in &list {
        let x = if Some(id) == shown { 0 } else { AWAY };
        unsafe {
            let _ = SetWindowPos(HWND(h as *mut _), None, x, 0, size.0, size.1, SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
        }
    }
    let shows = list.iter().any(|&(id, _)| Some(id) == shown);
    let frame = HWND(frame as *mut _);
    let own = unsafe { GetWindowThreadProcessId(frame, None) == GetCurrentThreadId() };
    let wait = if own { SET_WINDOW_POS_FLAGS(0) } else { SWP_ASYNCWINDOWPOS };
    unsafe {
        let _ = match rect.filter(|_| shows) {
            // Above the app's own page (its webview window is a sibling).
            Some((x, y, w, h)) => SetWindowPos(frame, Some(HWND_TOP), x, y, w, h, SWP_NOACTIVATE | wait),
            None => SetWindowPos(frame, None, AWAY, 0, 0, 0, SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOSIZE | wait),
        };
    }
}

// ------------------------------------------------------------- keyboard focus

/// The frame, for the focus guard (which runs inside window messages).
static FRAME: AtomicIsize = AtomicIsize::new(0);
/// The page's window that had keyboard focus when the app was left.
static PAGE_FOCUS: AtomicIsize = AtomicIsize::new(0);
/// The app was just activated by a click.
static CLICKED: AtomicBool = AtomicBool::new(false);

/// Keyboard focus between the app and a page. Activating the app's window
/// hands its focus to the app's own webview (wry moves it there on
/// WM_SETFOCUS), which would take the typing meant for a page field. When
/// the window is activated by a click on the page, or comes back while the
/// page had focus, the page gets the focus instead. Closing the window
/// waits for the pages to close first (see [`hold_close`]).
unsafe extern "system" fn main_focus_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
    match msg {
        WM_CLOSE if hold_close() => return LRESULT(0),
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

fn in_frame(h: HWND) -> bool {
    let frame = FRAME.load(Ordering::Relaxed);
    !h.is_invalid() && frame != 0 && unsafe { IsChild(HWND(frame as *mut _), h) }.as_bool()
}

fn rect(h: HWND) -> RECT {
    let mut r = RECT::default();
    unsafe {
        let _ = GetWindowRect(h, &mut r);
    }
    r
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

