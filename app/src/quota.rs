//! Plan limits per agent account: how much of each rate-limit window is
//! used and when it resets.
//!
//! - Codex: the newest `rate_limits` snapshot in its session logs.
//! - Claude Code: Anthropic's OAuth usage endpoint, with the access token
//!   the CLI keeps in `.credentials.json` (sent over stdin to `curl`, never
//!   logged, shown or stored).
//! - Cursor / Gemini: the signed-in account, when the CLI records it.
//!
//! Agent folders are only ever read.

use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, TimeZone, Utc};
use larrier_core::{AccountAuth, AgentKind, Config};

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
    /// Larrier account id (None = the CLI's default login).
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
pub fn fetch_all(cfg: &Config, agents: &[AgentKind], force: bool) -> Vec<Quota> {
    if !force {
        if let Ok(g) = cache().lock() {
            if let Some((at, v)) = g.as_ref() {
                if at.elapsed() < Duration::from_secs(300) {
                    return v.clone();
                }
            }
        }
    }
    let mut jobs: Vec<(AgentKind, Option<String>, String, Option<PathBuf>, bool)> = Vec::new();
    for kind in agents {
        let accounts = cfg.accounts_for(kind);
        if accounts.is_empty() {
            jobs.push((kind.clone(), None, "Default login".into(), None, false));
        }
        for a in accounts {
            let (dir, api) = match &a.auth {
                AccountAuth::ConfigDir { path } => (Some(PathBuf::from(path)), false),
                AccountAuth::ApiKey { .. } | AccountAuth::Env { .. } => (None, true),
                AccountAuth::SystemLogin => (None, false),
            };
            let label = if a.label.trim().is_empty() { a.auth.label().to_string() } else { a.label.clone() };
            jobs.push((kind.clone(), Some(a.id.clone()), label, dir, api));
        }
    }
    let out: Vec<Quota> = std::thread::scope(|s| {
        let handles: Vec<_> = jobs
            .into_iter()
            .map(|(kind, id, label, dir, api)| {
                let acc_plan = id.as_deref().and_then(|i| cfg.account(i)).and_then(|a| a.plan.clone());
                let acc_email = id.as_deref().and_then(|i| cfg.account(i)).and_then(|a| a.email.clone());
                s.spawn(move || {
                    let mut q = Quota::new(kind.clone(), id, label);
                    if api {
                        q.plan = Some("API key".into());
                        q.note = Some("Pay as you go — no plan limits. Spend is tracked below.".into());
                    } else {
                        match kind {
                            AgentKind::Codex => codex(&mut q, dir.as_deref()),
                            AgentKind::ClaudeCode => claude(&mut q, dir.as_deref()),
                            AgentKind::Cursor => cursor(&mut q),
                            AgentKind::Gemini => gemini(&mut q, dir.as_deref()),
                            _ => q.note = Some("Plan limits are not available for this agent.".into()),
                        }
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
        Ok(v) => {
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
                if let Some(extra) = obj.get("extra_usage").filter(|e| e["is_enabled"].as_bool() == Some(true)) {
                    let used = extra["used_credits"].as_f64().unwrap_or(0.0);
                    let limit = extra["monthly_limit"].as_f64().unwrap_or(0.0);
                    let pct = extra["utilization"].as_f64().unwrap_or(if limit > 0.0 { used / limit * 100.0 } else { 0.0 });
                    let detail = if limit > 0.0 { Some(format!("${:.2} of ${:.0}", used / 100.0, limit / 100.0)) } else { None };
                    q.windows.push(Window { name: "Extra usage".into(), used_pct: pct, resets_at: None, detail });
                }
            }
            // Keep a stable order: 5-hour, weekly, then the rest.
            let rank = |n: &str| match n {
                "5-hour" => 0,
                "Weekly" => 1,
                "Extra usage" => 9,
                _ => 5,
            };
            q.windows.sort_by_key(|w| rank(&w.name));
            q.as_of = Some(Utc::now());
            if q.windows.is_empty() {
                q.note = Some("No plan limits reported for this account.".into());
            }
        }
        Err(HttpError::Auth) => q.note = Some("Sign in with Claude Code to see limits.".into()),
        Err(HttpError::Other(e)) => q.note = Some(format!("Couldn't load limits ({e}).")),
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
        let mut cfg = format!("url = \"{url}\"\nheader = \"Authorization: Bearer {token}\"\nheader = \"Accept: application/json\"\nheader = \"User-Agent: larrier\"\nwrite-out = \"\\n%{{http_code}}\"\n");
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

    #[test]
    fn reads_codex_snapshot() {
        let dir = std::env::temp_dir().join(format!("larrier-quota-test-{}", std::process::id()));
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
