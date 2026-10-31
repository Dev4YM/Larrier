//! `Larrier mcp-browser`: an MCP server on stdio that gives an agent
//! Larrier's browser: the real browser views in Larrier's Browser tab, so
//! the user watches (and can use) the very pages the agent works in. The
//! tools follow Claude in Chrome's: tabs, navigate, computer (mouse, keys,
//! screenshots), read_page and find (refs), form_input, page text,
//! JavaScript, console and network logs, resizing, uploads and GIFs.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{json, Value};

use super::{address, control, view_running, view_target, Cdp};

const INSTRUCTIONS: &str = "Larrier's browser: real browser tabs the user sees live in Larrier's Browser tab (logins and cookies persist). \
Start with tabs_context to get the tab ids (it opens a tab if there is none), or tabs_create for a fresh tab. \
Read a page with read_page or find (they give elements refs like ref_12), get_page_text for articles, and computer to click, type, press keys, scroll and take screenshots. \
Coordinates are the page's CSS pixels, the frame every screenshot reports. Prefer refs over coordinates when you have them.";

/// What the tool server keeps between calls.
struct Browser {
    cdp: Cdp,
    /// Larrier tab → DevTools session.
    sessions: HashMap<u64, String>,
    /// Console messages and network requests seen per tab.
    console: HashMap<u64, Vec<Value>>,
    network: HashMap<u64, Vec<Value>>,
    /// Screenshots taken (id → base64 JPEG), for upload_image.
    shots: Vec<(String, String)>,
    shot_n: u64,
    /// A GIF being recorded: frames (base64 JPEG, a click point).
    recording: Option<Vec<Frame>>,
    stopped: Option<Vec<Frame>>,
    notes: Vec<String>,
}

#[derive(Clone)]
struct Frame {
    jpeg: String,
    click: Option<(f64, f64)>,
    label: String,
}

/// Serve until stdin closes; the exit code.
pub fn serve() -> i32 {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut browser: Option<Browser> = None;
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(msg) = serde_json::from_str::<Value>(line.trim()) else { continue };
        let Some(id) = msg.get("id").cloned().filter(|i| !i.is_null()) else { continue };
        let params = &msg["params"];
        let reply = match msg["method"].as_str().unwrap_or("") {
            "initialize" => json!({"result": {
                "protocolVersion": params["protocolVersion"].as_str().unwrap_or("2025-06-18"),
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "larrier-browser", "title": "Larrier browser", "version": env!("CARGO_PKG_VERSION")},
                "instructions": INSTRUCTIONS,
            }}),
            "tools/list" => json!({"result": {"tools": tools()}}),
            "tools/call" => {
                let name = params["name"].as_str().unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                json!({"result": call(&mut browser, name, &args)})
            }
            "ping" => json!({"result": {}}),
            m => json!({"error": {"code": -32601, "message": format!("unknown method {m}")}}),
        };
        let mut out = reply;
        out["jsonrpc"] = json!("2.0");
        out["id"] = id;
        if writeln!(stdout, "{out}").and_then(|_| stdout.flush()).is_err() {
            break;
        }
    }
    0
}

fn text(t: impl Into<String>) -> Value {
    json!({"content": [{"type": "text", "text": t.into()}]})
}

fn failed(t: impl Into<String>) -> Value {
    json!({"content": [{"type": "text", "text": t.into()}], "isError": true})
}

/// Larrier's browser, opened by Larrier if it isn't yet.
fn connect() -> Result<Cdp, String> {
    if view_running().is_none() {
        control::request(json!({"cmd": "ensure"}))?;
        let until = Instant::now() + Duration::from_secs(20);
        while view_running().is_none() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    Cdp::connect(&view_running().ok_or("Larrier's browser didn't start.")?)
}

/// Run a tool, reconnecting once if the browser went away.
fn call(browser: &mut Option<Browser>, name: &str, args: &Value) -> Value {
    for attempt in 0..2 {
        if browser.is_none() {
            match connect() {
                Ok(cdp) => {
                    *browser = Some(Browser {
                        cdp,
                        sessions: HashMap::new(),
                        console: HashMap::new(),
                        network: HashMap::new(),
                        shots: Vec::new(),
                        shot_n: 0,
                        recording: None,
                        stopped: None,
                        notes: Vec::new(),
                    })
                }
                Err(e) => return failed(e),
            }
        }
        let b = browser.as_mut().unwrap();
        b.pump();
        match run(b, name, args) {
            Ok(mut v) => {
                b.pump();
                // Dialogs answered meanwhile are worth knowing about.
                if !b.notes.is_empty() {
                    let extra = b.notes.drain(..).collect::<Vec<_>>().join("\n");
                    if let Some(c) = v["content"].as_array_mut() {
                        c.push(json!({"type": "text", "text": extra}));
                    }
                }
                return v;
            }
            Err(e) if attempt == 0 && (e.contains("connection closed") || e.contains("browser closed")) => *browser = None,
            Err(e) => return failed(e),
        }
    }
    failed("Larrier's browser isn't reachable.")
}

/// A tab's page, attached.
struct Tab<'a> {
    b: &'a Browser,
    id: u64,
    session: String,
}

impl Tab<'_> {
    fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        self.b.cdp.call(method, params, Some(&self.session))
    }

    fn eval(&self, expr: &str) -> Result<Value, String> {
        let r = self.call("Runtime.evaluate", json!({"expression": expr, "returnByValue": true, "awaitPromise": true, "userGesture": true}))?;
        if let Some(ex) = r.get("exceptionDetails") {
            let msg = ex["exception"]["description"].as_str().or_else(|| ex["text"].as_str()).unwrap_or("the script failed");
            return Err(msg.lines().next().unwrap_or(msg).to_string());
        }
        Ok(r["result"]["value"].clone())
    }

    /// Wait for a load the last action started (or `wait` to pass).
    fn settle(&self, wait: Duration) {
        let until = Instant::now() + wait;
        std::thread::sleep(Duration::from_millis(200));
        while Instant::now() < until {
            if self.eval("document.readyState").ok().and_then(|v| v.as_str().map(|s| s == "complete")).unwrap_or(false) {
                return;
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    fn state(&self) -> String {
        let v = self.eval("[document.title, location.href]").unwrap_or(Value::Null);
        format!("Tab {}: {} — {}", self.id, v[0].as_str().filter(|t| !t.is_empty()).unwrap_or("(untitled)"), v[1].as_str().unwrap_or(""))
    }

    fn mouse(&self, kind: &str, x: f64, y: f64, button: &str, clicks: u32, mods: u32) -> Result<(), String> {
        let buttons = match (kind, button) {
            ("mousePressed", "left") => 1,
            ("mousePressed", "right") => 2,
            ("mousePressed", "middle") => 4,
            _ => 0,
        };
        self.call("Input.dispatchMouseEvent", json!({"type": kind, "x": x, "y": y, "button": button, "buttons": buttons, "clickCount": clicks, "modifiers": mods}))?;
        Ok(())
    }

    /// Where an element is (its centre), scrolled into view first.
    fn locate(&self, r: &str) -> Result<(f64, f64, String), String> {
        let v = self.eval(&format!("({LOCATE_JS})({})", json!(r)))?;
        if v.is_null() {
            return Err(format!("No element has {r}. Refs come from read_page or find, and are lost when the page changes."));
        }
        Ok((v[0].as_f64().unwrap_or(0.0), v[1].as_f64().unwrap_or(0.0), v[2].as_str().unwrap_or("").to_string()))
    }

    /// The viewport: CSS width, height and device pixel ratio.
    fn viewport(&self) -> Result<(f64, f64, f64), String> {
        let v = self.eval("[innerWidth, innerHeight, devicePixelRatio]")?;
        Ok((v[0].as_f64().unwrap_or(1280.0), v[1].as_f64().unwrap_or(800.0), v[2].as_f64().unwrap_or(1.0)))
    }

    /// A JPEG of the viewport (or a region), in CSS pixels times `scale`.
    fn capture(&self, region: Option<(f64, f64, f64, f64)>, scale: f64) -> Result<(String, u32, u32), String> {
        let (w, h, dpr) = self.viewport()?;
        let (x, y, cw, ch) = region.unwrap_or((0.0, 0.0, w, h));
        let r = self.b.cdp.call_for(
            "Page.captureScreenshot",
            json!({"format": "jpeg", "quality": 82, "clip": {"x": x, "y": y, "width": cw, "height": ch, "scale": scale / dpr}}),
            Some(&self.session),
            Duration::from_secs(15),
        )?;
        let data = r["data"].as_str().ok_or("no screenshot")?.to_string();
        Ok((data, (cw * scale).round() as u32, (ch * scale).round() as u32))
    }
}

impl Browser {
    /// Events so far: console messages, requests, dialogs (answered by the
    /// connection), sorted to their tab.
    fn pump(&mut self) {
        let by_session: HashMap<String, u64> = self.sessions.iter().map(|(t, s)| (s.clone(), *t)).collect();
        for e in self.cdp.drain() {
            let Some(tab) = e.session.as_ref().and_then(|s| by_session.get(s)).copied() else { continue };
            let p = &e.params;
            match e.method.as_str() {
                "Page.javascriptDialogOpening" => {
                    let kind = p["type"].as_str().unwrap_or("dialog");
                    self.notes.push(format!("Tab {tab} showed a {kind}, accepted: {}", p["message"].as_str().unwrap_or("")));
                }
                "Runtime.consoleAPICalled" => {
                    let msg: Vec<String> = p["args"].as_array().into_iter().flatten().map(|a| a["value"].as_str().map(String::from).or_else(|| a["description"].as_str().map(String::from)).unwrap_or_else(|| a["value"].to_string())).collect();
                    push_capped(self.console.entry(tab).or_default(), json!({"level": p["type"], "text": msg.join(" "), "url": p["stackTrace"]["callFrames"][0]["url"]}));
                }
                "Runtime.exceptionThrown" => {
                    let d = &p["exceptionDetails"];
                    let t = d["exception"]["description"].as_str().or_else(|| d["text"].as_str()).unwrap_or("exception");
                    push_capped(self.console.entry(tab).or_default(), json!({"level": "error", "text": t, "url": d["url"]}));
                }
                "Log.entryAdded" => {
                    let en = &p["entry"];
                    push_capped(self.console.entry(tab).or_default(), json!({"level": en["level"], "text": en["text"], "url": en["url"]}));
                }
                "Network.requestWillBeSent" => {
                    push_capped(self.network.entry(tab).or_default(), json!({"requestId": p["requestId"], "method": p["request"]["method"], "url": p["request"]["url"], "type": p["type"]}));
                }
                "Network.responseReceived" | "Network.loadingFailed" => {
                    if let Some(r) = self.network.get_mut(&tab).and_then(|v| v.iter_mut().rev().find(|r| r["requestId"] == p["requestId"])) {
                        if e.method == "Network.loadingFailed" {
                            r["failed"] = p["errorText"].clone();
                        } else {
                            r["status"] = p["response"]["status"].clone();
                            r["mimeType"] = p["response"]["mimeType"].clone();
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// The tabs as Larrier has them: (id, url, title, active).
    fn tabs(&self) -> Result<Vec<(u64, String, String, bool)>, String> {
        let v = control::request(json!({"cmd": "tabs"}))?;
        Ok(v["tabs"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|t| (t["id"].as_u64().unwrap_or(0), t["url"].as_str().unwrap_or("").to_string(), t["title"].as_str().unwrap_or("").to_string(), t["active"].as_bool().unwrap_or(false)))
            .collect())
    }

    /// Open a tab in Larrier and wait until its page can be driven.
    fn new_tab(&mut self, url: &str) -> Result<u64, String> {
        let v = control::request(json!({"cmd": "new_tab", "url": url}))?;
        let id = v["id"].as_u64().ok_or("Larrier couldn't open a tab")?;
        let until = Instant::now() + Duration::from_secs(20);
        while Instant::now() < until {
            if view_target(&self.cdp, id).is_some() {
                return Ok(id);
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        Err("The new tab didn't open.".into())
    }

    /// Tab `id` (or the one shown, opening one if there is none), attached.
    fn tab(&mut self, id: Option<u64>) -> Result<Tab<'_>, String> {
        let id = match id {
            Some(i) => i,
            None => {
                let list = self.tabs()?;
                match list.iter().find(|t| t.3).or(list.first()) {
                    Some(t) => t.0,
                    None => self.new_tab("about:blank")?,
                }
            }
        };
        let alive = match self.sessions.get(&id) {
            Some(s) => self.cdp.call_for("Runtime.evaluate", json!({"expression": "1"}), Some(s), Duration::from_secs(3)).is_ok(),
            None => false,
        };
        if !alive {
            let target = view_target(&self.cdp, id).ok_or(format!("There's no tab {id}. tabs_context lists them."))?;
            let r = self.cdp.call("Target.attachToTarget", json!({"targetId": target, "flatten": true}), None)?;
            let session = r["sessionId"].as_str().ok_or("couldn't attach to the tab")?.to_string();
            for m in ["Page.enable", "Runtime.enable", "Log.enable", "Network.enable"] {
                let _ = self.cdp.call(m, json!({}), Some(&session));
            }
            self.sessions.insert(id, session);
        }
        let session = self.sessions[&id].clone();
        Ok(Tab { b: self, id, session })
    }

    fn keep_shot(&mut self, data: &str) -> String {
        self.shot_n += 1;
        let id = format!("ss_{}", self.shot_n);
        self.shots.push((id.clone(), data.to_string()));
        if self.shots.len() > 20 {
            self.shots.remove(0);
        }
        id
    }

    /// While recording a GIF: a frame of how the tab looks after an action.
    fn frame(&mut self, tab: Option<u64>, label: &str, click: Option<(f64, f64)>) {
        if self.recording.is_none() {
            return;
        }
        let Ok(t) = self.tab(tab) else { return };
        let Ok((w, _, _)) = t.viewport() else { return };
        let scale = (800.0 / w).min(1.0);
        if let Ok((jpeg, _, _)) = t.capture(None, scale) {
            let click = click.map(|(x, y)| (x * scale, y * scale));
            if let Some(f) = self.recording.as_mut() {
                f.push(Frame { jpeg, click, label: label.to_string() });
            }
        }
    }
}

fn push_capped(v: &mut Vec<Value>, item: Value) {
    v.push(item);
    if v.len() > 500 {
        v.remove(0);
    }
}

/// Modifier names ("ctrl+shift") as a CDP bitmask.
fn modifiers(s: &str) -> Result<u32, String> {
    let mut m = 0;
    for part in s.split('+').map(str::trim).filter(|p| !p.is_empty()) {
        m |= match part.to_ascii_lowercase().as_str() {
            "alt" | "option" => 1,
            "ctrl" | "control" => 2,
            "cmd" | "meta" | "command" | "win" | "windows" | "super" => 4,
            "shift" => 8,
            other => return Err(format!("unknown modifier {other}")),
        };
    }
    Ok(m)
}

/// A key or chord ("Enter", "ctrl+a", "a") as CDP key events.
fn press(t: &Tab, combo: &str) -> Result<(), String> {
    let parts: Vec<&str> = combo.split('+').map(str::trim).filter(|s| !s.is_empty()).collect();
    let (key, mods) = parts.split_last().ok_or("no key given")?;
    let m = modifiers(&mods.join("+"))?;
    let (k, code, vk, text) = key_info(key)?;
    let mut down = json!({"type": if text.is_empty() || m & 7 != 0 { "rawKeyDown" } else { "keyDown" }, "key": k, "code": code, "windowsVirtualKeyCode": vk, "modifiers": m});
    if !text.is_empty() && m & 7 == 0 {
        down["text"] = json!(if m & 8 != 0 { text.to_uppercase() } else { text.clone() });
    }
    // Editing shortcuts the page itself doesn't see as keys.
    if m & 6 != 0 {
        let cmd = match k.to_ascii_lowercase().as_str() {
            "a" => Some("selectAll"),
            "c" => Some("copy"),
            "x" => Some("cut"),
            "v" => Some("paste"),
            "z" if m & 8 != 0 => Some("redo"),
            "z" => Some("undo"),
            "y" => Some("redo"),
            _ => None,
        };
        if let Some(c) = cmd {
            down["commands"] = json!([c]);
        }
    }
    t.call("Input.dispatchKeyEvent", down)?;
    t.call("Input.dispatchKeyEvent", json!({"type": "keyUp", "key": k, "code": code, "windowsVirtualKeyCode": vk, "modifiers": m}))?;
    Ok(())
}

fn key_info(key: &str) -> Result<(String, String, u32, String), String> {
    let named = |k: &str, code: &str, vk: u32, text: &str| Ok((k.to_string(), code.to_string(), vk, text.to_string()));
    let lower = key.to_ascii_lowercase();
    if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()).filter(|n| (1..=12).contains(n)) {
        return named(&format!("F{n}"), &format!("F{n}"), 111 + n, "");
    }
    match lower.as_str() {
        "enter" | "return" => named("Enter", "Enter", 13, "\r"),
        "tab" => named("Tab", "Tab", 9, ""),
        "escape" | "esc" => named("Escape", "Escape", 27, ""),
        "backspace" => named("Backspace", "Backspace", 8, ""),
        "delete" | "del" => named("Delete", "Delete", 46, ""),
        "space" => named(" ", "Space", 32, " "),
        "arrowup" | "up" => named("ArrowUp", "ArrowUp", 38, ""),
        "arrowdown" | "down" => named("ArrowDown", "ArrowDown", 40, ""),
        "arrowleft" | "left" => named("ArrowLeft", "ArrowLeft", 37, ""),
        "arrowright" | "right" => named("ArrowRight", "ArrowRight", 39, ""),
        "home" => named("Home", "Home", 36, ""),
        "end" => named("End", "End", 35, ""),
        "pageup" => named("PageUp", "PageUp", 33, ""),
        "pagedown" => named("PageDown", "PageDown", 34, ""),
        "insert" => named("Insert", "Insert", 45, ""),
        _ if key.chars().count() == 1 => {
            let c = key.chars().next().unwrap();
            let up = c.to_ascii_uppercase();
            let code = if c.is_ascii_alphabetic() { format!("Key{up}") } else if c.is_ascii_digit() { format!("Digit{c}") } else { String::new() };
            Ok((key.to_string(), code, up as u32, key.to_string()))
        }
        _ => Err(format!("unknown key {key}")),
    }
}

fn coord(a: &Value, key: &str) -> Option<(f64, f64)> {
    let c = a[key].as_array()?;
    Some((c.first()?.as_f64()?, c.get(1)?.as_f64()?))
}

fn run(b: &mut Browser, name: &str, a: &Value) -> Result<Value, String> {
    let s = |k: &str| a[k].as_str().map(str::trim).filter(|v| !v.is_empty());
    let tab_id = a["tabId"].as_u64();
    let out = match name {
        "tabs_context" => {
            let mut tabs = b.tabs()?;
            if tabs.is_empty() {
                b.new_tab("about:blank")?;
                tabs = b.tabs()?;
            }
            let lines: Vec<String> = tabs.iter().map(|(id, url, title, on)| format!("tabId {id}{}: {} — {url}", if *on { " (shown)" } else { "" }, if title.is_empty() { "(untitled)" } else { title })).collect();
            text(format!("Tabs in Larrier's browser:\n{}", lines.join("\n")))
        }
        "tabs_create" => {
            let url = s("url").map(address).unwrap_or_else(|| "about:blank".into());
            let id = b.new_tab(&url)?;
            let t = b.tab(Some(id))?;
            t.settle(Duration::from_secs(15));
            text(format!("Opened tab {id}.\n{}", t.state()))
        }
        "tabs_close" => {
            let id = tab_id.ok_or("give the tabId")?;
            control::request(json!({"cmd": "close_tab", "id": id}))?;
            b.sessions.remove(&id);
            text(format!("Closed tab {id}."))
        }
        "navigate" => {
            let url = s("url").ok_or("give a url (or back / forward)")?.to_string();
            let t = b.tab(tab_id)?;
            match url.as_str() {
                "back" | "forward" => {
                    let h = t.call("Page.getNavigationHistory", json!({}))?;
                    let i = h["currentIndex"].as_i64().unwrap_or(0) + if url == "back" { -1 } else { 1 };
                    let entry = h["entries"].as_array().and_then(|e| e.get(i.max(0) as usize)).filter(|_| i >= 0).ok_or(format!("Nothing to go {url} to."))?;
                    t.call("Page.navigateToHistoryEntry", json!({"entryId": entry["id"]}))?;
                }
                _ => {
                    let r = t.call("Page.navigate", json!({"url": address(&url)}))?;
                    if let Some(e) = r["errorText"].as_str().filter(|e| !e.is_empty()) {
                        return Err(format!("couldn't open {url}: {e}"));
                    }
                }
            }
            t.settle(Duration::from_secs(20));
            let st = t.state();
            let id = t.id;
            b.frame(Some(id), &format!("navigate {url}"), None);
            text(st)
        }
        "computer" => return computer(b, tab_id, a),
        "read_page" => {
            let t = b.tab(tab_id)?;
            let opts = json!({"depth": a["depth"].as_u64().unwrap_or(15), "interactive": s("filter") == Some("interactive"), "ref": s("ref_id")});
            let v = t.eval(&format!("({})({opts})", TREE_JS.replace("__A11Y__", A11Y_JS)))?;
            if v.is_null() {
                return Err(format!("No element has {}.", s("ref_id").unwrap_or("that ref")));
            }
            let max = a["max_chars"].as_u64().unwrap_or(50_000) as usize;
            let tree = v.as_str().unwrap_or("");
            text(format!("{}\n\n{}", t.state(), cut_lines(tree, max)))
        }
        "find" => {
            let q = s("query").ok_or("say what to find")?;
            let t = b.tab(tab_id)?;
            let v = t.eval(&format!("({})({})", FIND_JS.replace("__A11Y__", A11Y_JS), json!(q)))?;
            let hits: Vec<&str> = v["hits"].as_array().into_iter().flatten().filter_map(|x| x.as_str()).collect();
            let mut out = if hits.is_empty() { format!("Nothing on the page matches \"{q}\". Try read_page.") } else { hits.join("\n") };
            if v["more"].as_bool() == Some(true) {
                out.push_str("\nMore than 20 match: be more specific.");
            }
            text(out)
        }
        "form_input" => {
            let r = s("ref").ok_or("give the ref")?;
            let t = b.tab(tab_id)?;
            let v = t.eval(&format!("({FORM_JS})({}, {})", json!(r), a["value"]))?;
            match v.as_str() {
                Some(msg) if msg.starts_with("Set") || msg.starts_with("Checked") || msg.starts_with("Unchecked") || msg.starts_with("Selected") => text(msg.to_string()),
                Some(msg) => return Err(msg.to_string()),
                None => return Err(format!("No element has {r}.")),
            }
        }
        "get_page_text" => {
            let t = b.tab(tab_id)?;
            let v = t.eval(PAGE_TEXT_JS)?;
            let max = a["max_chars"].as_u64().unwrap_or(50_000) as usize;
            text(format!("{}\n\n{}", t.state(), cut_lines(v.as_str().unwrap_or(""), max)))
        }
        "javascript_tool" => {
            let code = a["text"].as_str().or_else(|| a["code"].as_str()).ok_or("give the JavaScript")?;
            let t = b.tab(tab_id)?;
            let v = match t.eval(code) {
                Err(e) if e.contains("SyntaxError") => t.eval(&format!("(async () => {{ {code} }})()"))?,
                r => r?,
            };
            let out = if v.is_string() { v.as_str().unwrap().to_string() } else { serde_json::to_string_pretty(&v).unwrap_or_default() };
            text(if out.is_empty() || out == "null" { "(no value)".into() } else { cut_lines(&out, 30_000) })
        }
        "read_console_messages" => {
            let id = b.tab(tab_id)?.id;
            b.pump();
            let only_errors = a["onlyErrors"].as_bool().unwrap_or(false);
            let pattern = s("pattern").map(|p| p.to_lowercase());
            let limit = a["limit"].as_u64().unwrap_or(50) as usize;
            let all = b.console.get(&id).cloned().unwrap_or_default();
            let picked: Vec<String> = all
                .iter()
                .filter(|m| !only_errors || matches!(m["level"].as_str(), Some("error") | Some("assert")))
                .filter(|m| pattern.as_ref().is_none_or(|p| m["text"].as_str().unwrap_or("").to_lowercase().contains(p)))
                .map(|m| format!("[{}] {}{}", m["level"].as_str().unwrap_or("log"), m["text"].as_str().unwrap_or(""), m["url"].as_str().filter(|u| !u.is_empty()).map(|u| format!(" ({u})")).unwrap_or_default()))
                .collect();
            let start = picked.len().saturating_sub(limit);
            text(if picked.is_empty() { "No console messages since the tools attached to this tab.".into() } else { picked[start..].join("\n") })
        }
        "read_network_requests" => {
            let id = b.tab(tab_id)?.id;
            b.pump();
            if let Some(req) = s("requestId") {
                let t = b.tab(Some(id))?;
                let body = t.call("Network.getResponseBody", json!({"requestId": req}))?;
                let data = body["body"].as_str().unwrap_or("");
                return Ok(text(if body["base64Encoded"].as_bool() == Some(true) { format!("(binary, {} bytes base64)", data.len()) } else { cut_lines(data, 30_000) }));
            }
            let pattern = s("urlPattern").map(|p| p.to_lowercase());
            let limit = a["limit"].as_u64().unwrap_or(50) as usize;
            let all = b.network.get(&id).cloned().unwrap_or_default();
            let picked: Vec<String> = all
                .iter()
                .filter(|r| pattern.as_ref().is_none_or(|p| r["url"].as_str().unwrap_or("").to_lowercase().contains(p)))
                .map(|r| {
                    let status = r["failed"].as_str().map(String::from).or_else(|| r["status"].as_u64().map(|s| s.to_string())).unwrap_or_else(|| "…".into());
                    format!("{} {} {} [{}] requestId={}", r["method"].as_str().unwrap_or(""), status, r["url"].as_str().unwrap_or(""), r["type"].as_str().unwrap_or(""), r["requestId"].as_str().unwrap_or(""))
                })
                .collect();
            let start = picked.len().saturating_sub(limit);
            text(if picked.is_empty() { "No requests since the tools attached to this tab.".into() } else { picked[start..].join("\n") })
        }
        "resize_window" => {
            let t = b.tab(tab_id)?;
            let (w, h, mobile) = match s("preset") {
                Some("mobile") => (375.0, 812.0, true),
                Some("tablet") => (768.0, 1024.0, false),
                Some("desktop") => {
                    t.call("Emulation.clearDeviceMetricsOverride", json!({}))?;
                    return Ok(text(format!("Back to the tab's own size.\n{}", t.state())));
                }
                _ => (a["width"].as_f64().ok_or("give width and height, or a preset")?, a["height"].as_f64().ok_or("give width and height, or a preset")?, false),
            };
            t.call("Emulation.setDeviceMetricsOverride", json!({"width": w as u32, "height": h as u32, "deviceScaleFactor": 0, "mobile": mobile}))?;
            text(format!("The page now lays out at {w}×{h}{}.", if mobile { " as a phone" } else { "" }))
        }
        "file_upload" => {
            let r = s("ref").ok_or("give the file input's ref")?;
            let files: Vec<String> = a["paths"].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect();
            if files.is_empty() {
                return Err("give the file paths".into());
            }
            let t = b.tab(tab_id)?;
            set_files(&t, r, &files)?;
            text(format!("Chose {} file(s) in {r}.", files.len()))
        }
        "upload_image" => {
            let image = s("imageId").ok_or("give the screenshot's imageId")?;
            let data = b.shots.iter().find(|(i, _)| i == image).map(|(_, d)| d.clone()).ok_or(format!("No screenshot {image} (they expire after 20 more)."))?;
            let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| e.to_string())?;
            let name = s("filename").unwrap_or("image.jpg").to_string();
            let path = std::env::temp_dir().join(format!("larrier-upload-{}-{name}", std::process::id()));
            std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
            let file = path.to_string_lossy().to_string();
            let t = b.tab(tab_id)?;
            match (s("ref"), coord(a, "coordinate")) {
                (Some(r), _) => set_files(&t, r, &[file])?,
                (None, Some((x, y))) => {
                    let data = json!({"items": [], "files": [file], "dragOperationsMask": 1});
                    for kind in ["dragEnter", "dragOver", "drop"] {
                        t.call("Input.dispatchDragEvent", json!({"type": kind, "x": x, "y": y, "data": data}))?;
                    }
                }
                _ => return Err("give a ref (a file input) or a coordinate to drop it on".into()),
            }
            text(format!("Uploaded {image} as {name}."))
        }
        "gif_creator" => {
            let action = s("action").ok_or("give the action")?;
            match action {
                "start_recording" => {
                    b.recording = Some(Vec::new());
                    b.stopped = None;
                    b.frame(tab_id, "start", None);
                    text("Recording. Each action adds a frame; stop_recording, then export.")
                }
                "stop_recording" => {
                    b.frame(tab_id, "end", None);
                    b.stopped = b.recording.take();
                    text(format!("Stopped: {} frames.", b.stopped.as_ref().map(|f| f.len()).unwrap_or(0)))
                }
                "clear" => {
                    b.recording = None;
                    b.stopped = None;
                    text("Discarded the frames.")
                }
                "export" => {
                    let frames = b.stopped.clone().or_else(|| b.recording.clone()).filter(|f| !f.is_empty()).ok_or("Nothing recorded yet.")?;
                    let name = s("filename").map(String::from).unwrap_or_else(|| format!("recording-{}.gif", chrono::Local::now().format("%Y%m%d-%H%M%S")));
                    let dir = dirs::download_dir().unwrap_or_else(std::env::temp_dir);
                    let path = dir.join(&name);
                    let opts = &a["options"];
                    gif(&frames, &path, opts["showClickIndicators"].as_bool().unwrap_or(true), opts["showProgressBar"].as_bool().unwrap_or(true))?;
                    text(format!("Saved the GIF ({} frames) to {}.", frames.len(), path.display()))
                }
                other => return Err(format!("unknown action {other}")),
            }
        }
        other => return Ok(failed(format!("Unknown tool {other}"))),
    };
    Ok(out)
}

fn computer(b: &mut Browser, tab_id: Option<u64>, a: &Value) -> Result<Value, String> {
    let action = a["action"].as_str().ok_or("give the action")?;
    let mods = modifiers(a["modifiers"].as_str().unwrap_or(""))?;
    let t = b.tab(tab_id)?;
    let id = t.id;
    // A point: the coordinate, else the ref's centre.
    let at = |t: &Tab| -> Result<(f64, f64, String), String> {
        match (coord(a, "coordinate"), a["ref"].as_str()) {
            (Some((x, y)), _) => Ok((x, y, format!("({x:.0}, {y:.0})"))),
            (None, Some(r)) => t.locate(r),
            _ => Err("give a coordinate or a ref".into()),
        }
    };
    let mut click = None;
    let msg = match action {
        "left_click" | "right_click" | "double_click" | "triple_click" => {
            let (x, y, what) = at(&t)?;
            let button = if action == "right_click" { "right" } else { "left" };
            let n = match action {
                "double_click" => 2,
                "triple_click" => 3,
                _ => 1,
            };
            t.mouse("mouseMoved", x, y, "none", 0, mods)?;
            for i in 1..=n {
                t.mouse("mousePressed", x, y, button, i, mods)?;
                t.mouse("mouseReleased", x, y, button, i, mods)?;
            }
            click = Some((x, y));
            t.settle(Duration::from_secs(8));
            format!("{} at {what}.\n{}", action.replace('_', " "), t.state())
        }
        "hover" => {
            let (x, y, what) = at(&t)?;
            t.mouse("mouseMoved", x, y, "none", 0, mods)?;
            format!("Hovering at {what}.")
        }
        "left_click_drag" => {
            let (x0, y0) = coord(a, "start_coordinate").ok_or("give start_coordinate")?;
            let (x1, y1) = coord(a, "coordinate").ok_or("give coordinate")?;
            t.mouse("mouseMoved", x0, y0, "none", 0, mods)?;
            t.mouse("mousePressed", x0, y0, "left", 1, mods)?;
            for i in 1..=12 {
                let f = i as f64 / 12.0;
                t.call("Input.dispatchMouseEvent", json!({"type": "mouseMoved", "x": x0 + (x1 - x0) * f, "y": y0 + (y1 - y0) * f, "button": "left", "buttons": 1, "modifiers": mods}))?;
            }
            t.mouse("mouseReleased", x1, y1, "left", 1, mods)?;
            click = Some((x1, y1));
            format!("Dragged from ({x0:.0}, {y0:.0}) to ({x1:.0}, {y1:.0}).")
        }
        "type" => {
            let s = a["text"].as_str().ok_or("give the text")?;
            t.call("Input.insertText", json!({"text": s}))?;
            format!("Typed {} characters.", s.chars().count())
        }
        "key" => {
            let keys = a["text"].as_str().ok_or("give the key(s), like Enter or ctrl+a")?;
            let times = a["repeat"].as_u64().unwrap_or(1).clamp(1, 100);
            for _ in 0..times {
                for k in keys.split_whitespace() {
                    press(&t, k)?;
                }
            }
            t.settle(Duration::from_secs(5));
            format!("Pressed {keys}{}.\n{}", if times > 1 { format!(" ×{times}") } else { String::new() }, t.state())
        }
        "scroll" => {
            let (w, h, _) = t.viewport()?;
            let (x, y) = coord(a, "coordinate").unwrap_or((w / 2.0, h / 2.0));
            let n = a["scroll_amount"].as_f64().unwrap_or(3.0).clamp(1.0, 10.0) * 100.0;
            let (dx, dy) = match a["scroll_direction"].as_str().unwrap_or("down") {
                "up" => (0.0, -n),
                "left" => (-n, 0.0),
                "right" => (n, 0.0),
                _ => (0.0, n),
            };
            t.call("Input.dispatchMouseEvent", json!({"type": "mouseWheel", "x": x, "y": y, "deltaX": dx, "deltaY": dy, "modifiers": mods}))?;
            std::thread::sleep(Duration::from_millis(350));
            let at = t.eval("[Math.round(scrollX), Math.round(scrollY), Math.round(document.documentElement.scrollHeight - innerHeight)]")?;
            format!("Scrolled. The page is at {}, {} (of {} down).", at[0], at[1], at[2])
        }
        "scroll_to" => {
            let r = a["ref"].as_str().ok_or("give the ref")?;
            let (_, _, what) = t.locate(r)?;
            format!("Scrolled {what} into view.")
        }
        "wait" => {
            let secs = a["duration"].as_f64().unwrap_or(1.0).clamp(0.0, 10.0);
            std::thread::sleep(Duration::from_secs_f64(secs));
            format!("Waited {secs:.1}s.\n{}", t.state())
        }
        "screenshot" | "zoom" => {
            let (w, h, _) = t.viewport()?;
            let (region, scale) = if action == "zoom" {
                let r = a["region"].as_array().ok_or("give the region: [x0, y0, x1, y1]")?;
                let n: Vec<f64> = r.iter().filter_map(|v| v.as_f64()).collect();
                if n.len() != 4 || n[2] <= n[0] || n[3] <= n[1] {
                    return Err("region is [x0, y0, x1, y1], with x1 > x0 and y1 > y0".into());
                }
                let (cw, ch) = (n[2] - n[0], n[3] - n[1]);
                (Some((n[0], n[1], cw, ch)), (1280.0 / cw).min(900.0 / ch).clamp(1.0, 4.0) * a["scale"].as_f64().unwrap_or(1.0))
            } else {
                (None, a["scale"].as_f64().unwrap_or(1.0).clamp(0.1, 1.0))
            };
            let (data, iw, ih) = t.capture(region, scale)?;
            let st = t.state();
            let shot = b.keep_shot(&data);
            let note = match region {
                Some((x, y, cw, ch)) => format!("{shot}: the region ({x:.0}, {y:.0}) to ({:.0}, {:.0}), magnified to {iw}×{ih}.", x + cw, y + ch),
                None => format!("{shot}: {iw}×{ih}{}. Coordinates are in the page's {w:.0}×{h:.0} CSS pixels.", if scale < 1.0 { " (scaled down)" } else { "" }),
            };
            return Ok(json!({"content": [{"type": "image", "data": data, "mimeType": "image/jpeg"}, {"type": "text", "text": format!("{note}\n{st}")}]}));
        }
        other => return Err(format!("unknown action {other}")),
    };
    b.frame(Some(id), action, click);
    Ok(text(msg))
}

fn set_files(t: &Tab, r: &str, files: &[String]) -> Result<(), String> {
    let obj = t.call("Runtime.evaluate", json!({"expression": format!("document.querySelector('[data-larrier-ref={}]')", json!(r))}))?;
    let oid = obj["result"]["objectId"].as_str().ok_or(format!("No element has {r}."))?;
    t.call("DOM.setFileInputFiles", json!({"files": files, "objectId": oid}))?;
    Ok(())
}

/// Text cut at a line boundary, saying how much there was.
fn cut_lines(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = (0..=max).rev().find(|i| s.is_char_boundary(*i)).unwrap_or(0);
    if let Some(nl) = s[..end].rfind('\n') {
        end = nl;
    }
    format!("{}\n… cut at {max} of {} characters (pass a larger max_chars, or use depth / ref_id)", &s[..end], s.len())
}

/// The recorded frames as an animated GIF, with where clicks landed and a
/// progress bar.
fn gif(frames: &[Frame], path: &std::path::Path, clicks: bool, progress: bool) -> Result<(), String> {
    use image::codecs::gif::{GifEncoder, Repeat};
    use image::{Delay, Frame as GifFrame, Rgba};
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = GifEncoder::new_with_speed(file, 10);
    enc.set_repeat(Repeat::Infinite).map_err(|e| e.to_string())?;
    let n = frames.len();
    for (i, f) in frames.iter().enumerate() {
        let bytes = base64::engine::general_purpose::STANDARD.decode(&f.jpeg).map_err(|e| e.to_string())?;
        let mut img = image::load_from_memory(&bytes).map_err(|e| e.to_string())?.to_rgba8();
        let (w, h) = img.dimensions();
        if clicks {
            if let Some((cx, cy)) = f.click {
                for dy in -9i32..=9 {
                    for dx in -9i32..=9 {
                        let d = ((dx * dx + dy * dy) as f64).sqrt();
                        let (x, y) = (cx as i32 + dx, cy as i32 + dy);
                        if (6.5..=9.0).contains(&d) && x >= 0 && y >= 0 && (x as u32) < w && (y as u32) < h {
                            img.put_pixel(x as u32, y as u32, Rgba([242, 116, 34, 255]));
                        }
                    }
                }
            }
        }
        if progress && n > 1 {
            let filled = ((i + 1) as f64 / n as f64 * w as f64) as u32;
            for y in h.saturating_sub(4)..h {
                for x in 0..filled.min(w) {
                    img.put_pixel(x, y, Rgba([242, 116, 34, 255]));
                }
            }
        }
        let _ = &f.label;
        enc.encode_frame(GifFrame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(if i + 1 == n { 1500 } else { 700 }, 1))).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// An element by ref, scrolled into view if it's out of it: its centre and
/// a short description.
const LOCATE_JS: &str = r#"(ref) => {
  const el = document.querySelector('[data-larrier-ref="' + ref + '"]');
  if (!el) return null;
  let r = el.getBoundingClientRect();
  if (r.bottom < 0 || r.top > innerHeight || r.right < 0 || r.left > innerWidth) { el.scrollIntoView({ block: 'center', inline: 'center' }); r = el.getBoundingClientRect(); }
  const name = (el.getAttribute('aria-label') || el.innerText || el.value || el.getAttribute('placeholder') || '').trim().replace(/\s+/g, ' ').slice(0, 50);
  return [r.left + r.width / 2, r.top + r.height / 2, ref + ' (' + el.tagName.toLowerCase() + (name ? ' "' + name + '"' : '') + ')'];
}"#;

/// Shared by read_page and find: roles, names, refs.
const A11Y_JS: &str = r#"
  const R = (el) => {
    let ref = el.getAttribute('data-larrier-ref');
    if (!ref) { window.__larrierRefN = (window.__larrierRefN || 0) + 1; ref = 'ref_' + window.__larrierRefN; el.setAttribute('data-larrier-ref', ref); }
    return ref;
  };
  const clean = (t, n) => (t || '').replace(/\s+/g, ' ').trim().slice(0, n || 100);
  const INTERACTIVE = new Set(['link', 'button', 'textbox', 'searchbox', 'combobox', 'checkbox', 'radio', 'switch', 'slider', 'spinbutton', 'tab', 'menuitem', 'menuitemcheckbox', 'menuitemradio', 'option', 'listbox', 'treeitem']);
  const role = (el) => {
    const r = el.getAttribute('role'); if (r) return r.split(' ')[0];
    const t = el.tagName.toLowerCase();
    if (t === 'a') return el.hasAttribute('href') ? 'link' : '';
    if (t === 'button' || t === 'summary') return 'button';
    if (t === 'select') return el.multiple ? 'listbox' : 'combobox';
    if (t === 'textarea') return 'textbox';
    if (t === 'option') return 'option';
    if (t === 'input') { const ty = (el.type || 'text').toLowerCase(); return ({ checkbox: 'checkbox', radio: 'radio', submit: 'button', button: 'button', reset: 'button', image: 'button', range: 'slider', number: 'spinbutton', search: 'searchbox', file: 'button', hidden: '' })[ty] ?? 'textbox'; }
    if (/^h[1-6]$/.test(t)) return 'heading';
    if (t === 'img') return 'img';
    if (t === 'nav') return 'navigation';
    if (t === 'main') return 'main';
    if (t === 'header') return 'banner';
    if (t === 'footer') return 'contentinfo';
    if (t === 'aside') return 'complementary';
    if (t === 'form') return 'form';
    if (t === 'ul' || t === 'ol') return 'list';
    if (t === 'li') return 'listitem';
    if (t === 'table') return 'table';
    if (t === 'tr') return 'row';
    if (t === 'th') return 'columnheader';
    if (t === 'td') return 'cell';
    if (t === 'dialog') return 'dialog';
    if (t === 'label') return 'label';
    if (t === 'p') return 'paragraph';
    if (t === 'iframe') return 'iframe';
    if (t === 'video' || t === 'audio') return t;
    if (el.isContentEditable && el.contentEditable === 'true') return 'textbox';
    if (el.onclick || el.getAttribute('onclick') || el.getAttribute('tabindex') === '0') return 'generic-clickable';
    return '';
  };
  const name = (el, r) => {
    const by = el.getAttribute('aria-labelledby');
    if (by) { const t = by.split(' ').map(i => document.getElementById(i)).filter(Boolean).map(e => e.innerText).join(' '); if (t.trim()) return clean(t); }
    const a = el.getAttribute('aria-label'); if (a) return clean(a);
    // A label wrapping its control holds the control's own text too (a
    // select's options): only the label's words.
    if (el.labels && el.labels.length) { el.setAttribute('data-larrier-self', ''); const c = el.labels[0].cloneNode(true); el.removeAttribute('data-larrier-self'); c.querySelectorAll('[data-larrier-self], select, textarea').forEach(n => n.remove()); const lt = clean(c.textContent); if (lt) return lt; }
    const alt = el.getAttribute('alt') || el.getAttribute('title') || el.getAttribute('placeholder'); if (alt) return clean(alt);
    if (r === 'textbox' || r === 'searchbox' || r === 'combobox' || r === 'listbox' || r === 'spinbutton' || r === 'slider') return '';
    if (['link', 'button', 'heading', 'tab', 'menuitem', 'option', 'label', 'cell', 'columnheader', 'listitem', 'generic-clickable', 'checkbox', 'radio'].includes(r)) return clean(el.innerText || el.value);
    return '';
  };
  const shown = (el) => { const s = getComputedStyle(el); if (s.display === 'none' || s.visibility === 'hidden') return false; const b = el.getBoundingClientRect(); return b.width > 0 && b.height > 0; };
  const props = (el, r) => {
    const out = [];
    if (r === 'heading') out.push('level=' + el.tagName[1]);
    if (r === 'link') { const h = el.getAttribute('href'); if (h && !h.startsWith('javascript')) out.push('href=' + clean(h, 80)); }
    if ('checked' in el && (r === 'checkbox' || r === 'radio' || r === 'switch')) out.push(el.checked ? 'checked' : 'unchecked');
    if (el.disabled) out.push('disabled');
    if (el.getAttribute('aria-expanded')) out.push('expanded=' + el.getAttribute('aria-expanded'));
    if (el.getAttribute('aria-selected') === 'true' || el.selected) out.push('selected');
    if ((r === 'textbox' || r === 'searchbox' || r === 'combobox' || r === 'spinbutton') && el.type !== 'password') { const v = el.isContentEditable ? el.innerText : el.value; if (v) out.push('value="' + clean(v, 80) + '"'); }
    if (el.type === 'password' && el.value) out.push('value=(hidden)');
    if (!shown(el)) out.push('hidden');
    return out.join(' ');
  };
"#;

/// The page as an accessibility tree: one line per element with a role,
/// each with a ref.
const TREE_JS: &str = r#"(opts) => {
  __A11Y__
  const root = opts.ref ? document.querySelector('[data-larrier-ref="' + opts.ref + '"]') : document.body;
  if (!root) return null;
  const lines = [];
  if (opts.interactive) {
    for (const el of root.querySelectorAll('*')) {
      const r = role(el);
      if (!INTERACTIVE.has(r) && r !== 'generic-clickable') continue;
      if (!shown(el)) continue;
      const n = name(el, r), p = props(el, r);
      lines.push('- ' + r + (n ? ' "' + n + '"' : '') + ' [' + R(el) + ']' + (p ? ' ' + p : ''));
    }
    return lines.join('\n') || '(no interactive elements)';
  }
  const walk = (node, depth, indent) => {
    if (depth > opts.depth) return;
    for (const c of node.childNodes) {
      if (c.nodeType === 3) {
        const t = clean(c.nodeValue, 200);
        if (t && node !== document.body && !role(node)) lines.push(indent + '- text: ' + t);
        else if (t && node === document.body) lines.push(indent + '- text: ' + t);
        continue;
      }
      if (c.nodeType !== 1) continue;
      const tag = c.tagName.toLowerCase();
      if (['script', 'style', 'noscript', 'template', 'svg', 'link', 'meta'].includes(tag) || c.getAttribute('aria-hidden') === 'true') continue;
      const r = role(c);
      if (r) {
        const n = name(c, r), p = props(c, r);
        let line = indent + '- ' + r + (n ? ' "' + n + '"' : '') + ' [' + R(c) + ']' + (p ? ' ' + p : '');
        // Leaf text that names nothing yet (a paragraph's words).
        if (!n && (r === 'paragraph' || r === 'listitem' || r === 'cell' || r === 'label') && c.children.length === 0) line += ': ' + clean(c.innerText, 200);
        lines.push(line);
        if (!(c.children.length === 0 && (n || r === 'paragraph'))) walk(c, depth + 1, indent + '  ');
      } else {
        walk(c, depth, indent);
      }
    }
  };
  if (opts.ref) { const r = role(root) || 'generic'; lines.push('- ' + r + (name(root, r) ? ' "' + name(root, r) + '"' : '') + ' [' + opts.ref + ']'); walk(root, 1, '  '); }
  else walk(root, 0, '');
  return lines.join('\n');
}"#;

/// Elements matching words: their role, name and nearby text scored
/// against the query.
const FIND_JS: &str = r#"(query) => {
  __A11Y__
  const words = query.toLowerCase().split(/[^\p{L}\p{N}]+/u).filter(w => w.length > 1 && !['the', 'a', 'an', 'of', 'to', 'in', 'on', 'for', 'and', 'with'].includes(w));
  const syn = { search: ['searchbox', 'search'], input: ['textbox', 'searchbox'], field: ['textbox', 'searchbox', 'combobox'], box: ['textbox', 'searchbox', 'checkbox'], dropdown: ['combobox', 'listbox'], menu: ['menuitem', 'navigation'], image: ['img'], picture: ['img'], title: ['heading'] };
  const hits = [];
  for (const el of document.body.querySelectorAll('*')) {
    const r = role(el);
    if (!r || r === 'list' || r === 'row' || r === 'table') continue;
    const n = name(el, r);
    const extra = clean([el.getAttribute('placeholder'), el.getAttribute('name'), el.id, el.getAttribute('type'), typeof el.className === 'string' ? el.className : ''].filter(Boolean).join(' '), 200).toLowerCase();
    const hay = (r + ' ' + n + ' ' + extra + ' ' + (r === 'paragraph' ? clean(el.innerText, 200) : '')).toLowerCase();
    let score = 0;
    for (const w of words) {
      if (hay.includes(w)) score += 2;
      else if ((syn[w] || []).some(s => hay.includes(s))) score += 1.5;
    }
    if (!score) continue;
    if (n && n.toLowerCase() === query.toLowerCase()) score += 4;
    if (INTERACTIVE.has(r)) score += 0.5;
    if (shown(el)) score += 1; else score -= 2;
    const snippet = !n && !INTERACTIVE.has(r) ? clean(el.innerText, 80) : '';
    hits.push([score, '- ' + r + (n ? ' "' + n + '"' : '') + ' [' + R(el) + ']' + (props(el, r) ? ' ' + props(el, r) : '') + (snippet ? ': ' + snippet : '')]);
  }
  hits.sort((a, b) => b[0] - a[0]);
  return { hits: hits.slice(0, 20).map(h => h[1]), more: hits.length > 20 };
}"#;

/// Set a form element by ref: text, a choice, a checkbox.
const FORM_JS: &str = r#"(ref, value) => {
  const el = document.querySelector('[data-larrier-ref="' + ref + '"]');
  if (!el) return null;
  const fire = () => { el.dispatchEvent(new Event('input', { bubbles: true })); el.dispatchEvent(new Event('change', { bubbles: true })); };
  const t = el.tagName.toLowerCase();
  if (t === 'select') {
    const want = Array.isArray(value) ? value.map(String) : [String(value)];
    const picked = [];
    for (const o of el.options) { const on = want.includes(o.value) || want.includes(o.text.trim()) || want.includes(o.label); if (on || !el.multiple) o.selected = on; if (on) picked.push(o.text.trim()); }
    fire();
    return picked.length ? 'Selected ' + picked.join(', ') : 'No option of ' + ref + ' matches ' + JSON.stringify(value);
  }
  if (t === 'input' && (el.type === 'checkbox' || el.type === 'radio')) {
    const on = value === true || value === 'true' || value === 'on' || value === 1;
    if (el.checked !== on) el.click();
    return (el.checked ? 'Checked ' : 'Unchecked ') + ref;
  }
  if (el.isContentEditable) { el.focus(); el.innerText = String(value); fire(); return 'Set ' + ref; }
  if (t === 'input' || t === 'textarea') {
    el.focus();
    const proto = t === 'input' ? HTMLInputElement.prototype : HTMLTextAreaElement.prototype;
    Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, String(value));
    fire();
    return 'Set ' + ref + ' to ' + JSON.stringify(String(value).slice(0, 80));
  }
  return ref + ' is a ' + t + ', not a form field';
}"#;

/// The page's readable text, the article or main part first.
const PAGE_TEXT_JS: &str = r#"(() => {
  const main = document.querySelector('article') || document.querySelector('main, [role=main]') || document.body;
  const t = (main ? main.innerText : '').replace(/[ \t]+\n/g, '\n').replace(/\n{3,}/g, '\n\n').trim();
  return t || '(the page has no text)';
})()"#;

fn tools() -> Value {
    let tab = json!({"type": "number", "description": "The tab (from tabs_context or tabs_create). Leave it out for the tab shown in Larrier."});
    let obj = |mut props: Value, required: &[&str]| {
        props["tabId"] = tab.clone();
        json!({"type": "object", "properties": props, "required": required})
    };
    let ro = json!({"readOnlyHint": true, "openWorldHint": true});
    let act = json!({"readOnlyHint": false, "destructiveHint": false, "openWorldHint": true});
    json!([
        {"name": "tabs_context", "description": "List the tabs in Larrier's browser (opens one if there's none). Call it first to get tab ids.", "inputSchema": {"type": "object", "properties": {"createIfEmpty": {"type": "boolean"}}}, "annotations": ro},
        {"name": "tabs_create", "description": "Open a new tab in Larrier's browser (optionally at a URL) and return its id.", "inputSchema": {"type": "object", "properties": {"url": {"type": "string"}}}, "annotations": act},
        {"name": "tabs_close", "description": "Close a tab.", "inputSchema": obj(json!({}), &["tabId"]), "annotations": act},
        {"name": "navigate", "description": "Go to a URL (or a host, or words to search for) in a tab, or \"back\" / \"forward\" in its history.", "inputSchema": obj(json!({"url": {"type": "string"}}), &["url"]), "annotations": act},
        {"name": "computer", "description": "Use the mouse and keyboard in a tab, and take screenshots. Coordinates are the page's CSS pixels (the frame screenshots report). Click buttons and links in their centre. Actions: left_click, right_click, double_click, triple_click (coordinate or ref), hover, left_click_drag (start_coordinate → coordinate), type (text into the focused element), key (space-separated keys or chords like \"ctrl+a Backspace\"; repeat), scroll (coordinate, scroll_direction, scroll_amount), scroll_to (ref), wait (duration ≤ 10 s), screenshot (scale ≤ 1 for a smaller image), zoom (region [x0, y0, x1, y1] magnified).",
         "inputSchema": obj(json!({
            "action": {"type": "string", "enum": ["left_click", "right_click", "double_click", "triple_click", "type", "screenshot", "wait", "scroll", "key", "left_click_drag", "zoom", "scroll_to", "hover"]},
            "coordinate": {"type": "array", "items": {"type": "number"}, "description": "[x, y]"},
            "start_coordinate": {"type": "array", "items": {"type": "number"}},
            "ref": {"type": "string", "description": "An element's ref from read_page or find, instead of a coordinate"},
            "text": {"type": "string"},
            "modifiers": {"type": "string", "description": "ctrl, shift, alt, cmd; combine with +"},
            "scroll_direction": {"type": "string", "enum": ["up", "down", "left", "right"]},
            "scroll_amount": {"type": "number", "minimum": 1, "maximum": 10},
            "repeat": {"type": "number", "minimum": 1, "maximum": 100},
            "duration": {"type": "number", "minimum": 0, "maximum": 10},
            "region": {"type": "array", "items": {"type": "number"}},
            "scale": {"type": "number", "minimum": 0.1, "maximum": 1}
         }), &["action"]), "annotations": act},
        {"name": "read_page", "description": "The page as an accessibility tree: each element's role, name and state, with a ref the other tools take. filter \"interactive\" lists only controls; depth and ref_id focus on part of a big page.", "inputSchema": obj(json!({"filter": {"type": "string", "enum": ["interactive", "all"]}, "depth": {"type": "number"}, "ref_id": {"type": "string"}, "max_chars": {"type": "number"}}), &[]), "annotations": ro},
        {"name": "find", "description": "Find elements by what they are or say (\"search box\", \"add to cart button\"): up to 20 matches with refs.", "inputSchema": obj(json!({"query": {"type": "string"}}), &["query"]), "annotations": ro},
        {"name": "form_input", "description": "Set a form element by ref: text for fields, a value or label for selects, true/false for checkboxes.", "inputSchema": obj(json!({"ref": {"type": "string"}, "value": {}}), &["ref", "value"]), "annotations": act},
        {"name": "get_page_text", "description": "The page's text, the article or main part first. Best for reading.", "inputSchema": obj(json!({"max_chars": {"type": "number"}}), &[]), "annotations": ro},
        {"name": "javascript_tool", "description": "Run JavaScript in the page and get its (awaited) value; `return` works too.", "inputSchema": obj(json!({"text": {"type": "string"}}), &["text"]), "annotations": act},
        {"name": "read_console_messages", "description": "The tab's console messages and errors since the tools attached to it.", "inputSchema": obj(json!({"limit": {"type": "number"}, "onlyErrors": {"type": "boolean"}, "pattern": {"type": "string"}}), &[]), "annotations": ro},
        {"name": "read_network_requests", "description": "The tab's requests since the tools attached to it (method, status, URL), or one response's body by requestId.", "inputSchema": obj(json!({"limit": {"type": "number"}, "urlPattern": {"type": "string"}, "requestId": {"type": "string"}}), &[]), "annotations": ro},
        {"name": "resize_window", "description": "Lay the page out at a size (width, height) or a preset (mobile, tablet, desktop = back to normal).", "inputSchema": obj(json!({"width": {"type": "number"}, "height": {"type": "number"}, "preset": {"type": "string", "enum": ["mobile", "tablet", "desktop"]}}), &[]), "annotations": act},
        {"name": "file_upload", "description": "Put files (absolute paths) in a file input by its ref. Don't click file inputs: that opens a picker no one can see.", "inputSchema": obj(json!({"ref": {"type": "string"}, "paths": {"type": "array", "items": {"type": "string"}}}), &["ref", "paths"]), "annotations": act},
        {"name": "upload_image", "description": "Upload a screenshot you took (its imageId, like ss_3) to a file input (ref) or by dropping it at a coordinate.", "inputSchema": obj(json!({"imageId": {"type": "string"}, "ref": {"type": "string"}, "coordinate": {"type": "array", "items": {"type": "number"}}, "filename": {"type": "string"}}), &["imageId"]), "annotations": act},
        {"name": "gif_creator", "description": "Record the browser as an animated GIF: start_recording, act (each action adds a frame), stop_recording, then export (saved to Downloads), or clear.", "inputSchema": obj(json!({"action": {"type": "string", "enum": ["start_recording", "stop_recording", "export", "clear"]}, "filename": {"type": "string"}, "download": {"type": "boolean"}, "options": {"type": "object"}}), &["action"]), "annotations": act},
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_modifiers() {
        assert_eq!(modifiers("ctrl+shift").unwrap(), 10);
        assert_eq!(modifiers("").unwrap(), 0);
        assert!(modifiers("hyper").is_err());
        assert_eq!(key_info("Enter").unwrap().2, 13);
        assert_eq!(key_info("f5").unwrap().0, "F5");
        assert_eq!(key_info("a").unwrap().1, "KeyA");
        assert!(key_info("NotAKey").is_err());
    }

    #[test]
    fn long_text_is_cut_on_a_line() {
        let s = "aaa\nbbb\nccc";
        let c = cut_lines(s, 6);
        assert!(c.starts_with("aaa\n…"), "{c}");
        assert_eq!(cut_lines("short", 100), "short");
    }

    #[test]
    fn every_tool_has_a_schema() {
        let t = tools();
        let names: Vec<&str> = t.as_array().unwrap().iter().map(|x| x["name"].as_str().unwrap()).collect();
        for n in ["tabs_context", "tabs_create", "navigate", "computer", "read_page", "find", "form_input", "get_page_text", "javascript_tool", "read_console_messages", "read_network_requests", "resize_window", "file_upload", "upload_image", "gif_creator"] {
            assert!(names.contains(&n), "{n}");
        }
        // The page-side scripts share the role helpers.
        assert!(TREE_JS.contains("__A11Y__") && FIND_JS.contains("__A11Y__"));
    }
}
