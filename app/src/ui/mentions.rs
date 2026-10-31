//! Mentions as objects: the files, folders, other agents' chats, the git
//! diff, agent skills and subagents a prompt refers to. In a text box they
//! sit in the text as objects (marked tokens drawn as colored pills, removed
//! whole by one Backspace); on send they become what the agent understands
//! (`@path`, a leading `/skill`, context blocks); in the transcript the
//! injected text is folded back into objects in place.

use dioxus::prelude::*;
use larrier_core::{AgentKind, SessionSummary};

/// Marks around an object in a text box's text. Both are invisible and
/// zero-width (Unicode default-ignorables), so the box lays its text out
/// exactly as the colored copy behind it shows.
pub const OPEN: char = '\u{2063}';
pub const CLOSE: char = '\u{2064}';

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
    /// One of the agent's own subagents (Claude Code's `@agent-name`).
    Subagent,
    /// The agent of the user's own a new chat runs as (not sent as text).
    RunAs,
}

#[derive(Clone, PartialEq, Debug)]
pub struct MentionChip {
    pub kind: MentionKind,
    /// What is sent: session id (agent), path (file / folder), name (skill,
    /// command, paste, subagent).
    pub value: String,
    /// Shown on the chip.
    pub label: String,
    /// Tooltip.
    pub title: String,
    /// Glyph for agent chips.
    pub agent: Option<AgentKind>,
    /// The context block as it was sent, sent again as is when it can't be
    /// made afresh (a paste, a chat Larrier no longer has).
    pub block: Option<String>,
}

impl MentionChip {
    fn new(kind: MentionKind, value: String, label: String, title: String) -> MentionChip {
        MentionChip { kind, value, label, title, agent: None, block: None }
    }

    pub fn agent(s: &SessionSummary) -> MentionChip {
        MentionChip {
            agent: Some(s.agent.clone()),
            ..MentionChip::new(MentionKind::Agent, s.id.clone(), s.title.clone(), format!("{} · {} — sent as context", s.agent.display_name(), s.title))
        }
    }

    /// A file or folder, as a workspace-relative path.
    pub fn path(path: &str, folder: bool) -> MentionChip {
        let trimmed = path.trim_end_matches('/');
        let name = trimmed.rsplit('/').next().unwrap_or(trimmed).to_string();
        MentionChip::new(
            if folder { MentionKind::Folder } else { MentionKind::File },
            if folder { format!("{trimmed}/") } else { trimmed.to_string() },
            if name.is_empty() { trimmed.to_string() } else { name },
            path.to_string(),
        )
    }

    pub fn git_diff() -> MentionChip {
        MentionChip::new(MentionKind::GitDiff, "diff".into(), "Uncommitted changes".into(), "The workspace's git diff, sent as context".into())
    }

    pub fn skill(name: &str, is_skill: bool, description: &str) -> MentionChip {
        let name = name.trim_start_matches('/');
        MentionChip::new(
            if is_skill { MentionKind::Skill } else { MentionKind::Command },
            name.to_string(),
            format!("/{name}"),
            if description.is_empty() { format!("/{name}") } else { format!("/{name} — {description}") },
        )
    }

    pub fn subagent(name: &str, description: &str) -> MentionChip {
        let title = if description.is_empty() { format!("The {name} subagent") } else { format!("The {name} subagent — {description}") };
        MentionChip::new(MentionKind::Subagent, name.to_string(), name.to_string(), title)
    }

    pub fn run_as(name: &str, agent: &AgentKind) -> MentionChip {
        let title = format!("This chat runs as your {name} agent: its instructions, tools and MCP servers");
        MentionChip { agent: Some(agent.clone()), ..MentionChip::new(MentionKind::RunAs, name.to_string(), name.to_string(), title) }
    }

    /// How the object reads in a text box: `/name` or `@name`.
    pub fn token(&self) -> String {
        let label: String = self.label.chars().filter(|c| !matches!(*c, OPEN | CLOSE | '\n' | '\r')).collect();
        let label = label.trim_start_matches('@').to_string();
        let label = if label.chars().count() > 48 { format!("{}…", label.chars().take(47).collect::<String>().trim_end()) } else { label };
        match self.kind {
            MentionKind::Skill | MentionKind::Command => format!("/{}", label.trim_start_matches('/')),
            MentionKind::Folder => format!("@{}/", label.trim_end_matches('/')),
            _ => format!("@{label}"),
        }
    }

    /// The token as it sits in a text box's text.
    pub fn marked(&self) -> String {
        format!("{OPEN}{}{CLOSE}", self.token())
    }

    fn class(&self) -> &'static str {
        match self.kind {
            MentionKind::Skill | MentionKind::Command => "skill",
            MentionKind::Agent => "agent",
            MentionKind::RunAs | MentionKind::Subagent => "run-as",
            MentionKind::GitDiff => "diff",
            _ => "file",
        }
    }

}

/// A piece of a prompt: plain text or an object.
#[derive(Clone, PartialEq, Debug)]
pub enum Seg {
    Text(String),
    Chip(MentionChip),
}

/// A text box's text in pieces: its marked tokens become the matching
/// objects (a token nothing matches stays text).
pub fn token_segs(chips: &[MentionChip], text: &str) -> Vec<Seg> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(a) = rest.find(OPEN) {
        let after = &rest[a + OPEN.len_utf8()..];
        let Some(b) = after.find(CLOSE) else { break };
        let tok = &after[..b];
        if a > 0 {
            out.push(Seg::Text(rest[..a].to_string()));
        }
        match chips.iter().find(|c| c.token() == tok) {
            Some(c) => out.push(Seg::Chip(c.clone())),
            None => out.push(Seg::Text(tok.to_string())),
        }
        rest = &after[b + CLOSE.len_utf8()..];
    }
    let rest: String = rest.chars().filter(|c| !matches!(*c, OPEN | CLOSE)).collect();
    if !rest.is_empty() {
        out.push(Seg::Text(rest));
    }
    out
}

/// What the agent receives for a text box's text: each object becomes what
/// the agent understands, in place, and the first skill or command leads
/// (agents only run one at the start). Context blocks that can't be made
/// afresh follow the text.
pub fn to_prompt(chips: &[MentionChip], text: &str) -> String {
    let segs = token_segs(chips, text);
    let lead = segs.iter().position(|s| matches!(s, Seg::Chip(c) if matches!(c.kind, MentionKind::Skill | MentionKind::Command)));
    let mut head = String::new();
    let mut body = String::new();
    let mut blocks: Vec<String> = Vec::new();
    for (i, s) in segs.iter().enumerate() {
        let piece = match s {
            Seg::Text(t) => t.clone(),
            Seg::Chip(c) => {
                if let Some(b) = c.block.as_ref().filter(|_| c.kind == MentionKind::Pasted || (c.kind == MentionKind::Agent && c.value.is_empty())) {
                    blocks.push(b.clone());
                }
                match c.kind {
                    MentionKind::Skill | MentionKind::Command => format!("/{}", c.value),
                    MentionKind::File | MentionKind::Folder => format!("@{}", c.value),
                    MentionKind::Agent if !c.value.is_empty() => format!("@agent:{}", &c.value[c.value.len().saturating_sub(8)..]),
                    MentionKind::GitDiff => "@git:diff".into(),
                    MentionKind::Subagent => format!("@agent-{}", c.value),
                    _ => String::new(),
                }
            }
        };
        if Some(i) == lead {
            head = piece;
        } else {
            body.push_str(&piece);
        }
    }
    // Objects that send nothing leave their spaces behind.
    let body = body.split('\n').map(|l| l.split(' ').filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join("\n");
    let body = body.trim();
    let mut out = match (head.is_empty(), body.is_empty()) {
        (true, _) => body.to_string(),
        (false, true) => head,
        (false, false) => format!("{head} {body}"),
    };
    for b in blocks {
        out.push_str("\n\n");
        out.push_str(&b);
    }
    out
}

/// A sent prompt back into a text box's text and its objects, to edit it.
pub fn to_edit(text: &str, sessions: &[SessionSummary]) -> (String, Vec<MentionChip>) {
    let mut chips: Vec<MentionChip> = Vec::new();
    let mut out = String::new();
    for s in prompt_segs(text, sessions) {
        match s {
            Seg::Text(t) => out.push_str(&t),
            Seg::Chip(c) => {
                let c = unique(&chips, c);
                out.push_str(&c.marked());
                chips.push(c);
            }
        }
    }
    (out.trim().to_string(), chips)
}

/// The object with a token no other object in `chips` has (a second
/// `mod.rs` shows its folder too).
pub fn unique(chips: &[MentionChip], mut c: MentionChip) -> MentionChip {
    let clash = |c: &MentionChip| chips.iter().any(|o| o.token() == c.token() && (o.kind != c.kind || o.value != c.value));
    if clash(&c) && matches!(c.kind, MentionKind::File | MentionKind::Folder) {
        c.label = c.value.trim_end_matches('/').to_string();
    }
    let base = c.label.clone();
    let mut n = 2;
    while clash(&c) {
        c.label = format!("{base} {n}");
        n += 1;
    }
    c
}

/// A sent prompt in pieces, its objects in place: context blocks Larrier
/// appended (`<agent_context>`, `<git_diff>`, `<pasted_text>`) fold into the
/// object that asked for them; `@agent:`, `@git:diff`, `@agent-name` and
/// `@path` tokens and a leading `/skill` become objects.
pub fn prompt_segs(text: &str, sessions: &[SessionSummary]) -> Vec<Seg> {
    let mut rest = text.to_string();
    let mut agent_blocks: Vec<MentionChip> = Vec::new();
    let mut tail: Vec<MentionChip> = Vec::new();
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
            let block = rest[start..end].to_string();
            match open {
                "<agent_context" => {
                    let title = attr("title").unwrap_or_else(|| "Agent chat".into());
                    let agent_name = attr("agent").unwrap_or_default();
                    let agent = larrier_core::agent::catalog().iter().find(|s| s.name == agent_name).map(|s| s.kind.clone());
                    let tip = if agent_name.is_empty() { title.clone() } else { format!("{agent_name} · {title} — sent as context") };
                    agent_blocks.push(MentionChip { agent, block: Some(block), ..MentionChip::new(MentionKind::Agent, String::new(), title, tip) });
                }
                "<git_diff" => {}
                _ => {
                    let name = attr("name").unwrap_or_else(|| "Pasted text".into());
                    tail.push(MentionChip { block: Some(block), ..MentionChip::new(MentionKind::Pasted, name.clone(), name.clone(), name) });
                }
            }
            rest.replace_range(start..end, "");
        }
    }
    let rest = rest.trim_end();

    let mut segs: Vec<Seg> = Vec::new();
    let mut text_buf = String::new();
    let mut blocks = agent_blocks.into_iter();
    let mut first_word = true;
    let mut i = 0;
    let bytes: Vec<(usize, char)> = rest.char_indices().collect();
    while i < bytes.len() {
        let (start, ch) = bytes[i];
        if ch.is_whitespace() {
            text_buf.push(ch);
            i += 1;
            continue;
        }
        let mut j = i;
        while j < bytes.len() && !bytes[j].1.is_whitespace() {
            j += 1;
        }
        let end = if j < bytes.len() { bytes[j].0 } else { rest.len() };
        let word = &rest[start..end];
        let lead = first_word;
        first_word = false;
        i = j;
        let mut trailing = "";
        let chip = if let Some(id) = word.strip_prefix("@agent:") {
            let mut c = match blocks.next() {
                Some(b) => b,
                None => MentionChip::new(MentionKind::Agent, String::new(), "Agent chat".into(), format!("Agent chat {id}")),
            };
            c.value = id.to_string();
            if let Some(s) = sessions.iter().find(|s| !id.is_empty() && s.id.ends_with(id)) {
                let block = c.block.take();
                c = MentionChip { block, ..MentionChip::agent(s) };
            }
            Some(c)
        } else if word == "@git:diff" {
            Some(MentionChip::git_diff())
        } else if let Some(name) = word.strip_prefix("@agent-").filter(|n| !n.is_empty() && is_command_name(n)) {
            Some(MentionChip::subagent(name, ""))
        } else if let Some(name) = word.strip_prefix('/').filter(|n| lead && !n.is_empty() && is_command_name(n)) {
            Some(MentionChip::skill(name, true, ""))
        } else if let Some(p) = word.strip_prefix('@') {
            let path = p.trim_end_matches([',', ';', ':', '!', '?', ')', '.']);
            trailing = &p[path.len()..];
            (path.len() > 1 && (path.contains('/') || path.contains('.'))).then(|| MentionChip::path(path, path.ends_with('/')))
        } else {
            None
        };
        match chip {
            Some(c) => {
                if !text_buf.is_empty() {
                    segs.push(Seg::Text(std::mem::take(&mut text_buf)));
                }
                segs.push(Seg::Chip(c));
                text_buf.push_str(trailing);
            }
            None => text_buf.push_str(word),
        }
    }
    if !text_buf.trim().is_empty() {
        segs.push(Seg::Text(text_buf));
    }
    // Chats sent as context without a token in the text lead; pastes follow.
    let segs: Vec<Seg> = segs.into_iter().skip_while(|s| matches!(s, Seg::Text(t) if t.trim().is_empty())).collect();
    let mut out: Vec<Seg> = Vec::new();
    for c in blocks {
        out.push(Seg::Chip(c));
        out.push(Seg::Text(" ".into()));
    }
    out.extend(segs);
    if !out.is_empty() && !tail.is_empty() {
        out.push(Seg::Text("\n".into()));
    }
    out.extend(tail.into_iter().map(Seg::Chip));
    while matches!(out.last(), Some(Seg::Text(t)) if t.trim().is_empty()) {
        out.pop();
    }
    out
}


/// A sent prompt's text with its objects taken out, and the objects (for
/// one-line previews).
pub fn split_mentions(text: &str) -> (String, Vec<MentionChip>) {
    let mut chips = Vec::new();
    let mut out = String::new();
    for s in prompt_segs(text, &[]) {
        match s {
            Seg::Text(t) => out.push_str(&t),
            Seg::Chip(c) => chips.push(c),
        }
    }
    let out = out.split('\n').map(|l| l.split(' ').filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join("\n");
    (out.trim().to_string(), chips)
}

fn is_command_name(n: &str) -> bool {
    n.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'))
}

/// What is sent for a text box's text: its objects as the agent reads them,
/// chats and the diff expanded into context blocks.
pub fn expand(app: &crate::state::App, chips: &[MentionChip], text: &str, cwd: Option<&str>) -> String {
    let raw = to_prompt(chips, text);
    let mut extra = String::new();
    for tok in raw.split_whitespace() {
        if let Some(short) = tok.strip_prefix("@agent:") {
            let found = app.sessions.peek().iter().find(|s| s.id.ends_with(short)).map(|s| s.id.clone());
            let loaded = found.and_then(|id| crate::state::store().load(&id).ok());
            match loaded {
                Some(s) => {
                    let ctx = s.handoff_context(0, 24_000);
                    extra.push_str(&format!("\n\n<agent_context title=\"{}\" agent=\"{}\">\n{}\n</agent_context>", s.title, s.agent.display_name(), ctx.trim()));
                }
                // A chat that's gone: the context it was sent with.
                None => {
                    if let Some(b) = chips.iter().find(|c| c.kind == MentionKind::Agent && c.value.ends_with(short)).and_then(|c| c.block.clone()) {
                        extra.push_str("\n\n");
                        extra.push_str(&b);
                    }
                }
            }
        } else if tok == "@git:diff" {
            if let Some(dir) = cwd {
                let out = std::process::Command::new("git").args(["diff", "HEAD", "--no-color"]).current_dir(dir).output();
                if let Ok(o) = out {
                    let d = String::from_utf8_lossy(&o.stdout).to_string();
                    let d: String = d.chars().take(60_000).collect();
                    if !d.trim().is_empty() {
                        extra.push_str(&format!("\n\n<git_diff>\n{d}\n</git_diff>"));
                    }
                }
            }
        }
    }
    format!("{raw}{extra}")
}

/// A sent prompt with its objects in place, drawn as the text box draws
/// them (so editing it moves nothing).
#[component]
pub fn PromptText(text: String) -> Element {
    let app = crate::state::use_app();
    let segs = prompt_segs(&text, &app.sessions.peek());
    rsx! {
        for (i, s) in segs.into_iter().enumerate() {
            match s {
                Seg::Text(t) => rsx! { span { key: "{i}", "{t}" } },
                Seg::Chip(c) => rsx! { span { key: "{i}", class: "tok {c.class()}", title: "{c.title}", "{c.token()}" } },
            }
        }
    }
}

/// The colored copy drawn behind a text box: its text, with each object's
/// token as a pill (same text, same layout as the box itself).
#[component]
pub fn TokenBackdrop(text: String, chips: Vec<MentionChip>) -> Element {
    let segs = token_segs(&chips, &text);
    rsx! {
        div { class: "tok-back", "aria-hidden": "true",
            for (i, s) in segs.into_iter().enumerate() {
                match s {
                    Seg::Text(t) => rsx! { span { key: "{i}", "{t}" } },
                    Seg::Chip(c) => rsx! { span { key: "{i}", class: "tok {c.class()}", "{OPEN}{c.token()}{CLOSE}" } },
                }
            }
            // Keeps a last empty line as tall as the box's.
            "\u{200b}"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_from_objects_in_text() {
        let chips = vec![
            MentionChip::skill("docs", true, ""),
            MentionChip::path("src/main.rs", false),
            MentionChip::path("src/ui/", true),
            MentionChip::git_diff(),
            MentionChip::subagent("Explore", ""),
            MentionChip::run_as("site-builder", &AgentKind::ClaudeCode),
        ];
        let m = |i: usize| chips[i].marked();
        let text = format!("{} fix {} in {} and {} with {} {}", m(5), m(1), m(2), m(3), m(4), m(0));
        assert_eq!(to_prompt(&chips, &text), "/docs fix @src/main.rs in @src/ui/ and @git:diff with @agent-Explore");
        assert_eq!(to_prompt(&[], "hi"), "hi");
        // A token nothing knows stays as its text.
        assert_eq!(to_prompt(&[], &format!("{OPEN}@gone{CLOSE} hi")), "@gone hi");
    }

    #[test]
    fn sent_prompt_folds_back_into_objects_in_place() {
        let sent = "/docs @src/main.rs @agent:abcd1234 @git:diff fix the parser\nthanks\n\n<agent_context title=\"Parser work\" agent=\"Claude Code\">\nlots of context\n</agent_context>\n\n<git_diff>\ndiff --git a b\n</git_diff>";
        let segs = prompt_segs(sent, &[]);
        let kinds: Vec<MentionKind> = segs.iter().filter_map(|s| if let Seg::Chip(c) = s { Some(c.kind) } else { None }).collect();
        assert_eq!(kinds, vec![MentionKind::Skill, MentionKind::File, MentionKind::Agent, MentionKind::GitDiff]);
        let agent = segs.iter().find_map(|s| if let Seg::Chip(c) = s { (c.kind == MentionKind::Agent).then_some(c) } else { None }).unwrap();
        assert_eq!(agent.label, "Parser work");
        assert_eq!(agent.value, "abcd1234");
        let (text, chips) = split_mentions(sent);
        assert_eq!(text, "fix the parser\nthanks");
        assert_eq!(chips.len(), 4);
    }

    #[test]
    fn edit_round_trip() {
        let sent = "/docs check @src/a.rs, then @agent-Explore it\n\n<pasted_text name=\"log.txt\">\nerr\n</pasted_text>";
        let (text, chips) = to_edit(sent, &[]);
        assert!(text.starts_with(&format!("{OPEN}/docs{CLOSE} check {OPEN}@a.rs{CLOSE}, then {OPEN}@Explore{CLOSE} it")), "{text:?}");
        let again = to_prompt(&chips, &text);
        assert_eq!(again, "/docs check @src/a.rs, then @agent-Explore it\n\n<pasted_text name=\"log.txt\">\nerr\n</pasted_text>");
    }

    #[test]
    fn same_names_get_told_apart() {
        let a = MentionChip::path("src/ui/mod.rs", false);
        let b = unique(&[a.clone()], MentionChip::path("src/core/mod.rs", false));
        assert_eq!(a.token(), "@mod.rs");
        assert_eq!(b.token(), "@src/core/mod.rs");
    }

    #[test]
    fn prose_is_left_alone() {
        let (text, chips) = split_mentions("email me at a@b.com about /usr/bin later");
        assert_eq!(text, "email me at a@b.com about /usr/bin later");
        assert!(chips.is_empty());
        let (t2, c2) = split_mentions("<pasted_text name=\"log.txt\">\nerr\n</pasted_text>\nwhat is this");
        assert_eq!(t2, "what is this");
        assert_eq!(c2[0].label, "log.txt");
    }
}
