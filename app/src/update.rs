//! Updates from GitHub releases. A release carries each platform's program
//! and its SHA-256 (`<asset>.sha256`): `Larrier-<version>-windows-x64.exe`,
//! `…-macos-arm64.zip` / `…-macos-x64.zip` (Larrier.app) and
//! `…-linux-x64.tar.gz`. Larrier downloads the newest one's, checks it, and
//! on restart the new program takes the old one's place (on macOS the whole
//! Larrier.app). No source code is involved.
//!
//! The Stable channel follows the latest release; Preview also takes
//! pre-releases (`0.3.0-beta.2`). Versions compare as Semantic Versions
//! (see `crate::version`).
//!
//! The repository is `Dev4YM/Larrier` (`LARRIER_UPDATE_REPO` overrides it,
//! `LARRIER_UPDATE_API` points at another API root, e.g. for tests). Its
//! releases are public: no account or token is used.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};

use larrier_core::UpdateChannel;

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
    sha_url: Option<String>,
    pub size: u64,
}

/// The asset name a release's program has on this platform.
pub fn asset_suffix() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("-windows-x64.exe"),
        ("windows", "aarch64") => Some("-windows-arm64.exe"),
        ("macos", "aarch64") => Some("-macos-arm64.zip"),
        ("macos", "x86_64") => Some("-macos-x64.zip"),
        ("linux", "x86_64") => Some("-linux-x64.tar.gz"),
        ("linux", "aarch64") => Some("-linux-arm64.tar.gz"),
        _ => None,
    }
}

fn api_root() -> String {
    std::env::var("LARRIER_UPDATE_API").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "https://api.github.com".into())
}

pub fn repo() -> String {
    std::env::var("LARRIER_UPDATE_REPO").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| REPO.into())
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder().user_agent(concat!("Larrier/", env!("CARGO_PKG_VERSION"))).connect_timeout(Duration::from_secs(15)).build()?)
}

pub use crate::version::newer;

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
        sha_url: sha.and_then(|s| s["browser_download_url"].as_str()).map(String::from),
        size: asset["size"].as_u64().unwrap_or(0),
    })
}

async fn get_json(path: &str) -> Result<Value> {
    let url = format!("{}/repos/{}/{path}", api_root().trim_end_matches('/'), repo());
    let resp = client()?.get(&url).header("Accept", "application/vnd.github+json").send().await.context("reaching GitHub")?;
    match resp.status().as_u16() {
        200 => {}
        404 => bail!("No releases found for {}", repo()),
        s => bail!("GitHub answered {s}"),
    }
    resp.json().await.context("reading the releases")
}

/// The newest release on `channel`, when it is newer than this Larrier and
/// has a program for this platform.
pub async fn check(channel: UpdateChannel) -> Result<Option<Release>> {
    let rel = match channel {
        UpdateChannel::Stable => {
            let v = get_json("releases/latest").await?;
            parse_release(&v).ok_or_else(|| anyhow!("The latest release has no program for this computer"))?
        }
        UpdateChannel::Preview => {
            let list = get_json("releases?per_page=30").await?;
            newest(list.as_array().map(|a| a.as_slice()).unwrap_or(&[])).ok_or_else(|| anyhow!("No release has a program for this computer"))?
        }
    };
    Ok(newer(&rel.version, crate::version::VERSION).then_some(rel))
}

/// The newest published release (previews included) with a program for
/// this platform.
pub fn newest(list: &[Value]) -> Option<Release> {
    list.iter()
        .filter(|r| !r["draft"].as_bool().unwrap_or(false))
        .filter_map(parse_release)
        .filter_map(|r| crate::version::Version::parse(&r.version).map(|v| (v, r)))
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, r)| r)
}

/// Where downloaded updates wait.
pub fn cache_dir() -> PathBuf {
    match install::current() {
        Kind::Installed(scope) => install::layout(scope).shared_dir.join("updates"),
        Kind::App => dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("larrier").join("updates"),
        _ => install::layout(Scope::User).shared_dir.join("updates"),
    }
}

/// Fetch a release asset from its public download link.
async fn fetch(url: &str) -> Result<reqwest::Response> {
    let resp = client()?.get(url).send().await.context("downloading the update")?;
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
    let mut resp = fetch(&rel.asset_url).await?;
    let total = resp.content_length().unwrap_or(rel.size);
    let mut hasher = Sha256::new();
    let mut bytes: Vec<u8> = Vec::with_capacity(total as usize);
    while let Some(chunk) = resp.chunk().await.context("downloading the update")? {
        hasher.update(&chunk);
        bytes.extend_from_slice(&chunk);
        progress(bytes.len() as u64, total);
    }
    let got = hex(&hasher.finalize());
    let Some(url) = &rel.sha_url else { bail!("The release has no checksum for its program") };
    let want = fetch(url).await?.text().await?;
    let want = want.split_whitespace().next().unwrap_or("").to_lowercase();
    if want != got {
        bail!("The download doesn't match its published checksum");
    }
    if cfg!(windows) {
        let tmp = path.with_extension("exe.part");
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &path)?;
        return Ok(path);
    }
    unpack(&dir, &rel.asset_name, &bytes)
}

/// macOS and Linux: unpack a downloaded archive with the system's own tools
/// and return the program in it (inside Larrier.app on macOS).
fn unpack(dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf> {
    let archive = dir.join(name);
    std::fs::write(&archive, bytes)?;
    let out = dir.join("unpacked");
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out)?;
    let (tool, args, program): (&str, Vec<&std::ffi::OsStr>, PathBuf) = if name.ends_with(".zip") {
        ("ditto", vec!["-x".as_ref(), "-k".as_ref(), archive.as_os_str(), out.as_os_str()], out.join("Larrier.app/Contents/MacOS/Larrier"))
    } else {
        ("tar", vec!["-xzf".as_ref(), archive.as_os_str(), "-C".as_ref(), out.as_os_str()], out.join("larrier/larrier"))
    };
    let ok = std::process::Command::new(tool).args(args).status().map(|s| s.success()).unwrap_or(false);
    if !ok || !program.exists() {
        bail!("The download couldn't be unpacked");
    }
    Ok(program)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Hand over to a downloaded program: it waits for this Larrier to close,
/// takes its place and starts. The caller then exits.
pub fn restart_into(downloaded: &Path) -> Result<()> {
    let me = std::env::current_exe()?;
    let pid = std::process::id().to_string();
    install::spawn(downloaded, &["--apply-update", &me.display().to_string(), "--pid", &pid])
}

/// Like [`restart_into`] for a Larrier that is closing: the new program
/// takes its place once it has exited, without starting it again.
pub fn install_after_exit(downloaded: &Path) -> Result<()> {
    let me = std::env::current_exe()?;
    let pid = std::process::id().to_string();
    install::spawn(downloaded, &["--apply-update", &me.display().to_string(), "--pid", &pid, "--no-launch"])
}

/// Run by the downloaded program (`--apply-update <target> --pid <n>`): wait
/// for the old Larrier, replace it (asking for administrator rights when it
/// is installed for everyone), record the version and start it.
pub fn apply(target: &Path, pid: Option<u32>, relaunch: bool) -> Result<()> {
    if let Some(pid) = pid {
        wait_for_exit(pid);
    }
    let me = std::env::current_exe()?;
    let kind = install::kind_of(target);
    // macOS: the whole Larrier.app is replaced, not just its program.
    #[cfg(target_os = "macos")]
    if let (Some(old), Some(new)) = (install::bundle_of(target), install::bundle_of(&me)) {
        install::replace_bundle(&new, &old)?;
        if relaunch {
            install::launch(target, &[])?;
        }
        return Ok(());
    }
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
    } else if !cfg!(windows) {
        bail!("Larrier can't write to {}: update it the way you installed it", target.parent().unwrap_or(target).display());
    } else {
        let code = install::run_elevated(&me, &["--replace".into(), target.display().to_string()])?;
        if code != 0 {
            bail!("the update could not be put in place (code {code})");
        }
    }
    if relaunch {
        install::launch(target, &[])?;
    }
    Ok(())
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
    #[cfg(not(windows))]
    {
        for _ in 0..240 {
            let alive = std::process::Command::new("kill").args(["-0", &pid.to_string()]).stderr(std::process::Stdio::null()).status().map(|s| s.success()).unwrap_or(false);
            if !alive {
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        return;
    }
    #[allow(unreachable_code)]
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
    fn preview_channel_takes_the_newest_release() {
        let rel = |tag: &str, draft: bool| {
            let v = tag.trim_start_matches('v');
            serde_json::json!({ "tag_name": tag, "draft": draft, "html_url": "", "body": "",
                "assets": [{ "name": format!("Larrier-{v}-windows-x64.exe"), "url": "a", "browser_download_url": "b", "size": 1 }] })
        };
        let list = vec![rel("v0.2.0", false), rel("v0.3.0-beta.2", false), rel("v0.3.0-beta.10", false), rel("v0.4.0", true), rel("v0.2.1", false)];
        if asset_suffix() == Some("-windows-x64.exe") {
            assert_eq!(newest(&list).unwrap().version, "0.3.0-beta.10");
        }
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

    /// The published releases answer without a GitHub account or token
    /// (`cargo test -p larrier-app public_releases_need_no_token -- --ignored`).
    #[test]
    #[ignore]
    fn public_releases_need_no_token() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let rel = parse_release(&get_json("releases/latest").await.unwrap()).expect("a program for this computer");
            let sha = fetch(rel.sha_url.as_deref().expect("its checksum")).await.unwrap().text().await.unwrap();
            assert!(sha.trim().ends_with(&rel.asset_name), "{sha}");
            let program = fetch(&rel.asset_url).await.unwrap();
            assert_eq!(program.content_length(), Some(rel.size));
        });
    }
}
