//! Plan & Usage: plan limits per account, then tokens, spend and activity
//! across the agents you use — computed from your local sessions.

mod stats;

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::{DateTime, Local, NaiveDate, Utc};
use dioxus::prelude::*;
use larrier_agents::Detected;
use larrier_core::{AgentKind, Config, SessionSummary};

use crate::icons::{Icon, I};
use crate::logos::{agent_logo, model_logo, Logo};
use crate::quota::{Quota, Window, PLAN_AGENTS};
use crate::state::{use_app, App};
pub use stats::{date_of, day_num, use_cost, SessionStats, StatsSet, Use};

const CSS: &str = include_str!("usage.css");
static GENERATION: AtomicU64 = AtomicU64::new(0);

// ------------------------------------------------------------------ formatting

/// "950", "12.3k", "4.5M", "1.2B".
pub fn fmt_tokens(n: u64) -> String {
    let f = n as f64;
    let (v, suffix) = if n >= 1_000_000_000 {
        (f / 1e9, "B")
    } else if n >= 1_000_000 {
        (f / 1e6, "M")
    } else if n >= 1_000 {
        (f / 1e3, "k")
    } else {
        return n.to_string();
    };
    let s = if v >= 100.0 { format!("{v:.0}") } else { format!("{v:.1}") };
    format!("{}{suffix}", s.trim_end_matches(".0"))
}

/// "$0.00", "<$0.01", "$12.34", "$1,234".
pub fn fmt_cost(c: f64) -> String {
    if c <= 0.0 {
        return "$0.00".into();
    }
    if c < 0.01 {
        return "<$0.01".into();
    }
    if c < 1000.0 {
        return format!("${c:.2}");
    }
    format!("${}", fmt_int(c.round() as u64))
}

/// "1,234,567".
pub fn fmt_int(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// "in 42m", "in 3h 10m", "Thu 4:00 PM".
pub fn fmt_reset(t: DateTime<Utc>) -> String {
    let secs = (t - Utc::now()).num_seconds();
    if secs <= 0 {
        "now".into()
    } else if secs < 3600 {
        format!("in {}m", (secs / 60).max(1))
    } else if secs < 86_400 {
        format!("in {}h {}m", secs / 3600, (secs % 3600) / 60)
    } else {
        t.with_timezone(&Local).format("%a %-I:%M %p").to_string()
    }
}

/// Signed change vs the previous period: ("▲ 18%", 1 / -1 / 0).
pub fn delta(cur: f64, prev: f64) -> (String, i8) {
    if prev <= 0.0 {
        return if cur > 0.0 { ("new".into(), 1) } else { ("—".into(), 0) };
    }
    let ch = (cur - prev) / prev * 100.0;
    if ch.abs() < 0.5 {
        ("— 0%".into(), 0)
    } else if ch > 0.0 {
        (format!("▲ {:.0}%", ch.min(9999.0)), 1)
    } else {
        (format!("▼ {:.0}%", ch.abs()), -1)
    }
}

/// A round axis maximum and tick step for `v` (1 / 2 / 2.5 / 5 × 10ⁿ).
pub fn nice_scale(v: f64, integers: bool) -> (f64, f64) {
    if v <= 0.0 {
        return (if integers { 4.0 } else { 1.0 }, if integers { 1.0 } else { 0.25 });
    }
    let raw = v / 4.0;
    let mag = 10f64.powf(raw.log10().floor());
    let norm = raw / mag;
    let mut step = mag
        * if norm <= 1.0 {
            1.0
        } else if norm <= 2.0 {
            2.0
        } else if norm <= 2.5 {
            2.5
        } else if norm <= 5.0 {
            5.0
        } else {
            10.0
        };
    if integers {
        step = step.ceil().max(1.0);
    }
    ((v / step).ceil() * step, step)
}

// ------------------------------------------------------------------ model

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Range {
    Week,
    Month,
    Quarter,
    All,
}

impl Range {
    pub const ALL: [Range; 4] = [Range::Week, Range::Month, Range::Quarter, Range::All];

    pub fn label(self) -> &'static str {
        match self {
            Range::Week => "7D",
            Range::Month => "30D",
            Range::Quarter => "90D",
            Range::All => "All",
        }
    }

    pub fn long(self) -> &'static str {
        match self {
            Range::Week => "last 7 days",
            Range::Month => "last 30 days",
            Range::Quarter => "last 90 days",
            Range::All => "all time",
        }
    }

    pub fn days(self) -> Option<i64> {
        match self {
            Range::Week => Some(7),
            Range::Month => Some(30),
            Range::Quarter => Some(90),
            Range::All => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Metric {
    Tokens,
    Cost,
    Sessions,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Kpis {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost: f64,
    /// Tokens with a reported or list-price cost.
    pub priced: u64,
    pub unpriced: u64,
    pub sessions: usize,
    pub active_days: usize,
    pub prompts: u64,
}

impl Kpis {
    pub fn tokens(&self) -> u64 {
        self.input + self.output
    }
    pub fn cache(&self) -> u64 {
        self.cache_read + self.cache_write
    }
    pub fn hit_rate(&self) -> Option<f64> {
        let d = self.input + self.cache_read;
        (d > 0).then(|| self.cache_read as f64 / d as f64 * 100.0)
    }
    pub fn avg(&self) -> u64 {
        if self.sessions == 0 {
            0
        } else {
            self.tokens() / self.sessions as u64
        }
    }
    fn add(&mut self, u: &Use, cost: Option<f64>) {
        self.input += u.input;
        self.output += u.output;
        self.cache_read += u.cache_read;
        self.cache_write += u.cache_write;
        match cost {
            Some(c) => {
                self.cost += c;
                self.priced += u.input + u.output;
            }
            None => self.unpriced += u.input + u.output,
        }
    }
    fn merge(&mut self, o: &Kpis) {
        self.input += o.input;
        self.output += o.output;
        self.cache_read += o.cache_read;
        self.cache_write += o.cache_write;
        self.cost += o.cost;
        self.priced += o.priced;
        self.unpriced += o.unpriced;
        self.prompts += o.prompts;
    }
}

/// One chart column.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Bucket {
    pub start: i32,
    pub days: i32,
    pub input: u64,
    pub output: u64,
    pub cache: u64,
    pub cost: f64,
    pub sessions: u32,
    pub prompts: u32,
}

/// One breakdown row (an agent, a model, a workspace).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Agg {
    pub key: String,
    pub label: String,
    pub kind: Option<AgentKind>,
    pub tokens: u64,
    pub cache: u64,
    pub cost: f64,
    pub sessions: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TopRow {
    pub id: String,
    pub title: String,
    pub agent: AgentKind,
    pub sub: String,
    pub tokens: u64,
    pub cost: f64,
    pub updated: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Report {
    pub cur: Kpis,
    pub prev: Option<Kpis>,
    pub buckets: Vec<Bucket>,
    pub step: i32,
    pub span_days: i32,
    pub by_agent: Vec<Agg>,
    pub by_model: Vec<Agg>,
    pub by_repo: Vec<Agg>,
    pub top: Vec<TopRow>,
    /// Prompts by weekday (Monday first) × hour.
    pub heat: Vec<u32>,
    pub estimated: bool,
    /// Per account ("agent|account"): tokens, cost, sessions in range.
    pub accounts: HashMap<String, (u64, f64, usize)>,
}

/// The account a session ran on (its own, else the agent's default).
pub fn resolve_account(cfg: &Config, agent: &AgentKind, acc: Option<&str>) -> Option<String> {
    match acc {
        Some(a) if cfg.account(a).is_some() => Some(a.to_string()),
        _ => cfg.default_account(agent).map(|a| a.id.clone()),
    }
}

pub fn account_key(agent: &AgentKind, account: Option<&str>) -> String {
    format!("{}|{}", agent.id(), account.unwrap_or(""))
}

fn in_scope(cfg: &Config, s: &SessionSummary, st: &SessionStats, scope: &str) -> bool {
    if !cfg.agent(&s.agent).enabled {
        return false;
    }
    match scope.split_once(':') {
        Some(("agent", id)) => s.agent.id() == id,
        Some(("acct", id)) => resolve_account(cfg, &s.agent, st.account.as_deref()).as_deref() == Some(id),
        _ => true,
    }
}

fn top_with_others(mut v: Vec<Agg>, n: usize) -> Vec<Agg> {
    v.sort_by(|a, b| b.tokens.cmp(&a.tokens).then(b.cost.partial_cmp(&a.cost).unwrap_or(std::cmp::Ordering::Equal)).then(a.label.cmp(&b.label)));
    v.retain(|a| a.tokens > 0 || a.cost > 0.0);
    if v.len() <= n + 1 {
        return v;
    }
    let rest = v.split_off(n);
    let mut others = Agg { key: "~others".into(), label: format!("{} others", rest.len()), ..Default::default() };
    for a in rest {
        others.tokens += a.tokens;
        others.cache += a.cache;
        others.cost += a.cost;
        others.sessions += a.sessions;
    }
    v.push(others);
    v
}

/// Everything the page shows for one range + scope.
pub fn build_report(list: &[SessionSummary], stats: &HashMap<String, SessionStats>, cfg: &Config, range: Range, scope: &str, today: NaiveDate) -> Report {
    let t = day_num(today);
    let included: Vec<(&SessionSummary, &SessionStats)> = list
        .iter()
        .filter_map(|s| {
            let st = stats.get(&s.id)?;
            in_scope(cfg, s, st, scope).then_some((s, st))
        })
        .collect();
    let (first, step) = match range.days() {
        Some(d) => (t - d as i32 + 1, 1),
        None => {
            let first = included
                .iter()
                .filter_map(|(_, st)| st.uses.iter().filter(|u| u.input + u.output > 0).map(|u| u.day).min())
                .min()
                .unwrap_or(t)
                .min(t - 29);
            let span = t - first + 1;
            (first, if span > 730 { 30 } else if span > 120 { 7 } else { 1 })
        }
    };
    let n = ((t - first + 1 + step - 1) / step).max(1);
    let start = t - n * step + 1;
    let prev = range.days().map(|d| (start - d as i32, start - 1));

    let mut r = Report {
        step,
        span_days: n * step,
        buckets: (0..n).map(|i| Bucket { start: start + i * step, days: step, ..Default::default() }).collect(),
        heat: vec![0; 7 * 24],
        ..Default::default()
    };
    let mut days: BTreeSet<i32> = BTreeSet::new();
    let mut prev_k = Kpis::default();
    let mut prev_days: BTreeSet<i32> = BTreeSet::new();
    let mut agents: HashMap<String, Agg> = HashMap::new();
    let mut models: HashMap<String, Agg> = HashMap::new();
    let mut repos: HashMap<String, Agg> = HashMap::new();

    for (s, st) in included {
        let mut sk = Kpis::default();
        let mut active = false;
        let mut prev_active = false;
        let mut last_bucket = -1;
        let mut s_models: Vec<String> = Vec::new();
        for u in &st.uses {
            let cost = use_cost(u);
            if u.day >= start && u.day <= t {
                active = true;
                days.insert(u.day);
                sk.add(u, cost);
                let bi = (u.day - start) / step;
                if let Some(b) = r.buckets.get_mut(bi as usize) {
                    b.input += u.input;
                    b.output += u.output;
                    b.cache += u.cache_read + u.cache_write;
                    b.cost += cost.unwrap_or(0.0);
                    if bi != last_bucket {
                        b.sessions += 1;
                        last_bucket = bi;
                    }
                }
                let mk = if u.model.trim().is_empty() { "~unknown".to_string() } else { u.model.clone() };
                let label = if mk == "~unknown" { "Unknown model".to_string() } else { mk.clone() };
                let m = models.entry(mk.clone()).or_insert_with(|| Agg { key: mk.clone(), label, ..Default::default() });
                m.tokens += u.input + u.output;
                m.cache += u.cache_read + u.cache_write;
                m.cost += cost.unwrap_or(0.0);
                if !s_models.contains(&mk) {
                    s_models.push(mk);
                }
            } else if let Some((ps, pe)) = prev {
                if u.day >= ps && u.day <= pe {
                    prev_active = true;
                    prev_days.insert(u.day);
                    prev_k.add(u, cost);
                }
            }
        }
        for &(d, h, w, cnt) in &st.prompts {
            if d >= start && d <= t {
                active = true;
                days.insert(d);
                sk.prompts += cnt as u64;
                if let Some(c) = r.heat.get_mut(w as usize * 24 + h as usize) {
                    *c += cnt;
                }
                if let Some(b) = r.buckets.get_mut(((d - start) / step) as usize) {
                    b.prompts += cnt;
                }
            } else if let Some((ps, pe)) = prev {
                if d >= ps && d <= pe {
                    prev_active = true;
                    prev_days.insert(d);
                    prev_k.prompts += cnt as u64;
                }
            }
        }
        if prev_active {
            prev_k.sessions += 1;
        }
        if !active {
            continue;
        }
        for mk in s_models {
            if let Some(m) = models.get_mut(&mk) {
                m.sessions += 1;
            }
        }
        if st.estimated && sk.tokens() > 0 {
            r.estimated = true;
        }
        r.cur.merge(&sk);
        r.cur.sessions += 1;

        let a = agents.entry(s.agent.id()).or_insert_with(|| Agg { key: s.agent.id(), label: s.agent.display_name(), kind: Some(s.agent.clone()), ..Default::default() });
        a.tokens += sk.tokens();
        a.cache += sk.cache();
        a.cost += sk.cost;
        a.sessions += 1;

        let (rk, rl) = match &s.workspace {
            Some(w) => (w.path.replace('\\', "/").trim_end_matches('/').to_lowercase(), if w.name.is_empty() { w.path.clone() } else { w.name.clone() }),
            None => ("~none".to_string(), "No folder".to_string()),
        };
        let rp = repos.entry(rk.clone()).or_insert_with(|| Agg { key: rk, label: rl, ..Default::default() });
        rp.tokens += sk.tokens();
        rp.cache += sk.cache();
        rp.cost += sk.cost;
        rp.sessions += 1;

        let acct = resolve_account(cfg, &s.agent, st.account.as_deref());
        let e = r.accounts.entry(account_key(&s.agent, acct.as_deref())).or_default();
        e.0 += sk.tokens();
        e.1 += sk.cost;
        e.2 += 1;

        if sk.tokens() > 0 || sk.cost > 0.0 {
            let ws = s.workspace.as_ref().map(|w| w.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| "No folder".into());
            r.top.push(TopRow {
                id: s.id.clone(),
                title: s.title.clone(),
                agent: s.agent.clone(),
                sub: format!("{} · {}", s.agent.display_name(), ws),
                tokens: sk.tokens(),
                cost: sk.cost,
                updated: s.updated_at,
            });
        }
    }
    r.cur.active_days = days.len();
    if prev.is_some() {
        prev_k.active_days = prev_days.len();
        r.prev = Some(prev_k);
    }
    r.by_agent = top_with_others(agents.into_values().collect(), 10);
    r.by_model = top_with_others(models.into_values().collect(), 7);
    r.by_repo = top_with_others(repos.into_values().collect(), 7);
    r.top.sort_by(|a, b| b.cost.partial_cmp(&a.cost).unwrap_or(std::cmp::Ordering::Equal).then(b.tokens.cmp(&a.tokens)));
    r.top.truncate(8);
    r
}

/// Agents whose plans to show: enabled, and installed or with accounts.
pub fn plan_agents(cfg: &Config, detected: &[Detected]) -> Vec<AgentKind> {
    PLAN_AGENTS
        .iter()
        .filter(|k| cfg.agent(k).enabled)
        .filter(|k| detected.is_empty() || detected.iter().any(|d| &d.kind == *k && d.installed()) || !cfg.accounts_for(k).is_empty())
        .cloned()
        .collect()
}

/// Filter menu: all agents, each enabled agent with usage, and accounts of
/// agents that have more than one.
fn scope_options(cfg: &Config, list: &[SessionSummary], stats: Option<&HashMap<String, SessionStats>>, plan: &[AgentKind]) -> Vec<(String, String)> {
    let mut kinds: Vec<AgentKind> = plan.to_vec();
    if let Some(stats) = stats {
        for s in list {
            if kinds.contains(&s.agent) || !cfg.agent(&s.agent).enabled {
                continue;
            }
            if stats.get(&s.id).map(|st| st.uses.iter().any(|u| u.input + u.output > 0)).unwrap_or(false) {
                kinds.push(s.agent.clone());
            }
        }
    }
    kinds.sort_by_key(|k| k.display_name());
    let mut out = vec![("all".to_string(), "All agents".to_string())];
    for k in kinds {
        out.push((format!("agent:{}", k.id()), k.display_name()));
        let accts = cfg.accounts_for(&k);
        if accts.len() > 1 {
            for a in accts {
                let label = if a.label.trim().is_empty() { a.auth.label().to_string() } else { a.label.clone() };
                out.push((format!("acct:{}", a.id), format!("{} · {}", k.display_name(), label)));
            }
        }
    }
    out
}

// ------------------------------------------------------------------ quotas (shared)

#[derive(Clone, PartialEq)]
pub struct QuotaState {
    pub at: DateTime<Utc>,
    pub list: Vec<Quota>,
}

/// Latest plan limits for every enabled plan agent.
pub static QUOTAS: GlobalSignal<Option<QuotaState>> = Signal::global(|| None);
pub static QUOTA_BUSY: GlobalSignal<bool> = Signal::global(|| false);

/// Refresh plan limits in the background — at most every three minutes
/// unless `force`.
pub fn refresh_quotas(app: App, force: bool) {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    {
        let Ok(mut last) = LAST.lock() else { return };
        if !force && last.map(|t| t.elapsed() < Duration::from_secs(180)).unwrap_or(false) {
            return;
        }
        *last = Some(Instant::now());
    }
    let cfg = app.config.peek().clone();
    let agents = plan_agents(&cfg, &app.detected.peek());
    *QUOTA_BUSY.write() = true;
    dioxus::core::spawn_forever(async move {
        let r = tokio::task::spawn_blocking(move || crate::quota::fetch_all(&cfg, &agents, force)).await;
        if let Ok(list) = r {
            *QUOTAS.write() = Some(QuotaState { at: Utc::now(), list });
        }
        *QUOTA_BUSY.write() = false;
    });
}

/// A window's usage reported by a running agent: update the cached plan
/// limits right away, without asking the account again.
pub fn observe_limit(agent: &AgentKind, account_id: Option<&str>, window: &str, used_pct: f64, resets_at: Option<i64>) {
    let mut state = QUOTAS.write();
    let Some(state) = state.as_mut() else { return };
    let q = match state.list.iter().position(|q| &q.agent == agent && q.account_id.as_deref() == account_id) {
        Some(i) => Some(i),
        None => state.list.iter().position(|q| &q.agent == agent && q.account_id.is_none()),
    };
    let Some(q) = q.map(|i| &mut state.list[i]) else { return };
    let resets_at = resets_at.and_then(|t| DateTime::from_timestamp(t, 0));
    match q.windows.iter_mut().find(|w| w.name == window) {
        Some(w) => {
            w.used_pct = used_pct;
            w.resets_at = resets_at.or(w.resets_at);
        }
        None => q.windows.push(Window { name: window.to_string(), used_pct, resets_at, detail: None }),
    }
    q.as_of = Some(Utc::now());
}

fn short_window(name: &str) -> String {
    match name {
        "5-hour" => "5h".into(),
        "Weekly" => "wk".into(),
        "Daily" => "day".into(),
        n => n.strip_prefix("Weekly · ").map(|m| m.to_lowercase()).unwrap_or_else(|| n.to_lowercase()),
    }
}

fn severity(pct: f64) -> &'static str {
    if pct >= 90.0 {
        "crit"
    } else if pct >= 75.0 {
        "warn"
    } else {
        ""
    }
}

/// Compact plan-limit indicator for the composer footer: a tiny ring and
/// "5h 42% · wk 18%", with the full breakdown in the tooltip. Reads the
/// shared cache; refreshes it in the background at most every few minutes.
#[component]
pub fn QuotaChip(agent: AgentKind, account_id: Option<String>) -> Element {
    let app = use_app();
    use_hook(move || refresh_quotas(app, false));
    let target = account_id.clone().or_else(|| app.config.peek().default_account(&agent).map(|a| a.id.clone()));
    let state = QUOTAS.read();
    let Some(state) = state.as_ref() else { return rsx! {} };
    let q = state
        .list
        .iter()
        .find(|q| q.agent == agent && q.account_id == target)
        .or_else(|| state.list.iter().find(|q| q.agent == agent && q.account_id.is_none()));
    let Some(q) = q else { return rsx! {} };
    let wins: Vec<&Window> = q.windows.iter().filter(|w| w.detail.is_none()).collect();
    if wins.is_empty() {
        return rsx! {};
    }
    let shown: Vec<&Window> = wins.iter().take(2).copied().collect();
    let text = shown.iter().map(|w| format!("{} {:.0}%", short_window(&w.name), w.used_pct)).collect::<Vec<_>>().join(" · ");
    let peak = wins.iter().map(|w| w.used_pct).fold(0.0, f64::max);
    let mut tip = format!("{} plan limits", q.agent.display_name());
    if let Some(p) = &q.plan {
        tip.push_str(&format!(" · {p}"));
    }
    for w in &wins {
        tip.push_str(&format!("\n{}: {:.0}% used", w.name, w.used_pct));
        if let Some(r) = w.resets_at {
            tip.push_str(&format!(" · resets {}", fmt_reset(r)));
        }
    }
    let color = match severity(peak) {
        "crit" => "var(--red)",
        "warn" => "var(--amber)",
        _ => "var(--fg-2)",
    };
    let circ = 2.0 * std::f64::consts::PI * 6.0;
    let dash = (peak.clamp(0.0, 100.0) / 100.0 * circ).max(0.01);
    let dasharray = format!("{dash:.2} {circ:.2}");
    rsx! {
        span {
            class: "quota-chip",
            title: "{tip}",
            style: "display: inline-flex; align-items: center; gap: 5px; height: 22px; padding: 0 6px; border-radius: 6px; font-size: 11.5px; color: var(--fg-3); font-variant-numeric: tabular-nums; white-space: nowrap; cursor: default;",
            svg { width: "13", height: "13", view_box: "0 0 16 16", style: "flex-shrink: 0;",
                circle { cx: "8", cy: "8", r: "6", fill: "none", stroke: "var(--fg-4)", stroke_width: "2" }
                circle {
                    cx: "8", cy: "8", r: "6", fill: "none", stroke: "{color}", stroke_width: "2",
                    stroke_linecap: "round", stroke_dasharray: "{dasharray}", transform: "rotate(-90 8 8)",
                }
            }
            "{text}"
        }
    }
}

// ------------------------------------------------------------------ view

#[component]
pub fn UsageView() -> Element {
    let app = use_app();
    let mut range = use_signal(|| Range::Month);
    let mut scope = use_signal(|| "all".to_string());
    let mut metric = use_signal(|| Metric::Tokens);
    let mut with_cache = use_signal(|| false);
    let mut table = use_signal(|| false);
    let mut stats: Signal<Option<StatsSet>> = use_signal(|| None);
    let mut loading = use_signal(|| false);

    use_hook(move || refresh_quotas(app, false));

    // Re-analyze when sessions change; unchanged sessions come from cache.
    let key = {
        let l = app.sessions.read();
        (l.len(), l.iter().map(|s| s.updated_at.timestamp()).max().unwrap_or(0) / 20)
    };
    use_effect(use_reactive!(|key| {
        let _ = key;
        let list = app.sessions.peek().clone();
        loading.set(true);
        spawn(async move {
            if let Ok(map) = tokio::task::spawn_blocking(move || stats::load_all(&list)).await {
                stats.set(Some(StatsSet { map, generation: GENERATION.fetch_add(1, Ordering::SeqCst) + 1 }));
            }
            loading.set(false);
        });
    }));

    let cfg = app.config.read().clone();
    let detected = app.detected.read().clone();
    let plan = plan_agents(&cfg, &detected);
    let list = app.sessions.read().clone();
    let today = Local::now().date_naive();
    let r = range();
    let st = stats.read().clone();
    let options = scope_options(&cfg, &list, st.as_ref().map(|s| s.map.as_ref()), &plan);
    // A filter whose agent was disabled falls back to everything.
    let sc = Some(scope()).filter(|v| options.iter().any(|(o, _)| o == v)).unwrap_or_else(|| "all".into());
    let report = st.as_ref().map(|s| build_report(&list, &s.map, &cfg, r, &sc, today));
    let scope_plan: Vec<AgentKind> = match sc.split_once(':') {
        Some(("agent", id)) => plan.iter().filter(|k| k.id() == id).cloned().collect(),
        Some(("acct", id)) => cfg.account(id).map(|a| a.agent.clone()).into_iter().filter(|k| plan.contains(k)).collect(),
        _ => plan.clone(),
    };
    let scope_acct = sc.strip_prefix("acct:").map(|s| s.to_string());
    let n_sessions = list.len();
    let busy = loading();

    rsx! {
        style { {CSS} }
        div { class: "us-wrap",
            h1 { "Plan & Usage" }
            div { class: "sub", "Plan limits, tokens and spend for the agents you use — computed from your local sessions." }

            div { class: "us-filters",
                div { class: "seg",
                    for opt in Range::ALL {
                        button { key: "{opt.label()}", class: if opt == r { "on" } else { "" }, title: "{opt.long()}", onclick: move |_| range.set(opt), "{opt.label()}" }
                    }
                }
                crate::ui::Select { value: sc.clone(), options: options.clone(), onchange: move |v: String| scope.set(v), wide: true }
                div { class: "grow" }
                if busy {
                    span { class: "us-loading", super::loaders::Orbit { size: 13 } if st.is_none() { "Analyzing {fmt_int(n_sessions as u64)} sessions…" } else { "Updating…" } }
                } else if report.as_ref().map(|r| r.estimated).unwrap_or(false) {
                    span { class: "us-note", title: "Some agents (e.g. Codex) only report a total per session. Those tokens are spread over the days the agent replied.",
                        Icon { i: I::Info, size: 12 } "Some days are estimated"
                    }
                }
            }

            PlansSection { agents: scope_plan, only_account: scope_acct, accounts: report.as_ref().map(|r| r.accounts.clone()).unwrap_or_default(), period: r.long() }

            match report {
                None => rsx! {
                    div { class: "us-head", h3 { "Usage" } }
                    div { class: "us-card", div { class: "us-empty",
                        if n_sessions == 0 { "No sessions yet. Run an agent or import your sessions to see usage here." }
                        else { "Reading your sessions… this takes a moment the first time and is instant afterwards." }
                    } }
                },
                Some(rep) => rsx! {
                    UsageBody { rep, range: r, metric: metric(), with_cache: with_cache(), table: table(),
                        on_metric: move |m| metric.set(m),
                        on_cache: move |v| with_cache.set(v),
                        on_table: move |v| table.set(v),
                    }
                },
            }
        }
    }
}

#[component]
fn UsageBody(
    rep: Report,
    range: Range,
    metric: Metric,
    with_cache: bool,
    table: bool,
    on_metric: EventHandler<Metric>,
    on_cache: EventHandler<bool>,
    on_table: EventHandler<bool>,
) -> Element {
    let app = use_app();
    let c = &rep.cur;
    let p = rep.prev.clone();
    let vs = match range {
        Range::All => String::new(),
        r => format!("vs prior {}", r.long().trim_start_matches("last ")),
    };
    let d = |cur: f64, prev: Option<f64>| prev.map(|pv| delta(cur, pv));
    let hit = c.hit_rate();
    let hit_prev = p.as_ref().and_then(|p| p.hit_rate());
    let hit_delta = match (hit, hit_prev) {
        (Some(a), Some(b)) => {
            let pts = a - b;
            Some(if pts.abs() < 0.5 { ("— 0 pts".to_string(), 0) } else if pts > 0.0 { (format!("▲ {pts:.0} pts"), 1) } else { (format!("▼ {:.0} pts", pts.abs()), -1) })
        }
        _ => None,
    };
    let priced_share = if c.priced + c.unpriced == 0 { 100.0 } else { c.priced as f64 / (c.priced + c.unpriced) as f64 * 100.0 };
    let cost_note = if priced_share < 99.5 { format!("covers {priced_share:.0}% of tokens") } else { String::new() };
    let total_tokens = rep.by_agent.iter().map(|a| a.tokens).sum::<u64>().max(1);
    let any_usage = c.tokens() > 0 || c.cost > 0.0 || c.sessions > 0;
    let period_days = rep.span_days.max(1);
    let chart_title = match (metric, rep.step) {
        (Metric::Tokens, 1) => "Tokens per day",
        (Metric::Tokens, 7) => "Tokens per week",
        (Metric::Tokens, _) => "Tokens per month",
        (Metric::Cost, 1) => "Spend per day",
        (Metric::Cost, 7) => "Spend per week",
        (Metric::Cost, _) => "Spend per month",
        (Metric::Sessions, 1) => "Active sessions per day",
        (Metric::Sessions, 7) => "Active sessions per week",
        (Metric::Sessions, _) => "Active sessions per month",
    };

    rsx! {
        div { class: "us-head", h3 { "Overview" } span { class: "meta", "{range.long()}" } }
        div { class: "us-kpis",
            KpiTile { label: "Tokens", value: fmt_tokens(c.tokens()), unit: String::new(),
                sub: format!("in {} · out {}", fmt_tokens(c.input), fmt_tokens(c.output)),
                delta: d(c.tokens() as f64, p.as_ref().map(|p| p.tokens() as f64)), vs: vs.clone(), tip: "Input + output tokens (cache reads are counted separately)." }
            KpiTile { label: "Est. cost", value: fmt_cost(c.cost), unit: String::new(),
                sub: cost_note,
                delta: d(c.cost, p.as_ref().map(|p| p.cost)), vs: vs.clone(), tip: "Cost the agent reported, else an estimate at public API list prices for known models. Subscriptions are billed by plan, not per token." }
            KpiTile { label: "Sessions", value: fmt_int(c.sessions as u64), unit: String::new(),
                sub: format!("{} prompts", fmt_int(c.prompts)),
                delta: d(c.sessions as f64, p.as_ref().map(|p| p.sessions as f64)), vs: vs.clone(), tip: "Sessions with any activity in the period." }
            KpiTile { label: "Active days", value: fmt_int(c.active_days as u64), unit: if range == Range::All { String::new() } else { format!("/ {period_days}") },
                sub: String::new(),
                delta: d(c.active_days as f64, p.as_ref().map(|p| p.active_days as f64)), vs: vs.clone(), tip: "Days you prompted an agent or it used tokens." }
            KpiTile { label: "Avg per session", value: fmt_tokens(c.avg()), unit: String::new(),
                sub: "tokens".to_string(),
                delta: d(c.avg() as f64, p.as_ref().map(|p| p.avg() as f64)), vs: vs.clone(), tip: "Input + output tokens per active session." }
            KpiTile { label: "Cache hit rate", value: hit.map(|h| format!("{h:.0}%")).unwrap_or_else(|| "—".into()), unit: String::new(),
                sub: format!("{} cached", fmt_tokens(c.cache_read)),
                delta: hit_delta, vs: vs.clone(), good_up: true, tip: "Share of input served from the prompt cache — higher is cheaper and faster." }
        }

        if !any_usage {
            div { class: "us-card", style: "margin-top: 12px;",
                div { class: "us-empty", "No activity in the {range.long()} for this selection. Try a longer range." }
            }
        } else {
            div { class: "us-head", h3 { "Activity" } }
            div { class: "us-card",
                div { class: "us-card-head",
                    span { class: "t", "{chart_title}" }
                    div { class: "grow" }
                    if metric == Metric::Tokens {
                        button { class: if with_cache { "us-chk on" } else { "us-chk" }, onclick: move |_| on_cache.call(!with_cache),
                            span { class: "box", if with_cache { Icon { i: I::Check, size: 10, stroke: 3.0 } } }
                            "Include cache"
                        }
                    }
                    div { class: "seg",
                        button { class: if metric == Metric::Tokens { "on" } else { "" }, onclick: move |_| on_metric.call(Metric::Tokens), "Tokens" }
                        button { class: if metric == Metric::Cost { "on" } else { "" }, onclick: move |_| on_metric.call(Metric::Cost), "Cost" }
                        button { class: if metric == Metric::Sessions { "on" } else { "" }, onclick: move |_| on_metric.call(Metric::Sessions), "Sessions" }
                    }
                    button { class: if table { "icon-btn active" } else { "icon-btn" }, title: if table { "Show chart" } else { "Show as table" },
                        onclick: move |_| on_table.call(!table),
                        Icon { i: if table { I::AudioLines } else { I::Lines }, size: 14 }
                    }
                }
                UsageChart { buckets: rep.buckets.clone(), step: rep.step, metric, with_cache, table }
            }

            div { class: "us-head", h3 { "Breakdown" } }
            div { class: "us-grid2",
                Breakdown { title: "By agent", rows: rep.by_agent.clone(), total: total_tokens, kind: "agent" }
                Breakdown { title: "By model", rows: rep.by_model.clone(), total: total_tokens, kind: "model" }
                Breakdown { title: "By workspace", rows: rep.by_repo.clone(), total: total_tokens, kind: "repo" }
                Heatmap { heat: rep.heat.clone() }
            }

            if !rep.top.is_empty() {
                div { class: "us-head", h3 { "Top sessions" } span { class: "meta", "by cost, then tokens" } }
                div { class: "us-card us-top",
                    div { class: "us-rows-head", style: "padding-top: 10px;",
                        span {}
                        span { "Session" }
                        span { class: "hide-sm", "Last active" }
                        span { "Tokens" }
                        span { "Cost" }
                    }
                    for s in rep.top.clone() {
                        {
                            let id = s.id.clone();
                            let when = crate::state::rel_time(s.updated);
                            rsx! {
                                div { key: "{s.id}", class: "us-row click", title: "Open", onclick: move |_| app.open_session(&id),
                                    span { class: "ic",
                                        match agent_logo(&s.agent) {
                                            Some(svg) => rsx! { Logo { svg, size: 16 } },
                                            None => rsx! { crate::ui::AgentGlyph { kind: s.agent.clone() } },
                                        }
                                    }
                                    div { class: "us-name",
                                        div { class: "t", dir: "auto", span { style: "overflow: hidden; text-overflow: ellipsis;", "{s.title}" } }
                                        div { class: "s", "{s.sub}" }
                                    }
                                    span { class: "num hide-sm", "{when}" }
                                    span { class: "num", "{fmt_tokens(s.tokens)}" }
                                    span { class: "num", "{fmt_cost(s.cost)}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn KpiTile(
    label: &'static str,
    value: String,
    unit: String,
    sub: String,
    delta: Option<(String, i8)>,
    vs: String,
    #[props(default = false)] good_up: bool,
    tip: &'static str,
) -> Element {
    let has_delta = delta.is_some();
    rsx! {
        div { class: "us-kpi", title: "{tip}",
            div { class: "l", "{label}" }
            div { class: "v", "{value}" if !unit.is_empty() { small { "{unit}" } } }
            div { class: "d",
                if let Some((txt, dir)) = delta {
                    b { class: if good_up && dir > 0 { "good" } else if good_up && dir < 0 { "bad" } else { "" }, "{txt}" }
                    " {vs}"
                } else if !sub.is_empty() {
                    "{sub}"
                } else {
                    "\u{00a0}"
                }
            }
            if !sub.is_empty() && has_delta {
                div { class: "d", "{sub}" }
            }
        }
    }
}

// ------------------------------------------------------------------ plans

#[component]
fn PlansSection(agents: Vec<AgentKind>, only_account: Option<String>, accounts: HashMap<String, (u64, f64, usize)>, period: &'static str) -> Element {
    let app = use_app();
    let state = QUOTAS.read().clone();
    let busy = *QUOTA_BUSY.read();
    let cfg = app.config.read().clone();
    let updated = state.as_ref().map(|s| crate::state::rel_time(s.at)).map(|t| if t == "now" { "Updated just now".to_string() } else { format!("Updated {t} ago") });
    rsx! {
        div { class: "us-head",
            h3 { "Plans & limits" }
            if let Some(u) = updated { span { class: "meta", "{u}" } }
            div { class: "grow" }
            button { class: "us-link", disabled: busy, title: "Check limits now", onclick: move |_| refresh_quotas(app, true),
                if busy { super::loaders::Orbit { size: 13 } } else { Icon { i: I::Refresh, size: 12 } }
                "Refresh"
            }
        }
        if agents.is_empty() {
            div { class: "us-card", div { class: "us-empty",
                "No subscription agents are enabled. Enable Claude Code, Codex, Cursor or Gemini in Settings → Agents to see their plan limits here."
            } }
        } else {
            div { class: "us-plans",
                match state {
                    None => rsx! {
                        for k in agents.iter() {
                            div { key: "{k.id()}", class: "us-plan skeleton",
                                div { class: "top",
                                    span { class: "lg", if let Some(svg) = agent_logo(k) { Logo { svg, size: 16 } } }
                                    div { class: "who", div { class: "n", "{k.display_name()}" } div { class: "e", "Checking limits…" } }
                                }
                                div { class: "us-meter", div { class: "track" } }
                            }
                        }
                    },
                    Some(s) => rsx! {
                        for q in s.list.iter().filter(|q| agents.contains(&q.agent)).filter(|q| only_account.is_none() || q.account_id == only_account).cloned() {
                            {
                                let acct = q.account_id.clone().or_else(|| cfg.default_account(&q.agent).map(|a| a.id.clone()));
                                let used = accounts.get(&account_key(&q.agent, acct.as_deref())).copied();
                                let key = format!("{}-{}", q.agent.id(), q.account_id.clone().unwrap_or_default());
                                rsx! { PlanCard { key: "{key}", q, used, period } }
                            }
                        }
                    },
                }
            }
        }
    }
}

#[component]
fn PlanCard(q: Quota, used: Option<(u64, f64, usize)>, period: &'static str) -> Element {
    let who = match (&q.email, q.account_id.is_some()) {
        (Some(e), true) => format!("{} · {e}", q.label),
        (Some(e), false) => e.clone(),
        (None, _) => q.label.clone(),
    };
    let foot = match used {
        Some((t, c, s)) => format!("{} tokens · {} · {} session{} · {period}", fmt_tokens(t), fmt_cost(c), s, if s == 1 { "" } else { "s" }),
        None => format!("No sessions in the {period}"),
    };
    let as_of = q.as_of.filter(|t| (Utc::now() - *t).num_minutes() > 10).map(|t| {
        let r = crate::state::rel_time(t);
        // rel_time gives "5m" / "3h" / "2d" for recent times and a date after a week.
        if r.ends_with(['m', 'h', 'd']) && r.len() <= 4 { format!("as of {r} ago") } else { format!("as of {r}") }
    });
    let link = q.link;
    rsx! {
        div { class: "us-plan",
            div { class: "top",
                span { class: "lg",
                    match agent_logo(&q.agent) {
                        Some(svg) => rsx! { Logo { svg, size: 16 } },
                        None => rsx! { crate::ui::AgentGlyph { kind: q.agent.clone() } },
                    }
                }
                div { class: "who",
                    div { class: "n", "{q.agent.display_name()}" if let Some(p) = &q.plan { span { class: "us-badge", "{p}" } } }
                    div { class: "e", title: "{who}", "{who}" }
                }
            }
            if q.windows.is_empty() {
                if let Some(n) = &q.note { div { class: "note", "{n}" } }
            } else {
                for w in q.windows.iter().cloned() {
                    Meter { key: "{w.name}", w }
                }
            }
            div { class: "foot",
                span { class: "grow", title: "{foot}", "{foot}" }
                if let Some(a) = as_of { span { title: "Codex reports limits with each run", "{a}" } }
                if let Some(l) = link {
                    button { class: "us-link", title: "Open the usage page", onclick: move |_| { let _ = open::that(l); },
                        Icon { i: I::ArrowUpRight, size: 12 }
                    }
                }
            }
        }
    }
}

#[component]
fn Meter(w: Window) -> Element {
    let pct = w.used_pct.clamp(0.0, 100.0);
    let sev = severity(w.used_pct);
    let reached = w.used_pct >= 100.0;
    rsx! {
        div { class: "us-meter {sev}",
            div { class: "row",
                span { class: "nm", "{w.name}" }
                if let Some(d) = &w.detail {
                    span { class: "rs", "{d}" }
                }
                if reached {
                    span { class: "hit", Icon { i: I::Alert, size: 11 } "Limit reached" }
                }
                if let Some(r) = w.resets_at {
                    span { class: "rs", "resets {fmt_reset(r)}" }
                }
                if w.detail.is_none() || w.used_pct > 0.0 {
                    span { class: "pct", "{w.used_pct:.0}%" }
                }
            }
            if w.detail.is_none() || w.used_pct > 0.0 {
                div { class: "track", div { class: "fill", style: "width: {pct:.1}%;" } }
            }
        }
    }
}

// ------------------------------------------------------------------ chart

fn bucket_label(b: &Bucket, step: i32, long: bool) -> String {
    let d = date_of(b.start);
    if step == 1 {
        return if long { d.format("%a, %b %-d").to_string() } else { d.format("%b %-d").to_string() };
    }
    let end = date_of(b.start + b.days - 1);
    if long {
        format!("{} – {}", d.format("%b %-d"), end.format("%b %-d, %Y"))
    } else if step >= 28 {
        end.format("%b %Y").to_string()
    } else {
        d.format("%b %-d").to_string()
    }
}

#[component]
fn UsageChart(buckets: Vec<Bucket>, step: i32, metric: Metric, with_cache: bool, table: bool) -> Element {
    let mut hover = use_signal(|| None::<usize>);
    // (name, color, value) per stacked series, baseline first.
    let series: Vec<(&'static str, &'static str)> = match metric {
        Metric::Tokens if with_cache => vec![("Input", "var(--s1)"), ("Output", "var(--s2)"), ("Cache", "var(--s3)")],
        Metric::Tokens => vec![("Input", "var(--s1)"), ("Output", "var(--s2)")],
        Metric::Cost => vec![("Est. cost", "var(--s1)")],
        Metric::Sessions => vec![("Sessions", "var(--s1)")],
    };
    let values = |b: &Bucket| -> Vec<f64> {
        match metric {
            Metric::Tokens if with_cache => vec![b.input as f64, b.output as f64, b.cache as f64],
            Metric::Tokens => vec![b.input as f64, b.output as f64],
            Metric::Cost => vec![b.cost],
            Metric::Sessions => vec![b.sessions as f64],
        }
    };
    let fmt = move |v: f64| -> String {
        match metric {
            Metric::Tokens => fmt_tokens(v.round() as u64),
            Metric::Cost => fmt_cost(v),
            Metric::Sessions => fmt_int(v.round() as u64),
        }
    };
    let fmt_axis = move |v: f64| -> String {
        match metric {
            Metric::Cost if v > 0.0 && v < 10.0 => format!("${v:.2}").trim_end_matches('0').trim_end_matches('.').to_string(),
            Metric::Cost => format!("${}", fmt_tokens(v.round() as u64)),
            _ => fmt(v),
        }
    };
    let rows: Vec<Vec<f64>> = buckets.iter().map(values).collect();
    let max = rows.iter().map(|r| r.iter().sum::<f64>()).fold(0.0, f64::max);
    let (top, tick) = nice_scale(max, metric == Metric::Sessions);
    let ticks: Vec<(f64, String, String)> = (0..=((top / tick).round() as usize))
        .map(|i| {
            let v = i as f64 * tick;
            (v, format!("bottom: {:.2}%;", v / top * 100.0), fmt_axis(v))
        })
        .collect();
    // Segment heights in px of the 190px plot, with a 2px surface gap.
    let cols: Vec<Vec<(usize, String)>> = rows
        .iter()
        .map(|vals| {
            vals.iter()
                .enumerate()
                .filter(|(_, v)| **v > 0.0)
                .map(|(si, v)| (si, format!("height: {:.2}px; background: {};", (v / top * 190.0 - 2.0).max(2.0), series[si].1)))
                .collect()
        })
        .collect();
    let n = buckets.len().max(1);
    let label_idx: BTreeSet<usize> = if n <= 8 { (0..n).collect() } else { (0..=5).map(|k| ((k as f64) * (n - 1) as f64 / 5.0).round() as usize).collect() };

    if table {
        let table_rows: Vec<(i32, [String; 7])> = buckets
            .iter()
            .rev()
            .filter(|b| b.input + b.output + b.cache > 0 || b.sessions > 0 || b.prompts > 0)
            .map(|b| {
                (b.start, [
                    bucket_label(b, step, true),
                    fmt_tokens(b.input),
                    fmt_tokens(b.output),
                    fmt_tokens(b.cache),
                    fmt_cost(b.cost),
                    b.sessions.to_string(),
                    b.prompts.to_string(),
                ])
            })
            .collect();
        return rsx! {
            div { class: "uc-table-wrap",
                table { class: "uc-table",
                    thead { tr {
                        th { if step == 1 { "Day" } else { "Period" } }
                        th { "Input" } th { "Output" } th { "Cache" } th { "Est. cost" } th { "Sessions" } th { "Prompts" }
                    } }
                    tbody {
                        for (start, cells) in table_rows {
                            tr { key: "{start}",
                                for (ci, c) in cells.into_iter().enumerate() {
                                    td { key: "{ci}", "{c}" }
                                }
                            }
                        }
                    }
                }
            }
        };
    }

    if max <= 0.0 {
        return rsx! { div { class: "us-empty-chart", "Nothing to show for this metric yet." } };
    }

    // Tooltip for the hovered column: values lead, series names follow.
    let tip = hover().filter(|i| *i < buckets.len()).map(|i| {
        let b = &buckets[i];
        let vals = &rows[i];
        let total: f64 = vals.iter().sum();
        let lines: Vec<(usize, String, &'static str, &'static str)> =
            vals.iter().copied().enumerate().rev().map(|(si, v)| (si, fmt(v), series[si].0, series[si].1)).collect();
        let counts = format!(
            "{} session{} · {} prompt{}",
            b.sessions,
            if b.sessions == 1 { "" } else { "s" },
            b.prompts,
            if b.prompts == 1 { "" } else { "s" }
        );
        let est = (metric != Metric::Cost && b.cost > 0.0).then(|| format!("{} est.", fmt_cost(b.cost)));
        let left = format!("left: {:.2}%;", (i as f64 + 0.5) / n as f64 * 100.0);
        let flip = i as f64 > n as f64 / 2.0;
        (left, flip, bucket_label(b, step, true), lines, (vals.len() > 1).then(|| fmt(total)), counts, est)
    });
    let x_labels: Vec<(usize, &'static str, String, String)> = label_idx
        .iter()
        .copied()
        .map(|i| {
            let (cls, pos) = if i == 0 && n > 1 {
                ("first", "left: 0;".to_string())
            } else if i + 1 == n && n > 1 {
                ("last", "left: 100%;".to_string())
            } else {
                ("", format!("left: {:.2}%;", (i as f64 + 0.5) / n as f64 * 100.0))
            };
            (i, cls, pos, bucket_label(&buckets[i], step, false))
        })
        .collect();

    rsx! {
        div { class: "uc",
            div { class: "uc-y",
                for (i, (_, pos, label)) in ticks.iter().cloned().enumerate() {
                    span { key: "{i}", style: "{pos}", "{label}" }
                }
            }
            div { class: "uc-plot",
                for (i, (v, pos, _)) in ticks.iter().cloned().enumerate() {
                    div { key: "g{i}", class: if v == 0.0 { "uc-grid base" } else { "uc-grid" }, style: "{pos}" }
                }
                div { class: "uc-cols", onmouseleave: move |_| hover.set(None),
                    for (i, segs) in cols.into_iter().enumerate() {
                        div { key: "{i}", class: if hover() == Some(i) { "uc-col on" } else { "uc-col" },
                            onmouseenter: move |_| hover.set(Some(i)),
                            div { class: "uc-bar",
                                for (si, style) in segs {
                                    div { key: "{si}", class: "uc-seg", style: "{style}" }
                                }
                            }
                        }
                    }
                }
                if let Some((left, flip, when, lines, total, counts, est)) = tip {
                    div { class: if flip { "uc-tip flip" } else { "uc-tip" }, style: "{left}",
                        div { class: "when", "{when}" }
                        for (si, val, name, color) in lines {
                            div { key: "{si}", class: "r",
                                span { class: "k", style: "background: {color};" }
                                b { "{val}" }
                                span { class: "n", "{name}" }
                            }
                        }
                        if let Some(t) = total {
                            div { class: "r", span { class: "k" } b { "{t}" } span { class: "n", "Total" } }
                        }
                        div { class: "sep" }
                        div { class: "r", span { class: "n", "{counts}" } }
                        if let Some(e) = est {
                            div { class: "r", span { class: "n", "{e}" } }
                        }
                    }
                }
            }
            div { class: "uc-x",
                for (i, cls, pos, label) in x_labels {
                    span { key: "{i}", class: "{cls}", style: "{pos}", "{label}" }
                }
            }
        }
        if series.len() > 1 {
            div { class: "uc-legend",
                for (name, color) in series.iter().copied() {
                    span { key: "{name}", i { style: "background: {color};" } "{name}" }
                }
            }
        } else {
            div { style: "height: 8px;" }
        }
    }
}

// ------------------------------------------------------------------ breakdowns

#[component]
fn Breakdown(title: &'static str, rows: Vec<Agg>, total: u64, kind: &'static str) -> Element {
    let max = rows.iter().map(|a| a.tokens).max().unwrap_or(0).max(1);
    rsx! {
        div { class: "us-card",
            div { class: "us-card-head", span { class: "t", "{title}" } }
            if rows.is_empty() {
                div { class: "us-empty", "No token usage in this period." }
            } else {
                div { class: "us-rows-head", span {} span {} span { "Tokens" } span { "Cost" } }
                for a in rows {
                    {
                        let pct = a.tokens as f64 / max as f64 * 100.0;
                        let share = a.tokens as f64 / total as f64 * 100.0;
                        let tip = format!("{} · {} tokens · {} cached · {} · {} session{}", a.label, fmt_int(a.tokens), fmt_tokens(a.cache), fmt_cost(a.cost), a.sessions, if a.sessions == 1 { "" } else { "s" });
                        let share_s = format!("{share:.0}%");
                        let width = format!("width: {pct:.1}%;");
                        let icon: Element = match kind {
                            "agent" => match (a.kind.as_ref().and_then(agent_logo), a.kind.clone()) {
                                (Some(svg), _) => rsx! { Logo { svg, size: 16 } },
                                (None, Some(k)) => rsx! { crate::ui::AgentGlyph { kind: k } },
                                (None, None) => rsx! { Icon { i: I::More, size: 14 } },
                            },
                            "model" => match (!a.key.starts_with('~')).then(|| model_logo(&a.key)).flatten() {
                                Some(svg) => rsx! { Logo { svg, size: 15 } },
                                None => rsx! { Icon { i: if a.key == "~others" { I::More } else { I::Cpu }, size: 14, stroke: 1.5 } },
                            },
                            _ => rsx! { Icon { i: if a.key == "~none" { I::Home } else if a.key == "~others" { I::More } else { I::Folder }, size: 14, stroke: 1.5 } },
                        };
                        rsx! {
                            div { key: "{a.key}", class: "us-row", title: "{tip}",
                                span { class: "ic", {icon} }
                                div { class: "us-name",
                                    div { class: "t", dir: "auto",
                                        span { style: "overflow: hidden; text-overflow: ellipsis;", "{a.label}" }
                                        span { class: "sh", "{share_s}" }
                                    }
                                    div { class: "us-bar", span { style: "{width}" } }
                                }
                                span { class: "num", "{fmt_tokens(a.tokens)}" }
                                span { class: "num", "{fmt_cost(a.cost)}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

fn hour_label(h: usize) -> String {
    match h {
        0 => "12 AM".into(),
        12 => "12 PM".into(),
        h if h < 12 => format!("{h} AM"),
        h => format!("{} PM", h - 12),
    }
}

#[component]
fn Heatmap(heat: Vec<u32>) -> Element {
    let max = heat.iter().copied().max().unwrap_or(0);
    let total: u32 = heat.iter().sum();
    let total_s = fmt_int(total as u64);
    let peak = heat.iter().enumerate().max_by_key(|(_, v)| **v).filter(|(_, v)| **v > 0).map(|(i, _)| format!("Busiest: {} around {}", DAYS[i / 24], hour_label(i % 24)));
    let level = |v: u32| -> &'static str {
        if v == 0 || max == 0 {
            return "c";
        }
        match ((v as f64 / max as f64) * 5.0).ceil() as u32 {
            0 | 1 => "c l1",
            2 => "c l2",
            3 => "c l3",
            4 => "c l4",
            _ => "c l5",
        }
    };
    rsx! {
        div { class: "us-card",
            div { class: "us-card-head",
                span { class: "t", "When you work" }
                span { class: "s", "{total_s} prompts by weekday and hour" }
            }
            div { class: "hm",
                span {}
                for h in 0..24usize {
                    span { key: "h{h}", class: "hl", if h % 6 == 0 { {hour_label(h)} } }
                }
                for (d, name) in DAYS.iter().enumerate() {
                    span { key: "d{d}", class: "dl", "{name}" }
                    for h in 0..24usize {
                        {
                            let v = heat.get(d * 24 + h).copied().unwrap_or(0);
                            let cls = level(v);
                            let tip = format!("{name} {} · {v} prompt{}", hour_label(h), if v == 1 { "" } else { "s" });
                            rsx! { span { key: "{d}-{h}", class: "{cls}", title: "{tip}" } }
                        }
                    }
                }
            }
            div { class: "hm-legend",
                span { class: "hm-peak", if let Some(p) = peak { "{p}" } }
                div { class: "grow" }
                "Less"
                span { class: "c", style: "background: color-mix(in srgb, var(--fg) 5%, transparent);" }
                span { class: "c", style: "background: var(--h1);" }
                span { class: "c", style: "background: var(--h2);" }
                span { class: "c", style: "background: var(--h3);" }
                span { class: "c", style: "background: var(--h4);" }
                span { class: "c", style: "background: var(--h5);" }
                "More"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use larrier_core::{SessionStatus, Usage, WorkspaceRef};

    fn sum(id: &str, agent: AgentKind, ws: Option<&str>, at: DateTime<Utc>) -> SessionSummary {
        SessionSummary {
            id: id.into(),
            title: id.into(),
            agent: agent.clone(),
            origin: agent,
            model: None,
            status: SessionStatus::Done,
            workspace: ws.map(WorkspaceRef::from_path),
            environment: "Local".into(),
            created_at: at,
            updated_at: at,
            active_at: None,
            preview: String::new(),
            added: 0,
            removed: 0,
            files_changed: 0,
            message_count: 2,
            pinned: false,
            archived: false,
            unread: false,
            external_turn: false,
            parent_id: None,
            usage: Usage::default(),
        }
    }

    fn st(uses: Vec<(i32, &str, u64, u64, u64, f64)>, prompts: Vec<(i32, u8, u8, u32)>) -> SessionStats {
        SessionStats {
            updated: 0,
            account: None,
            uses: uses
                .into_iter()
                .map(|(day, model, input, output, cache_read, cost)| Use { day, model: model.into(), input, output, cache_read, cache_write: 0, cost, replies: 1 })
                .collect(),
            prompts,
            estimated: false,
        }
    }

    #[test]
    fn formats() {
        assert_eq!(fmt_tokens(950), "950");
        assert_eq!(fmt_tokens(12_300), "12.3k");
        assert_eq!(fmt_tokens(1_000), "1k");
        assert_eq!(fmt_tokens(4_500_000), "4.5M");
        assert_eq!(fmt_tokens(1_200_000_000), "1.2B");
        assert_eq!(fmt_cost(0.0), "$0.00");
        assert_eq!(fmt_cost(0.004), "<$0.01");
        assert_eq!(fmt_cost(12.345), "$12.35");
        assert_eq!(fmt_cost(1234.4), "$1,234");
        assert_eq!(fmt_int(1_234_567), "1,234,567");
        assert_eq!(delta(120.0, 100.0), ("▲ 20%".to_string(), 1));
        assert_eq!(delta(50.0, 100.0), ("▼ 50%".to_string(), -1));
        assert_eq!(delta(5.0, 0.0).0, "new");
    }

    #[test]
    fn nice_axis() {
        assert_eq!(nice_scale(4_400_000_000.0, false), (6e9, 2e9));
        let (top, step) = nice_scale(87.0, false);
        assert!(top >= 87.0 && (top / step).round() <= 5.0);
        let (top, step) = nice_scale(3.0, true);
        assert!(top >= 3.0 && step >= 1.0);
        assert_eq!(nice_scale(0.0, false), (1.0, 0.25));
    }

    #[test]
    fn report_ranges_and_deltas() {
        let today = Local::now().date_naive();
        let t = day_num(today);
        let now = Utc::now();
        let list = vec![
            sum("a", AgentKind::ClaudeCode, Some("/r/web"), now),
            sum("b", AgentKind::Codex, None, now),
            sum("old", AgentKind::ClaudeCode, Some("/r/web"), now),
        ];
        let mut stats = HashMap::new();
        stats.insert("a".to_string(), st(vec![(t, "claude-sonnet-4-5", 1000, 500, 4000, 0.0), (t - 2, "claude-sonnet-4-5", 100, 50, 0, 0.0)], vec![(t, 10, 0, 2)]));
        stats.insert("b".to_string(), st(vec![(t - 1, "gpt-5", 2000, 100, 0, 0.0)], vec![(t - 1, 9, 1, 1)]));
        // Activity only in the previous 7-day window.
        stats.insert("old".to_string(), st(vec![(t - 9, "claude-sonnet-4-5", 300, 0, 0, 1.0)], vec![]));
        let cfg = Config::default();
        let r = build_report(&list, &stats, &cfg, Range::Week, "all", today);
        assert_eq!(r.buckets.len(), 7);
        assert_eq!(r.cur.tokens(), 1000 + 500 + 100 + 50 + 2000 + 100);
        assert_eq!(r.cur.sessions, 2);
        assert_eq!(r.cur.active_days, 3);
        assert_eq!(r.cur.prompts, 3);
        assert_eq!(r.buckets[6].input, 1000);
        assert_eq!(r.buckets[6].sessions, 1);
        let prev = r.prev.as_ref().unwrap();
        assert_eq!(prev.tokens(), 300);
        assert_eq!(prev.sessions, 1);
        assert!((prev.cost - 1.0).abs() < 1e-9);
        assert_eq!(r.by_agent[0].label, "Codex");
        assert_eq!(r.heat[10], 2); // Monday 10:00
        assert!(r.cur.cost > 0.0); // priced at list prices
        assert_eq!(r.top.len(), 2);
        // Scope: one agent only.
        let r2 = build_report(&list, &stats, &cfg, Range::Week, "agent:codex", today);
        assert_eq!(r2.cur.tokens(), 2100);
        assert_eq!(r2.cur.sessions, 1);
        // All time spans at least 30 days, daily.
        let r3 = build_report(&list, &stats, &cfg, Range::All, "all", today);
        assert_eq!(r3.step, 1);
        assert!(r3.buckets.len() >= 30);
        assert!(r3.prev.is_none());
    }

    #[test]
    fn hides_disabled_agents() {
        let today = Local::now().date_naive();
        let t = day_num(today);
        let list = vec![sum("a", AgentKind::Codex, None, Utc::now())];
        let mut stats = HashMap::new();
        stats.insert("a".to_string(), st(vec![(t, "gpt-5", 10, 10, 0, 0.0)], vec![]));
        let mut cfg = Config::default();
        cfg.agent_mut(&AgentKind::Codex).enabled = false;
        let r = build_report(&list, &stats, &cfg, Range::Week, "all", today);
        assert_eq!(r.cur.tokens(), 0);
        assert!(r.by_agent.is_empty());
    }
}
