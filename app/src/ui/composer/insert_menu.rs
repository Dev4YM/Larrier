//! The @ and / menu above the composer, for the text before the caret.
//!
//! `@` starts on the kinds of context there are (files, chats, the
//! harnesses' sessions on this computer, the agent's own agents, the diff)
//! and `/` on what can be done (modes, the model,
//! commands, skills). A kind opens as a page of the same menu, with the
//! way back at its top; typing searches every kind at once.

use dioxus::prelude::*;
use lary_agents::harness::ItemKind;
use lary_core::AgentKind;

use super::{harness_items, mode_icon, pick_rows, strip_tokens};
use crate::icons::I;
use crate::state::App;
use crate::ui::mentions::MentionChip;

/// A page opened from the menu's first one.
#[derive(Clone, PartialEq, Debug)]
pub enum Level {
    /// One kind of things: Files, Chats, Model …
    Kind(&'static str),
    /// A folder of the project, browsed from Files ("src/ui/").
    Dir(String),
}

#[derive(Clone, PartialEq)]
pub enum SuggAct {
    /// Turn the token being typed into this object.
    Chip(MentionChip),
    /// Run a Lary command.
    Lary(&'static str),
    /// Run the new chat as this agent of the agent's own.
    RunAs(String, AgentKind),
    /// Open a page of the menu.
    Open(Level),
    /// Use this model (None: the default).
    Model(Option<String>),
    /// Think this hard (None: the model's default).
    Effort(Option<String>),
    /// A harness's own session, sent as context (read when picked).
    Native(Box<lary_formats::NativeSessionInfo>),
}

#[derive(Clone, PartialEq)]
pub struct Sugg {
    /// Heading in search results.
    pub group: &'static str,
    pub icon: I,
    pub logo: Option<&'static str>,
    pub label: String,
    /// Muted, after the label.
    pub sub: String,
    /// A second line under the label (the kinds on @'s first page).
    pub desc: String,
    /// The command it runs, at the right ("/plan").
    pub cmd: String,
    /// The current choice (a model, an effort).
    pub on: bool,
    pub act: SuggAct,
}

impl Sugg {
    fn new(group: &'static str, icon: I, label: impl Into<String>, act: SuggAct) -> Sugg {
        Sugg { group, icon, logo: None, label: label.into(), sub: String::new(), desc: String::new(), cmd: String::new(), on: false, act }
    }
    fn logo(mut self, l: Option<&'static str>) -> Sugg {
        self.logo = l;
        self
    }
    fn sub(mut self, s: impl Into<String>) -> Sugg {
        self.sub = s.into();
        self
    }
    fn desc(mut self, s: &str) -> Sugg {
        self.desc = s.to_string();
        self
    }
    fn cmd(mut self, c: &str) -> Sugg {
        self.cmd = c.to_string();
        self
    }
    fn on(mut self, on: bool) -> Sugg {
        self.on = on;
        self
    }
    /// Opens a page rather than doing something.
    pub fn opens(&self) -> bool {
        matches!(self.act, SuggAct::Open(_))
    }
}

/// What the menu needs to know about the composer.
pub struct Ctx<'a> {
    pub app: &'a App,
    /// The project's files (relative, forward slashes).
    pub files: &'a [String],
    /// A new chat (the start screen), not one under way.
    pub home: bool,
    pub agent: &'a AgentKind,
    /// This chat, left out of the chats to bring in.
    pub current: Option<&'a str>,
    /// Hasn't started: it can still run as one of the agent's agents.
    pub fresh: bool,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    /// The agent has a plan mode, and is in it.
    pub plan: bool,
    pub in_plan: bool,
    /// A mode other than the agent's default is chosen.
    pub mode_set: bool,
    pub has_ask: bool,
}

/// The menu as it is: the page's title (None on the first page) and rows.
pub struct Page {
    pub title: Option<String>,
    pub items: Vec<Sugg>,
}

/// The token being typed just before the caret: `@…` anywhere, or `/…`
/// first in the message. Its trigger, where it starts, and what follows it.
pub fn token(before: &str) -> Option<(char, usize, String)> {
    if let Some(q) = mention_query(before) {
        return Some(('@', before.len() - q.len() - 1, q));
    }
    slash_query(before).map(|q| ('/', before.len() - q.len() - 1, q))
}

/// Text after a trailing `@` token, if the caret is inside one.
fn mention_query(text: &str) -> Option<String> {
    if text.ends_with(char::is_whitespace) {
        return None;
    }
    let token = text.rsplit(|c: char| c.is_whitespace() || c == crate::ui::mentions::CLOSE).next()?;
    let q = token.strip_prefix('@')?;
    (!q.contains('@')).then(|| q.to_string())
}

/// `/command` typed first (after any objects).
fn slash_query(text: &str) -> Option<String> {
    let token = text.rsplit(|c: char| c.is_whitespace() || c == crate::ui::mentions::CLOSE).next()?;
    let before = &text[..text.len() - token.len()];
    let t = token.strip_prefix('/')?;
    strip_tokens(before).trim().is_empty().then(|| t.to_string())
}

pub fn page(ctx: &Ctx, trigger: char, q: &str, stack: &[Level]) -> Page {
    let ql = q.to_lowercase();
    match (trigger, stack.last()) {
        ('@', None) if ql.is_empty() => Page { title: None, items: at_kinds(ctx) },
        ('@', None) => Page { title: None, items: at_search(ctx, &ql) },
        (_, None) if ql.is_empty() => Page { title: None, items: slash_first(ctx) },
        (_, None) => Page { title: None, items: slash_search(ctx, &ql) },
        (_, Some(Level::Dir(dir))) => Page { title: Some(dir.clone()), items: files_page(ctx, dir, &ql) },
        (_, Some(Level::Kind(k))) => {
            let items = match *k {
                "Files" => files_page(ctx, "", &ql),
                "Chats" => chats(ctx, &ql, 60),
                "Sessions" => harness_sessions(ctx, &ql, 120),
                "Run as" => run_as(ctx, &ql),
                "Subagents" => subagents(ctx, &ql),
                "Model" => models(ctx, &ql),
                "Reasoning" => efforts(ctx),
                "Commands" => commands(ctx, &ql, "Commands"),
                "Skills" => commands(ctx, &ql, "Skills"),
                "MCP prompts" => commands(ctx, &ql, "MCP prompts"),
                _ => Vec::new(),
            };
            Page { title: Some(k.to_string()), items }
        }
    }
}

// ------------------------------------------------------------------ @

fn at_kinds(ctx: &Ctx) -> Vec<Sugg> {
    let mut out = Vec::new();
    if !ctx.files.is_empty() {
        out.push(Sugg::new("", I::Folder, "Files", SuggAct::Open(Level::Kind("Files"))).desc("Files and folders in this project"));
    }
    if !chats(ctx, "", 1).is_empty() {
        out.push(Sugg::new("", I::Message, "Chats", SuggAct::Open(Level::Kind("Chats"))).desc("Another chat's conversation, as context"));
    }
    if sessions_offered(ctx) {
        // Found in the background, ready by the time the page opens.
        crate::native_sessions::refresh();
        out.push(Sugg::new("", I::History, "Sessions", SuggAct::Open(Level::Kind("Sessions"))).desc("A harness session on this computer, as context"));
    }
    if !run_as(ctx, "").is_empty() {
        out.push(Sugg::new("", I::Orbit, "Run as", SuggAct::Open(Level::Kind("Run as"))).desc(&format!("Run this chat as one of {}'s agents", ctx.agent.display_name())));
    }
    if !subagents(ctx, "").is_empty() {
        out.push(Sugg::new("", I::Bot, "Subagents", SuggAct::Open(Level::Kind("Subagents"))).desc("Agents this chat can hand work to"));
    }
    if !ctx.files.is_empty() {
        out.push(Sugg::new("", I::Diff, "Git diff", SuggAct::Chip(MentionChip::git_diff())).desc("The uncommitted changes"));
    }
    out
}

/// Typing after @: every kind at once.
fn at_search(ctx: &Ctx, ql: &str) -> Vec<Sugg> {
    let mut out = search_files(ctx, "", ql, 6, 3);
    out.extend(chats(ctx, ql, 4));
    out.extend(harness_sessions(ctx, ql, 4));
    out.extend(run_as(ctx, ql));
    out.extend(subagents(ctx, ql).into_iter().take(6));
    if "git".starts_with(ql) || ql.starts_with("git") || "diff".starts_with(ql) {
        out.push(Sugg::new("Context", I::Diff, "Git diff", SuggAct::Chip(MentionChip::git_diff())).sub("uncommitted changes"));
    }
    out
}

/// A folder of the project: its folders (each opens), then its files, and
/// a row to add the folder itself. Typing searches everything under it.
fn files_page(ctx: &Ctx, dir: &str, ql: &str) -> Vec<Sugg> {
    if !ql.is_empty() {
        return search_files(ctx, dir, ql, 40, 8);
    }
    let mut dirs = std::collections::BTreeSet::new();
    let mut files: Vec<&str> = Vec::new();
    for f in ctx.files.iter().filter_map(|f| f.strip_prefix(dir)) {
        match f.split_once('/') {
            Some((d, _)) => {
                dirs.insert(d);
            }
            None => files.push(f),
        }
    }
    files.sort_by_key(|f| f.to_lowercase());
    let mut out = Vec::new();
    if !dir.is_empty() {
        out.push(Sugg::new("", I::FolderPlus, "Add this folder", SuggAct::Chip(MentionChip::path(dir, true))).sub(dir.to_string()));
    }
    for d in dirs.into_iter().take(200) {
        out.push(Sugg::new("", I::Folder, d, SuggAct::Open(Level::Dir(format!("{dir}{d}/")))));
    }
    for f in files.into_iter().take(300) {
        out.push(Sugg::new("", I::File, f, SuggAct::Chip(MentionChip::path(&format!("{dir}{f}"), false))));
    }
    out
}

/// Files, then folders, under `dir` whose path has `ql`; names that start
/// with it first.
fn search_files(ctx: &Ctx, dir: &str, ql: &str, n_files: usize, n_dirs: usize) -> Vec<Sugg> {
    let mut out = Vec::new();
    let under = || ctx.files.iter().filter(move |p| p.starts_with(dir));
    let mut hits: Vec<&String> = under().filter(|p| p[dir.len()..].to_lowercase().contains(ql)).take(400).collect();
    hits.sort_by_key(|p| (!p.rsplit('/').next().unwrap_or(p).to_lowercase().starts_with(ql), p.len()));
    for p in hits.into_iter().take(n_files) {
        let (d, name) = match p.rsplit_once('/') {
            Some((d, n)) => (d.to_string(), n.to_string()),
            None => (String::new(), p.clone()),
        };
        out.push(Sugg::new("Files", I::File, name, SuggAct::Chip(MentionChip::path(p, false))).sub(d));
    }
    let mut dirs: Vec<String> = under()
        .filter_map(|p| p.rsplit_once('/').map(|(d, _)| d.to_string()))
        .filter(|d| d.len() > dir.len() && d[dir.len()..].to_lowercase().contains(ql))
        .collect();
    dirs.sort();
    dirs.dedup();
    dirs.sort_by_key(|d| d.len());
    for d in dirs.into_iter().take(n_dirs) {
        out.push(Sugg::new("Folders", I::Folder, format!("{d}/"), SuggAct::Chip(MentionChip::path(&format!("{d}/"), true))));
    }
    out
}

fn chats(ctx: &Ctx, ql: &str, n: usize) -> Vec<Sugg> {
    let sessions = ctx.app.sessions.read();
    sessions
        .iter()
        .filter(|s| Some(s.id.as_str()) != ctx.current && !s.archived && (ql.is_empty() || s.title.to_lowercase().contains(ql)))
        .take(n)
        .map(|s| {
            Sugg::new("Chats", I::Message, s.title.clone(), SuggAct::Chip(MentionChip::agent(s)))
                .logo(crate::logos::agent_logo(&s.agent))
                .sub(format!("{} · {}", s.agent.display_name(), crate::state::rel_time(s.updated_at)))
        })
        .collect()
}

/// Whether @ offers the harnesses' sessions (Settings → Sessions).
fn sessions_offered(ctx: &Ctx) -> bool {
    ctx.app.config.peek().settings.sessions_in_mentions
}

/// The harnesses' sessions on this computer that aren't Lary's own chats
/// (those are under Chats), newest first.
fn harness_sessions(ctx: &Ctx, ql: &str, n: usize) -> Vec<Sugg> {
    if !sessions_offered(ctx) {
        return Vec::new();
    }
    crate::native_sessions::refresh();
    let found = crate::native_sessions::FOUND.read();
    let Some(list) = found.as_ref() else { return Vec::new() };
    let sessions = ctx.app.sessions.read();
    list.iter()
        .filter(|i| crate::native_sessions::matches(i, ql) && crate::native_sessions::in_lary(i, &sessions).is_none())
        .take(n)
        .map(|i| {
            Sugg::new("Sessions", I::History, crate::native_sessions::title(i), SuggAct::Native(Box::new(i.clone())))
                .logo(crate::logos::agent_logo(&i.agent))
                .sub(crate::native_sessions::detail(i))
        })
        .collect()
}

/// Claude Code's agents (its own and those found on disk): name, description.
fn agent_agents(ctx: &Ctx) -> Vec<(String, String)> {
    if *ctx.agent != AgentKind::ClaudeCode {
        return Vec::new();
    }
    let mut subs: Vec<(String, String)> = ctx.app.agent_models(ctx.agent).map(|m| m.subagents.iter().map(|s| (s.name.clone(), s.description.clone().unwrap_or_default())).collect()).unwrap_or_default();
    for it in harness_items(ctx.app).into_iter().filter(|it| it.kind == ItemKind::Subagent && it.agents.contains(ctx.agent)) {
        if !subs.iter().any(|(n, _)| *n == it.name) {
            subs.push((it.name.clone(), it.description.clone().unwrap_or_default()));
        }
    }
    subs
}

fn matches(name: &str, ql: &str) -> bool {
    ql.is_empty() || name.to_lowercase().contains(ql) || "agent".starts_with(ql)
}

/// A chat that hasn't started can run as one of the agent's agents
/// (Claude Code's own built-ins aren't agents to run as).
fn run_as(ctx: &Ctx, ql: &str) -> Vec<Sugg> {
    if !ctx.fresh {
        return Vec::new();
    }
    let logo = crate::logos::agent_logo(ctx.agent);
    agent_agents(ctx)
        .into_iter()
        .filter(|(n, _)| !matches!(n.as_str(), "general-purpose" | "Explore" | "Plan" | "statusline-setup" | "claude-code-guide" | "claude") && matches(n, ql))
        .map(|(name, desc)| Sugg::new("Run as", I::Orbit, name.clone(), SuggAct::RunAs(name, ctx.agent.clone())).logo(logo).sub(desc.chars().take(80).collect::<String>()))
        .collect()
}

fn subagents(ctx: &Ctx, ql: &str) -> Vec<Sugg> {
    let logo = crate::logos::agent_logo(ctx.agent);
    agent_agents(ctx)
        .into_iter()
        .filter(|(n, _)| matches(n, ql))
        .map(|(name, desc)| {
            let sub: String = desc.chars().take(80).collect();
            Sugg::new("Subagents", I::Bot, format!("@agent-{name}"), SuggAct::Chip(MentionChip::subagent(&name, &sub))).logo(logo).sub(sub)
        })
        .collect()
}

// ------------------------------------------------------------------ /

/// Lary's own commands: the command, its name, its icon.
const SLASH: &[(&str, &str, I)] = &[
    ("/plan", "Plan mode", I::List),
    ("/agent", "Agent mode", I::Code),
    ("/ask", "Ask mode", I::Eye),
    ("/new", "New chat", I::Plus),
    ("/fork", "Fork this chat", I::Fork),
    ("/terminal", "Open in terminal", I::Terminal),
    ("/export", "Export", I::Download),
    ("/worktree", "Run in a worktree", I::GitBranch),
    ("/local", "Run in the folder", I::Folder),
    ("/mcp", "MCP servers", I::Server),
    ("/usage", "Usage", I::Gauge),
    ("/settings", "Settings", I::Settings),
];

/// Which of Lary's commands make sense here.
fn lary_commands(ctx: &Ctx) -> Vec<Sugg> {
    SLASH
        .iter()
        .filter(|(c, ..)| match *c {
            "/plan" => ctx.plan,
            "/agent" => ctx.mode_set,
            "/ask" => ctx.has_ask,
            "/fork" | "/terminal" | "/export" => !ctx.home,
            "/worktree" | "/local" => ctx.home,
            _ => true,
        })
        .map(|&(c, label, icon)| {
            let icon = if c == "/plan" { mode_icon("plan") } else { icon };
            Sugg::new("Lary", icon, label, SuggAct::Lary(c)).cmd(c).on(c == "/plan" && ctx.in_plan)
        })
        .collect()
}

/// The model in use and its effort, as the picker names them.
fn now(ctx: &Ctx) -> crate::agent_models::Resolved {
    crate::agent_models::resolve(ctx.app, ctx.agent, ctx.model, ctx.effort)
}

fn effort_choices(ctx: &Ctx) -> Vec<String> {
    let r = now(ctx);
    let Some(id) = r.family.clone().or(r.model.clone()) else { return Vec::new() };
    crate::agent_models::choices(ctx.app, ctx.agent).into_iter().find(|c| c.id == id).map(|c| c.efforts).unwrap_or_default()
}

/// The agent's own commands, skills and MCP prompts: (group, name, description).
fn agent_commands(ctx: &Ctx) -> Vec<(&'static str, String, String)> {
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for c in ctx.app.agent_models(ctx.agent).map(|m| m.commands).unwrap_or_default() {
        if !seen.insert(c.name.clone()) {
            continue;
        }
        let desc = c.hint.clone().map(|h| format!("{h} · ")).unwrap_or_default() + &c.description.clone().unwrap_or_default();
        out.push((if c.name.starts_with("mcp__") { "MCP prompts" } else { "Commands" }, c.name.clone(), desc));
    }
    for it in harness_items(ctx.app).into_iter().filter(|it| matches!(it.kind, ItemKind::Command | ItemKind::Skill) && it.agents.contains(ctx.agent)) {
        let name = it.name.trim_start_matches('/').to_string();
        if !seen.insert(name.clone()) {
            continue;
        }
        out.push((if it.kind == ItemKind::Skill { "Skills" } else { "Commands" }, name, it.description.clone().unwrap_or_default()));
    }
    out
}

fn command_row(ctx: &Ctx, group: &'static str, name: &str, desc: &str) -> Sugg {
    let desc: String = desc.chars().take(80).collect();
    let (icon, logo) = match group {
        "Skills" => (I::Sparkle, None),
        "MCP prompts" => (I::Plug, None),
        _ => (I::Slash, crate::logos::agent_logo(ctx.agent)),
    };
    Sugg::new(group, icon, format!("/{name}"), SuggAct::Chip(MentionChip::skill(name, group == "Skills", &desc))).logo(logo).sub(desc)
}

fn slash_first(ctx: &Ctx) -> Vec<Sugg> {
    let mut cmds = lary_commands(ctx);
    let mut out: Vec<Sugg> = Vec::new();
    // Modes first, then the model and how hard it thinks, then the rest.
    let modes = cmds.iter().take_while(|s| matches!(s.act, SuggAct::Lary("/plan" | "/agent" | "/ask"))).count();
    out.extend(cmds.drain(..modes));
    let r = now(ctx);
    let model_now = r.family.clone().or(r.model.clone()).map(|m| crate::agent_models::short_name(ctx.app, ctx.agent, &m)).unwrap_or_default();
    out.push(Sugg::new("", I::Box, "Model", SuggAct::Open(Level::Kind("Model"))).sub(model_now));
    if !effort_choices(ctx).is_empty() {
        let e = r.effort.as_deref().map(lary_agents::models::effort_label).unwrap_or_else(|| "Default".into());
        out.push(Sugg::new("", I::Brain, "Reasoning", SuggAct::Open(Level::Kind("Reasoning"))).sub(e));
    }
    out.extend(cmds);
    let theirs = agent_commands(ctx);
    for (kind, icon, logo) in [("Commands", I::Slash, crate::logos::agent_logo(ctx.agent)), ("Skills", I::Sparkle, None), ("MCP prompts", I::Plug, None)] {
        if theirs.iter().any(|(g, ..)| *g == kind) {
            out.push(Sugg::new("", icon, kind, SuggAct::Open(Level::Kind(kind))).logo(logo));
        }
    }
    out
}

/// Typing after /: Lary's commands, models, then the agent's commands,
/// skills and MCP prompts.
fn slash_search(ctx: &Ctx, ql: &str) -> Vec<Sugg> {
    let mut out: Vec<Sugg> = lary_commands(ctx).into_iter().filter(|s| s.cmd[1..].starts_with(ql) || s.label.to_lowercase().contains(ql)).collect();
    if "reasoning".starts_with(ql) || "effort".starts_with(ql) {
        out.extend(slash_first(ctx).into_iter().filter(|s| s.act == SuggAct::Open(Level::Kind("Reasoning"))).map(|s| Sugg { group: "Lary", ..s }));
    }
    out.extend(models(ctx, ql).into_iter().filter(|s| s.label.to_lowercase().contains(ql)).take(6).map(|s| Sugg { group: "Models", ..s }));
    let mut theirs: Vec<Sugg> = agent_commands(ctx).iter().filter(|(_, n, _)| n.to_lowercase().contains(ql)).take(60).map(|&(g, ref n, ref d)| command_row(ctx, g, n, d)).collect();
    theirs.sort_by_key(|s| match s.group {
        "Commands" => 0,
        "Skills" => 1,
        _ => 2,
    });
    out.extend(theirs);
    out
}

fn models(ctx: &Ctx, ql: &str) -> Vec<Sugg> {
    // Shown: read again if the list is old, as the picker does (after this
    // render, not during it).
    let (app, agent) = (*ctx.app, ctx.agent.clone());
    spawn(async move { app.refresh_models_for(&agent, None) });
    let r = now(ctx);
    let current = r.family.clone().or(r.model.clone());
    pick_rows(ctx.app, ctx.agent, None, !ql.is_empty())
        .into_iter()
        .filter(|row| ql.is_empty() || row.label.to_lowercase().contains(ql) || row.id.as_deref().is_some_and(|i| i.to_lowercase().contains(ql)))
        .map(|row| {
            let on = match &row.id {
                None => ctx.model.is_none(),
                Some(id) => ctx.model.is_some() && current.as_deref() == Some(id.as_str()),
            };
            let logo = row.logo_id.as_deref().and_then(crate::logos::model_logo);
            Sugg::new("Models", I::Box, row.label.clone(), SuggAct::Model(row.id.clone())).logo(logo).sub(row.note.clone()).on(on)
        })
        .collect()
}

fn efforts(ctx: &Ctx) -> Vec<Sugg> {
    let mut out = vec![Sugg::new("", I::Gauge, "Default", SuggAct::Effort(None)).on(ctx.effort.is_none())];
    for e in effort_choices(ctx) {
        let on = ctx.effort == Some(e.as_str());
        out.push(Sugg::new("", I::Gauge, lary_agents::models::effort_label(&e), SuggAct::Effort(Some(e))).on(on));
    }
    out
}

fn commands(ctx: &Ctx, ql: &str, group: &'static str) -> Vec<Sugg> {
    agent_commands(ctx).iter().filter(|(g, n, _)| *g == group && (ql.is_empty() || n.to_lowercase().contains(ql))).map(|&(g, ref n, ref d)| command_row(ctx, g, n, d)).collect()
}

#[cfg(test)]
mod tests {
    use super::{mention_query, slash_query, token};

    #[test]
    fn mentions() {
        assert_eq!(mention_query("look at @src/ma").as_deref(), Some("src/ma"));
        assert_eq!(mention_query("look at @src/main.rs "), None);
        assert_eq!(slash_query("/pl").as_deref(), Some("pl"));
        assert_eq!(slash_query("/plan this"), None);
    }

    #[test]
    fn the_token_is_the_one_before_the_caret() {
        // "@" typed on a new first line, the text below it after the caret.
        assert_eq!(token("@"), Some(('@', 0, String::new())));
        assert_eq!(token("fix\n@sr"), Some(('@', 4, "sr".to_string())));
        assert_eq!(token("/mo"), Some(('/', 0, "mo".to_string())));
        assert_eq!(token("see /mo"), None);
        assert_eq!(token("done "), None);
    }
}
