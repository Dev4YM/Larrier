//! Agents' versions: the latest each one has published, read from where
//! its makers publish it (its npm package, its install script, its GitHub
//! releases, PyPI), and the commands that install or update it as its
//! makers give them. Nothing else is downloaded or run.

use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, Context};
use larrier_core::AgentKind;

/// Where an agent's releases are published.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Source {
    /// Its npm package (the `latest` tag).
    Npm(&'static str),
    /// Its official install script names the version it installs:
    /// (script, the text before the version, the text after it) for
    /// Windows, then for macOS / Linux.
    Script { windows: (&'static str, &'static str, &'static str), unix: (&'static str, &'static str, &'static str) },
    /// Its GitHub repository's latest release.
    GitHub(&'static str),
    /// Its PyPI package.
    PyPi(&'static str),
}

/// How an installed agent is updated.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Update {
    /// Its own command (`claude update`).
    Itself(&'static [&'static str]),
    /// The tool it was installed with (`uv tool upgrade kimi-cli`).
    Shell(&'static str),
    /// Its npm package at its latest when npm installed it, else its
    /// install script again.
    Package,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Versions {
    pub source: Source,
    pub update: Option<Update>,
}

/// What Larrier knows of an agent's versions (`None`: it can't tell).
pub fn of(kind: &AgentKind) -> Option<Versions> {
    use AgentKind as A;
    let (source, update) = match kind {
        A::ClaudeCode => (Source::Npm("@anthropic-ai/claude-code"), Some(Update::Itself(&["update"]))),
        A::Codex => (Source::Npm("@openai/codex"), Some(Update::Itself(&["update"]))),
        A::Cursor => (
            Source::Script {
                windows: ("https://cursor.com/install?win32=true", "downloads.cursor.com/lab/", "/"),
                unix: ("https://cursor.com/install", "downloads.cursor.com/lab/", "/"),
            },
            Some(Update::Itself(&["update"])),
        ),
        A::OpenCode => (Source::Npm("opencode-ai"), Some(Update::Itself(&["upgrade"]))),
        A::Gemini => (Source::Npm("@google/gemini-cli"), Some(Update::Package)),
        A::Pi => (Source::Npm("@earendil-works/pi-coding-agent"), Some(Update::Package)),
        A::Hermes => (Source::GitHub("NousResearch/hermes-agent"), Some(Update::Itself(&["update"]))),
        A::Goose => (Source::GitHub("aaif-goose/goose"), Some(Update::Package)),
        A::Amp => (Source::Npm("@sourcegraph/amp"), Some(Update::Package)),
        A::Copilot => (Source::Npm("@github/copilot"), Some(Update::Package)),
        A::Qwen => (Source::Npm("@qwen-code/qwen-code"), Some(Update::Package)),
        A::Kimi => (Source::PyPi("kimi-cli"), Some(Update::Shell("uv tool upgrade kimi-cli"))),
        A::Droid => (
            Source::Script {
                windows: ("https://app.factory.ai/cli/windows", "$version = \"", "\""),
                unix: ("https://app.factory.ai/cli", "VER=\"", "\""),
            },
            Some(Update::Itself(&["update"])),
        ),
        // Its installer sets up its own environment; update it from its site.
        A::Aider => (Source::PyPi("aider-chat"), None),
        _ => return None,
    };
    Some(Versions { source, update })
}

/// Where its latest version is read from, to show.
pub fn source_label(s: &Source) -> String {
    match s {
        Source::Npm(p) => format!("npm {p}"),
        Source::Script { windows, unix } => {
            let url = if cfg!(windows) { windows.0 } else { unix.0 };
            format!("its install script ({})", url.split('?').next().unwrap_or(url))
        }
        Source::GitHub(r) => format!("GitHub {r}"),
        Source::PyPi(p) => format!("PyPI {p}"),
    }
}

/// The latest version `kind` has published.
pub async fn latest(kind: &AgentKind) -> anyhow::Result<String> {
    let v = of(kind).ok_or_else(|| anyhow!("no known source"))?;
    let client = reqwest::Client::builder()
        .user_agent(concat!("Larrier/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(20))
        .build()?;
    let json = |url: String| {
        let client = client.clone();
        async move { client.get(&url).send().await?.error_for_status()?.json::<serde_json::Value>().await.context(url) }
    };
    let found = match v.source {
        Source::Npm(pkg) => json(format!("https://registry.npmjs.org/-/package/{pkg}/dist-tags")).await?["latest"].as_str().map(str::to_string),
        Source::GitHub(repo) => json(format!("https://api.github.com/repos/{repo}/releases/latest")).await?["tag_name"].as_str().map(str::to_string),
        Source::PyPi(pkg) => json(format!("https://pypi.org/pypi/{pkg}/json")).await?["info"]["version"].as_str().map(str::to_string),
        Source::Script { windows, unix } => {
            let (url, before, after) = if cfg!(windows) { windows } else { unix };
            let text = client.get(url).send().await?.error_for_status()?.text().await?;
            between(&text, before, after)
        }
    };
    found.map(|s| s.trim().trim_start_matches(['v', 'V']).to_string()).filter(|s| !s.is_empty()).ok_or_else(|| anyhow!("no version found"))
}

fn between(text: &str, before: &str, after: &str) -> Option<String> {
    let from = text.find(before)? + before.len();
    let len = text[from..].find(after)?;
    Some(text[from..from + len].to_string()).filter(|v| v.len() < 64 && v.chars().next().is_some_and(|c| c.is_ascii_digit()))
}

/// Whether `latest` is a newer release than `installed`: their numbers
/// compared part by part (what follows `-` or `+`, a build or a commit,
/// doesn't count).
pub fn is_newer(latest: &str, installed: &str) -> bool {
    fn parts(v: &str) -> Vec<u64> {
        let v = v.trim().trim_start_matches(['v', 'V']);
        v.split(['-', '+']).next().unwrap_or("").split('.').map(|p| p.parse().unwrap_or(0)).collect()
    }
    let (a, b) = (parts(latest), parts(installed));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

/// A command to run, and how it reads.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub program: String,
    pub args: Vec<String>,
    pub shown: String,
}

impl Step {
    /// A command line for the system's shell (PowerShell on Windows).
    fn shell(line: &str) -> Step {
        if cfg!(windows) {
            Step {
                program: "powershell".into(),
                args: vec!["-NoLogo".into(), "-NoProfile".into(), "-ExecutionPolicy".into(), "Bypass".into(), "-Command".into(), line.into()],
                shown: line.into(),
            }
        } else {
            Step { program: "sh".into(), args: vec!["-c".into(), line.into()], shown: line.into() }
        }
    }
}

/// Its official install command for this system (`None`: there isn't one).
pub fn install_step(kind: &AgentKind) -> Option<Step> {
    let install = kind.spec().install;
    let pick = |labels: &[&str]| labels.iter().find_map(|l| install.iter().find(|(k, _)| k == l)).map(|(_, c)| *c);
    let line = if cfg!(windows) {
        // A bash script doesn't run on Windows.
        pick(&["Windows", "npm", "Scoop", "uv", "pip"])
    } else {
        pick(&["macOS / Linux", "Script", "npm", "Homebrew", "uv", "pip"])
    }?;
    Some(Step::shell(line))
}

/// How to update `kind`, installed at `path`.
pub fn update_step(kind: &AgentKind, path: &Path) -> Option<Step> {
    let v = of(kind)?;
    match v.update? {
        Update::Itself(args) => {
            let (program, prefix) = crate::request::unwrap_shim(path);
            let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            Some(Step {
                program: program.to_string_lossy().to_string(),
                args: prefix.into_iter().chain(args.iter().map(|a| a.to_string())).collect(),
                shown: format!("{name} {}", args.join(" ")),
            })
        }
        Update::Shell(line) => Some(Step::shell(line)),
        Update::Package => match v.source {
            Source::Npm(pkg) if from_npm(path, pkg) => Some(Step::shell(&format!("npm install -g {pkg}@latest"))),
            _ => install_step(kind),
        },
    }
}

/// Whether npm installed the program at `path` (package `pkg`).
fn from_npm(path: &Path, pkg: &str) -> bool {
    let dir = path.parent();
    let rel: std::path::PathBuf = pkg.split('/').collect();
    dir.is_some_and(|d| d.join("node_modules").join(&rel).is_dir())
        || dir.and_then(Path::parent).is_some_and(|p| p.join("lib").join("node_modules").join(&rel).is_dir())
        || std::fs::canonicalize(path).is_ok_and(|c| c.to_string_lossy().replace('\\', "/").contains(&format!("node_modules/{pkg}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_versions() {
        assert!(is_newer("2.1.316", "2.1.309"));
        assert!(!is_newer("2.1.309", "2.1.309"));
        assert!(!is_newer("2.1.300", "2.1.309"));
        assert!(is_newer("0.22.5", "0.21.5"));
        assert!(is_newer("v1.10.0", "1.9.9"));
        // Cursor's are dated, with a commit after them.
        assert!(is_newer("2026.10.29-aa11bb2", "2026.10.22-27dcf5f"));
        assert!(!is_newer("2026.10.22-27dcf5f", "2026.10.22-27dcf5f"));
        assert!(!is_newer("0.166.0", "0.166.0-alpha.6"));
        assert!(is_newer("1.1", "1.0.9"));
    }

    #[test]
    fn a_script_names_its_version() {
        let cursor = "url=\"https://downloads.cursor.com/lab/2026.10.22-27dcf5f/${OS}/${ARCH}/agent-cli-package.tar.gz\"";
        assert_eq!(between(cursor, "downloads.cursor.com/lab/", "/").as_deref(), Some("2026.10.22-27dcf5f"));
        let droid = "$version = \"1.1.0\"\n$baseUrl = \"https://downloads.factory.ai\"";
        assert_eq!(between(droid, "$version = \"", "\"").as_deref(), Some("1.1.0"));
        assert_eq!(between("VER=\"${1}\"", "VER=\"", "\""), None, "not a version");
    }

    #[test]
    fn installed_agents_update_as_their_makers_say() {
        let claude = update_step(&AgentKind::ClaudeCode, Path::new("C:/Users/u/.local/bin/claude.exe")).unwrap();
        assert_eq!(claude.shown, "claude update");
        assert!(claude.args.ends_with(&["update".to_string()]));
        // Not installed with npm: its install script again.
        let gemini = update_step(&AgentKind::Gemini, Path::new("/nowhere/gemini")).unwrap();
        assert!(gemini.shown.contains("@google/gemini-cli"), "{}", gemini.shown);
        assert_eq!(update_step(&AgentKind::Kimi, Path::new("/x/kimi")).unwrap().shown, "uv tool upgrade kimi-cli");
        assert!(update_step(&AgentKind::Aider, Path::new("/x/aider")).is_none());
        assert!(of(&AgentKind::Chat).is_none());
    }

    /// Reads every agent's latest version from where it's published.
    #[tokio::test]
    #[ignore = "network"]
    async fn latest_versions_from_their_sources() {
        for k in AgentKind::BUILTIN.iter().filter(|k| of(k).is_some()) {
            let v = latest(k).await;
            println!("{:?}: {v:?}", k);
            assert!(v.is_ok_and(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit())), "{k:?}");
        }
    }

    #[test]
    fn npm_installs_are_known_by_their_package() {
        let dir = std::env::temp_dir().join(format!("larrier-npm-{}", std::process::id()));
        let pkg = dir.join("node_modules").join("@openai").join("codex");
        std::fs::create_dir_all(&pkg).unwrap();
        assert!(from_npm(&dir.join("codex.ps1"), "@openai/codex"));
        assert!(!from_npm(&dir.join("codex.ps1"), "opencode-ai"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
