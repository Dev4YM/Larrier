//! Markdown transcripts: Aider's `.aider.chat.history.md` and generic
//! "## User / ## Assistant" exports (including Larrier's own).

use anyhow::Result;
use larrier_core::{AgentKind, Session};

use crate::util::{finish, finish_as, parse_ts, Conv, Meta};

pub fn parse(text: &str, name: &str) -> Result<Vec<Session>> {
    if text.contains("\n#### ") || text.starts_with("#### ") || text.contains("# aider chat started") {
        return parse_aider(text);
    }
    Ok(vec![parse_generic(text, name)?])
}

/// Aider: each `# aider chat started at <ts>` begins a session; user input is
/// `#### ` prefixed, tool output is `> ` quoted, the rest is the model.
fn parse_aider(text: &str) -> Result<Vec<Session>> {
    let mut out = Vec::new();
    for (i, chunk) in text.split("# aider chat started at ").enumerate() {
        if chunk.trim().is_empty() {
            continue;
        }
        let mut lines = chunk.lines();
        let started = if i == 0 && !text.starts_with("# aider") { None } else { lines.next().and_then(|l| parse_ts(l.trim())) };
        let mut conv = Conv::new(AgentKind::Aider);
        let mut user = String::new();
        let mut asst = String::new();
        let flush_user = |conv: &mut Conv, user: &mut String| {
            if !user.trim().is_empty() {
                conv.user(user.trim().to_string(), started);
            }
            user.clear();
        };
        let flush_asst = |conv: &mut Conv, asst: &mut String| {
            if !asst.trim().is_empty() {
                conv.text(asst.trim(), started, None);
            }
            asst.clear();
        };
        for l in lines {
            if let Some(u) = l.strip_prefix("#### ") {
                flush_asst(&mut conv, &mut asst);
                user.push_str(u);
                user.push('\n');
            } else if l.starts_with("> ") || l == ">" {
                // Tool / command output: keep it out of the transcript body.
                continue;
            } else {
                flush_user(&mut conv, &mut user);
                asst.push_str(l);
                asst.push('\n');
            }
        }
        flush_user(&mut conv, &mut user);
        flush_asst(&mut conv, &mut asst);
        let meta = Meta { native_id: format!("aider-{}", started.map(|t| t.timestamp()).unwrap_or(i as i64)), created: started, ..Default::default() };
        if let Ok(s) = finish(conv, AgentKind::Aider, meta) {
            out.push(s);
        }
    }
    if out.is_empty() {
        anyhow::bail!("no aider conversations found");
    }
    Ok(out)
}

/// `## User` / `## Assistant` (or any `## <agent name>`) sections.
fn parse_generic(text: &str, name: &str) -> Result<Session> {
    let mut conv = Conv::new(AgentKind::Chat);
    let mut title = None;
    let mut role: Option<bool> = None; // true = user
    let mut buf = String::new();
    let flush = |conv: &mut Conv, role: Option<bool>, buf: &mut String| {
        let t = buf.trim().to_string();
        if !t.is_empty() {
            match role {
                Some(true) => conv.user(t, None),
                Some(false) => conv.text(&t, None, None),
                None => {}
            }
        }
        buf.clear();
    };
    for l in text.lines() {
        if let Some(h) = l.strip_prefix("# ").filter(|_| title.is_none() && role.is_none()) {
            title = Some(h.trim().to_string());
            continue;
        }
        if let Some(h) = l.strip_prefix("## ") {
            flush(&mut conv, role, &mut buf);
            let h = h.trim().to_lowercase();
            role = Some(matches!(h.as_str(), "user" | "you" | "human" | "me" | "prompt"));
            if h == "system" {
                role = None;
            }
            continue;
        }
        if role.is_some() {
            buf.push_str(l);
            buf.push('\n');
        }
    }
    flush(&mut conv, role, &mut buf);
    let meta = Meta { native_id: name.to_string(), title, ..Default::default() };
    finish_as(conv, AgentKind::Chat, AgentKind::Chat, meta)
}

#[cfg(test)]
mod tests {
    #[test]
    fn aider_and_generic() {
        let a = "# aider chat started at 2026-01-01 10:00:00\n\n> Aider v0.80\n\n#### add tests\n\nSure, here are tests.\n\n#### thanks\n\nYou're welcome.\n";
        let s = super::parse(a, "x.md").unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].messages.len(), 4);
        let g = "# My chat\n\n## User\n\nhello\n\n## Claude Code\n\nhi there\n";
        let s = super::parse(g, "g.md").unwrap();
        assert_eq!(s[0].title, "My chat");
        assert_eq!(s[0].messages.len(), 2);
    }
}
