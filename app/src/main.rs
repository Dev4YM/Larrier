//! Larrier — one window for every coding agent and AI chat.
#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]

mod agent_models;
mod automations;
mod browser_chrome;
mod browser_control;
mod browser_host;
mod highlight;
mod icons;
mod install;
mod keep;
mod live;
mod logo;
mod logos;
mod markdown;
mod models_sync;
mod notify;
mod quota;
mod spell;
mod state;
mod ui;
mod update;
mod version;

use dioxus::desktop::{Config, LogicalSize, WindowBuilder};


const CSS: &str = include_str!("styles.css");

/// When Larrier is started from inside another agent's session (e.g. a
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
    std::env::temp_dir().join("larrier-setup.log")
}

/// Steps that run without a window (installing, updating, uninstalling,
/// often with administrator rights). `None` starts a window.
fn headless(args: &[String]) -> Option<i32> {
    let has = |f: &str| args.iter().any(|a| a == f);
    let val = |f: &str| args.iter().position(|a| a == f).and_then(|i| args.get(i + 1)).cloned();
    if has("--version") || has("-V") {
        println!("Larrier {}", version::long());
        return Some(0);
    }
    let result: anyhow::Result<()> = if let Some(scope) = val("--install") {
        let scope = if scope == "machine" { install::Scope::Machine } else { install::Scope::User };
        install::install(scope, has("--desktop")).and_then(|exe| if has("--launch") { install::launch(&exe, &[]) } else { Ok(()) })
    } else if let Some(target) = val("--apply-update") {
        update::apply(std::path::Path::new(&target), val("--pid").and_then(|p| p.parse().ok()), !has("--no-launch"))
    } else if let Some(target) = val("--replace") {
        update::replace(std::path::Path::new(&target))
    } else if has("--uninstall") && (has("--quiet") || has("--confirmed")) {
        match install::current() {
            install::Kind::Installed(scope) if scope == install::Scope::User || install::can_install(scope) => install::uninstall(scope, has("--remove-data")),
            install::Kind::Installed(_) => std::env::current_exe().map_err(Into::into).and_then(|me| {
                let code = install::run_elevated(&me, &["--uninstall".into(), "--confirmed".into()])?;
                anyhow::ensure!(code == 0, "uninstall failed (code {code})");
                Ok(())
            }),
            _ => Err(anyhow::anyhow!("this copy of Larrier isn't installed")),
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

/// Which window to open.
#[derive(Clone, Copy, PartialEq)]
enum Start {
    App,
    /// A downloaded program run from outside an install: offer to install.
    Setup,
    Uninstall,
}

fn main() {
    scrub_host_session_env();
    let args: Vec<String> = std::env::args().skip(1).collect();
    // An agent's browser tools: an MCP server on stdio (before any logging,
    // which would write into its replies).
    if args.first().map(String::as_str) == Some("mcp-browser") {
        std::process::exit(larrier_agents::browser::mcp::serve());
    }
    let filter = std::env::var("LARRIER_LOG").unwrap_or_else(|_| "info,larrier=debug".into());
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();

    if let Some(code) = headless(&args) {
        std::process::exit(code);
    }
    install::tidy();
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
        larrier_agents::browser::register(&exe);
    }
    let dark = state::store().load_config().settings.theme == larrier_core::Theme::Dark;
    let bg = if dark { (26, 26, 24, 255) } else { (247, 247, 244, 255) };

    #[allow(unused_mut)]
    let mut window = crate::notify::apply_to_builder(
        WindowBuilder::new()
            .with_title("Larrier")
            .with_inner_size(LogicalSize::new(1440.0, 900.0))
            .with_min_inner_size(LogicalSize::new(560.0, 420.0)),
    )
    .with_decorations(false);
    #[cfg(windows)]
    {
        use dioxus::desktop::tao::platform::windows::WindowBuilderExtWindows;
        window = window.with_undecorated_shadow(true);
    }
    if start != Start::App {
        // The installer and uninstaller: a small window with the system's frame.
        window = WindowBuilder::new()
            .with_title(if start == Start::Setup { "Install Larrier" } else { "Uninstall Larrier" })
            .with_inner_size(LogicalSize::new(640.0, if start == Start::Setup { 560.0 } else { 360.0 }))
            .with_resizable(false);
    }

    let cfg = Config::new()
        .with_window(window.with_window_icon(Some(app_icon())))
        .with_icon(app_icon())
        .with_menu(None)
        .with_disable_context_menu(true)
        .with_background_color(bg)
        .with_data_directory(root.join("webview"))
        .with_custom_head(format!(
            "<meta name=\"color-scheme\" content=\"light dark\"><link rel=\"icon\" href=\"data:,\"><title>Larrier</title><style>{CSS}</style>"
        ))
        .with_windows_browser_args(match std::env::var("LARRIER_DEVTOOLS_PORT") {
            // Lets automated UI tests drive the webview over the DevTools protocol.
            Ok(port) if !port.is_empty() => format!("--remote-debugging-port={port}"),
            _ => String::new(),
        })
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

/// The Larrier mark as the window icon, drawn at runtime (no image files).
fn app_icon() -> dioxus::desktop::tao::window::Icon {
    const N: u32 = 64;
    dioxus::desktop::tao::window::Icon::from_rgba(logo::mark_rgba(N), N, N).expect("valid icon")
}
