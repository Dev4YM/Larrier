//! The windows Larrier shows instead of itself: the installer (the release
//! program run from wherever it was downloaded) and the uninstaller (run by
//! Settings → Apps).

use dioxus::prelude::*;

use crate::icons::{Icon, I};
use crate::install::{self, Kind, Scope};

const CSS: &str = r#"
.setup { height: 100vh; display: flex; align-items: center; justify-content: center; background: var(--win); color: var(--fg); padding: 24px; }
.setup-card { width: min(560px, 100%); display: flex; flex-direction: column; gap: 14px; }
.setup-head { display: flex; align-items: center; gap: 14px; }
.setup-mark { width: 52px; height: 52px; flex: none; }
.setup-mark svg { width: 100%; height: 100%; }
.setup h1 { font-size: 20px; line-height: 26px; font-weight: 600; margin: 0; }
.setup .sub { color: var(--fg-3); font-size: 12.5px; }
.setup-note { font-size: 12.5px; line-height: 18px; color: var(--fg-2); background: var(--chip); border-radius: 8px; padding: 8px 10px; }
.setup-opts { display: flex; flex-direction: column; gap: 8px; }
.setup-opt { display: flex; gap: 12px; align-items: flex-start; text-align: left; width: 100%; padding: 12px 14px; border-radius: 10px;
  border: 1px solid var(--border); background: var(--card); cursor: pointer; transition: border-color .15s, background .15s; }
.setup-opt:hover { border-color: var(--border-2); }
.setup-opt.on { border-color: var(--accent-c); background: color-mix(in srgb, var(--accent-c) 6%, var(--card)); }
.setup-opt .ic { display: inline-flex; color: var(--fg-2); margin-top: 1px; }
.setup-opt .t { font-weight: 600; font-size: 13.5px; }
.setup-opt .d { font-size: 12.5px; line-height: 18px; color: var(--fg-3); margin-top: 2px; }
.setup-opt .path { font-family: var(--mono); font-size: 11.5px; color: var(--fg-3); margin-top: 4px; word-break: break-all; }
.setup-check { display: flex; align-items: center; gap: 8px; font-size: 12.5px; color: var(--fg-2); cursor: pointer; user-select: none; }
.setup-actions { display: flex; align-items: center; gap: 8px; margin-top: 4px; }
.setup-actions .grow { flex: 1; }
.setup-actions button { height: 32px; padding: 0 14px; font-size: 13px; }
.setup-error { color: var(--red); font-size: 12.5px; line-height: 18px; white-space: pre-wrap; }
.setup-done { display: flex; align-items: center; gap: 8px; color: var(--green); font-size: 13px; }
"#;

/// Close the installer / uninstaller window.
fn quit() {
    std::process::exit(0);
}

fn theme() -> &'static str {
    match crate::state::store().load_config().settings.theme {
        larrier_core::Theme::Dark => "dark",
        _ => "light",
    }
}

#[component]
fn Frame(children: Element) -> Element {
    rsx! {
        style { {CSS} }
        div { class: "app larrier-root", "data-theme": theme(),
            div { class: "setup",
                div { class: "setup-card",
                    {children}
                }
            }
        }
    }
}

#[component]
fn Head(title: String, sub: String) -> Element {
    rsx! {
        div { class: "setup-head",
            div { class: "setup-mark", dangerous_inner_html: crate::logo::MARK_SVG }
            div {
                h1 { "{title}" }
                div { class: "sub", "{sub}" }
            }
        }
    }
}

/// Install the running program for the chosen scope, asking for
/// administrator rights when installing for everyone. Returns the installed
/// program.
fn install_now(scope: Scope, desktop: bool) -> anyhow::Result<std::path::PathBuf> {
    if scope == Scope::User || install::can_install(Scope::Machine) {
        return install::install(scope, desktop);
    }
    let me = std::env::current_exe()?;
    let mut args = vec!["--install".to_string(), "machine".to_string()];
    if desktop {
        args.push("--desktop".into());
    }
    let code = install::run_elevated(&me, &args)?;
    if code != 0 {
        let why = std::fs::read_to_string(crate::setup_log()).unwrap_or_default();
        anyhow::bail!("Installing for everyone failed{}", if why.trim().is_empty() { String::new() } else { format!(": {}", why.trim()) });
    }
    Ok(install::layout(Scope::Machine).exe())
}

/// The installer: for me or for everyone, a Desktop shortcut, or run as is.
#[component]
pub fn SetupRoot() -> Element {
    let mut scope = use_signal(|| Scope::User);
    let mut desktop = use_signal(|| true);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let existing = use_hook(install::existing);
    let user = install::layout(Scope::User);
    let machine = install::layout(Scope::Machine);
    let version = larrier_core::APP_VERSION;
    let opt = |s: Scope, on: Scope| if s == on { "setup-opt on" } else { "setup-opt" };
    let go = move |_| {
        busy.set(true);
        error.set(None);
        let (s, d) = (scope(), desktop());
        spawn(async move {
            match tokio::task::spawn_blocking(move || install_now(s, d)).await {
                Ok(Ok(exe)) => {
                    let _ = install::launch(&exe, &[]);
                    std::process::exit(0);
                }
                Ok(Err(e)) => error.set(Some(format!("{e:#}"))),
                Err(e) => error.set(Some(e.to_string())),
            }
            busy.set(false);
        });
    };
    rsx! {
        Frame {
            Head { title: "Install Larrier".to_string(), sub: format!("Version {version} · one window for every coding agent") }
            if let Some(r) = existing {
                div { class: "setup-note",
                    {format!("Larrier {} is installed {}. Installing replaces it; your sessions and settings stay.", r.version, if r.scope == Scope::Machine { "for everyone" } else { "for you" })}
                }
            }
            div { class: "setup-opts",
                button { class: opt(Scope::User, scope()), onclick: move |_| scope.set(Scope::User),
                    span { class: "ic", Icon { i: I::User, size: 18, stroke: 1.6 } }
                    div {
                        div { class: "t", "Just for me" }
                        div { class: "d", "No administrator rights needed. The app and your data stay in your user folder." }
                        div { class: "path", "{user.bin_dir.display()}" }
                    }
                }
                button { class: opt(Scope::Machine, scope()), onclick: move |_| scope.set(Scope::Machine),
                    span { class: "ic", Icon { i: I::Laptop, size: 18, stroke: 1.6 } }
                    div {
                        div { class: "t", "For everyone on this computer" }
                        div { class: "d", "Needs administrator rights. The app goes to Program Files; each person's sessions stay in their own profile." }
                        div { class: "path", "{machine.bin_dir.display()}" }
                    }
                }
            }
            label { class: "setup-check",
                input { r#type: "checkbox", checked: desktop(), onchange: move |e| desktop.set(e.checked()) }
                "Add a Desktop shortcut"
            }
            if let Some(e) = error() {
                div { class: "setup-error", "{e}" }
            }
            div { class: "setup-actions",
                button { class: "ghost-btn", disabled: busy(),
                    title: "Run this copy as it is, without installing",
                    onclick: move |_| {
                        if let Ok(me) = std::env::current_exe() {
                            if install::launch(&me, &["--portable"]).is_ok() {
                                std::process::exit(0);
                            }
                        }
                    },
                    "Run without installing"
                }
                div { class: "grow" }
                button { class: "primary-btn", disabled: busy(), onclick: go,
                    if busy() { super::loaders::Orbit { size: 13 } " Installing…" } else { "Install" }
                }
            }
        }
    }
}

/// The uninstaller: remove Larrier, and optionally this user's data.
#[component]
pub fn UninstallRoot() -> Element {
    let mut remove_data = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let mut done = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let kind = install::current();
    let data = larrier_core::store::data_root();
    let go = move |_| {
        busy.set(true);
        error.set(None);
        let rm = remove_data();
        spawn(async move {
            let r = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                let Kind::Installed(scope) = install::current() else { anyhow::bail!("This copy of Larrier isn't installed") };
                // This user's data goes from this (unelevated) process: an
                // administrator's window may run as someone else.
                if rm {
                    let _ = std::fs::remove_dir_all(larrier_core::store::data_root());
                }
                if scope == Scope::Machine && !install::can_install(Scope::Machine) {
                    let me = std::env::current_exe()?;
                    let code = install::run_elevated(&me, &["--uninstall".into(), "--confirmed".into()])?;
                    anyhow::ensure!(code == 0, "Uninstalling failed (code {code})");
                    Ok(())
                } else {
                    install::uninstall(scope, false)
                }
            })
            .await;
            match r {
                Ok(Ok(())) => {
                    done.set(true);
                    tokio::time::sleep(std::time::Duration::from_millis(1400)).await;
                    std::process::exit(0);
                }
                Ok(Err(e)) => error.set(Some(format!("{e:#}"))),
                Err(e) => error.set(Some(e.to_string())),
            }
            busy.set(false);
        });
    };
    let what = match kind {
        Kind::Installed(Scope::Machine) => "Larrier will be removed for everyone on this computer.",
        Kind::Installed(Scope::User) => "Larrier will be removed from your user account.",
        _ => "This copy of Larrier isn't installed.",
    };
    rsx! {
        Frame {
            Head { title: "Uninstall Larrier".to_string(), sub: format!("Version {}", larrier_core::APP_VERSION) }
            div { class: "setup-note", "{what}" }
            label { class: "setup-check",
                input { r#type: "checkbox", checked: remove_data(), onchange: move |e| remove_data.set(e.checked()) }
                "Also delete my sessions and settings ({data.display()})"
            }
            if let Some(e) = error() {
                div { class: "setup-error", "{e}" }
            }
            if done() {
                div { class: "setup-done", Icon { i: I::CircleCheck, size: 15 } "Larrier was uninstalled." }
            }
            div { class: "setup-actions",
                div { class: "grow" }
                button { class: "ghost-btn", disabled: busy(), onclick: move |_| quit(), "Cancel" }
                button { class: "danger-btn", disabled: busy() || done() || !matches!(kind, Kind::Installed(_)), onclick: go,
                    if busy() { "Uninstalling…" } else { "Uninstall" }
                }
            }
        }
    }
}
