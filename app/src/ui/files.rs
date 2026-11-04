//! Files panel: open-file tabs with language icons, a breadcrumb with
//! back / forward, quick open, a lazy file tree and a code editor (Monaco,
//! see `editor`) for any text file.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use dioxus::prelude::*;

use crate::highlight;
use crate::icons::{has_icon, Lucide};
use crate::logos::{logo, Logo};
use crate::state::use_app;

const MAX_TEXT: u64 = 12 * 1024 * 1024;
const MAX_IMAGE: u64 = 8 * 1024 * 1024;
const MAX_INDEX: usize = 50_000;
const SKIP_INDEX: &[&str] = &[".git", "node_modules", "target", "dist", "build", ".venv", "venv", "__pycache__", ".next", ".turbo", ".cache", ".idea"];

// ------------------------------------------------------------------ tab state

/// Open tabs + navigation history for one session (kept across re-mounts
/// and launches).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
struct Tabs {
    files: Vec<String>,
    /// The tab a single click opened; the next one replaces it.
    preview: Option<String>,
    active: Option<String>,
    history: Vec<String>,
    pos: usize,
}

fn tab_store() -> &'static Mutex<HashMap<String, Tabs>> {
    static TABS: OnceLock<Mutex<HashMap<String, Tabs>>> = OnceLock::new();
    TABS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn with_tabs<R>(sid: &str, f: impl FnOnce(&mut Tabs) -> R) -> R {
    let mut map = tab_store().lock().unwrap();
    let t = map.entry(sid.to_string()).or_insert_with(|| crate::keep::value(sid, "files").unwrap_or_default());
    let out = f(t);
    crate::keep::set_value(sid, "files", &(*t != Tabs::default()).then_some(&*t));
    out
}

/// Show a file. A file that is open is shown; otherwise the preview tab is
/// replaced (or a tab added). `keep` opens it as a regular tab.
fn open_tab(sid: &str, path: &str, keep: bool) {
    with_tabs(sid, |t| {
        if t.files.iter().any(|f| f == path) {
            if keep && t.preview.as_deref() == Some(path) {
                t.preview = None;
            }
        } else if let Some(i) = t.preview.as_ref().filter(|_| !keep).and_then(|p| t.files.iter().position(|f| f == p)) {
            t.files[i] = path.to_string();
            t.preview = Some(path.to_string());
        } else {
            t.files.push(path.to_string());
            t.preview = (!keep).then(|| path.to_string());
        }
        t.active = Some(path.to_string());
        if t.history.get(t.pos).map(|h| h.as_str()) != Some(path) {
            if !t.history.is_empty() {
                t.history.truncate(t.pos + 1);
            }
            t.history.push(path.to_string());
            t.pos = t.history.len() - 1;
        }
    });
}

fn keep_tab(sid: &str, path: &str) {
    with_tabs(sid, |t| {
        if t.preview.as_deref() == Some(path) {
            t.preview = None;
        }
    });
}

fn close_tab(sid: &str, path: &str) {
    with_tabs(sid, |t| {
        if t.preview.as_deref() == Some(path) {
            t.preview = None;
        }
        if let Some(i) = t.files.iter().position(|f| f == path) {
            t.files.remove(i);
            if t.active.as_deref() == Some(path) {
                t.active = t.files.get(i).or_else(|| t.files.get(i.saturating_sub(1))).cloned();
            }
        }
    });
}

fn go_history(sid: &str, delta: isize) {
    with_tabs(sid, |t| {
        let target = t.pos as isize + delta;
        if target < 0 || target as usize >= t.history.len() {
            return;
        }
        t.pos = target as usize;
        let p = t.history[t.pos].clone();
        if !t.files.contains(&p) {
            t.files.push(p.clone());
        }
        t.active = Some(p);
    });
}

// ------------------------------------------------------------------ helpers

fn norm(p: &str) -> String {
    p.replace('\\', "/")
}

/// Path relative to the workspace root (forward slashes), or the full path.
fn rel(root: Option<&str>, path: &str) -> String {
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

fn file_name(p: &str) -> String {
    norm(p).rsplit('/').next().unwrap_or(p).to_string()
}

fn ext_of(name: &str) -> String {
    name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
}

fn is_image(name: &str) -> bool {
    matches!(ext_of(name).as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" | "bmp")
}

fn mime(name: &str) -> &'static str {
    match ext_of(name).as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        _ => "application/octet-stream",
    }
}

/// Icon for a file: a brand logo for well-known ecosystems, else a Lucide
/// file glyph.
#[component]
fn FileIcon(name: String, #[props(default = 14)] size: u32) -> Element {
    let lower = name.to_ascii_lowercase();
    let brand = match lower.as_str() {
        "dockerfile" | "docker-compose.yml" | "docker-compose.yaml" | "compose.yaml" | ".dockerignore" => Some("docker"),
        ".gitignore" | ".gitattributes" | ".gitmodules" => Some("git"),
        "package.json" | "package-lock.json" | ".npmrc" => Some("npm"),
        "cargo.toml" | "cargo.lock" => Some("rust"),
        _ => match ext_of(&lower).as_str() {
            "rs" => Some("rust"),
            "py" | "pyw" | "pyi" | "ipynb" => Some("python"),
            "js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" | "mts" | "cts" => Some("node"),
            "sh" | "bash" | "zsh" => Some("bash"),
            "ps1" | "psm1" | "psd1" => Some("powershell"),
            _ => None,
        },
    };
    if let Some(svg) = brand.and_then(logo) {
        return rsx! { Logo { svg, size } };
    }
    let glyph = match ext_of(&lower).as_str() {
        "json" | "jsonc" | "json5" => "file-braces",
        "md" | "mdx" | "txt" | "rst" | "log" => "file-text",
        "toml" | "ini" | "cfg" | "conf" | "config" | "yaml" | "yml" | "env" | "csproj" | "props" | "editorconfig" => "file-cog",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" | "bmp" => "file-image",
        "zip" | "gz" | "tar" | "7z" | "rar" => "file-archive",
        "bat" | "cmd" => "file-terminal",
        "" if lower.starts_with(".env") => "file-cog",
        _ if highlight::knows(&lower) => "file-code",
        _ => "file",
    };
    let glyph = if has_icon(glyph) { glyph } else { "file" };
    rsx! { span { class: "fv-ic", Lucide { name: glyph.to_string(), size, stroke: 1.5 } } }
}

/// Every file under `root` (relative, forward slashes), skipping vendor dirs.
fn index_files(root: &str) -> Vec<String> {
    let base = Path::new(root);
    let mut out = Vec::new();
    let mut stack = vec![base.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let p = e.path();
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                if !SKIP_INDEX.contains(&name.as_str()) {
                    stack.push(p);
                }
            } else if let Ok(r) = p.strip_prefix(base) {
                out.push(norm(&r.to_string_lossy()));
                if out.len() >= MAX_INDEX {
                    return out;
                }
            }
        }
    }
    out.sort_by_key(|s| s.to_ascii_lowercase());
    out
}

/// Subsequence fuzzy score (higher is better); `None` when `q` doesn't match.
fn fuzzy(q: &str, path: &str) -> Option<i64> {
    if q.is_empty() {
        return Some(0);
    }
    let p = path.to_ascii_lowercase();
    let name_start = p.rfind('/').map(|i| i + 1).unwrap_or(0);
    let pb = p.as_bytes();
    let mut score: i64 = 0;
    let mut pi = 0usize;
    let mut last: Option<usize> = None;
    for qc in q.to_ascii_lowercase().bytes() {
        if qc == b' ' {
            continue;
        }
        let mut found = None;
        while pi < pb.len() {
            if pb[pi] == qc {
                found = Some(pi);
                pi += 1;
                break;
            }
            pi += 1;
        }
        let at = found?;
        score += 10;
        if at >= name_start {
            score += 8;
        }
        if last.map(|l| l + 1 == at).unwrap_or(false) {
            score += 12;
        }
        if at == name_start || (at > 0 && matches!(pb[at - 1], b'/' | b'_' | b'-' | b'.')) {
            score += 6;
        }
        last = Some(at);
    }
    if p[name_start..].starts_with(&q.to_ascii_lowercase()) {
        score += 40;
    }
    Some(score - path.len() as i64 / 4)
}

#[derive(Clone, Debug, PartialEq)]
struct Entry {
    name: String,
    path: String,
    dir: bool,
}

fn read_dir_sorted(dir: &str) -> Vec<Entry> {
    let mut v: Vec<Entry> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    if name == ".git" {
                        return None;
                    }
                    let dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    Some(Entry { path: e.path().to_string_lossy().to_string(), name, dir })
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| b.dir.cmp(&a.dir).then(a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase())));
    v
}

#[derive(Clone, Debug, PartialEq)]
enum Loaded {
    /// The file (`path`) as it is on disk, its line endings kept.
    Text { path: String, text: String, lang: Option<&'static str> },
    Image { url: String, bytes: u64 },
    Binary { bytes: u64 },
    TooLarge { bytes: u64 },
    Missing(String),
}

fn load(path: &str) -> Loaded {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => return Loaded::Missing(e.to_string()),
    };
    let bytes = meta.len();
    let name = file_name(path);
    if is_image(&name) {
        if bytes > MAX_IMAGE {
            return Loaded::TooLarge { bytes };
        }
        return match std::fs::read(path) {
            Ok(data) => {
                use base64::Engine;
                let b64 = base64::engine::general_purpose::STANDARD.encode(data);
                Loaded::Image { url: format!("data:{};base64,{b64}", mime(&name)), bytes }
            }
            Err(e) => Loaded::Missing(e.to_string()),
        };
    }
    if bytes > MAX_TEXT {
        return Loaded::TooLarge { bytes };
    }
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => return Loaded::Missing(e.to_string()),
    };
    if data.iter().take(8000).any(|b| *b == 0) {
        return Loaded::Binary { bytes };
    }
    let text = String::from_utf8_lossy(&data).to_string();
    let lang = highlight::syntax_name(&name);
    Loaded::Text { path: path.to_string(), text, lang }
}

fn human_bytes(n: u64) -> String {
    if n >= 1 << 20 {
        format!("{:.1} MB", n as f64 / (1u64 << 20) as f64)
    } else if n >= 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}

// ------------------------------------------------------------------ component

#[component]
pub fn FilesPanel(session_id: String, cwd: Option<String>) -> Element {
    let mut app = use_app();
    let mut rev = use_signal(|| 0u64);
    let mut tree_open = crate::keep::use_kept(&session_id, "files.tree", || true);
    let mut quick = use_signal(String::new);
    let mut quick_open = use_signal(|| false);
    let mut hl = use_signal(|| 0usize);
    let expanded: Signal<BTreeSet<String>> = crate::keep::use_kept(&session_id, "files.dirs", BTreeSet::new);
    let mut dirs: Signal<HashMap<String, Vec<Entry>>> = use_signal(HashMap::new);

    let _ = rev();
    let tabs = with_tabs(&session_id, |t| t.clone());
    let active = tabs.active.clone();
    let root = cwd.clone();

    // Root listing for the tree.
    let r0 = root.clone();
    use_hook(move || {
        if let Some(r) = r0 {
            let entries = read_dir_sorted(&r);
            dirs.write().insert(r, entries);
        }
        // Folders left open (in a past launch too) list their files again.
        let open: Vec<String> = expanded.peek().iter().cloned().collect();
        for d in open {
            if Path::new(&d).is_dir() {
                let entries = read_dir_sorted(&d);
                dirs.write().insert(d, entries);
            }
        }
    });

    // File index for quick open.
    let r1 = root.clone();
    let index = use_resource(use_reactive!(|r1| async move {
        let Some(r) = r1 else { return Vec::new() };
        tokio::task::spawn_blocking(move || index_files(&r)).await.unwrap_or_default()
    }));

    // Open whatever the rest of the app asked to reveal.
    let reveal = app.ui.read().reveal_file.clone();
    let (sid_r, root_r) = (session_id.clone(), root.clone());
    use_effect(use_reactive!(|reveal| {
        let Some(p) = reveal else { return };
        app.ui.write().reveal_file = None;
        let cand = PathBuf::from(&p);
        let full = if cand.is_absolute() { cand } else { root_r.as_deref().map(|r| Path::new(r).join(&p)).unwrap_or(cand) };
        if full.is_file() {
            open_tab(&sid_r, &full.to_string_lossy(), false);
            quick_open.set(false);
            rev += 1;
        } else {
            quick.set(p.clone());
            quick_open.set(true);
        }
    }));

    // Contents of the active file.
    let act = active.clone();
    let content = use_resource(use_reactive!(|act| async move {
        let p = act?;
        tokio::task::spawn_blocking(move || load(&p)).await.ok()
    }));

    let Some(root_dir) = root.clone() else {
        return rsx! {
            style { {CSS} }
            div { class: "fv-empty", "This agent has no workspace folder." }
        };
    };

    // Quick-open results.
    let q = quick.read().trim().to_string();
    let results: Vec<String> = if quick_open() || active.is_none() {
        let idx = index.read();
        let mut scored: Vec<(i64, &String)> = idx
            .as_ref()
            .map(|v| v.iter().filter_map(|p| fuzzy(&q, p).map(|s| (s, p))).collect())
            .unwrap_or_default();
        if !q.is_empty() {
            scored.sort_by(|a, b| b.0.cmp(&a.0));
        }
        scored.into_iter().take(50).map(|(_, p)| p.clone()).collect()
    } else {
        Vec::new()
    };
    let indexing = index.read().is_none();
    let sel = hl().min(results.len().saturating_sub(1));

    let open_rel = {
        let sid = session_id.clone();
        let rd = root_dir.clone();
        move |relp: String| {
            let full = Path::new(&rd).join(&relp);
            open_tab(&sid, &full.to_string_lossy(), false);
            quick_open.set(false);
            quick.set(String::new());
            hl.set(0);
            rev += 1;
        }
    };
    let results_for_keys = results.clone();
    let open_first = open_rel.clone();

    // Breadcrumb.
    let crumbs: Vec<String> = active.as_deref().map(|a| rel(Some(&root_dir), a).split('/').map(|s| s.to_string()).collect()).unwrap_or_default();
    let can_back = tabs.pos > 0 && !tabs.history.is_empty();
    let can_fwd = tabs.pos + 1 < tabs.history.len();

    // Visible window of lines.
    let loaded = content.read().clone().flatten();
    let editing = matches!(loaded, Some(Loaded::Text { .. }));
    let dirty = super::editor::DIRTY();

    let sid_tabs = session_id.clone();
    let sid_nav = session_id.clone();
    let sid_nav2 = session_id.clone();
    let active_for_menu = active.clone();

    rsx! {
        style { {CSS} }
        div { class: "fv",
            // ---- tab strip
            div { class: "fv-tabs",
                for f in tabs.files.iter() {
                    {
                        let name = file_name(f);
                        let relp = rel(Some(&root_dir), f);
                        let is_active = active.as_deref() == Some(f.as_str());
                        let preview = tabs.preview.as_deref() == Some(f.as_str());
                        let (f1, f2, f3) = (f.clone(), f.clone(), f.clone());
                        let (s1, s2, s3) = (sid_tabs.clone(), sid_tabs.clone(), sid_tabs.clone());
                        let mut cls = match (is_active, preview) {
                            (true, true) => "fv-tab on preview",
                            (true, false) => "fv-tab on",
                            (false, true) => "fv-tab preview",
                            (false, false) => "fv-tab",
                        }
                        .to_string();
                        if dirty.contains(f) {
                            cls.push_str(" dirty");
                        }
                        rsx! {
                            div {
                                key: "{f}",
                                class: "{cls}",
                                title: if preview { "{relp}\nPreview — double-click to keep open" } else { "{relp}" },
                                "data-path": "{relp}",
                                onclick: move |_| {
                                    open_tab(&s1, &f1, false);
                                    quick_open.set(false);
                                    rev += 1;
                                },
                                ondoubleclick: move |_| {
                                    keep_tab(&s3, &f3);
                                    rev += 1;
                                },
                                onmousedown: move |e| {
                                    // Middle click closes, like an editor.
                                    if e.trigger_button() == Some(dioxus::html::input_data::MouseButton::Auxiliary) {
                                        close_tab(&s2, &f2);
                                        rev += 1;
                                    }
                                },
                                FileIcon { name: name.clone() }
                                span { class: "fv-tab-name", "{name}" }
                                button {
                                    class: "fv-x",
                                    title: if dirty.contains(f) { "Close (unsaved changes stay until you close Larrier)" } else { "Close" },
                                    onclick: {
                                        let (s, f) = (sid_tabs.clone(), f.clone());
                                        move |e: MouseEvent| {
                                            e.stop_propagation();
                                            close_tab(&s, &f);
                                            rev += 1;
                                        }
                                    },
                                    Lucide { name: "x".to_string(), size: 12 }
                                }
                            }
                        }
                    }
                }
                button { class: "fv-btn", title: "Open a file (quick open)",
                    onclick: move |_| {
                        quick_open.set(true);
                        hl.set(0);
                        document::eval("setTimeout(() => document.getElementById('fv-quick')?.focus(), 20)");
                    },
                    Lucide { name: "plus".to_string(), size: 15, stroke: 1.5 }
                }
                div { class: "fv-grow" }
            }
            // ---- breadcrumb bar
            div { class: "fv-crumbs",
                button { class: "fv-btn", title: "Back", disabled: !can_back,
                    onclick: move |_| { go_history(&sid_nav, -1); rev += 1; },
                    Lucide { name: "arrow-left".to_string(), size: 14, stroke: 1.5 }
                }
                button { class: "fv-btn", title: "Forward", disabled: !can_fwd,
                    onclick: move |_| { go_history(&sid_nav2, 1); rev += 1; },
                    Lucide { name: "arrow-right".to_string(), size: 14, stroke: 1.5 }
                }
                div { class: "fv-path",
                    for (i, c) in crumbs.iter().enumerate() {
                        span { key: "{i}", class: "fv-crumb-item",
                            if i > 0 { span { class: "fv-sep", Lucide { name: "chevron-right".to_string(), size: 11 } } }
                            span { class: if i + 1 == crumbs.len() { "fv-crumb last" } else { "fv-crumb" }, "{c}" }
                        }
                    }
                }
                if let Some(Loaded::Text { lang: Some(l), .. }) = &loaded {
                    span { class: "fv-lang", "{l}" }
                }
                if editing {
                    {
                        let unsaved = active.as_ref().is_some_and(|a| dirty.contains(a));
                        rsx! {
                            button { class: if unsaved { "fv-btn fv-save on" } else { "fv-btn fv-save" }, title: "Save (Ctrl+S)", disabled: !unsaved,
                                onclick: move |_| super::editor::save(),
                                Lucide { name: "save".to_string(), size: 14, stroke: 1.5 }
                            }
                            button { class: if super::editor::wrap() { "fv-btn on" } else { "fv-btn" }, title: "Wrap long lines",
                                onclick: move |_| super::editor::set_wrap(!super::editor::wrap()),
                                Lucide { name: "wrap-text".to_string(), size: 14, stroke: 1.5 }
                            }
                            button { class: if super::editor::minimap() { "fv-btn on" } else { "fv-btn" }, title: "Minimap",
                                onclick: move |_| super::editor::set_minimap(!super::editor::minimap()),
                                Lucide { name: "map".to_string(), size: 14, stroke: 1.5 }
                            }
                        }
                    }
                }
                button { class: "fv-btn", title: "Quick open (search files)",
                    onclick: move |_| {
                        quick_open.toggle();
                        hl.set(0);
                        document::eval("setTimeout(() => document.getElementById('fv-quick')?.focus(), 20)");
                    },
                    Lucide { name: "search".to_string(), size: 14, stroke: 1.5 }
                }
                button { class: if tree_open() { "fv-btn on" } else { "fv-btn" }, title: "Toggle file tree",
                    onclick: move |_| tree_open.toggle(),
                    Lucide { name: "list-tree".to_string(), size: 14, stroke: 1.5 }
                }
                if let Some(a) = active_for_menu {
                    button { class: "fv-btn", title: "Open with the system editor",
                        onclick: move |_| { let _ = open::that(&a); },
                        Lucide { name: "external-link".to_string(), size: 14, stroke: 1.5 }
                    }
                }
            }
            div { class: "fv-body",
                // ---- tree
                if tree_open() {
                    div { class: "fv-tree",
                        TreeLevel {
                            dir: root_dir.clone(),
                            root: root_dir.clone(),
                            depth: 0,
                            expanded,
                            dirs,
                            active: active.clone(),
                            on_keep: {
                                let sid = session_id.clone();
                                move |p: String| {
                                    open_tab(&sid, &p, true);
                                    rev += 1;
                                }
                            },
                            on_open: {
                                let sid = session_id.clone();
                                move |p: String| {
                                    open_tab(&sid, &p, false);
                                    quick_open.set(false);
                                    rev += 1;
                                }
                            },
                        }
                    }
                }
                // ---- main
                div { class: "fv-main",
                    if quick_open() || active.is_none() {
                        div { class: if active.is_none() { "fv-quick centered" } else { "fv-quick" },
                            if active.is_none() {
                                div { class: "fv-quick-title", "Open a file" }
                            }
                            div { class: "fv-quick-input",
                                Lucide { name: "search".to_string(), size: 14 }
                                input {
                                    id: "fv-quick",
                                    autofocus: true,
                                    placeholder: if indexing { "Indexing files…" } else { "Search files by name" },
                                    value: "{quick}",
                                    oninput: move |e| { quick.set(e.value()); hl.set(0); },
                                    onkeydown: move |e| match e.key() {
                                        Key::ArrowDown => { e.prevent_default(); hl.set((sel + 1).min(results_for_keys.len().saturating_sub(1))); }
                                        Key::ArrowUp => { e.prevent_default(); hl.set(sel.saturating_sub(1)); }
                                        Key::Enter => {
                                            if let Some(p) = results_for_keys.get(sel).cloned() {
                                                let mut open = open_first.clone();
                                                open(p);
                                            }
                                        }
                                        Key::Escape => quick_open.set(false),
                                        _ => {}
                                    },
                                }
                            }
                            div { class: "fv-results",
                                for (i, p) in results.iter().enumerate() {
                                    {
                                        let name = file_name(p);
                                        let dir = p.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
                                        let p2 = p.clone();
                                        let mut open = open_rel.clone();
                                        rsx! {
                                            div {
                                                key: "{p}",
                                                class: if i == sel { "fv-result on" } else { "fv-result" },
                                                "data-path": "{p}",
                                                onmouseenter: move |_| hl.set(i),
                                                onclick: move |_| open(p2.clone()),
                                                FileIcon { name: name.clone() }
                                                span { class: "fv-r-name", "{name}" }
                                                span { class: "fv-r-dir", "{dir}" }
                                            }
                                        }
                                    }
                                }
                                if !indexing && results.is_empty() {
                                    div { class: "fv-note", "No matching files" }
                                }
                            }
                        }
                    }
                    if active.is_some() && !quick_open() {
                        match loaded.clone() {
                            None => rsx! { div { class: "fv-note", "Loading…" } },
                            Some(Loaded::Missing(e)) => rsx! { div { class: "fv-note", "Can't open this file: {e}" } },
                            Some(Loaded::Binary { bytes }) => rsx! { div { class: "fv-note", "Binary file · {human_bytes(bytes)}" } },
                            Some(Loaded::TooLarge { bytes }) => rsx! { div { class: "fv-note", "File is too large to preview · {human_bytes(bytes)}" } },
                            Some(Loaded::Image { url, bytes }) => rsx! {
                                div { class: "fv-image",
                                    img { src: "{url}" }
                                    div { class: "fv-note", "{human_bytes(bytes)}" }
                                }
                            },
                            // The file this text is of (not the tab just
                            // picked, whose text may still be on its way).
                            Some(Loaded::Text { path, text, .. }) => rsx! {
                                super::editor::CodeEditor { path, text }
                            },
                        }
                    }
                }
            }
        }
    }
}

/// One directory level of the lazy file tree.
#[component]
fn TreeLevel(
    dir: String,
    root: String,
    depth: usize,
    expanded: Signal<BTreeSet<String>>,
    dirs: Signal<HashMap<String, Vec<Entry>>>,
    active: Option<String>,
    on_open: EventHandler<String>,
    on_keep: EventHandler<String>,
) -> Element {
    let mut expanded = expanded;
    let mut dirs = dirs;
    let entries = dirs.read().get(&dir).cloned().unwrap_or_default();
    let active_norm = active.as_deref().map(norm);
    rsx! {
        for e in entries {
            {
                let open = expanded.read().contains(&e.path);
                let relp = rel(Some(&root), &e.path);
                let is_active = active_norm.as_deref() == Some(norm(&e.path).as_str());
                let p = e.path.clone();
                let p2 = e.path.clone();
                let is_dir = e.dir;
                let pad = 8 + depth * 12;
                rsx! {
                    div {
                        key: "{e.path}",
                        class: if is_active { "fv-node on" } else { "fv-node" },
                        style: "padding-left: {pad}px;",
                        title: "{relp}",
                        "data-path": "{relp}",
                        onclick: move |_| {
                            if is_dir {
                                let now_open = !expanded.read().contains(&p);
                                if now_open {
                                    if !dirs.read().contains_key(&p) {
                                        let list = read_dir_sorted(&p);
                                        dirs.write().insert(p.clone(), list);
                                    }
                                    expanded.write().insert(p.clone());
                                } else {
                                    expanded.write().remove(&p);
                                }
                            } else {
                                on_open.call(p.clone());
                            }
                        },
                        ondoubleclick: {
                            let p = p2.clone();
                            move |_| {
                                if !is_dir {
                                    on_keep.call(p.clone());
                                }
                            }
                        },
                        if is_dir {
                            span { class: "fv-chev", Lucide { name: if open { "chevron-down".to_string() } else { "chevron-right".to_string() }, size: 12 } }
                            span { class: "fv-ic", Lucide { name: if open { "folder-open".to_string() } else { "folder".to_string() }, size: 14, stroke: 1.5 } }
                        } else {
                            span { class: "fv-chev" }
                            FileIcon { name: e.name.clone(), size: 14 }
                        }
                        span { class: "fv-node-name", "{e.name}" }
                    }
                    if is_dir && open {
                        TreeLevel { dir: e.path.clone(), root: root.clone(), depth: depth + 1, expanded, dirs, active: active.clone(), on_open, on_keep }
                    }
                }
            }
        }
    }
}

const CSS: &str = r#"
.fv { flex: 1; min-height: 0; display: flex; flex-direction: column; background: var(--pane); }
.fv-empty { flex: 1; display: flex; align-items: center; justify-content: center; color: var(--fg-3); font-size: 12px; }
.fv-tabs { display: flex; align-items: center; gap: 2px; height: 38px; padding: 0 8px; border-bottom: 1px solid var(--border); overflow-x: auto; overflow-y: hidden; flex-shrink: 0; scrollbar-width: none; }
.fv-tabs::-webkit-scrollbar { display: none; }
.fv-tab { height: 28px; display: inline-flex; align-items: center; gap: 6px; padding: 0 6px 0 10px; border-radius: 7px; font-size: 13px; color: var(--fg-2); cursor: pointer; flex-shrink: 0; max-width: 220px; user-select: none; }
.fv-tab:hover { background: var(--hover); color: var(--fg); }
.fv-tab.on { background: var(--selected); color: var(--fg); }
.fv-tab.preview .fv-tab-name { font-style: italic; }
.fv-tab-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.fv-x { width: 18px; height: 18px; border-radius: 4px; display: inline-flex; align-items: center; justify-content: center; color: var(--fg-3); opacity: 0; }
.fv-tab:hover .fv-x, .fv-tab.on .fv-x { opacity: 1; }
.fv-x:hover { background: var(--hover); color: var(--fg); }
.fv-btn { width: 26px; height: 26px; border-radius: 6px; display: inline-flex; align-items: center; justify-content: center; color: var(--fg-3); flex-shrink: 0; }
.fv-btn:hover { background: var(--hover); color: var(--fg); }
.fv-btn.on { color: var(--fg); }
.fv-btn:disabled { opacity: .35; cursor: default; background: none; }
.fv-grow { flex: 1; }
.fv-ic { display: inline-flex; color: var(--fg-3); }
.fv-crumbs { display: flex; align-items: center; gap: 2px; height: 34px; padding: 0 8px; border-bottom: 1px solid var(--border); flex-shrink: 0; min-width: 0; }
.fv-path { flex: 1; min-width: 0; display: flex; align-items: center; gap: 2px; overflow: hidden; white-space: nowrap; font-size: 12.5px; color: var(--fg-3); padding-left: 4px; }
.fv-crumb.last { color: var(--fg); }
.fv-sep { display: inline-flex; color: var(--fg-4); }
.fv-crumb-item { display: inline-flex; align-items: center; gap: 2px; }
.fv-lang { font-size: 11px; color: var(--fg-3); padding: 0 8px; white-space: nowrap; }
.fv-body { flex: 1; min-height: 0; display: flex; }
.fv-tree { width: 230px; flex-shrink: 0; overflow: auto; border-right: 1px solid var(--border); padding: 4px 0 12px; }
.fv-node { height: 24px; display: flex; align-items: center; gap: 5px; padding-right: 8px; font-size: 12.5px; color: var(--fg-2); cursor: pointer; white-space: nowrap; user-select: none; }
.fv-node:hover { background: var(--hover); color: var(--fg); }
.fv-node.on { background: var(--selected); color: var(--fg); }
.fv-chev { width: 12px; display: inline-flex; color: var(--fg-4); flex-shrink: 0; }
.fv-node-name { overflow: hidden; text-overflow: ellipsis; }
.fv-main { flex: 1; min-width: 0; display: flex; flex-direction: column; position: relative; }
.fv-quick { padding: 10px 12px; border-bottom: 1px solid var(--border); background: var(--card-2); display: flex; flex-direction: column; gap: 6px; max-height: 60%; }
.fv-quick.centered { border: none; background: none; flex: 1; max-height: none; align-items: stretch; padding: 12vh 24px 24px; }
.fv-quick-title { font-size: 15px; font-weight: 500; color: var(--fg); margin-bottom: 4px; text-align: center; }
.fv-quick-input { display: flex; align-items: center; gap: 8px; height: 34px; padding: 0 10px; border: 1px solid var(--border); border-radius: 8px; background: var(--card); color: var(--fg-3); }
.fv-quick-input:focus-within { border-color: var(--fg-4); }
.fv-quick-input input { flex: 1; border: none; outline: none; background: transparent; font-size: 13px; color: var(--fg); min-width: 0; }
.fv-results { overflow-y: auto; display: flex; flex-direction: column; gap: 1px; min-height: 0; }
.fv-result { height: 30px; display: flex; align-items: center; gap: 8px; padding: 0 8px; border-radius: 6px; font-size: 13px; cursor: pointer; min-width: 0; }
.fv-result.on { background: var(--hover); }
.fv-r-name { color: var(--fg); white-space: nowrap; }
.fv-r-dir { color: var(--fg-3); font-size: 12px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; }
.fv-note { padding: 18px; color: var(--fg-3); font-size: 12px; text-align: center; }
.fv-image { flex: 1; overflow: auto; display: flex; flex-direction: column; align-items: center; justify-content: center; padding: 24px; gap: 8px; background-image: linear-gradient(45deg, var(--hover) 25%, transparent 25%), linear-gradient(-45deg, var(--hover) 25%, transparent 25%), linear-gradient(45deg, transparent 75%, var(--hover) 75%), linear-gradient(-45deg, transparent 75%, var(--hover) 75%); background-size: 16px 16px; background-position: 0 0, 0 8px, 8px -8px, -8px 0; }
.fv-image img { max-width: 100%; max-height: 80%; object-fit: contain; box-shadow: 0 2px 10px rgba(0,0,0,.12); }
.fv-editor { flex: 1; min-height: 0; min-width: 0; position: relative; overflow: hidden; }
.fv-editor .monaco-editor { --vscode-font-family: var(--font, system-ui); }
.fv-editor .monaco-editor .suggest-widget, .fv-editor .monaco-editor .monaco-hover, .monaco-editor .suggest-widget, .monaco-editor .monaco-hover { border-radius: 8px; overflow: hidden; }
.fv-tab.dirty .fv-x { opacity: 1; }
.fv-tab.dirty .fv-x svg { display: none; }
.fv-tab.dirty .fv-x::before { content: ""; width: 7px; height: 7px; border-radius: 50%; background: var(--fg-2); }
.fv-tab.dirty .fv-x:hover svg { display: block; }
.fv-tab.dirty .fv-x:hover::before { display: none; }
.fv-save.on { color: var(--accent-c, var(--blue)); }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_fuzzy_prefers_file_names() {
        let a = fuzzy("main", "src/main.rs").unwrap();
        let b = fuzzy("main", "src/domain/thing.rs").unwrap_or(i64::MIN);
        assert!(a > b);
        assert!(fuzzy("xyz", "src/main.rs").is_none());
        assert_eq!(fuzzy("", "anything"), Some(0));
    }

    #[test]
    fn files_rel_and_names() {
        assert_eq!(rel(Some("C:\\repo"), "C:\\repo\\src\\a.rs"), "src/a.rs");
        assert_eq!(rel(Some("/r/"), "/r/x/y.txt"), "x/y.txt");
        assert_eq!(rel(Some("/r"), "/other/z"), "/other/z");
        assert_eq!(file_name("a/b/c.rs"), "c.rs");
        assert!(is_image("x.PNG"));
    }

    #[test]
    fn files_tabs_and_history() {
        let sid = "test-session-files";
        open_tab(sid, "/r/a.rs", true);
        open_tab(sid, "/r/b.rs", false);
        open_tab(sid, "/r/a.rs", false);
        let t = with_tabs(sid, |t| t.clone());
        assert_eq!(t.files.len(), 2);
        assert_eq!(t.active.as_deref(), Some("/r/a.rs"));
        go_history(sid, -1);
        assert_eq!(with_tabs(sid, |t| t.active.clone()).as_deref(), Some("/r/b.rs"));
        close_tab(sid, "/r/b.rs");
        assert_eq!(with_tabs(sid, |t| t.active.clone()).as_deref(), Some("/r/a.rs"));
    }

    #[test]
    fn files_preview_tab_is_replaced_until_kept() {
        let sid = "test-session-preview";
        open_tab(sid, "/r/a.rs", false);
        open_tab(sid, "/r/b.rs", false);
        assert_eq!(with_tabs(sid, |t| t.files.clone()), ["/r/b.rs"]);
        keep_tab(sid, "/r/b.rs");
        open_tab(sid, "/r/c.rs", false);
        open_tab(sid, "/r/d.rs", false);
        let t = with_tabs(sid, |t| t.clone());
        assert_eq!(t.files, ["/r/b.rs", "/r/d.rs"]);
        assert_eq!(t.preview.as_deref(), Some("/r/d.rs"));
    }

    #[test]
    fn files_load_text_binary_image() {
        let dir = std::env::temp_dir().join(format!("larrier-fv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let t = dir.join("x.rs");
        std::fs::write(&t, "fn a() {}\nfn b() {}\n").unwrap();
        match load(&t.to_string_lossy()) {
            Loaded::Text { text, lang, .. } => {
                assert_eq!(text.lines().count(), 2);
                assert_eq!(lang, Some("Rust"));
            }
            other => panic!("{other:?}"),
        }
        let b = dir.join("blob.bin");
        std::fs::write(&b, [0u8, 1, 2, 3]).unwrap();
        assert!(matches!(load(&b.to_string_lossy()), Loaded::Binary { .. }));
        let i = dir.join("p.png");
        std::fs::write(&i, [137u8, 80, 78, 71]).unwrap();
        assert!(matches!(load(&i.to_string_lossy()), Loaded::Image { .. }));
        assert!(matches!(load(&dir.join("nope.txt").to_string_lossy()), Loaded::Missing(_)));
        let _ = std::fs::remove_dir_all(dir);
    }
}
