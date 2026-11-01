//! What each agent itself offers and prefers: its default model and
//! reasoning effort, the models it lists, and the providers configured in it.
//! Read from the agent's own files and CLI listings, never written; the app
//! caches the result and refreshes it now and then.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use larrier_core::AgentKind;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Reasoning efforts in order, as agents spell them.
pub const EFFORT_ORDER: &[&str] = &["none", "off", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"];

/// "Extra high" for `xhigh`, and so on.
pub fn effort_label(e: &str) -> String {
    match e {
        "xhigh" | "extra-high" | "extra_high" => "Extra high".into(),
        "max" => "Max".into(),
        "none" => "None".into(),
        "off" => "Off".into(),
        _ => {
            let mut c = e.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
    }
}

fn effort_rank(e: &str) -> usize {
    EFFORT_ORDER.iter().position(|x| *x == e).unwrap_or(EFFORT_ORDER.len())
}

fn sort_efforts(v: &mut Vec<String>) {
    v.sort_by_key(|e| effort_rank(e));
    v.dedup();
}

/// Everything an agent says about models.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct AgentModels {
    /// The agent's own default model, as its `--model` takes it.
    #[serde(default)]
    pub default_model: Option<String>,
    /// The agent's own default reasoning effort.
    #[serde(default)]
    pub default_effort: Option<String>,
    #[serde(default)]
    pub models: Vec<AgentModel>,
    /// Providers and keys configured in the agent itself.
    #[serde(default)]
    pub providers: Vec<NativeProvider>,
    /// Worth knowing about the agent's own setup.
    #[serde(default)]
    pub notes: Vec<String>,
    /// Files and commands this was read from.
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub fetched_at: Option<DateTime<Utc>>,
    /// When the agent's model list last came from its CLI.
    #[serde(default)]
    pub listed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub error: Option<String>,
    /// The agent's own / commands (built-in, custom, skills, plugins, MCP
    /// prompts), as the agent lists them.
    #[serde(default)]
    pub commands: Vec<AgentCommand>,
    /// Subagents it can be asked to hand work to (`@agent-<name>`).
    #[serde(default)]
    pub subagents: Vec<AgentCommand>,
    /// MCP servers as the agent itself reports them (with the ones its
    /// account or plugins add, which no config file names).
    #[serde(default)]
    pub mcp: Vec<McpServer>,
}

/// An MCP server as an agent reports it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct McpServer {
    pub name: String,
    /// connected, failed, needs-auth, pending or disabled.
    pub status: String,
    /// Where it comes from: user, project, local, plugin, claudeai, …
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub tools: usize,
    #[serde(default)]
    pub error: Option<String>,
}

/// One of an agent's own commands or subagents.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct AgentCommand {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// What to type after it ("consent | revoke").
    #[serde(default)]
    pub hint: Option<String>,
}

/// What an agent's CLI lists besides its models.
#[derive(Clone, Debug, Default)]
pub struct Menu {
    pub commands: Vec<AgentCommand>,
    pub subagents: Vec<AgentCommand>,
    pub mcp: Vec<McpServer>,
}

impl AgentModels {
    pub fn model(&self, id: &str) -> Option<&AgentModel> {
        self.models
            .iter()
            .find(|m| m.id == id)
            .or_else(|| self.models.iter().find(|m| m.variants.iter().any(|v| v.id == id)))
            .or_else(|| self.models.iter().find(|m| m.aliases.iter().any(|a| a == id)))
    }

    /// The model and variant an id names (Cursor's ids name a variant; one
    /// with parameters names its model).
    pub fn variant(&self, id: &str) -> Option<(&AgentModel, Option<&Variant>)> {
        let id = split_params(id).0;
        let m = self.model(id)?;
        Some((m, m.variants.iter().find(|v| v.id == id)))
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct AgentModel {
    /// What the agent's `--model` takes (for Cursor, the family: its
    /// variants carry the ids).
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Reasoning efforts the agent accepts for it.
    #[serde(default)]
    pub efforts: Vec<String>,
    #[serde(default)]
    pub default_effort: Option<String>,
    /// In the agent's own picker: Cursor's enabled models, Codex's listed
    /// ones, OpenCode's configured and recent ones.
    #[serde(default)]
    pub featured: bool,
    /// The user's own model in the agent (a custom model or provider).
    #[serde(default)]
    pub custom: bool,
    /// From the agent's model list (CLI or its cache), not its settings.
    #[serde(default)]
    pub listed: bool,
    /// Provider or family heading.
    #[serde(default)]
    pub group: Option<String>,
    /// One id per effort and speed (Cursor).
    #[serde(default)]
    pub variants: Vec<Variant>,
    #[serde(default)]
    pub context: Option<u64>,
    /// The agent's default when nothing is chosen (from its listing).
    #[serde(default)]
    pub default: bool,
    /// Other ids for the same model (what Claude Code's `opus` stands for).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    /// How the agent takes its settings as parameters of the model
    /// (Cursor's `grok-4.7[context=500k,reasoning_effort=high,fast=false]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<ModelParams>,
}

/// A model's parameters as its agent names them (Cursor's app lists them).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct ModelParams {
    /// Context sizes to choose from: (value, label) — ("500k", "500K").
    #[serde(default)]
    pub contexts: Vec<(String, String)>,
    /// The context used when none is chosen.
    #[serde(default)]
    pub default_context: Option<String>,
    /// The effort parameter's name (`effort`, `reasoning_effort`, `reasoning`)
    /// and its values as the agent spells them (`extra-high`).
    #[serde(default)]
    pub effort: Option<(String, Vec<String>)>,
    #[serde(default)]
    pub fast: bool,
}

impl ModelParams {
    /// `id[context=…,<effort>=…,fast=…]` for an effort (Larrier's names)
    /// and speed.
    pub fn compose(&self, id: &str, context: &str, effort: Option<&str>, fast: bool) -> String {
        let mut parts = vec![format!("context={context}")];
        if let (Some((name, values)), Some(e)) = (&self.effort, effort) {
            let raw = values.iter().find(|v| normalize_effort(v) == e).cloned().unwrap_or_else(|| e.to_string());
            parts.push(format!("{name}={raw}"));
        }
        if self.fast {
            parts.push(format!("fast={fast}"));
        }
        format!("{id}[{}]", parts.join(","))
    }
}

/// An effort as Larrier names it (`extra-high` → `xhigh`).
pub fn normalize_effort(v: &str) -> String {
    if v == "extra-high" { "xhigh".to_string() } else { v.to_string() }
}

/// A model id with parameters, split: (`grok-4.7`, [(context, 500k), …]).
pub fn split_params(id: &str) -> (&str, Vec<(String, String)>) {
    match id.split_once('[') {
        Some((base, rest)) => {
            let inner = rest.trim_end_matches(']');
            let kv = inner.split(',').filter_map(|p| p.split_once('=')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
            (base, kv)
        }
        None => (id, Vec::new()),
    }
}

impl AgentModel {
    fn new(id: impl Into<String>, name: impl Into<String>) -> AgentModel {
        AgentModel { id: id.into(), name: name.into(), ..Default::default() }
    }

    /// The variant for an effort and speed; the closest one when that
    /// exact pair isn't offered.
    pub fn pick_variant(&self, effort: Option<&str>, fast: bool) -> Option<&Variant> {
        let exact = self.variants.iter().find(|v| v.effort.as_deref() == effort && v.fast == fast);
        exact
            .or_else(|| self.variants.iter().find(|v| v.effort.as_deref() == effort))
            .or_else(|| self.variants.iter().find(|v| v.effort.is_none() && v.fast == fast))
            .or_else(|| self.variants.iter().find(|v| v.fast == fast))
            .or_else(|| self.variants.first())
    }

    pub fn has_fast(&self) -> bool {
        self.variants.iter().any(|v| v.fast)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Variant {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub fast: bool,
}

/// A provider, custom endpoint or own key set up in the agent itself.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct NativeProvider {
    pub name: String,
    #[serde(default)]
    pub base_url: Option<String>,
    /// A key is configured (never its value).
    #[serde(default)]
    pub has_key: bool,
    /// Switched on, for keys an app can turn on and off (Cursor).
    #[serde(default)]
    pub active: Option<bool>,
    #[serde(default)]
    pub models: Vec<String>,
}

/// Whether Larrier can read a model list from the agent's CLI (slow: it
/// may go to the network).
pub fn has_cli_listing(agent: &AgentKind) -> bool {
    matches!(agent, AgentKind::ClaudeCode | AgentKind::Cursor | AgentKind::OpenCode | AgentKind::Droid)
}

/// Read what `agent` offers. `program` is its executable (for the CLI
/// listing, when `deep`); otherwise the listing from `prev` is kept.
pub async fn read(agent: &AgentKind, program: Option<&Path>, deep: bool, prev: Option<&AgentModels>) -> AgentModels {
    let listing = match (deep, program) {
        (true, Some(p)) if has_cli_listing(agent) => cli_listing(agent, p).await,
        _ => None,
    };
    let kept = || Menu {
        commands: prev.map(|p| p.commands.clone()).unwrap_or_default(),
        subagents: prev.map(|p| p.subagents.clone()).unwrap_or_default(),
        mcp: prev.map(|p| p.mcp.clone()).unwrap_or_default(),
    };
    let (listed, menu, listed_at, listing_error) = match listing {
        Some(Ok((v, menu))) => (v, menu, Some(Utc::now()), None),
        Some(Err(e)) => (prev.map(|p| keep_listed(p)).unwrap_or_default(), kept(), prev.and_then(|p| p.listed_at), Some(e)),
        None => (prev.map(|p| keep_listed(p)).unwrap_or_default(), kept(), prev.and_then(|p| p.listed_at), None),
    };
    let agent2 = agent.clone();
    let mut m = tokio::task::spawn_blocking(move || read_files(&agent2, listed)).await.unwrap_or_default();
    m.fetched_at = Some(Utc::now());
    m.listed_at = listed_at;
    m.commands = menu.commands;
    m.subagents = menu.subagents;
    m.mcp = menu.mcp;
    if m.error.is_none() {
        m.error = listing_error;
    }
    m
}

fn keep_listed(p: &AgentModels) -> Vec<AgentModel> {
    p.models.iter().filter(|m| m.listed).cloned().collect()
}

/// The agent's files, on top of its CLI model list.
pub fn read_files(agent: &AgentKind, listed: Vec<AgentModel>) -> AgentModels {
    let mut m = match agent {
        AgentKind::ClaudeCode => claude(&larrier_formats::claude::root(None), listed),
        AgentKind::Codex => codex(&larrier_formats::codex::root(None)),
        AgentKind::Cursor => cursor(&larrier_formats::cursor::config_dir(), larrier_formats::cursor::ide_db().as_deref(), listed),
        AgentKind::OpenCode => opencode(&opencode_config_dir(), &opencode_state_dir(), listed),
        AgentKind::Droid => droid(&factory_home(), listed),
        AgentKind::Hermes => hermes(&larrier_formats::hermes::home_dir()),
        AgentKind::Gemini => gemini(&larrier_formats::util::home().join(".gemini")),
        AgentKind::Pi => pi(&pi_dir()),
        _ => AgentModels::default(),
    };
    if m.models.is_empty() {
        // Agents Larrier doesn't read yet: the catalog's suggestions.
        m.models = agent
            .spec()
            .models
            .iter()
            .filter(|id| !matches!(**id, "default" | "auto"))
            .map(|id| AgentModel { featured: true, ..AgentModel::new(*id, larrier_core::models::display_name(id)) })
            .collect();
    }
    m
}

// ------------------------------------------------------------------ helpers

fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok().or_else(|| serde_json::from_str(&strip_jsonc(&text)).ok())
}

/// JSON with comments and trailing commas (OpenCode's `opencode.jsonc`).
pub fn strip_jsonc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let b: Vec<char> = text.chars().collect();
    let (mut i, mut in_str) = (0, false);
    while i < b.len() {
        let c = b[i];
        if in_str {
            out.push(c);
            if c == '\\' && i + 1 < b.len() {
                out.push(b[i + 1]);
                i += 1;
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
            out.push(c);
        } else if c == '/' && b.get(i + 1) == Some(&'/') {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
            continue;
        } else if c == '/' && b.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == '*' && b[i + 1] == '/') {
                i += 1;
            }
            i += 2;
            continue;
        } else if c == ',' {
            // Drop a comma that only has whitespace before `}` or `]`.
            let next = b[i + 1..].iter().find(|c| !c.is_whitespace());
            if !matches!(next, Some('}') | Some(']')) {
                out.push(c);
            }
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

fn str_of<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut cur = v;
    for p in path {
        cur = cur.get(*p)?;
    }
    cur.as_str().filter(|s| !s.trim().is_empty())
}

fn note_source(m: &mut AgentModels, path: &Path) {
    if path.exists() {
        m.sources.push(path.display().to_string());
    }
}

/// Run a CLI quietly and return its output (stdout, then stderr).
async fn run_cli(program: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    let (prog, prefix) = crate::request::unwrap_shim(program);
    let mut cmd = tokio::process::Command::new(prog);
    cmd.args(prefix).args(args).stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    cmd.kill_on_drop(true);
    cmd.env("NO_COLOR", "1");
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    match tokio::time::timeout(timeout, cmd.output()).await {
        Err(_) => Err("timed out".into()),
        Ok(Err(e)) => Err(e.to_string()),
        Ok(Ok(o)) => {
            let mut text = String::from_utf8_lossy(&o.stdout).to_string();
            text.push('\n');
            text.push_str(&String::from_utf8_lossy(&o.stderr));
            Ok(text)
        }
    }
}

async fn cli_listing(agent: &AgentKind, program: &Path) -> Option<Result<(Vec<AgentModel>, Menu), String>> {
    if *agent == AgentKind::ClaudeCode {
        return Some(claude_listing(program).await);
    }
    let (args, secs): (&[&str], u64) = match agent {
        AgentKind::Cursor => (&["--list-models"], 60),
        AgentKind::OpenCode => (&["models", "--verbose"], 90),
        AgentKind::Droid => (&["exec", "--help"], 30),
        _ => return None,
    };
    let text = match run_cli(program, args, Duration::from_secs(secs)).await {
        Ok(t) => t,
        Err(e) => return Some(Err(format!("Couldn't list models: {e}"))),
    };
    let models = match agent {
        AgentKind::Cursor => parse_cursor_list(&text),
        AgentKind::OpenCode => parse_opencode_models(&text),
        AgentKind::Droid => parse_droid_help(&text).models,
        _ => Vec::new(),
    };
    Some(if models.is_empty() { Err(first_line(&text)) } else { Ok((models, Menu::default())) })
}

fn first_line(text: &str) -> String {
    let l = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("no output");
    format!("Couldn't list models: {}", l.chars().take(160).collect::<String>())
}

/// Printable text only (Cursor pads some names with zero-width spaces).
fn clean(s: &str) -> String {
    s.chars().filter(|c| !c.is_control() && !matches!(c, '\u{200b}'..='\u{200f}' | '\u{feff}')).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

// ------------------------------------------------------------------ Claude Code

/// Claude Code's own model menu: what `initialize` answers over its control
/// protocol (no prompt is sent, so nothing is spent).
async fn claude_listing(program: &Path) -> Result<(Vec<AgentModel>, Menu), String> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (prog, prefix) = crate::request::unwrap_shim(program);
    let mut cmd = tokio::process::Command::new(prog);
    cmd.args(prefix).args(["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"]);
    cmd.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
    cmd.kill_on_drop(true);
    cmd.env_remove("CLAUDECODE");
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let mut child = cmd.spawn().map_err(|e| format!("Couldn't list models: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let line = "{\"type\":\"control_request\",\"request_id\":\"larrier_models\",\"request\":{\"subtype\":\"initialize\"}}\n";
    stdin.write_all(line.as_bytes()).await.map_err(|e| e.to_string())?;
    let mut lines = BufReader::new(child.stdout.take().ok_or("no stdout")?).lines();
    let read = async {
        while let Ok(Some(l)) = lines.next_line().await {
            let Ok(v) = serde_json::from_str::<Value>(&l) else { continue };
            if v["type"] == "control_response" && v["response"]["request_id"] == "larrier_models" {
                return Some(v);
            }
        }
        None
    };
    let reply = tokio::time::timeout(Duration::from_secs(30), read).await.ok().flatten();
    // Then its MCP servers, as it connects to them (account connectors
    // and plugins included). They load in the background after initialize
    // (none are listed for the first moments), so ask again until the list
    // has settled.
    let mut mcp: Vec<McpServer> = Vec::new();
    if reply.is_some() {
        let started = std::time::Instant::now();
        for n in 0.. {
            let rid = format!("larrier_mcp_{n}");
            let line = format!("{{\"type\":\"control_request\",\"request_id\":\"{rid}\",\"request\":{{\"subtype\":\"mcp_status\"}}}}\n");
            if stdin.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            let read = async {
                while let Ok(Some(l)) = lines.next_line().await {
                    let Ok(v) = serde_json::from_str::<Value>(&l) else { continue };
                    if v["type"] == "control_response" && v["response"]["request_id"] == rid.as_str() {
                        return Some(v);
                    }
                }
                None
            };
            let Some(r) = tokio::time::timeout(Duration::from_secs(20), read).await.ok().flatten() else { break };
            mcp = parse_mcp_status(&r["response"]["response"]);
            let settled = !mcp.is_empty() && mcp.iter().all(|s| s.status != "pending");
            if settled || started.elapsed() > Duration::from_secs(8) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(800)).await;
        }
    }
    drop(stdin);
    let _ = child.kill().await;
    let v = reply.ok_or("Couldn't list models: Claude Code didn't answer")?;
    let init = &v["response"]["response"];
    let models = parse_claude_models(init);
    if models.is_empty() {
        return Err(v["response"]["error"].as_str().map(|e| format!("Couldn't list models: {e}")).unwrap_or_else(|| "Couldn't list models: none reported".into()));
    }
    let mut menu = parse_claude_menu(init);
    menu.mcp = mcp;
    Ok((models, menu))
}

/// Claude Code's `mcp_status` reply: each server with its state.
pub fn parse_mcp_status(v: &Value) -> Vec<McpServer> {
    v["mcpServers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            Some(McpServer {
                name: str_of(s, &["name"])?.to_string(),
                status: str_of(s, &["status"]).unwrap_or("pending").to_string(),
                source: str_of(s, &["source"]).or_else(|| str_of(s, &["scope"])).map(str::to_string),
                tools: s["tools"].as_array().map(|t| t.len()).unwrap_or(0),
                error: str_of(s, &["error"]).map(str::to_string),
            })
        })
        .collect()
}

/// Claude Code's own / commands and subagents (its `initialize` reply).
/// Internal entries (`__…`) are left out.
pub fn parse_claude_menu(init: &Value) -> Menu {
    let list = |key: &str| -> Vec<AgentCommand> {
        init[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| {
                let name = str_of(c, &["name"])?.to_string();
                (!name.starts_with("__")).then(|| AgentCommand {
                    name,
                    description: str_of(c, &["description"]).map(str::to_string).filter(|d| !d.trim().is_empty()),
                    hint: str_of(c, &["argumentHint"]).map(str::to_string).filter(|h| !h.trim().is_empty()),
                })
            })
            .collect()
    };
    Menu { commands: list("commands"), subagents: list("agents"), mcp: Vec::new() }
}

/// Models from Claude Code's `initialize` reply: the latest of each family
/// by alias (`opus` → "Opus 5.5"), older ones by their own id.
pub fn parse_claude_models(init: &Value) -> Vec<AgentModel> {
    let mut out = Vec::new();
    for x in init["models"].as_array().into_iter().flatten() {
        let Some(id) = str_of(x, &["value"]) else { continue };
        let resolved = str_of(x, &["resolvedModel"]).unwrap_or(id).to_string();
        let mut m = AgentModel::new(id, str_of(x, &["displayName"]).unwrap_or(id));
        m.description = str_of(x, &["description"]).map(str::to_string);
        m.efforts = x["supportedEffortLevels"].as_array().map(|a| a.iter().filter_map(|e| e.as_str().map(str::to_string)).collect()).unwrap_or_default();
        m.listed = true;
        m.context = larrier_core::models::lookup(&resolved).and_then(|i| i.context);
        if id == "default" {
            // The agent's own default: Larrier lists it as "Default".
            m.default = true;
            m.aliases = vec![resolved];
        } else {
            let alias = !id.starts_with("claude-");
            m.featured = alias;
            m.group = Some(if alias { "Latest".into() } else { "Older models".into() });
            if resolved != id {
                m.aliases = vec![resolved];
            }
        }
        out.push(m);
    }
    out
}

const CLAUDE_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

#[cfg(test)]
mod param_tests {
    use super::*;

    #[test]
    fn a_model_with_parameters_reads_and_writes_them() {
        let p = ModelParams {
            contexts: vec![("256k".into(), "256K".into()), ("500k".into(), "500K".into())],
            default_context: Some("256k".into()),
            effort: Some(("reasoning".into(), vec!["low".into(), "high".into(), "extra-high".into()])),
            fast: true,
        };
        assert_eq!(p.compose("gpt-5.5", "500k", Some("xhigh"), false), "gpt-5.5[context=500k,reasoning=extra-high,fast=false]");
        let (base, kv) = split_params("grok-4.7[context=500k,reasoning_effort=high,fast=true]");
        assert_eq!(base, "grok-4.7");
        assert_eq!(kv, vec![("context".into(), "500k".into()), ("reasoning_effort".into(), "high".into()), ("fast".into(), "true".into())]);
        assert_eq!(split_params("opus").0, "opus");
    }
}

fn claude(dir: &Path, listed: Vec<AgentModel>) -> AgentModels {
    use larrier_core::models::{self, ModelStatus, Source, Vendor};
    let mut m = AgentModels::default();
    let settings_path = dir.join("settings.json");
    note_source(&mut m, &settings_path);
    let s = read_json(&settings_path).unwrap_or(Value::Null);
    let env = |k: &str| str_of(&s, &["env", k]).map(str::to_string);
    let efforts: Vec<String> = CLAUDE_EFFORTS.iter().map(|e| e.to_string()).collect();
    m.default_model = env("ANTHROPIC_MODEL").or_else(|| str_of(&s, &["model"]).map(str::to_string));
    // What an alias stands for: the pin in the agent's env, else the newest.
    let resolve = |id: &str| -> String {
        let pinned = match id {
            "opus" => env("ANTHROPIC_DEFAULT_OPUS_MODEL"),
            "sonnet" => env("ANTHROPIC_DEFAULT_SONNET_MODEL"),
            "haiku" => env("ANTHROPIC_DEFAULT_HAIKU_MODEL"),
            "fable" => env("ANTHROPIC_DEFAULT_FABLE_MODEL"),
            _ => None,
        };
        pinned.unwrap_or_else(|| models::alias(id).map(|(t, _)| t.to_string()).unwrap_or_else(|| models::normalize(id)))
    };
    // Per-model effort first (what /effort saves), then the general one.
    let effort_for = |id: &str| -> Option<String> {
        let canon = resolve(id);
        s.get("modelSettings")
            .and_then(|ms| ms.get(&canon).or_else(|| ms.get(id)))
            .and_then(|x| str_of(x, &["effortLevel"]))
            .or_else(|| str_of(&s, &["effortLevel"]))
            .map(str::to_string)
    };
    m.default_effort = env("CLAUDE_CODE_EFFORT_LEVEL").or_else(|| effort_for(m.default_model.as_deref().unwrap_or("opus")));
    let allowed: Vec<String> = s.get("availableModels").and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
    if !listed.is_empty() {
        // Claude Code's own menu, with the effort its settings keep per model.
        let default_target = listed.iter().find(|x| x.default).and_then(|x| x.aliases.first().cloned());
        for mut am in listed.into_iter().filter(|x| !x.default) {
            if m.default_model.is_none() && default_target.as_ref().is_some_and(|t| am.aliases.contains(t) || &am.id == t) {
                m.default_model = Some(am.id.clone());
            }
            am.default_effort = effort_for(&am.id).filter(|e| am.efforts.contains(e));
            if !allowed.is_empty() {
                am.featured = allowed.iter().any(|a| *a == am.id || am.aliases.contains(a));
            }
            m.models.push(am);
        }
        if m.default_effort.is_none() {
            m.default_effort = effort_for(m.default_model.as_deref().unwrap_or("opus"));
        }
    }
    let catalog = m.models.is_empty();
    for (alias, label) in [("opus", "Opus"), ("sonnet", "Sonnet"), ("haiku", "Haiku"), ("fable", "Fable")].into_iter().filter(|_| catalog) {
        let target = resolve(alias);
        let mut am = AgentModel::new(alias, label);
        am.description = Some(format!("Latest · {}", models::display_name(&target)));
        am.efforts = if alias == "haiku" { Vec::new() } else { efforts.clone() };
        am.default_effort = effort_for(alias).filter(|_| !am.efforts.is_empty());
        am.featured = allowed.is_empty() || allowed.iter().any(|a| a == alias);
        am.group = Some("Aliases".into());
        m.models.push(am);
    }
    for info in models::all().into_iter().filter(|i| catalog && i.vendor == Vendor::Anthropic && i.source == Source::Official && matches!(i.status, ModelStatus::Active | ModelStatus::Legacy)) {
        let mut am = AgentModel::new(info.id.clone(), info.name.clone());
        am.efforts = if info.id.contains("haiku") { Vec::new() } else { efforts.clone() };
        am.default_effort = s.get("modelSettings").and_then(|ms| ms.get(&info.id)).and_then(|x| str_of(x, &["effortLevel"])).map(str::to_string);
        am.featured = allowed.iter().any(|a| models::normalize(a) == info.id) || m.default_model.as_deref() == Some(info.id.as_str());
        am.context = info.context;
        am.description = info.tag.clone();
        am.group = Some(if info.status == ModelStatus::Active { "Anthropic".into() } else { "Older models".into() });
        m.models.push(am);
    }
    for a in &allowed {
        if !m.models.iter().any(|x| &x.id == a || models::normalize(&x.id) == models::normalize(a)) {
            m.models.push(AgentModel { featured: true, custom: true, group: Some("Allowed in settings".into()), ..AgentModel::new(a.clone(), models::display_name(a)) });
        }
    }
    if let Some(id) = env("ANTHROPIC_CUSTOM_MODEL_OPTION") {
        let name = env("ANTHROPIC_CUSTOM_MODEL_OPTION_NAME").unwrap_or_else(|| models::display_name(&id));
        m.models.push(AgentModel { featured: true, custom: true, group: Some("Custom".into()), ..AgentModel::new(id, name) });
    }
    if let Some(d) = m.default_model.clone() {
        if !m.models.iter().any(|x| x.id == d) {
            m.models.push(AgentModel { featured: true, custom: true, efforts: efforts.clone(), ..AgentModel::new(d.clone(), models::display_name(&d)) });
        }
    }
    if let Some(url) = env("ANTHROPIC_BASE_URL") {
        let has_key = env("ANTHROPIC_API_KEY").is_some() || env("ANTHROPIC_AUTH_TOKEN").is_some() || str_of(&s, &["apiKeyHelper"]).is_some();
        m.providers.push(NativeProvider { name: "Custom endpoint".into(), base_url: Some(url.clone()), has_key, active: Some(true), models: Vec::new() });
        m.notes.push(format!("Claude Code's settings send its requests to {url}."));
    } else if env("ANTHROPIC_API_KEY").is_some() || env("ANTHROPIC_AUTH_TOKEN").is_some() {
        m.providers.push(NativeProvider { name: "Anthropic API key".into(), base_url: None, has_key: true, active: Some(true), models: Vec::new() });
    }
    for (var, cloud) in [("CLAUDE_CODE_USE_BEDROCK", "Amazon Bedrock"), ("CLAUDE_CODE_USE_VERTEX", "Google Vertex AI"), ("CLAUDE_CODE_USE_FOUNDRY", "Microsoft Foundry")] {
        if env(var).map(|v| v == "1" || v == "true").unwrap_or(false) {
            m.notes.push(format!("Claude Code runs on {cloud} (from its settings)."));
        }
    }
    m
}

// ------------------------------------------------------------------ Codex

fn codex(dir: &Path) -> AgentModels {
    let mut m = AgentModels::default();
    let cfg_path = dir.join("config.toml");
    note_source(&mut m, &cfg_path);
    let cfg = toml::Value::Table(std::fs::read_to_string(&cfg_path).ok().and_then(|t| toml::from_str::<toml::Table>(&t).ok()).unwrap_or_default());
    let get = |k: &str| cfg.get(k).and_then(|v| v.as_str()).map(str::to_string);
    m.default_model = get("model");
    m.default_effort = get("model_reasoning_effort");
    let provider = get("model_provider").unwrap_or_else(|| "openai".into());
    if get("service_tier").as_deref() == Some("priority") || get("service_tier").as_deref() == Some("fast") {
        m.notes.push("Fast mode (priority processing) is on in Codex's settings.".into());
    }
    let cache_path = dir.join("models_cache.json");
    if let Some(cache) = read_json(&cache_path) {
        note_source(&mut m, &cache_path);
        for x in cache.get("models").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
            let Some(slug) = str_of(&x, &["slug"]) else { continue };
            let mut am = AgentModel::new(slug, str_of(&x, &["display_name"]).map(codex_name).unwrap_or_else(|| larrier_core::models::display_name(slug)));
            am.description = str_of(&x, &["description"]).map(str::to_string);
            am.efforts = x.get("supported_reasoning_levels").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|e| str_of(e, &["effort"]).map(str::to_string)).collect()).unwrap_or_default();
            am.default_effort = str_of(&x, &["default_reasoning_level"]).map(str::to_string);
            am.featured = str_of(&x, &["visibility"]) != Some("hide");
            am.listed = true;
            am.group = Some("OpenAI".into());
            m.models.push(am);
        }
    }
    if let Some(providers) = cfg.get("model_providers").and_then(|v| v.as_table()) {
        for (id, p) in providers {
            let name = p.get("name").and_then(|v| v.as_str()).unwrap_or(id).to_string();
            let base_url = p.get("base_url").and_then(|v| v.as_str()).map(str::to_string);
            let has_key = p.get("env_key").and_then(|v| v.as_str()).map(|k| std::env::var_os(k).is_some()).unwrap_or(false) || p.get("experimental_bearer_token").is_some();
            m.providers.push(NativeProvider { name: format!("{name} ({id})"), base_url, has_key, active: Some(*id == provider), models: Vec::new() });
        }
    }
    if provider != "openai" {
        m.notes.push(format!("Codex uses the \"{provider}\" provider from its settings."));
    }
    if let Some(d) = m.default_model.clone() {
        if !m.models.iter().any(|x| x.id == d) {
            m.models.push(AgentModel { featured: true, custom: provider != "openai", group: Some(provider.clone()), ..AgentModel::new(d.clone(), larrier_core::models::display_name(&d)) });
        }
    }
    m
}

/// "GPT-6.1-Sol" → "GPT-6.1 Sol" (Codex's names hyphenate the tier).
fn codex_name(n: &str) -> String {
    match n.rsplit_once('-') {
        Some((head, tail)) if head.ends_with(|c: char| c.is_ascii_digit()) && !tail.is_empty() && tail.chars().all(char::is_alphabetic) => format!("{head} {tail}"),
        _ => n.to_string(),
    }
}

// ------------------------------------------------------------------ Cursor

/// `cursor-agent --list-models`: "id - Name" lines, one per variant;
/// variants of a model are grouped by stripping the effort and speed.
pub fn parse_cursor_list(text: &str) -> Vec<AgentModel> {
    let mut out: Vec<AgentModel> = Vec::new();
    for line in text.lines() {
        let line = clean(line);
        let Some((id, name)) = line.split_once(" - ") else { continue };
        let id = id.trim();
        if id.is_empty() || id.contains(' ') {
            continue;
        }
        let (base, effort, fast) = cursor_split(id);
        let name = name.trim().trim_end_matches("(default)").trim().to_string();
        let base_name = cursor_base_name(&name);
        let v = Variant { id: id.to_string(), name: name.clone(), effort: effort.clone(), fast };
        match out.iter_mut().find(|m| m.id == base) {
            Some(m) => {
                if effort.is_none() && !fast {
                    m.name = base_name;
                }
                m.variants.push(v);
            }
            None => {
                let mut m = AgentModel::new(base.clone(), base_name);
                m.listed = true;
                m.featured = true;
                m.variants.push(v);
                out.push(m);
            }
        }
    }
    for m in out.iter_mut() {
        let mut efforts: Vec<String> = m.variants.iter().filter_map(|v| v.effort.clone()).collect();
        sort_efforts(&mut efforts);
        m.efforts = efforts;
        m.default_effort = if m.variants.iter().any(|v| v.effort.is_none()) { None } else { ["medium", "high"].iter().find(|e| m.efforts.iter().any(|x| x == *e)).map(|e| e.to_string()) };
        m.group = Some(larrier_core::models::Vendor::infer(m.id.trim_start_matches("cursor-"), "").name().to_string());
    }
    out
}

/// `claude-opus-5-5-high-fast` → (`claude-opus-5-5`, high, fast).
pub fn cursor_split(id: &str) -> (String, Option<String>, bool) {
    let mut s = id;
    let fast = s.ends_with("-fast");
    if fast {
        s = &s[..s.len() - 5];
    }
    for e in ["xhigh", "medium", "high", "low", "max", "minimal", "none"] {
        if let Some(rest) = s.strip_suffix(&format!("-{e}")) {
            return (rest.to_string(), Some(e.to_string()), fast);
        }
    }
    (s.to_string(), None, fast)
}

/// A variant's name without its effort and speed words ("Claude Opus 5 1M
/// Low Thinking Fast" → "Claude Opus 5 1M Thinking").
fn cursor_base_name(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    let mut out: Vec<&str> = Vec::new();
    for (i, w) in words.iter().enumerate() {
        let effort_word = matches!(*w, "Fast" | "Low" | "Medium" | "High" | "Max" | "Minimal") || (*w == "Extra" && words.get(i + 1) == Some(&"High"));
        if i == 0 || !effort_word {
            out.push(w);
        }
    }
    out.join(" ")
}

fn cursor(cli_dir: &Path, ide_db: Option<&Path>, listed: Vec<AgentModel>) -> AgentModels {
    let mut m = AgentModels { models: listed, ..Default::default() };
    let cli_path = cli_dir.join("cli-config.json");
    note_source(&mut m, &cli_path);
    let cli = read_json(&cli_path).unwrap_or(Value::Null);
    let param = |params: Option<&Value>| -> Option<String> {
        params?.as_array()?.iter().find(|p| matches!(str_of(p, &["id"]), Some("effort" | "reasoning_effort" | "reasoning"))).and_then(|p| str_of(p, &["value"])).map(|v| if v == "extra-high" { "xhigh".to_string() } else { v.to_string() })
    };
    let cli_model = str_of(&cli, &["selectedModel", "modelId"]).or_else(|| str_of(&cli, &["model", "modelId"])).map(str::to_string);
    let cli_effort = param(cli.pointer("/selectedModel/parameters"));
    let fast = cli.pointer("/selectedModel/parameters").and_then(|p| p.as_array()).map(|a| a.iter().any(|p| str_of(p, &["id"]) == Some("fast") && str_of(p, &["value"]) == Some("true"))).unwrap_or(false);
    // The CLI keeps the family and its parameters: name the variant.
    m.default_model = cli_model.as_ref().map(|base| {
        m.models
            .iter()
            .find(|x| &x.id == base || x.variants.iter().any(|v| &v.id == base))
            .and_then(|x| x.pick_variant(cli_effort.as_deref(), fast))
            .map(|v| v.id.clone())
            .unwrap_or_else(|| base.clone())
    });
    m.default_effort = cli_effort.clone();
    let history: Vec<String> = cli.get("modelSelectionHistory").and_then(|h| h.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
    // Per-model effort the user last picked (CLI, then the app).
    let mut prefs: Vec<(String, String)> = cli
        .get("modelParameters")
        .and_then(|p| p.as_object())
        .map(|o| o.iter().filter_map(|(k, v)| param(Some(v)).map(|e| (k.clone(), e))).collect())
        .unwrap_or_default();

    // The Cursor app: which models are switched on, and its own keys.
    let mut enabled: Option<Vec<String>> = None;
    let mut params: Vec<(String, ModelParams)> = Vec::new();
    if let Some(db) = ide_db {
        if let Ok(conn) = larrier_formats::util::open_sqlite(db) {
            let get = |k: &str| conn.query_row("SELECT value FROM ItemTable WHERE key = ?1", [k], |r| r.get::<_, String>(0)).ok();
            let has = |k: &str| conn.query_row("SELECT 1 FROM ItemTable WHERE key = ?1", [k], |_| Ok(())).is_ok();
            if let Some(user) = get("src.vs.platform.reactivestorage.browser.reactiveStorageServiceImpl.persistentStorage.applicationUser").and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
                m.sources.push(format!("{} (Cursor app)", db.display()));
                let ai = user.get("aiSettings").cloned().unwrap_or(Value::Null);
                let list = |k: &str| -> Vec<String> { ai.get(k).and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default() };
                let on = list("modelOverrideEnabled");
                let off = list("modelOverrideDisabled");
                let mut set: Vec<String> = user
                    .get("availableDefaultModels2")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter(|x| x.get("defaultOn").and_then(|d| d.as_bool()).unwrap_or(false)).filter_map(|x| str_of(x, &["name"]).map(str::to_string)).collect())
                    .unwrap_or_default();
                set.extend(on);
                set.retain(|x| !off.contains(x));
                enabled = Some(set);
                // Each model's parameters (context sizes, how it names its
                // effort), and the context last picked for it.
                let picked_context = |name: &str| -> Option<String> {
                    let from = |v: Option<&Value>| v.and_then(|p| p.as_array()).and_then(|a| a.iter().find(|p| str_of(p, &["id"]) == Some("context"))).and_then(|p| str_of(p, &["value"])).map(str::to_string);
                    from(cli.pointer(&format!("/modelParameters/{name}/parameters"))).or_else(|| from(ai.pointer(&format!("/modelParameterPreferences/{name}/parameters"))))
                };
                if let Some(defs) = user.get("availableDefaultModels2").and_then(|v| v.as_array()) {
                    for d in defs {
                        let Some(name) = str_of(d, &["name"]) else { continue };
                        let mut p = ModelParams::default();
                        for def in d.get("parameterDefinitions").and_then(|x| x.as_array()).into_iter().flatten() {
                            let id = str_of(def, &["id"]).unwrap_or("");
                            let values: Vec<(String, String)> = def
                                .pointer("/parameterType/enumParameter/values")
                                .and_then(|v| v.as_array())
                                .map(|a| a.iter().filter_map(|x| str_of(x, &["value"]).map(|v| (v.to_string(), str_of(x, &["displayName"]).unwrap_or(v).to_string()))).collect())
                                .unwrap_or_default();
                            match id {
                                "context" if values.len() > 1 => p.contexts = values,
                                "effort" | "reasoning_effort" | "reasoning" if !values.is_empty() => p.effort = Some((id.to_string(), values.into_iter().map(|(v, _)| v).collect())),
                                "fast" => p.fast = def.pointer("/parameterType/booleanParameter").is_some(),
                                _ => {}
                            }
                        }
                        if p.contexts.is_empty() {
                            continue;
                        }
                        p.default_context = picked_context(name).filter(|c| p.contexts.iter().any(|(v, _)| v == c)).or_else(|| p.contexts.first().map(|(v, _)| v.clone()));
                        params.push((name.to_string(), p));
                    }
                }
                if let Some(o) = ai.get("modelParameterPreferences").and_then(|p| p.as_object()) {
                    for (k, v) in o {
                        if let Some(e) = param(v.get("parameters")) {
                            if !prefs.iter().any(|(id, _)| id == k) {
                                prefs.push((k.clone(), e));
                            }
                        }
                    }
                }
                let openai_on = user.get("useOpenAIKey").and_then(|v| v.as_bool()).unwrap_or(false);
                let claude_on = user.get("useClaudeKey").and_then(|v| v.as_bool()).unwrap_or(false);
                let base = user.get("openAIBaseUrl").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(str::to_string);
                let openai_key = has("secret://cursorAuth/openAIKey");
                let claude_key = has("secret://cursorAuth/claudeKey");
                let google_key = has("secret://cursorAuth/googleKey");
                if openai_key || openai_on || base.is_some() {
                    m.providers.push(NativeProvider { name: "OpenAI key (Cursor app)".into(), base_url: base, has_key: openai_key, active: Some(openai_on), models: Vec::new() });
                }
                if claude_key || claude_on {
                    m.providers.push(NativeProvider { name: "Anthropic key (Cursor app)".into(), base_url: None, has_key: claude_key, active: Some(claude_on), models: Vec::new() });
                }
                if google_key {
                    m.providers.push(NativeProvider { name: "Google key (Cursor app)".into(), base_url: None, has_key: true, active: None, models: Vec::new() });
                }
            }
        }
    }
    // The app's names are families ("grok-4.7", "claude-opus-5-5").
    let matches = |base: &str, name: &str| base == name || base.strip_prefix("cursor-") == Some(name) || base.strip_suffix("-thinking") == Some(name);
    for x in m.models.iter_mut() {
        let on = match &enabled {
            Some(set) => set.iter().any(|n| matches(&x.id, n)),
            None => true,
        };
        x.featured = x.id == "auto" || on || history.iter().any(|h| matches(&x.id, h) || x.variants.iter().any(|v| &v.id == h));
        if let Some((_, e)) = prefs.iter().find(|(k, _)| matches(&x.id, k)) {
            if x.efforts.iter().any(|y| y == e) {
                x.default_effort = Some(e.clone());
            }
        }
        if let Some((_, p)) = params.iter().find(|(n, _)| matches(&x.id, n)) {
            x.params = Some(p.clone());
        }
    }
    if let Some(d) = m.default_model.clone() {
        if let Some(x) = m.models.iter_mut().find(|x| x.id == d || x.variants.iter().any(|v| v.id == d)) {
            x.featured = true;
        }
    }
    m.notes.push("Cursor Agent runs every model through your Cursor account. Your own OpenAI or Anthropic keys and custom models only apply inside the Cursor app.".into());
    m
}

// ------------------------------------------------------------------ OpenCode

fn opencode_config_dir() -> PathBuf {
    larrier_formats::util::env_dir("XDG_CONFIG_HOME").unwrap_or_else(|| larrier_formats::util::home().join(".config")).join("opencode")
}

fn opencode_state_dir() -> PathBuf {
    larrier_formats::util::env_dir("XDG_STATE_HOME").unwrap_or_else(|| larrier_formats::util::home().join(".local").join("state")).join("opencode")
}

/// `opencode models --verbose`: "provider/model" lines, each followed by
/// the model as pretty JSON.
pub fn parse_opencode_models(text: &str) -> Vec<AgentModel> {
    let mut out = Vec::new();
    let mut header: Option<String> = None;
    let mut buf = String::new();
    for line in text.lines() {
        if header.is_some() {
            buf.push_str(line);
            buf.push('\n');
            if line == "}" {
                let id = header.take().unwrap_or_default();
                if let Ok(v) = serde_json::from_str::<Value>(&buf) {
                    out.push(opencode_model(&id, &v));
                }
                buf.clear();
            }
        } else if line.starts_with('{') && !out.is_empty() {
            continue;
        } else if !line.trim().is_empty() && !line.starts_with(char::is_whitespace) && line.contains('/') && !line.contains(' ') {
            header = Some(line.trim().to_string());
            buf.clear();
        }
    }
    out
}

fn opencode_model(id: &str, v: &Value) -> AgentModel {
    let mut m = AgentModel::new(id, str_of(v, &["name"]).map(str::to_string).unwrap_or_else(|| larrier_core::models::display_name(id)));
    m.group = str_of(v, &["providerID"]).map(str::to_string).or_else(|| id.split_once('/').map(|(p, _)| p.to_string()));
    m.efforts = v.get("variants").and_then(|x| x.as_object()).map(|o| o.keys().filter(|k| *k != "default").cloned().collect()).unwrap_or_default();
    sort_efforts(&mut m.efforts);
    m.context = v.pointer("/limit/context").and_then(|x| x.as_u64());
    m.listed = true;
    m
}

fn opencode_data_dir() -> PathBuf {
    larrier_formats::util::env_dir("XDG_DATA_HOME").unwrap_or_else(|| larrier_formats::util::home().join(".local").join("share")).join("opencode")
}

fn opencode(cfg_dir: &Path, state_dir: &Path, listed: Vec<AgentModel>) -> AgentModels {
    let mut m = AgentModels { models: listed, ..Default::default() };
    // Providers signed in with `opencode auth login` (only which ones).
    let logged_in: Vec<String> = read_json(&opencode_data_dir().join("auth.json")).and_then(|v| v.as_object().map(|o| o.keys().cloned().collect())).unwrap_or_default();
    let cfg_path = ["opencode.jsonc", "opencode.json", "config.json"].iter().map(|n| cfg_dir.join(n)).find(|p| p.exists());
    let cfg = cfg_path.as_deref().and_then(read_json).unwrap_or(Value::Null);
    if let Some(p) = &cfg_path {
        note_source(&mut m, p);
    }
    m.default_model = str_of(&cfg, &["model"]).map(str::to_string);
    let state_path = state_dir.join("model.json");
    let state = read_json(&state_path).unwrap_or(Value::Null);
    note_source(&mut m, &state_path);
    let refs = |k: &str| -> Vec<String> {
        state.get(k).and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| Some(format!("{}/{}", str_of(x, &["providerID"])?, str_of(x, &["modelID"])?))).collect()).unwrap_or_default()
    };
    let mut featured: Vec<String> = refs("favorite");
    featured.extend(refs("recent"));
    if let Some(d) = &m.default_model {
        featured.push(d.clone());
        m.default_effort = str_of(&state, &["variant", d.as_str()]).filter(|v| *v != "default").map(str::to_string);
    }
    if let Some(providers) = cfg.get("provider").and_then(|p| p.as_object()) {
        for (pid, p) in providers {
            let models: Vec<String> = p.get("models").and_then(|x| x.as_object()).map(|o| o.keys().map(|k| format!("{pid}/{k}")).collect()).unwrap_or_default();
            featured.extend(models.iter().cloned());
            let base_url = str_of(p, &["options", "baseURL"]).map(str::to_string);
            let has_key = str_of(p, &["options", "apiKey"]).is_some() || logged_in.contains(pid);
            m.providers.push(NativeProvider { name: str_of(p, &["name"]).unwrap_or(pid).to_string(), base_url, has_key, active: None, models: models.clone() });
            for id in models {
                if !m.models.iter().any(|x| x.id == id) {
                    m.models.push(AgentModel { custom: true, group: Some(pid.clone()), ..AgentModel::new(id.clone(), larrier_core::models::display_name(&id)) });
                }
            }
        }
    }
    for x in m.models.iter_mut() {
        x.featured = featured.contains(&x.id);
        if let Some(e) = str_of(&state, &["variant", x.id.as_str()]).filter(|v| *v != "default") {
            x.default_effort = Some(e.to_string());
        }
    }
    if let Some(d) = m.default_model.clone() {
        if !m.models.iter().any(|x| x.id == d) {
            m.models.push(AgentModel { featured: true, ..AgentModel::new(d.clone(), larrier_core::models::display_name(&d)) });
        }
    }
    m
}

// ------------------------------------------------------------------ Droid

fn factory_home() -> PathBuf {
    larrier_formats::util::env_dir("FACTORY_HOME_OVERRIDE").unwrap_or_else(larrier_formats::util::home).join(".factory")
}

/// What `droid exec --help` lists: models, custom models, each one's
/// reasoning efforts, and the default.
#[derive(Default)]
pub struct DroidHelp {
    pub models: Vec<AgentModel>,
    pub default_model: Option<String>,
}

pub fn parse_droid_help(text: &str) -> DroidHelp {
    let mut h = DroidHelp::default();
    let mut section = "";
    // Name → (efforts, default) from "Model details".
    let mut details: Vec<(String, Vec<String>, Option<String>)> = Vec::new();
    for raw in text.lines() {
        let line = clean(raw);
        match line.as_str() {
            "Available Models:" => {
                section = "models";
                continue;
            }
            "Custom Models:" => {
                section = "custom";
                continue;
            }
            "Model details:" => {
                section = "details";
                continue;
            }
            _ => {}
        }
        if line.is_empty() {
            if section != "details" {
                section = "";
            }
            continue;
        }
        if !raw.starts_with(' ') && !line.starts_with('-') {
            section = "";
            continue;
        }
        match section {
            "models" | "custom" => {
                let Some((id, name)) = line.split_once(' ') else { continue };
                let mut name = name.trim().to_string();
                let default = name.ends_with("(default)");
                if default {
                    name = name.trim_end_matches("(default)").trim().to_string();
                    h.default_model = Some(id.to_string());
                }
                let mut m = AgentModel::new(id, name);
                m.default = default;
                m.listed = true;
                m.featured = true;
                m.custom = section == "custom";
                m.group = Some(if m.custom { "Custom".into() } else { larrier_core::models::Vendor::infer(id, "").name().to_string() });
                h.models.push(m);
            }
            "details" => {
                // "- Claude Opus 4.8: supports reasoning: Yes; supported: [off, low]; default: high"
                let Some(rest) = line.strip_prefix("- ") else { continue };
                let Some((name, info)) = rest.split_once(": supports reasoning:") else { continue };
                let efforts = info.split_once("supported: [").and_then(|(_, r)| r.split_once(']')).map(|(l, _)| l.split(',').map(|e| e.trim().to_string()).filter(|e| !e.is_empty() && e != "none").collect()).unwrap_or_default();
                let default = info.split_once("default:").map(|(_, d)| d.trim().trim_end_matches(';').to_string()).filter(|d| !d.is_empty() && d != "none");
                details.push((name.trim().to_string(), efforts, default));
            }
            _ => {}
        }
    }
    for m in h.models.iter_mut() {
        if let Some((_, e, d)) = details.iter().find(|(n, _, _)| *n == m.name) {
            m.efforts = e.clone();
            sort_efforts(&mut m.efforts);
            m.default_effort = d.clone();
        }
    }
    h
}

fn droid(home: &Path, listed: Vec<AgentModel>) -> AgentModels {
    let mut m = AgentModels { models: listed, ..Default::default() };
    let path = home.join("settings.json");
    note_source(&mut m, &path);
    let s = read_json(&path).unwrap_or(Value::Null);
    // The listing marks the default; settings may name another.
    m.default_model = str_of(&s, &["model"]).or_else(|| str_of(&s, &["sessionDefaultSettings", "model"])).map(str::to_string);
    m.default_effort = str_of(&s, &["reasoningEffort"]).or_else(|| str_of(&s, &["sessionDefaultSettings", "reasoningEffort"])).map(str::to_string);
    for c in s.get("customModels").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
        let Some(id) = str_of(&c, &["id"]) else { continue };
        let name = str_of(&c, &["displayName"]).or_else(|| str_of(&c, &["model"])).unwrap_or(id).to_string();
        let base_url = str_of(&c, &["baseUrl"]).map(str::to_string);
        m.providers.push(NativeProvider { name: name.clone(), base_url, has_key: str_of(&c, &["apiKey"]).is_some(), active: None, models: vec![id.to_string()] });
        match m.models.iter_mut().find(|x| x.id == id) {
            Some(x) => x.custom = true,
            None => m.models.push(AgentModel { featured: true, custom: true, group: Some("Custom".into()), ..AgentModel::new(id, name) }),
        }
    }
    if m.default_model.is_none() {
        // `droid exec --help` marks its own default.
        m.default_model = m.models.iter().find(|x| x.default).map(|x| x.id.clone());
    }
    if let Some(d) = m.default_model.clone() {
        if m.default_effort.is_none() {
            m.default_effort = m.models.iter().find(|x| x.id == d).and_then(|x| x.default_effort.clone());
        }
    }
    m
}

// ------------------------------------------------------------------ Hermes

const HERMES_EFFORTS: &[&str] = &["none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"];

/// The `key: value` pairs of a top-level YAML section (Hermes's
/// `config.yaml`; only plain scalars are needed).
pub fn yaml_section(text: &str, section: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if !line.starts_with(' ') && !line.trim().is_empty() {
            inside = line.trim_end() == format!("{section}:");
            continue;
        }
        if !inside {
            continue;
        }
        let t = line.trim();
        if t.starts_with('#') || line.starts_with("    ") {
            continue;
        }
        if let Some((k, v)) = t.split_once(':') {
            let v = v.trim().trim_matches(|c| c == '"' || c == '\'');
            if !v.is_empty() {
                out.push((k.trim().to_string(), v.to_string()));
            }
        }
    }
    out
}

fn hermes(home: &Path) -> AgentModels {
    let mut m = AgentModels::default();
    let path = home.join("config.yaml");
    note_source(&mut m, &path);
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let model = yaml_section(&text, "model");
    let get = |k: &str| model.iter().find(|(x, _)| x == k).map(|(_, v)| v.clone());
    m.default_model = get("default");
    m.default_effort = yaml_section(&text, "agent").into_iter().find(|(k, _)| k == "reasoning_effort").map(|(_, v)| v);
    let provider = get("provider");
    let efforts: Vec<String> = HERMES_EFFORTS.iter().map(|e| e.to_string()).collect();
    let cache_path = home.join("provider_models_cache.json");
    if let (Some(cache), Some(p)) = (read_json(&cache_path), provider.as_deref()) {
        note_source(&mut m, &cache_path);
        for id in cache.get(p).and_then(|x| x.get("models")).and_then(|x| x.as_array()).cloned().unwrap_or_default().iter().filter_map(|x| x.as_str()) {
            m.models.push(AgentModel { featured: true, listed: true, efforts: efforts.clone(), group: Some(p.to_string()), ..AgentModel::new(id, larrier_core::models::display_name(id)) });
        }
    }
    if let Some(p) = &provider {
        m.providers.push(NativeProvider { name: format!("Provider: {p}"), base_url: get("base_url"), has_key: true, active: Some(true), models: Vec::new() });
    }
    if let Some(d) = m.default_model.clone() {
        if !m.models.iter().any(|x| x.id == d) {
            m.models.push(AgentModel { featured: true, efforts: efforts.clone(), ..AgentModel::new(d.clone(), larrier_core::models::display_name(&d)) });
        }
    }
    m
}

// ------------------------------------------------------------------ Gemini, Pi

fn gemini(dir: &Path) -> AgentModels {
    let mut m = AgentModels::default();
    let path = dir.join("settings.json");
    note_source(&mut m, &path);
    let s = read_json(&path).unwrap_or(Value::Null);
    m.default_model = str_of(&s, &["model", "name"]).or_else(|| str_of(&s, &["model"])).map(str::to_string);
    m
}

fn pi_dir() -> PathBuf {
    larrier_formats::util::env_dir("PI_CODING_AGENT_DIR").unwrap_or_else(|| larrier_formats::util::home().join(".pi").join("agent"))
}

fn pi(dir: &Path) -> AgentModels {
    let mut m = AgentModels::default();
    let path = dir.join("settings.json");
    note_source(&mut m, &path);
    let s = read_json(&path).unwrap_or(Value::Null);
    if let (Some(p), Some(model)) = (str_of(&s, &["defaultProvider"]), str_of(&s, &["defaultModel"])) {
        m.default_model = Some(format!("{p}/{model}"));
    }
    m.default_effort = str_of(&s, &["defaultThinkingLevel"]).map(str::to_string);
    let models_path = dir.join("models.json");
    if let Some(v) = read_json(&models_path) {
        note_source(&mut m, &models_path);
        for (pid, p) in v.get("providers").and_then(|x| x.as_object()).cloned().unwrap_or_default() {
            let ids: Vec<String> = p.get("models").and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|x| str_of(x, &["id"]).map(|id| format!("{pid}/{id}"))).collect()).unwrap_or_default();
            m.providers.push(NativeProvider { name: pid.clone(), base_url: str_of(&p, &["baseUrl"]).map(str::to_string), has_key: str_of(&p, &["apiKey"]).is_some(), active: None, models: ids.clone() });
            for id in ids {
                m.models.push(AgentModel { featured: true, custom: true, group: Some(pid.clone()), ..AgentModel::new(id.clone(), larrier_core::models::display_name(&id)) });
            }
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_variants_group_by_family() {
        let text = "Available models\n\nauto - Auto (default)\ngpt-5.3-codex-low - Codex 5.3 Low\ngpt-5.3-codex - Codex 5.3\ngpt-5.3-codex-high-fast - Codex 5.3 High Fast\n\
                    claude-opus-5-5-medium - Claude Opus 5.5 1M\nclaude-opus-5-5-xhigh - Claude Opus 5.5 1M Extra High\nclaude-opus-5-5-max-fast - Claude Opus 5.5 1M Max Fast\n\
                    grok-4.7-low-fast - Grok 4.7  Low Fast\u{200b}\u{200b}\n";
        let v = parse_cursor_list(text);
        let ids: Vec<&str> = v.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["auto", "gpt-5.3-codex", "claude-opus-5-5", "grok-4.7"]);
        let codex = &v[1];
        assert_eq!(codex.name, "Codex 5.3");
        assert_eq!(codex.efforts, ["low", "high"]);
        assert_eq!(codex.pick_variant(Some("high"), true).unwrap().id, "gpt-5.3-codex-high-fast");
        assert_eq!(codex.pick_variant(None, false).unwrap().id, "gpt-5.3-codex");
        let opus = &v[2];
        assert_eq!(opus.name, "Claude Opus 5.5 1M");
        assert_eq!(opus.efforts, ["medium", "xhigh", "max"]);
        assert_eq!(opus.default_effort.as_deref(), Some("medium"));
        assert!(opus.has_fast());
        assert_eq!(v[3].name, "Grok 4.7");
        assert_eq!(cursor_split("claude-opus-5-thinking-xhigh-fast"), ("claude-opus-5-thinking".into(), Some("xhigh".into()), true));
    }

    #[test]
    fn droid_help_lists_models_and_efforts() {
        let text = "Usage: droid exec\n\nAvailable Models:\n  auto                         Auto Model\n  claude-opus-4-8              Claude Opus 4.8 (default)\n  gpt-5.5                      GPT-5.5\n\n\
                    Custom Models:\n  custom:GLM-5.2-0             GLM 5.2\n\nModel details:\n  - Auto Model: supports reasoning: No; supported: [none]; default: none\n\
                    \x20 - Claude Opus 4.8: supports reasoning: Yes; supported: [off, low, medium, high, xhigh, max]; default: high\n  - GPT-5.5: supports reasoning: Yes; supported: [low, medium, high, xhigh]; default: medium\n\nExamples:\n  droid exec \"hi\"\n";
        let h = parse_droid_help(text);
        assert_eq!(h.default_model.as_deref(), Some("claude-opus-4-8"));
        let ids: Vec<&str> = h.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["auto", "claude-opus-4-8", "gpt-5.5", "custom:GLM-5.2-0"]);
        assert_eq!(h.models[1].name, "Claude Opus 4.8");
        assert_eq!(h.models[1].efforts, ["off", "low", "medium", "high", "xhigh", "max"]);
        assert_eq!(h.models[1].default_effort.as_deref(), Some("high"));
        assert!(h.models[3].custom);
        assert!(h.models[0].efforts.is_empty());
    }

    #[test]
    fn opencode_verbose_listing() {
        let text = "openrouter/~anthropic/claude-fable-latest\n{\n  \"id\": \"~anthropic/claude-fable-latest\",\n  \"providerID\": \"openrouter\",\n  \"name\": \"Claude Fable Latest\",\n  \"limit\": {\n    \"context\": 1000000\n  },\n  \"variants\": {\n    \"low\": {},\n    \"high\": {}\n  }\n}\nopencode/big-pickle\n{\n  \"id\": \"big-pickle\",\n  \"providerID\": \"opencode\",\n  \"name\": \"Big Pickle\"\n}\n";
        let v = parse_opencode_models(text);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].id, "openrouter/~anthropic/claude-fable-latest");
        assert_eq!(v[0].name, "Claude Fable Latest");
        assert_eq!(v[0].efforts, ["low", "high"]);
        assert_eq!(v[0].context, Some(1_000_000));
        assert_eq!(v[1].group.as_deref(), Some("opencode"));
    }

    #[test]
    fn jsonc_and_yaml() {
        let j = strip_jsonc("{\n // c\n \"model\": \"a//b\", /* x */\n \"provider\": {\"p\": {\"models\": {\"m\": {},},},},\n}");
        let v: Value = serde_json::from_str(&j).unwrap();
        assert_eq!(v["model"], "a//b");
        assert!(v["provider"]["p"]["models"]["m"].is_object());
        let y = "model:\n  provider: openai-codex\n  default: gpt-6.1-sol\n  base_url: \"https://x\"\nagent:\n  reasoning_effort: xhigh\nvoice:\n  auto_tts: false\n";
        assert_eq!(yaml_section(y, "model"), vec![("provider".to_string(), "openai-codex".to_string()), ("default".into(), "gpt-6.1-sol".into()), ("base_url".into(), "https://x".into())]);
        assert_eq!(yaml_section(y, "agent"), vec![("reasoning_effort".to_string(), "xhigh".to_string())]);
    }

    #[test]
    fn effort_labels_and_codex_names() {
        assert_eq!(effort_label("xhigh"), "Extra high");
        assert_eq!(effort_label("medium"), "Medium");
        assert_eq!(codex_name("GPT-6.1-Sol"), "GPT-6.1 Sol");
        assert_eq!(codex_name("GPT-6.1 Terra"), "GPT-6.1 Terra");
    }

    /// Claude Code's `mcp_status` reply (shape from the Agent SDK's
    /// McpServerStatus; this machine has no servers to report).
    #[test]
    fn claude_mcp_status() {
        let v = serde_json::json!({"mcpServers": [
            {"name": "github", "status": "connected", "scope": "user", "source": "plugin", "tools": [{"name": "search"}, {"name": "pr"}]},
            {"name": "Gmail", "status": "needs-auth", "scope": "claudeai"},
            {"name": "broken", "status": "failed", "error": "spawn ENOENT"}
        ]});
        let s = parse_mcp_status(&v);
        assert_eq!((s[0].name.as_str(), s[0].status.as_str(), s[0].source.as_deref(), s[0].tools), ("github", "connected", Some("plugin"), 2));
        assert_eq!(s[1].source.as_deref(), Some("claudeai"), "scope when there is no source");
        assert_eq!(s[2].error.as_deref(), Some("spawn ENOENT"));
        assert!(parse_mcp_status(&serde_json::json!({"mcpServers": []})).is_empty());
    }

    /// Claude Code's / commands and subagents, as its `initialize` reply
    /// lists them (shapes from 2.1.310).
    #[test]
    fn claude_commands_and_subagents() {
        let init = serde_json::json!({
            "commands": [
                {"name": "compact", "description": "Clear conversation history but keep a summary in context", "argumentHint": "<optional custom summarization instructions>", "builtin": true},
                {"name": "design", "description": "Grant or revoke Claude agent access to your Design projects", "argumentHint": "consent | revoke", "builtin": true},
                {"name": "__remote-workflow", "description": "", "builtin": true},
                {"name": "mcp__github__review", "description": "Review a pull request", "argumentHint": ""}
            ],
            "agents": [
                {"name": "Explore", "description": "Fast read-only search agent for locating code."},
                {"name": "Plan", "description": "Software architect agent for designing implementation plans."}
            ]
        });
        let menu = parse_claude_menu(&init);
        let names: Vec<&str> = menu.commands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["compact", "design", "mcp__github__review"], "internal commands are left out");
        assert_eq!(menu.commands[1].hint.as_deref(), Some("consent | revoke"));
        assert_eq!(menu.commands[2].hint, None, "an empty hint is none");
        assert_eq!(menu.subagents.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["Explore", "Plan"]);
    }

    /// Claude Code 2.1.310's own menu (its `initialize` reply): the latest
    /// models by alias under their real names, older ones to add.
    #[test]
    fn claude_models_from_its_menu() {
        let v: Value = serde_json::from_str(include_str!("../tests/fixtures/claude_init_models.json")).unwrap();
        let listed = parse_claude_models(&v);
        let latest: Vec<(&str, &str)> = listed.iter().filter(|m| m.featured).map(|m| (m.id.as_str(), m.name.as_str())).collect();
        assert_eq!(latest, [("opus", "Opus 5.5"), ("fable", "Fable 5.1"), ("sonnet", "Sonnet 5.5"), ("haiku", "Haiku 5.5")]);
        let opus = listed.iter().find(|m| m.id == "opus").unwrap();
        assert_eq!(opus.aliases, ["claude-opus-5-5"]);
        assert_eq!(opus.efforts, ["low", "medium", "high", "xhigh", "max"]);
        let older: Vec<&str> = listed.iter().filter(|m| m.group.as_deref() == Some("Older models")).map(|m| m.name.as_str()).collect();
        assert!(older.contains(&"Opus 4.8") && older.contains(&"Haiku 4.5"), "{older:?}");
        assert!(listed.iter().any(|m| m.default && m.aliases == ["claude-opus-5-5"]));

        // In the reader: the default is the alias, and the id Claude reports
        // after a turn finds the same model.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("settings.json"), r#"{"effortLevel":"xhigh"}"#).unwrap();
        let m = claude(dir.path(), listed);
        assert_eq!(m.default_model.as_deref(), Some("opus"));
        assert_eq!(m.model("claude-opus-5-5").map(|x| x.id.as_str()), Some("opus"));
        assert!(!m.models.iter().any(|x| x.default), "the default row isn't a model of its own");
        assert_eq!(m.models.iter().find(|x| x.id == "opus").and_then(|x| x.default_effort.as_deref()), Some("xhigh"));
    }
}

#[cfg(test)]
mod real {
    use super::*;

    /// Prints what Larrier reads from the agents on this machine (read only).
    #[tokio::test]
    #[ignore]
    async fn real_agent_models() {
        for agent in [AgentKind::ClaudeCode, AgentKind::Codex, AgentKind::Cursor, AgentKind::OpenCode, AgentKind::Droid, AgentKind::Hermes] {
            let program = crate::detect::find_binary(&agent, None);
            let m = read(&agent, program.as_deref(), true, None).await;
            let featured: Vec<String> = m.models.iter().filter(|x| x.featured).take(12).map(|x| format!("{} [{}]", x.id, x.efforts.join(","))).collect();
            println!(
                "\n== {:?}: default {:?} effort {:?} | {} models ({} featured) | error {:?}\n   featured: {}\n   providers: {:?}\n   notes: {:?}",
                agent,
                m.default_model,
                m.default_effort,
                m.models.len(),
                m.models.iter().filter(|x| x.featured).count(),
                m.error,
                featured.join(" · "),
                m.providers.iter().map(|p| format!("{} key={} active={:?} url={:?}", p.name, p.has_key, p.active, p.base_url)).collect::<Vec<_>>(),
                m.notes
            );
            println!(
                "   commands {} · subagents {} · mcp {:?}",
                m.commands.len(),
                m.subagents.len(),
                m.mcp.iter().map(|s| format!("{} {} {:?} tools={}", s.name, s.status, s.source, s.tools)).collect::<Vec<_>>()
            );
        }
    }
}
