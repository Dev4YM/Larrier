//! Larrier — one window for every coding agent and AI chat.
#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]

mod automations;
mod icons;
mod logos;
mod markdown;
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
    let mut window = WindowBuilder::new()
        .with_title("Larrier")
        .with_inner_size(LogicalSize::new(1440.0, 900.0))
        .with_min_inner_size(LogicalSize::new(880.0, 560.0))
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
            "<meta name=\"color-scheme\" content=\"light dark\"><title>Larrier</title><style>{CSS}</style>"
        ))
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

/// The Larrier mark — a dark rounded square with a light "L" — rasterized at
/// runtime so no image assets are needed.
fn app_icon() -> dioxus::desktop::tao::window::Icon {
    const N: u32 = 64;
    let mut px = vec![0u8; (N * N * 4) as usize];
    let radius = 14.0f32;
    for y in 0..N {
        for x in 0..N {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // Rounded-rect coverage with a 1px antialiased edge.
            let cx = fx.clamp(radius, N as f32 - radius);
            let cy = fy.clamp(radius, N as f32 - radius);
            let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
            let cover = (radius - d + 0.5).clamp(0.0, 1.0);
            if cover <= 0.0 {
                continue;
            }
            // "L": vertical stem + foot.
            let stem = (21.0..30.0).contains(&fx) && (15.0..49.0).contains(&fy);
            let foot = (21.0..45.0).contains(&fx) && (40.0..49.0).contains(&fy);
            let (r, g, b) = if stem || foot { (247u8, 247u8, 244u8) } else { (38u8, 37u8, 30u8) };
            let i = ((y * N + x) * 4) as usize;
            px[i] = r;
            px[i + 1] = g;
            px[i + 2] = b;
            px[i + 3] = (cover * 255.0) as u8;
        }
    }
    dioxus::desktop::tao::window::Icon::from_rgba(px, N, N).expect("valid icon")
}
