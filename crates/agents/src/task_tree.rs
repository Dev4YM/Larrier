//! A background command the user stopped stops for good.
//!
//! Claude Code stops one by ending its shell, and on Windows what that shell
//! started can outlive it (an emulator, a server, a long `ping`): a program
//! isn't ended with the shell that started it, and when Claude Code itself
//! ends right after, nothing is left to end it. So when the user stops a
//! command, its shell is found among Claude Code's (by the command it runs)
//! and everything under it is noted; whatever of that still runs a moment
//! later is ended. Only those processes, each checked by when it started
//! (an id used again since is someone else's).

use std::time::Duration;

/// One process: its id, its parent's, when it started, its command line.
#[derive(Clone, Debug, PartialEq)]
pub struct Proc {
    pub pid: u32,
    pub parent: u32,
    pub born: String,
    pub cmd: String,
}

/// The tree of Claude Code's (`agent_pid`) shell running `command`, as it is
/// now: noted before Claude Code is asked to stop it (once the shell has
/// ended, what it started can't be told apart). Takes about a second.
pub fn note(agent_pid: u32, command: &str) -> Option<Vec<Proc>> {
    if !cfg!(windows) {
        return None;
    }
    let tree = tree_of(&list()?, agent_pid, command);
    if tree.is_none() {
        tracing::debug!("task stop: no single shell runs {command:?}");
    }
    tree
}

/// End what of `tree` still runs after `grace`, on a thread of its own.
pub fn end_later(tree: Vec<Proc>, grace: Duration) {
    let _ = std::thread::Builder::new().name("lary-task-stop".into()).spawn(move || {
        std::thread::sleep(grace);
        let Some(now) = list() else { return };
        let left = still_running(&tree, &now);
        if left.is_empty() {
            return;
        }
        tracing::info!("task stop: ending {} process(es) the stopped command left running", left.len());
        let mut kill = std::process::Command::new("taskkill");
        kill.arg("/F");
        for pid in &left {
            kill.args(["/PID", &pid.to_string()]);
        }
        no_window(&mut kill);
        let _ = kill.output();
    });
}

/// Of `tree`, the processes still running in `now` (same id, same start).
pub fn still_running(tree: &[Proc], now: &[Proc]) -> Vec<u32> {
    tree.iter().filter(|t| now.iter().any(|p| p.pid == t.pid && p.born == t.born)).map(|t| t.pid).collect()
}

/// The shell of `agent`'s that runs `command`, and everything under it.
/// None when none does, or more than one (which one isn't sure).
pub fn tree_of(procs: &[Proc], agent: u32, command: &str) -> Option<Vec<Proc>> {
    let want = letters(command);
    if want.len() < 3 {
        return None;
    }
    let mut shells = procs.iter().filter(|p| p.parent == agent && p.pid != agent && letters(&p.cmd).contains(&want));
    let shell = match (shells.next(), shells.next()) {
        (Some(s), None) => s.clone(),
        _ => return None,
    };
    let mut out = vec![shell];
    let mut i = 0;
    while i < out.len() {
        let pid = out[i].pid;
        let kids: Vec<Proc> = procs.iter().filter(|p| p.parent == pid && p.pid != pid && !out.iter().any(|o| o.pid == p.pid)).cloned().collect();
        out.extend(kids);
        i += 1;
    }
    Some(out)
}

/// A command line's letters and digits alone: the shell quotes and escapes
/// the command it runs, so it's matched without them.
fn letters(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_lowercase()).collect()
}

/// Every process now (Windows).
fn list() -> Option<Vec<Proc>> {
    let mut cmd = std::process::Command::new("powershell");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "Get-CimInstance Win32_Process | ForEach-Object { $b = 0; if ($_.CreationDate) { $b = $_.CreationDate.ToFileTimeUtc() }; ($_.ProcessId, $_.ParentProcessId, $b, ($_.CommandLine -replace '[\\r\\n\\t]', ' ')) -join [char]9 }",
    ]);
    no_window(&mut cmd);
    let out = cmd.output().ok()?;
    Some(parse(&String::from_utf8_lossy(&out.stdout)))
}

pub fn parse(text: &str) -> Vec<Proc> {
    text.lines()
        .filter_map(|l| {
            let mut f = l.splitn(4, '\t');
            Some(Proc { pid: f.next()?.trim().parse().ok()?, parent: f.next()?.trim().parse().ok()?, born: f.next()?.trim().to_string(), cmd: f.next().unwrap_or("").to_string() })
        })
        .collect()
}

#[cfg(windows)]
fn no_window(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000);
}

#[cfg(not(windows))]
fn no_window(_: &mut std::process::Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real shapes (2026-11-20): Claude Code's shell runs the command in
    /// `eval '…'`, quoted and escaped, under two more bash layers.
    #[test]
    fn finds_the_stopped_commands_tree() {
        let procs = parse(concat!(
            "100\t1\t5\tclaude.exe -p\n",
            "201\t100\t6\t\"C:\\Program Files\\Git\\bin\\bash.exe\" -c \"source snap.sh && eval 'ping -n 900 127.0.0.1' < /dev/null && pwd -P\"\n",
            "202\t201\t6\tbash.exe -c \"eval 'ping -n 900 127.0.0.1'\"\n",
            "203\t202\t7\tC:\\WINDOWS\\system32\\PING.EXE -n 900 127.0.0.1\n",
            "301\t100\t8\t\"C:\\Program Files\\Git\\bin\\bash.exe\" -c \"eval 'npm run dev'\"\n",
            "302\t301\t8\tnode server.js\n",
        ));
        let tree = tree_of(&procs, 100, "ping -n 900 127.0.0.1").unwrap();
        assert_eq!(tree.iter().map(|p| p.pid).collect::<Vec<_>>(), [201, 202, 203]);
        // Escaped quotes in the shell's line still match.
        let ps = parse("401\t100\t9\tbash.exe -c \"eval 'powershell -NoProfile -Command \\\"Start-Process ping -ArgumentList '\\\"'\\\"'-n'\\\"'\\\"'\\\"'\"\n");
        assert!(tree_of(&ps, 100, "powershell -NoProfile -Command \"Start-Process ping -ArgumentList '-n'\"").is_some());
        // Two shells run it: which one is meant isn't sure, so none.
        let twice = [procs.clone(), parse("501\t100\t9\tbash.exe -c \"eval 'ping -n 900 127.0.0.1'\"\n")].concat();
        assert!(tree_of(&twice, 100, "ping -n 900 127.0.0.1").is_none());
        // Ended since, or its id now someone else's: left alone.
        let now = parse("201\t1\t6\tbash\n203\t1\t99\tother\n");
        assert_eq!(still_running(&tree, &now), [201]);
    }
}
