//! Larrier's browser: real browser views (WebView2, the engine Larrier runs
//! on) laid over the Browser tab, one per page tab. They share a profile of
//! their own and a DevTools port, so agents drive the very pages the user
//! sees (`Larrier mcp-browser` attaches to that port) and sites see an
//! ordinary Edge, not a headless one.
//!
//! The views are native child windows: they live on the main thread, are
//! placed over the tab's area as the layout reports it, and step aside
//! (offscreen, still rendering) when the tab isn't shown or a menu or dialog
//! opens over them.

use std::cell::RefCell;
use std::sync::OnceLock;

use dioxus::core::spawn_forever;
use dioxus::desktop::wry::dpi::{PhysicalPosition, PhysicalSize};
use dioxus::desktop::wry::{NewWindowResponse, PageLoadEvent, Rect, WebContext, WebView, WebViewBuilder, WebViewBuilderExtWindows};
use dioxus::prelude::*;

/// One page tab as the UI shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tab {
    pub id: u64,
    pub url: String,
    pub title: String,
    pub loading: bool,
    pub can_back: bool,
    pub can_forward: bool,
}

/// What the Browser tab shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub tabs: Vec<Tab>,
    pub active: Option<u64>,
    pub error: Option<String>,
}

pub static STATE: GlobalSignal<State> = Signal::global(State::default);
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

/// What the views report, handled on the UI side.
enum Ev {
    Load { id: u64, started: bool, url: String },
    Title { id: u64, title: String },
    Open { url: String },
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
}

static EVENTS: OnceLock<tokio::sync::mpsc::UnboundedSender<Ev>> = OnceLock::new();

/// The views' profile (shared with the agents' tools, which find their
/// DevTools port there).
fn data_dir() -> std::path::PathBuf {
    larrier_agents::browser::view_dir()
}

fn events() -> tokio::sync::mpsc::UnboundedSender<Ev> {
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
                            // Where back and forward lead, once it has loaded.
                            if !started {
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
                        Ev::Open { url } => {
                            new_tab(&url);
                        }
                        Ev::Failed { id, error } => {
                            let mut st = STATE.write();
                            st.tabs.retain(|t| t.id != id);
                            if st.active == Some(id) {
                                st.active = st.tabs.last().map(|t| t.id);
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
    for (id, v) in &h.views {
        let shown = Some(*id) == h.active && h.area.is_some() && !h.aside;
        let _ = v.set_bounds(bounds(h.area, shown));
    }
}

/// Open a page in a new tab and show it. The tab is listed at once; its
/// view is built a moment later (see `build_pending`).
pub fn new_tab(url: &str) -> Option<u64> {
    let url = normalize(url);
    // Started here, inside the UI's runtime (the builder isn't).
    events();
    let id = with_host(|h| {
        let id = h.next;
        h.next += 1;
        h.active = Some(id);
        id
    })?;
    {
        let mut st = STATE.write();
        st.tabs.push(Tab { id, url: url.clone(), loading: true, ..Default::default() });
        st.active = Some(id);
        st.error = None;
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
            .with_hotkeys_zoom(true)
            // Its DevTools port is for the agents; pages aren't told the
            // browser is automated (it's the user's own browsing too).
            .with_additional_browser_args("--remote-debugging-port=0 --disable-blink-features=AutomationControlled --disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection")
            // Which Larrier tab a page is in, for the agents' tools.
            .with_initialization_script(format!("Object.defineProperty(window, '__larrierTab', {{ value: {id} }});"))
            .with_initialization_script(include_str!("browser_annotate.js").replace("__LARRIER_TOKEN__", annotate_token()))
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
                let _ = t3.send(Ev::Open { url });
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

pub fn activate(id: u64) {
    STATE.write().active = Some(id);
    with_host(|h| {
        h.active = Some(id);
        place(h);
    });
}

pub fn close_tab(id: u64) {
    PENDING.with(|p| p.borrow_mut().retain(|(t, _)| *t != id));
    let mut st = STATE.write();
    let i = st.tabs.iter().position(|t| t.id == id);
    st.tabs.retain(|t| t.id != id);
    if st.active == Some(id) {
        st.active = i.and_then(|i| st.tabs.get(i.min(st.tabs.len().saturating_sub(1)))).map(|t| t.id);
    }
    let active = st.active;
    drop(st);
    with_host(|h| {
        if !h.views.iter().any(|(v, _)| *v == id) {
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

pub fn navigate(input: &str) {
    let url = normalize(input);
    if STATE.peek().active.is_none() {
        new_tab(&url);
        return;
    }
    active_view(|v| v.load_url(&url));
}

pub fn back() {
    active_view(|v| v.evaluate_script("history.back()"));
}

pub fn forward() {
    active_view(|v| v.evaluate_script("history.forward()"));
}

pub fn reload() {
    active_view(|v| v.reload());
}

pub fn stop() {
    active_view(|v| v.evaluate_script("window.stop()"));
}

pub fn devtools() {
    active_view(|v| v.open_devtools());
}

/// Turn annotate mode on or off in the tab shown.
pub fn annotate(on: bool) {
    *ANNOTATING.write() = on;
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
                let shots = tokio::task::spawn_blocking(move || clips.into_iter().map(|c| larrier_agents::browser::view_capture(id, Some(c))).collect::<Vec<_>>()).await.unwrap_or_default();
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
                    let shot = larrier_agents::browser::view_snapshot(tab);
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
        return "about:blank".into();
    }
    if s.contains("://") || s.starts_with("about:") || s.starts_with("data:") || s.starts_with("file:") {
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
    format!("https://www.bing.com/search?q={}", urlencode(s))
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
        assert_eq!(normalize("rust lifetimes"), "https://www.bing.com/search?q=rust+lifetimes");
        assert_eq!(normalize("intranet/wiki/Home/?C=x"), "http://intranet/wiki/Home/?C=x");
        assert_eq!(normalize("intranet"), "https://www.bing.com/search?q=intranet");
    }
}
