//! Each agent's own models, defaults and providers (read by
//! `lary_agents::models`), per account: what an agent offers and what its
//! Default runs depend on the account (a subscription's plan, an API), so
//! they're read the way that account's chats run and kept by account.
//! Cached on disk so the picker has them at once; re-read at start, when the
//! picker or the agent's settings open and the list is a few minutes old,
//! and every few hours.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use lary_agents::models::{self, AgentModels};
use lary_core::AgentKind;

use crate::state::{store, App};

/// How old a CLI model listing may get before it is refreshed in the
/// background.
const LISTING_TTL_HOURS: i64 = 6;

/// How old a listing may be when the picker opens before it's read again.
const PICKER_TTL_MINUTES: i64 = 10;

/// Where an agent's models are kept: by the account they were read for
/// (`None`: the agent's default account), or by the agent when it has no
/// accounts.
pub fn models_key(cfg: &lary_core::Config, kind: &AgentKind, account: Option<&str>) -> String {
    match cfg.account_for(kind, account) {
        Some(a) => format!("{}@{}", kind.id(), a.id),
        None => kind.id().to_string(),
    }
}

fn cache_path() -> PathBuf {
    store().root().join("cache").join("agent-models.json")
}

/// The cached models, by agent id.
pub fn load() -> HashMap<String, AgentModels> {
    std::fs::read_to_string(cache_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(all: &HashMap<String, AgentModels>) {
    let p = cache_path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(t) = serde_json::to_string(all) {
        let _ = std::fs::write(p, t);
    }
}

/// Refresh every installed agent once detection has found them.
pub fn start(app: App) {
    spawn_forever(async move {
        for _ in 0..60 {
            if !app.detected.peek().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        app.refresh_agent_models(None, false);
        loop {
            tokio::time::sleep(Duration::from_secs(LISTING_TTL_HOURS as u64 * 3600)).await;
            app.refresh_agent_models(None, false);
        }
    });
}

impl App {
    /// What `kind` offers and prefers for its default account, as last read.
    pub fn agent_models(&self, kind: &AgentKind) -> Option<AgentModels> {
        self.agent_models_for(kind, None)
    }

    /// What `kind` offers and prefers for `account` (None: its default), as
    /// last read for it. Until it has been, the list read for the agent
    /// before Lary kept them by account stands in, without its Default
    /// (that was read from another setup, and may not be what runs).
    pub fn agent_models_for(&self, kind: &AgentKind, account: Option<&str>) -> Option<AgentModels> {
        let key = models_key(&self.config.read(), kind, account);
        let all = self.native_models.read();
        all.get(&key).cloned().or_else(|| {
            all.get(kind.id().as_str()).filter(|_| key != kind.id()).cloned().map(|mut m| {
                m.default_model = None;
                m.default_effort = None;
                m
            })
        })
    }

    /// Re-read agents' models for their default accounts: `only` one, or
    /// every installed agent. With `deep`, their CLI listings run too;
    /// otherwise only stale ones do.
    pub fn refresh_agent_models(self, only: Option<AgentKind>, deep: bool) {
        let detected = self.detected.peek().clone();
        let kinds: Vec<AgentKind> = detected
            .into_iter()
            .filter(|d| d.installed() || only.as_ref() == Some(&d.kind))
            .filter(|d| only.is_some() || self.config.peek().agent(&d.kind).enabled)
            .filter(|d| only.as_ref().map(|k| *k == d.kind).unwrap_or(true))
            .map(|d| d.kind)
            .collect();
        self.read_models(kinds.into_iter().map(|k| (k, None)).collect(), deep, LISTING_TTL_HOURS * 60);
    }

    /// A list of `kind`'s models is shown for a chat on `account` (the
    /// picker, the `/` menu's Model page, an automation's model): read them
    /// again when the list is a few minutes old, was made by another Lary
    /// version, or never was. Every place that shows models calls this, so
    /// none shows a list the others would have replaced.
    pub fn refresh_models_for(self, kind: &AgentKind, account: Option<&str>) {
        if self.models_refreshing.peek().contains(&kind.id()) {
            return;
        }
        let key = models_key(&self.config.peek(), kind, account);
        if needs_listing(kind, self.native_models.peek().get(&key), PICKER_TTL_MINUTES) {
            self.read_models(vec![(kind.clone(), account.map(str::to_string))], false, PICKER_TTL_MINUTES);
        }
    }

    /// Read `targets` (an agent and the account), each the way its chats
    /// run; a CLI listing runs when `deep` or when it is older than
    /// `max_age` minutes.
    fn read_models(mut self, targets: Vec<(AgentKind, Option<String>)>, deep: bool, max_age: i64) {
        let busy: HashSet<String> = targets.iter().map(|(k, _)| k.id().to_string()).collect();
        if busy.is_empty() {
            return;
        }
        self.models_refreshing.write().extend(busy.iter().cloned());
        spawn_forever(async move {
            for (kind, account) in targets {
                let (key, at) = {
                    let cfg = self.config.peek();
                    (models_key(&cfg, &kind, account.as_deref()), models::read_for(&cfg, &kind, account.as_deref()))
                };
                let prev = self.native_models.peek().get(&key).cloned();
                let list = deep || needs_listing(&kind, prev.as_ref(), max_age);
                let mut m = models::read(&kind, &at, list, prev.as_ref()).await;
                // A list read again without listing (or whose listing failed)
                // is still the one listed before.
                let listed = list && (!models::has_cli_listing(&kind) || m.listed_at != prev.as_ref().and_then(|p| p.listed_at));
                m.listed_by = if listed { Some(env!("CARGO_PKG_VERSION").to_string()) } else { prev.as_ref().and_then(|p| p.listed_by.clone()) };
                self.native_models.write().insert(key, m);
                self.models_refreshing.write().remove(&kind.id());
            }
            let all = self.native_models.peek().clone();
            let _ = tokio::task::spawn_blocking(move || save(&all)).await;
        });
    }
}

/// Whether `kind`'s models need its CLI's listing again: never listed,
/// listed more than `max_age` minutes ago, or listed by another Lary version
/// (which reads listings its own way).
fn needs_listing(kind: &AgentKind, prev: Option<&AgentModels>, max_age: i64) -> bool {
    let Some(p) = prev else { return true };
    // An agent with no CLI listing (Codex) is read from its files alone.
    let read_at = if models::has_cli_listing(kind) { p.listed_at } else { p.fetched_at };
    let old = read_at.is_none_or(|t| chrono::Utc::now() - t > chrono::Duration::minutes(max_age));
    let other_version = p.listed_by.as_deref() != Some(env!("CARGO_PKG_VERSION"));
    // Listed before Lary read Claude Code's commands, or before it kept the
    // Default its listing names.
    let incomplete = *kind == AgentKind::ClaudeCode && (p.commands.is_empty() || p.listed_default.is_none());
    old || other_version || incomplete
}

// ------------------------------------------------------------------ choices

/// One model as Lary offers it for an agent: the agent's own, or one the
/// user added.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    /// What a session stores (for Cursor, the family; picking resolves a
    /// variant).
    pub id: String,
    pub name: String,
    pub note: Option<String>,
    pub efforts: Vec<String>,
    pub default_effort: Option<String>,
    /// Has a fast variant (Cursor).
    pub fast: bool,
    /// Claude Code's Ultracode can be turned on with it.
    pub ultracode: bool,
    /// The agent's own custom model or provider.
    pub custom: bool,
    /// Added in Lary.
    pub added: bool,
    /// Name of the endpoint that serves it.
    pub endpoint: Option<String>,
    /// The agent features it in its own picker.
    pub featured: bool,
    /// In Lary's picker.
    pub shown: bool,
    pub group: Option<String>,
    pub context: Option<u64>,
    /// Context sizes and how the agent takes them (Cursor).
    pub params: Option<models::ModelParams>,
}

/// Every model Lary knows for `agent`, the agent's own first.
pub fn choices(app: &App, agent: &AgentKind) -> Vec<Choice> {
    let ac = app.config.read().agent(agent);
    let native = app.agent_models(agent);
    let mut out: Vec<Choice> = Vec::new();
    let from_native = native.as_ref().map(|n| n.models.clone()).unwrap_or_else(|| {
        // Not read yet (or not installed): the catalog's suggestions.
        agent
            .spec()
            .models
            .iter()
            .filter(|id| !matches!(**id, "default" | "auto"))
            .map(|id| models::AgentModel { id: id.to_string(), name: lary_core::models::display_name(id), featured: true, ..Default::default() })
            .collect()
    });
    for m in from_native {
        let shown = if m.featured { !ac.hidden_models.contains(&m.id) } else { ac.shown_models.contains(&m.id) };
        out.push(Choice {
            id: m.id.clone(),
            name: m.name.clone(),
            note: m.description.clone(),
            efforts: m.efforts.clone(),
            default_effort: m.default_effort.clone(),
            fast: m.has_fast(),
            ultracode: m.ultracode,
            custom: m.custom,
            added: false,
            endpoint: None,
            featured: m.featured,
            shown,
            group: m.group.clone(),
            context: m.context,
            params: m.params.clone(),
        });
    }
    for c in ac.added_models() {
        // The API account that serves it, by name.
        let endpoint = c.account.as_deref().and_then(|id| app.config.read().account(id).map(|a| a.label.clone()));
        match out.iter_mut().find(|x| x.id == c.id) {
            Some(x) => {
                x.added = true;
                x.shown = true;
                if let Some(n) = c.name.clone() {
                    x.name = n;
                }
                x.endpoint = endpoint;
            }
            None => out.push(Choice {
                id: c.id.clone(),
                name: c.name.clone().unwrap_or_else(|| lary_core::models::display_name(&c.id)),
                note: None,
                efforts: if lary_agents::request::takes_effort(agent) { native_efforts(native.as_ref()) } else { Vec::new() },
                default_effort: None,
                fast: false,
                ultracode: false,
                custom: true,
                added: true,
                endpoint,
                featured: false,
                shown: true,
                group: Some("Added".into()),
                context: lary_core::models::lookup(&c.id).and_then(|i| i.context),
                params: None,
            }),
        }
    }
    out
}

/// The efforts an agent offers at all (for models it doesn't list).
fn native_efforts(n: Option<&AgentModels>) -> Vec<String> {
    let mut v: Vec<String> = n.map(|n| n.models.iter().flat_map(|m| m.efforts.clone()).collect()).unwrap_or_default();
    v.sort_by_key(|e| models::EFFORT_ORDER.iter().position(|x| x == e).unwrap_or(99));
    v.dedup();
    v
}

/// The models in Lary's picker for `agent`.
pub fn picker_choices(app: &App, agent: &AgentKind) -> Vec<Choice> {
    choices(app, agent).into_iter().filter(|c| c.shown).collect()
}

/// What a session runs: its own picks, else Lary's defaults for the
/// agent, else the agent's own settings.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Resolved {
    /// The model id in effect (None: unknown, the agent decides).
    pub model: Option<String>,
    /// The family it belongs to (Cursor variants), for looking it up.
    pub family: Option<String>,
    pub effort: Option<String>,
    pub fast: bool,
    /// The context size picked (Cursor), as the agent names it.
    pub context: Option<String>,
    /// Taken from the agent's own settings rather than chosen.
    pub model_from_agent: bool,
    pub effort_from_agent: bool,
}

pub fn resolve(app: &App, agent: &AgentKind, model: Option<&str>, effort: Option<&str>) -> Resolved {
    resolve_for(app, agent, None, model, effort)
}

/// [`resolve`] for a chat on `account` (None: the agent's default one):
/// its Default is what that account's chats run.
pub fn resolve_for(app: &App, agent: &AgentKind, account: Option<&str>, model: Option<&str>, effort: Option<&str>) -> Resolved {
    let ac = app.config.read().agent(agent);
    let native = app.agent_models_for(agent, account);
    let mut r = Resolved::default();
    let chosen = model.map(str::to_string).or(ac.default_model.clone()).filter(|m| m != "default" && m != "auto");
    r.model_from_agent = chosen.is_none();
    r.model = chosen.or_else(|| native.as_ref().and_then(|n| n.default_model.clone()));
    let found = r.model.as_deref().and_then(|id| native.as_ref().and_then(|n| n.variant(id).map(|(m, v)| (m.clone(), v.cloned()))));
    // A model with its parameters spelled out (a context picked).
    if let (Some((m, _)), Some(id)) = (&found, r.model.as_deref()) {
        let (_, kv) = models::split_params(id);
        if !kv.is_empty() {
            r.family = Some(m.id.clone());
            r.context = kv.iter().find(|(k, _)| k == "context").map(|(_, v)| v.clone());
            r.effort = kv.iter().find(|(k, _)| matches!(k.as_str(), "effort" | "reasoning_effort" | "reasoning")).map(|(_, v)| models::normalize_effort(v));
            r.fast = kv.iter().any(|(k, v)| k == "fast" && v == "true");
            r.effort_from_agent = false;
            return r;
        }
    }
    if let Some((m, v)) = &found {
        r.family = Some(m.id.clone());
        if let Some(v) = v {
            // A Cursor variant carries its effort and speed.
            r.fast = v.fast;
            r.effort = v.effort.clone();
            r.effort_from_agent = r.model_from_agent;
            return r;
        }
    }
    let picked = effort.map(str::to_string).or(ac.default_effort.clone());
    r.effort_from_agent = picked.is_none();
    r.effort = picked.or_else(|| {
        let model_default = found.as_ref().and_then(|(m, _)| m.default_effort.clone());
        // The agent's own default effort goes with its default model.
        let is_agent_default = r.model_from_agent || native.as_ref().and_then(|n| n.default_model.as_deref()) == r.model.as_deref();
        let agent_default = if is_agent_default { native.as_ref().and_then(|n| n.default_effort.clone()) } else { None };
        agent_default.or(model_default)
    });
    r
}

/// Short name for a model id ("Opus 5.5", "GPT-6.1 Sol", "Kimi K2.7 Code").
pub fn short_name(app: &App, agent: &AgentKind, id: &str) -> String {
    use lary_core::models as reg;
    let id = models::split_params(id).0;
    let native = app.agent_models(agent);
    // An alias by the model it stands for now ("Opus 5.5" for Claude
    // Code's `opus`).
    if let Some(real) = native.as_ref().and_then(|n| {
        let m = n.model(id).filter(|m| m.id == id)?;
        let target = m.aliases.first().filter(|t| t.as_str() != id)?;
        n.model(target).filter(|r| &r.id == target).map(|r| r.name.clone())
    }) {
        return real.trim_start_matches("Claude ").to_string();
    }
    // The agent's own name for it.
    if let Some(m) = native.as_ref().and_then(|n| n.model(id).filter(|m| m.listed).map(|m| m.name.clone())) {
        return m.trim_start_matches("Claude ").to_string();
    }
    if let Some((target, label)) = reg::alias(id) {
        return reg::lookup(target).map(|i| i.short_name()).unwrap_or_else(|| label.to_string());
    }
    let ac = app.config.read().agent(agent);
    if let Some(n) = ac.added_models().into_iter().find(|c| c.id == id).and_then(|c| c.name) {
        return n;
    }
    if let Some(info) = reg::lookup(id) {
        return info.short_name();
    }
    if let Some(m) = app.agent_models(agent).as_ref().and_then(|n| n.model(id).map(|m| m.name.clone())) {
        return m.trim_start_matches("Claude ").to_string();
    }
    reg::pretty_name(id)
}

/// The model id and effort to store for a pick: Cursor names one variant
/// per effort and speed; other agents take the effort on its own.
pub fn compose(app: &App, agent: &AgentKind, family: &str, effort: Option<&str>, fast: bool) -> (String, Option<String>) {
    compose_with(app, agent, family, effort, fast, None)
}

/// [`compose`], with a context size (the model then runs with its
/// parameters spelled out, as Cursor takes them).
pub fn compose_with(app: &App, agent: &AgentKind, family: &str, effort: Option<&str>, fast: bool, context: Option<&str>) -> (String, Option<String>) {
    let family = models::split_params(family).0;
    if let Some(m) = app.agent_models(agent).as_ref().and_then(|n| n.models.iter().find(|m| m.id == family).cloned()) {
        if let (Some(p), Some(c)) = (&m.params, context) {
            let e = effort.map(str::to_string).or(m.default_effort.clone());
            return (p.compose(&m.id, c, e.as_deref(), fast), None);
        }
        if !m.variants.is_empty() {
            let e = effort.map(str::to_string).or(m.default_effort.clone());
            if let Some(v) = m.pick_variant(e.as_deref(), fast) {
                return (v.id.clone(), None);
            }
        }
    }
    (family.to_string(), effort.map(str::to_string))
}
