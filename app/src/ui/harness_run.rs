//! How Lary connects to a harness (Settings → Harnesses → one agent →
//! Connection): the ways Lary can drive it, each with the command it runs
//! and what it can do there, and the settings of its program. Claude Code,
//! Codex, Cursor and Grok have one way, their makers' own.

use dioxus::prelude::*;
use lary_core::{AgentKind, Config};

use super::{Select, Toggle};
use crate::state::use_app;

/// One way Lary runs a harness.
#[derive(Clone, PartialEq)]
pub struct Way {
    /// Over the Agent Client Protocol (`prefer_acp`), else its own protocol.
    pub acp: bool,
    pub label: &'static str,
    /// Its name in a few words, for the list of harnesses.
    pub short: &'static str,
    pub command: String,
    pub detail: String,
    /// What its maker documents for an app like Lary.
    pub recommended: bool,
    /// The only way, its maker's own (see `config::official_acp`).
    pub official: bool,
}

const NO_ASKS: &str = "It can't ask while it runs: what would need asking is refused, and Lary says so.";

/// The harness's own protocol, as Lary's driver runs it.
fn native(kind: &AgentKind) -> Option<(&'static str, &'static str, &'static str, String)> {
    Some(match kind {
        AgentKind::ClaudeCode => (
            "Claude Agent SDK protocol",
            "SDK",
            "claude -p --input-format stream-json --output-format stream-json --permission-prompt-tool stdio",
            "How the Claude Agent SDK runs Claude Code. Live approvals, questions, plans and steering.".into(),
        ),
        AgentKind::Codex => (
            "Codex app server",
            "app server",
            "codex app-server",
            "OpenAI's protocol for apps, as Codex's own use it. Live approvals, questions, plans and steering.".into(),
        ),
        AgentKind::Cursor => ("Print mode", "print mode", "cursor-agent -p --output-format stream-json", NO_ASKS.into()),
        AgentKind::Gemini => ("Headless mode", "headless", "gemini -p … -o stream-json", NO_ASKS.into()),
        AgentKind::OpenCode => ("Run mode", "run mode", "opencode run --format json", NO_ASKS.into()),
        AgentKind::Pi => ("JSON mode", "JSON mode", "pi --mode json", NO_ASKS.into()),
        AgentKind::Hermes => ("One-shot mode", "one-shot", "hermes chat --oneshot --format stream-json", NO_ASKS.into()),
        AgentKind::Droid => ("Exec mode", "exec mode", "droid exec -o stream-json", NO_ASKS.into()),
        AgentKind::Amp => ("Stream mode", "stream mode", "amp -x … --stream-json", NO_ASKS.into()),
        AgentKind::Qwen => ("Headless mode", "headless", "qwen -p … --output-format stream-json", NO_ASKS.into()),
        AgentKind::Aider => ("One message at a time", "plain text", "aider --message …", "Lary shows what it prints; it doesn't stream steps or ask.".into()),
        _ => return None,
    })
}

/// Every way Lary can run `kind`, its own protocol first; for Claude Code,
/// Codex, Cursor and Grok only their official one.
pub fn ways(kind: &AgentKind) -> Vec<Way> {
    let official = lary_core::config::official_acp(kind);
    let mut all = every_way(kind);
    if let Some(acp) = official {
        all.retain(|w| w.acp == acp);
        all.iter_mut().for_each(|w| w.official = true);
    }
    all
}

fn every_way(kind: &AgentKind) -> Vec<Way> {
    let spec = kind.spec();
    let mut out = Vec::new();
    if let Some((label, short, command, detail)) = native(kind) {
        // Their makers point apps at these; the rest at ACP, where they can ask.
        let recommended = matches!(kind, AgentKind::ClaudeCode | AgentKind::Codex | AgentKind::Amp | AgentKind::Aider);
        out.push(Way { acp: false, label, short, command: command.to_string(), detail, recommended, official: false });
    }
    if let Some(cmd) = spec.acp_command {
        let command = cmd.join(" ");
        // An adapter npm fetches, or the agent's own ACP mode.
        let adapter = cmd.first() == Some(&"npx");
        let package = cmd.iter().find(|a| a.starts_with('@') || a.ends_with("-acp")).copied().unwrap_or_default();
        let (label, detail) = match kind {
            AgentKind::ClaudeCode => (
                "Claude Agent SDK, over ACP",
                format!("{package}, the ACP project's adapter built on the Claude Agent SDK. npm downloads it the first time. Live approvals, questions and plans."),
            ),
            AgentKind::Codex => (
                "Agent Client Protocol",
                format!("{package}, the ACP project's adapter, which runs the app server. npm downloads it the first time."),
            ),
            _ if adapter => (
                "Agent Client Protocol",
                format!("Through {package}, an adapter npm downloads the first time. Live approvals, questions and plans."),
            ),
            _ => ("Agent Client Protocol", "Its own ACP mode: live approvals, questions and plans.".to_string()),
        };
        let recommended = !matches!(kind, AgentKind::ClaudeCode | AgentKind::Codex | AgentKind::Amp | AgentKind::Aider);
        out.push(Way { acp: true, label, short: "ACP", command, detail, recommended, official: false });
    }
    out
}

/// The way `kind` runs now, in a word or two (empty when it has one way).
pub fn current_label(cfg: &Config, kind: &AgentKind) -> String {
    let all = ways(kind);
    if all.len() < 2 {
        return String::new();
    }
    let acp = cfg.agent(kind).acp();
    all.iter().find(|w| w.acp == acp).map(|w| w.short.to_string()).unwrap_or_default()
}

#[component]
pub fn RunPanel(kind: AgentKind) -> Element {
    let app = use_app();
    let spec = kind.spec();
    let custom = match &kind {
        AgentKind::Custom(id) => app.config.read().custom_agent(id).cloned(),
        _ => None,
    };
    let ac = app.config.read().agent(&kind);
    let all = ways(&kind);
    let d = app.detected.read().iter().find(|d| d.kind == kind).cloned();
    let found = d.as_ref().and_then(|d| d.path.clone()).map(|p| p.to_string_lossy().to_string());
    let (k1, k2, k3) = (kind.clone(), kind.clone(), kind.clone());
    if let Some(c) = custom {
        return rsx! {
            div { class: "set-card ad-card",
                div { class: "ad-row",
                    span { class: "k", "Command" }
                    div { class: "v", span { class: "cmd mono", title: "{c.command} {c.args.join(\" \")}", "{c.command} {c.args.join(\" \")}" } }
                }
                div { class: "ad-row",
                    span { class: "k", "Protocol" }
                    div { class: "v", span { class: "plain", "{super::settings::protocol_label(c.protocol)}" } }
                }
            }
        };
    }
    rsx! {
        if !all.is_empty() {
            div { class: "hn-line", if all.len() > 1 { "How Lary connects to {spec.name}. Pick one; new chats use it." } else { "How Lary connects to {spec.name}." } }
            div { class: "set-card hn-ways",
                for w in all.clone() {
                    {
                        let on = all.len() == 1 || w.acp == ac.acp();
                        let k = kind.clone();
                        let acp = w.acp;
                        rsx! {
                            button { key: "{w.label}", class: if on { "hn-way on" } else { "hn-way" }, disabled: all.len() == 1,
                                onclick: move |_| { let k = k.clone(); app.update_config(move |c| c.agent_mut(&k).prefer_acp = acp); },
                                // One way is no choice: no radio.
                                if all.len() > 1 { span { class: "hn-radio" } }
                                div { class: "lbl",
                                    div { class: "t", "{w.label}"
                                        if w.official { span { class: "hn-tag", "Official" } } else if w.recommended && all.len() > 1 { span { class: "hn-tag", "Recommended" } }
                                    }
                                    div { class: "d", "{w.detail}" }
                                    div { class: "cmd mono", "{w.command}" }
                                }
                            }
                        }
                    }
                }
            }
        }
        div { class: "set-group-title", "Program" }
        div { class: "set-card ad-card",
            div { class: "ad-row",
                span { class: "k", "Executable" }
                div { class: "v",
                    input { class: "input full", spellcheck: "false",
                        placeholder: found.as_deref().map(super::settings::short_path).unwrap_or_else(|| spec.binaries.join(" / ")),
                        value: "{ac.binary.clone().unwrap_or_default()}",
                        onchange: move |e| { let v = e.value(); app.update_config(|c| c.agent_mut(&k1).binary = if v.trim().is_empty() { None } else { Some(v) }); app.detect_agents(); },
                    }
                    div { class: "h", if found.is_some() { "Found on this computer. Set a path to use another copy." } else { "Not found: install it from Version, or set its path." } }
                }
            }
            for (gi, g) in spec.mode_groups.iter().enumerate() {
                {
                    let kk = kind.clone();
                    let current = if gi == 0 { ac.default_mode.clone() } else { ac.default_permission.clone() };
                    let auto_label = format!("{} (from Autonomy)", g.default_for(app.config.read().settings.run_mode).label);
                    let options: Vec<(String, String)> = std::iter::once((String::new(), auto_label))
                        .chain(g.options.iter().map(|o| (o.id.to_string(), o.label.to_string())))
                        .collect();
                    rsx! {
                        div { key: "m{gi}", class: "ad-row",
                            span { class: "k", "Default {g.label.to_lowercase()}" }
                            div { class: "v",
                                Select { value: current.unwrap_or_default(), options, wide: true,
                                    onchange: move |v: String| {
                                        let v = if v.is_empty() { None } else { Some(v) };
                                        let k = kk.clone();
                                        app.update_config(move |c| if gi == 0 { c.agent_mut(&k).default_mode = v } else { c.agent_mut(&k).default_permission = v });
                                    } }
                            }
                        }
                    }
                }
            }
            div { class: "ad-row",
                span { class: "k", "Extra arguments" }
                div { class: "v",
                    input { class: "input full", placeholder: "e.g. --max-turns 40", spellcheck: "false",
                        value: "{ac.extra_args.join(\" \")}",
                        onchange: move |e| { let v: Vec<String> = e.value().split_whitespace().map(|s| s.to_string()).collect(); app.update_config(|c| c.agent_mut(&k2).extra_args = v); },
                    }
                }
            }
            // Companion mode brings each harness's sessions in.
            if spec.can_import && app.companion() {
                div { class: "ad-row",
                    span { class: "k", "Import sessions" }
                    div { class: "v",
                        Toggle { on: ac.import, onchange: move |v| app.update_config(|c| c.agent_mut(&k3).import = v) }
                        div { class: "h", "From " span { class: "mono", "{spec.session_location}" } }
                    }
                }
            }
        }
    }
}
