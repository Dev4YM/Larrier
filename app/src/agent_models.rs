//! Each agent's own models, defaults and providers (read by
//! `larrier_agents::models`), cached on disk so the picker has them at
//! once. The agents' files are re-read at start and whenever their settings
//! open; the slower CLI listings refresh every few hours.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use larrier_agents::models::{self, AgentModels};
use larrier_core::AgentKind;

use crate::state::{store, App};

/// How old a CLI model listing may get before it is refreshed.
const LISTING_TTL_HOURS: i64 = 6;

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
    /// What `kind` itself offers and prefers, as last read.
    pub fn agent_models(&self, kind: &AgentKind) -> Option<AgentModels> {
        self.native_models.read().get(&kind.id()).cloned()
    }

    /// Re-read agents' models: `only` one, or every installed agent. With
    /// `deep`, their CLI listings run too; otherwise only stale ones do.
    pub fn refresh_agent_models(mut self, only: Option<AgentKind>, deep: bool) {
        let detected = self.detected.peek().clone();
        let targets: Vec<(AgentKind, Option<PathBuf>)> = detected
            .into_iter()
            .filter(|d| d.installed() || only.as_ref() == Some(&d.kind))
            .filter(|d| only.is_some() || self.config.peek().agent(&d.kind).enabled)
            .filter(|d| only.as_ref().map(|k| *k == d.kind).unwrap_or(true))
            .map(|d| (d.kind, d.path))
            .collect();
        let busy: HashSet<String> = targets.iter().map(|(k, _)| k.id().to_string()).collect();
        if busy.is_empty() {
            return;
        }
        self.models_refreshing.write().extend(busy.iter().cloned());
        spawn_forever(async move {
            for (kind, path) in targets {
                let prev = self.native_models.peek().get(&kind.id()).cloned();
                let stale = prev.as_ref().and_then(|p| p.listed_at).map(|t| chrono::Utc::now() - t > chrono::Duration::hours(LISTING_TTL_HOURS)).unwrap_or(true)
                    // Listed before Larrier read Claude Code's commands.
                    || (kind == AgentKind::ClaudeCode && prev.as_ref().is_some_and(|p| p.commands.is_empty()));
                let m = models::read(&kind, path.as_deref(), deep || stale, prev.as_ref()).await;
                self.native_models.write().insert(kind.id().to_string(), m);
                self.models_refreshing.write().remove(&kind.id());
            }
            let all = self.native_models.peek().clone();
            let _ = tokio::task::spawn_blocking(move || save(&all)).await;
        });
    }
}

// ------------------------------------------------------------------ choices

/// One model as Larrier offers it for an agent: the agent's own, or one the
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
    /// The agent's own custom model or provider.
    pub custom: bool,
    /// Added in Larrier.
    pub added: bool,
    /// Name of the endpoint that serves it.
    pub endpoint: Option<String>,
    /// The agent features it in its own picker.
    pub featured: bool,
    /// In Larrier's picker.
    pub shown: bool,
    pub group: Option<String>,
    pub context: Option<u64>,
}

/// Every model Larrier knows for `agent`, the agent's own first.
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
            .map(|id| models::AgentModel { id: id.to_string(), name: larrier_core::models::display_name(id), featured: true, ..Default::default() })
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
            custom: m.custom,
            added: false,
            endpoint: None,
            featured: m.featured,
            shown,
            group: m.group.clone(),
            context: m.context,
        });
    }
    for c in ac.added_models() {
        let endpoint = c.endpoint.as_deref().and_then(|e| ac.endpoint(e)).map(|e| e.name.clone());
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
                name: c.name.clone().unwrap_or_else(|| larrier_core::models::display_name(&c.id)),
                note: None,
                efforts: if larrier_agents::request::takes_effort(agent) { native_efforts(native.as_ref()) } else { Vec::new() },
                default_effort: None,
                fast: false,
                custom: true,
                added: true,
                endpoint,
                featured: false,
                shown: true,
                group: Some("Added".into()),
                context: larrier_core::models::lookup(&c.id).and_then(|i| i.context),
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

/// The models in Larrier's picker for `agent`.
pub fn picker_choices(app: &App, agent: &AgentKind) -> Vec<Choice> {
    choices(app, agent).into_iter().filter(|c| c.shown).collect()
}

/// What a session runs: its own picks, else Larrier's defaults for the
/// agent, else the agent's own settings.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Resolved {
    /// The model id in effect (None: unknown, the agent decides).
    pub model: Option<String>,
    /// The family it belongs to (Cursor variants), for looking it up.
    pub family: Option<String>,
    pub effort: Option<String>,
    pub fast: bool,
    /// Taken from the agent's own settings rather than chosen.
    pub model_from_agent: bool,
    pub effort_from_agent: bool,
}

pub fn resolve(app: &App, agent: &AgentKind, model: Option<&str>, effort: Option<&str>) -> Resolved {
    let ac = app.config.read().agent(agent);
    let native = app.agent_models(agent);
    let mut r = Resolved::default();
    let chosen = model.map(str::to_string).or(ac.default_model.clone()).filter(|m| m != "default" && m != "auto");
    r.model_from_agent = chosen.is_none();
    r.model = chosen.or_else(|| native.as_ref().and_then(|n| n.default_model.clone()));
    let found = r.model.as_deref().and_then(|id| native.as_ref().and_then(|n| n.variant(id).map(|(m, v)| (m.clone(), v.cloned()))));
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
    use larrier_core::models as reg;
    // The agent's own name for it ("Opus 5.5" for Claude Code's `opus`).
    if let Some(m) = app.agent_models(agent).as_ref().and_then(|n| n.model(id).filter(|m| m.listed).map(|m| m.name.clone())) {
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
    if let Some(m) = app.agent_models(agent).as_ref().and_then(|n| n.models.iter().find(|m| m.id == family).cloned()) {
        if !m.variants.is_empty() {
            let e = effort.map(str::to_string).or(m.default_effort.clone());
            if let Some(v) = m.pick_variant(e.as_deref(), fast) {
                return (v.id.clone(), None);
            }
        }
    }
    (family.to_string(), effort.map(str::to_string))
}
