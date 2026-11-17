//! Signing a Codex account in the way OpenAI's own clients do: through
//! `codex app-server` (`account/login/start`), in the account's own
//! `CODEX_HOME`. ChatGPT sign-in opens OpenAI's page in the browser and
//! the app server finishes it; an API key is handed to it directly.

use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};

/// A Codex app server for one account's folder, past its handshake.
struct Server {
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    next: u64,
}

impl Server {
    async fn start(program: &Path, home: &Path) -> Result<Server> {
        std::fs::create_dir_all(home).with_context(|| format!("creating {}", home.display()))?;
        let (prog, prefix) = crate::request::unwrap_shim(program);
        let mut cmd = tokio::process::Command::new(prog);
        cmd.args(prefix).arg("app-server").env("CODEX_HOME", home);
        cmd.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000);
        let mut child = cmd.spawn().context("starting Codex's app server")?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let lines = BufReader::new(child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?).lines();
        let mut s = Server { child, stdin, lines, next: 1 };
        let version = env!("CARGO_PKG_VERSION");
        s.call("initialize", json!({"clientInfo": {"name": "lary", "title": "Lary", "version": version}, "capabilities": {"experimentalApi": true}})).await?;
        s.write(json!({"method": "initialized"})).await?;
        Ok(s)
    }

    async fn write(&mut self, v: Value) -> Result<()> {
        self.stdin.write_all(format!("{v}\n").as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    /// Send a request and wait for its answer.
    async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next;
        self.next += 1;
        self.write(json!({"id": id, "method": method, "params": params})).await?;
        let wait = async {
            while let Some(line) = self.lines.next_line().await? {
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                if v["id"].as_u64() == Some(id) && v.get("method").is_none() {
                    if let Some(e) = v.get("error") {
                        bail!("{}", e["message"].as_str().unwrap_or("Codex refused"));
                    }
                    return Ok(v["result"].clone());
                }
            }
            bail!("Codex's app server stopped")
        };
        tokio::time::timeout(Duration::from_secs(60), wait).await.map_err(|_| anyhow!("Codex's app server didn't answer"))?
    }

    /// Wait for a notification.
    async fn until(&mut self, method: &str, within: Duration) -> Result<Value> {
        let wait = async {
            while let Some(line) = self.lines.next_line().await? {
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                if v["method"].as_str() == Some(method) {
                    return Ok(v["params"].clone());
                }
            }
            bail!("Codex's app server stopped")
        };
        tokio::time::timeout(within, wait).await.map_err(|_| anyhow!("the sign-in wasn't finished in time"))?
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

/// Sign in with ChatGPT: `open` gets the page to show (OpenAI's sign-in);
/// this returns once Codex says the sign-in finished (within 15 minutes).
pub async fn chatgpt(program: &Path, home: &Path, open: impl FnOnce(&str)) -> Result<()> {
    let mut s = Server::start(program, home).await?;
    let started = s.call("account/login/start", json!({"type": "chatgpt"})).await?;
    let url = started["authUrl"].as_str().ok_or_else(|| anyhow!("Codex didn't give a sign-in page"))?;
    open(url);
    let done = s.until("account/login/completed", Duration::from_secs(15 * 60)).await?;
    if done["success"].as_bool() == Some(true) {
        Ok(())
    } else {
        bail!("{}", done["error"].as_str().unwrap_or("the sign-in didn't go through"))
    }
}

/// Sign in with an API key (kept by Codex in the account's folder).
pub async fn api_key(program: &Path, home: &Path, key: &str) -> Result<()> {
    let mut s = Server::start(program, home).await?;
    s.call("account/login/start", json!({"type": "apiKey", "apiKey": key})).await?;
    Ok(())
}
