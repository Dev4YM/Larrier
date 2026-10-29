//! Mentions as objects: the files, folders, other agents' chats, the git
//! diff and agent skills a prompt refers to. The composer holds them as
//! chips; on send they become what the agent understands (`@path`, a
//! leading `/skill`, context blocks); in the transcript the injected text
//! is folded back into chips.

use dioxus::prelude::*;
use larrier_core::{AgentKind, SessionSummary};

use super::AgentGlyph;
use crate::icons::{Icon, I};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MentionKind {
    /// Another agent's conversation, passed as context.
    Agent,
    File,
    Folder,
    /// Uncommitted changes in the workspace.
    GitDiff,
    /// An agent skill, sent as a leading `/name`.
    Skill,
    /// An agent slash command, sent as a leading `/name`.
    Command,
    /// A long paste sent as a named block.
    Pasted,
}

#[derive(Clone, PartialEq, Debug)]
pub struct MentionChip {
    pub kind: MentionKind,
    /// What is sent: session id (agent), path (file / folder), name (skill,
    /// command, paste).
    pub value: String,
    /// Shown on the chip.
    pub label: String,
    /// Tooltip.
    pub title: String,
    /// Glyph for agent chips.
    pub agent: Option<AgentKind>,
}

impl MentionChip {
    pub fn agent(s: &SessionSummary) -> MentionChip {
        MentionChip {
            kind: MentionKind::Agent,
            value: s.id.clone(),
            label: s.title.clone(),
            title: format!("{} · {} — sent as context", s.agent.display_name(), s.title),
            agent: Some(s.agent.clone()),
        }
    }

    /// A file or folder, as a workspace-relative path.
    pub fn path(path: &str, folder: bool) -> MentionChip {
        let trimmed = path.trim_end_matches('/');
        let name = trimmed.rsplit('/').next().unwrap_or(trimmed).to_string();
        MentionChip {
            kind: if folder { MentionKind::Folder } else { MentionKind::File },
            value: if folder { format!("{trimmed}/") } else { trimmed.to_string() },
            label: if name.is_empty() { trimmed.to_string() } else { name },
            title: path.to_string(),
            agent: None,
        }
    }

    pub fn git_diff() -> MentionChip {
        MentionChip {
            kind: MentionKind::GitDiff,
            value: "diff".into(),
            label: "Uncommitted changes".into(),
            title: "The workspace's git diff, sent as context".into(),
            agent: None,
        }
    }

    pub fn skill(name: &str, is_skill: bool, description: &str) -> MentionChip {
        let name = name.trim_start_matches('/');
        MentionChip {
            kind: if is_skill { MentionKind::Skill } else { MentionKind::Command },
            value: name.to_string(),
            label: format!("/{name}"),
            title: if description.is_empty() { format!("/{name}") } else { format!("/{name} — {description}") },
            agent: None,
        }
    }

    fn icon(&self) -> I {
        match self.kind {
            MentionKind::Agent => I::Message,
            MentionKind::File => I::File,
            MentionKind::Folder => I::Folder,
            MentionKind::GitDiff => I::Diff,
            MentionKind::Skill => I::Sparkle,
            MentionKind::Command => I::Slash,
            MentionKind::Pasted => I::Lines,
        }
    }
}

/// The prompt an agent receives for these chips and the typed text: a
/// leading `/skill`, `@path` mentions, then `@agent:<id>` / `@git:diff`
/// tokens (expanded into context blocks before sending), then the text.
pub fn to_prompt(chips: &[MentionChip], text: &str) -> String {
    let mut head: Vec<String> = Vec::new();
    let mut skills = chips.iter().filter(|c| matches!(c.kind, MentionKind::Skill | MentionKind::Command));
    if let Some(first) = skills.next() {
        head.push(format!("/{}", first.value));
    }
    for c in chips {
        match c.kind {
            MentionKind::File | MentionKind::Folder => head.push(format!("@{}", c.value)),
            MentionKind::Agent => head.push(format!("@agent:{}", &c.value[c.value.len().saturating_sub(8)..])),
            MentionKind::GitDiff => head.push("@git:diff".into()),
            _ => {}
        }
    }
    // Further skills can only be mentioned, not invoked.
    for c in skills {
        head.push(format!("/{}", c.value));
    }
    let text = text.trim();
    match (head.is_empty(), text.is_empty()) {
        (true, _) => text.to_string(),
        (false, true) => head.join(" "),
        (false, false) => format!("{} {}", head.join(" "), text),
    }
}

/// Fold what a sent prompt carries back into chips: context blocks
/// (`<agent_context>`, `<git_diff>`, `<pasted_text>`), `@agent:` /
/// `@git:diff` tokens, and the leading run of `/skill` and `@path` tokens.
/// Returns the text left for display and the chips.
pub fn split_mentions(text: &str) -> (String, Vec<MentionChip>) {
    let mut chips: Vec<MentionChip> = Vec::new();
    let mut rest = text.to_string();

    // Context blocks Larrier appended.
    for (open, close) in [("<agent_context", "</agent_context>"), ("<git_diff", "</git_diff>"), ("<pasted_text", "</pasted_text>")] {
        while let Some(start) = rest.find(open) {
            let after_open = &rest[start..];
            let Some(tag_end) = after_open.find('>') else { break };
            let tag = &after_open[..tag_end];
            let end = match after_open.find(close) {
                Some(i) => start + i + close.len(),
                None => rest.len(),
            };
            let attr = |name: &str| {
                let key = format!("{name}=\"");
                tag.find(&key).map(|i| {
                    let v = &tag[i + key.len()..];
                    v[..v.find('"').unwrap_or(v.len())].to_string()
                })
            };
            match open {
                "<agent_context" => {
                    let title = attr("title").unwrap_or_else(|| "Agent chat".into());
                    let agent_name = attr("agent").unwrap_or_default();
                    let agent = larrier_core::agent::catalog().iter().find(|s| s.name == agent_name).map(|s| s.kind.clone());
                    chips.push(MentionChip {
                        kind: MentionKind::Agent,
                        value: String::new(),
                        label: title.clone(),
                        title: if agent_name.is_empty() { title } else { format!("{agent_name} · {title} — sent as context") },
                        agent,
                    });
                }
                "<git_diff" => {
                    if !chips.iter().any(|c| c.kind == MentionKind::GitDiff) {
                        chips.push(MentionChip::git_diff());
                    }
                }
                _ => {
                    let name = attr("name").unwrap_or_else(|| "Pasted text".into());
                    chips.push(MentionChip { kind: MentionKind::Pasted, value: name.clone(), label: name.clone(), title: name, agent: None });
                }
            }
            rest.replace_range(start..end, "");
        }
    }

    // Tokens: the leading run of mentions, and agent / diff tokens anywhere.
    let mut lead = true;
    let mut kept: Vec<String> = Vec::new();
    let mut agent_tokens: Vec<String> = Vec::new();
    for line in rest.split('\n') {
        let mut out_words: Vec<&str> = Vec::new();
        for w in line.split(' ') {
            if w.is_empty() {
                if !out_words.is_empty() || !lead {
                    out_words.push(w);
                }
                continue;
            }
            if let Some(id) = w.strip_prefix("@agent:") {
                agent_tokens.push(id.to_string());
                continue;
            }
            if w == "@git:diff" {
                if !chips.iter().any(|c| c.kind == MentionKind::GitDiff) {
                    chips.push(MentionChip::git_diff());
                }
                continue;
            }
            if lead {
                if let Some(name) = w.strip_prefix('/').filter(|n| !n.is_empty() && kept.is_empty() && out_words.is_empty() && is_command_name(n)) {
                    chips.push(MentionChip::skill(name, true, ""));
                    continue;
                }
                if let Some(path) = w.strip_prefix('@').filter(|p| p.len() > 1 && (p.contains('/') || p.contains('.'))) {
                    chips.push(MentionChip::path(path, path.ends_with('/')));
                    continue;
                }
                lead = false;
            }
            out_words.push(w);
        }
        kept.push(out_words.join(" "));
    }
    // An agent token whose context block is missing (the chat was not found
    // when sending) still gets a chip.
    let blocks = chips.iter().filter(|c| c.kind == MentionKind::Agent).count();
    for id in agent_tokens.into_iter().skip(blocks) {
        chips.push(MentionChip { kind: MentionKind::Agent, value: id.clone(), label: "Agent chat".into(), title: format!("Agent chat {id}"), agent: None });
    }

    // Tidy the whitespace the removals left behind.
    let mut text_out = String::new();
    let mut blank = 0;
    for line in kept.iter().map(|l| l.trim_end()) {
        if line.trim().is_empty() {
            blank += 1;
            continue;
        }
        if !text_out.is_empty() {
            text_out.push_str(if blank > 0 { "\n\n" } else { "\n" });
        }
        blank = 0;
        text_out.push_str(line);
    }
    (text_out.trim().to_string(), chips)
}

fn is_command_name(n: &str) -> bool {
    n.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'))
}

/// Resolve agent chips that only carry a session id to the chat's title.
pub fn resolve_titles(chips: &mut [MentionChip], sessions: &[SessionSummary]) {
    for c in chips.iter_mut().filter(|c| c.kind == MentionKind::Agent && c.label == "Agent chat" && !c.value.is_empty()) {
        if let Some(s) = sessions.iter().find(|s| s.id.ends_with(&c.value)) {
            *c = MentionChip::agent(s);
        }
    }
}

/// One chip; `onremove` adds the × (composer).
#[component]
pub fn MentionChipView(chip: MentionChip, #[props(default)] onremove: Option<EventHandler<()>>) -> Element {
    let icon = chip.icon();
    let cls = match chip.kind {
        MentionKind::Skill | MentionKind::Command => "mention-chip skill",
        MentionKind::Agent => "mention-chip agent",
        _ => "mention-chip",
    };
    rsx! {
        span { class: "{cls}", title: "{chip.title}",
            span { class: "ic",
                if let Some(k) = chip.agent.clone() { AgentGlyph { kind: k } } else { Icon { i: icon, size: 12, stroke: 1.6 } }
            }
            span { class: "nm", "{chip.label}" }
            if let Some(cb) = onremove {
                button { class: "x", title: "Remove",
                    onclick: move |e| { e.stop_propagation(); cb.call(()); },
                    Icon { i: I::X, size: 10 }
                }
            }
        }
    }
}

/// Read-only chips for a sent prompt (agent titles resolved from the
/// sidebar's sessions).
#[component]
pub fn MentionChips(chips: Vec<MentionChip>) -> Element {
    let app = crate::state::use_app();
    let mut chips = chips;
    if chips.iter().any(|c| c.kind == MentionKind::Agent && c.label == "Agent chat") {
        resolve_titles(&mut chips, &app.sessions.read());
    }
    if chips.is_empty() {
        return rsx! {};
    }
    rsx! {
        div { class: "mention-row",
            for (i, c) in chips.into_iter().enumerate() {
                MentionChipView { key: "{i}", chip: c }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_from_chips() {
        let chips = vec![
            MentionChip::skill("docs", true, ""),
            MentionChip::path("src/main.rs", false),
            MentionChip::path("src/ui/", true),
            MentionChip::git_diff(),
        ];
        assert_eq!(to_prompt(&chips, " fix it "), "/docs @src/main.rs @src/ui/ @git:diff fix it");
        assert_eq!(to_prompt(&[], "hi"), "hi");
    }

    #[test]
    fn split_folds_tokens_and_blocks_back_into_chips() {
        let sent = "/docs @src/main.rs @agent:abcd1234 @git:diff fix the parser\nthanks\n\n<agent_context title=\"Parser work\" agent=\"Claude Code\">\nlots of context\n</agent_context>\n\n<git_diff>\ndiff --git a b\n</git_diff>";
        let (text, chips) = split_mentions(sent);
        assert_eq!(text, "fix the parser\nthanks");
        let kinds: Vec<MentionKind> = chips.iter().map(|c| c.kind).collect();
        assert!(kinds.contains(&MentionKind::Skill));
        assert!(kinds.contains(&MentionKind::File));
        assert!(kinds.contains(&MentionKind::GitDiff));
        assert_eq!(chips.iter().filter(|c| c.kind == MentionKind::GitDiff).count(), 1);
        let agent = chips.iter().find(|c| c.kind == MentionKind::Agent).unwrap();
        assert_eq!(agent.label, "Parser work");
        assert_eq!(chips.iter().filter(|c| c.kind == MentionKind::Agent).count(), 1, "token and block are one chip");
    }

    #[test]
    fn prose_is_left_alone() {
        let (text, chips) = split_mentions("email me at a@b.com about /usr/bin and @src/x.rs later");
        assert_eq!(text, "email me at a@b.com about /usr/bin and @src/x.rs later");
        assert!(chips.is_empty());
        let (t2, c2) = split_mentions("<pasted_text name=\"log.txt\">\nerr\n</pasted_text>\nwhat is this");
        assert_eq!(t2, "what is this");
        assert_eq!(c2[0].label, "log.txt");
    }
}
