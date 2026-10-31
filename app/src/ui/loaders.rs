//! Larrier's loaders, one set for the whole app:
//!
//! - [`Dots`]: a tool at work (a command running, a file being written, a
//!   terminal executing).
//! - [`Stamp`]: a subagent working in the background.
//! - [`Orbit`]: a chat at work (sidebar, tabs), and anything loading.
//!
//! With Reduce Motion they hold still.

use dioxus::prelude::*;

/// Six dots (a braille ⠿) lit one after another.
#[component]
pub fn Dots(#[props(default = 12)] size: u32) -> Element {
    // Clockwise around the 2×3 grid, so the light travels in a loop.
    const AT: [(u8, u8); 6] = [(2, 2), (6, 2), (6, 6), (6, 10), (2, 10), (2, 6)];
    let w = size * 2 / 3;
    let dots = AT.iter().enumerate().map(|(k, (x, y))| (*x, *y, format!("animation-delay: {:.1}s", k as f32 * 0.2 - 1.2)));
    rsx! {
        svg { class: "tk-dots", width: "{w}", height: "{size}", view_box: "0 0 8 12", role: "img", "aria-label": "Running",
            for (x, y, delay) in dots {
                circle { key: "{x}-{y}", cx: "{x}", cy: "{y}", r: "1.35", style: "{delay}" }
            }
        }
    }
}

/// Ink Orbit: two ink strokes circling, easing as they go.
#[component]
pub fn Orbit(#[props(default = 14)] size: u32) -> Element {
    rsx! {
        svg { class: "ink-loader ink-orbit", width: "{size}", height: "{size}", view_box: "6 6 36 36", role: "img", "aria-label": "Working",
            g { class: "ink-orbit-ring",
                path { d: "M12 15 C17 8 26 7 32 11 L29 14 C24 11 18 13 15 19 Z" }
                path { d: "M36 33 C31 40 22 42 15 37 L18 34 C23 37 30 36 33 29 Z" }
            }
        }
    }
}

/// Seal Stamp: two corners pressing together, the seal's dot showing.
#[component]
pub fn Stamp(#[props(default = 14)] size: u32) -> Element {
    rsx! {
        svg { class: "ink-loader ink-stamp", width: "{size}", height: "{size}", view_box: "6 6 36 36", role: "img", "aria-label": "Working in the background",
            path { class: "ink-stamp-a", d: "M11 27 L11 13 Q11 10 14 10 L27 10 L26 14 L15 15 L15 27 Z" }
            path { class: "ink-stamp-b", d: "M37 21 L37 35 Q37 38 34 38 L21 38 L22 34 L33 33 L33 21 Z" }
            circle { class: "ink-stamp-dot", cx: "24", cy: "24", r: "2.4" }
        }
    }
}
