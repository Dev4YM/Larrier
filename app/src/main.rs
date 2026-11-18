//! Lary — one window for every coding agent and AI chat.
#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]

mod agent_models;
mod agent_versions;
mod automations;
mod browser_cef;
mod browser_control;
mod browser_host;
mod fonts;
mod highlight;
mod icons;
mod install;
mod keep;
mod live;
mod logo;
mod logos;
mod markdown;
mod models_sync;
mod native_sessions;
mod notify;
mod platform;
mod quota;
mod spell;
mod state;
mod ui;
mod update;
mod version;

use dioxus::desktop::{Config, LogicalSize, WindowBuilder};


const CSS: &str = include_str!("styles.css");

/// When Lary is started from inside another agent's session (e.g. a
/// terminal in Claude Code or Claude Desktop), that host's session markers
/// leak into our environment and make child agents believe they are nested
/// sessions that receive credentials from a host — they then report "not
/// logged in". Drop those markers before any agent is spawned.
fn scrub_host_session_env() {
    let nested = ["CLAUDECODE", "CLAUDE_CODE_HOST_SESSION_ID", "CLAUDE_CODE_CHILD_SESSION", "CLAUDE_AGENT_SDK_VERSION"]
        .iter()
        .any(|k| std::env::var_os(k).is_some());
    if !nested {
        return;
    }
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy().to_string();
        let host_marker = k.starts_with("CLAUDE_CODE_")
            || k.starts_with("CLAUDE_AGENT_SDK_")
            || matches!(k.as_str(), "CLAUDECODE" | "CLAUDE_PID" | "CLAUDE_EFFORT" | "AI_AGENT" | "BAGGAGE" | "CLAUDE_PREVIEW_CLASSIFIER_FLOOR");
        if host_marker {
            // Single-threaded at this point in main.
            std::env::remove_var(&k);
        }
    }
}

/// Where installing, updating and uninstalling note what went wrong (an
/// elevated helper can only hand back an exit code).
pub fn setup_log() -> std::path::PathBuf {
    std::env::temp_dir().join("lary-setup.log")
}

/// Steps that run without a window (installing, updating, uninstalling,
/// often with administrator rights). `None` starts a window.
fn headless(args: &[String]) -> Option<i32> {
    let has = |f: &str| args.iter().any(|a| a == f);
    let val = |f: &str| args.iter().position(|a| a == f).and_then(|i| args.get(i + 1)).cloned();
    if has("--version") || has("-V") {
        println!("Lary {}", version::long());
        return Some(0);
    }
    let result: anyhow::Result<()> = if let Some(scope) = val("--install") {
        let scope = if scope == "machine" { install::Scope::Machine } else { install::Scope::User };
        install::install(scope, has("--desktop")).and_then(|exe| if has("--launch") { install::launch(&exe, &[]) } else { Ok(()) })
    } else if let Some(target) = val("--apply-update") {
        update::apply(std::path::Path::new(&target), val("--pid").and_then(|p| p.parse().ok()), !has("--no-launch"))
    } else if let Some(target) = val("--replace") {
        update::replace(std::path::Path::new(&target))
    } else if let Some(scope) = val("--replace-larrier") {
        let scope = if scope == "machine" { install::Scope::Machine } else { install::Scope::User };
        install::replace_larrier(scope).map(|_| ())
    } else if has("--uninstall") && (has("--quiet") || has("--confirmed")) {
        match install::current() {
            install::Kind::Installed(scope) if scope == install::Scope::User || install::can_install(scope) => install::uninstall(scope, has("--remove-data")),
            install::Kind::Installed(_) => std::env::current_exe().map_err(Into::into).and_then(|me| {
                let code = install::run_elevated(&me, &["--uninstall".into(), "--confirmed".into()])?;
                anyhow::ensure!(code == 0, "uninstall failed (code {code})");
                Ok(())
            }),
            _ => Err(anyhow::anyhow!("this copy of Lary isn't installed")),
        }
    } else {
        return None;
    };
    Some(match result {
        Ok(()) => 0,
        Err(e) => {
            let _ = std::fs::write(setup_log(), format!("{e:#}"));
            1
        }
    })
}

/// Move Larrier's data folder over (see `store::migrate_from_larrier`). One
/// that's still in use can't move on Windows: rather than start on an empty
/// folder and leave the chats behind, ask to close what holds it.
fn move_larriers_data() {
    loop {
        match lary_core::store::migrate_from_larrier() {
            Ok(Some(what)) => return tracing::info!("data from Larrier: {what}"),
            Ok(None) => return,
            Err(e) => {
                tracing::warn!("data from Larrier: {e:#}");
                let answer = rfd::MessageDialog::new()
                    .set_level(rfd::MessageLevel::Warning)
                    .set_title("Lary")
                    .set_description(format!(
                        "Larrier is Lary now, and your chats and settings move over when Lary first opens. Larrier's folder is still in use, so they can't move yet.\n\nClose Larrier, and any terminal or editor open in one of its worktrees, then choose OK to try again. Cancel closes Lary.\n\n{e:#}"
                    ))
                    .set_buttons(rfd::MessageButtons::OkCancel)
                    .show();
                if answer != rfd::MessageDialogResult::Ok {
                    std::process::exit(0);
                }
            }
        }
    }
}

/// Which window to open.
#[derive(Clone, Copy, PartialEq)]
enum Start {
    App,
    /// A downloaded program run from outside an install: offer to install.
    Setup,
    Uninstall,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // One of the browser engine's helper processes (renderer, GPU, …): this
    // program again, started by the engine. Nothing else of Lary's runs.
    #[cfg(windows)]
    if browser_cef::is_subprocess(&args) {
        std::process::exit(browser_cef::subprocess());
    }
    scrub_host_session_env();
    // An agent's browser tools: an MCP server on stdio (before any logging,
    // which would write into its replies).
    if args.first().map(String::as_str) == Some("mcp-browser") {
        std::process::exit(lary_agents::browser::mcp::serve());
    }
    let filter = std::env::var("LARY_LOG").unwrap_or_else(|_| "info,lary=debug".into());
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
    // Larrier is Lary now: an installed program updated in Larrier's place
    // installs itself as Lary.
    if let Some(code) = install::migrate_from_larrier(&args) {
        std::process::exit(code);
    }

    if let Some(code) = headless(&args) {
        std::process::exit(code);
    }
    // Larrier's data folder becomes Lary's when a window first opens (not in
    // the steps above, which can run while Larrier is still closing), and
    // before anything makes Lary's own folder.
    move_larriers_data();
    // macOS / Linux: the agents and their tools as a terminal finds them.
    platform::adopt_login_path();
    install::tidy();
    #[cfg(windows)]
    browser_cef::tidy();
    let start = if args.iter().any(|a| a == "--uninstall") {
        Start::Uninstall
    } else if install::current() == install::Kind::Portable && !args.iter().any(|a| a == "--portable") {
        Start::Setup
    } else {
        Start::App
    };

    let root = state::store().root().to_path_buf();
    // Agents start this program for their browser tools.
    if let Ok(exe) = std::env::current_exe() {
        lary_agents::browser::register(&exe);
    }
    let dark = state::store().load_config().settings.theme == lary_core::Theme::Dark;
    let bg = if dark { (26, 26, 24, 255) } else { (247, 247, 244, 255) };

    #[allow(unused_mut)]
    let mut window = crate::notify::apply_to_builder(
        WindowBuilder::new()
            .with_title("Lary")
            .with_inner_size(LogicalSize::new(1440.0, 900.0))
            .with_min_inner_size(LogicalSize::new(560.0, 420.0)),
    );
    // Windows: no system frame (Lary draws the bar and its buttons).
    // macOS: the traffic lights over Lary's own top bar. Linux: the
    // system's frame, which every window manager can move and resize.
    #[cfg(windows)]
    {
        use dioxus::desktop::tao::platform::windows::WindowBuilderExtWindows;
        window = window.with_decorations(false).with_undecorated_shadow(true);
    }
    #[cfg(target_os = "macos")]
    {
        use dioxus::desktop::tao::platform::macos::WindowBuilderExtMacOS;
        window = window.with_titlebar_transparent(true).with_fullsize_content_view(true).with_title_hidden(true);
    }
    if start != Start::App {
        // The installer and uninstaller: a small window with the system's frame.
        window = WindowBuilder::new()
            .with_title(if start == Start::Setup { "Install Lary" } else { "Uninstall Lary" })
            .with_inner_size(LogicalSize::new(640.0, if start == Start::Setup { 560.0 } else { 360.0 }))
            .with_resizable(false);
    }

    let cfg = Config::new()
        .with_window(window.with_window_icon(Some(app_icon())))
        .with_icon(app_icon())
        // macOS keeps the app's commands (and Edit's, which copy and paste
        // need there) in the system menu bar; elsewhere Lary draws its own.
        .with_menu(if platform::SYSTEM_MENU_BAR && start == Start::App { Some(ui::menubar::system_menu()) } else { None })
        .with_disable_context_menu(true)
        .with_background_color(bg)
        .with_data_directory(root.join("webview"))
        .with_custom_head(format!(
            "<meta name=\"color-scheme\" content=\"light dark\"><link rel=\"icon\" href=\"data:,\"><title>Lary</title><style>{}</style><style>{CSS}</style>",
            fonts::faces_css()
        ))
        .with_windows_browser_args(match std::env::var("LARY_DEVTOOLS_PORT") {
            // Lets automated UI tests drive the webview over the DevTools protocol.
            Ok(port) if !port.is_empty() => format!("--remote-debugging-port={port}"),
            _ => String::new(),
        })
        // The code editor's files (Monaco), served from the program.
        .with_custom_protocol("lary", |_, req| ui::editor::serve(req))
        .with_navigation_handler(|url: &str| {
            let internal = url.contains("dioxus.") || url.starts_with("about:") || url.starts_with("data:");
            if !internal && (url.starts_with("http://") || url.starts_with("https://")) {
                let _ = open::that(url);
                return false;
            }
            true
        });

    match start {
        Start::App => dioxus::LaunchBuilder::desktop().with_cfg(cfg).launch(ui::Root),
        Start::Setup => dioxus::LaunchBuilder::desktop().with_cfg(cfg).launch(ui::setup::SetupRoot),
        Start::Uninstall => dioxus::LaunchBuilder::desktop().with_cfg(cfg).launch(ui::setup::UninstallRoot),
    }
}

/// The Lary mark as the window icon, drawn at runtime (no image files).
fn app_icon() -> dioxus::desktop::tao::window::Icon {
    const N: u32 = 64;
    dioxus::desktop::tao::window::Icon::from_rgba(logo::mark_rgba(N), N, N).expect("valid icon")
}
