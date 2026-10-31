//! Review pane: Changes (git diff, commit, PR), Browser, Terminal, Files.

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
        RightTab::Tasks => (I::Workflow, "Background Tasks", ""),
    }
}

#[component]
pub fn RightPane(id: String) -> Element {
    let mut app = use_app();
    let mut plus = use_signal(|| false);
    let mut q = use_signal(String::new);
    let tab = crate::state::use_ui(|u| u.right_tab)();
    let tabs = crate::state::use_ui(|u| u.right_tabs.clone())();
    let id_c = id.clone();
    let cwd = use_memo(move || app.with_session(&id_c, |s| s.cwd()).flatten())();
    let mut open_q = move |_: ()| {
        let v = q.read().trim().to_string();
        plus.set(false);
        q.set(String::new());
        let mut ui = app.ui.write();
        if v.contains("://") || v.starts_with("localhost") || v.contains('.') && !v.contains(' ') && !v.contains('\\') && !v.contains('/') {
            ui.browser_url = if v.contains("://") { v } else { format!("http://{v}") };
            ui.open_panel(RightTab::Browser, false);
        } else {
            ui.reveal_file = if v.is_empty() { None } else { Some(v) };
            ui.open_panel(RightTab::Files, false);
        }
    };
    rsx! {
        div { class: "right-pane",
            div { class: "rp-head",
                div { class: "rp-tabs", role: "tablist",
                    for t in tabs {
                        {
                            let (ticon, ttitle, _) = tab_meta(t.tab);
                            let mut cls = String::from("rp-tab");
                            if t.tab == tab {
                                cls.push_str(" active");
                            }
                            if t.preview {
                                cls.push_str(" preview");
                            }
                            let kind = t.tab;
                            rsx! {
                                span {
                                    key: "{ttitle}",
                                    class: "{cls}",
                                    role: "tab",
                                    title: if t.preview { "{ttitle} · Preview — double-click to keep open" } else { "{ttitle}" },
                                    onclick: move |_| app.ui.write().right_tab = kind,
                                    ondoubleclick: move |_| app.ui.write().keep_panel(kind),
                                    onmousedown: move |e| {
                                        if e.trigger_button() == Some(dioxus::html::input_data::MouseButton::Auxiliary) {
                                            e.prevent_default();
                                            app.ui.write().close_panel(kind);
                                        }
                                    },
                                    Icon { i: ticon, size: 14, stroke: 1.5 }
                                    span { class: "t", "{ttitle}" }
                                    button { class: "icon-btn sm x", title: "Close tab",
                                        onclick: move |e| {
                                            e.stop_propagation();
                                            app.ui.write().close_panel(kind);
                                        },
                                        Icon { i: I::X, size: 12 }
                                    }
                                }
                            }
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
                        for t in [RightTab::Files, RightTab::Terminal, RightTab::Browser, RightTab::Changes, RightTab::Tasks] {
                            {
                                let (i, label, kb) = tab_meta(t);
                                rsx! {
                                    button { key: "{label}", class: "menu-item", style: "height: 32px;",
                                        onclick: move |_| { plus.set(false); app.ui.write().open_panel(t, false); },
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
            }
            div { class: "rp-body",
                match tab {
                    RightTab::Home => rsx! {
                        div { class: "rp-empty",
                            div { style: "display: grid; grid-template-columns: 150px 150px; gap: 14px;",
                                for t in [RightTab::Changes, RightTab::Browser, RightTab::Terminal, RightTab::Files, RightTab::Tasks] {
                                    {
                                        let (i, label, _) = tab_meta(t);
                                        rsx! {
                                            button { key: "{label}", class: "tile",
                                                style: "height: 120px; border: 1px solid var(--border); border-radius: 12px; background: var(--card-2); display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 10px; color: var(--fg-2); font-size: 13px;",
                                                onclick: move |_| app.ui.write().open_panel(t, false),
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
                    RightTab::Terminal => rsx! { super::terminal::TerminalPanel { key: "t-{id}", session_id: id.clone(), cwd: cwd.clone() } },
                    RightTab::Files => rsx! { super::files::FilesPanel { key: "f-{id}", session_id: id.clone(), cwd: cwd.clone() } },
                    RightTab::Tasks => rsx! { super::tasks::TasksPanel { key: "k-{id}", session_id: id.clone() } },
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
    let mut nonce = use_signal(|| 0u32);
    let mut view_agent = use_signal(|| false);
    let mut commit_msg = use_signal(String::new);
    let mut busy = use_signal(|| false);
    // Only what this tab shows, memoized: streaming text never re-renders it.
    let id_m = id.clone();
    let meta = use_memo(move || {
        app.with_session(&id_m, |s| {
            let wt = match &s.environment {
                Environment::Worktree { branch, base, .. } => Some((branch.clone(), base.clone())),
                _ => None,
            };
            (super::chat::change_key(s), s.title.clone(), wt, s.workspace.as_ref().map(|w| w.path.clone()))
        })
    });
    let running = app.is_running(&id);
    let ck = meta().map(|m| m.0).unwrap_or(0);
    let cwd2 = cwd.clone();
    // Re-read git when the agent changed files, finished, or on refresh.
    let git_state = use_resource(use_reactive!(|cwd2, ck, running| {
        let _ = (ck, running, nonce());
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

    let (id_d, cwd_d) = (id.clone(), cwd.clone());
    let agent_diffs = use_memo(move || {
        let _ = meta();
        app.peek_session(&id_d, |s| agent_diffs_of(s, cwd_d.as_deref())).unwrap_or_default()
    });
    let Some((_, title, worktree, repo_root)) = meta() else { return rsx! {} };
    // Used when the message box is left empty.
    let title_default = super::chat::commit_message(&title, &app.peek_session(&id, |s| s.last_assistant_text().unwrap_or_default()).unwrap_or_default());
    let subject_hint = title_default.lines().next().unwrap_or("").to_string();
    let agent_diffs = agent_diffs.read().clone();
    let gs = git_state.read().clone().flatten();

    let Some(cwd) = cwd else {
        if agent_diffs.is_empty() {
            return rsx! {
                div { class: "rp-empty",
                    Icon { i: I::Folder, size: 22 }
                    "This agent has no workspace folder."
                }
            };
        }
        // No folder to compare with git: the edits the agent made.
        return rsx! {
            div { class: "rp-note", Icon { i: I::Folder, size: 13 } "No workspace folder: these are the edits the agent made." }
            div { class: "files-summary",
                Icon { i: I::List, size: 13 }
                "{agent_diffs.len()} Files Edited by Agent"
                span { class: "add", "+{agent_diffs.iter().map(|d| d.added).sum::<u32>()}" }
                span { class: "del", "-{agent_diffs.iter().map(|d| d.removed).sum::<u32>()}" }
            }
            div { class: "diff-list", AgentDiffList { diffs: agent_diffs.clone() } }
        };
    };

    let show_agent = view_agent() || gs.as_ref().map(|g| !g.is_repo).unwrap_or(false);
    let (files, totals, branch, ahead) = match &gs {
        Some(g) if g.is_repo => (g.status.files.clone(), g.status.totals(), g.status.branch.clone().unwrap_or_default(), g.status.ahead),
        _ => (Vec::new(), (0, 0), String::new(), 0),
    };
    let pr = gs.as_ref().and_then(|g| g.pr.clone());
    let base = gs.as_ref().map(|g| g.base.clone()).unwrap_or_else(|| "main".into());
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
                    div { class: "rp-empty", super::loaders::Orbit { size: 18 } }
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
                        dir: "auto",
                        placeholder: "{subject_hint}",
                        title: "Leave empty to use the chat title with the agent's summary as the body",
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
                                    let t = title.clone();
                                    let msg_default = title_default.clone();
                                    let has_files = !files.is_empty();
                                    let summary = app.peek_session(&id, |s| s.last_assistant_text().unwrap_or_default()).unwrap_or_default();
                                    let branch = branch.clone();
                                    let base = base.clone();
                                    move |_| {
                                        let msg = if commit_msg().trim().is_empty() { msg_default.clone() } else { commit_msg() };
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
                span { class: "path", title: "{d.path}",
                    {
                        let (dir, name) = split_path(&d.path);
                        rsx! { if !dir.is_empty() { span { class: "dir", "{dir}/" } } "{name}" }
                    }
                }
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

/// The agents' browser, live: what an agent does there shows here, and the
/// user can click, scroll and type in it too.
#[component]
fn BrowserTab() -> Element {
    use crate::browser_host as host;
    let st = host::STATE();
    let active = st.active.and_then(|id| st.tabs.iter().find(|t| t.id == id).cloned());
    let mut input = use_signal(String::new);
    let mut editing = use_signal(|| false);
    // The address follows the page unless the user is typing one.
    let shown_url = active.as_ref().map(|t| t.url.clone()).unwrap_or_default();
    use_effect(use_reactive!(|shown_url| {
        if !*editing.peek() {
            input.set(if host::is_blank(&shown_url) { String::new() } else { shown_url });
        }
    }));
    let loading = active.as_ref().is_some_and(|t| t.loading);
    let has_page = active.is_some();
    let url_now = active.as_ref().map(|t| t.url.clone()).unwrap_or_default();
    let external = has_page && (url_now.starts_with("http://") || url_now.starts_with("https://"));
    let url_open = url_now.clone();
    rsx! {
        if st.tabs.len() > 1 {
            div { class: "browser-tabs",
                for t in st.tabs.clone() {
                    {
                        let on = Some(t.id) == st.active;
                        let label = tab_label(&t);
                        rsx! {
                            div { key: "{t.id}", class: if on { "browser-tab on" } else { "browser-tab" }, title: "{t.url}",
                                onclick: move |_| host::activate(t.id),
                                if t.loading { super::loaders::Orbit { size: 12 } } else { Icon { i: I::Globe, size: 12 } }
                                span { class: "t", "{label}" }
                                button { class: "x", title: "Close tab",
                                    onclick: move |e| { e.stop_propagation(); host::close_tab(t.id); },
                                    Icon { i: I::X, size: 10 }
                                }
                            }
                        }
                    }
                }
            }
        }
        div { class: "browser-bar",
            button { class: "icon-btn sm", title: "Back", disabled: !active.as_ref().is_some_and(|t| t.can_back), onclick: move |_| host::back(), Icon { i: I::ArrowLeft, size: 14 } }
            button { class: "icon-btn sm", title: "Forward", disabled: !active.as_ref().is_some_and(|t| t.can_forward), onclick: move |_| host::forward(), Icon { i: I::ArrowRight, size: 14 } }
            if loading {
                button { class: "icon-btn sm", title: "Stop", onclick: move |_| host::stop(), Icon { i: I::X, size: 14 } }
            } else {
                button { class: "icon-btn sm", title: "Reload", disabled: !has_page, onclick: move |_| host::reload(), Icon { i: I::Refresh, size: 13 } }
            }
            input { class: "url-input",
                placeholder: "Search or enter an address",
                value: "{input}",
                spellcheck: "false",
                onfocus: move |_| {
                    editing.set(true);
                    // The whole address, ready to be replaced (as browsers do).
                    document::eval("setTimeout(() => document.querySelector('.browser-bar .url-input')?.select(), 0)");
                },
                onblur: move |_| editing.set(false),
                oninput: move |e| input.set(e.value()),
                onkeydown: move |e| {
                    if e.key() == Key::Enter && !input().trim().is_empty() {
                        host::navigate(input().trim());
                        editing.set(false);
                        document::eval("document.querySelector('.browser-bar .url-input')?.blur()");
                    }
                    if e.key() == Key::Escape {
                        input.set(if host::is_blank(&url_now) { String::new() } else { url_now.clone() });
                        document::eval("document.querySelector('.browser-bar .url-input')?.blur()");
                    }
                },
            }
            button { class: "icon-btn sm", title: "New tab",
                onclick: move |_| {
                    host::new_tab("");
                    document::eval("setTimeout(() => document.querySelector('.browser-bar .url-input')?.focus(), 30)");
                },
                Icon { i: I::Plus, size: 14 }
            }
            button { class: if host::ANNOTATING() { "icon-btn sm annotate on" } else { "icon-btn sm annotate" },
                title: if host::ANNOTATING() { "Stop annotating (Esc)" } else { "Annotate: point at elements, click one (Shift+click for more), write what to do, Enter attaches it to the chat" },
                disabled: !has_page,
                onclick: move |_| host::annotate(!host::ANNOTATING()),
                Icon { i: I::Annotate, size: 14 }
            }
            button { class: "icon-btn sm", title: "Developer tools", disabled: !has_page, onclick: move |_| host::devtools(), Icon { i: I::Code, size: 13 } }
            button { class: "icon-btn sm", title: "Open in your browser", disabled: !external,
                onclick: move |_| { let _ = open::that(url_open.clone()); },
                Icon { i: I::External, size: 13 }
            }
            if loading { div { class: "browser-progress" } }
        }
        // The page itself is a native view laid over this area.
        div { id: "lb-area", class: "browser-area",
            // The page's picture while a menu or dialog is over it.
            if let Some(shot) = host::SNAPSHOT() {
                img { class: "browser-snap", src: "data:image/jpeg;base64,{shot}", draggable: "false" }
            }
            if !has_page {
                div { class: "browser-empty",
                    Icon { i: I::Globe, size: 22 }
                    if let Some(e) = st.error.clone() {
                        div { class: "t", "The browser couldn't start" }
                        div { class: "d", "{e}" }
                    } else {
                        div { class: "t", "Larrier's browser" }
                        div { class: "d", "Type an address above. Agents browse here with their browser tools, in the same pages you see." }
                    }
                }
            }
        }
    }
}

/// A page's host name, for a tab without a title yet.
fn page_host(url: &str) -> String {
    url.split("://").nth(1).unwrap_or(url).split('/').next().unwrap_or(url).to_string()
}

/// A page tab's name: its page's title, "New Tab" for a new one.
fn tab_label(t: &crate::browser_host::Tab) -> String {
    if crate::browser_host::is_blank(&t.url) {
        "New Tab".into()
    } else if !t.title.is_empty() {
        t.title.clone()
    } else {
        page_host(&t.url)
    }
}

/// Latest diff per file from the agent's own tool calls.
fn agent_diffs_of(session: &larrier_core::Session, cwd: Option<&str>) -> Vec<FileDiff> {
    let mut out: Vec<FileDiff> = Vec::new();
    for m in &session.messages {
        for t in m.tool_calls() {
            if t.is_error {
                continue;
            }
            for d in &t.diffs {
                let mut d = d.clone();
                d.path = relative_to(&d.path, cwd);
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
                    out.push(d);
                }
            }
        }
    }
    out
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

fn split_path(p: &str) -> (String, String) {
    let norm = p.replace('\\', "/");
    match norm.rsplit_once('/') {
        Some((d, n)) => (d.to_string(), n.to_string()),
        None => (String::new(), norm),
    }
}
