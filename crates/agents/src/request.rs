//! Turning a session + configuration into a concrete launch request.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use larrier_core::{
    config::CustomAgent, secrets, Account, AccountAuth, AgentKind, Config, Message, Part, Protocol, ProviderKind,
    ModeSelection, RunMode, Session,
};

use crate::detect;
use crate::engine;

/// Everything a driver needs to run one turn.
#[derive(Clone, Debug)]
pub struct TurnRequest {
    pub agent: AgentKind,
    pub protocol: Protocol,
    pub session_id: String,
    /// The user's prompt for this turn (without handoff context).
    pub prompt: String,
    /// Images / files attached to this turn.
    pub attachments: Vec<Part>,
    /// Full conversation so far, including the new user message (HTTP chat
    /// replays it; CLI agents use `resume_id` instead).
    pub history: Vec<Message>,
    pub model: Option<String>,
    /// Chosen option of the agent's first mode group (also the ACP mode id).
    pub mode: Option<String>,
    /// Every mode group resolved: the CLI arguments to add and, for modes
    /// without a CLI flag, an instruction to put before the prompt.
    pub modes: ModeSelection,
    pub cwd: PathBuf,
    pub run_mode: RunMode,
    /// Agent-native session to resume.
    pub resume_id: Option<String>,
    /// Path of the native session file, when known (Claude Code and Pi can
    /// resume directly from a file path, which is the most robust form).
    pub resume_path: Option<String>,
    /// Resume the native session only up to this entry, dropping what came
    /// after it (an earlier prompt was edited). Claude Code only.
    pub resume_at: Option<String>,
    /// Transcript of earlier turns the agent has not seen (ported sessions).
    pub handoff: Option<String>,
    pub launch: Launch,
    pub chat: Option<ChatTarget>,
}

impl TurnRequest {
    /// Prompt including the handoff preamble, for agents that only accept text.
    pub fn full_prompt(&self) -> String {
        let prompt = match &self.modes.prompt_prefix {
            Some(p) => format!("{p}\n\n{}", self.prompt),
            None => self.prompt.clone(),
        };
        match &self.handoff {
            Some(h) if !h.trim().is_empty() => format!(
                "<conversation_history>\nThis conversation was started with another agent and has been handed to you. \
                 The transcript so far:\n\n{h}\n</conversation_history>\n\nContinue from here. The user's new message:\n\n{prompt}"
            ),
            _ => prompt,
        }
    }

    /// The prompt to pass as a command-line argument. Windows limits command
    /// lines to ~32K characters and batch-file shims cannot carry newlines,
    /// so long (or, for batch shims, multi-line) prompts are written to a
    /// temporary file the agent is asked to read.
    pub fn prompt_for_arg(&self, max_chars: usize) -> String {
        let full = self.full_prompt();
        let batch = is_batch(&self.launch.program);
        if full.chars().count() <= max_chars && !(batch && full.contains(['\n', '\r'])) {
            return full;
        }
        match self.write_prompt_file(&full) {
            Some(path) => format!(
                "Your full task is in the file {} — read that whole file first and follow it exactly.",
                path.display()
            ),
            None => full.chars().take(max_chars).collect(),
        }
    }

    /// Write `text` to a per-turn temporary prompt file.
    pub fn write_prompt_file(&self, text: &str) -> Option<PathBuf> {
        let dir = std::env::temp_dir().join("larrier").join("prompts");
        std::fs::create_dir_all(&dir).ok()?;
        let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S%3f");
        let short: String = self.session_id.chars().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect();
        let path = dir.join(format!("prompt-{short}-{stamp}.md"));
        std::fs::write(&path, text).ok()?;
        Some(path)
    }

    /// Local image paths attached to this turn.
    pub fn image_paths(&self) -> Vec<String> {
        self.attachments
            .iter()
            .filter_map(|a| match a {
                Part::Image { source: larrier_core::lsf::MediaSource::Path { path }, .. } => Some(path.clone()),
                _ => None,
            })
            .collect()
    }

    /// Attached (non-image) files, mentioned to agents as `@path` references.
    pub fn file_paths(&self) -> Vec<String> {
        self.attachments
            .iter()
            .filter_map(|a| match a {
                Part::File { path, .. } => Some(path.clone()),
                _ => None,
            })
            .collect()
    }
}

fn is_batch(p: &Path) -> bool {
    matches!(
        p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).as_deref(),
        Some("cmd") | Some("bat")
    )
}

/// A resolved process to spawn.
#[derive(Clone, Debug, Default)]
pub struct Launch {
    pub program: PathBuf,
    /// Arguments placed before the driver's own arguments (e.g. the JS entry
    /// point when an npm `.cmd` shim is unwrapped to `node script.js`, or the
    /// ACP sub-command).
    pub prefix_args: Vec<String>,
    /// User-configured extra arguments appended by the driver.
    pub extra_args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl Launch {
    pub fn command(&self) -> tokio::process::Command {
        let mut c = tokio::process::Command::new(&self.program);
        c.args(&self.prefix_args);
        for (k, v) in &self.env {
            c.env(k, v);
        }
        #[cfg(windows)]
        {
            // CREATE_NO_WINDOW: don't flash a console for every agent run.
            c.creation_flags(0x0800_0000);
        }
        c.kill_on_drop(true);
        c
    }

    pub fn is_empty(&self) -> bool {
        self.program.as_os_str().is_empty()
    }
}

#[derive(Clone, Debug)]
pub struct ChatTarget {
    pub kind: ProviderKind,
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
    pub provider_name: String,
}

/// Build the env for an account.
pub fn account_env(agent: &AgentKind, account: &Account) -> Vec<(String, String)> {
    let spec = agent.spec();
    match &account.auth {
        AccountAuth::SystemLogin => Vec::new(),
        AccountAuth::ConfigDir { path } => match spec.home_env {
            Some(var) => vec![(var.to_string(), path.clone())],
            None => Vec::new(),
        },
        AccountAuth::ApiKey { env_var, secret_ref } => {
            secrets::get(secret_ref).map(|v| vec![(env_var.clone(), v)]).unwrap_or_default()
        }
        AccountAuth::Env { vars } => vars.iter().map(|(k, v)| (k.clone(), secrets::resolve(v))).collect(),
    }
}

/// Environment for an agent: per-agent config env plus the account's env.
fn agent_env(cfg: &Config, agent: &AgentKind, account_id: Option<&str>) -> Vec<(String, String)> {
    let ac = cfg.agent(agent);
    let mut env: Vec<(String, String)> = ac.env.iter().map(|(k, v)| (k.clone(), secrets::resolve(v))).collect();
    let account = account_id.and_then(|id| cfg.account(id)).or_else(|| cfg.default_account(agent));
    if let Some(acc) = account {
        env.extend(account_env(agent, acc));
    }
    env
}

/// Resolve how to launch `agent` with the configured binary/account.
pub fn resolve_launch(cfg: &Config, agent: &AgentKind, account_id: Option<&str>) -> Result<Launch> {
    let mut launch = Launch::default();
    if let AgentKind::Custom(id) = agent {
        let custom: &CustomAgent = cfg.custom_agent(id).ok_or_else(|| anyhow!("custom agent '{id}' is not configured"))?;
        let program = detect::which_program(&custom.command).unwrap_or_else(|| PathBuf::from(&custom.command));
        let (program, prefix) = unwrap_shim(&program);
        launch.program = program;
        launch.prefix_args = prefix;
        launch.prefix_args.extend(custom.args.iter().cloned());
        launch.env = custom.env.iter().map(|(k, v)| (k.clone(), secrets::resolve(v))).collect();
        return Ok(launch);
    }
    let spec = agent.spec();
    if spec.binaries.is_empty() {
        return Ok(launch);
    }
    let ac = cfg.agent(agent);
    let program = detect::find_binary(agent, ac.binary.as_deref()).ok_or_else(|| not_installed(agent))?;
    let (program, prefix) = unwrap_shim(&program);
    launch.program = program;
    launch.prefix_args = prefix;
    launch.extra_args = ac.extra_args.clone();
    launch.env = agent_env(cfg, agent, account_id);
    Ok(launch)
}

fn not_installed(agent: &AgentKind) -> anyhow::Error {
    let spec = agent.spec();
    anyhow!(
        "{} is not installed (looked for {}). Install it with: {}",
        spec.name,
        spec.binaries.join(", "),
        spec.install.first().map(|(_, c)| *c).unwrap_or("see its docs")
    )
}

/// Launch for an agent's ACP mode (`gemini --acp`, `npx -y …-acp`, …).
/// When the ACP command starts with the agent's own binary, the configured
/// binary override is honoured.
pub fn resolve_acp_launch(cfg: &Config, agent: &AgentKind, account_id: Option<&str>) -> Result<Launch> {
    if let AgentKind::Custom(_) = agent {
        return resolve_launch(cfg, agent, account_id);
    }
    let spec = agent.spec();
    let acp = spec.acp_command.ok_or_else(|| anyhow!("{} does not support the Agent Client Protocol", spec.name))?;
    let (first, rest) = acp.split_first().ok_or_else(|| anyhow!("empty ACP command"))?;
    let ac = cfg.agent(agent);
    let program = if spec.binaries.contains(first) {
        detect::find_binary(agent, ac.binary.as_deref()).ok_or_else(|| not_installed(agent))?
    } else {
        detect::which_program(first).ok_or_else(|| anyhow!("`{first}` was not found on PATH (needed to start {} over ACP)", spec.name))?
    };
    let (program, mut prefix) = unwrap_shim(&program);
    prefix.extend(rest.iter().map(|s| s.to_string()));
    Ok(Launch { program, prefix_args: prefix, extra_args: ac.extra_args.clone(), env: agent_env(cfg, agent, account_id) })
}

/// On Windows CLIs are often installed as `.cmd` / `.ps1` shims. Batch files
/// cannot safely carry arbitrary arguments, so unwrap them to the real
/// program when we can:
///
/// * npm `cmd-shim` / `.ps1` shims → `node <script.js>`
/// * shims that wrap an `.exe` (`opencode.cmd`) → that exe
/// * one-line shims (`hermes.cmd`: `"python.exe" -I -c "…" %*`) → that line
/// * Cursor's versioned layout (`cursor-agent.ps1`) → `versions/<latest>/node.exe index.js`
///
/// Anything else is returned unchanged: Rust's standard library escapes
/// batch-file arguments safely (and refuses ones it can't escape), and
/// `.ps1` scripts run through `powershell -File`.
pub fn unwrap_shim(program: &Path) -> (PathBuf, Vec<String>) {
    let ext = program.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let dir = program.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    match ext.as_str() {
        "cmd" | "bat" => {
            let text = std::fs::read_to_string(program).unwrap_or_default();
            // A batch shim that just forwards to a PowerShell script.
            if text.to_ascii_lowercase().contains(".ps1") {
                let ps1 = program.with_extension("ps1");
                if ps1.exists() {
                    if let Some(found) = unwrap_ps1(&ps1) {
                        return found;
                    }
                }
            }
            if let Some(found) = unwrap_cmd_text(&text, &dir) {
                return found;
            }
            (program.to_path_buf(), Vec::new())
        }
        "ps1" => {
            if let Some(found) = unwrap_ps1(program) {
                return found;
            }
            // Prefer a sibling batch shim when one exists.
            for alt in ["cmd", "exe"] {
                let p = program.with_extension(alt);
                if p.exists() {
                    return if alt == "cmd" { unwrap_shim(&p) } else { (p, Vec::new()) };
                }
            }
            let ps = std::env::var("SystemRoot")
                .map(|r| PathBuf::from(r).join("System32").join("WindowsPowerShell").join("v1.0").join("powershell.exe"))
                .unwrap_or_else(|_| PathBuf::from("powershell.exe"));
            (
                ps,
                vec![
                    "-NoProfile".into(),
                    "-ExecutionPolicy".into(),
                    "Bypass".into(),
                    "-File".into(),
                    program.to_string_lossy().to_string(),
                ],
            )
        }
        _ => (program.to_path_buf(), Vec::new()),
    }
}

fn node_beside(dir: &Path) -> PathBuf {
    let node = dir.join("node.exe");
    if node.exists() {
        node
    } else {
        detect::which_program("node").unwrap_or_else(|| PathBuf::from("node"))
    }
}

fn unwrap_cmd_text(text: &str, dir: &Path) -> Option<(PathBuf, Vec<String>)> {
    // npm/cmd-shim:  "%_prog%"  "%dp0%\node_modules\pkg\cli.js" %*
    for ext in [".js\"", ".mjs\"", ".cjs\""] {
        if let Some(rel) = find_quoted(text, ext) {
            let script = dir.join(strip_dp0(&rel));
            if script.exists() {
                return Some((node_beside(dir), vec![script.to_string_lossy().to_string()]));
            }
        }
    }
    // One-line forwarding shim:  "<exe>" args… %*
    let line = text
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('@') && !l.to_ascii_lowercase().starts_with("rem") && !l.starts_with("::"))
        .find(|l| l.ends_with("%*"))?;
    let body = line.trim_end_matches("%*").trim();
    let tokens = tokenize_cmd(body)?;
    let (first, rest) = tokens.split_first()?;
    let exe = PathBuf::from(strip_dp0_join(first, dir));
    let lower = exe.to_string_lossy().to_ascii_lowercase();
    if !lower.ends_with(".exe") || !exe.exists() {
        return None;
    }
    let mut args = Vec::new();
    for t in rest {
        let t = strip_dp0_join(t, dir);
        if t.contains('%') || t.contains('!') {
            return None;
        }
        args.push(t);
    }
    Some((exe, args))
}

/// Split a batch command line into tokens, honouring double quotes.
fn tokenize_cmd(s: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    let mut has = false;
    for c in s.chars() {
        match c {
            '"' => {
                in_q = !in_q;
                has = true;
            }
            c if c.is_whitespace() && !in_q => {
                if has {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            '&' | '|' | '<' | '>' if !in_q => return None,
            c => {
                cur.push(c);
                has = true;
            }
        }
    }
    if in_q {
        return None;
    }
    if has {
        out.push(cur);
    }
    Some(out)
}

fn strip_dp0(s: &str) -> String {
    s.replace("%dp0%\\", "").replace("%~dp0\\", "").replace("%dp0%", "").replace("%~dp0", "").trim_start_matches(['\\', '/']).to_string()
}

fn strip_dp0_join(s: &str, dir: &Path) -> String {
    if s.contains("%dp0%") || s.contains("%~dp0") {
        dir.join(strip_dp0(s)).to_string_lossy().to_string()
    } else {
        s.to_string()
    }
}

fn unwrap_ps1(ps1: &Path) -> Option<(PathBuf, Vec<String>)> {
    let dir = ps1.parent()?.to_path_buf();
    // Cursor's installer: <dir>\node.exe + index.js, or <dir>\versions\<v>\…
    if dir.join("node.exe").exists() && dir.join("index.js").exists() {
        return Some((dir.join("node.exe"), vec![dir.join("index.js").to_string_lossy().to_string()]));
    }
    let versions = dir.join("versions");
    if versions.is_dir() {
        let mut candidates: Vec<(String, PathBuf)> = std::fs::read_dir(&versions)
            .ok()?
            .flatten()
            .map(|e| (e.file_name().to_string_lossy().to_string(), e.path()))
            .filter(|(_, p)| p.join("node.exe").exists() && p.join("index.js").exists())
            .collect();
        candidates.sort_by_key(|(name, _)| version_key(name));
        if let Some((_, best)) = candidates.last() {
            return Some((best.join("node.exe"), vec![best.join("index.js").to_string_lossy().to_string()]));
        }
    }
    // npm ps1 shim:  & "$basedir/node$exe"  "$basedir/node_modules/pkg/cli.js" $args
    let text = std::fs::read_to_string(ps1).ok()?;
    for ext in [".js\"", ".mjs\"", ".cjs\""] {
        if let Some(rel) = find_quoted(&text, ext) {
            let rel = rel.replace("$basedir/", "").replace("$basedir\\", "").replace("$PSScriptRoot\\", "").replace("$PSScriptRoot/", "");
            let script = dir.join(rel.trim_start_matches(['\\', '/']));
            if script.exists() {
                return Some((node_beside(&dir), vec![script.to_string_lossy().to_string()]));
            }
        }
    }
    None
}

/// Sort key for `YYYY.MM.DD[-HH-MM-SS]-commit` version directory names.
fn version_key(name: &str) -> (u32, u32, u32, String) {
    let mut parts = name.split('-');
    let date = parts.next().unwrap_or("");
    let nums: Vec<u32> = date.split('.').map(|n| n.parse().unwrap_or(0)).collect();
    let rest: Vec<&str> = parts.collect();
    (
        nums.first().copied().unwrap_or(0),
        nums.get(1).copied().unwrap_or(0),
        nums.get(2).copied().unwrap_or(0),
        rest.join("-"),
    )
}

/// Find the last quoted string in `text` that ends with `suffix` (suffix
/// includes the closing quote).
fn find_quoted(text: &str, suffix: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let end = lower.rfind(suffix)? + suffix.len() - 1;
    let start = text[..end].rfind('"')? + 1;
    Some(text[start..end].to_string())
}

/// Assemble a full request for the session's next turn. The new user message
/// must already be the last message in `session`.
pub fn build_request(cfg: &Config, session: &Session, agent: &AgentKind) -> Result<TurnRequest> {
    let spec = agent.spec();
    let ac = cfg.agent(agent);
    let custom_protocol = match agent {
        AgentKind::Custom(id) => cfg.custom_agent(id).map(|c| c.protocol),
        _ => None,
    };
    let protocol = custom_protocol.unwrap_or_else(|| {
        if ac.prefer_acp && spec.acp_command.is_some() {
            Protocol::Acp
        } else {
            spec.protocol
        }
    });
    let last = session.messages.last().ok_or_else(|| anyhow!("session has no prompt"))?;
    let mut prompt = last.text();
    // Pasted text travels inline; agents only see plain prompts.
    for p in &last.parts {
        if let Part::File { path, text: Some(t), .. } = p {
            prompt.push_str(&format!("\n\n<pasted_text name=\"{path}\">\n{t}\n</pasted_text>"));
        }
    }
    let attachments: Vec<Part> = last.parts.iter().filter(|p| matches!(p, Part::Image { .. } | Part::File { .. })).cloned().collect();
    let cwd = session
        .cwd()
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")));
    let link = session.native_link(agent);
    let resume_id = link.map(|l| l.id.clone());
    let resume_path = link.and_then(|l| l.path.clone()).filter(|p| Path::new(p).exists());
    let resume_at = link.and_then(|l| l.resume_at.clone()).filter(|_| resume_id.is_some());
    let handoff = if protocol != Protocol::HttpChat && engine::needs_handoff(session, agent) {
        let from = engine::unsynced_from(session, agent);
        let upto = session.messages.len().saturating_sub(1);
        let mut s = session.clone();
        s.messages.truncate(upto);
        Some(s.handoff_context(from, 60_000))
    } else {
        None
    };
    let model = session.model.clone().or(ac.default_model.clone()).filter(|m| m != "default" && !m.is_empty());
    // The agent's own modes (Claude Code permission modes, Codex plan +
    // permissions, …): session pick, else the agent's default, else the
    // global autonomy level.
    let modes = spec.resolve_modes(
        [session.mode.as_deref(), session.permission.as_deref()],
        [ac.default_mode.as_deref(), ac.default_permission.as_deref()],
        cfg.settings.run_mode,
    );
    let mode = modes.ids.first().cloned().or_else(|| session.mode.clone().or(ac.default_mode.clone()));

    let (launch, chat) = match protocol {
        Protocol::HttpChat => (Launch::default(), Some(resolve_chat(cfg, session)?)),
        Protocol::Acp => (resolve_acp_launch(cfg, agent, session.account_id.as_deref())?, None),
        _ => (resolve_launch(cfg, agent, session.account_id.as_deref())?, None),
    };

    Ok(TurnRequest {
        agent: agent.clone(),
        protocol,
        session_id: session.id.clone(),
        prompt,
        attachments,
        history: session.messages.clone(),
        model,
        mode,
        modes,
        cwd,
        run_mode: cfg.settings.run_mode,
        resume_id,
        resume_path,
        resume_at,
        handoff,
        launch,
        chat,
    })
}

/// For the built-in Chat agent, `session.model` is `provider_id/model`.
pub fn resolve_chat(cfg: &Config, session: &Session) -> Result<ChatTarget> {
    let raw = session.model.clone().unwrap_or_default();
    let (pid, model) = match raw.split_once('/') {
        Some((p, m)) if cfg.provider(p).is_some() => (p.to_string(), m.to_string()),
        _ => {
            let p = cfg
                .providers
                .iter()
                .find(|p| p.enabled)
                .ok_or_else(|| anyhow!("No chat provider is enabled. Add an API key in Settings → Models."))?;
            let m = if raw.is_empty() { p.default_model.clone().or(p.models.first().cloned()).unwrap_or_default() } else { raw.clone() };
            (p.id.clone(), m)
        }
    };
    let p = cfg.provider(&pid).ok_or_else(|| anyhow!("provider {pid} not found"))?;
    let api_key = p.secret_ref.as_deref().and_then(secrets::get);
    if p.kind.needs_key() && api_key.is_none() {
        return Err(anyhow!("{} has no API key. Add one in Settings → Models.", p.name));
    }
    if model.trim().is_empty() {
        return Err(anyhow!("Pick a model for {} first.", p.name));
    }
    Ok(ChatTarget { kind: p.kind, base_url: p.base_url.clone(), api_key, model, provider_name: p.name.clone() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("larrier-shim-{name}-{}", larrier_core::lsf::new_id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn npm_cmd_shim_unwraps_to_node() {
        let d = tmpdir("npm");
        std::fs::create_dir_all(d.join("node_modules/@openai/codex/bin")).unwrap();
        std::fs::write(d.join("node_modules/@openai/codex/bin/codex.js"), "").unwrap();
        std::fs::write(d.join("node.exe"), "").unwrap();
        let shim = d.join("codex.cmd");
        std::fs::write(&shim, "@ECHO off\r\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\"  \"%dp0%\\node_modules\\@openai\\codex\\bin\\codex.js\" %*\r\n").unwrap();
        let (prog, args) = unwrap_shim(&shim);
        assert_eq!(prog, d.join("node.exe"));
        assert!(args[0].ends_with("codex.js"));
        // The .ps1 flavour resolves to the same thing.
        std::fs::write(d.join("codex.ps1"), "& \"$basedir/node$exe\"  \"$basedir/node_modules/@openai/codex/bin/codex.js\" $args").unwrap();
        let (prog2, args2) = unwrap_shim(&d.join("codex.ps1"));
        assert_eq!(prog2, d.join("node.exe"));
        assert!(args2[0].ends_with("codex.js"));
    }

    #[test]
    fn exe_and_one_line_shims() {
        let d = tmpdir("exe");
        std::fs::create_dir_all(d.join("node_modules/opencode-ai/bin")).unwrap();
        std::fs::write(d.join("node_modules/opencode-ai/bin/opencode.exe"), "").unwrap();
        std::fs::write(d.join("opencode.cmd"), "@ECHO off\r\nGOTO start\r\n:start\r\n\"%dp0%\\node_modules\\opencode-ai\\bin\\opencode.exe\"   %*\r\n").unwrap();
        let (prog, args) = unwrap_shim(&d.join("opencode.cmd"));
        assert!(prog.to_string_lossy().ends_with("opencode.exe"));
        assert!(args.is_empty());

        let py = d.join("python.exe");
        std::fs::write(&py, "").unwrap();
        std::fs::write(d.join("hermes.cmd"), format!("@echo off\r\n\"{}\" -I -c \"import base64; exec(base64.b64decode('aGk='))\" %*\r\n", py.display())).unwrap();
        let (prog, args) = unwrap_shim(&d.join("hermes.cmd"));
        assert_eq!(prog, py);
        assert_eq!(args, vec!["-I".to_string(), "-c".into(), "import base64; exec(base64.b64decode('aGk='))".into()]);
    }

    #[test]
    fn cursor_versions_layout() {
        let d = tmpdir("cursor");
        for v in ["2026.08.01-abc", "2026.09.10-fd3934a", "2026.9.2-def"] {
            let vd = d.join("versions").join(v);
            std::fs::create_dir_all(&vd).unwrap();
            std::fs::write(vd.join("node.exe"), "").unwrap();
            std::fs::write(vd.join("index.js"), "").unwrap();
        }
        std::fs::write(d.join("cursor-agent.ps1"), "& \"$nodePath\" \"$scriptPath\\versions\\$versionName\\index.js\" $args").unwrap();
        std::fs::write(d.join("cursor-agent.cmd"), "powershell.exe -NoProfile -File \"%SCRIPT_DIR%\\cursor-agent.ps1\" %*").unwrap();
        let (prog, args) = unwrap_shim(&d.join("cursor-agent.cmd"));
        assert!(prog.to_string_lossy().contains("2026.09.10-fd3934a"), "{prog:?}");
        assert!(args[0].ends_with("index.js"));
    }

    #[test]
    fn unknown_batch_is_left_for_std_escaping() {
        let d = tmpdir("odd");
        let shim = d.join("odd.cmd");
        std::fs::write(&shim, "@echo off\r\ncall something %1 & other\r\n").unwrap();
        let (prog, args) = unwrap_shim(&shim);
        assert_eq!(prog, shim);
        assert!(args.is_empty());
    }
}
