//! The browser Larrier shares with its agents: a headless Edge or Chrome
//! with a profile of its own, driven over the DevTools protocol. Agents use
//! it through `Larrier mcp-browser` (an MCP server on stdio, added to their
//! runs); the Browser tab shows it live and passes the user's clicks and
//! keys on, so the user can watch and take over.

pub mod cdp;
pub mod mcp;

use std::path::PathBuf;
use std::time::{Duration, Instant};

pub use cdp::{Cdp, Event};
use serde_json::{json, Value};

/// The page size agents and the Browser tab see.
pub const VIEWPORT: (u32, u32) = (1280, 800);

/// The MCP server's name as agents know it (`mcp__larrier_browser__…`).
pub const SERVER_NAME: &str = "larrier_browser";

/// How an agent starts the browser server: Larrier's own program.
#[derive(Clone, Debug, PartialEq)]
pub struct Server {
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

static SERVER: std::sync::OnceLock<Server> = std::sync::OnceLock::new();

/// Called once by the app: this program serves the browser tools.
pub fn register(exe: &std::path::Path) {
    let _ = SERVER.set(Server {
        command: exe.to_string_lossy().to_string(),
        args: vec!["mcp-browser".into()],
        env: vec![("LARRIER_HOME".into(), larrier_core::store::data_root().to_string_lossy().to_string())],
    });
}

pub fn server() -> Option<Server> {
    SERVER.get().cloned()
}

impl Server {
    /// Claude Code's `--mcp-config` (and Cursor's mcp.json) shape.
    pub fn mcp_json(&self) -> Value {
        let env: serde_json::Map<String, Value> = self.env.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
        json!({"mcpServers": {SERVER_NAME: {"type": "stdio", "command": self.command, "args": self.args, "env": env}}})
    }

    /// An ACP `session/new` server entry.
    pub fn acp(&self) -> Value {
        let env: Vec<Value> = self.env.iter().map(|(k, v)| json!({"name": k, "value": v})).collect();
        json!({"name": SERVER_NAME, "command": self.command, "args": self.args, "env": env})
    }

    /// Codex config overrides (`-c key=value`).
    pub fn codex_args(&self) -> Vec<String> {
        let s = |v: &str| serde_json::to_string(v).unwrap_or_default();
        let args = self.args.iter().map(|a| s(a)).collect::<Vec<_>>().join(", ");
        let env = self.env.iter().map(|(k, v)| format!("{k} = {}", s(v))).collect::<Vec<_>>().join(", ");
        let key = format!("mcp_servers.{SERVER_NAME}");
        vec![
            "-c".into(),
            format!("{key}.command={}", s(&self.command)),
            "-c".into(),
            format!("{key}.args=[{args}]"),
            "-c".into(),
            format!("{key}.env={{ {env} }}"),
            // Its tools run without asking each time.
            "-c".into(),
            format!("{key}.default_tools_approval_mode=\"approve\""),
        ]
    }
}

pub fn profile_dir() -> PathBuf {
    larrier_core::store::data_root().join("browser")
}

/// The page the user and the agents are on (a target id), so the Browser
/// tab follows an agent that opens or switches tabs.
fn active_file() -> PathBuf {
    profile_dir().join("larrier-active-page")
}

pub fn active_page() -> Option<String> {
    std::fs::read_to_string(active_file()).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Edge or Chrome on this machine.
pub fn find_browser() -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    #[cfg(windows)]
    for var in ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"] {
        if let Some(base) = std::env::var_os(var).map(PathBuf::from) {
            found.push(base.join(r"Google\Chrome\Application\chrome.exe"));
        }
    }
    // Edge is on every Windows; Chrome first when there is one (a new Edge
    // profile signs into the Windows account and opens sync pages).
    #[cfg(windows)]
    for var in ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"] {
        if let Some(base) = std::env::var_os(var).map(PathBuf::from) {
            found.push(base.join(r"Microsoft\Edge\Application\msedge.exe"));
        }
    }
    #[cfg(target_os = "macos")]
    for p in [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
    ] {
        found.push(PathBuf::from(p));
    }
    found
        .into_iter()
        .find(|p| p.is_file())
        .or_else(|| ["msedge", "google-chrome", "chrome", "chromium", "chromium-browser"].iter().find_map(|n| which::which(n).ok()))
}

/// The shared browser's websocket address, when it is running.
pub fn running() -> Option<String> {
    let text = std::fs::read_to_string(profile_dir().join("DevToolsActivePort")).ok()?;
    let mut lines = text.lines();
    let port = lines.next()?.trim().to_string();
    let path = lines.next()?.trim().to_string();
    // A file left by a browser that's gone names a closed port.
    let addr = format!("127.0.0.1:{port}").parse().ok()?;
    std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(300)).ok()?;
    Some(format!("ws://127.0.0.1:{port}{path}"))
}

/// The shared browser, started if it isn't running.
pub fn ensure() -> Result<String, String> {
    if let Some(u) = running() {
        return Ok(u);
    }
    let exe = find_browser().ok_or("No Edge or Chrome was found to run the agents' browser.")?;
    let dir = profile_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(dir.join("DevToolsActivePort"));
    let mut cmd = std::process::Command::new(&exe);
    cmd.args(["--headless=new", "--remote-debugging-port=0", "--no-first-run", "--no-default-browser-check", "--disable-sync"])
        .arg(format!("--user-data-dir={}", dir.display()))
        .arg(format!("--window-size={},{}", VIEWPORT.0, VIEWPORT.1))
        .arg("about:blank")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    let spawned = {
        use std::os::windows::process::CommandExt;
        // Detached, in a group of its own, out of the agent's job: it
        // outlives the run that started it (Larrier closes it on exit).
        cmd.creation_flags(0x08 | 0x200 | 0x0100_0000);
        cmd.spawn().or_else(|_| {
            cmd.creation_flags(0x08 | 0x200);
            cmd.spawn()
        })
    };
    #[cfg(not(windows))]
    let spawned = cmd.spawn();
    spawned.map_err(|e| format!("couldn't start {}: {e}", exe.display()))?;
    let until = Instant::now() + Duration::from_secs(20);
    while Instant::now() < until {
        if let Some(u) = running() {
            return Ok(u);
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    Err("The agents' browser didn't start.".into())
}

/// Close the shared browser (when Larrier quits).
pub fn shutdown() {
    if let Some(url) = running() {
        if let Ok(c) = Cdp::connect(&url) {
            let _ = c.call_for("Browser.close", json!({}), None, Duration::from_secs(3));
        }
    }
}

/// The open pages (target ids), oldest first.
pub fn pages(cdp: &Cdp) -> Result<Vec<Value>, String> {
    let r = cdp.call("Target.getTargets", json!({}), None)?;
    Ok(r["targetInfos"].as_array().into_iter().flatten().filter(|t| t["type"] == "page").cloned().collect())
}

/// One page of the shared browser, attached over a session.
pub struct Page {
    pub cdp: Cdp,
    pub target: String,
    pub session: String,
}

impl Page {
    /// The page the user and agents are on, starting the browser if needed.
    pub fn open() -> Result<Page, String> {
        Self::on(&ensure()?)
    }

    /// The page the user and agents are on, if the browser is running.
    pub fn open_running() -> Option<Page> {
        Self::on(&running()?).ok()
    }

    fn on(url: &str) -> Result<Page, String> {
        let cdp = Cdp::connect(url)?;
        // The browser's own pages (a sign-in or sync prompt) aren't anyone's.
        let mut list = pages(&cdp)?;
        list.retain(|t| {
            let own = ["edge://", "chrome://"].iter().any(|p| t["url"].as_str().unwrap_or("").starts_with(p));
            if own {
                let _ = cdp.call("Target.closeTarget", json!({"targetId": t["targetId"]}), None);
            }
            !own
        });
        let id = |t: &Value| t["targetId"].as_str().map(String::from);
        let target = match active_page().filter(|a| list.iter().any(|t| id(t).as_deref() == Some(a.as_str()))) {
            Some(t) => t,
            None => match list.first().and_then(id) {
                Some(t) => t,
                None => new_page(&cdp, "about:blank")?,
            },
        };
        let mut p = Page { cdp, target: String::new(), session: String::new() };
        p.attach(&target)?;
        Ok(p)
    }

    /// Switch to another page (and make it the one shown).
    pub fn attach(&mut self, target: &str) -> Result<(), String> {
        let r = self.cdp.call("Target.attachToTarget", json!({"targetId": target, "flatten": true}), None)?;
        self.session = r["sessionId"].as_str().ok_or("couldn't attach to the page")?.to_string();
        self.target = target.to_string();
        for m in ["Page.enable", "Runtime.enable"] {
            let _ = self.call(m, json!({}));
        }
        // Look like the browser it is, not a headless one.
        if let Ok(v) = self.cdp.call("Browser.getVersion", json!({}), None) {
            if let Some(ua) = v["userAgent"].as_str() {
                let _ = self.call("Emulation.setUserAgentOverride", json!({"userAgent": ua.replace("HeadlessChrome", "Chrome")}));
            }
        }
        let _ = self.cdp.call("Target.activateTarget", json!({"targetId": target}), None);
        let _ = std::fs::write(active_file(), target);
        Ok(())
    }

    pub fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        self.cdp.call(method, params, Some(&self.session))
    }

    /// Run JavaScript in the page; its value (awaited) or the error it threw.
    pub fn eval(&self, expr: &str) -> Result<Value, String> {
        let r = self.call("Runtime.evaluate", json!({"expression": expr, "returnByValue": true, "awaitPromise": true, "userGesture": true}))?;
        if let Some(ex) = r.get("exceptionDetails") {
            let msg = ex["exception"]["description"].as_str().or_else(|| ex["text"].as_str()).unwrap_or("the script failed");
            return Err(msg.lines().next().unwrap_or(msg).to_string());
        }
        Ok(r["result"]["value"].clone())
    }

    /// Wait for the page to finish loading (or `wait` to pass).
    pub fn settle(&self, wait: Duration) {
        let until = Instant::now() + wait;
        std::thread::sleep(Duration::from_millis(250));
        while Instant::now() < until {
            if self.eval("document.readyState").ok().and_then(|v| v.as_str().map(|s| s == "complete")).unwrap_or(false) {
                return;
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    /// Title and address.
    pub fn location(&self) -> (String, String) {
        let v = self.eval("[document.title, location.href]").unwrap_or(Value::Null);
        (v[0].as_str().unwrap_or("").to_string(), v[1].as_str().unwrap_or("").to_string())
    }

    pub fn navigate(&self, url: &str) -> Result<(), String> {
        let r = self.call("Page.navigate", json!({"url": url}))?;
        if let Some(e) = r["errorText"].as_str().filter(|e| !e.is_empty()) {
            return Err(format!("couldn't open {url}: {e}"));
        }
        self.settle(Duration::from_secs(20));
        Ok(())
    }
}

/// Open a new page and return its target id.
pub fn new_page(cdp: &Cdp, url: &str) -> Result<String, String> {
    let r = cdp.call("Target.createTarget", json!({"url": url}), None)?;
    r["targetId"].as_str().map(String::from).ok_or_else(|| "couldn't open a page".into())
}

/// An address as typed: a URL, a host, or words to search for.
pub fn address(input: &str) -> String {
    let t = input.trim();
    if t.contains("://") || t.starts_with("about:") || t.starts_with("data:") {
        return t.to_string();
    }
    let host_like = !t.contains(' ') && (t.contains('.') || t.starts_with("localhost") || t.contains(':'));
    if host_like {
        let local = t.starts_with("localhost") || t.starts_with("127.") || t.starts_with("[::1]");
        return format!("{}://{t}", if local { "http" } else { "https" });
    }
    let q: String = t.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else if b == b' ' { "+".into() } else { format!("%{b:02X}") }).collect();
    format!("https://www.bing.com/search?q={q}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        assert_eq!(address("example.com"), "https://example.com");
        assert_eq!(address("localhost:3000/x"), "http://localhost:3000/x");
        assert_eq!(address("http://a.b"), "http://a.b");
        assert_eq!(address("rust lang"), "https://www.bing.com/search?q=rust+lang");
    }
}
