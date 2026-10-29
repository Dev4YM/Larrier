//! Shared plumbing for agents driven as a child process that prints one JSON
//! event per line.

use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::events::{EventSink, TurnCommand};

pub struct ProcessOutcome {
    pub exit_code: Option<i32>,
    pub cancelled: bool,
    /// Last lines of stderr, for error reporting.
    pub stderr_tail: Vec<String>,
}

impl ProcessOutcome {
    pub fn success(&self) -> bool {
        !self.cancelled && self.exit_code == Some(0)
    }

    pub fn error_message(&self, agent: &str) -> String {
        let tail = self.stderr_tail.iter().rev().take(6).rev().cloned().collect::<Vec<_>>().join("\n");
        if self.cancelled {
            "Stopped".to_string()
        } else if let Some(reason) = stderr_reason(&self.stderr_tail) {
            format!("{agent}: {reason}")
        } else if tail.trim().is_empty() {
            format!("{agent} exited with code {}", self.exit_code.map(|c| c.to_string()).unwrap_or_else(|| "?".into()))
        } else {
            format!("{agent} exited with code {}:\n{tail}", self.exit_code.map(|c| c.to_string()).unwrap_or_else(|| "?".into()))
        }
    }
}

/// The reason a CLI gives on stderr for failing, without the wrapper:
/// "ActionRequiredError: You've hit your usage limit …" followed by
/// "Error: command failed unexpectedly." reads "You've hit your usage
/// limit …". None when no line names a specific error.
fn stderr_reason(lines: &[String]) -> Option<String> {
    lines.iter().map(|l| l.trim()).filter(|l| !l.is_empty() && !l.starts_with("at ")).find_map(|l| {
        let (name, msg) = l.split_once(": ")?;
        let specific = name.len() > "Error".len() && name.ends_with("Error") && name.chars().all(|c| c.is_ascii_alphanumeric());
        (specific && !msg.trim().is_empty()).then(|| msg.trim().to_string())
    })
}

/// Spawn `cmd`, optionally feed `stdin_data`, and call `on_line` for every
/// stdout line until the process exits or the turn is cancelled.
pub async fn run_lines<F>(
    mut cmd: Command,
    stdin_data: Option<String>,
    sink: &EventSink,
    control: &mut UnboundedReceiver<TurnCommand>,
    mut on_line: F,
) -> ProcessOutcome
where
    F: FnMut(&str),
{
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd.stdin(if stdin_data.is_some() { Stdio::piped() } else { Stdio::null() });
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            sink.error(format!("Failed to start agent: {e}"));
            return ProcessOutcome { exit_code: None, cancelled: false, stderr_tail: vec![e.to_string()] };
        }
    };
    if let Some(data) = stdin_data {
        if let Some(mut stdin) = child.stdin.take() {
            tokio::spawn(async move {
                let _ = stdin.write_all(data.as_bytes()).await;
                let _ = stdin.shutdown().await;
            });
        }
    }
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");
    let mut out_lines = BufReader::new(stdout).lines();
    let mut err_lines = BufReader::new(stderr).lines();
    let mut stderr_tail: Vec<String> = Vec::new();
    let mut cancelled = false;
    let mut out_done = false;
    let mut err_done = false;

    while !(out_done && err_done) {
        tokio::select! {
            line = out_lines.next_line(), if !out_done => match line {
                Ok(Some(l)) => {
                    let l = l.trim_end_matches('\r');
                    if !l.trim().is_empty() {
                        on_line(l);
                    }
                }
                _ => out_done = true,
            },
            line = err_lines.next_line(), if !err_done => match line {
                Ok(Some(l)) => {
                    sink.log(l.clone());
                    stderr_tail.push(strip_ansi(&l));
                    if stderr_tail.len() > 40 { stderr_tail.remove(0); }
                }
                _ => err_done = true,
            },
            cmd = control.recv() => match cmd {
                Some(TurnCommand::Cancel) | None => {
                    cancelled = true;
                    let _ = child.start_kill();
                    break;
                }
                Some(TurnCommand::Permission { .. }) => {}
            },
        }
    }
    let status = child.wait().await.ok();
    ProcessOutcome { exit_code: status.and_then(|s| s.code()), cancelled, stderr_tail }
}

pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for n in chars.by_ref() {
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod reason_tests {
    use super::*;

    #[test]
    fn names_the_specific_error() {
        let out = |lines: &[&str]| ProcessOutcome { exit_code: Some(1), cancelled: false, stderr_tail: lines.iter().map(|l| l.to_string()).collect() };
        let limit = out(&["ActionRequiredError: You've hit your usage limit for Grok 4.7.", "    at run (index.js:1:1)", "Error: command failed unexpectedly."]);
        assert_eq!(limit.error_message("Cursor Agent"), "Cursor Agent: You've hit your usage limit for Grok 4.7.");
        let plain = out(&["Error: command failed unexpectedly."]);
        assert_eq!(plain.error_message("Cursor Agent"), "Cursor Agent exited with code 1:\nError: command failed unexpectedly.");
    }
}
