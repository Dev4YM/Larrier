//! `Larrier mcp-browser`: an MCP server on stdio that gives an agent the
//! shared browser (open pages, read them, click, type, screenshots…). The
//! user watches the same browser in Larrier's Browser tab.

use std::io::{BufRead, Write};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::{address, new_page, pages, Page};

const INSTRUCTIONS: &str = "Larrier's browser, which the user watches live in Larrier's Browser tab. \
Open pages with navigate, read them with snapshot (it lists the page's controls with refs like e12 and its text), \
then click, type or select by ref. Take a screenshot to see the page. Logins and cookies persist between runs.";

/// Serve until stdin closes; the exit code.
pub fn serve() -> i32 {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut page: Option<Page> = None;
    let mut notes: Vec<String> = Vec::new();
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
                json!({"result": call(&mut page, &mut notes, name, &args)})
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

/// Run a tool, reconnecting once if the browser went away (closed, restarted).
fn call(page: &mut Option<Page>, notes: &mut Vec<String>, name: &str, args: &Value) -> Value {
    for attempt in 0..2 {
        if page.is_none() {
            match Page::open() {
                Ok(p) => *page = Some(p),
                Err(e) => return failed(e),
            }
        }
        let p = page.as_mut().unwrap();
        match run(p, notes, name, args) {
            Ok(mut v) => {
                // Dialogs answered meanwhile are worth knowing about.
                if !notes.is_empty() {
                    let extra = notes.drain(..).collect::<Vec<_>>().join("\n");
                    if let Some(c) = v["content"].as_array_mut() {
                        c.push(json!({"type": "text", "text": extra}));
                    }
                }
                return v;
            }
            Err(e) if attempt == 0 && (e.contains("connection closed") || e.contains("browser closed") || e.contains("No target") || e.contains("not found")) => {
                *page = None;
            }
            Err(e) => return failed(e),
        }
    }
    failed("The browser isn't reachable.")
}

/// Answer dialogs the page opened (alert, confirm, beforeunload) so
/// nothing hangs; say what they said.
fn handle_events(p: &Page, notes: &mut Vec<String>) {
    for e in p.cdp.drain() {
        // (The connection accepts it at once; this says what it said.)
        if e.method == "Page.javascriptDialogOpening" && e.session.as_deref() == Some(p.session.as_str()) {
            let kind = e.params["type"].as_str().unwrap_or("dialog");
            let msg = e.params["message"].as_str().unwrap_or("");
            notes.push(format!("The page showed a{} {kind}, accepted: {msg}", if kind.starts_with(['a', 'e', 'i', 'o', 'u']) { "n" } else { "" }));
        }
    }
}

fn state(p: &Page) -> String {
    let (title, url) = p.location();
    format!("Page: {} — {}", if title.is_empty() { "(untitled)" } else { &title }, url)
}

/// Find an element by ref, CSS selector or visible text; scroll it into
/// view and return its centre.
fn locate(p: &Page, args: &Value) -> Result<(f64, f64, String), String> {
    let spec = json!({"ref": args["ref"], "selector": args["selector"], "text": args["text"].as_str().filter(|_| args["ref"].is_null() && args["selector"].is_null())});
    let js = format!(
        r#"((a) => {{
  let el = null;
  if (a.ref) el = document.querySelector('[data-lb="' + a.ref + '"]');
  if (!el && a.selector) el = document.querySelector(a.selector);
  if (!el && a.text) {{
    const all = [...document.querySelectorAll('a,button,[role=button],[role=link],[role=tab],[role=menuitem],input[type=submit],input[type=button],label,summary,[onclick]')];
    const t = (e) => (e.innerText || e.value || e.getAttribute('aria-label') || '').trim();
    el = all.find(e => t(e) === a.text) || all.find(e => t(e).toLowerCase().includes(a.text.toLowerCase()));
  }}
  if (!el) return null;
  el.scrollIntoView({{block: 'center', inline: 'center'}});
  const r = el.getBoundingClientRect();
  const lab = el.labels && el.labels[0] ? el.labels[0].innerText : '';
  const name = (el.getAttribute('aria-label') || lab || el.getAttribute('placeholder') || el.innerText || el.value || '').trim().replace(/\s+/g, ' ').slice(0, 60);
  return [r.left + r.width / 2, r.top + r.height / 2, el.tagName.toLowerCase() + (name ? ' "' + name + '"' : '')];
}})({spec})"#
    );
    let v = p.eval(&js)?;
    if v.is_null() {
        let what = args["ref"].as_str().map(|r| format!("ref {r}")).or_else(|| args["selector"].as_str().map(|s| format!("selector {s}"))).or_else(|| args["text"].as_str().map(|t| format!("text \"{t}\""))).unwrap_or_else(|| "the element".into());
        return Err(format!("No element matches {what}. Take a new snapshot: refs change when the page does."));
    }
    Ok((v[0].as_f64().unwrap_or(0.0), v[1].as_f64().unwrap_or(0.0), v[2].as_str().unwrap_or("").to_string()))
}

fn mouse(p: &Page, kind: &str, x: f64, y: f64, button: &str, clicks: u32) -> Result<(), String> {
    p.call("Input.dispatchMouseEvent", json!({"type": kind, "x": x, "y": y, "button": button, "clickCount": clicks}))?;
    Ok(())
}

fn click_at(p: &Page, x: f64, y: f64, button: &str, double: bool) -> Result<(), String> {
    mouse(p, "mouseMoved", x, y, "none", 0)?;
    for n in 1..=(if double { 2 } else { 1 }) {
        mouse(p, "mousePressed", x, y, button, n)?;
        mouse(p, "mouseReleased", x, y, button, n)?;
    }
    Ok(())
}

/// A key by name ("Enter", "Control+A", "a") as CDP key events.
pub fn press(p: &Page, combo: &str) -> Result<(), String> {
    let parts: Vec<&str> = combo.split('+').map(str::trim).filter(|s| !s.is_empty()).collect();
    let (key, mods) = parts.split_last().ok_or("no key given")?;
    let mut modifiers = 0;
    for m in mods {
        modifiers |= match m.to_ascii_lowercase().as_str() {
            "alt" => 1,
            "control" | "ctrl" => 2,
            "meta" | "cmd" | "command" => 4,
            "shift" => 8,
            other => return Err(format!("unknown modifier {other}")),
        };
    }
    let (k, code, vk, text) = key_info(key);
    let mut down = json!({"type": if text.is_empty() || modifiers & 7 != 0 { "rawKeyDown" } else { "keyDown" }, "key": k, "code": code, "windowsVirtualKeyCode": vk, "modifiers": modifiers});
    if !text.is_empty() && modifiers & 7 == 0 {
        down["text"] = json!(text);
    }
    if modifiers == 2 && k.eq_ignore_ascii_case("a") {
        down["commands"] = json!(["selectAll"]);
    }
    p.call("Input.dispatchKeyEvent", down)?;
    p.call("Input.dispatchKeyEvent", json!({"type": "keyUp", "key": k, "code": code, "windowsVirtualKeyCode": vk, "modifiers": modifiers}))?;
    Ok(())
}

fn key_info(key: &str) -> (String, String, u32, String) {
    let named = |k: &str, code: &str, vk: u32, text: &str| (k.to_string(), code.to_string(), vk, text.to_string());
    match key.to_ascii_lowercase().as_str() {
        "enter" | "return" => named("Enter", "Enter", 13, "\r"),
        "tab" => named("Tab", "Tab", 9, ""),
        "escape" | "esc" => named("Escape", "Escape", 27, ""),
        "backspace" => named("Backspace", "Backspace", 8, ""),
        "delete" => named("Delete", "Delete", 46, ""),
        "space" => named(" ", "Space", 32, " "),
        "arrowup" | "up" => named("ArrowUp", "ArrowUp", 38, ""),
        "arrowdown" | "down" => named("ArrowDown", "ArrowDown", 40, ""),
        "arrowleft" | "left" => named("ArrowLeft", "ArrowLeft", 37, ""),
        "arrowright" | "right" => named("ArrowRight", "ArrowRight", 39, ""),
        "home" => named("Home", "Home", 36, ""),
        "end" => named("End", "End", 35, ""),
        "pageup" => named("PageUp", "PageUp", 33, ""),
        "pagedown" => named("PageDown", "PageDown", 34, ""),
        _ => {
            let c = key.chars().next().unwrap_or(' ');
            let up = c.to_ascii_uppercase();
            let code = if c.is_ascii_alphabetic() { format!("Key{up}") } else if c.is_ascii_digit() { format!("Digit{c}") } else { String::new() };
            (key.to_string(), code, up as u32, key.to_string())
        }
    }
}

fn run(p: &mut Page, notes: &mut Vec<String>, name: &str, a: &Value) -> Result<Value, String> {
    handle_events(p, notes);
    let s = |k: &str| a[k].as_str().map(str::trim).filter(|v| !v.is_empty());
    let out = match name {
        "navigate" => {
            let url = address(s("url").ok_or("give a url")?);
            p.navigate(&url)?;
            text(state(p))
        }
        "snapshot" => {
            let v = p.eval(SNAPSHOT_JS)?;
            let els: Vec<&str> = v["elements"].as_array().into_iter().flatten().filter_map(|x| x.as_str()).collect();
            let mut t = format!("Page: {} — {}\n\nControls (use the ref to act on one):\n{}", v["title"].as_str().unwrap_or(""), v["url"].as_str().unwrap_or(""), if els.is_empty() { "(none)".into() } else { els.join("\n") });
            t.push_str(&format!("\n\nText:\n{}", v["text"].as_str().unwrap_or("")));
            if v["more"].as_bool() == Some(true) {
                t.push_str("\n… (more text below; scroll or use evaluate to read it)");
            }
            text(t)
        }
        "click" => {
            let button = s("button").unwrap_or("left");
            let double = a["double"].as_bool().unwrap_or(false);
            let (x, y, what) = match (a["x"].as_f64(), a["y"].as_f64()) {
                (Some(x), Some(y)) if a["ref"].is_null() && a["selector"].is_null() && a["text"].is_null() => (x, y, format!("at {x:.0},{y:.0}")),
                _ => locate(p, a)?,
            };
            click_at(p, x, y, button, double)?;
            p.settle(Duration::from_secs(10));
            handle_events(p, notes);
            text(format!("Clicked {what}.\n{}", state(p)))
        }
        "hover" => {
            let (x, y, what) = locate(p, a)?;
            mouse(p, "mouseMoved", x, y, "none", 0)?;
            text(format!("Hovering over {what}."))
        }
        "type" => {
            let value = a["text"].as_str().ok_or("give the text to type")?;
            let mut target = a.clone();
            target["text"] = Value::Null;
            let what = if target["ref"].is_null() && target["selector"].is_null() {
                "the focused element".to_string()
            } else {
                let (x, y, what) = locate(p, &target)?;
                click_at(p, x, y, "left", false)?;
                what
            };
            if a["clear"].as_bool().unwrap_or(true) {
                press(p, "Control+A")?;
                press(p, "Backspace")?;
            }
            p.call("Input.insertText", json!({"text": value}))?;
            if a["submit"].as_bool().unwrap_or(false) {
                press(p, "Enter")?;
                p.settle(Duration::from_secs(10));
            }
            text(format!("Typed into {what}.\n{}", state(p)))
        }
        "press_key" => {
            let key = s("key").ok_or("give a key, like Enter or Control+A")?;
            press(p, key)?;
            p.settle(Duration::from_secs(5));
            text(format!("Pressed {key}.\n{}", state(p)))
        }
        "select_option" => {
            let values: Vec<String> = match &a["values"] {
                Value::Array(v) => v.iter().filter_map(|x| x.as_str().map(String::from)).collect(),
                Value::String(v) => vec![v.clone()],
                _ => a["value"].as_str().map(|v| vec![v.to_string()]).ok_or("give the option(s) to pick")?,
            };
            locate(p, a)?;
            let js = format!(
                r#"((sel, want) => {{
  const el = sel.ref ? document.querySelector('[data-lb="' + sel.ref + '"]') : document.querySelector(sel.selector);
  if (!el || el.tagName !== 'SELECT') return 'not a select';
  const picked = [];
  for (const o of el.options) {{ const on = want.includes(o.value) || want.includes(o.label) || want.includes(o.text.trim()); if (on || !el.multiple) o.selected = on; if (on) picked.push(o.text.trim()); }}
  el.dispatchEvent(new Event('input', {{bubbles: true}}));
  el.dispatchEvent(new Event('change', {{bubbles: true}}));
  return picked.length ? 'Selected ' + picked.join(', ') : 'No option matched';
}})({}, {})"#,
                json!({"ref": a["ref"], "selector": a["selector"]}),
                json!(values)
            );
            text(p.eval(&js)?.as_str().unwrap_or("").to_string())
        }
        "scroll" => {
            if a["ref"].is_string() || a["selector"].is_string() {
                let (_, _, what) = locate(p, a)?;
                text(format!("Scrolled {what} into view."))
            } else {
                let dy = a["dy"].as_f64().unwrap_or(600.0);
                let (w, h) = (super::VIEWPORT.0 as f64 / 2.0, super::VIEWPORT.1 as f64 / 2.0);
                p.call("Input.dispatchMouseEvent", json!({"type": "mouseWheel", "x": w, "y": h, "deltaX": a["dx"].as_f64().unwrap_or(0.0), "deltaY": dy}))?;
                std::thread::sleep(Duration::from_millis(300));
                let at = p.eval("[Math.round(scrollY), Math.round(document.documentElement.scrollHeight - innerHeight)]")?;
                text(format!("Scrolled to {} of {} px.", at[0], at[1]))
            }
        }
        "screenshot" => {
            let full = a["full_page"].as_bool().unwrap_or(false);
            let mut params = json!({"format": "png", "captureBeyondViewport": full});
            if full {
                let m = p.call("Page.getLayoutMetrics", json!({}))?;
                let size = &m["cssContentSize"];
                params["clip"] = json!({"x": 0, "y": 0, "width": size["width"], "height": size["height"].as_f64().unwrap_or(800.0).min(12000.0), "scale": 1});
            }
            let r = p.call("Page.captureScreenshot", params)?;
            json!({"content": [{"type": "image", "data": r["data"], "mimeType": "image/png"}, {"type": "text", "text": state(p)}]})
        }
        "evaluate" => {
            let expr = s("expression").ok_or("give a JavaScript expression")?;
            let v = match p.eval(expr) {
                Err(e) if e.contains("SyntaxError") => p.eval(&format!("(async () => {{ {expr} }})()"))?,
                r => r?,
            };
            let mut t = if v.is_string() { v.as_str().unwrap().to_string() } else { serde_json::to_string_pretty(&v).unwrap_or_default() };
            if t.len() > 20_000 {
                let cut = (0..=20_000).rev().find(|i| t.is_char_boundary(*i)).unwrap_or(0);
                t.truncate(cut);
                t.push_str("\n… (cut)");
            }
            text(if t.is_empty() { "(no value)".into() } else { t })
        }
        "wait_for" => {
            let secs = a["seconds"].as_f64().unwrap_or(if a["text"].is_string() { 15.0 } else { 2.0 }).clamp(0.0, 60.0);
            match s("text") {
                Some(t) => {
                    let until = Instant::now() + Duration::from_secs_f64(secs);
                    let js = format!("(document.body ? document.body.innerText : '').includes({})", json!(t));
                    loop {
                        if p.eval(&js)?.as_bool() == Some(true) {
                            break text(format!("\"{t}\" is on the page.\n{}", state(p)));
                        }
                        if Instant::now() > until {
                            break failed(format!("\"{t}\" didn't appear within {secs:.0}s.\n{}", state(p)));
                        }
                        handle_events(p, notes);
                        std::thread::sleep(Duration::from_millis(250));
                    }
                }
                None => {
                    std::thread::sleep(Duration::from_secs_f64(secs));
                    text(format!("Waited {secs:.1}s.\n{}", state(p)))
                }
            }
        }
        "back" | "forward" => {
            let h = p.call("Page.getNavigationHistory", json!({}))?;
            let i = h["currentIndex"].as_i64().unwrap_or(0) + if name == "back" { -1 } else { 1 };
            let entry = h["entries"].as_array().and_then(|e| e.get(i.max(0) as usize)).filter(|_| i >= 0).ok_or(format!("Nothing to go {name} to."))?;
            p.call("Page.navigateToHistoryEntry", json!({"entryId": entry["id"]}))?;
            p.settle(Duration::from_secs(15));
            text(state(p))
        }
        "reload" => {
            p.call("Page.reload", json!({}))?;
            p.settle(Duration::from_secs(20));
            text(state(p))
        }
        "tabs" => {
            let action = s("action").unwrap_or("list");
            let list = pages(&p.cdp)?;
            let pick = |list: &[Value]| -> Result<String, String> {
                let i = a["index"].as_u64().ok_or("give the tab's index (from tabs list)")? as usize;
                list.get(i).and_then(|t| t["targetId"].as_str()).map(String::from).ok_or_else(|| format!("There's no tab {i}."))
            };
            match action {
                "new" => {
                    let url = s("url").map(address).unwrap_or_else(|| "about:blank".into());
                    let id = new_page(&p.cdp, &url)?;
                    p.attach(&id)?;
                    p.settle(Duration::from_secs(20));
                    text(format!("Opened a new tab.\n{}", state(p)))
                }
                "select" => {
                    let id = pick(&list)?;
                    p.attach(&id)?;
                    text(state(p))
                }
                "close" => {
                    let id = pick(&list)?;
                    p.cdp.call("Target.closeTarget", json!({"targetId": id}), None)?;
                    if id == p.target {
                        let rest = pages(&p.cdp)?;
                        let next = match rest.first().and_then(|t| t["targetId"].as_str()).map(String::from) {
                            Some(n) => n,
                            None => new_page(&p.cdp, "about:blank")?,
                        };
                        p.attach(&next)?;
                    }
                    text(format!("Closed the tab.\n{}", state(p)))
                }
                _ => {
                    let lines: Vec<String> = list
                        .iter()
                        .enumerate()
                        .map(|(i, t)| format!("{i}: {}{} — {}", if t["targetId"].as_str() == Some(p.target.as_str()) { "(current) " } else { "" }, t["title"].as_str().unwrap_or(""), t["url"].as_str().unwrap_or("")))
                        .collect();
                    text(lines.join("\n"))
                }
            }
        }
        "upload_file" => {
            let files: Vec<String> = a["paths"].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect();
            if files.is_empty() {
                return Err("give the file paths".into());
            }
            let (_, _, what) = locate(p, a)?;
            let sel = json!({"ref": a["ref"], "selector": a["selector"]});
            let obj = p.call("Runtime.evaluate", json!({"expression": format!("((s) => s.ref ? document.querySelector('[data-lb=\"' + s.ref + '\"]') : document.querySelector(s.selector))({sel})")}))?;
            let oid = obj["result"]["objectId"].as_str().ok_or("couldn't reach the file input")?;
            p.call("DOM.setFileInputFiles", json!({"files": files, "objectId": oid}))?;
            text(format!("Chose {} file(s) in {what}.", files.len()))
        }
        other => return Ok(failed(format!("Unknown tool {other}"))),
    };
    Ok(out)
}

/// The page's controls (each given a ref the other tools take) and its text.
const SNAPSHOT_JS: &str = r#"(() => {
  const MAX = 250;
  let n = window.__lbN || 0;
  const vis = (el) => { const r = el.getBoundingClientRect(); if (r.width < 1 || r.height < 1) return false; const cs = getComputedStyle(el); return cs.visibility !== 'hidden' && cs.display !== 'none'; };
  const clean = (t) => (t || '').trim().replace(/\s+/g, ' ').slice(0, 80);
  // A <label> wrapping its control also holds the control's own text (a
  // select's options): leave that out.
  const labelled = (el) => { const l = el.labels && el.labels[0]; if (!l) return ''; const own = el.tagName === 'SELECT' || el.tagName === 'TEXTAREA' ? el.innerText : ''; return own ? l.innerText.replace(own, '') : l.innerText; };
  const label = (el) => clean(el.getAttribute('aria-label') || labelled(el) || el.getAttribute('placeholder') || el.getAttribute('title') || el.getAttribute('alt') || el.innerText || (el.type !== 'password' ? el.value : '') || '');
  const role = (el) => {
    const r = el.getAttribute('role'); if (r) return r;
    const t = el.tagName.toLowerCase();
    if (t === 'a') return 'link';
    if (t === 'button' || t === 'summary') return 'button';
    if (t === 'select') return 'combobox';
    if (t === 'textarea') return 'textbox';
    if (t === 'input') { const ty = (el.type || 'text').toLowerCase(); return ({checkbox: 'checkbox', radio: 'radio', submit: 'button', button: 'button', reset: 'button', range: 'slider', file: 'file'})[ty] || 'textbox'; }
    if (el.isContentEditable) return 'textbox';
    return 'clickable';
  };
  const sel = 'a[href],button,input:not([type=hidden]),select,textarea,summary,[role=button],[role=link],[role=checkbox],[role=radio],[role=tab],[role=menuitem],[role=option],[role=switch],[role=combobox],[role=textbox],[role=searchbox],[contenteditable=""],[contenteditable="true"],[onclick]';
  const lines = [];
  for (const el of document.querySelectorAll(sel)) {
    if (lines.length >= MAX) break;
    if (!vis(el)) continue;
    let ref = el.getAttribute('data-lb');
    if (!ref) { ref = 'e' + (++n); el.setAttribute('data-lb', ref); }
    const r = role(el);
    let line = '[' + ref + '] ' + r + ' "' + label(el) + '"';
    if ((r === 'textbox' || r === 'combobox') && el.type !== 'password' && el.value) line += ' value="' + String(el.value).slice(0, 60) + '"';
    if (el.checked) line += ' checked';
    if (el.disabled) line += ' disabled';
    const h = el.tagName === 'A' ? el.getAttribute('href') : null;
    if (h && !h.startsWith('javascript')) line += ' -> ' + h.slice(0, 80);
    lines.push(line);
  }
  window.__lbN = n;
  const text = (document.body ? document.body.innerText : '').replace(/\n{3,}/g, '\n\n').trim();
  return { title: document.title, url: location.href, elements: lines, text: text.slice(0, 8000), more: text.length > 8000 };
})()"#;

fn tools() -> Value {
    let target = json!({
        "ref": {"type": "string", "description": "An element's ref from the latest snapshot, like e12"},
        "selector": {"type": "string", "description": "A CSS selector, when there's no ref"},
    });
    let with = |extra: Value, required: &[&str]| {
        let mut props = target.clone();
        if let (Some(p), Some(e)) = (props.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                p.insert(k.clone(), v.clone());
            }
        }
        json!({"type": "object", "properties": props, "required": required})
    };
    let plain = |props: Value, required: &[&str]| json!({"type": "object", "properties": props, "required": required});
    let ro = json!({"readOnlyHint": true, "openWorldHint": true});
    let act = json!({"readOnlyHint": false, "destructiveHint": false, "openWorldHint": true});
    json!([
        {"name": "navigate", "description": "Open a URL (or a host, or words to search for) in the current tab.", "inputSchema": plain(json!({"url": {"type": "string"}}), &["url"]), "annotations": act},
        {"name": "snapshot", "description": "Read the page: its controls, each with a ref for click/type/select, and its text.", "inputSchema": plain(json!({}), &[]), "annotations": ro},
        {"name": "click", "description": "Click an element by ref, selector or visible text, or a point (x, y in page pixels).", "inputSchema": with(json!({"text": {"type": "string", "description": "Visible text of a button or link"}, "x": {"type": "number"}, "y": {"type": "number"}, "double": {"type": "boolean"}, "button": {"type": "string", "enum": ["left", "right", "middle"]}}), &[]), "annotations": act},
        {"name": "type", "description": "Type text into a field (by ref or selector; else the focused one). Replaces what's there unless clear is false; submit presses Enter.", "inputSchema": with(json!({"text": {"type": "string"}, "submit": {"type": "boolean"}, "clear": {"type": "boolean"}}), &["text"]), "annotations": act},
        {"name": "press_key", "description": "Press a key or combination: Enter, Tab, Escape, ArrowDown, Control+A, …", "inputSchema": plain(json!({"key": {"type": "string"}}), &["key"]), "annotations": act},
        {"name": "select_option", "description": "Pick option(s) of a <select> by value or label.", "inputSchema": with(json!({"values": {"type": "array", "items": {"type": "string"}}}), &["values"]), "annotations": act},
        {"name": "hover", "description": "Move the mouse over an element.", "inputSchema": with(json!({}), &[]), "annotations": act},
        {"name": "scroll", "description": "Scroll the page by dy pixels (negative is up), or bring an element (ref/selector) into view.", "inputSchema": with(json!({"dy": {"type": "number"}, "dx": {"type": "number"}}), &[]), "annotations": ro},
        {"name": "screenshot", "description": "See the page as an image (the visible part, or the full page).", "inputSchema": plain(json!({"full_page": {"type": "boolean"}}), &[]), "annotations": ro},
        {"name": "evaluate", "description": "Run JavaScript in the page and return its (awaited) value.", "inputSchema": plain(json!({"expression": {"type": "string"}}), &["expression"]), "annotations": act},
        {"name": "wait_for", "description": "Wait until some text is on the page (up to seconds), or just wait some seconds.", "inputSchema": plain(json!({"text": {"type": "string"}, "seconds": {"type": "number"}}), &[]), "annotations": ro},
        {"name": "back", "description": "Go back in the tab's history.", "inputSchema": plain(json!({}), &[]), "annotations": act},
        {"name": "forward", "description": "Go forward in the tab's history.", "inputSchema": plain(json!({}), &[]), "annotations": act},
        {"name": "reload", "description": "Reload the page.", "inputSchema": plain(json!({}), &[]), "annotations": act},
        {"name": "tabs", "description": "List tabs, open a new one (url), or select/close one by index.", "inputSchema": plain(json!({"action": {"type": "string", "enum": ["list", "new", "select", "close"]}, "index": {"type": "integer"}, "url": {"type": "string"}}), &[]), "annotations": act},
        {"name": "upload_file", "description": "Choose files in a file input (by ref or selector).", "inputSchema": with(json!({"paths": {"type": "array", "items": {"type": "string"}}}), &["paths"]), "annotations": act},
    ])
}
