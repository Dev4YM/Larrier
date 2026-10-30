//! The Browser tab's live view of the agents' browser: the page streamed as
//! frames (a DevTools screencast), and the user's clicks, scrolls and keys
//! sent back to it, so the user watches an agent work and can take over.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use larrier_agents::browser::{self, Page};
use serde_json::{json, Value};

/// What the Browser tab shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    pub url: String,
    pub title: String,
    /// The browser is running and this view is attached to its page.
    pub live: bool,
    pub starting: bool,
    pub error: Option<String>,
}

pub static VIEW: GlobalSignal<View> = Signal::global(View::default);
/// The latest frame (base64 JPEG).
pub static FRAME: GlobalSignal<Option<String>> = Signal::global(|| None);

pub enum Cmd {
    /// The tab is shown (frames wanted) or hidden.
    Show(bool),
    Navigate(String),
    Back,
    Forward,
    Reload,
    /// Mouse, wheel, key or text from the view (the bridge's message).
    Input(Value),
}

enum Update {
    View(View),
    Frame(String),
}

static TX: OnceLock<Sender<Cmd>> = OnceLock::new();

/// Send the view a command (starting its worker the first time; call from
/// the UI, which owns the signals the worker reports to).
pub fn send(cmd: Cmd) {
    let tx = TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        let (up_tx, mut up_rx) = tokio::sync::mpsc::unbounded_channel::<Update>();
        std::thread::Builder::new().name("browser-view".into()).spawn(move || worker(rx, up_tx)).expect("browser view thread");
        spawn_forever(async move {
            while let Some(u) = up_rx.recv().await {
                match u {
                    Update::View(v) => {
                        if *VIEW.peek() != v {
                            *VIEW.write() = v;
                        }
                    }
                    Update::Frame(f) => *FRAME.write() = Some(f),
                }
            }
        });
        tx
    });
    let _ = tx.send(cmd);
}

struct Live {
    page: Page,
    casting: bool,
    view: View,
}

fn worker(cmds: Receiver<Cmd>, out: tokio::sync::mpsc::UnboundedSender<Update>) {
    let mut live: Option<Live> = None;
    let mut showing = false;
    let mut checked = Instant::now() - Duration::from_secs(5);
    let report = |v: &View| {
        let _ = out.send(Update::View(v.clone()));
    };
    loop {
        match cmds.recv_timeout(Duration::from_millis(25)) {
            Ok(Cmd::Show(on)) => {
                showing = on;
                if let Some(l) = live.as_mut().filter(|l| l.casting && !on) {
                    let _ = l.page.call("Page.stopScreencast", json!({}));
                    l.casting = false;
                }
                checked = Instant::now() - Duration::from_secs(5);
            }
            Ok(Cmd::Navigate(u)) => {
                if live.is_none() {
                    report(&View { starting: true, ..View::default() });
                    match Page::open() {
                        Ok(page) => live = Some(Live { page, casting: false, view: View { live: true, ..View::default() } }),
                        Err(e) => {
                            report(&View { error: Some(e), ..View::default() });
                            continue;
                        }
                    }
                }
                if let Some(l) = live.as_mut() {
                    let url = browser::address(&u);
                    if let Err(e) = l.page.call("Page.navigate", json!({"url": url})) {
                        l.view.error = Some(e);
                    } else {
                        l.view.url = url;
                        l.view.error = None;
                    }
                    report(&l.view);
                }
            }
            Ok(Cmd::Back) | Ok(Cmd::Forward) | Ok(Cmd::Reload) if live.is_none() => {}
            Ok(Cmd::Reload) => {
                let _ = live.as_ref().map(|l| l.page.call("Page.reload", json!({})));
            }
            Ok(c @ (Cmd::Back | Cmd::Forward)) => {
                if let Some(l) = live.as_ref() {
                    let back = matches!(c, Cmd::Back);
                    if let Ok(h) = l.page.call("Page.getNavigationHistory", json!({})) {
                        let i = h["currentIndex"].as_i64().unwrap_or(0) + if back { -1 } else { 1 };
                        if let Some(e) = h["entries"].as_array().and_then(|e| e.get(i.max(0) as usize)).filter(|_| i >= 0) {
                            let _ = l.page.call("Page.navigateToHistoryEntry", json!({"entryId": e["id"]}));
                        }
                    }
                }
            }
            Ok(Cmd::Input(v)) => {
                if let Some(l) = live.as_ref() {
                    input(&l.page, &v);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        // Every half second: is the browser there, and which page is it on
        // (an agent may have opened or switched tabs)?
        if checked.elapsed() > Duration::from_millis(500) && (showing || live.is_some()) {
            checked = Instant::now();
            match live.as_mut() {
                None if showing => {
                    if let Some(page) = Page::open_running() {
                        let mut l = Live { page, casting: false, view: View { live: true, ..View::default() } };
                        refresh(&mut l);
                        report(&l.view);
                        live = Some(l);
                    } else {
                        // Not running (the receiver drops repeats).
                        report(&View::default());
                    }
                }
                None => {}
                Some(l) => {
                    let alive = l.page.call("Runtime.evaluate", json!({"expression": "1", "returnByValue": true})).is_ok();
                    if !alive {
                        live = None;
                        let _ = out.send(Update::Frame(String::new()));
                        report(&View::default());
                        continue;
                    }
                    if let Some(a) = browser::active_page().filter(|a| *a != l.page.target) {
                        if l.page.attach(&a).is_ok() {
                            l.casting = false;
                            refresh(l);
                            report(&l.view);
                        }
                    }
                }
            }
        }
        let Some(l) = live.as_mut() else { continue };
        if showing && !l.casting {
            let (w, h) = browser::VIEWPORT;
            l.casting = l.page.call("Page.startScreencast", json!({"format": "jpeg", "quality": 70, "maxWidth": w, "maxHeight": h, "everyNthFrame": 1})).is_ok();
        }
        let mut changed = false;
        let mut frame = None;
        for e in l.page.cdp.drain() {
            if e.session.as_deref() != Some(l.page.session.as_str()) {
                continue;
            }
            match e.method.as_str() {
                "Page.screencastFrame" => {
                    l.page.cdp.send("Page.screencastFrameAck", json!({"sessionId": e.params["sessionId"]}), Some(&l.page.session));
                    frame = e.params["data"].as_str().map(String::from);
                }
                "Page.frameNavigated" if e.params["frame"]["parentId"].is_null() => {
                    l.view.url = e.params["frame"]["url"].as_str().unwrap_or("").to_string();
                    changed = true;
                }
                "Page.loadEventFired" | "Page.domContentEventFired" => changed = true,
                _ => {}
            }
        }
        // Only the latest frame matters.
        if let Some(f) = frame {
            let _ = out.send(Update::Frame(f));
        }
        if changed {
            refresh(l);
            report(&l.view);
        }
    }
}

fn refresh(l: &mut Live) {
    let (title, url) = l.page.location();
    l.view.title = title;
    if !url.is_empty() {
        l.view.url = url;
    }
    l.view.live = true;
    l.view.starting = false;
}

/// The view's input as DevTools input events (fire and forget: the
/// screencast shows the result).
fn input(p: &Page, v: &Value) {
    let s = Some(p.session.as_str());
    let num = |k: &str| v[k].as_f64().unwrap_or(0.0);
    let mods = v["mods"].as_u64().unwrap_or(0);
    match v["k"].as_str() {
        Some("mouse") => p.cdp.send(
            "Input.dispatchMouseEvent",
            json!({"type": v["type"], "x": num("x"), "y": num("y"), "button": v["button"].as_str().unwrap_or("none"), "clickCount": v["clicks"].as_u64().unwrap_or(0), "modifiers": mods}),
            s,
        ),
        Some("wheel") => p.cdp.send("Input.dispatchMouseEvent", json!({"type": "mouseWheel", "x": num("x"), "y": num("y"), "deltaX": num("dx"), "deltaY": num("dy"), "modifiers": mods}), s),
        Some("key") => {
            let mut e = json!({"type": v["type"], "key": v["key"], "code": v["code"], "windowsVirtualKeyCode": v["vk"], "modifiers": mods});
            if let Some(t) = v["text"].as_str().filter(|t| !t.is_empty()) {
                e["text"] = json!(t);
            }
            p.cdp.send("Input.dispatchKeyEvent", e, s);
        }
        Some("text") => p.cdp.send("Input.insertText", json!({"text": v["text"]}), s),
        _ => {}
    }
}
