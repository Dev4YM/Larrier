//! Lary's own copies of the harnesses it runs the official way: Claude
//! Code, Codex, Cursor Agent and Grok Build, downloaded from each vendor's
//! own release source into Lary's folder (checked against the vendor's
//! published checksum where there is one). Lary never runs, updates or
//! configures the copies installed on the computer unless an agent is set
//! to use its installed one.
//!
//! Layout: `<data root>/runtimes/<agent>/current.json` names the version
//! and its program; each version sits in its own folder beside it.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use lary_core::AgentKind;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Agents Lary can keep a copy of.
pub fn supported(agent: &AgentKind) -> bool {
    matches!(agent, AgentKind::ClaudeCode | AgentKind::Codex | AgentKind::Cursor | AgentKind::Grok)
}

fn dir(agent: &AgentKind) -> PathBuf {
    lary_core::store::data_root().join("runtimes").join(agent.id())
}

#[derive(Serialize, Deserialize)]
struct Current {
    version: String,
    /// The program, relative to the agent's folder.
    program: String,
}

fn current(agent: &AgentKind) -> Option<Current> {
    serde_json::from_str(&std::fs::read_to_string(dir(agent).join("current.json")).ok()?).ok()
}

/// The program of Lary's copy, when it has been downloaded.
pub fn program(agent: &AgentKind) -> Option<PathBuf> {
    let c = current(agent)?;
    let p = dir(agent).join(c.program);
    p.exists().then_some(p)
}

/// The version of Lary's copy.
pub fn version(agent: &AgentKind) -> Option<String> {
    program(agent)?;
    current(agent).map(|c| c.version)
}

/// What the newest official release is and where its file is.
struct Release {
    version: String,
    url: String,
    /// SHA-256 the vendor publishes for the file (hex), if it does.
    sha256: Option<String>,
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder().user_agent(concat!("Lary/", env!("CARGO_PKG_VERSION"))).connect_timeout(std::time::Duration::from_secs(20)).build()?)
}

async fn text(url: &str) -> Result<String> {
    let r = client()?.get(url).send().await.with_context(|| format!("reaching {url}"))?;
    if !r.status().is_success() {
        bail!("{url} answered {}", r.status());
    }
    Ok(r.text().await?.trim().to_string())
}

async fn json(url: &str) -> Result<serde_json::Value> {
    let r = client()?.get(url).header("Accept", "application/vnd.github+json").send().await.with_context(|| format!("reaching {url}"))?;
    if !r.status().is_success() {
        bail!("{url} answered {}", r.status());
    }
    Ok(r.json().await?)
}

fn arm() -> bool {
    cfg!(target_arch = "aarch64")
}

/// The newest release for this computer, from the vendor's own source.
async fn latest(agent: &AgentKind) -> Result<Release> {
    match agent {
        // Anthropic's native builds, as `claude.ai/install.{ps1,sh}` fetch
        // them: the newest version, then its manifest's checksum.
        AgentKind::ClaudeCode => {
            const BASE: &str = "https://downloads.claude.ai/claude-code-releases";
            let platform = match (std::env::consts::OS, arm()) {
                ("windows", false) => "win32-x64",
                ("windows", true) => "win32-arm64",
                ("macos", false) => "darwin-x64",
                ("macos", true) => "darwin-arm64",
                ("linux", false) => "linux-x64",
                ("linux", true) => "linux-arm64",
                _ => bail!("Claude Code has no build for this system"),
            };
            let version = text(&format!("{BASE}/latest")).await?;
            if !version.starts_with(|c: char| c.is_ascii_digit()) {
                bail!("downloads.claude.ai didn't name a version (it may not be available in your region)");
            }
            let manifest = json(&format!("{BASE}/{version}/manifest.json")).await?;
            let sha256 = manifest["platforms"][platform]["checksum"].as_str().map(str::to_lowercase);
            let exe = if cfg!(windows) { "claude.exe" } else { "claude" };
            Ok(Release { url: format!("{BASE}/{version}/{platform}/{exe}"), version, sha256 })
        }
        // OpenAI's releases on GitHub, with the digest GitHub keeps.
        AgentKind::Codex => {
            let rel = json("https://api.github.com/repos/openai/codex/releases/latest").await?;
            let tag = rel["tag_name"].as_str().unwrap_or_default();
            let version = tag.trim_start_matches("rust-v").trim_start_matches('v').to_string();
            let arch = if arm() { "aarch64" } else { "x86_64" };
            let name = match std::env::consts::OS {
                "windows" => format!("codex-{arch}-pc-windows-msvc.exe"),
                "macos" => format!("codex-{arch}-apple-darwin.tar.gz"),
                "linux" => format!("codex-{arch}-unknown-linux-musl.tar.gz"),
                _ => bail!("Codex has no build for this system"),
            };
            let asset = rel["assets"].as_array().into_iter().flatten().find(|a| a["name"].as_str() == Some(name.as_str())).ok_or_else(|| anyhow!("Codex {version} has no {name}"))?;
            let sha256 = asset["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(str::to_lowercase);
            Ok(Release { version, url: asset["browser_download_url"].as_str().unwrap_or_default().to_string(), sha256 })
        }
        // Cursor's install script names the current build.
        AgentKind::Cursor => {
            let script = text("https://cursor.com/install").await?;
            let marker = "downloads.cursor.com/lab/";
            let at = script.find(marker).ok_or_else(|| anyhow!("Cursor's install script didn't name a build"))?;
            let version: String = script[at + marker.len()..].chars().take_while(|c| *c != '/').collect();
            let (os, ext) = match std::env::consts::OS {
                "windows" => ("windows", "zip"),
                "macos" => ("darwin", "tar.gz"),
                "linux" => ("linux", "tar.gz"),
                _ => bail!("Cursor Agent has no build for this system"),
            };
            let arch = if arm() { "arm64" } else { "x64" };
            Ok(Release { url: format!("https://downloads.cursor.com/lab/{version}/{os}/{arch}/agent-cli-package.{ext}"), version, sha256: None })
        }
        // xAI's stable channel, as `x.ai/cli/install.{ps1,sh}` fetch it.
        AgentKind::Grok => {
            const BASE: &str = "https://x.ai/cli";
            let version = text(&format!("{BASE}/stable")).await?;
            let os = match std::env::consts::OS {
                "windows" => "windows",
                "macos" => "macos",
                "linux" => "linux",
                _ => bail!("Grok Build has no build for this system"),
            };
            let arch = if arm() { "aarch64" } else { "x86_64" };
            let ext = if cfg!(windows) { ".exe" } else { "" };
            Ok(Release { url: format!("{BASE}/grok-{version}-{os}-{arch}{ext}"), version, sha256: None })
        }
        _ => bail!("Lary doesn't keep its own copy of {}", agent.display_name()),
    }
}

/// The newest official version, without downloading it.
pub async fn latest_version(agent: &AgentKind) -> Result<String> {
    Ok(latest(agent).await?.version)
}

/// Download the newest official release of `agent` into Lary's folder and
/// make it the one Lary runs. Reports progress (bytes, total). Returns the
/// version and its program.
pub async fn fetch(agent: &AgentKind, mut progress: impl FnMut(u64, u64)) -> Result<(String, PathBuf)> {
    let rel = latest(agent).await?;
    let root = dir(agent);
    std::fs::create_dir_all(&root).with_context(|| format!("creating {}", root.display()))?;
    let mut resp = client()?.get(&rel.url).send().await.with_context(|| format!("downloading {}", rel.url))?;
    if !resp.status().is_success() {
        bail!("{} answered {}", rel.url, resp.status());
    }
    let total = resp.content_length().unwrap_or(0);
    let mut bytes: Vec<u8> = Vec::with_capacity(total as usize);
    let mut hasher = Sha256::new();
    while let Some(chunk) = resp.chunk().await.context("downloading")? {
        hasher.update(&chunk);
        bytes.extend_from_slice(&chunk);
        progress(bytes.len() as u64, total);
    }
    if let Some(want) = &rel.sha256 {
        let got: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if &got != want {
            bail!("the download of {} {} doesn't match its published checksum", agent.display_name(), rel.version);
        }
    }
    let program = place(agent, &root, &rel, &bytes)?;
    let relative = program.strip_prefix(&root).unwrap_or(&program).to_string_lossy().to_string();
    std::fs::write(root.join("current.json"), serde_json::to_vec_pretty(&Current { version: rel.version.clone(), program: relative })?)?;
    tidy(&root, &rel.version);
    Ok((rel.version, program))
}

/// Put a downloaded release in its folder, laid out as the vendor's own
/// installer lays it out, and return its program.
fn place(agent: &AgentKind, root: &Path, rel: &Release, bytes: &[u8]) -> Result<PathBuf> {
    let exe = |name: &str| if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    match agent {
        AgentKind::ClaudeCode | AgentKind::Grok | AgentKind::Codex if !rel.url.ends_with(".tar.gz") => {
            let name = match agent {
                AgentKind::ClaudeCode => "claude",
                AgentKind::Grok => "grok",
                _ => "codex",
            };
            let folder = root.join(&rel.version);
            std::fs::create_dir_all(&folder)?;
            let program = folder.join(exe(name));
            write_program(&program, bytes)?;
            Ok(program)
        }
        // Codex for macOS / Linux: one program in a tar.gz.
        AgentKind::Codex => {
            let folder = root.join(&rel.version);
            unpack(bytes, "tar.gz", &folder)?;
            let found = std::fs::read_dir(&folder)?.flatten().map(|e| e.path()).find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("codex-"))).ok_or_else(|| anyhow!("the Codex download had no program"))?;
            let program = folder.join("codex");
            std::fs::rename(&found, &program)?;
            make_executable(&program)?;
            Ok(program)
        }
        // Cursor: `versions/<build>/` holds its runtime, and its launchers
        // sit beside `versions/`, as its installer leaves them.
        AgentKind::Cursor => {
            let ext = if rel.url.ends_with(".zip") { "zip" } else { "tar.gz" };
            let staging = root.join(format!("{}.unpacking", rel.version));
            let _ = std::fs::remove_dir_all(&staging);
            unpack(bytes, ext, &staging)?;
            let package = staging.join("dist-package");
            let package = if package.is_dir() { package } else { staging.clone() };
            let version_dir = root.join("versions").join(&rel.version);
            let _ = std::fs::remove_dir_all(&version_dir);
            std::fs::create_dir_all(version_dir.parent().unwrap_or(root))?;
            std::fs::rename(&package, &version_dir).with_context(|| format!("moving Cursor into {}", version_dir.display()))?;
            let _ = std::fs::remove_dir_all(&staging);
            if cfg!(windows) {
                for e in std::fs::read_dir(&version_dir)?.flatten() {
                    let name = e.file_name().to_string_lossy().to_string();
                    if name.starts_with("cursor-agent") {
                        std::fs::copy(e.path(), root.join(&name))?;
                    }
                }
                let program = root.join("cursor-agent.cmd");
                if !program.exists() {
                    bail!("the Cursor download had no launcher");
                }
                Ok(program)
            } else {
                let program = version_dir.join("cursor-agent");
                make_executable(&program)?;
                Ok(program)
            }
        }
        _ => bail!("Lary doesn't keep its own copy of {}", agent.display_name()),
    }
}

fn write_program(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    let _ = std::fs::remove_file(path);
    std::fs::rename(&tmp, path)?;
    make_executable(path)
}

fn make_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    let _ = path;
    Ok(())
}

/// Unpack an archive with the system's `tar` (Windows 10 and later, macOS
/// and Linux all have one; it reads zip too).
fn unpack(bytes: &[u8], ext: &str, into: &Path) -> Result<()> {
    std::fs::create_dir_all(into)?;
    let archive = into.with_extension(ext);
    std::fs::write(&archive, bytes)?;
    let mut cmd = std::process::Command::new("tar");
    cmd.arg(if ext == "zip" { "-xf" } else { "-xzf" }).arg(&archive).arg("-C").arg(into);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let ok = cmd.status().map(|s| s.success()).unwrap_or(false);
    let _ = std::fs::remove_file(&archive);
    if !ok {
        bail!("couldn't unpack the download");
    }
    Ok(())
}

/// Older versions go once a new one is in place (one that's running stays
/// until it can be removed).
fn tidy(root: &Path, keep: &str) {
    for base in [root.to_path_buf(), root.join("versions")] {
        for e in std::fs::read_dir(&base).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let versioned = name.starts_with(|c: char| c.is_ascii_digit());
            if e.path().is_dir() && versioned && name != keep {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_official_harnesses() {
        assert!(supported(&AgentKind::ClaudeCode) && supported(&AgentKind::Codex) && supported(&AgentKind::Cursor) && supported(&AgentKind::Grok));
        assert!(!supported(&AgentKind::Gemini) && !supported(&AgentKind::OpenCode));
    }

    /// Reaches the vendors (run by hand: `cargo test -p lary-agents runtime -- --ignored`).
    #[tokio::test]
    #[ignore]
    async fn each_vendor_names_its_newest_release() {
        for a in [AgentKind::ClaudeCode, AgentKind::Codex, AgentKind::Cursor, AgentKind::Grok] {
            let r = latest(&a).await.unwrap_or_else(|e| panic!("{}: {e:#}", a.display_name()));
            println!("{}: {} {} sha256={:?}", a.display_name(), r.version, r.url, r.sha256);
            assert!(!r.version.is_empty() && r.url.starts_with("https://"));
        }
    }
}
