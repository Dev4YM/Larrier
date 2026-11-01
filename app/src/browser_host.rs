//! Larrier's browser: real browser pages laid over the Browser tab, one per
//! page tab. With Google Chrome installed they're Larrier's own Chrome's
//! (see `browser_chrome`); otherwise WebView2 views (the engine Larrier runs
//! on). Either way they have a profile of their own and a DevTools port, so
//! agents drive the very pages the user sees (`Larrier mcp-browser` attaches
//! to that port) and sites see an ordinary browser, not a headless one.
//!
//! The pages are native child windows: placed over the tab's area as the
//! layout reports it, they step aside (offscreen, still rendering) when the
//! tab isn't shown or a menu or dialog opens over them.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;

use dioxus::core::spawn_forever;
use dioxus::desktop::wry::dpi::{PhysicalPosition, PhysicalSize};
use dioxus::desktop::wry::{NewWindowResponse, PageLoadEvent, Rect, WebContext, WebView, WebViewBuilder};
use dioxus::prelude::*;

/// One page tab as the UI shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tab {
    pub id: u64,
    /// The chat whose browser it's in.
    pub owner: String,
    pub url: String,
    pub title: String,
    pub loading: bool,
    pub can_back: bool,
    pub can_forward: bool,
}

/// What the Browser tab shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// Every chat's tabs.
    pub tabs: Vec<Tab>,
    /// The tab shown (one of `owner`'s).
    pub active: Option<u64>,
    pub error: Option<String>,
    /// The chat whose browser is shown: each chat has tabs of its own.
    pub owner: String,
    /// The tab each other chat last showed.
    pub last: HashMap<String, u64>,
}

impl State {
    /// The tabs of the chat shown.
    pub fn mine(&self) -> Vec<Tab> {
        self.tabs.iter().filter(|t| t.owner == self.owner).cloned().collect()
    }

    /// The tab `owner`'s browser shows (or would, when shown).
    pub fn active_of(&self, owner: &str) -> Option<u64> {
        if owner == self.owner {
            return self.active;
        }
        self.last.get(owner).copied().filter(|id| self.tabs.iter().any(|t| t.id == *id))
    }

    /// The tab to show after `gone` (at index `i`) left `owner`'s browser.
    fn next_after(&self, owner: &str, i: Option<usize>) -> Option<u64> {
        let left: Vec<&Tab> = self.tabs.iter().filter(|t| t.owner == owner).collect();
        let before = i.map(|i| self.tabs[..i.min(self.tabs.len())].iter().filter(|t| t.owner == owner).count()).unwrap_or(left.len());
        left.get(before.min(left.len().saturating_sub(1))).map(|t| t.id)
    }
}

pub static STATE: GlobalSignal<State> = Signal::global(State::default);

/// A chat's browser as kept between launches: its pages (address, title)
/// and the one it showed.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
struct Kept {
    pages: Vec<(String, String)>,
    active: Option<usize>,
}

/// Chats whose kept tabs are back (or had none to bring back).
fn restored() -> std::sync::MutexGuard<'static, std::collections::HashSet<String>> {
    static SET: OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    SET.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// Bring back the tabs `owner`'s browser had when Larrier last closed, the
/// first time it's shown or opens a page.
pub fn restore(owner: &str) {
    if owner.is_empty() || !restored().insert(owner.to_string()) {
        return;
    }
    let Some(k) = crate::keep::value::<Kept>(owner, "browser") else { return };
    let mut ids = Vec::new();
    for (url, title) in k.pages {
        if let Some(id) = new_tab_in(Some(owner), &url) {
            if let Some(t) = STATE.write().tabs.iter_mut().find(|t| t.id == id) {
                t.title = title;
            }
            ids.push(id);
        }
    }
    if let Some(id) = k.active.and_then(|i| ids.get(i)) {
        activate(*id);
    }
}

/// Keep each chat's tabs for the next launch (those of chats not shown
/// since Larrier started stay as they were kept).
pub fn keep() {
    let st = STATE.peek();
    let mut owners: Vec<String> = restored().iter().cloned().collect();
    for t in &st.tabs {
        if !owners.contains(&t.owner) {
            owners.push(t.owner.clone());
        }
    }
    for o in owners.iter().filter(|o| !o.is_empty()) {
        let mine: Vec<&Tab> = st.tabs.iter().filter(|t| &t.owner == o).collect();
        let active = st.active_of(o).and_then(|a| mine.iter().position(|t| t.id == a));
        let k = Kept { pages: mine.iter().map(|t| (t.url.clone(), t.title.clone())).collect(), active };
        // A browser with no tabs left keeps nothing.
        crate::keep::set_value(o, "browser", &(!k.pages.is_empty()).then_some(k));
    }
}
/// A picture of the page, shown in its place while it steps aside for a
/// menu or dialog of the app's (base64 JPEG).
pub static SNAPSHOT: GlobalSignal<Option<String>> = Signal::global(|| None);
/// Annotate mode is on in the tab shown.
pub static ANNOTATING: GlobalSignal<bool> = Signal::global(|| false);

/// Pages hear from Larrier's own annotate script only with this (each run's
/// own), so a page can't post as it.
fn annotate_token() -> &'static str {
    static TOKEN: OnceLock<String> = OnceLock::new();
    TOKEN.get_or_init(|| uuid::Uuid::new_v4().simple().to_string())
}

/// Where the Browser tab's page area is, in the window's own pixels (the
/// app's zoom and the display's scale applied), if shown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// What the pages report, handled on the UI side.
pub(crate) enum Ev {
    Load { id: u64, started: bool, url: String },
    Title { id: u64, title: String },
    /// The page's address changed (a link, the page itself). From Chrome;
    /// webviews report it as they load.
    #[cfg(windows)]
    Url { id: u64, url: String },
    /// The tab's page closed by itself (the page closed it, Chrome quit).
    #[cfg(windows)]
    Gone { id: u64 },
    /// A page opened another (a link to a new window): a tab in the same
    /// chat's browser.
    Open { url: String, from: Option<u64> },
    Failed { id: u64, error: String },
    Nav { id: u64, back: bool, forward: bool },
    /// A message from the page's annotate script.
    Annotate { id: u64, body: String },
    /// The picture to cover with is ready (or couldn't be taken): step aside.
    Covered { gen: u64, shot: Option<String> },
}

struct Host {
    /// The views' profile, lent out while a view is built.
    ctx: Option<WebContext>,
    /// Tabs closed before their view was built.
    closed: std::collections::HashSet<u64>,
    views: Vec<(u64, WebView)>,
    /// The tab shown (kept here too: the view builder runs outside the UI).
    active: Option<u64>,
    area: Option<Area>,
    /// Something of the app's is drawn over the area (a menu, a dialog).
    covered: bool,
    /// The view is out of the way for it (once its picture is up).
    aside: bool,
    gen: u64,
    next: u64,
}

thread_local! {
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
    /// Views to build: (tab id, address). Building one runs a nested message
    /// loop, so it happens at the top of an event-loop turn (a wry event
    /// handler), never inside the UI's own event handling.
    static PENDING: RefCell<Vec<(u64, String)>> = const { RefCell::new(Vec::new()) };
    static BUILDER: RefCell<Option<std::sync::Arc<dioxus::desktop::tao::window::Window>>> = const { RefCell::new(None) };
    /// The app's state, for what the views hand to it (set by the root).
    static APP: std::cell::Cell<Option<crate::state::App>> = const { std::cell::Cell::new(None) };
}

/// Called once by the root component.
pub fn set_app(app: crate::state::App) {
    APP.with(|a| a.set(Some(app)));
    #[cfg(windows)]
    {
        use dioxus::desktop::tao::platform::windows::WindowExtWindows;
        crate::browser_chrome::setup(dioxus::desktop::window().window.hwnd(), events(), annotate_script());
    }
}

/// Larrier's own Chrome is the engine when Google Chrome is installed;
/// WebView2 views otherwise.
#[cfg(windows)]
fn chrome() -> bool {
    crate::browser_chrome::chrome_exe().is_some()
}

#[cfg(not(windows))]
fn chrome() -> bool {
    false
}

#[cfg(windows)]
fn to_chrome(cmd: crate::browser_chrome::Cmd) {
    crate::browser_chrome::send(cmd);
}

/// What a new tab opens: Chrome's New Tab page, or a blank page.
pub fn blank() -> &'static str {
    if chrome() {
        "chrome://newtab/"
    } else {
        "about:blank"
    }
}

/// A new tab's page (no address worth showing).
pub fn is_blank(url: &str) -> bool {
    url.is_empty() || url == "about:blank" || url.starts_with("chrome://newtab") || url.starts_with("chrome://new-tab-page") || url.starts_with("chrome-search://")
}

/// The annotate script, with this run's token.
fn annotate_script() -> String {
    include_str!("browser_annotate.js").replace("__LARRIER_TOKEN__", annotate_token())
}

/// Run `js` in tab `id`'s page: its value.
fn eval(id: u64, js: &str) -> Option<serde_json::Value> {
    #[cfg(windows)]
    if chrome() {
        return crate::browser_chrome::eval(id, js);
    }
    use larrier_agents::browser::{view_running, view_target, Cdp};
    let cdp = Cdp::connect(&view_running()?).ok()?;
    let target = view_target(&cdp, id)?;
    let r = cdp.call("Target.attachToTarget", serde_json::json!({"targetId": target, "flatten": true}), None).ok()?;
    let session = r["sessionId"].as_str()?.to_string();
    let v = cdp.call("Runtime.evaluate", serde_json::json!({"expression": js, "returnByValue": true}), Some(&session)).ok()?;
    Some(v["result"]["value"].clone())
}

/// Hand a page tab (`"active"`: the one shown) to a chat's box as context:
/// an object in its text carrying the page's address, title and text as
/// they are now, and a picture of the page attached alongside.
pub fn to_chat(which: &str, target: Option<String>) {
    let st = STATE.peek().clone();
    let id = if which == "active" { st.active } else { which.parse().ok() };
    let Some(tab) = id.and_then(|id| st.tabs.iter().find(|t| t.id == id)).cloned() else { return };
    let Some(app) = APP.with(|a| a.get()) else { return };
    if is_blank(&tab.url) {
        app.toast("That tab has no page yet.");
        return;
    }
    spawn_forever(async move {
        let id = tab.id;
        const PAGE: &str = "(() => ({ url: location.href, title: document.title, text: (document.body ? document.body.innerText : '').slice(0, 40000) }))()";
        let (page, shot) = tokio::task::spawn_blocking(move || (eval(id, PAGE), capture(id, None))).await.unwrap_or((None, None));
        let field = |k: &str| page.as_ref().and_then(|p| p[k].as_str()).map(String::from).filter(|s| !s.is_empty());
        let url = field("url").unwrap_or(tab.url.clone());
        let title = field("title").unwrap_or(tab.title.clone());
        let chip = crate::ui::mentions::MentionChip::page(&title, &url, &field("text").unwrap_or_default());
        let picture = shot.and_then(|s| crate::state::save_data_url(&format!("data:image/jpeg;base64,{s}"), "page.jpg").ok());
        let mut app = app;
        app.ui.write().chip_inbox.push((target, chip, picture));
    });
}

/// A picture of tab `id`, or of part of it (CSS pixels): base64 JPEG.
fn capture(id: u64, clip: Option<(f64, f64, f64, f64)>) -> Option<String> {
    #[cfg(windows)]
    if chrome() {
        return crate::browser_chrome::capture(id, clip);
    }
    larrier_agents::browser::view_capture(id, clip)
}

static EVENTS: OnceLock<tokio::sync::mpsc::UnboundedSender<Ev>> = OnceLock::new();

/// The views' profile (shared with the agents' tools, which find their
/// DevTools port there).
fn data_dir() -> std::path::PathBuf {
    larrier_agents::browser::view_dir()
}

pub(crate) fn events() -> tokio::sync::mpsc::UnboundedSender<Ev> {
    EVENTS
        .get_or_init(|| {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Ev>();
            spawn_forever(async move {
                while let Some(ev) = rx.recv().await {
                    match ev {
                        Ev::Load { id, started, url } => {
                            let mut st = STATE.write();
                            if let Some(t) = st.tabs.iter_mut().find(|t| t.id == id) {
                                t.loading = started;
                                if !url.is_empty() {
                                    t.url = url;
                                }
                            }
                            // A new page starts without annotate mode.
                            if started && st.active == Some(id) {
                                *ANNOTATING.write() = false;
                            }
                            // Where back and forward lead, once it has loaded
                            // (Chrome's worker says so itself).
                            if !started && !chrome() {
                                let tx = events();
                                std::thread::spawn(move || {
                                    if let Some((back, forward)) = history(id) {
                                        let _ = tx.send(Ev::Nav { id, back, forward });
                                    }
                                });
                            }
                        }
                        Ev::Annotate { id, body } => annotated(id, &body),
                        Ev::Nav { id, back, forward } => {
                            let mut st = STATE.write();
                            if let Some(t) = st.tabs.iter_mut().find(|t| t.id == id) {
                                t.can_back = back;
                                t.can_forward = forward;
                            }
                        }
                        Ev::Covered { gen, shot } => {
                            let current = with_host(|h| h.gen == gen && h.covered).unwrap_or(false);
                            if current {
                                *SNAPSHOT.write() = shot;
                                with_host(|h| {
                                    h.aside = true;
                                    place(h);
                                });
                            }
                        }
                        Ev::Title { id, title } => {
                            let mut st = STATE.write();
                            if let Some(t) = st.tabs.iter_mut().find(|t| t.id == id) {
                                t.title = title;
                            }
                        }
                        #[cfg(windows)]
                        Ev::Url { id, url } => {
                            let mut st = STATE.write();
                            if let Some(t) = st.tabs.iter_mut().find(|t| t.id == id) {
                                t.url = url;
                            }
                        }
                        #[cfg(windows)]
                        Ev::Gone { id } => close_tab(id),
                        Ev::Open { url, from } => {
                            let owner = from.and_then(|f| STATE.peek().tabs.iter().find(|t| t.id == f).map(|t| t.owner.clone()));
                            new_tab_in(owner.as_deref(), &url);
                        }
                        Ev::Failed { id, error } => {
                            let mut st = STATE.write();
                            st.tabs.retain(|t| t.id != id);
                            if st.active == Some(id) {
                                let owner = st.owner.clone();
                                st.active = st.next_after(&owner, None);
                            }
                            st.error = Some(error);
                            let active = st.active;
                            drop(st);
                            with_host(|h| {
                                h.active = active;
                                place(h);
                            });
                        }
                    }
                }
            });
            tx
        })
        .clone()
}

/// Off to the side: still rendering (agents can work in it), out of sight.
const AWAY: f64 = -30_000.0;

fn bounds(area: Option<Area>, shown: bool) -> Rect {
    let a = area.unwrap_or(Area { x: 0.0, y: 0.0, w: 1280.0, h: 800.0 });
    let x = if shown { a.x } else { AWAY };
    Rect { position: PhysicalPosition::new(x as i32, a.y as i32).into(), size: PhysicalSize::new(a.w.max(1.0) as u32, a.h.max(1.0) as u32).into() }
}

fn with_host<R>(f: impl FnOnce(&mut Host) -> R) -> Option<R> {
    HOST.with(|h| {
        let mut h = h.borrow_mut();
        if h.is_none() {
            let _ = std::fs::create_dir_all(data_dir());
            *h = Some(Host { ctx: Some(WebContext::new(Some(data_dir()))), closed: Default::default(), views: Vec::new(), active: None, area: None, covered: false, aside: false, gen: 0, next: 1 });
        }
        h.as_mut().map(f)
    })
}

/// Put every view where it belongs: the active one over the area (when the
/// tab is shown and nothing covers it), the rest out of sight.
fn place(h: &Host) {
    #[cfg(windows)]
    if chrome() {
        crate::browser_chrome::place(h.area.filter(|_| !h.aside), h.active);
        return;
    }
    for (id, v) in &h.views {
        let shown = Some(*id) == h.active && h.area.is_some() && !h.aside;
        let _ = v.set_bounds(bounds(h.area, shown));
    }
}

/// Open a page in a new tab of the chat shown and show it.
pub fn new_tab(url: &str) -> Option<u64> {
    new_tab_in(None, url)
}

/// Open a page in a new tab of `owner`'s browser (`None`: the chat shown),
/// shown if that's the browser shown. The tab is listed at once; its view
/// is built a moment later (see `build_pending`).
pub fn new_tab_in(owner: Option<&str>, url: &str) -> Option<u64> {
    let url = normalize(url);
    // Started here, inside the UI's runtime (the builder isn't).
    events();
    let owner = owner.map(String::from).unwrap_or_else(|| STATE.peek().owner.clone());
    // Its kept tabs first, so a new one goes after them.
    restore(&owner);
    let shown = owner == STATE.peek().owner;
    let id = with_host(|h| {
        let id = h.next;
        h.next += 1;
        if shown {
            h.active = Some(id);
        }
        id
    })?;
    {
        let mut st = STATE.write();
        st.tabs.push(Tab { id, owner: owner.clone(), url: url.clone(), loading: true, ..Default::default() });
        if shown {
            st.active = Some(id);
        } else {
            st.last.insert(owner, id);
        }
        st.error = None;
    }
    #[cfg(windows)]
    if chrome() {
        to_chrome(crate::browser_chrome::Cmd::NewTab { id, url });
        with_host(|h| place(h));
        return Some(id);
    }
    PENDING.with(|p| p.borrow_mut().push((id, url)));
    BUILDER.with(|b| b.borrow_mut().get_or_insert_with(|| dioxus::desktop::window().window.clone()).clone());
    schedule_build();
    Some(id)
}

/// Build the waiting views from the message loop itself (a thread timer):
/// building one runs a nested message loop, which must not happen inside
/// the UI's or the event loop's own handling, where it stalls the window.
#[cfg(windows)]
fn schedule_build() {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};
    unsafe extern "system" fn tick(_: HWND, _: u32, id: usize, _: u32) {
        unsafe {
            let _ = KillTimer(None, id);
        }
        // Nothing may unwind out of a Windows callback.
        let _ = std::panic::catch_unwind(build_pending);
    }
    unsafe {
        SetTimer(None, 0, 0, Some(tick));
    }
}

#[cfg(not(windows))]
fn schedule_build() {
    build_pending();
}

/// Build the views waiting to be built (see `PENDING`).
fn build_pending() {
    let todo: Vec<(u64, String)> = PENDING.with(|p| std::mem::take(&mut *p.borrow_mut()));
    if todo.is_empty() {
        return;
    }
    let Some(window) = BUILDER.with(|b| b.borrow().clone()) else { return };
    for (id, url) in todo {
        // The UI keeps running while a view is built: nothing of the host's
        // is borrowed meanwhile (its profile is lent out and given back).
        let Some((ctx, area)) = with_host(|h| h.ctx.take().map(|c| (c, h.area))).flatten() else {
            // Another build is under way: this one waits its turn.
            PENDING.with(|p| p.borrow_mut().push((id, url)));
            schedule_build();
            return;
        };
        let mut ctx = ctx;
        let tx = events();
        let (t1, t2, t3, t4) = (tx.clone(), tx.clone(), tx.clone(), tx);
        let made = WebViewBuilder::new_with_web_context(&mut ctx)
            .with_url(&url)
            .with_bounds(bounds(area, false))
            .with_devtools(true)
            .with_hotkeys_zoom(true);
        // Its DevTools port is for the agents; pages aren't told the browser
        // is automated (it's the user's own browsing too).
        #[cfg(windows)]
        let made = {
            use dioxus::desktop::wry::WebViewBuilderExtWindows;
            made.with_additional_browser_args("--remote-debugging-port=0 --disable-blink-features=AutomationControlled --disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection")
        };
        let made = made
            // Which Larrier tab a page is in, for the agents' tools.
            .with_initialization_script(format!("Object.defineProperty(window, '__larrierTab', {{ value: {id} }});"))
            .with_initialization_script(annotate_script())
            .with_ipc_handler(move |req| {
                let _ = t4.send(Ev::Annotate { id, body: req.body().clone() });
            })
            .with_on_page_load_handler(move |ev, url| {
                let _ = t1.send(Ev::Load { id, started: matches!(ev, PageLoadEvent::Started), url });
            })
            .with_document_title_changed_handler(move |title| {
                let _ = t2.send(Ev::Title { id, title });
            })
            // Links that open a window open a tab here instead.
            .with_new_window_req_handler(move |url, _| {
                let _ = t3.send(Ev::Open { url, from: Some(id) });
                NewWindowResponse::Deny
            })
            .build_as_child(&*window);
        with_host(|h| {
            h.ctx = Some(ctx);
            match made {
                // A tab closed while its view was being built.
                Ok(v) if h.closed.remove(&id) => drop(v),
                Ok(v) => h.views.push((id, v)),
                Err(e) => {
                    let _ = events().send(Ev::Failed { id, error: e.to_string() });
                }
            }
            place(h);
        });
    }
}

/// Show tab `id` in its chat's browser (on screen if that's the one shown).
pub fn activate(id: u64) {
    let mut st = STATE.write();
    let Some(owner) = st.tabs.iter().find(|t| t.id == id).map(|t| t.owner.clone()) else { return };
    if owner != st.owner {
        st.last.insert(owner, id);
        return;
    }
    st.active = Some(id);
    drop(st);
    with_host(|h| {
        h.active = Some(id);
        place(h);
    });
}

/// Show chat `owner`'s browser: its tabs, and the one it last showed.
pub fn set_owner(owner: &str) {
    let mut st = STATE.write();
    if st.owner == owner {
        return;
    }
    let prev = std::mem::replace(&mut st.owner, owner.to_string());
    if let Some(a) = st.active {
        st.last.insert(prev, a);
    }
    let active = st.last.remove(owner).filter(|id| st.tabs.iter().any(|t| t.id == *id)).or_else(|| st.tabs.iter().rev().find(|t| t.owner == owner).map(|t| t.id));
    st.active = active;
    drop(st);
    *ANNOTATING.write() = false;
    with_host(|h| {
        h.active = active;
        place(h);
    });
}

pub fn close_tab(id: u64) {
    PENDING.with(|p| p.borrow_mut().retain(|(t, _)| *t != id));
    let mut st = STATE.write();
    let i = st.tabs.iter().position(|t| t.id == id);
    let owner = i.map(|i| st.tabs[i].owner.clone());
    st.tabs.retain(|t| t.id != id);
    if st.active == Some(id) {
        let o = st.owner.clone();
        st.active = st.next_after(&o, i);
    }
    if let Some(o) = owner.filter(|o| st.last.get(o) == Some(&id)) {
        match st.next_after(&o, i) {
            Some(n) => st.last.insert(o, n),
            None => st.last.remove(&o),
        };
    }
    let active = st.active;
    drop(st);
    #[cfg(windows)]
    if chrome() {
        to_chrome(crate::browser_chrome::Cmd::Close { id });
    }
    with_host(|h| {
        if !chrome() && !h.views.iter().any(|(v, _)| *v == id) {
            h.closed.insert(id);
        }
        h.views.retain(|(v, _)| *v != id);
        h.active = active;
        place(h);
    });
}

fn active_view<R>(f: impl FnOnce(&WebView) -> R) -> Option<R> {
    with_host(|h| h.views.iter().find(|(v, _)| Some(*v) == h.active).map(|(_, w)| f(w))).flatten()
}

/// Hand Chrome's worker a command for the tab shown, when Chrome is the
/// engine (`false`: it isn't, the view does it).
#[cfg(windows)]
fn chrome_active(cmd: impl FnOnce(u64) -> crate::browser_chrome::Cmd) -> bool {
    if !chrome() {
        return false;
    }
    if let Some(id) = STATE.peek().active {
        to_chrome(cmd(id));
    }
    true
}

pub fn navigate(input: &str) {
    let url = normalize(input);
    if STATE.peek().active.is_none() {
        new_tab(&url);
        return;
    }
    #[cfg(windows)]
    if chrome_active(|id| crate::browser_chrome::Cmd::Navigate { id, url: url.clone() }) {
        return;
    }
    active_view(|v| v.load_url(&url));
}

pub fn back() {
    #[cfg(windows)]
    if chrome_active(|id| crate::browser_chrome::Cmd::History { id, back: true }) {
        return;
    }
    active_view(|v| v.evaluate_script("history.back()"));
}

pub fn forward() {
    #[cfg(windows)]
    if chrome_active(|id| crate::browser_chrome::Cmd::History { id, back: false }) {
        return;
    }
    active_view(|v| v.evaluate_script("history.forward()"));
}

pub fn reload() {
    #[cfg(windows)]
    if chrome_active(|id| crate::browser_chrome::Cmd::Reload { id }) {
        return;
    }
    active_view(|v| v.reload());
}

pub fn stop() {
    #[cfg(windows)]
    if chrome_active(|id| crate::browser_chrome::Cmd::Stop { id }) {
        return;
    }
    active_view(|v| v.evaluate_script("window.stop()"));
}

pub fn devtools() {
    #[cfg(windows)]
    if chrome_active(|id| crate::browser_chrome::Cmd::DevTools { id }) {
        return;
    }
    active_view(|v| v.open_devtools());
}

/// Turn annotate mode on or off in the tab shown.
pub fn annotate(on: bool) {
    *ANNOTATING.write() = on;
    #[cfg(windows)]
    if chrome_active(|id| crate::browser_chrome::Cmd::Script { id, js: format!("window.__larrierAnnotate && window.__larrierAnnotate({on})") }) {
        return;
    }
    active_view(|v| {
        let _ = v.evaluate_script(&format!("window.__larrierAnnotate && window.__larrierAnnotate({on})"));
        if on {
            let _ = v.focus();
        }
    });
}

/// What the annotate script said: it was turned off, or the user picked
/// elements and wrote a note. Each element goes to the chat's box as a
/// picture and its details, the note as the text.
fn annotated(id: u64, body: &str) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else { return };
    if v["larrier"].as_str() != Some(annotate_token()) {
        return;
    }
    match v["kind"].as_str() {
        Some("off") => *ANNOTATING.write() = false,
        Some("attach") => {
            *ANNOTATING.write() = false;
            let items: Vec<serde_json::Value> = v["items"].as_array().cloned().unwrap_or_default();
            let (url, title, note) = (v["url"].as_str().unwrap_or("").to_string(), v["title"].as_str().unwrap_or("").to_string(), v["note"].as_str().unwrap_or("").trim().to_string());
            let Some(mut app) = APP.with(|a| a.get()) else { return };
            spawn_forever(async move {
                // Pictures of the elements, a little room around each.
                let clips: Vec<(f64, f64, f64, f64)> = items.iter().map(|i| {
                    let r = &i["rect"];
                    let (x, y, w, h) = (r["x"].as_f64().unwrap_or(0.0), r["y"].as_f64().unwrap_or(0.0), r["w"].as_f64().unwrap_or(1.0), r["h"].as_f64().unwrap_or(1.0));
                    (x - 8.0, y - 8.0, w + 16.0, h + 16.0)
                }).collect();
                let shots = tokio::task::spawn_blocking(move || clips.into_iter().map(|c| capture(id, Some(c))).collect::<Vec<_>>()).await.unwrap_or_default();
                let mut parts = Vec::new();
                for (n, item) in items.iter().enumerate() {
                    if let Some(Some(jpeg)) = shots.get(n) {
                        if let Ok(p) = crate::state::save_data_url(&format!("data:image/jpeg;base64,{jpeg}"), "element.jpg") {
                            parts.push(p);
                        }
                    }
                    let short = item["short"].as_str().unwrap_or("element");
                    let mut details = format!("Element on {} ({url})\n{short}\nSelector: {}", if title.is_empty() { "the page" } else { &title }, item["selector"].as_str().unwrap_or(""));
                    if let Some(t) = item["text"].as_str().filter(|t| !t.is_empty()) {
                        details.push_str(&format!("\nText: {t}"));
                    }
                    details.push_str(&format!("\nHTML:\n{}", item["html"].as_str().unwrap_or("")));
                    parts.push(larrier_core::Part::File { path: format!("Element · {}", short.chars().take(60).collect::<String>()), mime: Some("text/plain".into()), text: Some(details) });
                }
                let session = app.current_id();
                let mut ui = app.ui.write();
                ui.attach_inbox.extend(parts);
                if !note.is_empty() {
                    ui.compose_inbox.push((session, note));
                }
                ui.focus_composer += 1;
            });
        }
        _ => {}
    }
}

/// The tab's page area moved, appeared (`Some`) or went away (`None`), or a
/// menu or dialog of the app's opened over it (`covered`). Under a menu the
/// page steps aside, leaving a picture of itself in its place.
pub fn set_area(area: Option<Area>, covered: bool) {
    let cover = with_host(|h| {
        let newly = covered && !h.covered;
        h.area = area;
        h.covered = covered;
        h.gen += 1;
        if !covered {
            h.aside = false;
        }
        place(h);
        newly.then_some((h.gen, h.active))
    })
    .flatten();
    if !covered {
        *SNAPSHOT.write() = None;
    }
    if let Some((gen, active)) = cover {
        let tx = events();
        match active {
            Some(tab) => {
                std::thread::spawn(move || {
                    let shot = capture(tab, None);
                    let _ = tx.send(Ev::Covered { gen, shot });
                });
            }
            None => {
                let _ = tx.send(Ev::Covered { gen, shot: None });
            }
        }
    }
}

/// Whether tab `id` can go back and forward.
fn history(id: u64) -> Option<(bool, bool)> {
    use larrier_agents::browser::{view_running, view_target, Cdp};
    let cdp = Cdp::connect(&view_running()?).ok()?;
    let target = view_target(&cdp, id)?;
    let r = cdp.call("Target.attachToTarget", serde_json::json!({"targetId": target, "flatten": true}), None).ok()?;
    let session = r["sessionId"].as_str()?.to_string();
    let h = cdp.call("Page.getNavigationHistory", serde_json::json!({}), Some(&session)).ok()?;
    let i = h["currentIndex"].as_u64()? as usize;
    let n = h["entries"].as_array()?.len();
    Some((i > 0, i + 1 < n))
}

/// An address, or words to search for.
fn normalize(input: &str) -> String {
    let s = input.trim();
    if s.is_empty() {
        return blank().into();
    }
    if s.contains("://") || s.starts_with("about:") || s.starts_with("data:") || s.starts_with("file:") || s.starts_with("chrome:") {
        return s.to_string();
    }
    let host = s.split(['/', '?', '#']).next().unwrap_or("");
    // An intranet name with a path ("intranet/wiki/…") is an address too.
    let intranet = !host.contains('.') && s.len() > host.len() && !host.is_empty() && host.chars().all(|c| c.is_alphanumeric() || c == '-' || c == ':');
    let looks_like_host = !s.contains(' ') && (host.contains('.') || host.starts_with("localhost") || host.parse::<std::net::IpAddr>().is_ok() || host.contains(':') || intranet);
    if looks_like_host {
        let local = host.starts_with("localhost") || host.starts_with("127.") || host.ends_with(".localhost") || host.ends_with(".test") || intranet;
        return format!("{}://{s}", if local { "http" } else { "https" });
    }
    format!("https://www.google.com/search?q={}", urlencode(s))
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            b' ' => "+".into(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn addresses_and_searches() {
        assert_eq!(normalize("example.com"), "https://example.com");
        assert_eq!(normalize("localhost:3000/x"), "http://localhost:3000/x");
        assert_eq!(normalize("http://a.b"), "http://a.b");
        assert_eq!(normalize("rust lifetimes"), "https://www.google.com/search?q=rust+lifetimes");
        assert_eq!(normalize("intranet/wiki/Home/?C=x"), "http://intranet/wiki/Home/?C=x");
        assert_eq!(normalize("intranet"), "https://www.google.com/search?q=intranet");
    }
}
