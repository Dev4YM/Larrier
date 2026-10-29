//! Larrier — one window for every coding agent and AI chat.
#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]

mod automations;
mod highlight;
mod icons;
mod live;
mod logo;
mod logos;
mod markdown;
mod models_sync;
mod notify;
mod quota;
mod state;
mod ui;

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

fn main() {
    scrub_host_session_env();
    let filter = std::env::var("LARRIER_LOG").unwrap_or_else(|_| "info,larrier=debug".into());
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();

    let root = state::store().root().to_path_buf();
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

    dioxus::LaunchBuilder::desktop().with_cfg(cfg).launch(ui::Root);
}

/// The Larrier mark as the window icon, drawn at runtime (no image files).
fn app_icon() -> dioxus::desktop::tao::window::Icon {
    const N: u32 = 64;
    dioxus::desktop::tao::window::Icon::from_rgba(logo::mark_rgba(N), N, N).expect("valid icon")
}
