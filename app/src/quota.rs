//! Plan limits per agent account: how much of each rate-limit window is
//! used and when it resets.
//!
//! - Codex: the newest `rate_limits` snapshot in its session logs.
//! - Claude Code: Anthropic's OAuth usage endpoint, with the access token
//!   the CLI keeps in `.credentials.json` (sent over stdin to `curl`, never
//!   logged, shown or stored). A setup token can't ask it (it lacks the
//!   profile scope): its windows are the ones Claude Code last reported
//!   while a chat ran on it, kept in `cache/plan-limits.json`.
//! - Cursor / Gemini: the signed-in account, when the CLI records it.
//!
//! Agent folders are only ever read.

use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, TimeZone, Utc};
use lary_core::{AccountAuth, AgentKind, Config};

/// One rate-limit window ("5-hour", "Weekly").
#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub name: String,
    pub used_pct: f64,
    pub resets_at: Option<DateTime<Utc>>,
    /// Extra detail, e.g. "$12 of $50" for usage-based credit.
    pub detail: Option<String>,
}

/// What we know about one account's plan.
#[derive(Clone, Debug, PartialEq)]
pub struct Quota {
    pub agent: AgentKind,
    /// Lary account id (None = the CLI's default login).
    pub account_id: Option<String>,
    pub label: String,
    pub email: Option<String>,
    pub plan: Option<String>,
    pub windows: Vec<Window>,
    /// When the limits were observed (Codex reports them per run).
    pub as_of: Option<DateTime<Utc>>,
    /// Shown when there are no windows.
    pub note: Option<String>,
    pub link: Option<&'static str>,
}

impl Quota {
    fn new(agent: AgentKind, account_id: Option<String>, label: String) -> Self {
        Quota { agent, account_id, label, email: None, plan: None, windows: Vec::new(), as_of: None, note: None, link: None }
    }
}

/// Agents whose plans we can describe.
pub const PLAN_AGENTS: [AgentKind; 4] = [AgentKind::ClaudeCode, AgentKind::Codex, AgentKind::Cursor, AgentKind::Gemini];

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn cache() -> &'static Mutex<Option<(Instant, Vec<Quota>)>> {
    static C: OnceLock<Mutex<Option<(Instant, Vec<Quota>)>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

/// Fetch limits for every enabled plan agent's accounts (blocking). Reuses
/// results younger than five minutes unless `force`.
#[cfg(test)]
pub fn fetch_all(cfg: &Config, agents: &[AgentKind], force: bool) -> Vec<Quota> {
    fetch_all_asking(cfg, agents, force, false)
}

/// Fetch limits for every enabled plan agent's accounts (blocking), reusing
/// results younger than five minutes unless `force`; `ask_tokens`: also ask the plan for its limits now on
/// setup-token accounts (a call of its own each, see [`token_now`]), as when
/// the user refreshes or opens a plan panel.
pub fn fetch_all_asking(cfg: &Config, agents: &[AgentKind], force: bool, ask_tokens: bool) -> Vec<Quota> {
    if !force {
        if let Ok(g) = cache().lock() {
            if let Some((at, v)) = g.as_ref() {
                if at.elapsed() < Duration::from_secs(300) {
                    return v.clone();
                }
            }
        }
    }
    let mut jobs: Vec<(AgentKind, Option<String>, String, Source)> = Vec::new();
    for kind in agents {
        let accounts = cfg.accounts_for(kind);
        if accounts.is_empty() {
            jobs.push((kind.clone(), None, "Current sign-in".into(), Source::SignIn(None)));
        }
        for a in accounts {
            let source = match &a.auth {
                AccountAuth::ConfigDir { path } => Source::SignIn(Some(PathBuf::from(path))),
                AccountAuth::SystemLogin => Source::SignIn(None),
                // A subscription's token, not a key: its plan has limits.
                AccountAuth::Env { vars } if a.auth.is_setup_token() => Source::Token(
                    vars.iter().find(|(k, _)| k == "CLAUDE_CODE_OAUTH_TOKEN").map(|(_, v)| lary_core::secrets::resolve(v)).filter(|t| ask_tokens && !t.is_empty()),
                ),
                AccountAuth::Api(_) | AccountAuth::ApiKey { .. } | AccountAuth::Env { .. } => Source::Api,
            };
            let label = if a.label.trim().is_empty() { a.auth.label().to_string() } else { a.label.clone() };
            jobs.push((kind.clone(), Some(a.id.clone()), label, source));
        }
    }
    let out: Vec<Quota> = std::thread::scope(|s| {
        let handles: Vec<_> = jobs
            .into_iter()
            .map(|(kind, id, label, source)| {
                let acc_plan = id.as_deref().and_then(|i| cfg.account(i)).and_then(|a| a.plan.clone());
                let acc_email = id.as_deref().and_then(|i| cfg.account(i)).and_then(|a| a.email.clone());
                s.spawn(move || {
                    let mut q = Quota::new(kind.clone(), id, label);
                    match source {
                        Source::Api => {
                            q.plan = Some("API key".into());
                            q.note = Some("Pay as you go — no plan limits. Spend is tracked below.".into());
                        }
                        Source::Token(now) => {
                            if let Some(t) = now {
                                if let Err(e) = token_now(&q, &t) {
                                    tracing::warn!("plan limits: setup token: {}", match e { HttpError::Auth => "not accepted".to_string(), HttpError::Other(e) => e });
                                }
                            }
                            token(&mut q)
                        }
                        Source::SignIn(dir) => match kind {
                            AgentKind::Codex => codex(&mut q, dir.as_deref()),
                            AgentKind::ClaudeCode => claude(&mut q, dir.as_deref()),
                            AgentKind::Cursor => cursor(&mut q),
                            AgentKind::Gemini => gemini(&mut q, dir.as_deref()),
                            _ => q.note = Some("Plan limits are not available for this agent.".into()),
                        },
                    }
                    if q.plan.is_none() {
                        q.plan = acc_plan;
                    }
                    if q.email.is_none() {
                        q.email = acc_email;
                    }
                    q
                })
            })
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    });
    if let Ok(mut g) = cache().lock() {
        *g = Some((Instant::now(), out.clone()));
    }
    out
}

/// Where an account's plan limits come from.
enum Source {
    /// The agent's own sign-in (its folder, or this computer's).
    SignIn(Option<PathBuf>),
    /// Claude Code's setup token: a subscription, read from its runs; with
    /// the token when its plan is to be asked now.
    Token(Option<String>),
    /// A key: pay as you go, no plan.
    Api,
}

/// Windows in a steady order: 5-hour, weekly, each model's week, the rest,
/// extra usage last.
pub fn sort_windows(w: &mut [Window]) {
    let rank = |n: &str| match n {
        "5-hour" => 0,
        "Weekly" => 1,
        "Extra usage" => 9,
        n if n.starts_with("Weekly · ") => 2,
        _ => 5,
    };
    w.sort_by_key(|w| rank(&w.name));
}

// ------------------------------------------------------------------ seen in runs

/// A window as an agent last reported it while running on an account.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct SeenWindow {
    name: String,
    used_pct: f64,
    /// Unix seconds.
    #[serde(default)]
    resets_at: Option<i64>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct Seen {
    /// Unix seconds of the last report.
    at: i64,
    windows: Vec<SeenWindow>,
}

fn seen_path() -> PathBuf {
    crate::state::store().root().join("cache").join("plan-limits.json")
}

fn seen_key(agent: &AgentKind, account_id: Option<&str>) -> String {
    format!("{}@{}", agent.id(), account_id.unwrap_or("default"))
}

fn seen_map() -> &'static Mutex<Option<std::collections::BTreeMap<String, Seen>>> {
    static M: OnceLock<Mutex<Option<std::collections::BTreeMap<String, Seen>>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(None))
}

/// A window an agent reported while running on an account (Claude Code's
/// `rate_limit_event`): kept, so the account's plan shows it
/// where it can't be asked for (a setup token), restarts included.
pub fn remember(agent: &AgentKind, account_id: Option<&str>, window: &str, used_pct: f64, resets_at: Option<DateTime<Utc>>) {
    let Ok(mut g) = seen_map().lock() else { return };
    let map = g.get_or_insert_with(|| std::fs::read_to_string(seen_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default());
    let now = Utc::now().timestamp();
    let e = map.entry(seen_key(agent, account_id)).or_insert_with(|| Seen { at: now, windows: Vec::new() });
    e.at = now;
    let resets_at = resets_at.map(|t| t.timestamp());
    match e.windows.iter_mut().find(|w| w.name == window) {
        Some(w) => {
            w.used_pct = used_pct;
            w.resets_at = resets_at.or(w.resets_at);
        }
        None => e.windows.push(SeenWindow { name: window.to_string(), used_pct, resets_at }),
    }
    let Ok(text) = serde_json::to_string_pretty(&*map) else { return };
    drop(g);
    let p = seen_path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(p, text);
}

/// The windows last reported for an account, and when. A window whose reset
/// has passed since starts again at nothing used.
fn seen(agent: &AgentKind, account_id: Option<&str>) -> Option<(DateTime<Utc>, Vec<Window>)> {
    let mut g = seen_map().lock().ok()?;
    let map = g.get_or_insert_with(|| std::fs::read_to_string(seen_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default());
    let s = map.get(&seen_key(agent, account_id))?;
    Some((Utc.timestamp_opt(s.at, 0).single()?, seen_windows(s, Utc::now())))
}

fn seen_windows(s: &Seen, now: DateTime<Utc>) -> Vec<Window> {
    let mut windows: Vec<Window> = s
        .windows
        .iter()
        .map(|w| {
            let resets = w.resets_at.and_then(|t| Utc.timestamp_opt(t, 0).single());
            match resets {
                Some(r) if r <= now => Window { name: w.name.clone(), used_pct: 0.0, resets_at: None, detail: None },
                _ => Window { name: w.name.clone(), used_pct: w.used_pct, resets_at: resets, detail: None },
            }
        })
        .collect();
    sort_windows(&mut windows);
    windows
}

/// Claude Code on a setup token (`claude setup-token`): a subscription
/// whose limits can't be asked for with the token (the usage endpoint wants
/// a sign-in's profile scope), so they're the ones Claude Code reported in
/// this account's latest chat.
fn token(q: &mut Quota) {
    q.plan = Some("Setup token".into());
    q.link = Some("https://claude.ai/settings/usage");
    match seen(&q.agent, q.account_id.as_deref()) {
        Some((at, windows)) if !windows.is_empty() => {
            q.windows = windows;
            q.as_of = Some(at);
        }
        _ => q.note = Some("Its limits show here once a chat runs on this account.".into()),
    }
}

/// A setup token's plan limits now. The usage endpoint won't take the token,
/// but every answer to it carries the plan's windows (how much of each is
/// used, when it resets), as Claude Code reads them on its own calls: so the
/// smallest call there is (Haiku, one token of reply) is made, and its
/// windows kept like those a run reports. The token goes to `curl` over stdin.
fn token_now(q: &Quota, token: &str) -> Result<(), HttpError> {
    let body = r#"{"model":"claude-haiku-5-5","max_tokens":1,"system":"You are Claude Code, Anthropic's official CLI for Claude.","messages":[{"role":"user","content":"hi"}]}"#;
    let mut cmd = std::process::Command::new("curl");
    cmd.args(["-sS", "--max-time", "20", "-K", "-"]);
    cmd.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().map_err(|_| HttpError::Other("curl not found".into()))?;
    {
        let cfg = format!(
            "url = \"https://api.anthropic.com/v1/messages\"\nheader = \"Authorization: Bearer {token}\"\nheader = \"anthropic-version: 2023-06-01\"\nheader = \"anthropic-beta: oauth-2025-04-20\"\nheader = \"content-type: application/json\"\nheader = \"User-Agent: lary\"\ninclude\ndata = \"{}\"\nwrite-out = \"\\n%{{http_code}}\"\n",
            body.replace('\\', "\\\\").replace('"', "\\\"")
        );
        let Some(mut stdin) = child.stdin.take() else { return Err(HttpError::Other("no stdin".into())) };
        let _ = stdin.write_all(cfg.as_bytes());
    }
    let mut out = String::new();
    if let Some(mut so) = child.stdout.take() {
        let _ = so.read_to_string(&mut out);
    }
    let _ = child.wait();
    let code: u16 = out.rsplit_once('\n').map(|(_, c)| c).unwrap_or(&out).trim().parse().unwrap_or(0);
    match code {
        200..=299 => {}
        401 | 403 => return Err(HttpError::Auth),
        0 => return Err(HttpError::Other("offline".into())),
        c => return Err(HttpError::Other(format!("HTTP {c}"))),
    }
    let windows = unified_windows(&out);
    if windows.is_empty() {
        return Err(HttpError::Other("no limits in the answer".into()));
    }
    for (name, pct, resets) in windows {
        remember(&q.agent, q.account_id.as_deref(), &name, pct, resets);
    }
    Ok(())
}

/// The plan's windows in an answer's headers
/// (`anthropic-ratelimit-unified-5h-utilization: 0.25`, `…-5h-reset: <unix>`):
/// name, share used (percent) and reset.
fn unified_windows(head: &str) -> Vec<(String, f64, Option<DateTime<Utc>>)> {
    let mut seen: Vec<(String, Option<f64>, Option<DateTime<Utc>>)> = Vec::new();
    for line in head.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        let k = k.trim().to_ascii_lowercase();
        let Some(rest) = k.strip_prefix("anthropic-ratelimit-unified-") else { continue };
        let (win, what) = match rest.rsplit_once('-') {
            Some((w, what @ ("utilization" | "reset"))) => (w.to_string(), what),
            _ => continue,
        };
        let name = match win.as_str() {
            "5h" => "5-hour".to_string(),
            "7d" => "Weekly".to_string(),
            w => match w.strip_prefix("7d_") {
                Some(model) => format!("Weekly · {}", title_case(model)),
                None => continue,
            },
        };
        let i = match seen.iter().position(|(n, ..)| *n == name) {
            Some(i) => i,
            None => {
                seen.push((name, None, None));
                seen.len() - 1
            }
        };
        let v = v.trim();
        if what == "utilization" {
            seen[i].1 = v.parse::<f64>().ok().map(|f| f * 100.0);
        } else {
            seen[i].2 = v.parse::<i64>().ok().and_then(|t| Utc.timestamp_opt(t, 0).single());
        }
    }
    seen.into_iter().filter_map(|(n, p, r)| Some((n, p?, r))).collect()
}

fn title_case(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn window_name(minutes: u64) -> String {
    match minutes {
        300 => "5-hour".into(),
        10080 => "Weekly".into(),
        1440 => "Daily".into(),
        m if m % 1440 == 0 => format!("{}-day", m / 1440),
        m if m % 60 == 0 => format!("{}-hour", m / 60),
        m => format!("{m}-min"),
    }
}

// ------------------------------------------------------------------ Codex

fn codex(q: &mut Quota, dir: Option<&Path>) {
    let root = dir
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("CODEX_HOME").map(PathBuf::from))
        .unwrap_or_else(|| home().join(".codex"));
    q.link = Some("https://chatgpt.com/codex/settings/usage");
    if let Some((email, plan)) = codex_identity(&root) {
        q.email = email;
        q.plan = plan.map(|p| title_case(&p));
    }
    match latest_codex_limits(&root.join("sessions")) {
        Some((at, rl)) => {
            if let Some(p) = rl["plan_type"].as_str() {
                q.plan = Some(title_case(p));
            }
            for key in ["primary", "secondary"] {
                let w = &rl[key];
                if !w.is_object() {
                    continue;
                }
                let minutes = w["window_minutes"].as_u64().unwrap_or(0);
                let resets = w["resets_at"]
                    .as_i64()
                    .and_then(|t| Utc.timestamp_opt(t, 0).single())
                    .or_else(|| w["resets_in_seconds"].as_i64().map(|s| at + chrono::Duration::seconds(s)));
                let mut used = w["used_percent"].as_f64().unwrap_or(0.0);
                // The window rolled over since this snapshot.
                if resets.map(|r| r < Utc::now()).unwrap_or(false) {
                    used = 0.0;
                }
                q.windows.push(Window { name: window_name(minutes), used_pct: used, resets_at: resets.filter(|r| *r > Utc::now()), detail: None });
            }
            let credits = &rl["credits"];
            if credits["unlimited"].as_bool() == Some(true) {
                q.windows.push(Window { name: "Credits".into(), used_pct: 0.0, resets_at: None, detail: Some("Unlimited".into()) });
            } else if let Some(b) = credits["balance"].as_f64() {
                q.windows.push(Window { name: "Credits".into(), used_pct: 0.0, resets_at: None, detail: Some(format!("{b:.2} left")) });
            }
            q.as_of = Some(at);
        }
        None => q.note = Some("Limits appear after your next Codex run.".into()),
    }
}

/// Email and plan from the claims of Codex's ID token (the token itself is
/// never kept).
fn codex_identity(root: &Path) -> Option<(Option<String>, Option<String>)> {
    use base64::Engine;
    let text = std::fs::read_to_string(root.join("auth.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let jwt = v["tokens"]["id_token"].as_str()?;
    let payload = jwt.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let email = claims["email"].as_str().map(|s| s.to_string());
    let plan = claims["https://api.openai.com/auth"]["chatgpt_plan_type"].as_str().map(|s| s.to_string());
    Some((email, plan))
}

/// Newest `rate_limits` snapshot across the most recent rollout files.
fn latest_codex_limits(sessions: &Path) -> Option<(DateTime<Utc>, serde_json::Value)> {
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    // sessions/YYYY/MM/DD/rollout-*.jsonl — walk the newest days first.
    let sorted_dirs = |p: &Path| -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(p).map(|r| r.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()).unwrap_or_default();
        v.sort();
        v.reverse();
        v
    };
    'outer: for y in sorted_dirs(sessions) {
        for m in sorted_dirs(&y) {
            for d in sorted_dirs(&m) {
                if let Ok(rd) = std::fs::read_dir(&d) {
                    for e in rd.flatten() {
                        let p = e.path();
                        if p.extension().and_then(|x| x.to_str()) == Some("jsonl") {
                            let t = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
                            files.push((t, p));
                        }
                    }
                }
                if files.len() >= 12 {
                    break 'outer;
                }
            }
        }
    }
    files.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, f) in files.into_iter().take(12) {
        let Ok(file) = std::fs::File::open(&f) else { continue };
        let mut best: Option<(DateTime<Utc>, serde_json::Value)> = None;
        for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
            if !line.contains("\"rate_limits\"") {
                continue;
            }
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
            let rl = if v["payload"]["rate_limits"].is_object() { v["payload"]["rate_limits"].clone() } else { v["rate_limits"].clone() };
            if !rl.is_object() {
                continue;
            }
            let at = v["timestamp"].as_str().and_then(|t| DateTime::parse_from_rfc3339(t).ok()).map(|t| t.with_timezone(&Utc)).unwrap_or_else(Utc::now);
            best = Some((at, rl));
        }
        if best.is_some() {
            return best;
        }
    }
    None
}

// ------------------------------------------------------------------ Claude Code

fn claude(q: &mut Quota, dir: Option<&Path>) {
    let root = dir
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from))
        .unwrap_or_else(|| home().join(".claude"));
    q.link = Some("https://claude.ai/settings/usage");
    // Account email lives in .claude.json (next to the config dir by default).
    for p in [root.join(".claude.json"), home().join(".claude.json")] {
        if let Some(acc) = std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) {
            if let Some(e) = acc["oauthAccount"]["emailAddress"].as_str() {
                q.email = Some(e.to_string());
                break;
            }
        }
        if dir.is_some() {
            break;
        }
    }
    let creds = std::fs::read_to_string(root.join(".credentials.json")).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
    let Some(oauth) = creds.as_ref().map(|c| &c["claudeAiOauth"]).filter(|o| o.is_object()) else {
        q.note = Some("Sign in with Claude Code to see limits.".into());
        return;
    };
    if let Some(sub) = oauth["subscriptionType"].as_str() {
        q.plan = Some(title_case(sub));
    }
    let Some(token) = oauth["accessToken"].as_str().filter(|t| !t.is_empty()) else {
        q.note = Some("Sign in with Claude Code to see limits.".into());
        return;
    };
    let expired = oauth["expiresAt"].as_i64().map(|ms| ms < Utc::now().timestamp_millis()).unwrap_or(false);
    if expired {
        q.note = Some("Your Claude Code sign-in expired — run Claude Code once to refresh it.".into());
        return;
    }
    match http_get_json("https://api.anthropic.com/api/oauth/usage", token, &[("anthropic-beta", "oauth-2025-04-20")]) {
        Ok(v) => claude_usage(q, &v),
        Err(HttpError::Auth) => q.note = Some("Sign in with Claude Code to see limits.".into()),
        Err(HttpError::Other(e)) => q.note = Some(format!("Couldn't load limits ({e}).")),
    }
}

/// The usage endpoint's answer: each window by name, each model's own week
/// from its `limits` list, and extra usage when it's on.
fn claude_usage(q: &mut Quota, v: &serde_json::Value) {
    let names = [
        ("five_hour", "5-hour"),
        ("seven_day", "Weekly"),
        ("seven_day_opus", "Weekly · Opus"),
        ("seven_day_sonnet", "Weekly · Sonnet"),
        ("seven_day_oauth_apps", "Weekly · Apps"),
    ];
    if let Some(obj) = v.as_object() {
        for (key, w) in obj {
            let Some(util) = w.get("utilization").and_then(|u| u.as_f64()) else { continue };
            let name = names.iter().find(|(k, _)| k == key).map(|(_, n)| n.to_string()).unwrap_or_else(|| title_case(&key.replace('_', " ")));
            let resets = w.get("resets_at").and_then(|r| r.as_str()).and_then(|t| DateTime::parse_from_rfc3339(t).ok()).map(|t| t.with_timezone(&Utc));
            q.windows.push(Window { name, used_pct: util, resets_at: resets, detail: None });
        }
        // Each model's own week ("weekly_scoped": Fable's), named by
        // the server.
        for l in obj.get("limits").and_then(|l| l.as_array()).into_iter().flatten() {
            if l["kind"].as_str() != Some("weekly_scoped") {
                continue;
            }
            let (Some(model), Some(pct)) = (l["scope"]["model"]["display_name"].as_str().filter(|m| !m.trim().is_empty()), l["percent"].as_f64()) else { continue };
            let name = format!("Weekly · {}", model.trim());
            if q.windows.iter().any(|w| w.name == name) {
                continue;
            }
            let resets = l["resets_at"].as_str().and_then(|t| DateTime::parse_from_rfc3339(t).ok()).map(|t| t.with_timezone(&Utc));
            q.windows.push(Window { name, used_pct: pct, resets_at: resets, detail: None });
        }
        if let Some(extra) = obj.get("extra_usage").filter(|e| e["is_enabled"].as_bool() == Some(true)) {
            let used = extra["used_credits"].as_f64().unwrap_or(0.0);
            let limit = extra["monthly_limit"].as_f64().unwrap_or(0.0);
            let pct = extra["utilization"].as_f64().unwrap_or(if limit > 0.0 { used / limit * 100.0 } else { 0.0 });
            let detail = if limit > 0.0 { Some(format!("${:.2} of ${:.0}", used / 100.0, limit / 100.0)) } else { None };
            q.windows.push(Window { name: "Extra usage".into(), used_pct: pct, resets_at: None, detail });
        }
    }
    sort_windows(&mut q.windows);
    q.as_of = Some(Utc::now());
    if q.windows.is_empty() {
        q.note = Some("No plan limits reported for this account.".into());
    }
}

enum HttpError {
    Auth,
    Other(String),
}

/// GET a JSON document with a bearer token. Uses the system `curl` and
/// passes the token on stdin so it never appears in a process listing.
fn http_get_json(url: &str, token: &str, headers: &[(&str, &str)]) -> Result<serde_json::Value, HttpError> {
    let mut cmd = std::process::Command::new("curl");
    cmd.args(["-sS", "--max-time", "15", "-K", "-"]);
    cmd.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().map_err(|_| HttpError::Other("curl not found".into()))?;
    {
        let mut cfg = format!("url = \"{url}\"\nheader = \"Authorization: Bearer {token}\"\nheader = \"Accept: application/json\"\nheader = \"User-Agent: lary\"\nwrite-out = \"\\n%{{http_code}}\"\n");
        for (k, v) in headers {
            cfg.push_str(&format!("header = \"{k}: {v}\"\n"));
        }
        let Some(mut stdin) = child.stdin.take() else { return Err(HttpError::Other("no stdin".into())) };
        let _ = stdin.write_all(cfg.as_bytes());
    }
    let mut out = String::new();
    if let Some(mut so) = child.stdout.take() {
        let _ = so.read_to_string(&mut out);
    }
    let _ = child.wait();
    let (body, code) = out.rsplit_once('\n').unwrap_or(("", out.as_str()));
    let code: u16 = code.trim().parse().unwrap_or(0);
    match code {
        200..=299 => serde_json::from_str(body).map_err(|_| HttpError::Other("unexpected response".into())),
        401 | 403 => Err(HttpError::Auth),
        0 => Err(HttpError::Other("offline".into())),
        c => Err(HttpError::Other(format!("HTTP {c}"))),
    }
}

// ------------------------------------------------------------------ Cursor / Gemini

fn cursor(q: &mut Quota) {
    q.link = Some("https://cursor.com/dashboard?tab=usage");
    let cfg = std::fs::read_to_string(home().join(".cursor").join("cli-config.json")).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
    if let Some(e) = cfg.as_ref().and_then(|c| c["authInfo"]["email"].as_str()) {
        q.email = Some(e.to_string());
    }
    q.note = Some(if q.email.is_some() { "Cursor shows plan usage on its dashboard.".into() } else { "Sign in to the Cursor CLI to link this account.".into() });
}

fn gemini(q: &mut Quota, dir: Option<&Path>) {
    let root = dir.map(Path::to_path_buf).unwrap_or_else(|| home().join(".gemini"));
    if let Some(v) = std::fs::read_to_string(root.join("google_accounts.json")).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) {
        if let Some(e) = v["active"].as_str() {
            q.email = Some(e.to_string());
        }
    }
    q.link = Some("https://aistudio.google.com/usage");
    q.note = Some("Gemini CLI doesn't report plan limits locally.".into());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The windows in a real answer's headers (2026-11-20, a setup token).
    #[test]
    fn reads_the_plans_windows_from_an_answer() {
        let head = "HTTP/2 200\r\nanthropic-ratelimit-unified-5h-reset: 1795189200\r\nanthropic-ratelimit-unified-5h-status: allowed\r\nanthropic-ratelimit-unified-5h-utilization: 0.25\r\nanthropic-ratelimit-unified-7d-reset: 1795604400\r\nanthropic-ratelimit-unified-7d-utilization: 0.16\r\nanthropic-ratelimit-unified-7d_opus-utilization: 0.5\r\nanthropic-ratelimit-unified-fallback-percentage: 0.5\r\nanthropic-ratelimit-unified-reset: 1795189200\r\n\r\n{\"id\":\"msg\"}\n200";
        let w = unified_windows(head);
        assert_eq!(w.len(), 3, "{w:?}");
        assert_eq!(w[0].0, "5-hour");
        assert!((w[0].1 - 25.0).abs() < 1e-9);
        assert_eq!(w[0].2.map(|t| t.timestamp()), Some(1795189200));
        assert_eq!(w[1].0, "Weekly");
        assert!((w[1].1 - 16.0).abs() < 1e-9);
        assert_eq!(w[2].0, "Weekly · Opus");
        assert!(w[2].2.is_none());
    }

    #[test]
    fn window_names() {
        assert_eq!(window_name(300), "5-hour");
        assert_eq!(window_name(10080), "Weekly");
        assert_eq!(window_name(4320), "3-day");
    }

    /// Live check against this machine's agents (prints no secrets or emails).
    #[test]
    #[ignore]
    fn real_quotas() {
        let cfg = Config::default();
        for q in fetch_all(&cfg, &PLAN_AGENTS, true) {
            let w: Vec<String> = q.windows.iter().map(|w| format!("{} {:.0}% resets {:?}", w.name, w.used_pct, w.resets_at.map(|r| r.to_rfc3339()))).collect();
            eprintln!("{}: plan={:?} email={} windows={w:?} note={:?}", q.agent.display_name(), q.plan, q.email.is_some(), q.note);
        }
    }

    /// The usage endpoint's answer as it is now (trimmed; a real one):
    /// windows by key, Fable's week from `limits`, extra usage off.
    #[test]
    fn reads_claude_usage() {
        let v: serde_json::Value = serde_json::from_str(r#"{
            "five_hour":{"utilization":2.0,"resets_at":"2026-11-18T15:50:00.974279+03:00","limit_dollars":null},
            "seven_day":{"utilization":94.0,"resets_at":"2026-11-18T13:59:59.974298+03:00"},
            "seven_day_opus":null,"seven_day_sonnet":null,"tangelo":null,
            "extra_usage":{"is_enabled":false,"monthly_limit":null,"used_credits":null,"utilization":null},
            "limits":[
                {"kind":"session","group":"session","percent":2,"resets_at":"2026-11-18T15:50:00.974279+03:00","scope":null},
                {"kind":"weekly_all","group":"weekly","percent":94,"resets_at":"2026-11-18T13:59:59.974298+03:00","scope":null},
                {"kind":"weekly_scoped","group":"weekly","percent":84,"resets_at":"2026-11-18T14:00:00.974477+03:00","scope":{"model":{"id":null,"display_name":"Fable"},"surface":null}}
            ],
            "spend":{"percent":0},"weekly_scoped_shares":{"used_percent_of_weekly":42}
        }"#).unwrap();
        let mut q = Quota::new(AgentKind::ClaudeCode, None, "Default".into());
        claude_usage(&mut q, &v);
        let got: Vec<(&str, f64)> = q.windows.iter().map(|w| (w.name.as_str(), w.used_pct)).collect();
        assert_eq!(got, vec![("5-hour", 2.0), ("Weekly", 94.0), ("Weekly · Fable", 84.0)]);
        assert_eq!(q.windows[2].resets_at.map(|t| t.timestamp()), Some(1794999600));
        assert!(q.note.is_none());
    }

    /// A setup token's windows are the ones its runs reported; one whose
    /// reset has passed starts again at nothing used.
    #[test]
    fn seen_windows_roll_over() {
        let now = Utc.timestamp_opt(1795000000, 0).unwrap();
        let s = Seen {
            at: 1794990000,
            windows: vec![
                SeenWindow { name: "Weekly · Fable".into(), used_pct: 84.0, resets_at: Some(1795003200) },
                SeenWindow { name: "Weekly".into(), used_pct: 94.0, resets_at: Some(1794999600) },
                SeenWindow { name: "5-hour".into(), used_pct: 2.0, resets_at: Some(1795006200) },
            ],
        };
        let w = seen_windows(&s, now);
        let got: Vec<(&str, f64, bool)> = w.iter().map(|w| (w.name.as_str(), w.used_pct, w.resets_at.is_some())).collect();
        assert_eq!(got, vec![("5-hour", 2.0, true), ("Weekly", 0.0, false), ("Weekly · Fable", 84.0, true)]);
    }

    #[test]
    fn reads_codex_snapshot() {
        let dir = std::env::temp_dir().join(format!("lary-quota-test-{}", std::process::id()));
        let day = dir.join("sessions").join("2026").join("10").join("29");
        std::fs::create_dir_all(&day).unwrap();
        let future = Utc::now().timestamp() + 3600;
        let line = format!(
            r#"{{"timestamp":"2026-10-29T10:00:00Z","type":"event_msg","payload":{{"type":"token_count","rate_limits":{{"primary":{{"used_percent":13.0,"window_minutes":300,"resets_at":{future}}},"secondary":{{"used_percent":2.0,"window_minutes":10080,"resets_at":{future}}},"plan_type":"team"}}}}}}"#
        );
        std::fs::write(day.join("rollout-x.jsonl"), format!("{{\"type\":\"session_meta\"}}\n{line}\n")).unwrap();
        let mut q = Quota::new(AgentKind::Codex, None, "Default".into());
        codex(&mut q, Some(&dir));
        assert_eq!(q.plan.as_deref(), Some("Team"));
        assert_eq!(q.windows.len(), 2);
        assert_eq!(q.windows[0].name, "5-hour");
        assert!((q.windows[0].used_pct - 13.0).abs() < 1e-9);
        assert!(q.windows[0].resets_at.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
