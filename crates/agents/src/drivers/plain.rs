//! Plain-text agents: stdout is the answer. Used for Aider
//! (`aider --message … --yes-always --no-stream`) and custom commands.

use larrier_core::AgentKind;
use tokio::sync::mpsc::UnboundedReceiver;

use super::process::strip_ansi;
use super::{run_stream, LineParser};
use crate::events::{EventSink, TurnCommand};
use crate::request::TurnRequest;

pub async fn run(req: TurnRequest, sink: EventSink, mut control: UnboundedReceiver<TurnCommand>) {
    let mut cmd = req.launch.command();
    cmd.current_dir(&req.cwd);
    let name = req.agent.display_name();
    if req.agent == AgentKind::Aider {
        cmd.args(["--yes-always", "--no-stream", "--no-pretty", "--no-fancy-input", "--no-check-update", "--no-show-release-notes"]);
        if let Some(m) = &req.model {
            cmd.args(["--model", m]);
        }
        match req.mode.as_deref() {
            Some("architect") => {
                cmd.args(["--chat-mode", "architect"]);
            }
            Some("ask") => {
                cmd.args(["--chat-mode", "ask"]);
            }
            _ => {}
        }
        cmd.args(&req.launch.extra_args);
        // Attached files join the chat as editable files.
        for f in req.file_paths() {
            cmd.arg(f);
        }
        let prompt = req.full_prompt();
        let file = if prompt.chars().count() > 8_000 || prompt.contains('\n') { req.write_prompt_file(&prompt) } else { None };
        match file {
            Some(p) => {
                cmd.arg("--message-file").arg(p);
            }
            None => {
                cmd.args(["--message", &prompt]);
            }
        }
    } else {
        cmd.args(&req.launch.extra_args);
        cmd.arg(req.prompt_for_arg(24_000));
    }
    cmd.env("NO_COLOR", "1");
    let mut parser = PlainText::default();
    run_stream(&name, cmd, None, &sink, &mut control, &mut parser).await;
}

/// Every stdout line becomes assistant text.
#[derive(Default)]
pub struct PlainText {
    started: bool,
}

impl LineParser for PlainText {
    fn handle(&mut self, line: &str, sink: &EventSink) {
        let clean = strip_ansi(line);
        if !self.started && clean.trim().is_empty() {
            return;
        }
        self.started = true;
        sink.text(format!("{clean}\n"));
    }

    fn done(&self) -> bool {
        false
    }
}
