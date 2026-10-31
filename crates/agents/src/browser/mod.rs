//! The browser Larrier shares with its agents: the real browser views in
//! Larrier's Browser tab (WebView2, with a profile of their own), driven
//! over their DevTools port. Agents use them through `Larrier mcp-browser`
//! (an MCP server on stdio, added to their runs), so the user sees and can
//! use the very pages the agent works in.

pub mod cdp;
pub mod control;
pub mod mcp;

use std::path::PathBuf;
use std::time::Duration;

pub use cdp::{Cdp, Event};
use serde_json::{json, Value};

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

/// Larrier's own browser views (WebView2 in the app's Browser tab): their
/// profile folder.
pub fn view_dir() -> PathBuf {
    larrier_core::store::data_root().join("browser-view")
}

/// The views' DevTools websocket address, when Larrier has them open.
pub fn view_running() -> Option<String> {
    let text = std::fs::read_to_string(view_dir().join("EBWebView").join("DevToolsActivePort")).ok()?;
    let mut lines = text.lines();
    let port = lines.next()?.trim().to_string();
    let path = lines.next()?.trim().to_string();
    let addr = format!("127.0.0.1:{port}").parse().ok()?;
    std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(300)).ok()?;
    Some(format!("ws://127.0.0.1:{port}{path}"))
}

/// The page target showing Larrier tab `tab` (each view tells its pages
/// their tab as `window.__larrierTab`).
pub fn view_target(cdp: &Cdp, tab: u64) -> Option<String> {
    for t in pages(cdp).ok()? {
        let Some(id) = t["targetId"].as_str() else { continue };
        let Ok(r) = cdp.call("Target.attachToTarget", json!({"targetId": id, "flatten": true}), None) else { continue };
        let Some(session) = r["sessionId"].as_str() else { continue };
        let v = cdp.call("Runtime.evaluate", json!({"expression": "window.__larrierTab", "returnByValue": true}), Some(session));
        let _ = cdp.call("Target.detachFromTarget", json!({"sessionId": session}), None);
        if v.ok().and_then(|v| v["result"]["value"].as_u64()) == Some(tab) {
            return Some(id.to_string());
        }
    }
    None
}

/// A picture of Larrier tab `tab` as it looks now (base64 JPEG).
pub fn view_snapshot(tab: u64) -> Option<String> {
    view_capture(tab, None)
}

/// A picture of part of tab `tab` (x, y, width, height in CSS pixels of the
/// viewport), or all of it (base64 JPEG).
pub fn view_capture(tab: u64, clip: Option<(f64, f64, f64, f64)>) -> Option<String> {
    let cdp = Cdp::connect(&view_running()?).ok()?;
    let target = view_target(&cdp, tab)?;
    let r = cdp.call("Target.attachToTarget", json!({"targetId": target, "flatten": true}), None).ok()?;
    let session = r["sessionId"].as_str()?.to_string();
    let mut params = json!({"format": "jpeg", "quality": 85});
    if let Some((x, y, w, h)) = clip {
        params["clip"] = json!({"x": x.max(0.0), "y": y.max(0.0), "width": w.max(1.0), "height": h.max(1.0), "scale": 1});
    }
    let shot = cdp.call_for("Page.captureScreenshot", params, Some(&session), Duration::from_secs(3)).ok()?;
    shot["data"].as_str().map(String::from)
}

/// The open pages (target ids), oldest first.
pub fn pages(cdp: &Cdp) -> Result<Vec<Value>, String> {
    let r = cdp.call("Target.getTargets", json!({}), None)?;
    Ok(r["targetInfos"].as_array().into_iter().flatten().filter(|t| t["type"] == "page").cloned().collect())
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
