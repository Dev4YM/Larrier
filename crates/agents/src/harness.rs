//! Read-only discovery of every agent's customizations — MCP servers,
//! skills, subagents, rules, slash commands, hooks and plugins — across
//! Claude Code, Codex, Cursor, OpenCode, Gemini CLI, Hermes and Droid, at
//! user and project scope. Nothing here writes to an agent's config.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use larrier_core::AgentKind;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Plugin,
    Mcp,
    Skill,
    Subagent,
    Rule,
    Command,
    Hook,
}

impl ItemKind {
    pub const ALL: [ItemKind; 7] =
        [ItemKind::Plugin, ItemKind::Mcp, ItemKind::Skill, ItemKind::Subagent, ItemKind::Rule, ItemKind::Command, ItemKind::Hook];

    pub fn label(self) -> &'static str {
        match self {
            ItemKind::Plugin => "Plugin",
            ItemKind::Mcp => "MCP",
            ItemKind::Skill => "Skill",
            ItemKind::Subagent => "Subagent",
            ItemKind::Rule => "Rule",
            ItemKind::Command => "Command",
            ItemKind::Hook => "Hook",
        }
    }

    pub fn plural(self) -> &'static str {
        match self {
            ItemKind::Plugin => "Plugins",
            ItemKind::Mcp => "MCPs",
            ItemKind::Skill => "Skills",
            ItemKind::Subagent => "Subagents",
            ItemKind::Rule => "Rules",
            ItemKind::Command => "Commands",
            ItemKind::Hook => "Hooks",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "path", rename_all = "snake_case")]
pub enum Scope {
    User,
    Project(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "message", rename_all = "snake_case")]
pub enum Status {
    Ok,
    NeedsAuth,
    Error(String),
    Disabled,
    Unknown,
}

impl Status {
    pub fn needs_attention(&self) -> bool {
        matches!(self, Status::NeedsAuth | Status::Error(_))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HarnessItem {
    pub kind: ItemKind,
    pub name: String,
    pub agents: Vec<AgentKind>,
    pub scope: Scope,
    /// The file the item was read from.
    pub source: PathBuf,
    pub description: Option<String>,
    /// MCP command line / URL, hook event + matcher + command, …
    pub detail: Option<String>,
    pub status: Status,
}

/// Where each agent keeps its user-level configuration. Separate from the
/// environment so tests can point it at fixtures.
#[derive(Clone, Debug, Default)]
pub struct Roots {
    pub home: PathBuf,
    /// `~/.claude` (or `CLAUDE_CONFIG_DIR`).
    pub claude: PathBuf,
    /// `~/.claude.json` (or `$CLAUDE_CONFIG_DIR/.claude.json`).
    pub claude_json: PathBuf,
    pub codex: PathBuf,
    pub cursor: PathBuf,
    pub opencode: PathBuf,
    pub gemini: PathBuf,
    /// Hermes homes (Windows keeps data in `%LOCALAPPDATA%\hermes`, skills
    /// may also live in `~/.hermes`).
    pub hermes: Vec<PathBuf>,
    pub factory: PathBuf,
}

impl Roots {
    pub fn from_env() -> Roots {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        let (claude, claude_json) = match env("CLAUDE_CONFIG_DIR") {
            Some(d) => (d.clone(), d.join(".claude.json")),
            None => (home.join(".claude"), home.join(".claude.json")),
        };
        let gemini = match env("GEMINI_CLI_HOME") {
            Some(d) if d.join(".gemini").is_dir() => d.join(".gemini"),
            Some(d) => d,
            None => home.join(".gemini"),
        };
        let mut hermes = Vec::new();
        if let Some(h) = env("HERMES_HOME") {
            hermes.push(h);
        } else {
            if cfg!(windows) {
                if let Some(la) = env("LOCALAPPDATA") {
                    hermes.push(la.join("hermes"));
                }
            }
            hermes.push(home.join(".hermes"));
        }
        let opencode = env("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config")).join("opencode");
        Roots {
            codex: env("CODEX_HOME").unwrap_or_else(|| home.join(".codex")),
            cursor: env("CURSOR_CONFIG_DIR").unwrap_or_else(|| home.join(".cursor")),
            factory: home.join(".factory"),
            claude,
            claude_json,
            opencode,
            gemini,
            hermes,
            home,
        }
    }
}

/// Discover every customization for the current user plus the given
/// workspace folders.
pub fn discover(workspaces: &[PathBuf]) -> Vec<HarnessItem> {
    discover_in(&Roots::from_env(), workspaces)
}

pub fn discover_in(roots: &Roots, workspaces: &[PathBuf]) -> Vec<HarnessItem> {
    let mut c = Collector::default();
    claude_user(roots, &mut c);
    codex_user(roots, &mut c);
    cursor_user(roots, &mut c);
    opencode_user(roots, &mut c);
    gemini_user(roots, &mut c);
    hermes_user(roots, &mut c);
    droid_user(roots, &mut c);
    for ws in workspaces {
        if ws.is_dir() {
            project(ws, &mut c);
        }
    }
    c.finish()
}

// ---------------------------------------------------------------------------
// Collection + dedupe
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Collector {
    items: Vec<HarnessItem>,
}

impl Collector {
    fn push(&mut self, item: HarnessItem) {
        // The same MCP server (or skill) configured for several agents shows
        // once, tagged with every agent that uses it.
        let mergeable = matches!(item.kind, ItemKind::Mcp | ItemKind::Skill);
        if mergeable {
            if let Some(existing) = self
                .items
                .iter_mut()
                .find(|e| e.kind == item.kind && e.name == item.name && e.detail == item.detail && e.scope == item.scope)
            {
                for a in item.agents {
                    if !existing.agents.contains(&a) {
                        existing.agents.push(a);
                    }
                }
                return;
            }
        }
        self.items.push(item);
    }

    fn finish(mut self) -> Vec<HarnessItem> {
        self.items.sort_by(|a, b| {
            b.status
                .needs_attention()
                .cmp(&a.status.needs_attention())
                .then(a.kind.cmp(&b.kind))
                .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        self.items
    }
}

fn item(kind: ItemKind, name: impl Into<String>, agent: AgentKind, scope: &Scope, source: &Path) -> HarnessItem {
    HarnessItem {
        kind,
        name: name.into(),
        agents: vec![agent],
        scope: scope.clone(),
        source: source.to_path_buf(),
        description: None,
        detail: None,
        status: Status::Ok,
    }
}

// ---------------------------------------------------------------------------
// Per-agent sources
// ---------------------------------------------------------------------------

fn claude_user(r: &Roots, c: &mut Collector) {
    let user = Scope::User;
    let a = AgentKind::ClaudeCode;
    if let Some(v) = read_json(&r.claude_json) {
        mcp_map(v.get("mcpServers"), a.clone(), &user, &r.claude_json, c);
        if let Some(projects) = v.get("projects").and_then(|p| p.as_object()) {
            for (path, p) in projects {
                mcp_map(p.get("mcpServers"), a.clone(), &Scope::Project(path.clone()), &r.claude_json, c);
            }
        }
    }
    let settings = r.claude.join("settings.json");
    if let Some(v) = read_json(&settings) {
        claude_hooks(v.get("hooks"), &user, &settings, c);
    }
    claude_plugins(r, c);
    skills_dir(&r.claude.join("skills"), a.clone(), &user, c);
    md_items(&r.claude.join("agents"), ItemKind::Subagent, a.clone(), &user, false, c);
    md_items(&r.claude.join("commands"), ItemKind::Command, a.clone(), &user, true, c);
    rule_file(&r.claude.join("CLAUDE.md"), &[a], &user, c);
}

fn claude_hooks(hooks: Option<&Value>, scope: &Scope, source: &Path, c: &mut Collector) {
    let Some(map) = hooks.and_then(|h| h.as_object()) else { return };
    for (event, groups) in map {
        for g in groups.as_array().into_iter().flatten() {
            let matcher = g.get("matcher").and_then(|m| m.as_str()).unwrap_or("*");
            for h in g.get("hooks").and_then(|h| h.as_array()).into_iter().flatten() {
                let cmd = h.get("command").and_then(|c| c.as_str()).unwrap_or_default();
                let mut it = item(ItemKind::Hook, event.clone(), AgentKind::ClaudeCode, scope, source);
                it.detail = Some(format!("matcher {matcher} · {}", clip(cmd, 120)));
                it.status = command_status(cmd);
                c.push(it);
            }
        }
    }
}

fn claude_plugins(r: &Roots, c: &mut Collector) {
    let installed_path = r.claude.join("plugins").join("installed_plugins.json");
    let Some(installed) = read_json(&installed_path) else { return };
    let enabled: BTreeMap<String, bool> = read_json(&r.claude.join("settings.json"))
        .and_then(|s| s.get("enabledPlugins").cloned())
        .and_then(|e| e.as_object().cloned())
        .map(|o| o.into_iter().map(|(k, v)| (k, v.as_bool().unwrap_or(true))).collect())
        .unwrap_or_default();
    let Some(plugins) = installed.get("plugins").and_then(|p| p.as_object()) else { return };
    for (id, entries) in plugins {
        let entry = entries.as_array().and_then(|a| a.first()).cloned().unwrap_or(Value::Null);
        let (name, market) = id.split_once('@').unwrap_or((id.as_str(), ""));
        let mut it = item(ItemKind::Plugin, name, AgentKind::ClaudeCode, &Scope::User, &installed_path);
        let version = entry.get("version").and_then(|v| v.as_str()).unwrap_or("");
        it.detail = Some(format!("{market} {version}").trim().to_string());
        it.status = if enabled.get(id).copied().unwrap_or(false) { Status::Ok } else { Status::Disabled };
        if let Some(install) = entry.get("installPath").and_then(|p| p.as_str()).map(PathBuf::from) {
            let manifest = install.join(".claude-plugin").join("plugin.json");
            if let Some(m) = read_json(&manifest) {
                it.description = m.get("description").and_then(|d| d.as_str()).map(|s| s.to_string());
            }
            it.source = install.clone();
            // Components the plugin brings along.
            let ps = Scope::User;
            let mcp = install.join(".mcp.json");
            if let Some(v) = read_json(&mcp) {
                mcp_map(v.get("mcpServers").or(Some(&v)), AgentKind::ClaudeCode, &ps, &mcp, c);
            }
            skills_dir(&install.join("skills"), AgentKind::ClaudeCode, &ps, c);
            md_items(&install.join("agents"), ItemKind::Subagent, AgentKind::ClaudeCode, &ps, false, c);
            md_items(&install.join("commands"), ItemKind::Command, AgentKind::ClaudeCode, &ps, true, c);
        }
        c.push(it);
    }
}

fn codex_user(r: &Roots, c: &mut Collector) {
    let a = AgentKind::Codex;
    let user = Scope::User;
    let cfg_path = r.codex.join("config.toml");
    if let Some(cfg) = std::fs::read_to_string(&cfg_path).ok().and_then(|t| t.parse::<toml::Table>().ok()) {
        if let Some(servers) = cfg.get("mcp_servers").and_then(|s| s.as_table()) {
            for (name, s) in servers {
                let json = toml_to_json(s);
                c.push(mcp_item(name, &json, a.clone(), &user, &cfg_path));
            }
        }
        if let Some(plugins) = cfg.get("plugins").and_then(|p| p.as_table()) {
            for (id, p) in plugins {
                let (name, market) = id.split_once('@').unwrap_or((id.as_str(), ""));
                let mut it = item(ItemKind::Plugin, name, a.clone(), &user, &cfg_path);
                it.detail = (!market.is_empty()).then(|| market.to_string());
                let enabled = p.get("enabled").and_then(|e| e.as_bool()).unwrap_or(true);
                it.status = if enabled { Status::Ok } else { Status::Disabled };
                c.push(it);
            }
        }
    }
    rule_file(&r.codex.join("AGENTS.md"), &[a.clone()], &user, c);
    md_items(&r.codex.join("prompts"), ItemKind::Command, a.clone(), &user, true, c);
    skills_dir(&r.codex.join("skills"), a, &user, c);
}

fn cursor_user(r: &Roots, c: &mut Collector) {
    let a = AgentKind::Cursor;
    let user = Scope::User;
    let mcp = r.cursor.join("mcp.json");
    if let Some(v) = read_json(&mcp) {
        mcp_map(v.get("mcpServers"), a.clone(), &user, &mcp, c);
    }
    skills_dir(&r.cursor.join("skills"), a.clone(), &user, c);
    md_items(&r.cursor.join("commands"), ItemKind::Command, a.clone(), &user, true, c);
    md_items(&r.cursor.join("agents"), ItemKind::Subagent, a.clone(), &user, false, c);
    let rules = r.cursor.join("rules");
    rule_dir(&rules, &[a], &user, c);
}

fn opencode_config(path: &Path, scope: &Scope, c: &mut Collector) {
    let a = AgentKind::OpenCode;
    let Some(v) = read_json(path) else { return };
    if let Some(mcp) = v.get("mcp").and_then(|m| m.as_object()) {
        for (name, s) in mcp {
            // OpenCode: {type: local|remote, command: [..], url, enabled}
            let mut norm = s.clone();
            if let Some(arr) = s.get("command").and_then(|c| c.as_array()) {
                let mut parts = arr.iter().filter_map(|x| x.as_str());
                if let (Some(cmd), Value::Object(o)) = (parts.next(), &mut norm) {
                    o.insert("command".into(), Value::String(cmd.to_string()));
                    o.insert("args".into(), Value::Array(parts.map(|p| Value::String(p.to_string())).collect()));
                }
            }
            c.push(mcp_item(name, &norm, a.clone(), scope, path));
        }
    }
    if let Some(agents) = v.get("agent").and_then(|m| m.as_object()) {
        for (name, s) in agents {
            let mut it = item(ItemKind::Subagent, name, a.clone(), scope, path);
            it.description = s.get("description").and_then(|d| d.as_str()).map(|s| s.to_string());
            if s.get("disable").and_then(|d| d.as_bool()).unwrap_or(false) {
                it.status = Status::Disabled;
            }
            c.push(it);
        }
    }
    if let Some(cmds) = v.get("command").and_then(|m| m.as_object()) {
        for (name, s) in cmds {
            let mut it = item(ItemKind::Command, format!("/{name}"), a.clone(), scope, path);
            it.description = s.get("description").and_then(|d| d.as_str()).map(|s| s.to_string());
            c.push(it);
        }
    }
    for p in v.get("plugin").and_then(|p| p.as_array()).into_iter().flatten() {
        if let Some(name) = p.as_str() {
            c.push(item(ItemKind::Plugin, name, a.clone(), scope, path));
        }
    }
}

fn opencode_user(r: &Roots, c: &mut Collector) {
    let user = Scope::User;
    for f in ["opencode.json", "opencode.jsonc", "config.json"] {
        opencode_config(&r.opencode.join(f), &user, c);
    }
    let a = AgentKind::OpenCode;
    md_items(&r.opencode.join("agent"), ItemKind::Subagent, a.clone(), &user, false, c);
    md_items(&r.opencode.join("agents"), ItemKind::Subagent, a.clone(), &user, false, c);
    md_items(&r.opencode.join("command"), ItemKind::Command, a.clone(), &user, true, c);
    md_items(&r.opencode.join("commands"), ItemKind::Command, a.clone(), &user, true, c);
    rule_file(&r.opencode.join("AGENTS.md"), &[a], &user, c);
}

fn gemini_settings(path: &Path, scope: &Scope, c: &mut Collector) {
    if let Some(v) = read_json(path) {
        mcp_map(v.get("mcpServers"), AgentKind::Gemini, scope, path, c);
    }
}

fn gemini_user(r: &Roots, c: &mut Collector) {
    let a = AgentKind::Gemini;
    let user = Scope::User;
    gemini_settings(&r.gemini.join("settings.json"), &user, c);
    rule_file(&r.gemini.join("GEMINI.md"), &[a.clone()], &user, c);
    toml_commands(&r.gemini.join("commands"), a.clone(), &user, c);
    if let Ok(rd) = std::fs::read_dir(r.gemini.join("extensions")) {
        for e in rd.flatten() {
            let manifest = e.path().join("gemini-extension.json");
            if let Some(m) = read_json(&manifest) {
                let name = m.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()).unwrap_or_else(|| e.file_name().to_string_lossy().to_string());
                let mut it = item(ItemKind::Plugin, name, a.clone(), &user, &manifest);
                it.description = m.get("description").and_then(|d| d.as_str()).map(|s| s.to_string());
                it.detail = m.get("version").and_then(|v| v.as_str()).map(|v| format!("v{v}"));
                c.push(it);
                mcp_map(m.get("mcpServers"), a.clone(), &user, &manifest, c);
            }
        }
    }
}

fn hermes_user(r: &Roots, c: &mut Collector) {
    let a = AgentKind::Hermes;
    let user = Scope::User;
    for home in &r.hermes {
        let cfg = home.join("config.yaml");
        if let Ok(text) = std::fs::read_to_string(&cfg) {
            for (name, server) in yaml_section_entries(&text, "mcp_servers") {
                c.push(mcp_item(&name, &server, a.clone(), &user, &cfg));
            }
            for (event, cmds) in yaml_hooks(&text) {
                for cmd in cmds {
                    let mut it = item(ItemKind::Hook, event.clone(), a.clone(), &user, &cfg);
                    it.detail = Some(clip(&cmd, 120));
                    it.status = command_status(&cmd);
                    c.push(it);
                }
            }
        }
        skills_dir(&home.join("skills"), a.clone(), &user, c);
    }
}

fn droid_user(r: &Roots, c: &mut Collector) {
    let a = AgentKind::Droid;
    let user = Scope::User;
    let mcp = r.factory.join("mcp.json");
    if let Some(v) = read_json(&mcp) {
        mcp_map(v.get("mcpServers"), a.clone(), &user, &mcp, c);
    }
    md_items(&r.factory.join("droids"), ItemKind::Subagent, a.clone(), &user, false, c);
    md_items(&r.factory.join("commands"), ItemKind::Command, a.clone(), &user, true, c);
    skills_dir(&r.factory.join("skills"), a.clone(), &user, c);
    rule_file(&r.factory.join("AGENTS.md"), &[a], &user, c);
}

/// Project-scoped files inside one workspace.
fn project(ws: &Path, c: &mut Collector) {
    let scope = Scope::Project(ws.to_string_lossy().to_string());
    // Claude Code
    let mcp = ws.join(".mcp.json");
    if let Some(v) = read_json(&mcp) {
        mcp_map(v.get("mcpServers"), AgentKind::ClaudeCode, &scope, &mcp, c);
    }
    for f in ["settings.json", "settings.local.json"] {
        let p = ws.join(".claude").join(f);
        if let Some(v) = read_json(&p) {
            claude_hooks(v.get("hooks"), &scope, &p, c);
        }
    }
    skills_dir(&ws.join(".claude").join("skills"), AgentKind::ClaudeCode, &scope, c);
    md_items(&ws.join(".claude").join("agents"), ItemKind::Subagent, AgentKind::ClaudeCode, &scope, false, c);
    md_items(&ws.join(".claude").join("commands"), ItemKind::Command, AgentKind::ClaudeCode, &scope, true, c);
    rule_file(&ws.join("CLAUDE.md"), &[AgentKind::ClaudeCode], &scope, c);
    rule_file(&ws.join(".claude").join("CLAUDE.md"), &[AgentKind::ClaudeCode], &scope, c);
    // AGENTS.md is read by several agents.
    rule_file(&ws.join("AGENTS.md"), &[AgentKind::Codex, AgentKind::OpenCode, AgentKind::Cursor, AgentKind::Droid], &scope, c);
    // Cursor
    let cm = ws.join(".cursor").join("mcp.json");
    if let Some(v) = read_json(&cm) {
        mcp_map(v.get("mcpServers"), AgentKind::Cursor, &scope, &cm, c);
    }
    rule_dir(&ws.join(".cursor").join("rules"), &[AgentKind::Cursor], &scope, c);
    rule_file(&ws.join(".cursorrules"), &[AgentKind::Cursor], &scope, c);
    skills_dir(&ws.join(".cursor").join("skills"), AgentKind::Cursor, &scope, c);
    md_items(&ws.join(".cursor").join("commands"), ItemKind::Command, AgentKind::Cursor, &scope, true, c);
    // OpenCode
    for f in ["opencode.json", "opencode.jsonc"] {
        opencode_config(&ws.join(f), &scope, c);
    }
    // Gemini
    gemini_settings(&ws.join(".gemini").join("settings.json"), &scope, c);
    rule_file(&ws.join("GEMINI.md"), &[AgentKind::Gemini], &scope, c);
    toml_commands(&ws.join(".gemini").join("commands"), AgentKind::Gemini, &scope, c);
    // Codex
    md_items(&ws.join(".codex").join("prompts"), ItemKind::Command, AgentKind::Codex, &scope, true, c);
    // Droid
    md_items(&ws.join(".factory").join("droids"), ItemKind::Subagent, AgentKind::Droid, &scope, false, c);
}

// ---------------------------------------------------------------------------
// MCP servers
// ---------------------------------------------------------------------------

fn mcp_map(map: Option<&Value>, agent: AgentKind, scope: &Scope, source: &Path, c: &mut Collector) {
    let Some(obj) = map.and_then(|m| m.as_object()) else { return };
    for (name, server) in obj {
        c.push(mcp_item(name, server, agent.clone(), scope, source));
    }
}

fn mcp_item(name: &str, s: &Value, agent: AgentKind, scope: &Scope, source: &Path) -> HarnessItem {
    let mut it = item(ItemKind::Mcp, name, agent, scope, source);
    let url = s.get("url").or_else(|| s.get("httpUrl")).or_else(|| s.get("serverUrl")).and_then(|u| u.as_str());
    let command = s.get("command").and_then(|c| c.as_str());
    let args: Vec<String> = s
        .get("args")
        .and_then(|a| a.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();
    it.detail = match (url, command) {
        (Some(u), _) => Some(redact_url(u)),
        (None, Some(cmd)) => Some(clip(&std::iter::once(cmd.to_string()).chain(args.iter().cloned()).collect::<Vec<_>>().join(" "), 160)),
        _ => None,
    };
    let disabled = s.get("disabled").and_then(|d| d.as_bool()).unwrap_or(false)
        || s.get("enabled").and_then(|d| d.as_bool()).map(|e| !e).unwrap_or(false);
    it.status = if disabled {
        Status::Disabled
    } else if let Some(cmd) = command {
        command_status(cmd)
    } else if url.is_some() {
        let has_token = s.get("headers").and_then(|h| h.as_object()).map(|h| !h.is_empty()).unwrap_or(false)
            || s.get("bearer_token_env_var").is_some()
            || s.get("bearerToken").is_some();
        let wants_auth = s.get("oauth").is_some() || s.get("auth").is_some() || s.get("authProviderType").is_some();
        if wants_auth && !has_token {
            Status::NeedsAuth
        } else if has_token {
            Status::Ok
        } else {
            // Remote servers usually authenticate with OAuth tokens kept by the
            // agent itself; we can't see them, so don't claim either way.
            Status::Unknown
        }
    } else {
        Status::Unknown
    };
    it
}

/// `Ok` when the first token of a shell command resolves, else an error.
fn command_status(cmd: &str) -> Status {
    let first = first_token(cmd);
    if first.is_empty() {
        return Status::Unknown;
    }
    let p = Path::new(&first);
    if p.is_absolute() {
        return if p.exists() { Status::Ok } else { Status::Error(format!("not found: {first}")) };
    }
    if first.contains('$') || first.contains('%') {
        return Status::Unknown;
    }
    if crate::detect::which_program(&first).is_some() {
        Status::Ok
    } else {
        Status::Error(format!("command not found: {first}"))
    }
}

fn first_token(cmd: &str) -> String {
    let t = cmd.trim();
    if let Some(rest) = t.strip_prefix('"') {
        return rest.split('"').next().unwrap_or("").to_string();
    }
    if let Some(rest) = t.strip_prefix('\'') {
        return rest.split('\'').next().unwrap_or("").to_string();
    }
    t.split_whitespace().next().unwrap_or("").to_string()
}

/// Drop query strings (they often carry keys) from URLs we display.
fn redact_url(u: &str) -> String {
    match u.split_once('?') {
        Some((base, _)) => format!("{base}?…"),
        None => u.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Markdown-based items (skills, subagents, commands, rules)
// ---------------------------------------------------------------------------

/// `<dir>/**/SKILL.md` (skipping hidden / trash folders).
fn skills_dir(dir: &Path, agent: AgentKind, scope: &Scope, c: &mut Collector) {
    if !dir.is_dir() {
        return;
    }
    for f in walk(dir, 5, &|p| p.file_name().map(|n| n == "SKILL.md").unwrap_or(false)) {
        let text = std::fs::read_to_string(&f).unwrap_or_default();
        let fm = front_matter(&text);
        let folder = f.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let mut it = item(ItemKind::Skill, fm.get("name").cloned().unwrap_or(folder), agent.clone(), scope, &f);
        it.description = fm.get("description").cloned();
        c.push(it);
    }
}

/// Markdown files in a folder (subagents, commands). Commands keep their
/// relative path as a namespaced name (`/frontend:review`).
fn md_items(dir: &Path, kind: ItemKind, agent: AgentKind, scope: &Scope, recursive: bool, c: &mut Collector) {
    if !dir.is_dir() {
        return;
    }
    let depth = if recursive { 4 } else { 1 };
    for f in walk(dir, depth, &|p| p.extension().map(|e| e == "md").unwrap_or(false)) {
        let text = std::fs::read_to_string(&f).unwrap_or_default();
        let fm = front_matter(&text);
        let rel = f.strip_prefix(dir).unwrap_or(&f).with_extension("");
        let rel = rel.to_string_lossy().replace(['\\', '/'], ":");
        let name = match kind {
            ItemKind::Command => format!("/{rel}"),
            _ => fm.get("name").cloned().unwrap_or(rel),
        };
        // MCP servers a subagent defines for itself (Claude Code's
        // `mcpServers:` front matter): there only while it runs.
        if kind == ItemKind::Subagent {
            for server in agent_mcp_servers(&text) {
                let mut m = item(ItemKind::Mcp, server, agent.clone(), scope, &f);
                m.description = Some(format!("Defined in the {name} agent; there when it runs"));
                c.push(m);
            }
        }
        let mut it = item(kind, name, agent.clone(), scope, &f);
        it.description = fm.get("description").cloned().or_else(|| first_text_line(&text));
        if let Some(m) = fm.get("model") {
            it.detail = Some(format!("model {m}"));
        }
        c.push(it);
    }
}

/// Servers an agent file defines inline under `mcpServers:` in its front
/// matter (`- name:` followed by its settings); bare names only refer to
/// servers configured elsewhere.
fn agent_mcp_servers(text: &str) -> Vec<String> {
    let t = text.trim_start_matches('\u{feff}');
    let Some(rest) = t.strip_prefix("---") else { return Vec::new() };
    let Some(end) = rest.find("\n---") else { return Vec::new() };
    let mut out = Vec::new();
    let mut inside = false;
    // The indent of the list's own entries (deeper ones are their settings).
    let mut level = None;
    for line in rest[..end].lines() {
        if line.trim().is_empty() {
            continue;
        }
        if !line.starts_with([' ', '\t', '-']) {
            inside = line.trim_end() == "mcpServers:";
            level = None;
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if let Some(entry) = line.trim_start().strip_prefix("- ").filter(|_| inside && *level.get_or_insert(indent) == indent) {
            if let Some(name) = entry.trim().strip_suffix(':') {
                out.push(unquote(name));
            }
        }
    }
    out
}

/// Gemini custom commands: `commands/**/*.toml` with `description` / `prompt`.
fn toml_commands(dir: &Path, agent: AgentKind, scope: &Scope, c: &mut Collector) {
    if !dir.is_dir() {
        return;
    }
    for f in walk(dir, 4, &|p| p.extension().map(|e| e == "toml").unwrap_or(false)) {
        let rel = f.strip_prefix(dir).unwrap_or(&f).with_extension("");
        let name = format!("/{}", rel.to_string_lossy().replace(['\\', '/'], ":"));
        let mut it = item(ItemKind::Command, name, agent.clone(), scope, &f);
        if let Some(t) = std::fs::read_to_string(&f).ok().and_then(|t| t.parse::<toml::Table>().ok()) {
            it.description = t.get("description").and_then(|d| d.as_str()).map(|s| s.to_string());
        }
        c.push(it);
    }
}

fn rule_file(path: &Path, agents: &[AgentKind], scope: &Scope, c: &mut Collector) {
    if !path.is_file() {
        return;
    }
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let mut it = item(ItemKind::Rule, name, agents[0].clone(), scope, path);
    it.agents = agents.to_vec();
    it.description = first_text_line(&text);
    it.detail = Some(format!("{} lines", text.lines().count()));
    c.push(it);
}

/// Cursor rules: `rules/**/*.mdc|*.md` with optional `description` /
/// `alwaysApply` front matter.
fn rule_dir(dir: &Path, agents: &[AgentKind], scope: &Scope, c: &mut Collector) {
    if !dir.is_dir() {
        return;
    }
    for f in walk(dir, 4, &|p| p.extension().map(|e| e == "mdc" || e == "md").unwrap_or(false)) {
        let text = std::fs::read_to_string(&f).unwrap_or_default();
        let fm = front_matter(&text);
        let name = f.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let mut it = item(ItemKind::Rule, name, agents[0].clone(), scope, &f);
        it.agents = agents.to_vec();
        it.description = fm.get("description").cloned().filter(|d| !d.is_empty()).or_else(|| first_text_line(&text));
        it.detail = match (fm.get("alwaysApply").map(|s| s.as_str()), fm.get("globs")) {
            (Some("true"), _) => Some("always applied".into()),
            (_, Some(g)) if !g.is_empty() => Some(format!("globs {g}")),
            _ => None,
        };
        c.push(it);
    }
}

fn walk(dir: &Path, depth: usize, keep: &dyn Fn(&Path) -> bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    fn go(d: &Path, depth: usize, keep: &dyn Fn(&Path) -> bool, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            if name.starts_with('.') || name == "node_modules" {
                continue;
            }
            if p.is_dir() {
                if depth > 1 {
                    go(&p, depth - 1, keep, out);
                }
            } else if keep(&p) {
                out.push(p);
            }
            if out.len() > 2000 {
                return;
            }
        }
    }
    go(dir, depth, keep, &mut out);
    out
}

/// Simple YAML front matter (`---` … `---`): `key: value`, quoted values and
/// folded `key: >-` blocks.
pub fn front_matter(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let t = text.trim_start_matches('\u{feff}');
    let Some(rest) = t.strip_prefix("---") else { return out };
    let Some(end) = rest.find("\n---") else { return out };
    let block = &rest[..end];
    let mut current: Option<String> = None;
    for line in block.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let indented = line.starts_with(' ') || line.starts_with('\t');
        if indented {
            if let Some(k) = &current {
                let v = out.entry(k.clone()).or_insert_with(String::new);
                if !v.is_empty() {
                    v.push(' ');
                }
                v.push_str(line.trim());
            }
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            let k = k.trim().to_string();
            let v = v.trim();
            if matches!(v, ">" | ">-" | "|" | "|-" | "") {
                out.insert(k.clone(), String::new());
                current = Some(k);
            } else {
                out.insert(k, unquote(v));
                current = None;
            }
        }
    }
    out.retain(|_, v| !v.is_empty());
    out
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    if v.len() >= 2 && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\''))) {
        v[1..v.len() - 1].to_string()
    } else {
        v.to_string()
    }
}

fn first_text_line(text: &str) -> Option<String> {
    let body = match text.trim_start().strip_prefix("---") {
        Some(rest) => rest.find("\n---").map(|i| &rest[i + 4..]).unwrap_or(text),
        None => text,
    };
    body.lines()
        .map(|l| l.trim().trim_start_matches('#').trim())
        .find(|l| !l.is_empty() && !l.starts_with("<!--") && !l.starts_with("---"))
        .map(|l| clip(l, 140))
}

fn clip(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max - 1).collect::<String>())
    }
}

// ---------------------------------------------------------------------------
// Minimal YAML (Hermes config.yaml) — UNVERIFIED beyond the shapes seen here
// ---------------------------------------------------------------------------

/// Entries of a two-level mapping like:
/// ```yaml
/// mcp_servers:
///   github:
///     command: npx
///     args: ["-y", "@modelcontextprotocol/server-github"]
///     url: https://…
/// ```
fn yaml_section_entries(text: &str, section: &str) -> Vec<(String, Value)> {
    let mut out: Vec<(String, Value)> = Vec::new();
    let mut in_section = false;
    let mut current: Option<(String, serde_json::Map<String, Value>)> = None;
    let mut entry_indent = usize::MAX;
    for line in text.lines() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if indent == 0 {
            if let Some((n, m)) = current.take() {
                out.push((n, Value::Object(m)));
            }
            in_section = line.trim_end() == format!("{section}:");
            entry_indent = usize::MAX;
            continue;
        }
        if !in_section {
            continue;
        }
        let t = line.trim();
        if entry_indent == usize::MAX {
            entry_indent = indent;
        }
        if indent == entry_indent {
            if let Some((n, m)) = current.take() {
                out.push((n, Value::Object(m)));
            }
            if let Some(name) = t.strip_suffix(':') {
                current = Some((unquote(name), serde_json::Map::new()));
            }
        } else if let (Some((_, m)), Some((k, v))) = (current.as_mut(), t.split_once(':')) {
            let k = k.trim().to_string();
            let v = v.trim();
            let val = if v.starts_with('[') {
                Value::Array(v.trim_matches(['[', ']']).split(',').map(unquote).filter(|x| !x.is_empty()).map(Value::String).collect())
            } else if v == "true" || v == "false" {
                Value::Bool(v == "true")
            } else {
                Value::String(unquote(v))
            };
            if !v.is_empty() && k != "env" && k != "headers" {
                m.insert(k, val);
            } else if k == "headers" {
                m.insert(k, Value::Object(serde_json::Map::from_iter([("set".to_string(), Value::Bool(true))])));
            }
        }
    }
    if let Some((n, m)) = current.take() {
        out.push((n, Value::Object(m)));
    }
    out
}

/// Hermes hooks: `hooks:` → `event:` → `- command: …` list items.
fn yaml_hooks(text: &str) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    let mut in_hooks = false;
    for line in text.lines() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let t = line.trim();
        if indent == 0 {
            in_hooks = t == "hooks:";
            continue;
        }
        if !in_hooks {
            continue;
        }
        if let Some(event) = t.strip_suffix(':').filter(|_| !t.starts_with('-')) {
            out.push((event.to_string(), Vec::new()));
        } else if let Some(cmd) = t.strip_prefix("- command:").or_else(|| t.strip_prefix("command:")) {
            if let Some(last) = out.last_mut() {
                last.1.push(unquote(cmd));
            }
        }
    }
    out.retain(|(_, c)| !c.is_empty());
    out
}

// ---------------------------------------------------------------------------
// JSON with comments / trailing commas
// ---------------------------------------------------------------------------

pub fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    parse_jsonc(&text)
}

/// Parse JSONC: strips `//` and `/* */` comments (outside strings) and
/// trailing commas.
pub fn parse_jsonc(text: &str) -> Option<Value> {
    let text = text.trim_start_matches('\u{feff}');
    if let Ok(v) = serde_json::from_str(text) {
        return Some(v);
    }
    let cleaned = strip_comments_utf8(text);
    let no_trailing = remove_trailing_commas(&cleaned);
    serde_json::from_str(&no_trailing).ok()
}

fn strip_comments_utf8(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_str = false;
    while let Some(c) = chars.next() {
        if in_str {
            out.push(c);
            if c == '\\' {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = ' ';
                for n in chars.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
            }
            _ => out.push(c),
        }
    }
    out
}

fn remove_trailing_commas(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_str = false;
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_str {
            out.push(c);
            if c == '\\' && i + 1 < chars.len() {
                out.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if c == '"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_str = true;
        }
        if c == ',' {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if j < chars.len() && (chars[j] == '}' || chars[j] == ']') {
                i += 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

fn toml_to_json(v: &toml::Value) -> Value {
    match v {
        toml::Value::String(s) => Value::String(s.clone()),
        toml::Value::Integer(i) => Value::from(*i),
        toml::Value::Float(f) => Value::from(*f),
        toml::Value::Boolean(b) => Value::Bool(*b),
        toml::Value::Datetime(d) => Value::String(d.to_string()),
        toml::Value::Array(a) => Value::Array(a.iter().map(toml_to_json).collect()),
        toml::Value::Table(t) => Value::Object(t.iter().map(|(k, v)| (k.clone(), toml_to_json(v))).collect()),
    }
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("larrier-harness-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(p: &Path, text: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn roots(base: &Path) -> Roots {
        Roots {
            home: base.to_path_buf(),
            claude: base.join(".claude"),
            claude_json: base.join(".claude.json"),
            codex: base.join(".codex"),
            cursor: base.join(".cursor"),
            opencode: base.join(".config").join("opencode"),
            gemini: base.join(".gemini"),
            hermes: vec![base.join("hermes")],
            factory: base.join(".factory"),
        }
    }

    fn find<'a>(items: &'a [HarnessItem], kind: ItemKind, name: &str) -> &'a HarnessItem {
        items.iter().find(|i| i.kind == kind && i.name == name).unwrap_or_else(|| panic!("missing {kind:?} {name}: {items:#?}"))
    }

    #[test]
    fn jsonc() {
        let v = parse_jsonc("{\n // c\n \"a\": \"http://x\", /* b */ \"b\": [1,2,],\n}").unwrap();
        assert_eq!(v["a"], "http://x");
        assert_eq!(v["b"].as_array().unwrap().len(), 2);
        let v = parse_jsonc("{\"é\": \"// not a comment\",}").unwrap();
        assert_eq!(v["é"], "// not a comment");
    }

    #[test]
    fn agent_defined_mcp_servers() {
        let text = "---\nname: site-builder\nmcpServers:\n  - site_builder:\n      type: stdio\n      command: \"node\"\n      args:\n        - \"server.js\"\n        - --flag:\n  - github\n  - \"docs\":\n      type: http\nmodel: inherit\n---\nbody";
        assert_eq!(agent_mcp_servers(text), ["site_builder", "docs"], "inline definitions, not references or their args");
        assert!(agent_mcp_servers("---\nname: x\n---\n").is_empty());
    }

    #[test]
    fn front_matter_folded() {
        let fm = front_matter("---\nname: worker\ndescription: >-\n  General purpose\n  worker.\nmodel: inherit\n---\nbody");
        assert_eq!(fm["name"], "worker");
        assert_eq!(fm["description"], "General purpose worker.");
        assert_eq!(fm["model"], "inherit");
    }

    #[test]
    fn discovers_every_format() {
        let d = tmp();
        let r = roots(&d);
        // Claude
        write(&r.claude_json, r#"{"mcpServers":{"gh":{"command":"definitely-not-a-real-binary-xyz","args":["serve"],"env":{"TOKEN":"secret"}}},"projects":{"D:/p":{"mcpServers":{"remote":{"type":"http","url":"https://mcp.example.com/sse?key=abc","oauth":{}}}}}}"#);
        write(
            &r.claude.join("settings.json"),
            r#"{"enabledPlugins":{"lsp@official":true},"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"definitely-missing-hook.exe --x"}]}]}}"#,
        );
        write(&r.claude.join("skills").join("pdf").join("SKILL.md"), "---\nname: pdf\ndescription: Work with PDFs\n---\n");
        write(&r.claude.join("skills").join(".trash").join("old").join("SKILL.md"), "---\nname: old\n---\n");
        write(&r.claude.join("agents").join("reviewer.md"), "---\nname: reviewer\ndescription: Reviews code\n---\n");
        write(&r.claude.join("commands").join("git").join("ship.md"), "Ship the branch\n");
        let plugin_dir = d.join("plugcache").join("lsp");
        write(&plugin_dir.join(".claude-plugin").join("plugin.json"), r#"{"name":"lsp","description":"Language server"}"#);
        write(
            &r.claude.join("plugins").join("installed_plugins.json"),
            &format!(r#"{{"version":2,"plugins":{{"lsp@official":[{{"scope":"user","installPath":{},"version":"1.0.0"}}]}}}}"#, serde_json::to_string(&plugin_dir).unwrap()),
        );
        // Codex
        write(
            &r.codex.join("config.toml"),
            "[mcp_servers.docs]\ncommand = \"definitely-not-a-real-binary-xyz\"\nargs = [\"serve\"]\n\n[plugins.\"github@openai-curated\"]\nenabled = false\n",
        );
        write(&r.codex.join("AGENTS.md"), "# House rules\nBe nice\n");
        write(&r.codex.join("prompts").join("fix.md"), "Fix the bug\n");
        // Cursor
        write(&r.cursor.join("mcp.json"), r#"{"mcpServers":{"gh":{"command":"definitely-not-a-real-binary-xyz","args":["serve"]},"runpod":{"url":"https://mcp.runpod.io"}}}"#);
        write(&r.cursor.join("commands").join("verify.md"), "Verify the UI\n");
        // OpenCode (jsonc with trailing comma)
        write(
            &r.opencode.join("opencode.jsonc"),
            "{\n // comment\n \"mcp\": {\"fs\": {\"type\": \"local\", \"command\": [\"npx\", \"-y\", \"fs-mcp\"], \"enabled\": false},},\n \"agent\": {\"plan\": {\"description\": \"Plans\"}},\n}",
        );
        // Gemini
        write(&r.gemini.join("settings.json"), r#"{"mcpServers":{"web":{"httpUrl":"https://web.example.com/mcp","headers":{"Authorization":"Bearer x"}}}}"#);
        write(&r.gemini.join("commands").join("review.toml"), "description = \"Review code\"\nprompt = \"Review\"\n");
        write(&r.gemini.join("extensions").join("ext").join("gemini-extension.json"), r#"{"name":"ext","version":"1.2.0"}"#);
        // Hermes
        write(
            &r.hermes[0].join("config.yaml"),
            "model:\n  default: x\nmcp_servers:\n  github:\n    command: definitely-not-a-real-binary-xyz\n    args: [\"-y\", \"gh\"]\nhooks:\n  on_session_start:\n    - command: definitely-missing-hook\n      timeout: 5\n",
        );
        write(&r.hermes[0].join("skills").join("notes").join("SKILL.md"), "---\nname: apple-notes\ndescription: Notes\n---\n");
        // Droid
        write(&r.factory.join("droids").join("worker.md"), "---\nname: worker\ndescription: >-\n  General worker.\n---\n");
        // Project
        let ws = d.join("ws");
        write(&ws.join("AGENTS.md"), "Project rules\n");
        write(&ws.join(".cursor").join("rules").join("style.mdc"), "---\ndescription: Style\nalwaysApply: true\n---\nUse tabs\n");
        write(&ws.join(".mcp.json"), r#"{"mcpServers":{"local":{"command":"definitely-not-a-real-binary-xyz"}}}"#);

        let items = discover_in(&r, std::slice::from_ref(&ws));

        // Same MCP (name + command) in Claude and Cursor → one row, two agents.
        let gh = find(&items, ItemKind::Mcp, "gh");
        assert!(gh.agents.contains(&AgentKind::ClaudeCode) && gh.agents.contains(&AgentKind::Cursor));
        assert!(matches!(gh.status, Status::Error(_)));
        assert!(!gh.detail.clone().unwrap().contains("secret"));
        let remote = find(&items, ItemKind::Mcp, "remote");
        assert_eq!(remote.status, Status::NeedsAuth);
        assert_eq!(remote.scope, Scope::Project("D:/p".into()));
        assert!(!remote.detail.clone().unwrap().contains("abc"));
        assert_eq!(find(&items, ItemKind::Mcp, "runpod").status, Status::Unknown);
        assert_eq!(find(&items, ItemKind::Mcp, "web").status, Status::Ok);
        assert_eq!(find(&items, ItemKind::Mcp, "fs").status, Status::Disabled);
        assert_eq!(find(&items, ItemKind::Mcp, "docs").agents, vec![AgentKind::Codex]);
        assert!(find(&items, ItemKind::Mcp, "github").agents.contains(&AgentKind::Hermes));

        let hook = find(&items, ItemKind::Hook, "PreToolUse");
        assert!(hook.detail.clone().unwrap().contains("matcher Bash"));
        assert!(matches!(hook.status, Status::Error(_)));
        find(&items, ItemKind::Hook, "on_session_start");

        assert_eq!(find(&items, ItemKind::Skill, "pdf").description.as_deref(), Some("Work with PDFs"));
        assert!(items.iter().all(|i| i.name != "old"), "trash skipped");
        find(&items, ItemKind::Skill, "apple-notes");
        find(&items, ItemKind::Subagent, "reviewer");
        assert_eq!(find(&items, ItemKind::Subagent, "worker").description.as_deref(), Some("General worker."));
        find(&items, ItemKind::Subagent, "plan");
        find(&items, ItemKind::Command, "/git:ship");
        find(&items, ItemKind::Command, "/fix");
        find(&items, ItemKind::Command, "/verify");
        assert_eq!(find(&items, ItemKind::Command, "/review").description.as_deref(), Some("Review code"));

        let lsp = find(&items, ItemKind::Plugin, "lsp");
        assert_eq!(lsp.status, Status::Ok);
        assert_eq!(lsp.description.as_deref(), Some("Language server"));
        assert_eq!(find(&items, ItemKind::Plugin, "github").status, Status::Disabled);
        find(&items, ItemKind::Plugin, "ext");

        let rules: Vec<_> = items.iter().filter(|i| i.kind == ItemKind::Rule).collect();
        assert!(rules.iter().any(|r| r.name == "AGENTS.md" && r.scope == Scope::User));
        let proj_agents = rules.iter().find(|r| r.name == "AGENTS.md" && r.scope != Scope::User).unwrap();
        assert!(proj_agents.agents.len() >= 3);
        assert_eq!(rules.iter().find(|r| r.name == "style").unwrap().detail.as_deref(), Some("always applied"));
        find(&items, ItemKind::Mcp, "local");

        // Needs-attention items sort first.
        assert!(items[0].status.needs_attention());
        let _ = std::fs::remove_dir_all(d);
    }

    /// Read-only scan of this machine: `cargo test -p larrier-agents harness_smoke -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn harness_smoke() {
        let items = discover(&[]);
        let mut counts: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
        for i in &items {
            for a in &i.agents {
                counts.entry((a.id(), i.kind.plural().to_string())).or_default().push(format!(
                    "{}{}",
                    i.name,
                    match &i.status {
                        Status::Ok => "",
                        Status::NeedsAuth => " [needs auth]",
                        Status::Error(_) => " [error]",
                        Status::Disabled => " [disabled]",
                        Status::Unknown => " [?]",
                    }
                ));
            }
        }
        for ((agent, kind), names) in counts {
            println!("{agent:12} {kind:10} {:3}  {}", names.len(), names.iter().take(12).cloned().collect::<Vec<_>>().join(", "));
        }
        println!("total {}", items.len());
    }
}
