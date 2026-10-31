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

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
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
    /// A page from Larrier's browser (its address, title and text, as it
    /// was when it was added), sent as a block.
    Page,
    /// Text quoted from an agent's reply, sent in place as a quote block.
    Quote,
}

/// Where a quote sits while the text around it is tidied.
const QUOTE_MARK: (char, char) = ('\u{E000}', '\u{E001}');

#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
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

    /// A browser page as it is now: its title, address and text.
    pub fn page(title: &str, url: &str, text: &str) -> MentionChip {
        let title = if title.trim().is_empty() { url.to_string() } else { title.trim().to_string() };
        let attr = |s: &str| s.replace('"', "'").replace(['\n', '\r'], " ");
        let block = format!("<browser_page title=\"{}\" url=\"{}\">\n{}\n</browser_page>", attr(&title), attr(url), text.trim());
        MentionChip { block: Some(block), ..MentionChip::new(MentionKind::Page, url.to_string(), title.clone(), format!("{title} — {url} — sent as context")) }
    }

    /// Text quoted from an agent's reply.
    pub fn quote(text: &str) -> MentionChip {
        let text = text.trim();
        let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let label = if flat.chars().count() > 32 { format!("{}…", flat.chars().take(31).collect::<String>().trim_end()) } else { flat };
        MentionChip { block: Some(format!("<quote>\n{text}\n</quote>")), ..MentionChip::new(MentionKind::Quote, text.to_string(), label, text.to_string()) }
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
            MentionKind::Quote => format!("“{}”", label.trim_end_matches('…').trim_end().to_string() + if label.ends_with('…') { "…" } else { "" }),
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
            MentionKind::Page => "page",
            MentionKind::Quote => "quote",
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
    let mut quotes: Vec<String> = Vec::new();
    for (i, s) in segs.iter().enumerate() {
        let piece = match s {
            Seg::Text(t) => t.clone(),
            Seg::Chip(c) => {
                if let Some(b) = c.block.as_ref().filter(|_| matches!(c.kind, MentionKind::Pasted | MentionKind::Page) || (c.kind == MentionKind::Agent && c.value.is_empty())) {
                    blocks.push(b.clone());
                }
                match c.kind {
                    MentionKind::Skill | MentionKind::Command => format!("/{}", c.value),
                    MentionKind::File | MentionKind::Folder => format!("@{}", c.value),
                    MentionKind::Agent if !c.value.is_empty() => format!("@agent:{}", &c.value[c.value.len().saturating_sub(8)..]),
                    MentionKind::GitDiff => "@git:diff".into(),
                    MentionKind::Subagent => format!("@agent-{}", c.value),
                    // In place, once the text around it is tidied.
                    MentionKind::Quote => {
                        quotes.push(c.block.clone().unwrap_or_else(|| format!("<quote>\n{}\n</quote>", c.value)));
                        format!(" {}{}{} ", QUOTE_MARK.0, quotes.len() - 1, QUOTE_MARK.1)
                    }
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
    if !quotes.is_empty() {
        for (i, q) in quotes.iter().enumerate() {
            let mark = format!("{}{i}{}", QUOTE_MARK.0, QUOTE_MARK.1);
            out = out.replace(&format!(" {mark}"), &mark).replace(&format!("{mark} "), &mark);
            out = out.replace(&mark, &format!("\n{q}\n"));
        }
        // Each quote on lines of its own, right against the text around it.
        while out.contains("\n\n<quote>") {
            out = out.replace("\n\n<quote>", "\n<quote>");
        }
        while out.contains("</quote>\n\n") {
            out = out.replace("</quote>\n\n", "</quote>\n");
        }
        out = out.trim().to_string();
    }
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
/// appended (`<agent_context>`, `<git_diff>`, `<pasted_text>`,
/// `<browser_page>`) fold into the object that asked for them; `@agent:`, `@git:diff`, `@agent-name` and
/// `@path` tokens and a leading `/skill` become objects.
pub fn prompt_segs(text: &str, sessions: &[SessionSummary]) -> Vec<Seg> {
    let mut rest = text.to_string();
    let mut agent_blocks: Vec<MentionChip> = Vec::new();
    let mut tail: Vec<MentionChip> = Vec::new();
    // Quotes fold back where they were.
    let mut quotes: Vec<MentionChip> = Vec::new();
    while let Some(start) = rest.find("<quote>") {
        let inner_at = start + "<quote>".len();
        let (inner_end, end) = match rest[inner_at..].find("</quote>") {
            Some(i) => (inner_at + i, inner_at + i + "</quote>".len()),
            None => (rest.len(), rest.len()),
        };
        quotes.push(MentionChip::quote(&rest[inner_at..inner_end]));
        rest.replace_range(start..end, &format!(" {}{}{} ", QUOTE_MARK.0, quotes.len() - 1, QUOTE_MARK.1));
    }
    for (open, close) in [("<agent_context", "</agent_context>"), ("<git_diff", "</git_diff>"), ("<pasted_text", "</pasted_text>"), ("<browser_page", "</browser_page>")] {
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
                "<browser_page" => {
                    let url = attr("url").unwrap_or_default();
                    let title = attr("title").filter(|t| !t.is_empty()).unwrap_or_else(|| url.clone());
                    let tip = format!("{title} — {url} — sent as context");
                    tail.push(MentionChip { block: Some(block), ..MentionChip::new(MentionKind::Page, url, title, tip) });
                }
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
        let quote = word.strip_prefix(QUOTE_MARK.0).and_then(|w| w.strip_suffix(QUOTE_MARK.1)).and_then(|n| n.parse::<usize>().ok()).and_then(|n| quotes.get(n));
        let chip = if let Some(q) = quote {
            Some(q.clone())
        } else if let Some(id) = word.strip_prefix("@agent:") {
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
    fn browser_page_is_sent_as_a_block_and_folds_back() {
        let page = MentionChip::page("Order \"form\"", "http://127.0.0.1:3000/order", "Order form
Place order");
        let text = format!("why is {} broken?", page.marked());
        let sent = to_prompt(&[page], &text);
        assert_eq!(sent, "why is broken?

<browser_page title=\"Order 'form'\" url=\"http://127.0.0.1:3000/order\">
Order form
Place order
</browser_page>");
        let segs = prompt_segs(&sent, &[]);
        let chip = segs.iter().find_map(|s| if let Seg::Chip(c) = s { Some(c.clone()) } else { None }).unwrap();
        assert_eq!((chip.kind, chip.label.as_str(), chip.value.as_str()), (MentionKind::Page, "Order 'form'", "http://127.0.0.1:3000/order"));
        // Sent again as it was.
        let (again, chips) = to_edit(&sent, &[]);
        assert_eq!(to_prompt(&chips, &again), sent);
    }

    #[test]
    fn quotes_are_sent_in_place_and_fold_back() {
        let a = MentionChip::quote("Both work on Local.\n  Checking what production still lacks.");
        let b = MentionChip::quote("fn main() {\n    println!(\"hi\");\n}");
        assert_eq!(a.token(), "“Both work on Local. Checking wh…”");
        let text = format!("{} why?\n{} and this", a.marked(), b.marked());
        let sent = to_prompt(&[a.clone(), b.clone()], &text);
        assert_eq!(sent, "<quote>\nBoth work on Local.\n  Checking what production still lacks.\n</quote>\nwhy?\n<quote>\nfn main() {\n    println!(\"hi\");\n}\n</quote>\nand this");
        let segs = prompt_segs(&sent, &[]);
        let kinds: Vec<String> = segs.iter().map(|s| match s { Seg::Chip(c) => format!("{:?}", c.kind), Seg::Text(t) => t.trim().to_string() }).filter(|s| !s.is_empty()).collect();
        assert_eq!(kinds, ["Quote", "why?", "Quote", "and this"]);
        // Edited and sent again as it was.
        let (again, chips) = to_edit(&sent, &[]);
        assert_eq!(to_prompt(&chips, &again), sent);
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
