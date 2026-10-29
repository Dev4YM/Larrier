//! Usage analytics from the session files: tokens and cost per day, model
//! and account, plus prompt activity by hour.
//!
//! The session index only knows each session's last active day, so this
//! reads every session once (a light, parts-skipping parse, in parallel)
//! and caches the result on disk keyed by `updated_at`. Later opens only
//! re-read sessions that changed.

use std::collections::{BTreeMap, HashMap};
use std::io::BufRead;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use chrono::{DateTime, Datelike, Local, NaiveDate, Timelike, Utc};
use larrier_core::{SessionSummary, Usage};
use serde::{Deserialize, Serialize};

use crate::state::store;

const CACHE_VERSION: u32 = 1;

/// Tokens of one model on one local day within a session.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Use {
    /// Local day (`NaiveDate::num_days_from_ce`).
    pub day: i32,
    pub model: String,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    /// Cost the agent reported (0 when it reports none).
    pub cost: f64,
    pub replies: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct SessionStats {
    /// `updated_at` in milliseconds — the cache key.
    pub updated: i64,
    pub account: Option<String>,
    pub uses: Vec<Use>,
    /// Prompts sent: (day, local hour, weekday 0 = Monday, count).
    pub prompts: Vec<(i32, u8, u8, u32)>,
    /// Part of the usage was spread over active days because the agent
    /// only reports a session total (e.g. Codex).
    pub estimated: bool,
}

/// All sessions' stats; compared by generation so memos stay cheap.
#[derive(Clone)]
pub struct StatsSet {
    pub map: Arc<HashMap<String, SessionStats>>,
    pub generation: u64,
}

impl PartialEq for StatsSet {
    fn eq(&self, other: &Self) -> bool {
        self.generation == other.generation
    }
}

pub fn day_num(d: NaiveDate) -> i32 {
    d.num_days_from_ce()
}

pub fn date_of(n: i32) -> NaiveDate {
    NaiveDate::from_num_days_from_ce_opt(n).unwrap_or_default()
}

fn local(t: DateTime<Utc>) -> DateTime<Local> {
    t.with_timezone(&Local)
}

/// The subset of an LSF JSONL line we need; everything else (parts, tool
/// output…) is skipped by serde without being materialized.
#[derive(Deserialize, Default)]
struct Line {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    usage: Option<Usage>,
    #[serde(default)]
    account_id: Option<String>,
}

fn sub(a: u64, b: u64) -> u64 {
    a.saturating_sub(b)
}

/// Read one session file into per-day stats.
pub fn compute(path: &std::path::Path, summary: &SessionSummary) -> SessionStats {
    let mut out = SessionStats { updated: summary.updated_at.timestamp_millis(), ..Default::default() };
    let Ok(file) = std::fs::File::open(path) else { return out };
    let reader = std::io::BufReader::with_capacity(256 * 1024, file);

    let mut header_usage = Usage::default();
    let mut header_model: Option<String> = summary.model.clone();
    let mut last_at: DateTime<Utc> = summary.updated_at;
    let mut uses: BTreeMap<(i32, String), Use> = BTreeMap::new();
    let mut prompts: BTreeMap<(i32, u8, u8), u32> = BTreeMap::new();
    let mut reply_days: BTreeMap<i32, u32> = BTreeMap::new();
    let mut msg_total = Usage::default();

    for (i, line) in reader.lines().enumerate() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(l) = serde_json::from_str::<Line>(&line) else { continue };
        if i == 0 || l.kind.as_deref() == Some("session") {
            if let Some(u) = l.usage {
                header_usage = u;
            }
            if l.model.is_some() {
                header_model = l.model;
            }
            out.account = l.account_id;
            if let Some(t) = l.updated_at {
                last_at = t;
            }
            continue;
        }
        let Some(at) = l.created_at else { continue };
        let lt = local(at);
        let day = day_num(lt.date_naive());
        match l.role.as_deref() {
            Some("user") => {
                // Skip tool results and other non-prompt user lines.
                if !line.contains("\"type\":\"text\"") && !line.contains("\"type\":\"image\"") {
                    continue;
                }
                *prompts.entry((day, lt.hour() as u8, lt.weekday().num_days_from_monday() as u8)).or_default() += 1;
            }
            Some("assistant") => {
                *reply_days.entry(day).or_default() += 1;
                let model = l.model.clone().or_else(|| header_model.clone()).unwrap_or_default();
                let e = uses.entry((day, model.clone())).or_insert_with(|| Use { day, model, ..Default::default() });
                e.replies += 1;
                if let Some(u) = &l.usage {
                    e.input += u.input_tokens;
                    e.output += u.output_tokens;
                    e.cache_read += u.cache_read_tokens;
                    e.cache_write += u.cache_write_tokens;
                    e.cost += u.cost_usd.unwrap_or(0.0);
                    msg_total.add(u);
                }
            }
            _ => {}
        }
    }

    // Usage the session reports but no message carries (agents that only
    // report totals): spread it over the days the agent replied.
    let rem = Usage {
        input_tokens: sub(header_usage.input_tokens, msg_total.input_tokens),
        output_tokens: sub(header_usage.output_tokens, msg_total.output_tokens),
        cache_read_tokens: sub(header_usage.cache_read_tokens, msg_total.cache_read_tokens),
        cache_write_tokens: sub(header_usage.cache_write_tokens, msg_total.cache_write_tokens),
        reasoning_tokens: 0,
        cost_usd: Some((header_usage.cost_usd.unwrap_or(0.0) - msg_total.cost_usd.unwrap_or(0.0)).max(0.0)),
        ..Default::default()
    };
    let rem_cost = rem.cost_usd.unwrap_or(0.0);
    if rem.total_tokens() > 0 || rem_cost > 0.0001 {
        out.estimated = msg_total.total_tokens() == 0;
        if reply_days.is_empty() {
            reply_days.insert(day_num(local(last_at).date_naive()), 1);
        }
        let weight: u32 = reply_days.values().sum();
        let model = header_model.clone().unwrap_or_default();
        let n = reply_days.len();
        let mut acc = (0u64, 0u64, 0u64, 0u64, 0f64);
        for (k, (day, w)) in reply_days.iter().enumerate() {
            let last = k + 1 == n;
            let f = *w as f64 / weight.max(1) as f64;
            let part = |total: u64, used: u64| if last { total - used } else { (total as f64 * f).round() as u64 };
            let (i, o, cr, cw) = (
                part(rem.input_tokens, acc.0).min(rem.input_tokens - acc.0),
                part(rem.output_tokens, acc.1).min(rem.output_tokens - acc.1),
                part(rem.cache_read_tokens, acc.2).min(rem.cache_read_tokens - acc.2),
                part(rem.cache_write_tokens, acc.3).min(rem.cache_write_tokens - acc.3),
            );
            let c = if last { (rem_cost - acc.4).max(0.0) } else { rem_cost * f };
            acc = (acc.0 + i, acc.1 + o, acc.2 + cr, acc.3 + cw, acc.4 + c);
            let e = uses.entry((*day, model.clone())).or_insert_with(|| Use { day: *day, model: model.clone(), ..Default::default() });
            e.input += i;
            e.output += o;
            e.cache_read += cr;
            e.cache_write += cw;
            e.cost += c;
        }
    }
    out.uses = uses.into_values().collect();
    out.prompts = prompts.into_iter().map(|((d, h, w), n)| (d, h, w, n)).collect();
    out
}

#[derive(Serialize, Deserialize, Default)]
struct DiskCache {
    version: u32,
    sessions: HashMap<String, SessionStats>,
}

fn cache_path() -> PathBuf {
    store().root().join("cache").join("usage-stats.json")
}

fn memory() -> &'static Mutex<Option<HashMap<String, SessionStats>>> {
    static M: OnceLock<Mutex<Option<HashMap<String, SessionStats>>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(None))
}

/// Stats for every session in `list` (blocking — call from a worker
/// thread). Unchanged sessions come from the cache.
pub fn load_all(list: &[SessionSummary]) -> Arc<HashMap<String, SessionStats>> {
    let mut guard = memory().lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        let disk = std::fs::read(cache_path())
            .ok()
            .and_then(|b| serde_json::from_slice::<DiskCache>(&b).ok())
            .filter(|c| c.version == CACHE_VERSION)
            .map(|c| c.sessions)
            .unwrap_or_default();
        *guard = Some(disk);
    }
    let cache = guard.as_mut().unwrap();
    let stale: Vec<&SessionSummary> = list
        .iter()
        .filter(|s| cache.get(&s.id).map(|c| c.updated != s.updated_at.timestamp_millis()).unwrap_or(true))
        .collect();
    let changed = !stale.is_empty();
    if changed {
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 8);
        let chunk = stale.len().div_ceil(threads).max(1);
        let fresh: Vec<(String, SessionStats)> = std::thread::scope(|scope| {
            let handles: Vec<_> = stale
                .chunks(chunk)
                .map(|part| {
                    scope.spawn(move || {
                        part.iter()
                            .map(|s| (s.id.clone(), compute(&store().session_path(&s.id), s)))
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
        });
        for (id, st) in fresh {
            cache.insert(id, st);
        }
    }
    // Forget sessions that no longer exist.
    let before = cache.len();
    let ids: std::collections::HashSet<&str> = list.iter().map(|s| s.id.as_str()).collect();
    cache.retain(|k, _| ids.contains(k.as_str()));
    if changed || cache.len() != before {
        let disk = DiskCache { version: CACHE_VERSION, sessions: cache.clone() };
        if let Ok(bytes) = serde_json::to_vec(&disk) {
            let p = cache_path();
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let tmp = p.with_extension("json.tmp");
            if std::fs::write(&tmp, bytes).is_ok() {
                let _ = std::fs::rename(&tmp, &p);
            }
        }
    }
    Arc::new(cache.clone())
}

// ------------------------------------------------------------------ pricing

/// Public API list prices per million tokens: (input, output, cache read,
/// cache write). Only used when the agent reports no cost itself.
pub fn price(model: &str) -> Option<(f64, f64, f64, f64)> {
    if let Some(p) = larrier_core::models::price(model) {
        return Some((p.input, p.output, p.cache_read.unwrap_or(p.input * 0.1), p.cache_write.unwrap_or(p.input * 1.25)));
    }
    let m = model.to_lowercase();
    let has = |s: &str| m.contains(s);
    if has("opus") {
        // Opus 4.5 and later are priced lower than earlier Opus models.
        let new = ["opus-4-5", "opus-4.5", "opus-4-6", "opus-4.6", "opus-5", "opus-4-7"].iter().any(|v| m.contains(v));
        return Some(if new { (5.0, 25.0, 0.5, 6.25) } else { (15.0, 75.0, 1.5, 18.75) });
    }
    if has("sonnet") {
        return Some((3.0, 15.0, 0.3, 3.75));
    }
    if has("haiku") {
        return Some(if has("haiku-4") || has("haiku-5") { (1.0, 5.0, 0.1, 1.25) } else if has("3-5") || has("3.5") { (0.8, 4.0, 0.08, 1.0) } else { (0.25, 1.25, 0.03, 0.3) });
    }
    if has("gpt-5") || has("codex") {
        if has("nano") {
            return Some((0.05, 0.4, 0.005, 0.0));
        }
        if has("mini") {
            return Some((0.25, 2.0, 0.025, 0.0));
        }
        return Some((1.25, 10.0, 0.125, 0.0));
    }
    if has("gpt-4.1") {
        return Some(if has("nano") { (0.1, 0.4, 0.025, 0.0) } else if has("mini") { (0.4, 1.6, 0.1, 0.0) } else { (2.0, 8.0, 0.5, 0.0) });
    }
    if has("gpt-4o") {
        return Some(if has("mini") { (0.15, 0.6, 0.075, 0.0) } else { (2.5, 10.0, 1.25, 0.0) });
    }
    if has("o4-mini") {
        return Some((1.1, 4.4, 0.275, 0.0));
    }
    if m.starts_with("o3") || has("/o3") {
        return Some((2.0, 8.0, 0.5, 0.0));
    }
    if has("gemini") {
        if has("flash-lite") {
            return Some((0.1, 0.4, 0.025, 0.0));
        }
        if has("flash") {
            return Some((0.3, 2.5, 0.075, 0.0));
        }
        if has("pro") {
            return Some((1.25, 10.0, 0.31, 0.0));
        }
    }
    None
}

/// Reported cost, else an estimate at list prices (None when unknown).
pub fn use_cost(u: &Use) -> Option<f64> {
    if u.cost > 0.0 {
        return Some(u.cost);
    }
    let (i, o, cr, cw) = price(&u.model)?;
    Some((u.input as f64 * i + u.output as f64 * o + u.cache_read as f64 * cr + u.cache_write as f64 * cw) / 1e6)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prices() {
        assert_eq!(price("claude-opus-4-1-20250805").unwrap().0, 15.0);
        assert_eq!(price("claude-opus-4-5").unwrap().0, 5.0);
        assert_eq!(price("claude-sonnet-4-5").unwrap().1, 15.0);
        assert_eq!(price("gpt-5-codex").unwrap().0, 1.25);
        assert!(price("some-unknown-model").is_none());
        let u = Use { model: "claude-sonnet-4-5".into(), input: 1_000_000, output: 1_000_000, ..Default::default() };
        assert!((use_cost(&u).unwrap() - 18.0).abs() < 1e-9);
        let reported = Use { cost: 2.0, ..u };
        assert_eq!(use_cost(&reported), Some(2.0));
    }

    /// `LARRIER_HOME=<store> cargo test -p larrier-app real_store_timing -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_store_timing() {
        let list = store().list();
        *memory().lock().unwrap() = None;
        let _ = std::fs::remove_file(cache_path());
        let t = std::time::Instant::now();
        let m = load_all(&list);
        let uses: usize = m.values().map(|s| s.uses.len()).sum();
        eprintln!("cold: {} sessions → {} stats ({uses} day×model rows) in {:?}", list.len(), m.len(), t.elapsed());
        *memory().lock().unwrap() = None;
        let t = std::time::Instant::now();
        let _ = load_all(&list);
        eprintln!("from disk cache: {:?}", t.elapsed());
        let t = std::time::Instant::now();
        let m = load_all(&list);
        eprintln!("from memory: {:?}", t.elapsed());
        let today = chrono::Local::now().date_naive();
        let t = std::time::Instant::now();
        let r = crate::ui::usage::build_report(&list, &m, &larrier_core::Config::default(), crate::ui::usage::Range::All, "all", today);
        eprintln!("report (all time): {:?} · {} buckets · tokens {} · cost ${:.2} · sessions {}", t.elapsed(), r.buckets.len(), r.cur.tokens(), r.cur.cost, r.cur.sessions);
    }

    #[test]
    fn compute_spreads_session_totals() {
        let dir = std::env::temp_dir().join(format!("larrier-usage-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("s.jsonl");
        let lines = [
            r#"{"lsf":"1","kind":"session","id":"s","model":"gpt-5","usage":{"input_tokens":1000,"output_tokens":100,"cache_read_tokens":500}}"#,
            r#"{"kind":"message","id":"u1","role":"user","created_at":"2026-10-01T10:00:00Z","parts":[{"type":"text","text":"hi"}]}"#,
            r#"{"kind":"message","id":"a1","role":"assistant","created_at":"2026-10-01T10:00:05Z","parts":[{"type":"text","text":"yo"}]}"#,
            r#"{"kind":"message","id":"u2","role":"user","created_at":"2026-10-03T10:00:00Z","parts":[{"type":"text","text":"again"}]}"#,
            r#"{"kind":"message","id":"a2","role":"assistant","created_at":"2026-10-03T10:00:05Z","parts":[{"type":"text","text":"ok"}]}"#,
        ];
        std::fs::write(&p, lines.join("\n")).unwrap();
        let now = Utc::now();
        let sum = SessionSummary {
            id: "s".into(),
            title: "s".into(),
            agent: larrier_core::AgentKind::Codex,
            origin: larrier_core::AgentKind::Codex,
            model: None,
            status: larrier_core::SessionStatus::Done,
            workspace: None,
            environment: "Local".into(),
            created_at: now,
            updated_at: now,
            preview: String::new(),
            added: 0,
            removed: 0,
            files_changed: 0,
            message_count: 4,
            pinned: false,
            archived: false,
            unread: false,
            external_turn: false,
            parent_id: None,
            usage: Usage::default(),
        };
        let st = compute(&p, &sum);
        assert!(st.estimated);
        assert_eq!(st.uses.iter().map(|u| u.input).sum::<u64>(), 1000);
        assert_eq!(st.uses.iter().map(|u| u.output).sum::<u64>(), 100);
        assert_eq!(st.uses.iter().map(|u| u.cache_read).sum::<u64>(), 500);
        assert_eq!(st.uses.len(), 2);
        assert_eq!(st.prompts.iter().map(|p| p.3).sum::<u32>(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
