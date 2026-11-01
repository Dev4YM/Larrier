//! The two rings under a chat's box: how much of the plan's limits is used
//! (the session window's share is its label) and how full the chat's
//! context window is. Each opens a panel with the details: the plan's
//! windows and this chat's tokens; the context's categories on a colored
//! bar, each lit together with its line in the list.

use std::collections::HashMap;

use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use larrier_core::{AgentKind, Usage};
use serde_json::Value;

use super::loaders::Orbit;
use super::usage::{fmt_cost, fmt_int, fmt_reset, fmt_tokens, refresh_quotas, severity, QUOTAS, QUOTA_BUSY};
use super::Dropdown;
use crate::icons::{Icon, I};
use crate::quota::{Quota, Window};
use crate::state::{use_app, App, SettingsTab};

/// A small ring, `pct` of it drawn.
#[component]
fn Ring(pct: f64, sev: &'static str) -> Element {
    let circ = 2.0 * std::f64::consts::PI * 6.0;
    let dash = (pct.clamp(0.0, 100.0) / 100.0 * circ).max(0.01);
    rsx! {
        svg { class: "m-ring {sev}", width: "14", height: "14", view_box: "0 0 16 16",
            circle { class: "bg", cx: "8", cy: "8", r: "6" }
            circle { class: "fg", cx: "8", cy: "8", r: "6", stroke_dasharray: "{dash:.2} {circ:.2}", transform: "rotate(-90 8 8)" }
        }
    }
}

/// A plan window as the ring's label names it: "session", "week".
fn window_noun(name: &str) -> String {
    match name {
        "5-hour" => "session".into(),
        "Weekly" => "week".into(),
        "Daily" => "day".into(),
        n => n.strip_prefix("Weekly · ").map(|m| format!("{m} week")).unwrap_or_else(|| n.to_lowercase()),
    }
}

/// A plan window as the panel names it.
fn window_title(name: &str) -> String {
    match name {
        "5-hour" => "Current session".into(),
        "Weekly" => "Current week".into(),
        "Daily" => "Today".into(),
        n => n.strip_prefix("Weekly · ").map(|m| format!("Current week · {m}")).unwrap_or_else(|| n.to_string()),
    }
}

// ------------------------------------------------------------------ plan usage

/// The plan's ring: the session window's share used ("42% of session"),
/// colored by the fullest window; opens the plan's windows and this chat's
/// tokens.
#[component]
pub fn PlanRing(agent: AgentKind, account_id: Option<String>, usage: Option<Usage>, cost: Option<f64>) -> Element {
    let app = use_app();
    use_hook(move || refresh_quotas(app, false));
    let mut open = use_signal(|| false);
    let target = account_id.clone().or_else(|| app.config.peek().default_account(&agent).map(|a| a.id.clone()));
    let quota: Option<Quota> = QUOTAS.read().as_ref().and_then(|st| {
        st.list.iter().find(|q| q.agent == agent && q.account_id == target).or_else(|| st.list.iter().find(|q| q.agent == agent && q.account_id.is_none())).cloned()
    });
    let limits: Vec<Window> = quota.as_ref().map(|q| q.windows.iter().filter(|w| w.detail.is_none()).cloned().collect()).unwrap_or_default();
    let lead = limits.iter().find(|w| w.name == "5-hour").or(limits.first()).cloned();
    let used = usage.as_ref().map(|u| u.total_tokens()).unwrap_or(0);
    // Limits still on their way: nothing yet, rather than another label first.
    let waiting = QUOTAS.read().is_none() && crate::quota::PLAN_AGENTS.contains(&agent);
    if waiting || (lead.is_none() && used == 0) {
        return rsx! {};
    }
    let peak = limits.iter().map(|w| w.used_pct).fold(0.0, f64::max);
    let label = match &lead {
        Some(w) => format!("{:.0}% of {}", w.used_pct, window_noun(&w.name)),
        None => cost.map(fmt_cost).unwrap_or_else(|| format!("{} tokens", fmt_tokens(used))),
    };
    rsx! {
        div { class: "menu-anchor",
            button { class: if open() { "meter-btn open" } else { "meter-btn" }, title: "Plan usage",
                onclick: move |_| open.set(true),
                Ring { pct: lead.as_ref().map(|w| w.used_pct).unwrap_or(0.0), sev: severity(peak) }
                span { "{label}" }
            }
            Dropdown { open, up: true, right: true, class: "meter-pop".to_string(),
                PlanPanel { agent, quota, usage, cost }
            }
        }
    }
}

#[component]
fn PlanPanel(agent: AgentKind, quota: Option<Quota>, usage: Option<Usage>, cost: Option<f64>) -> Element {
    let app = use_app();
    // Fresh numbers when it opens, and every two minutes while it's open.
    super::usage::use_panel_watch();
    let windows: Vec<Window> = quota.as_ref().map(|q| q.windows.clone()).unwrap_or_default();
    let busy = QUOTA_BUSY();
    let name = agent.display_name();
    let note = quota.as_ref().and_then(|q| q.note.clone()).unwrap_or_else(|| format!("{name} doesn't report plan limits here."));
    let u = usage.filter(|u| u.total_tokens() > 0);
    rsx! {
        div { class: "mp-head",
            span { class: "t", "{name} plan" }
            if let Some(p) = quota.as_ref().and_then(|q| q.plan.clone()) { span { class: "s", "{p}" } }
            div { class: "grow" }
            button { class: "icon-btn sm", title: "Check again", disabled: busy, onclick: move |_| refresh_quotas(app, true),
                if busy { Orbit { size: 12 } } else { Icon { i: I::Refresh, size: 12 } }
            }
        }
        if windows.is_empty() || quota.as_ref().is_some_and(|q| q.note.is_some()) {
            div { class: "note", "{note}" }
        }
        for w in windows {
            {
                let pct = w.used_pct.clamp(0.0, 100.0);
                rsx! {
                    div { key: "{w.name}", class: "win {severity(w.used_pct)}",
                        div { class: "row",
                            span { class: "nm", "{window_title(&w.name)}" }
                            if let Some(d) = &w.detail { span { class: "rs", "{d}" } }
                            if let Some(r) = w.resets_at { span { class: "rs", "resets {fmt_reset(r)}" } }
                            if w.detail.is_none() || w.used_pct > 0.0 { span { class: "pct", "{w.used_pct:.0}%" } }
                        }
                        if w.detail.is_none() || w.used_pct > 0.0 {
                            div { class: "track", div { class: "fill", style: "width: {pct:.1}%;" } }
                        }
                    }
                }
            }
        }
        if let Some(u) = u {
            div { class: "sec", "This chat" }
            div { class: "grid",
                span { class: "k", "Input" } span { class: "v", "{fmt_int(u.input_tokens)}" }
                span { class: "k", "Output" } span { class: "v", "{fmt_int(u.output_tokens)}" }
                if u.reasoning_tokens > 0 { span { class: "k", "Reasoning" } span { class: "v", "{fmt_int(u.reasoning_tokens)}" } }
                span { class: "k", "Cache read" } span { class: "v", "{fmt_int(u.cache_read_tokens)}" }
                span { class: "k", "Cache write" } span { class: "v", "{fmt_int(u.cache_write_tokens)}" }
                if let Some(c) = cost { span { class: "k", "Cost" } span { class: "v", "{fmt_cost(c)}" } }
            }
        }
        div { class: "foot",
            button { class: "menu-item", onclick: move |_| app.open_settings(SettingsTab::Usage),
                span { class: "muted", Icon { i: I::Cpu, size: 14 } }
                span { "Plan & Usage" }
            }
        }
    }
}

// ------------------------------------------------------------------ context

/// One part of the context window.
#[derive(Clone, Debug, PartialEq)]
struct Part {
    id: String,
    name: String,
    tokens: u64,
    /// "used", "buffer" (kept free for compacting), "free", or "deferred"
    /// (counted only once used).
    kind: String,
}

#[derive(Clone, Debug, PartialEq)]
struct Breakdown {
    total: u64,
    max: u64,
    parts: Vec<Part>,
}

#[derive(Clone, Debug, PartialEq)]
struct Asked {
    /// The chat as it was when asked (its message count).
    key: usize,
    loading: bool,
    result: Option<Result<Breakdown, String>>,
}

/// Claude Code's breakdowns, per chat.
static BREAKDOWNS: GlobalSignal<HashMap<String, Asked>> = Signal::global(HashMap::new);

fn breakdown(v: &Value) -> Breakdown {
    let parts = v["categories"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| Part {
            id: c["id"].as_str().map(String::from).unwrap_or_else(|| c["name"].as_str().unwrap_or("").to_lowercase().replace(' ', "_")),
            name: c["name"].as_str().unwrap_or("").to_string(),
            tokens: c["tokens"].as_u64().unwrap_or(0),
            kind: c["kind"].as_str().map(String::from).unwrap_or_else(|| if c["isDeferred"].as_bool() == Some(true) { "deferred".into() } else { "used".into() }),
        })
        .filter(|p| p.tokens > 0)
        .collect();
    Breakdown { total: v["totalTokens"].as_u64().unwrap_or(0), max: v["maxTokens"].as_u64().unwrap_or(0), parts }
}

/// Ask Claude Code what chat `id`'s context is made of, unless already asked
/// at this point of the chat.
fn ask(app: App, id: String, key: usize) {
    let current = BREAKDOWNS.peek().get(&id).cloned();
    if current.as_ref().is_some_and(|a| a.loading || (a.key == key && a.result.is_some())) {
        return;
    }
    let req = app.peek_session(&id, |s| larrier_agents::build_request(&app.config.peek(), s, &s.agent.clone()));
    BREAKDOWNS.write().insert(id.clone(), Asked { key, loading: true, result: current.and_then(|a| a.result) });
    spawn_forever(async move {
        let result = match req {
            Some(Ok(req)) => larrier_agents::drivers::claude::context_usage(req).await.map(|v| breakdown(&v)),
            Some(Err(e)) => Err(e.to_string()),
            None => Err("The chat isn't open.".into()),
        };
        BREAKDOWNS.write().insert(id, Asked { key, loading: false, result: Some(result) });
    });
}

/// A part's color on the bar and in the list.
fn color(id: &str, i: usize) -> String {
    match id {
        "system_prompt" => "#64748b".into(),
        "system_tools" => "#94a3b8".into(),
        "mcp_tools" => "#0891b2".into(),
        "custom_agents" => "#7c3aed".into(),
        "memory_files" => "#d97706".into(),
        "skills" => "#db2777".into(),
        "slash_commands" => "#059669".into(),
        "messages" | "in_use" => "#2563eb".into(),
        "autocompact_buffer" => "repeating-linear-gradient(135deg, color-mix(in srgb, var(--fg-3) 55%, transparent) 0 2px, color-mix(in srgb, var(--fg-3) 18%, transparent) 2px 5px)".into(),
        "free_space" => "color-mix(in srgb, var(--fg-3) 16%, transparent)".into(),
        _ => ["#0d9488", "#ea580c", "#4f46e5", "#65a30d", "#be185d"][i % 5].into(),
    }
}

/// The context ring: the share of the window in use ("38% context");
/// opens what it's made of.
#[component]
pub fn ContextRing(session_id: String, agent: AgentKind, context: Option<(u64, Option<u64>)>, messages: usize) -> Element {
    let app = use_app();
    let mut open = use_signal(|| false);
    let is_open = open();
    let claude = agent == AgentKind::ClaudeCode;
    let sid = session_id.clone();
    use_effect(use_reactive!(|is_open, messages| {
        if is_open && claude {
            ask(app, sid.clone(), messages);
        }
    }));
    let Some((used, window)) = context else { return rsx! {} };
    let pct = window.filter(|w| *w > 0).map(|w| used as f64 / w as f64 * 100.0);
    let label = match pct {
        Some(p) => format!("{p:.0}% context"),
        None => format!("{} context", fmt_tokens(used)),
    };
    rsx! {
        div { class: "menu-anchor",
            button { class: if is_open { "meter-btn open" } else { "meter-btn" }, title: "Context window",
                onclick: move |_| open.set(true),
                Ring { pct: pct.unwrap_or(0.0), sev: severity(pct.unwrap_or(0.0)) }
                span { "{label}" }
            }
            Dropdown { open, up: true, right: true, class: "meter-pop".to_string(),
                ContextPanel { session_id, agent, used, window }
            }
        }
    }
}

#[component]
fn ContextPanel(session_id: String, agent: AgentKind, used: u64, window: Option<u64>) -> Element {
    let mut hot = use_signal(|| None::<String>);
    let asked = BREAKDOWNS.read().get(&session_id).cloned();
    let claude = agent == AgentKind::ClaudeCode;
    let loading = asked.as_ref().is_some_and(|a| a.loading);
    // Without a breakdown: what's in use and what's free.
    let plain = |used: u64, max: u64| {
        let mut parts = vec![Part { id: "in_use".into(), name: "In use".into(), tokens: used, kind: "used".into() }];
        if max > used {
            parts.push(Part { id: "free_space".into(), name: "Free space".into(), tokens: max - used, kind: "free".into() });
        }
        Breakdown { total: used, max, parts }
    };
    let max = window.unwrap_or(0);
    let (b, note) = match asked.and_then(|a| a.result) {
        Some(Ok(b)) if b.max > 0 => (b, None),
        Some(Ok(_)) => (plain(used, max), None),
        Some(Err(e)) => (plain(used, max), Some(format!("Claude Code couldn't break it down: {e}"))),
        None if claude => (plain(used, max), None),
        None => (plain(used, max), Some(format!("{} reports only how much is in use, not what it's made of.", agent.display_name()))),
    };
    let share = |t: u64| if b.max > 0 { t as f64 / b.max as f64 * 100.0 } else { 0.0 };
    let total_pct = share(b.total);
    let on_bar: Vec<(usize, Part)> = b.parts.iter().cloned().enumerate().filter(|(_, p)| p.kind != "deferred").collect();
    let lit = hot();
    rsx! {
        div { class: "mp-head",
            span { class: "t", "Context window" }
            div { class: "grow" }
            if loading { span { class: "s", title: "Asking Claude Code what's in it", Orbit { size: 12 } } }
        }
        div { class: "ctx-total",
            span { class: "big", "{fmt_tokens(b.total)}" }
            if b.max > 0 {
                span { class: "s", " of {fmt_tokens(b.max)} tokens · {total_pct:.0}%" }
            } else {
                span { class: "s", " tokens in use" }
            }
        }
        if b.max > 0 {
            div { class: if lit.is_some() { "ctx-bar has-hot" } else { "ctx-bar" }, onmouseleave: move |_| hot.set(None),
                for (i, p) in on_bar {
                    {
                        let id = p.id.clone();
                        let on = lit.as_deref() == Some(p.id.as_str());
                        rsx! {
                            div { key: "{p.id}", class: if on { "part hot" } else { "part" },
                                style: "flex-grow: {p.tokens}; background: {color(&p.id, i)};",
                                onmouseenter: move |_| hot.set(Some(id.clone())),
                            }
                        }
                    }
                }
            }
        }
        div { class: if lit.is_some() { "ctx-legend has-hot" } else { "ctx-legend" }, onmouseleave: move |_| hot.set(None),
            for (i, p) in b.parts.iter().cloned().enumerate() {
                {
                    let id = p.id.clone();
                    let on = lit.as_deref() == Some(p.id.as_str());
                    let deferred = p.kind == "deferred";
                    let mut cls = String::from("row");
                    if on { cls.push_str(" hot"); }
                    if deferred { cls.push_str(" deferred"); }
                    rsx! {
                        div { key: "{p.id}", class: "{cls}",
                            title: if deferred { "Not in the context until used" } else { "" },
                            onmouseenter: move |_| if !deferred { hot.set(Some(id.clone())) },
                            span { class: "dot", style: if deferred { "background: transparent; box-shadow: inset 0 0 0 1.5px {color(&p.id, i)};".to_string() } else { format!("background: {};", color(&p.id, i)) } }
                            span { class: "nm", "{p.name}" }
                            span { class: "v", "{fmt_tokens(p.tokens)}" }
                            span { class: "p", if deferred { "—" } else { "{share(p.tokens):.1}%" } }
                        }
                    }
                }
            }
        }
        if let Some(n) = note { div { class: "note", "{n}" } }
    }
}
