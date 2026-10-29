//! Automations: agent runs that start on a schedule or when a repository
//! changes. They run locally with any agent the user has configured.

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::agent::AgentKind;
use crate::lsf::new_id;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Automation {
    pub id: String,
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub agent: AgentKind,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub workspace: Option<String>,
    pub prompt: String,
    pub trigger: Trigger,
    /// Run each time in a fresh git worktree.
    #[serde(default)]
    pub worktree: bool,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub last_run: Option<DateTime<Utc>>,
    /// `started`, `review`, `done`, `error`, …
    #[serde(default)]
    pub last_status: Option<String>,
    #[serde(default)]
    pub last_session_id: Option<String>,
    #[serde(default)]
    pub runs: u32,
    /// Template this automation was created from, if any.
    #[serde(default)]
    pub template: Option<String>,
    /// Last seen `HEAD` for [`Trigger::OnGitChange`].
    #[serde(default)]
    pub last_head: Option<String>,
}

fn yes() -> bool {
    true
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Trigger {
    /// Every `minutes` minutes.
    Interval { minutes: u32 },
    /// At a local wall-clock time. `weekdays` uses 0 = Monday; empty means
    /// every day.
    Daily {
        hour: u8,
        minute: u8,
        #[serde(default)]
        weekdays: Vec<u8>,
    },
    /// When the workspace `HEAD` moves (new commits, branch switch). With a
    /// branch set, only while that branch is checked out.
    OnGitChange {
        #[serde(default)]
        branch: Option<String>,
    },
    /// Only when started by hand.
    Manual,
}

impl Trigger {
    pub fn kind(&self) -> &'static str {
        match self {
            Trigger::Interval { .. } => "interval",
            Trigger::Daily { .. } => "daily",
            Trigger::OnGitChange { .. } => "git",
            Trigger::Manual => "manual",
        }
    }

    /// Human description: "Every 30 min", "Weekdays at 09:00", …
    pub fn describe(&self) -> String {
        match self {
            Trigger::Interval { minutes } => {
                let m = (*minutes).max(1);
                if m % 1440 == 0 {
                    let d = m / 1440;
                    if d == 1 { "Every day".into() } else { format!("Every {d} days") }
                } else if m % 60 == 0 {
                    let h = m / 60;
                    if h == 1 { "Every hour".into() } else { format!("Every {h} h") }
                } else {
                    format!("Every {m} min")
                }
            }
            Trigger::Daily { hour, minute, weekdays } => {
                let at = format!("{hour:02}:{minute:02}");
                let mut days: Vec<u8> = weekdays.iter().copied().filter(|d| *d < 7).collect();
                days.sort_unstable();
                days.dedup();
                if days.is_empty() || days.len() == 7 {
                    format!("Every day at {at}")
                } else if days == [0, 1, 2, 3, 4] {
                    format!("Weekdays at {at}")
                } else if days == [5, 6] {
                    format!("Weekends at {at}")
                } else {
                    const NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
                    let names: Vec<&str> = days.iter().map(|d| NAMES[*d as usize]).collect();
                    format!("{} at {at}", names.join(", "))
                }
            }
            Trigger::OnGitChange { branch: Some(b) } if !b.trim().is_empty() => format!("On new commits to {b}"),
            Trigger::OnGitChange { .. } => "On new commits".into(),
            Trigger::Manual => "Manual".into(),
        }
    }

    /// Short label for the template cards ("Scheduled", "On commit", …).
    pub fn badge(&self) -> &'static str {
        match self {
            Trigger::Interval { .. } | Trigger::Daily { .. } => "Scheduled",
            Trigger::OnGitChange { .. } => "On commit",
            Trigger::Manual => "Manual",
        }
    }
}

impl Automation {
    pub fn new(name: impl Into<String>, agent: AgentKind, prompt: impl Into<String>, trigger: Trigger) -> Self {
        Automation {
            id: new_id(),
            name: name.into(),
            enabled: true,
            agent,
            model: None,
            workspace: None,
            prompt: prompt.into(),
            trigger,
            worktree: false,
            created_at: Utc::now(),
            last_run: None,
            last_status: None,
            last_session_id: None,
            runs: 0,
            template: None,
            last_head: None,
        }
    }

    /// When the next scheduled run is due, computed from the last run (or the
    /// creation time for a new automation). `None` for event-driven and
    /// manual triggers. A run that was missed while Larrier was closed is
    /// due immediately, once — missed runs are never replayed one by one.
    pub fn next_due(&self, now: DateTime<Local>) -> Option<DateTime<Local>> {
        let _ = now;
        let base = self.last_run.unwrap_or(self.created_at).with_timezone(&Local);
        match &self.trigger {
            Trigger::Interval { minutes } => Some(base + Duration::minutes((*minutes).max(1) as i64)),
            Trigger::Daily { hour, minute, weekdays } => next_daily(base, *hour, *minute, weekdays),
            Trigger::OnGitChange { .. } | Trigger::Manual => None,
        }
    }

    /// Enabled and its scheduled time has arrived.
    pub fn is_due(&self, now: DateTime<Local>) -> bool {
        self.enabled && self.next_due(now).map(|t| t <= now).unwrap_or(false)
    }
}

/// First local `hour:minute` strictly after `base` on an allowed weekday.
fn next_daily(base: DateTime<Local>, hour: u8, minute: u8, weekdays: &[u8]) -> Option<DateTime<Local>> {
    let hour = hour.min(23) as u32;
    let minute = minute.min(59) as u32;
    let allowed = |d: NaiveDate| weekdays.is_empty() || weekdays.contains(&(d.weekday().num_days_from_monday() as u8));
    let start = base.date_naive();
    for offset in 0..15 {
        let day = start + Duration::days(offset);
        if !allowed(day) {
            continue;
        }
        let naive = day.and_hms_opt(hour, minute, 0)?;
        // DST gaps have no local time; fall forward an hour. Ambiguous
        // (repeated) times use the earlier instant.
        let at = Local
            .from_local_datetime(&naive)
            .earliest()
            .or_else(|| Local.from_local_datetime(&(naive + Duration::hours(1))).earliest());
        if let Some(at) = at {
            if at > base {
                return Some(at);
            }
        }
    }
    None
}

/// A starting point offered on the Automations page.
#[derive(Clone, Debug, PartialEq)]
pub struct Template {
    pub id: &'static str,
    pub category: &'static str,
    pub popular: bool,
    pub title: &'static str,
    pub description: &'static str,
    pub trigger: Trigger,
    /// What the run produces, shown as `trigger → action`.
    pub action: &'static str,
    pub prompt: &'static str,
    /// Lucide icon name.
    pub icon: &'static str,
    /// Accent color for the icon.
    pub color: &'static str,
}

pub const CATEGORIES: &[&str] = &["Popular", "Code Review", "Security", "Incidents & Triage", "Data & Research", "Environment"];

impl Template {
    pub fn in_category(&self, cat: &str) -> bool {
        if cat == "Popular" {
            self.popular
        } else {
            self.category == cat
        }
    }

    /// A new automation prefilled from this template.
    pub fn instantiate(&self, agent: AgentKind) -> Automation {
        let mut a = Automation::new(self.title, agent, self.prompt, self.trigger.clone());
        a.template = Some(self.id.to_string());
        a
    }
}

fn daily(hour: u8, minute: u8, weekdays: &[u8]) -> Trigger {
    Trigger::Daily { hour, minute, weekdays: weekdays.to_vec() }
}

const WEEKDAYS: &[u8] = &[0, 1, 2, 3, 4];

pub fn templates() -> Vec<Template> {
    vec![
        Template {
            id: "find-critical-bugs",
            category: "Code Review",
            popular: true,
            title: "Find critical bugs",
            description: "Analyze the last day of commits for high-severity correctness bugs and propose safe fixes.",
            trigger: daily(9, 0, WEEKDAYS),
            action: "Report in chat",
            prompt: "Review every commit made in this repository in the last 24 hours (use `git log --since=\"24 hours ago\" -p`). \
Look only for high-severity correctness bugs: crashes, data loss, wrong results, broken error handling, race conditions. \
For each real bug, explain the failure scenario, point to the exact file and line, and propose a minimal fix. \
Apply a fix only when it is small and clearly safe. If you find nothing serious, say so in one line.",
            icon: "bug",
            color: "#3a6a9f",
        },
        Template {
            id: "scan-vulnerabilities",
            category: "Security",
            popular: true,
            title: "Scan codebase for vulnerabilities",
            description: "Review the repository on a schedule and flag only validated, high-impact security issues.",
            trigger: daily(2, 0, &[]),
            action: "Report in chat",
            prompt: "Do a security review of this repository. Focus on injection (SQL, command, path traversal), authentication and \
authorization flaws, secrets committed to the repo, unsafe deserialization, SSRF and insecure defaults. Validate each \
finding by tracing the data flow; report only issues you are confident are exploitable, ranked by impact, with file \
and line and a concrete fix. Do not change code.",
            icon: "shield-check",
            color: "#1f8a65",
        },
        Template {
            id: "generate-docs",
            category: "Data & Research",
            popular: true,
            title: "Generate docs",
            description: "Create and update developer documentation for recently changed code.",
            trigger: daily(18, 0, WEEKDAYS),
            action: "Edits docs",
            prompt: "Find the code that changed in the last 24 hours (`git log --since=\"24 hours ago\" --name-only`). For public \
functions, modules and APIs that changed, update or add concise doc comments and keep README / docs pages accurate. \
Do not rewrite unrelated documentation. Summarize what you updated.",
            icon: "book-open",
            color: "#c96442",
        },
        Template {
            id: "add-test-coverage",
            category: "Code Review",
            popular: true,
            title: "Add test coverage",
            description: "Review recent changes and add tests for high-risk logic that lacks adequate coverage.",
            trigger: daily(20, 0, WEEKDAYS),
            action: "Adds tests",
            prompt: "Look at the code changed in the last 24 hours. Identify high-risk logic (parsing, money, auth, state machines, \
edge cases) that has no or weak tests. Add focused tests that follow the project's existing test style, run the test \
suite, and make sure everything passes. Summarize the tests you added and anything you could not cover.",
            icon: "flask-conical",
            color: "#3a6a9f",
        },
        Template {
            id: "summarize-changes-daily",
            category: "Data & Research",
            popular: true,
            title: "Summarize changes daily",
            description: "Write a daily digest of notable repository changes and risks to a Markdown file.",
            trigger: daily(17, 30, WEEKDAYS),
            action: "Writes digest",
            prompt: "Summarize what changed in this repository in the last 24 hours across all branches \
(`git log --all --since=\"24 hours ago\" --stat`). Group by theme, call out risky changes, migrations, dependency bumps \
and anything that needs a reviewer's attention. Write the digest to `.larrier/digests/<today's date>.md` (create the \
folder if needed) and also reply with the summary.",
            icon: "mail",
            color: "#9b3b8f",
        },
        Template {
            id: "review-uncommitted",
            category: "Code Review",
            popular: true,
            title: "Review uncommitted changes",
            description: "Every hour, review work in progress and point out bugs before they get committed.",
            trigger: Trigger::Interval { minutes: 60 },
            action: "Report in chat",
            prompt: "Review the uncommitted changes in this repository (`git diff` and `git diff --staged`, plus new untracked \
files). Point out bugs, missing error handling, leftover debug code and anything inconsistent with the surrounding \
code. Be brief and concrete; do not modify files. If there are no changes, reply \"Nothing to review\".",
            icon: "eye",
            color: "#26251e",
        },
        Template {
            id: "dependency-check",
            category: "Security",
            popular: false,
            title: "Dependency update check",
            description: "Check for outdated and vulnerable dependencies and summarize safe upgrades.",
            trigger: daily(8, 0, &[0]),
            action: "Report in chat",
            prompt: "Check this project's dependencies for available updates and known vulnerabilities using the ecosystem's \
own tools (for example `npm outdated` / `npm audit`, `cargo outdated` / `cargo audit`, `pip list --outdated` / \
`pip-audit`). Summarize which upgrades are safe patch/minor bumps, which are breaking, and which fix security issues. \
Do not upgrade anything.",
            icon: "package",
            color: "#c08532",
        },
        Template {
            id: "lint-and-fix",
            category: "Environment",
            popular: false,
            title: "Lint & fix",
            description: "Run the project's linters and formatters and fix the warnings they report.",
            trigger: daily(19, 0, WEEKDAYS),
            action: "Edits code",
            prompt: "Run this project's configured linters and formatters (look at package.json scripts, Cargo, pyproject, \
Makefile, etc.). Fix the warnings and errors they report without changing behaviour, re-run them until clean, and \
summarize what you changed. Skip anything that would need a design decision and list it instead.",
            icon: "wand-sparkles",
            color: "#7c5cd6",
        },
        Template {
            id: "triage-todos",
            category: "Incidents & Triage",
            popular: false,
            title: "Triage TODO / FIXME",
            description: "Collect TODO, FIXME and HACK comments and rank them by risk.",
            trigger: daily(10, 0, &[4]),
            action: "Writes report",
            prompt: "Find all TODO, FIXME, HACK and XXX comments in this repository. Group them by area, rank them by risk \
(security, data loss, correctness, cleanup), and note which look stale. Write the result to \
`.larrier/triage/todos.md` and reply with the top ten.",
            icon: "list-todo",
            color: "#c08532",
        },
        Template {
            id: "changelog-draft",
            category: "Data & Research",
            popular: false,
            title: "Changelog draft",
            description: "Draft release notes from commits since the last tag.",
            trigger: Trigger::Manual,
            action: "Writes changelog",
            prompt: "Draft release notes for the changes since the most recent git tag (`git describe --tags --abbrev=0`, then \
`git log <tag>..HEAD`). Group into Highlights, Features, Fixes and Internal, written for users rather than developers. \
Save the draft to `.larrier/CHANGELOG-draft.md` and reply with it.",
            icon: "scroll-text",
            color: "#3a6a9f",
        },
        Template {
            id: "build-health",
            category: "Incidents & Triage",
            popular: false,
            title: "Check build health",
            description: "When new commits land, build and run the tests, and report what broke.",
            trigger: Trigger::OnGitChange { branch: None },
            action: "Report in chat",
            prompt: "New commits just landed. Build the project and run its test suite using the project's own commands. \
If everything passes, reply with one line. If something fails, find the commit and the change that caused it, explain \
the root cause, and propose a fix (do not commit).",
            icon: "hammer",
            color: "#cf2d56",
        },
        Template {
            id: "stale-branches",
            category: "Environment",
            popular: false,
            title: "Clean up stale branches report",
            description: "List merged and long-idle branches that are safe to delete.",
            trigger: daily(9, 30, &[0]),
            action: "Report in chat",
            prompt: "List local and remote branches in this repository. Identify branches already merged into the default \
branch and branches with no commits in 30 days. For each, show last commit date and author. Reply with a table and the \
exact commands to delete the safe ones — do not delete anything.",
            icon: "git-branch",
            color: "#1f8a65",
        },
        Template {
            id: "incident-log-scan",
            category: "Incidents & Triage",
            popular: false,
            title: "Scan logs for new errors",
            description: "Every 30 minutes, scan local log files for new errors and explain them.",
            trigger: Trigger::Interval { minutes: 30 },
            action: "Report in chat",
            prompt: "Look for log files in this project (for example `logs/`, `*.log`). Find errors and exceptions written in \
the last 30 minutes, de-duplicate them, and explain the likely cause of each with the relevant code location. \
If there are none, reply \"No new errors\".",
            icon: "siren",
            color: "#cf2d56",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Local> {
        Local.from_local_datetime(&NaiveDate::from_ymd_opt(y, m, d).unwrap().and_hms_opt(h, min, 0).unwrap()).earliest().unwrap()
    }

    fn with_base(trigger: Trigger, base: DateTime<Local>) -> Automation {
        let mut a = Automation::new("t", AgentKind::ClaudeCode, "p", trigger);
        a.created_at = base.with_timezone(&Utc);
        a
    }

    #[test]
    fn interval_due() {
        let base = at(2026, 3, 10, 12, 0);
        let mut a = with_base(Trigger::Interval { minutes: 30 }, base);
        assert_eq!(a.next_due(base), Some(base + Duration::minutes(30)));
        assert!(!a.is_due(base + Duration::minutes(29)));
        assert!(a.is_due(base + Duration::minutes(30)));
        a.last_run = Some((base + Duration::minutes(31)).with_timezone(&Utc));
        assert!(!a.is_due(base + Duration::minutes(40)));
        a.enabled = false;
        assert!(!a.is_due(base + Duration::days(3)));
    }

    #[test]
    fn daily_same_day_and_next_day() {
        // Created at 08:00 → due 09:00 the same day.
        let base = at(2026, 3, 10, 8, 0);
        let a = with_base(daily(9, 0, &[]), base);
        let next = a.next_due(base).unwrap();
        assert_eq!((next.day(), next.hour(), next.minute()), (10, 9, 0));
        // Created at 10:00 → due 09:00 the next day.
        let a = with_base(daily(9, 0, &[]), at(2026, 3, 10, 10, 0));
        let next = a.next_due(base).unwrap();
        assert_eq!((next.day(), next.hour()), (11, 9));
    }

    #[test]
    fn daily_weekdays_skip_weekend() {
        // 2026-03-13 is a Friday; created after 09:00 → next weekday run is Monday the 16th.
        let fri = at(2026, 3, 13, 10, 0);
        assert_eq!(fri.weekday().num_days_from_monday(), 4);
        let a = with_base(daily(9, 0, WEEKDAYS), fri);
        let next = a.next_due(fri).unwrap();
        assert_eq!((next.day(), next.weekday().num_days_from_monday()), (16, 0));
    }

    #[test]
    fn missed_runs_fire_once() {
        let base = at(2026, 3, 1, 8, 0);
        let mut a = with_base(daily(9, 0, &[]), base);
        let now = at(2026, 3, 5, 12, 0);
        assert!(a.is_due(now));
        a.last_run = Some(now.with_timezone(&Utc));
        assert!(!a.is_due(now + Duration::hours(1)));
        let next = a.next_due(now).unwrap();
        assert_eq!((next.day(), next.hour()), (6, 9));
    }

    #[test]
    fn event_and_manual_never_scheduled() {
        let base = at(2026, 3, 10, 8, 0);
        assert!(with_base(Trigger::Manual, base).next_due(base).is_none());
        assert!(!with_base(Trigger::OnGitChange { branch: None }, base).is_due(base + Duration::days(9)));
    }

    #[test]
    fn describe_triggers() {
        assert_eq!(Trigger::Interval { minutes: 30 }.describe(), "Every 30 min");
        assert_eq!(Trigger::Interval { minutes: 120 }.describe(), "Every 2 h");
        assert_eq!(daily(9, 0, &[]).describe(), "Every day at 09:00");
        assert_eq!(daily(9, 5, WEEKDAYS).describe(), "Weekdays at 09:05");
        assert_eq!(daily(9, 0, &[0, 2]).describe(), "Mon, Wed at 09:00");
        assert_eq!(Trigger::OnGitChange { branch: Some("main".into()) }.describe(), "On new commits to main");
    }

    #[test]
    fn templates_are_valid() {
        let t = templates();
        assert!(t.len() >= 12);
        for c in CATEGORIES {
            assert!(t.iter().any(|x| x.in_category(c)), "no template in {c}");
        }
        let mut ids: Vec<&str> = t.iter().map(|x| x.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), t.len());
    }

    #[test]
    fn serde_roundtrip() {
        let mut a = Automation::new("x", AgentKind::Codex, "p", daily(7, 15, &[1, 3]));
        a.workspace = Some("D:/repo".into());
        let json = serde_json::to_string(&a).unwrap();
        let back: Automation = serde_json::from_str(&json).unwrap();
        assert_eq!(back, a);
        let legacy: Automation = serde_json::from_str(
            r#"{"id":"1","name":"n","agent":"codex","prompt":"p","trigger":{"type":"manual"},"created_at":"2026-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        assert!(legacy.enabled);
    }
}
