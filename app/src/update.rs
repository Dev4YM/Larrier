//! Updates from GitHub releases. A release carries the Windows program
//! (`Larrier-<version>-windows-x64.exe`) and its SHA-256
//! (`….exe.sha256`); Larrier downloads the newest one's program, checks it,
//! and on restart the new program takes the old one's place. No source
//! code is involved.
//!
//! The repository is `Dev4YM/Larrier` (`LARRIER_UPDATE_REPO` overrides it,
//! `LARRIER_UPDATE_API` points at another API root, e.g. for tests). A
//! private repository needs a token: `LARRIER_GITHUB_TOKEN`, `GITHUB_TOKEN`
//! or GitHub CLI's login.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::install::{self, Kind, Scope};

const REPO: &str = "Dev4YM/Larrier";

/// A release newer than the running Larrier.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub page: String,
    pub asset_name: String,
    asset_url: String,
    asset_api: String,
    sha_api: Option<String>,
    sha_url: Option<String>,
    pub size: u64,
}

/// The asset name a release's program has on this platform.
pub fn asset_suffix() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("-windows-x64.exe"),
        ("windows", "aarch64") => Some("-windows-arm64.exe"),
        _ => None,
    }
}

fn api_root() -> String {
    std::env::var("LARRIER_UPDATE_API").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "https://api.github.com".into())
}

fn repo() -> String {
    std::env::var("LARRIER_UPDATE_REPO").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| REPO.into())
}

/// A token for a private repository, if there is one.
fn token() -> Option<String> {
    for v in ["LARRIER_GITHUB_TOKEN", "GITHUB_TOKEN"] {
        if let Some(t) = std::env::var(v).ok().filter(|t| !t.trim().is_empty()) {
            return Some(t.trim().to_string());
        }
    }
    let mut cmd = std::process::Command::new("gh");
    cmd.args(["auth", "token"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().ok()?;
    let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !t.is_empty()).then_some(t)
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder().user_agent(concat!("Larrier/", env!("CARGO_PKG_VERSION"))).connect_timeout(Duration::from_secs(15)).build()?)
}

/// `a` is a newer version than `b` (numeric dot parts, `v` prefix and
/// pre-release suffixes ignored).
pub fn newer(a: &str, b: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> {
        v.trim().trim_start_matches(['v', 'V']).split(['-', '+']).next().unwrap_or("").split('.').map(|p| p.parse().unwrap_or(0)).collect()
    };
    let (a, b) = (parts(a), parts(b));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

/// Pick this platform's program (and its checksum) out of a release.
pub fn parse_release(v: &Value) -> Option<Release> {
    let suffix = asset_suffix()?;
    let tag = v["tag_name"].as_str()?;
    let assets = v["assets"].as_array()?;
    let asset = assets.iter().find(|a| a["name"].as_str().map(|n| n.ends_with(suffix)).unwrap_or(false))?;
    let name = asset["name"].as_str()?.to_string();
    let sha = assets.iter().find(|a| a["name"].as_str() == Some(&format!("{name}.sha256")));
    Some(Release {
        version: tag.trim_start_matches(['v', 'V']).to_string(),
        notes: v["body"].as_str().unwrap_or("").to_string(),
        page: v["html_url"].as_str().unwrap_or("").to_string(),
        asset_name: name,
        asset_url: asset["browser_download_url"].as_str().unwrap_or("").to_string(),
        asset_api: asset["url"].as_str().unwrap_or("").to_string(),
        sha_api: sha.and_then(|s| s["url"].as_str()).map(String::from),
        sha_url: sha.and_then(|s| s["browser_download_url"].as_str()).map(String::from),
        size: asset["size"].as_u64().unwrap_or(0),
    })
}

/// The newest release, when it is newer than this Larrier and has a program
/// for this platform.
pub async fn check() -> Result<Option<Release>> {
    let url = format!("{}/repos/{}/releases/latest", api_root().trim_end_matches('/'), repo());
    let mut req = client()?.get(&url).header("Accept", "application/vnd.github+json");
    if let Some(t) = token() {
        req = req.bearer_auth(t);
    }
    let resp = req.send().await.context("reaching GitHub")?;
    match resp.status().as_u16() {
        200 => {}
        404 => bail!("No releases found for {} (a private repository needs a GitHub token)", repo()),
        s => bail!("GitHub answered {s}"),
    }
    let v: Value = resp.json().await.context("reading the release")?;
    let rel = parse_release(&v).ok_or_else(|| anyhow!("The latest release has no program for this computer"))?;
    Ok(newer(&rel.version, larrier_core::APP_VERSION).then_some(rel))
}

/// Where downloaded updates wait.
fn cache_dir() -> PathBuf {
    match install::current() {
        Kind::Installed(scope) => install::layout(scope).shared_dir.join("updates"),
        _ => install::layout(Scope::User).shared_dir.join("updates"),
    }
}

/// Fetch a release asset: the browser URL, or the API URL with a token
/// (private repositories).
async fn fetch(api: &str, browser: &str) -> Result<reqwest::Response> {
    let c = client()?;
    let req = match token() {
        Some(t) if !api.is_empty() => c.get(api).bearer_auth(t).header("Accept", "application/octet-stream"),
        _ => c.get(browser),
    };
    let resp = req.send().await.context("downloading the update")?;
    if !resp.status().is_success() {
        bail!("the download answered {}", resp.status());
    }
    Ok(resp)
}

/// Download a release's program, report progress (bytes, total), and check
/// it against its published SHA-256. Returns the downloaded program.
pub async fn download(rel: &Release, mut progress: impl FnMut(u64, u64)) -> Result<PathBuf> {
    let dir = cache_dir().join(&rel.version);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(install::EXE_NAME);
    let mut resp = fetch(&rel.asset_api, &rel.asset_url).await?;
    let total = resp.content_length().unwrap_or(rel.size);
    let mut hasher = Sha256::new();
    let mut bytes: Vec<u8> = Vec::with_capacity(total as usize);
    while let Some(chunk) = resp.chunk().await.context("downloading the update")? {
        hasher.update(&chunk);
        bytes.extend_from_slice(&chunk);
        progress(bytes.len() as u64, total);
    }
    let got = hex(&hasher.finalize());
    match (&rel.sha_api, &rel.sha_url) {
        (api, Some(url)) => {
            let want = fetch(api.as_deref().unwrap_or(""), url).await?.text().await?;
            let want = want.split_whitespace().next().unwrap_or("").to_lowercase();
            if want != got {
                bail!("The download doesn't match its published checksum");
            }
        }
        _ => bail!("The release has no checksum for its program"),
    }
    let tmp = path.with_extension("exe.part");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Hand over to a downloaded program: it waits for this Larrier to close,
/// takes its place and starts. The caller then exits.
pub fn restart_into(downloaded: &Path) -> Result<()> {
    let me = std::env::current_exe()?;
    let pid = std::process::id().to_string();
    install::launch(downloaded, &["--apply-update", &me.display().to_string(), "--pid", &pid])
}

/// Run by the downloaded program (`--apply-update <target> --pid <n>`): wait
/// for the old Larrier, replace it (asking for administrator rights when it
/// is installed for everyone), record the version and start it.
pub fn apply(target: &Path, pid: Option<u32>) -> Result<()> {
    if let Some(pid) = pid {
        wait_for_exit(pid);
    }
    let me = std::env::current_exe()?;
    let kind = install::kind_of(target);
    let writable = target.parent().map(|d| {
        let probe = d.join(".larrier-write-test");
        let ok = std::fs::write(&probe, b"").is_ok();
        let _ = std::fs::remove_file(&probe);
        ok
    });
    if writable == Some(true) {
        install::replace_file(&me, target)?;
        if let Kind::Installed(scope) = kind {
            let _ = install::set_version(scope, larrier_core::APP_VERSION);
        }
    } else {
        let code = install::run_elevated(&me, &["--replace".into(), target.display().to_string()])?;
        if code != 0 {
            bail!("the update could not be put in place (code {code})");
        }
    }
    install::launch(target, &[])
}

/// Run elevated (`--replace <target>`): put this program in `target`'s place.
pub fn replace(target: &Path) -> Result<()> {
    let me = std::env::current_exe()?;
    install::replace_file(&me, target)?;
    if let Kind::Installed(scope) = install::kind_of(target) {
        install::set_version(scope, larrier_core::APP_VERSION)?;
    }
    Ok(())
}

fn wait_for_exit(pid: u32) {
    for _ in 0..240 {
        let mut cmd = std::process::Command::new("tasklist");
        cmd.args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let running = cmd.output().map(|o| String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\""))).unwrap_or(false);
        if !running {
            return;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(newer("0.2.0", "0.1.0"));
        assert!(newer("v1.0.0", "0.9.9"));
        assert!(newer("0.10.0", "0.9.0"));
        assert!(!newer("0.1.0", "0.1.0"));
        assert!(!newer("0.1.0-beta", "0.1.0"));
        assert!(!newer("0.0.9", "0.1.0"));
    }

    #[test]
    fn finds_this_platforms_program() {
        let v = serde_json::json!({
            "tag_name": "v0.2.0", "html_url": "https://github.com/Dev4YM/Larrier/releases/tag/v0.2.0", "body": "Notes",
            "assets": [
                { "name": "Larrier-0.2.0-windows-x64.exe", "url": "api/1", "browser_download_url": "dl/1", "size": 10 },
                { "name": "Larrier-0.2.0-windows-x64.exe.sha256", "url": "api/2", "browser_download_url": "dl/2", "size": 1 },
            ]
        });
        if asset_suffix() == Some("-windows-x64.exe") {
            let r = parse_release(&v).unwrap();
            assert_eq!((r.version.as_str(), r.asset_name.as_str(), r.sha_url.as_deref()), ("0.2.0", "Larrier-0.2.0-windows-x64.exe", Some("dl/2")));
        }
    }
}
