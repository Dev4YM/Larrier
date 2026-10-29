//! `larrier` — command line access to the Larrier session store.
//!
//! ```text
//! larrier agents                      list agents and whether they're installed
//! larrier sessions [--agent X]        list sessions in the store
//! larrier discover [--agent X]        list native sessions found on this machine
//! larrier sync                        import new/changed native sessions
//! larrier import <file>...            import LSF / native files / chat exports
//! larrier export <id> --format F      print a session in another format
//! larrier port <id> --to <agent>      write a native resumable session for an agent
//! larrier show <id>                   print a session as Markdown
//! larrier run --agent X [--cwd D] <prompt>   run one turn and record it
//! ```

use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use larrier_agents::{build_request, engine::TurnRecorder, start_turn, AgentEvent};
use larrier_core::{
    store::{import_key, ImportRecord},
    AgentKind, Message, Session, Store, WorkspaceRef,
};
use larrier_formats::ExportFormat;

#[derive(Parser)]
#[command(name = "larrier", version, about = "Manage and port agent sessions with the Larrier Session Format")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List known agents and detection status.
    Agents,
    /// List sessions stored in Larrier.
    Sessions {
        #[arg(long)]
        agent: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// List native sessions found in agents' own storage.
    Discover {
        #[arg(long)]
        agent: Option<String>,
    },
    /// Import new or changed native sessions from every agent.
    Sync,
    /// Import files (LSF, Claude/Codex/Pi jsonl, Gemini json, ChatGPT/Claude.ai exports).
    Import { files: Vec<PathBuf> },
    /// Export a session (by id or id prefix).
    Export {
        id: String,
        #[arg(long, default_value = "lsf-jsonl")]
        format: String,
        #[arg(long, short)]
        out: Option<PathBuf>,
    },
    /// Write a native, resumable session file for another agent.
    Port {
        id: String,
        #[arg(long)]
        to: String,
    },
    /// Print a session as Markdown.
    Show { id: String },
    /// Run one turn with an agent and record it as a session.
    Run {
        #[arg(long, default_value = "claude-code")]
        agent: String,
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(long)]
        model: Option<String>,
        /// Continue an existing session instead of starting a new one.
        #[arg(long)]
        session: Option<String>,
        prompt: Vec<String>,
    },
}

fn parse_format(s: &str) -> Result<ExportFormat> {
    Ok(match s {
        "lsf" | "lsf-jsonl" | "jsonl" => ExportFormat::LsfJsonl,
        "lsf-json" | "json" => ExportFormat::LsfJson,
        "md" | "markdown" => ExportFormat::Markdown,
        "claude" | "claude-code" => ExportFormat::ClaudeCode,
        "codex" => ExportFormat::Codex,
        "gemini" => ExportFormat::Gemini,
        "pi" => ExportFormat::Pi,
        other => return Err(anyhow!("unknown format '{other}' (lsf-jsonl, lsf-json, markdown, claude-code, codex, gemini, pi)")),
    })
}

fn resolve_id(store: &Store, id: &str) -> Result<String> {
    if store.exists(id) {
        return Ok(id.to_string());
    }
    let matches: Vec<_> = store.list().into_iter().filter(|s| s.id.starts_with(id) || s.id.ends_with(id)).collect();
    match matches.len() {
        0 => Err(anyhow!("no session matches '{id}'")),
        1 => Ok(matches[0].id.clone()),
        n => Err(anyhow!("'{id}' matches {n} sessions; use more characters")),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let store = Store::open_default()?;
    match cli.cmd {
        Cmd::Agents => {
            let cfg = store.load_config();
            for d in larrier_agents::detect_all(&cfg).await {
                let spec = d.kind.spec();
                match &d.path {
                    Some(p) => println!("✓ {:<14} {:<10} {}", spec.name, d.version.clone().unwrap_or_default(), p.display()),
                    None => println!("· {:<14} {:<10} {}", spec.name, "missing", spec.install.first().map(|i| i.1).unwrap_or("")),
                }
            }
        }
        Cmd::Sessions { agent, limit } => {
            let filter = agent.map(|a| AgentKind::from_id(&a));
            for s in store.list().into_iter().filter(|s| filter.as_ref().map(|f| &s.agent == f || &s.origin == f).unwrap_or(true)).take(limit) {
                println!(
                    "{}  {:<12} {:<16} {}",
                    &s.id[s.id.len() - 12..],
                    s.updated_at.format("%Y-%m-%d %H:%M"),
                    s.agent.display_name(),
                    s.title
                );
            }
        }
        Cmd::Discover { agent } => {
            let list = match agent {
                Some(a) => larrier_formats::discover(&AgentKind::from_id(&a)),
                None => larrier_formats::discover_all(),
            };
            for n in &list {
                println!(
                    "{:<12} {:<38} {}  {}",
                    n.agent.id(),
                    n.native_id,
                    n.updated_at.map(|t| t.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default(),
                    n.title.clone().unwrap_or_default()
                );
            }
            eprintln!("{} native sessions", list.len());
        }
        Cmd::Sync => {
            let r = larrier_formats::sync(&store, &larrier_formats::importable_agents());
            println!("imported {}, updated {}, unchanged {}", r.imported, r.updated, r.skipped);
            for e in r.errors.iter().take(20) {
                eprintln!("  ! {e}");
            }
        }
        Cmd::Import { files } => {
            let mut n = 0;
            for f in files {
                let list = larrier_formats::import_file(&f).with_context(|| format!("importing {}", f.display()))?;
                n += list.len();
                store.save_many(&list)?;
            }
            println!("imported {n} sessions");
        }
        Cmd::Export { id, format, out } => {
            let s = store.load(&resolve_id(&store, &id)?)?;
            let text = larrier_formats::export_string(&s, parse_format(&format)?)?;
            match out {
                Some(p) => std::fs::write(&p, text)?,
                None => print!("{text}"),
            }
        }
        Cmd::Port { id, to } => {
            let id = resolve_id(&store, &id)?;
            let mut s = store.load(&id)?;
            let agent = AgentKind::from_id(&to);
            let link = larrier_formats::export_native(&s, &agent, None)?;
            store.set_import_record(&import_key(&agent, &link.id), ImportRecord::linked(&s.id));
            store.flush_imports();
            println!("wrote {} session {}", agent.display_name(), link.id);
            if let Some(p) = &link.path {
                println!("  {p}");
            }
            s.set_native_link(link);
            s.agent = agent;
            store.save(&s)?;
        }
        Cmd::Show { id } => {
            let s = store.load(&resolve_id(&store, &id)?)?;
            print!("{}", s.to_markdown());
        }
        Cmd::Run { agent, cwd, model, session, prompt } => {
            let cfg = store.load_config();
            let agent = AgentKind::from_id(&agent);
            let prompt = prompt.join(" ");
            if prompt.trim().is_empty() {
                return Err(anyhow!("empty prompt"));
            }
            let mut s = match session {
                Some(id) => store.load(&resolve_id(&store, &id)?)?,
                None => {
                    let dir = cwd.unwrap_or(std::env::current_dir()?);
                    Session::new(agent.clone(), Some(WorkspaceRef::from_path(dir.to_string_lossy().to_string())))
                }
            };
            s.agent = agent.clone();
            if model.is_some() {
                s.model = model;
            }
            s.push(Message::user_text(prompt));
            s.auto_title();
            let req = build_request(&cfg, &s, &agent)?;
            let (m, acc) = (s.model.clone(), s.account_id.clone());
            let mut rec = TurnRecorder::begin(&mut s, agent.clone(), m, acc);
            let mut handle = start_turn(req);
            while let Some(ev) = handle.events.recv().await {
                match &ev {
                    AgentEvent::TextDelta { text } | AgentEvent::Text { text } => print!("{text}"),
                    AgentEvent::ToolStart { tool } => eprintln!("\n  ▸ {}", tool.title.clone().unwrap_or_else(|| tool.name.clone())),
                    AgentEvent::Error { message } => eprintln!("\n  ! {message}"),
                    AgentEvent::NativeSession { id, .. } => {
                        store.set_import_record(&import_key(&agent, id), ImportRecord::linked(&s.id));
                    }
                    _ => {}
                }
                let done = matches!(ev, AgentEvent::Done { .. });
                rec.apply(&mut s, &ev);
                if done {
                    break;
                }
            }
            rec.finish(&mut s, rec.ok || rec.finished);
            store.flush_imports();
            store.save(&s)?;
            println!("\n\nsaved session {} ({})", &s.id, s.status.label());
        }
    }
    Ok(())
}
