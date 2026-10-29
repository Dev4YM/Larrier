//! Git operations for the Changes panel, worktrees and pull requests.
//! Uses the `git` and `gh` executables so behaviour matches the user's setup.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

fn git(cwd: &Path) -> Command {
    let mut c = Command::new("git");
    c.current_dir(cwd);
    c.env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    c
}

fn run(mut c: Command) -> Result<String> {
    let out = c.output().map_err(|e| anyhow!("failed to run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let msg = if err.is_empty() { String::from_utf8_lossy(&out.stdout).trim().to_string() } else { err };
        Err(anyhow!(msg))
    }
}

pub fn is_repo(path: &Path) -> bool {
    let mut c = git(path);
    c.args(["rev-parse", "--is-inside-work-tree"]);
    run(c).map(|s| s.trim() == "true").unwrap_or(false)
}

pub fn repo_root(path: &Path) -> Option<PathBuf> {
    let mut c = git(path);
    c.args(["rev-parse", "--show-toplevel"]);
    run(c).ok().map(|s| PathBuf::from(s.trim()))
}

pub fn current_branch(path: &Path) -> Option<String> {
    let mut c = git(path);
    c.args(["rev-parse", "--abbrev-ref", "HEAD"]);
    run(c).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

pub fn default_branch(path: &Path) -> String {
    let mut c = git(path);
    c.args(["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]);
    run(c)
        .ok()
        .map(|s| s.trim().trim_start_matches("origin/").to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "main".to_string())
}

pub fn remote_url(path: &Path) -> Option<String> {
    let mut c = git(path);
    c.args(["remote", "get-url", "origin"]);
    run(c).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileStatus {
    pub path: String,
    /// Porcelain code: `M`, `A`, `D`, `R`, `?` (untracked), `U` (conflict).
    pub code: char,
    pub staged: bool,
    pub added: u32,
    pub removed: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RepoStatus {
    pub branch: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub files: Vec<FileStatus>,
}

impl RepoStatus {
    pub fn totals(&self) -> (u32, u32) {
        self.files.iter().fold((0, 0), |(a, r), f| (a + f.added, r + f.removed))
    }
}

pub fn status(path: &Path) -> Result<RepoStatus> {
    let mut c = git(path);
    c.args(["status", "--porcelain=v1", "-b", "-z", "--untracked-files=all"]);
    let out = run(c)?;
    let mut st = RepoStatus::default();
    let mut entries = out.split('\0').peekable();
    while let Some(e) = entries.next() {
        if e.is_empty() {
            continue;
        }
        if let Some(b) = e.strip_prefix("## ") {
            let name = b.split("...").next().unwrap_or(b).trim().to_string();
            st.branch = Some(name.replace("No commits yet on ", ""));
            if let Some(i) = b.find('[') {
                let info = &b[i + 1..b.len().saturating_sub(1)];
                for part in info.split(',') {
                    let part = part.trim();
                    if let Some(n) = part.strip_prefix("ahead ") {
                        st.ahead = n.parse().unwrap_or(0);
                    } else if let Some(n) = part.strip_prefix("behind ") {
                        st.behind = n.parse().unwrap_or(0);
                    }
                }
            }
            continue;
        }
        if e.len() < 4 {
            continue;
        }
        let x = e.as_bytes()[0] as char;
        let y = e.as_bytes()[1] as char;
        let file = e[3..].to_string();
        if x == 'R' || x == 'C' {
            entries.next(); // original path follows
        }
        let code = if x == '?' {
            '?'
        } else if x == 'U' || y == 'U' {
            'U'
        } else if y != ' ' {
            y
        } else {
            x
        };
        st.files.push(FileStatus { path: file, code, staged: x != ' ' && x != '?', added: 0, removed: 0 });
    }
    // Line counts against HEAD (tracked) and full length for untracked.
    let mut c = git(path);
    c.args(["diff", "HEAD", "--numstat", "-z"]);
    if let Ok(num) = run(c) {
        for rec in num.split('\0') {
            let mut it = rec.split('\t');
            let (Some(a), Some(r), Some(p)) = (it.next(), it.next(), it.next()) else { continue };
            if let Some(f) = st.files.iter_mut().find(|f| f.path == p.trim()) {
                f.added = a.parse().unwrap_or(0);
                f.removed = r.parse().unwrap_or(0);
            }
        }
    }
    for f in st.files.iter_mut().filter(|f| f.code == '?' || (f.code == 'A' && f.added == 0)) {
        if let Ok(t) = std::fs::read_to_string(path.join(&f.path)) {
            f.added = t.lines().count() as u32;
        }
    }
    Ok(st)
}

/// Unified diff of one file against HEAD (untracked files diff against empty).
pub fn file_diff(path: &Path, file: &str) -> Result<String> {
    let mut c = git(path);
    c.args(["diff", "HEAD", "--no-color", "--", file]);
    let d = run(c).unwrap_or_default();
    if !d.trim().is_empty() {
        return Ok(d);
    }
    let full = path.join(file);
    if full.exists() {
        let text = std::fs::read_to_string(&full).unwrap_or_default();
        let mut out = format!("--- /dev/null\n+++ b/{file}\n@@ -0,0 +1,{} @@\n", text.lines().count());
        for l in text.lines() {
            out.push('+');
            out.push_str(l);
            out.push('\n');
        }
        return Ok(out);
    }
    Ok(String::new())
}

pub fn stage(path: &Path, file: &str) -> Result<()> {
    let mut c = git(path);
    c.args(["add", "--", file]);
    run(c).map(|_| ())
}

pub fn unstage(path: &Path, file: &str) -> Result<()> {
    let mut c = git(path);
    c.args(["restore", "--staged", "--", file]);
    run(c).map(|_| ())
}

/// Throw away local changes to one file.
pub fn discard(path: &Path, file: &FileStatus) -> Result<()> {
    if file.code == '?' {
        let p = path.join(&file.path);
        if p.is_dir() {
            std::fs::remove_dir_all(p)?;
        } else {
            std::fs::remove_file(p)?;
        }
        return Ok(());
    }
    let mut c = git(path);
    c.args(["checkout", "HEAD", "--", &file.path]);
    run(c).map(|_| ())
}

pub fn commit(path: &Path, message: &str, all: bool) -> Result<String> {
    if all {
        let mut c = git(path);
        c.args(["add", "-A"]);
        run(c)?;
    }
    let mut c = git(path);
    c.args(["commit", "-m", message]);
    run(c)
}

pub fn push(path: &Path) -> Result<String> {
    let branch = current_branch(path).ok_or_else(|| anyhow!("no branch"))?;
    let mut c = git(path);
    c.args(["push", "-u", "origin", &branch]);
    run(c)
}

pub fn checkout_new_branch(path: &Path, branch: &str) -> Result<()> {
    let mut c = git(path);
    c.args(["checkout", "-b", branch]);
    run(c).map(|_| ())
}

/// Create an isolated worktree on a new branch for an agent.
pub fn create_worktree(repo: &Path, dir: &Path, branch: &str) -> Result<PathBuf> {
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut c = git(repo);
    c.args(["worktree", "add", "-b", branch]);
    c.arg(dir);
    c.arg("HEAD");
    run(c)?;
    Ok(dir.to_path_buf())
}

pub fn remove_worktree(repo: &Path, dir: &Path) -> Result<()> {
    let mut c = git(repo);
    c.args(["worktree", "remove", "--force"]);
    c.arg(dir);
    run(c).map(|_| ())
}

/// Merge a worktree's branch back into the repository's current branch.
pub fn merge_branch(repo: &Path, branch: &str) -> Result<String> {
    let mut c = git(repo);
    c.args(["merge", "--no-ff", branch, "-m", &format!("Merge {branch}")]);
    run(c)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub state: String,
    pub head: String,
    pub base: String,
    #[serde(default)]
    pub is_draft: bool,
}

fn gh(cwd: &Path) -> Command {
    let mut c = Command::new("gh");
    c.current_dir(cwd);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    c
}

/// The PR for the current branch, if any (requires `gh`).
pub fn current_pr(path: &Path) -> Option<PullRequest> {
    let mut c = gh(path);
    c.args(["pr", "view", "--json", "number,title,url,state,headRefName,baseRefName,isDraft"]);
    let out = run(c).ok()?;
    let v: serde_json::Value = serde_json::from_str(&out).ok()?;
    Some(PullRequest {
        number: v["number"].as_u64()?,
        title: v["title"].as_str().unwrap_or("").to_string(),
        url: v["url"].as_str().unwrap_or("").to_string(),
        state: v["state"].as_str().unwrap_or("").to_string(),
        head: v["headRefName"].as_str().unwrap_or("").to_string(),
        base: v["baseRefName"].as_str().unwrap_or("").to_string(),
        is_draft: v["isDraft"].as_bool().unwrap_or(false),
    })
}

pub fn create_pr(path: &Path, title: &str, body: &str, draft: bool) -> Result<String> {
    let mut c = gh(path);
    c.args(["pr", "create", "--title", title, "--body", body]);
    if draft {
        c.arg("--draft");
    }
    run(c).map(|s| s.trim().to_string())
}

pub fn merge_pr(path: &Path, squash: bool) -> Result<String> {
    let mut c = gh(path);
    c.args(["pr", "merge", if squash { "--squash" } else { "--merge" }, "--delete-branch"]);
    run(c)
}

/// Slug suitable for a branch name.
pub fn branch_slug(title: &str) -> String {
    let mut s: String = title
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    let s = s.trim_matches('-');
    let s: String = s.chars().take(40).collect();
    if s.is_empty() {
        "agent".into()
    } else {
        s.trim_end_matches('-').to_string()
    }
}
