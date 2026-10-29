//! Importers and exporters between LSF and every agent's native session
//! storage, plus chat exports (ChatGPT, claude.ai) and Markdown.
//!
//! * [`discover`] lists native sessions on this machine.
//! * [`import_native`] / [`import_file`] convert them into LSF sessions.
//! * [`export_native`] writes an LSF session into an agent's own store so the
//!   agent can resume it natively (Claude Code, Codex, Gemini CLI, Pi).
//! * [`export_string`] renders a session in a portable file format.
//! * [`sync`] incrementally imports everything new into the Larrier store.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use larrier_core::{AgentKind, NativeLink, Session, Store};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod chats;
pub mod claude;
pub mod codex;
pub mod cursor;
pub mod droid;
pub mod gemini;
pub mod hermes;
pub mod markdown;
pub mod opencode;
pub mod pi;
mod sync;
pub mod util;

pub use util::ImportSkip;

/// A session found in an agent's native storage.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NativeSessionInfo {
    pub agent: AgentKind,
    pub native_id: String,
    pub path: PathBuf,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub size: u64,
    /// Source modification time in unix millis (import fingerprint).
    #[serde(default)]
    pub mtime_ms: i64,
}

/// File formats a session can be exported to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    LsfJsonl,
    LsfJson,
    Markdown,
    ClaudeCode,
    Codex,
    Gemini,
    Pi,
}

impl ExportFormat {
    pub const ALL: &'static [ExportFormat] = &[
        ExportFormat::LsfJsonl,
        ExportFormat::LsfJson,
        ExportFormat::Markdown,
        ExportFormat::ClaudeCode,
        ExportFormat::Codex,
        ExportFormat::Gemini,
        ExportFormat::Pi,
    ];
    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::LsfJsonl => "Larrier Session (.lsf.jsonl)",
            ExportFormat::LsfJson => "Larrier Session (.lsf.json)",
            ExportFormat::Markdown => "Markdown (.md)",
            ExportFormat::ClaudeCode => "Claude Code session (.jsonl)",
            ExportFormat::Codex => "Codex rollout (.jsonl)",
            ExportFormat::Gemini => "Gemini CLI chat (.jsonl)",
            ExportFormat::Pi => "Pi session (.jsonl)",
        }
    }
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::LsfJsonl => "lsf.jsonl",
            ExportFormat::LsfJson => "lsf.json",
            ExportFormat::Markdown => "md",
            ExportFormat::ClaudeCode | ExportFormat::Codex | ExportFormat::Pi | ExportFormat::Gemini => "jsonl",
        }
    }
}

/// Result of a sync pass.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SyncReport {
    pub imported: usize,
    pub updated: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

/// Agents whose native sessions this crate can read.
pub fn importable_agents() -> Vec<AgentKind> {
    larrier_core::agent::catalog().iter().filter(|s| s.can_import).map(|s| s.kind.clone()).collect()
}

/// Agents this crate can write native, resumable session files for.
pub fn supports_native_export(agent: &AgentKind) -> bool {
    matches!(agent, AgentKind::ClaudeCode | AgentKind::Codex | AgentKind::Gemini | AgentKind::Pi)
}

/// List native sessions for one agent.
pub fn discover(agent: &AgentKind) -> Vec<NativeSessionInfo> {
    match agent {
        AgentKind::ClaudeCode => claude::discover(),
        AgentKind::Codex => codex::discover(),
        AgentKind::Cursor => cursor::discover(),
        AgentKind::OpenCode => opencode::discover(),
        AgentKind::Gemini => gemini::discover(),
        AgentKind::Pi => pi::discover(),
        AgentKind::Hermes => hermes::discover(),
        AgentKind::Droid => droid::discover(),
        _ => Vec::new(),
    }
}

/// List native sessions for every importable agent.
pub fn discover_all() -> Vec<NativeSessionInfo> {
    let mut agents = importable_agents();
    if !agents.contains(&AgentKind::Droid) {
        agents.push(AgentKind::Droid);
    }
    agents.iter().flat_map(discover).collect()
}

/// Convert one native session into LSF.
pub fn import_native(info: &NativeSessionInfo) -> Result<Session> {
    match info.agent {
        AgentKind::ClaudeCode => claude::import(info),
        AgentKind::Codex => codex::import(info),
        AgentKind::Cursor => cursor::import(info),
        AgentKind::OpenCode => opencode::import(info),
        AgentKind::Gemini => gemini::import(info),
        AgentKind::Pi => pi::import(info),
        AgentKind::Hermes => hermes::import(info),
        AgentKind::Droid => droid::import(info),
        _ => Err(anyhow!("importing {} sessions is not supported yet", info.agent.display_name())),
    }
}

/// Import any supported file (LSF, native session files, chat exports).
/// Chat exports can contain many conversations.
pub fn import_file(path: &Path) -> Result<Vec<Session>> {
    let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    if name.ends_with(".zip") {
        return Err(anyhow!("unzip the export first, then import its conversations.json"));
    }
    let text = util::read_text(path)?;
    let stem = name.trim_end_matches(".zst").to_string();
    if stem.ends_with(".lsf.jsonl") || stem.ends_with(".lsf.json") {
        return Ok(vec![Session::from_any(&text)?]);
    }
    if stem.ends_with(".md") || stem.ends_with(".markdown") {
        return markdown::parse(&text, &name);
    }
    let trimmed = text.trim_start();

    // Whole-document JSON (chat exports, LSF documents, legacy Gemini).
    if stem.ends_with(".json") || trimmed.starts_with('[') {
        if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
            if chats::is_chatgpt(&v) {
                return collect(chats::parse_chatgpt(&v));
            }
            if chats::is_claude_ai(&v) {
                return collect(chats::parse_claude_ai(&v));
            }
            if v.get("lsf").is_some() || (v.get("id").is_some() && v.get("origin").is_some()) {
                return Ok(vec![Session::from_any(&text)?]);
            }
            if v.get("sessionId").is_some() && v.get("messages").is_some() {
                return Ok(vec![gemini::parse(&text, Some(path))?]);
            }
            if let Some(list) = v.as_array().filter(|a| a.iter().all(|x| x.get("role").is_some())) {
                // OpenAI-style message array.
                let mut conv = util::Conv::new(AgentKind::Chat);
                for m in list {
                    let t = util::content_text(&m["content"]);
                    match m["role"].as_str() {
                        Some("user") => conv.user(t, None),
                        Some("assistant") => conv.text(&t, None, None),
                        _ => {}
                    }
                }
                let meta = util::Meta { native_id: name.clone(), ..Default::default() };
                return Ok(vec![util::finish_as(conv, AgentKind::Chat, AgentKind::Chat, meta)?]);
            }
        }
    }

    // JSON Lines: sniff the first records.
    let head: Vec<Value> = text.lines().filter(|l| !l.trim().is_empty()).take(12).filter_map(|l| serde_json::from_str(l).ok()).collect();
    let first = head.first().cloned().unwrap_or(Value::Null);
    let ty = |v: &Value| v.get("type").and_then(|t| t.as_str()).map(String::from);
    let one = |r: Result<Session>| r.map(|s| vec![s]);
    let fallback_id = path.file_stem().map(|n| n.to_string_lossy().trim_end_matches(".jsonl").to_string()).unwrap_or_default();
    if first.get("lsf").is_some() || ty(&first).as_deref() == Some("session") && first.get("lsf").is_some() {
        return Ok(vec![Session::from_any(&text)?]);
    }
    match ty(&first).as_deref() {
        Some("session_meta") => return one(codex::parse(&text, Some(path))),
        Some("session") if first.get("version").is_some() => return one(pi::parse(&text, Some(path))),
        Some("session_start") => return one(droid::parse(&text, Some(path))),
        _ => {}
    }
    if first.get("sessionId").is_some() && first.get("projectHash").is_some() {
        return one(gemini::parse(&text, Some(path)));
    }
    if head.iter().any(|v| v.get("sessionId").is_some() && matches!(ty(v).as_deref(), Some("user") | Some("assistant") | Some("summary") | Some("system"))) {
        return one(claude::parse(&text, Some(path), &fallback_id));
    }
    if head.iter().any(|v| v.get("role").is_some() && v.get("message").is_some()) {
        let (_, mtime) = util::file_info(path);
        return one(cursor::parse_transcript(&text, &fallback_id, Some(path), util::ts_from_ms(mtime)));
    }
    Err(anyhow!("unrecognized session format: {}", path.display()))
}

fn collect(list: Vec<Result<Session>>) -> Result<Vec<Session>> {
    let total = list.len();
    let ok: Vec<Session> = list.into_iter().filter_map(|r| r.ok()).collect();
    if ok.is_empty() && total > 0 {
        return Err(anyhow!("no conversations with messages in this export"));
    }
    Ok(ok)
}

/// Render a session to a portable file format.
pub fn export_string(session: &Session, format: ExportFormat) -> Result<String> {
    let now = Utc::now();
    match format {
        ExportFormat::LsfJsonl => Ok(session.to_jsonl()),
        ExportFormat::LsfJson => Ok(session.to_json_pretty()),
        ExportFormat::Markdown => Ok(session.to_markdown()),
        ExportFormat::ClaudeCode => Ok(claude::export_lines(session, &uuid::Uuid::new_v4().to_string())),
        ExportFormat::Codex => Ok(codex::export_lines(session, &uuid::Uuid::now_v7().to_string(), now)),
        ExportFormat::Gemini => Ok(gemini::export_lines(session, &uuid::Uuid::new_v4().to_string(), &util::session_cwd(session), now)),
        ExportFormat::Pi => Ok(pi::export_lines(session, &uuid::Uuid::now_v7().to_string(), &util::session_cwd(session))),
    }
}

/// Write the session into the agent's own storage so it can resume it.
/// `home` overrides the agent's config dir (multi-account setups).
pub fn export_native(session: &Session, agent: &AgentKind, home: Option<&Path>) -> Result<NativeLink> {
    match agent {
        AgentKind::ClaudeCode => claude::export_native(session, home),
        AgentKind::Codex => codex::export_native(session, home),
        AgentKind::Gemini => gemini::export_native(session, home),
        AgentKind::Pi => pi::export_native(session, home),
        other => Err(anyhow!("{} cannot load external sessions natively", other.display_name())),
    }
}

/// Incrementally import new/changed native sessions into the store.
pub fn sync(store: &Store, agents: &[AgentKind]) -> SyncReport {
    sync::sync(store, agents)
}

#[cfg(test)]
mod tests {
    use super::*;
    use larrier_core::{Message, WorkspaceRef};

    fn sample() -> Session {
        let mut s = Session::new(AgentKind::ClaudeCode, Some(WorkspaceRef::from_path(std::env::temp_dir().to_string_lossy().to_string())));
        s.title = "Port me".into();
        s.push(Message::user_text("refactor the parser"));
        let mut a = Message::assistant(AgentKind::ClaudeCode, Some("claude-sonnet-4-5".into()));
        a.push_text("Refactored.");
        let mut t = larrier_core::ToolCall::new("t1", "Bash", serde_json::json!({"command": "cargo test"}));
        t.output = Some("ok".into());
        a.parts.push(larrier_core::Part::Tool(t));
        s.push(a);
        s.push(Message::user_text("thanks"));
        s
    }

    #[test]
    fn export_native_then_import_file() {
        let s = sample();
        for agent in [AgentKind::ClaudeCode, AgentKind::Codex, AgentKind::Gemini, AgentKind::Pi] {
            let home = tempfile::tempdir().unwrap();
            let link = export_native(&s, &agent, Some(home.path())).unwrap();
            let path = PathBuf::from(link.path.clone().unwrap());
            assert!(path.starts_with(home.path()), "{agent:?} wrote outside home: {}", path.display());
            let back = import_file(&path).unwrap().remove(0);
            assert_eq!(back.origin.agent, agent);
            assert_eq!(back.messages.len(), 3, "{agent:?}");
            assert_eq!(back.messages[0].text(), "refactor the parser");
            assert!(back.messages[1].text().contains("Refactored."));
            assert_eq!(back.messages[2].text(), "thanks");
            assert_eq!(back.origin.native_id.as_deref(), Some(link.id.as_str()), "{agent:?}");
        }
    }

    #[test]
    fn import_file_detects_formats() {
        let dir = tempfile::tempdir().unwrap();
        let s = sample();
        let p = dir.path().join("a.lsf.jsonl");
        std::fs::write(&p, s.to_jsonl()).unwrap();
        assert_eq!(import_file(&p).unwrap()[0].id, s.id);
        let p = dir.path().join("b.json");
        std::fs::write(&p, s.to_json_pretty()).unwrap();
        assert_eq!(import_file(&p).unwrap()[0].id, s.id);
        let p = dir.path().join("c.md");
        std::fs::write(&p, s.to_markdown()).unwrap();
        assert_eq!(import_file(&p).unwrap()[0].messages.len(), 3);
        for f in [ExportFormat::ClaudeCode, ExportFormat::Codex, ExportFormat::Gemini, ExportFormat::Pi] {
            let p = dir.path().join(format!("x-{f:?}.{}", f.extension()));
            std::fs::write(&p, export_string(&s, f).unwrap()).unwrap();
            let back = import_file(&p).unwrap();
            assert_eq!(back[0].messages.len(), 3, "{f:?}");
        }
        let p = dir.path().join("conversations.json");
        std::fs::write(&p, r#"[{"uuid":"z","name":"Hi","chat_messages":[{"uuid":"1","sender":"human","text":"hey"},{"uuid":"2","sender":"assistant","text":"hello"}]}]"#).unwrap();
        let back = import_file(&p).unwrap();
        assert_eq!(back[0].origin.agent, AgentKind::ClaudeAi);
    }
}
