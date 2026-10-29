//! Review pane: Changes (git diff, commit, PR), Browser, Terminal, Files.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use dioxus::prelude::*;
use larrier_agents::git::{self, FileStatus, PullRequest, RepoStatus};
use larrier_core::{lsf::short_path, Environment, FileDiff};

use super::DragRegion;
use crate::icons::{Icon, I};
use crate::state::{use_app, RightTab};

fn tab_meta(t: RightTab) -> (I, &'static str, &'static str) {
    match t {
        RightTab::Home => (I::PanelRight, "New Tab", ""),
        RightTab::Changes => (I::Diff, "Changes", "Ctrl+E"),
        RightTab::Browser => (I::Globe, "Browser", "Ctrl+Shift+B"),
        RightTab::Terminal => (I::Terminal, "Terminal", "Ctrl+J"),
        RightTab::Files => (I::Files, "File", "Ctrl+G"),
    }
}

#[component]
pub fn RightPane(id: String) -> Element {
    let mut app = use_app();
    let mut plus = use_signal(|| false);
    let mut q = use_signal(String::new);
    let tab = app.ui.read().right_tab;
    let cwd = app.open.read().get(&id).and_then(|s| s.cwd());
    let (ticon, ttitle, _) = tab_meta(tab);
    let mut open_q = move |_: ()| {
        let v = q.read().trim().to_string();
        plus.set(false);
        q.set(String::new());
        let mut ui = app.ui.write();
        if v.contains("://") || v.starts_with("localhost") || v.contains('.') && !v.contains(' ') && !v.contains('\\') && !v.contains('/') {
            ui.browser_url = if v.contains("://") { v } else { format!("http://{v}") };
            ui.right_tab = RightTab::Browser;
        } else {
            ui.reveal_file = if v.is_empty() { None } else { Some(v) };
            ui.right_tab = RightTab::Files;
        }
    };
    rsx! {
        div { class: "right-pane",
            div { class: "rp-head",
                if tab != RightTab::Home {
                    span { class: "rp-tab active",
                        Icon { i: ticon, size: 14, stroke: 1.5 }
                        "{ttitle}"
                        button { class: "icon-btn sm", style: "margin-right: -6px;", title: "Close tab",
                            onclick: move |_| app.ui.write().right_tab = RightTab::Home,
                            Icon { i: I::X, size: 12 }
                        }
                    }
                }
                div { class: "menu-anchor",
                    button { class: "icon-btn", title: "New tab", onclick: move |_| plus.set(true), Icon { i: I::Plus, size: 16, stroke: 1.5 } }
                    super::Dropdown { open: plus, up: false,
                        div { class: "menu-search", style: "padding: 8px 10px 10px;",
                            Icon { i: I::Search, size: 14 }
                            input { placeholder: "Open any file, URL, …", autofocus: true, value: "{q}",
                                oninput: move |e| q.set(e.value()),
                                onkeydown: move |e| if e.key() == Key::Enter { open_q(()) },
                            }
                        }
                        for t in [RightTab::Files, RightTab::Terminal, RightTab::Browser, RightTab::Changes] {
                            {
                                let (i, label, kb) = tab_meta(t);
                                rsx! {
                                    button { key: "{label}", class: "menu-item", style: "height: 32px;",
                                        onclick: move |_| { plus.set(false); app.ui.write().right_tab = t; },
                                        span { class: "muted", Icon { i, size: 15, stroke: 1.5 } }
                                        span { "{label}" }
                                        span { class: "kb", "{kb}" }
                                    }
                                }
                            }
                        }
                    }
                }
                DragRegion {}
                button { class: "icon-btn", title: "Expand panel",
                    onclick: move |_| {
                        let w = app.config.read().settings.chat_width;
                        app.config.write().settings.chat_width = if w > 460 { 420 } else { 760 };
                    },
                    Icon { i: I::Maximize2, size: 15, stroke: 1.5 }
                }
                button { class: "icon-btn active", title: "Hide panel (Ctrl+Shift+J)",
                    onclick: move |_| {
                        app.ui.write().right_open = false;
                        app.update_config(|c| c.settings.right_panel_open = false);
                    },
                    Icon { i: I::PanelRight, size: 16, stroke: 1.5 }
                }
            }
            div { class: "rp-body",
                match tab {
                    RightTab::Home => rsx! {
                        div { class: "rp-empty",
                            div { style: "display: grid; grid-template-columns: 150px 150px; gap: 14px;",
                                for t in [RightTab::Changes, RightTab::Browser, RightTab::Terminal, RightTab::Files] {
                                    {
                                        let (i, label, _) = tab_meta(t);
                                        rsx! {
                                            button { key: "{label}", class: "tile",
                                                style: "height: 120px; border: 1px solid var(--border); border-radius: 12px; background: var(--card-2); display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 10px; color: var(--fg-2); font-size: 13px;",
                                                onclick: move |_| app.ui.write().right_tab = t,
                                                Icon { i, size: 18, stroke: 1.5 }
                                                "{label}"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    },
                    RightTab::Changes => rsx! { ChangesTab { id: id.clone(), cwd: cwd.clone() } },
                    RightTab::Browser => rsx! { BrowserTab {} },
                    RightTab::Terminal => rsx! { TerminalTab { key: "{id}", cwd: cwd.clone() } },
                    RightTab::Files => rsx! { FilesTab { key: "{id}", cwd: cwd.clone() } },
                }
            }
        }
    }
}

// ----------------------------------------------------------------- changes

#[derive(Clone, PartialEq)]
struct GitState {
    is_repo: bool,
    status: RepoStatus,
    pr: Option<PullRequest>,
    base: String,
}

#[component]
fn ChangesTab(id: String, cwd: Option<String>) -> Element {
    let app = use_app();
    let session = app.open.read().get(&id).cloned();
    let Some(session) = session else { return rsx! {} };
    let mut nonce = use_signal(|| 0u32);
    let mut view_agent = use_signal(|| false);
    let cwd2 = cwd.clone();
    // Re-read git whenever the session changes or the user refreshes.
    let updated = session.updated_at;
    let git_state = use_resource(use_reactive!(|cwd2, updated| {
        let _ = (updated, nonce());
        async move {
            let Some(c) = cwd2 else { return None };
            tokio::task::spawn_blocking(move || {
                let p = PathBuf::from(&c);
                if !git::is_repo(&p) {
                    return Some(GitState { is_repo: false, status: RepoStatus::default(), pr: None, base: String::new() });
                }
                let status = git::status(&p).unwrap_or_default();
                let pr = git::current_pr(&p);
                let base = git::default_branch(&p);
                Some(GitState { is_repo: true, status, pr, base })
            })
            .await
            .ok()
            .flatten()
        }
    }));

    let agent_diffs: Vec<FileDiff> = {
        // Latest diff per file from the agent's own tool calls.
        let mut out: Vec<FileDiff> = Vec::new();
        for m in &session.messages {
            for t in m.tool_calls() {
                if t.is_error {
                    continue;
                }
                for d in &t.diffs {
                    let mut d = d.clone();
                    d.path = relative_to(&d.path, cwd.as_deref());
                    let d = &d;
                    if let Some(e) = out.iter_mut().find(|x| x.path == d.path) {
                        e.added += d.added;
                        e.removed += d.removed;
                        if d.unified.is_some() || d.new_text.is_some() {
                            e.unified = match (&e.unified, &d.unified) {
                                (Some(a), Some(b)) => Some(format!("{a}{b}")),
                                (_, b) => b.clone().or(e.unified.clone()),
                            };
                            if d.new_text.is_some() {
                                e.old_text = d.old_text.clone();
                                e.new_text = d.new_text.clone();
                            }
                        }
                    } else {
                        out.push(d.clone());
                    }
                }
            }
        }
        out
    };

    let gs = git_state.read().clone().flatten();
    let worktree = match &session.environment {
        Environment::Worktree { branch, base, .. } => Some((branch.clone(), base.clone())),
        _ => None,
    };
    let repo_root = session.workspace.as_ref().map(|w| w.path.clone());

    let Some(cwd) = cwd else {
        return rsx! {
            div { class: "rp-empty",
                Icon { i: I::Folder, size: 22 }
                "This agent has no workspace folder."
                if !agent_diffs.is_empty() { AgentDiffList { diffs: agent_diffs.clone() } }
            }
        };
    };

    let show_agent = view_agent() || gs.as_ref().map(|g| !g.is_repo).unwrap_or(false);
    let (files, totals, branch, ahead) = match &gs {
        Some(g) if g.is_repo => (g.status.files.clone(), g.status.totals(), g.status.branch.clone().unwrap_or_default(), g.status.ahead),
        _ => (Vec::new(), (0, 0), String::new(), 0),
    };
    let pr = gs.as_ref().and_then(|g| g.pr.clone());
    let base = gs.as_ref().map(|g| g.base.clone()).unwrap_or_else(|| "main".into());
    let title_default = session.title.clone();
    let mut commit_msg = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let reveal = app.ui.read().reveal_file.clone();

    let mut run_git = move |label: &'static str, f: Box<dyn FnOnce() -> anyhow::Result<String> + Send>| {
        busy.set(true);
        spawn(async move {
            let r = tokio::task::spawn_blocking(f).await;
            busy.set(false);
            match r {
                Ok(Ok(out)) => {
                    let line = out.lines().last().unwrap_or("").to_string();
                    app.toast(if line.is_empty() { label.to_string() } else { format!("{label}: {line}") });
                }
                Ok(Err(e)) => app.toast_err(format!("{label} failed: {e}")),
                Err(e) => app.toast_err(e.to_string()),
            }
            nonce += 1;
        });
    };

    rsx! {
        div { class: "changes-head",
            div { class: "changes-tabs",
                button { class: if !show_agent { "on" } else { "" }, onclick: move |_| view_agent.set(false), "Local Changes" }
                button { class: if show_agent { "on" } else { "" }, onclick: move |_| view_agent.set(true), "Agent Edits" }
                if let Some(p) = &pr {
                    span { class: "on", "PR #{p.number}" }
                }
                div { class: "grow" }
                button { class: "icon-btn sm", title: "Refresh", onclick: move |_| nonce += 1, Icon { i: I::Refresh, size: 13 } }
            }
            if let Some(p) = pr.clone() {
                div { class: "pr-title",
                    span { class: "ellipsis", "{p.title}" }
                    a { href: "{p.url}", title: "Open on GitHub", Icon { i: I::External, size: 13 } }
                }
                div { class: "row",
                    span { class: if p.state == "MERGED" { "badge-open badge-merged" } else { "badge-open" },
                        Icon { i: I::PullRequest, size: 12 }
                        {if p.is_draft { "Draft".to_string() } else { titlecase(&p.state) }}
                    }
                    span { class: "branch-line", "{p.head}" Icon { i: I::ArrowRight, size: 12 } "{p.base}" }
                }
            } else if !branch.is_empty() {
                div { class: "branch-line",
                    Icon { i: I::GitBranch, size: 12 }
                    "{branch}"
                    if let Some((_, Some(b))) = &worktree {
                        Icon { i: I::ArrowRight, size: 12 }
                        "{b}"
                    }
                    if ahead > 0 {
                        span { class: "faint", "· {ahead} ahead" }
                    }
                }
            }
        }
        if show_agent {
            div { class: "files-summary",
                Icon { i: I::List, size: 13 }
                "{agent_diffs.len()} Files Edited by Agent"
                span { class: "add", "+{agent_diffs.iter().map(|d| d.added).sum::<u32>()}" }
                span { class: "del", "-{agent_diffs.iter().map(|d| d.removed).sum::<u32>()}" }
            }
            div { class: "diff-list",
                if agent_diffs.is_empty() {
                    div { class: "rp-empty", "The agent hasn't edited any files yet." }
                }
                AgentDiffList { diffs: agent_diffs.clone() }
            }
        } else {
            div { class: "files-summary",
                Icon { i: I::List, size: 13 }
                "{files.len()} Files Changed"
                span { class: "add", "+{totals.0}" }
                span { class: "del", "-{totals.1}" }
                div { class: "grow" }
                if let Some(p) = pr.clone().filter(|p| p.state == "OPEN") {
                    button { class: "primary-btn green", disabled: busy(),
                        onclick: {
                            let c = cwd.clone();
                            let _ = p;
                            move |_| { let c = c.clone(); run_git("Merged", Box::new(move || git::merge_pr(Path::new(&c), true))); }
                        },
                        "Squash & Merge"
                    }
                }
            }
            div { class: "diff-list",
                if gs.is_none() {
                    div { class: "rp-empty", div { class: "spinner lg" } }
                } else if files.is_empty() {
                    div { class: "rp-empty",
                        Icon { i: I::CircleCheck, size: 22 }
                        "Working tree clean"
                    }
                }
                for f in files.clone() {
                    GitFile {
                        key: "{f.path}",
                        cwd: cwd.clone(),
                        file: f.clone(),
                        start_open: files.len() <= 12 || reveal.as_deref().map(|r| r.replace('\\', "/").ends_with(&f.path)).unwrap_or(false),
                        on_change: move |_| nonce += 1,
                    }
                }
            }
            if !files.is_empty() || ahead > 0 {
                div { class: "commit-box",
                    textarea {
                        placeholder: "Commit message (leave empty to use the agent title)",
                        value: "{commit_msg}",
                        oninput: move |e| commit_msg.set(e.value()),
                    }
                    div { class: "row", style: "flex-wrap: wrap;",
                        button { class: "primary-btn", disabled: busy() || files.is_empty(),
                            onclick: {
                                let c = cwd.clone();
                                let t = title_default.clone();
                                move |_| {
                                    let msg = if commit_msg().trim().is_empty() { t.clone() } else { commit_msg() };
                                    let c = c.clone();
                                    commit_msg.set(String::new());
                                    run_git("Committed", Box::new(move || git::commit(Path::new(&c), &msg, true)));
                                }
                            },
                            Icon { i: I::Commit, size: 13 }
                            "Commit"
                        }
                        button { class: "ghost-btn", disabled: busy(),
                            onclick: {
                                let c = cwd.clone();
                                let t = title_default.clone();
                                let has_files = !files.is_empty();
                                move |_| {
                                    let msg = if commit_msg().trim().is_empty() { t.clone() } else { commit_msg() };
                                    let c = c.clone();
                                    commit_msg.set(String::new());
                                    run_git("Pushed", Box::new(move || {
                                        let p = Path::new(&c);
                                        if has_files { git::commit(p, &msg, true)?; }
                                        git::push(p)
                                    }));
                                }
                            },
                            Icon { i: I::Upload, size: 13 }
                            "Commit & Push"
                        }
                        if pr.is_none() {
                            button { class: "ghost-btn", disabled: busy(),
                                onclick: {
                                    let c = cwd.clone();
                                    let t = title_default.clone();
                                    let has_files = !files.is_empty();
                                    let summary = session.last_assistant_text().unwrap_or_default();
                                    let branch = branch.clone();
                                    let base = base.clone();
                                    move |_| {
                                        let msg = if commit_msg().trim().is_empty() { t.clone() } else { commit_msg() };
                                        let (c, t, body, branch, base) = (c.clone(), t.clone(), summary.clone(), branch.clone(), base.clone());
                                        run_git("Pull request", Box::new(move || {
                                            let p = Path::new(&c);
                                            if branch == base {
                                                git::checkout_new_branch(p, &format!("larrier/{}", git::branch_slug(&t)))?;
                                            }
                                            if has_files { git::commit(p, &msg, true)?; }
                                            git::push(p)?;
                                            git::create_pr(p, &t, &format!("{body}\n\n— Created with Larrier"), false)
                                        }));
                                    }
                                },
                                Icon { i: I::PullRequest, size: 13 }
                                "Create PR"
                            }
                        }
                        if let (Some((wt_branch, _)), Some(root)) = (worktree.clone(), repo_root.clone()) {
                            button { class: "ghost-btn", disabled: busy() || !files.is_empty(), title: "Merge this worktree's branch into the main checkout",
                                onclick: move |_| {
                                    let (b, r) = (wt_branch.clone(), root.clone());
                                    run_git("Merged worktree", Box::new(move || git::merge_branch(Path::new(&r), &b)));
                                },
                                Icon { i: I::Layers, size: 13 }
                                "Merge to local"
                            }
                        }
                    }
                }
            }
        }
    }
}

fn titlecase(s: &str) -> String {
    let lower = s.to_lowercase();
    let mut c = lower.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

#[component]
fn GitFile(cwd: String, file: FileStatus, start_open: bool, on_change: EventHandler<()>) -> Element {
    let app = use_app();
    let mut open = use_signal(|| start_open);
    let c = cwd.clone();
    let p = file.path.clone();
    let diff = use_resource(move || {
        let (c, p) = (c.clone(), p.clone());
        let o = open();
        async move {
            if !o {
                return None;
            }
            tokio::task::spawn_blocking(move || git::file_diff(Path::new(&c), &p).ok()).await.ok().flatten()
        }
    });
    let code = if file.code == '?' { 'Q' } else { file.code };
    let code_label = if file.code == '?' { 'U' } else { file.code };
    let f2 = file.clone();
    let cwd2 = cwd.clone();
    rsx! {
        div { class: "diff-file",
            div { class: "diff-file-head", onclick: move |_| open.toggle(),
                span { class: "faint", if open() { Icon { i: I::ChevronDown, size: 12 } } else { Icon { i: I::ChevronRight, size: 12 } } }
                span { class: "code-status {code}", "{code_label}" }
                span { class: "path", title: "{file.path}",
                    {
                        let (dir, name) = split_path(&file.path);
                        rsx! { if !dir.is_empty() { span { class: "dir", "{dir}/" } } "{name}" }
                    }
                }
                div { class: "actions",
                    span { class: "add", style: "font-size: 11px;", "+{file.added}" }
                    span { class: "del", style: "font-size: 11px;", "-{file.removed}" }
                    button { class: "icon-btn sm", title: "Discard changes",
                        onclick: move |e| {
                            e.stop_propagation();
                            match git::discard(Path::new(&cwd2), &f2) {
                                Ok(()) => { app.toast(format!("Discarded {}", short_path(&f2.path))); on_change.call(()); }
                                Err(e) => app.toast_err(e.to_string()),
                            }
                        },
                        Icon { i: I::Undo, size: 12 }
                    }
                }
            }
            if open() {
                match diff.read().clone().flatten() {
                    Some(d) => rsx! { DiffView { unified: d } },
                    None => rsx! { div { style: "padding: 8px 16px", class: "faint", "Loading…" } },
                }
            }
        }
    }
}

#[component]
fn AgentDiffList(diffs: Vec<FileDiff>) -> Element {
    rsx! {
        for d in diffs {
            AgentDiff { key: "{d.path}", d: d.clone() }
        }
    }
}

#[component]
fn AgentDiff(d: FileDiff) -> Element {
    let mut open = use_signal(|| true);
    let unified = d.unified.clone().unwrap_or_else(|| synth_unified(d.old_text.as_deref().unwrap_or(""), d.new_text.as_deref().unwrap_or("")));
    rsx! {
        div { class: "diff-file",
            div { class: "diff-file-head", onclick: move |_| open.toggle(),
                span { class: "faint", if open() { Icon { i: I::ChevronDown, size: 12 } } else { Icon { i: I::ChevronRight, size: 12 } } }
                span { class: "path", "{d.path}" }
                div { class: "actions",
                    span { class: "add", style: "font-size: 11px;", "+{d.added}" }
                    span { class: "del", style: "font-size: 11px;", "-{d.removed}" }
                }
            }
            if open() {
                DiffView { unified }
            }
        }
    }
}

fn synth_unified(old: &str, new: &str) -> String {
    // Real unified diff (with line numbers) so the review pane matches git.
    similar::TextDiff::from_lines(old, new).unified_diff().context_radius(3).to_string()
}

#[component]
pub fn DiffView(unified: String) -> Element {
    // Parse into rows with new-file line numbers.
    let mut rows: Vec<(&'static str, Option<u32>, String)> = Vec::new();
    let mut new_ln: u32 = 0;
    for l in unified.lines() {
        if l.starts_with("diff --git") || l.starts_with("index ") || l.starts_with("--- ") || l.starts_with("+++ ") || l.starts_with("new file") || l.starts_with("deleted file") {
            continue;
        }
        if let Some(rest) = l.strip_prefix("@@") {
            if let Some(plus) = rest.split('+').nth(1) {
                new_ln = plus.split([',', ' ']).next().and_then(|n| n.parse().ok()).unwrap_or(1);
            }
            rows.push(("hunk", None, l.to_string()));
            continue;
        }
        if let Some(t) = l.strip_prefix('+') {
            rows.push(("add", Some(new_ln), t.to_string()));
            new_ln += 1;
        } else if let Some(t) = l.strip_prefix('-') {
            rows.push(("del", None, t.to_string()));
        } else {
            let t = l.strip_prefix(' ').unwrap_or(l);
            rows.push(("ctx", Some(new_ln), t.to_string()));
            new_ln += 1;
        }
    }
    let truncated = rows.len() > 3000;
    rows.truncate(3000);
    rsx! {
        div { class: "diff-body",
            for (i, (kind, ln, text)) in rows.into_iter().enumerate() {
                div { key: "{i}", class: "diff-line {kind}",
                    span { class: "ln", {ln.map(|n| n.to_string()).unwrap_or_default()} }
                    span { class: "tx", "{text}" }
                }
            }
            if truncated {
                div { class: "faint", style: "padding: 6px 16px;", "Diff truncated" }
            }
        }
    }
}

// ----------------------------------------------------------------- browser

#[component]
fn BrowserTab() -> Element {
    let mut app = use_app();
    let url = app.ui.read().browser_url.clone();
    let mut input = use_signal(|| url.clone());
    let mut reload = use_signal(|| 0u32);
    let src = url.clone();
    rsx! {
        div { class: "browser-bar",
            button { class: "icon-btn sm", title: "Back", onclick: move |_| { document::eval("history.back()"); }, Icon { i: I::ArrowLeft, size: 13 } }
            button { class: "icon-btn sm", title: "Reload", onclick: move |_| reload += 1, Icon { i: I::Refresh, size: 13 } }
            input { class: "url-input",
                value: "{input}",
                oninput: move |e| input.set(e.value()),
                onkeydown: move |e| {
                    if e.key() == Key::Enter {
                        let mut u = input().trim().to_string();
                        if !u.contains("://") {
                            u = if u.starts_with("localhost") || u.starts_with("127.") { format!("http://{u}") } else { format!("https://{u}") };
                        }
                        input.set(u.clone());
                        app.ui.write().browser_url = u;
                    }
                },
            }
            button { class: "icon-btn sm", title: "Open in your browser",
                onclick: move |_| { let _ = open::that(app.ui.read().browser_url.clone()); },
                Icon { i: I::External, size: 13 }
            }
        }
        {
            let k = format!("{src}-{}", reload());
            rsx! { iframe { key: "{k}", class: "browser-frame", src: "{src}" } }
        }
    }
}

// ----------------------------------------------------------------- terminal

#[derive(Clone, PartialEq)]
enum TermLine {
    Cmd(String),
    Out(String),
    Err(String),
}

#[component]
fn TerminalTab(cwd: Option<String>) -> Element {
    let mut lines = use_signal(Vec::<TermLine>::new);
    let mut input = use_signal(String::new);
    let mut running = use_signal(|| false);
    let mut history = use_signal(Vec::<String>::new);
    let mut hist_idx = use_signal(|| None::<usize>);
    let mut cancel: Signal<Option<tokio::sync::oneshot::Sender<()>>> = use_signal(|| None);
    let dir = cwd.clone().unwrap_or_else(|| dirs::home_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|| ".".into()));
    let dir_label = short_path(&dir);
    let n = lines.read().len();
    use_effect(use_reactive!(|n| {
        let _ = n;
        document::eval("const t=document.getElementById('term'); if(t) t.scrollTop=t.scrollHeight;");
    }));

    let mut run = {
        let dir = dir.clone();
        move |cmd: String| {
            if cmd.trim().is_empty() {
                return;
            }
            if cmd.trim() == "clear" || cmd.trim() == "cls" {
                lines.set(Vec::new());
                return;
            }
            history.write().push(cmd.clone());
            hist_idx.set(None);
            lines.write().push(TermLine::Cmd(cmd.clone()));
            running.set(true);
            let dir = dir.clone();
            let (tx, rx) = tokio::sync::oneshot::channel::<()>();
            cancel.set(Some(tx));
            spawn(async move {
                use tokio::io::{AsyncBufReadExt, BufReader};
                let mut c = if cfg!(windows) {
                    let mut c = tokio::process::Command::new("cmd");
                    c.args(["/D", "/S", "/C", &cmd]);
                    c
                } else {
                    let mut c = tokio::process::Command::new("sh");
                    c.args(["-c", &cmd]);
                    c
                };
                c.current_dir(&dir).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
                #[cfg(windows)]
                c.creation_flags(0x0800_0000);
                match c.spawn() {
                    Ok(mut child) => {
                        let mut out = BufReader::new(child.stdout.take().unwrap()).lines();
                        let mut err = BufReader::new(child.stderr.take().unwrap()).lines();
                        let mut rx = rx;
                        let (mut od, mut ed) = (false, false);
                        while !(od && ed) {
                            tokio::select! {
                                l = out.next_line(), if !od => match l { Ok(Some(l)) => lines.write().push(TermLine::Out(l)), _ => od = true },
                                l = err.next_line(), if !ed => match l { Ok(Some(l)) => lines.write().push(TermLine::Err(l)), _ => ed = true },
                                _ = &mut rx => { let _ = child.start_kill(); lines.write().push(TermLine::Err("^C".into())); break; }
                            }
                        }
                        if let Ok(st) = child.wait().await {
                            if !st.success() {
                                lines.write().push(TermLine::Err(format!("exit code {}", st.code().unwrap_or(-1))));
                            }
                        }
                    }
                    Err(e) => lines.write().push(TermLine::Err(e.to_string())),
                }
                running.set(false);
                cancel.set(None);
            });
        }
    };

    rsx! {
        div { class: "term", id: "term",
            if lines.read().is_empty() {
                div { class: "faint", "Run commands in {dir_label}. Output streams here." }
            }
            for (i, l) in lines.read().iter().enumerate() {
                match l {
                    TermLine::Cmd(c) => rsx! { div { key: "{i}", class: "cmd-line", "{c}" } },
                    TermLine::Out(o) => rsx! { div { key: "{i}", class: "out", "{o}" } },
                    TermLine::Err(o) => rsx! { div { key: "{i}", class: "err", "{o}" } },
                }
            }
        }
        div { class: "term-input",
            span { class: "term-cwd", "{dir_label} ›" }
            input {
                value: "{input}",
                placeholder: if running() { "running… (Ctrl+C to stop)" } else { "" },
                oninput: move |e| input.set(e.value()),
                onkeydown: move |e| {
                    match e.key() {
                        Key::Enter if !running() => {
                            let c = input();
                            input.set(String::new());
                            run(c);
                        }
                        Key::ArrowUp => {
                            let h = history.read().clone();
                            if h.is_empty() { return; }
                            let i = hist_idx().map(|i| i.saturating_sub(1)).unwrap_or(h.len() - 1);
                            hist_idx.set(Some(i));
                            input.set(h[i].clone());
                        }
                        Key::Character(ref c) if c == "c" && e.modifiers().ctrl() && running() => {
                            if let Some(tx) = cancel.write().take() { let _ = tx.send(()); }
                        }
                        _ => {}
                    }
                },
            }
        }
    }
}

// ----------------------------------------------------------------- files

#[component]
fn FilesTab(cwd: Option<String>) -> Element {
    let Some(root) = cwd else {
        return rsx! { div { class: "rp-empty", "No workspace folder" } };
    };
    let mut expanded = use_signal(BTreeSet::<String>::new);
    let mut selected = use_signal(|| None::<String>);
    let mut filter = use_signal(String::new);
    let root2 = root.clone();
    let content = use_resource(move || {
        let sel = selected();
        async move {
            let p = sel?;
            tokio::task::spawn_blocking(move || {
                let meta = std::fs::metadata(&p).ok()?;
                if meta.len() > 2_000_000 {
                    return Some("File too large to preview".to_string());
                }
                let bytes = std::fs::read(&p).ok()?;
                if bytes.iter().take(8000).any(|b| *b == 0) {
                    return Some("Binary file".to_string());
                }
                Some(String::from_utf8_lossy(&bytes).to_string())
            })
            .await
            .ok()
            .flatten()
        }
    });
    let rows = flatten_tree(&root, &expanded.read(), &filter.read());
    let sel = selected();
    let rel = sel.as_ref().map(|s| s.strip_prefix(&root2).unwrap_or(s).trim_start_matches(['/', '\\']).to_string());
    rsx! {
        div { class: "browser-bar",
            Icon { i: I::Search, size: 13 }
            input { class: "url-input", placeholder: "Filter files…", value: "{filter}", oninput: move |e| filter.set(e.value()) }
        }
        div { class: "files-split",
            div { class: "file-tree",
                for (path, name, depth, is_dir) in rows {
                    {
                        let p2 = path.clone();
                        let open = expanded.read().contains(&path);
                        rsx! {
                            div {
                                key: "{path}",
                                class: if sel.as_deref() == Some(path.as_str()) { "tree-row sel" } else { "tree-row" },
                                style: "padding-left: {8 + depth * 12}px;",
                                onclick: move |_| {
                                    if is_dir {
                                        let mut e = expanded.write();
                                        if !e.remove(&p2) { e.insert(p2.clone()); }
                                    } else {
                                        selected.set(Some(p2.clone()));
                                    }
                                },
                                if is_dir {
                                    span { class: "faint", if open { Icon { i: I::ChevronDown, size: 11 } } else { Icon { i: I::ChevronRight, size: 11 } } }
                                    Icon { i: if open { I::FolderOpen } else { I::Folder }, size: 13 }
                                } else {
                                    span { style: "width: 11px" }
                                    Icon { i: I::File, size: 13 }
                                }
                                span { class: "ellipsis", "{name}" }
                            }
                        }
                    }
                }
            }
            div { class: "file-view",
                if let Some(r) = rel {
                    div { class: "crumbs",
                        for (i, seg) in r.split(['/', '\\']).enumerate() {
                            if i > 0 { span { class: "faint", Icon { i: I::ChevronRight, size: 11 } } }
                            span { "{seg}" }
                        }
                    }
                    match content.read().clone().flatten() {
                        Some(text) => rsx! {
                            div { class: "code-view",
                                for (i, l) in text.lines().take(20000).enumerate() {
                                    div { key: "{i}", class: "code-line", span { class: "ln", "{i + 1}" } span { "{l}" } }
                                }
                            }
                        },
                        None => rsx! { div { class: "rp-empty", "Loading…" } },
                    }
                } else {
                    div { class: "rp-empty", Icon { i: I::File, size: 22 } "Select a file to view" }
                }
            }
        }
    }
}

const SKIP_DIRS: &[&str] = &[".git", "node_modules", "target", ".next", "dist", "build", "__pycache__", ".venv", "venv", ".idea", ".turbo"];

fn flatten_tree(root: &str, expanded: &BTreeSet<String>, filter: &str) -> Vec<(String, String, usize, bool)> {
    let mut out = Vec::new();
    let f = filter.to_lowercase();
    fn walk(dir: &Path, depth: usize, expanded: &BTreeSet<String>, f: &str, out: &mut Vec<(String, String, usize, bool)>) {
        if out.len() > 4000 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<(String, PathBuf, bool)> = rd
            .flatten()
            .map(|e| {
                let p = e.path();
                let is_dir = p.is_dir();
                (e.file_name().to_string_lossy().to_string(), p, is_dir)
            })
            .filter(|(n, _, d)| !(*d && SKIP_DIRS.contains(&n.as_str())))
            .collect();
        entries.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.to_lowercase().cmp(&b.0.to_lowercase())));
        for (name, p, is_dir) in entries {
            let ps = p.to_string_lossy().to_string();
            if !f.is_empty() {
                if is_dir {
                    if depth < 6 {
                        walk(&p, depth, expanded, f, out);
                    }
                } else if name.to_lowercase().contains(f) {
                    out.push((ps, name, depth, false));
                }
                continue;
            }
            let open = expanded.contains(&ps);
            out.push((ps, name, depth, is_dir));
            if is_dir && open {
                walk(&p, depth + 1, expanded, f, out);
            }
        }
    }
    walk(Path::new(root), 0, expanded, &f, &mut out);
    out
}

fn split_path(p: &str) -> (String, String) {
    let norm = p.replace('\\', "/");
    match norm.rsplit_once('/') {
        Some((d, n)) => (d.to_string(), n.to_string()),
        None => (String::new(), norm),
    }
}

/// Show agent edit paths relative to the workspace when they're inside it.
fn relative_to(path: &str, root: Option<&str>) -> String {
    let norm = |s: &str| s.replace('\\', "/");
    let p = norm(path);
    if let Some(r) = root {
        let r = norm(r);
        let r = r.trim_end_matches('/');
        if p.len() > r.len() + 1 && p[..r.len()].eq_ignore_ascii_case(r) && p.as_bytes()[r.len()] == b'/' {
            return p[r.len() + 1..].to_string();
        }
    }
    p
}
