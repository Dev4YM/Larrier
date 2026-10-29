//! Finding installed agents and their versions.

use std::path::{Path, PathBuf};
use std::time::Duration;

use larrier_core::{AgentKind, Config};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Detected {
    pub kind: AgentKind,
    pub path: Option<PathBuf>,
    pub version: Option<String>,
    /// Why a found binary was rejected (e.g. a stub that prints "not found").
    #[serde(default)]
    pub note: Option<String>,
}

impl Detected {
    pub fn installed(&self) -> bool {
        self.path.is_some()
    }
}

/// Extra places CLIs land that are often missing from a GUI app's PATH.
fn extra_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(h) = dirs::home_dir() {
        v.push(h.join(".local").join("bin"));
        v.push(h.join(".cargo").join("bin"));
        v.push(h.join(".bun").join("bin"));
        v.push(h.join(".opencode").join("bin"));
        v.push(h.join(".claude").join("local"));
        v.push(h.join(".npm-global").join("bin"));
        v.push(h.join(".volta").join("bin"));
        v.push(h.join("go").join("bin"));
        v.push(h.join("bin"));
        if cfg!(windows) {
            v.push(h.join("AppData").join("Roaming").join("npm"));
            v.push(h.join("AppData").join("Local").join("Programs").join("cursor").join("resources").join("app").join("bin"));
            v.push(h.join("scoop").join("shims"));
        } else {
            v.push(PathBuf::from("/usr/local/bin"));
            v.push(PathBuf::from("/opt/homebrew/bin"));
        }
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        v.push(Path::new(&appdata).join("npm"));
    }
    if let Ok(la) = std::env::var("LOCALAPPDATA") {
        v.push(Path::new(&la).join("cursor-agent"));
        v.push(Path::new(&la).join("Programs").join("cursor-agent"));
        v.push(Path::new(&la).join("hermes").join("bin"));
        v.push(Path::new(&la).join("Programs").join("goose"));
    }
    v
}

pub fn which_program(name: &str) -> Option<PathBuf> {
    let p = Path::new(name);
    if p.is_absolute() && p.exists() {
        return Some(p.to_path_buf());
    }
    if let Ok(found) = which::which(name) {
        return Some(found);
    }
    let exts: &[&str] = if cfg!(windows) { &[".exe", ".cmd", ".bat", ".ps1", ""] } else { &[""] };
    for dir in extra_dirs() {
        for ext in exts {
            let cand = dir.join(format!("{name}{ext}"));
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

pub fn find_binary(kind: &AgentKind, override_path: Option<&str>) -> Option<PathBuf> {
    if let Some(p) = override_path.filter(|p| !p.trim().is_empty()) {
        return which_program(p);
    }
    kind.spec().binaries.iter().find_map(|b| which_program(b))
}

/// Outcome of running `<bin> --version`.
#[derive(Clone, Debug, PartialEq)]
pub enum Probe {
    Version(String),
    /// Ran fine but printed nothing version-like (or timed out).
    Unknown,
    /// The binary is a stub / broken install (e.g. "… was not found").
    Broken(String),
}

/// Run `<bin> --version` with a timeout.
pub async fn probe(path: &Path) -> Probe {
    let (program, prefix) = crate::request::unwrap_shim(path);
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(prefix).arg("--version").stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    cmd.kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let out = match tokio::time::timeout(Duration::from_secs(12), cmd.output()).await {
        Err(_) => return Probe::Unknown,
        Ok(Err(e)) => return Probe::Broken(e.to_string()),
        Ok(Ok(o)) => o,
    };
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    text.push('\n');
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    classify_version_output(out.status.success(), &text)
}

fn classify_version_output(success: bool, text: &str) -> Probe {
    let lower = text.to_ascii_lowercase();
    let missing = ["not found", "not installed", "is not recognized", "no such file", "cannot find"].iter().any(|m| lower.contains(m));
    let version = text.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).find_map(extract_version);
    let first_line = text.lines().map(|l| l.trim()).find(|l| !l.is_empty()).unwrap_or("").chars().take(120).collect::<String>();
    match version {
        Some(v) if success || !missing => Probe::Version(v),
        _ if missing || !success => Probe::Broken(if first_line.is_empty() { "exited with an error".into() } else { first_line }),
        _ => Probe::Unknown,
    }
}

/// Run `<bin> --version`; `None` unless it printed a real version.
pub async fn version_of(path: &Path) -> Option<String> {
    match probe(path).await {
        Probe::Version(v) => Some(v),
        _ => None,
    }
}

/// First version-looking token (`1.2.3`, `v0.21.5+build` → `0.21.5`).
fn extract_version(line: &str) -> Option<String> {
    for tok in line.split(|c: char| c.is_whitespace() || c == ',' || c == '(' || c == ')' || c == '/') {
        let t = tok.trim_start_matches(['v', 'V']).trim_end_matches(['.', ',', ';', ':']);
        let t = t.split('+').next().unwrap_or(t);
        if t.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false)
            && t.contains('.')
            && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        {
            return Some(t.to_string());
        }
    }
    None
}

/// Detect every runnable built-in agent concurrently.
pub async fn detect_all(cfg: &Config) -> Vec<Detected> {
    let kinds: Vec<AgentKind> = larrier_core::agent::runnable().filter(|s| !s.binaries.is_empty()).map(|s| s.kind.clone()).collect();
    let futs = kinds.into_iter().map(|k| {
        let ov = cfg.agent(&k).binary.clone();
        async move {
            let found = find_binary(&k, ov.as_deref());
            let (path, version, note) = match found {
                Some(p) => match probe(&p).await {
                    Probe::Version(v) => (Some(p), Some(v), None),
                    Probe::Unknown => (Some(p), None, None),
                    // A stub that only prints "not found" is not an install.
                    Probe::Broken(why) => (None, None, Some(format!("{}: {why}", p.display()))),
                },
                None => (None, None, None),
            };
            Detected { kind: k, path, version, note }
        }
    });
    futures::future::join_all(futs).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(extract_version("2.0.31 (Claude Code)").as_deref(), Some("2.0.31"));
        assert_eq!(extract_version("codex-cli 0.58.0").as_deref(), Some("0.58.0"));
        assert_eq!(extract_version("v1.2.3").as_deref(), Some("1.2.3"));
        assert_eq!(extract_version("Hermes Agent v0.21.5+9141.ga28a5d0").as_deref(), Some("0.21.5"));
        assert_eq!(extract_version("2026.09.10-fd3934a").as_deref(), Some("2026.09.10-fd3934a"));
        assert_eq!(extract_version("GitHub Copilot CLI was not found."), None);
    }

    #[test]
    fn broken_stubs_are_not_installs() {
        assert!(matches!(
            classify_version_output(true, "GitHub Copilot CLI was not found. Run copilot install"),
            Probe::Broken(_)
        ));
        assert!(matches!(classify_version_output(false, "boom"), Probe::Broken(_)));
        assert_eq!(classify_version_output(true, "codex-cli 0.154.0\n"), Probe::Version("0.154.0".into()));
        assert_eq!(classify_version_output(true, ""), Probe::Unknown);
    }
}

#[cfg(test)]
mod machine {
    /// `cargo test -p larrier-agents detect_this_machine -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn detect_this_machine() {
        let cfg = larrier_core::Config::default();
        for d in super::detect_all(&cfg).await {
            let launch = d.path.as_ref().map(|p| crate::request::unwrap_shim(p));
            println!("{:<14} {:<22} {:?} {:?} {:?}", d.kind.id(), d.version.clone().unwrap_or_default(), d.path, launch, d.note);
        }
    }
}
