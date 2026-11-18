//! The loading screen while Lary starts: the Lary mark cut as glass, in its
//! own colours only — the ink tile in four flat facets meeting at its
//! centre and the letter in two, light slowly turning over them — with what
//! Lary is doing under it. It stays until the window is ready
//! (`Ui::booting`), then fades out.

use std::time::Duration;

use dioxus::prelude::*;

use crate::state::use_app;

/// The mark as faceted glass: the tile's four planes (light from the top),
/// a hairline rim, and the letter's two planes split along the diagonal.
const MARK: &str = r##"<svg viewBox="0 0 64 64" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
  <defs>
    <clipPath id="bm-tile"><rect width="64" height="64" rx="15"/></clipPath>
    <clipPath id="bm-near"><polygon points="0,0 64,0 0,64"/></clipPath>
    <clipPath id="bm-far"><polygon points="64,0 64,64 0,64"/></clipPath>
    <g id="bm-letter">
      <path d="M17.99 18.00 18.08 18.88 18.09 19.50 18.05 20.39 18.10 21.13 18.23 21.94 18.12 22.61 18.25 23.35 18.26 24.41 18.28 25.22 18.35 26.31 18.43 27.11 18.44 27.80 18.52 28.55 18.51 29.34 18.56 30.38 18.63 30.98 18.56 31.67 18.59 32.56 18.70 33.22 18.69 33.87 18.79 34.51 18.77 35.31 18.76 36.36 18.87 37.00 18.81 38.07 18.87 39.16 18.93 39.98 18.97 40.69 19.06 41.72 19.13 42.45 19.07 43.31 19.12 44.18 19.26 45.24 19.18 46.17 19.33 47.67 31.80 35.20 31.68 34.70 31.62 33.68 31.67 33.01 31.65 32.39 31.52 31.43 31.61 30.43 31.45 29.38 31.49 28.62 31.36 27.83 31.32 27.20 31.35 26.23 31.41 25.25 31.25 24.47 31.38 23.62 31.30 22.57 31.16 21.50 31.15 20.80 31.12 20.18 31.21 19.09 31.01 18.02 30.97 18.00 30.96 17.54 30.87 17.09 30.77 16.64 30.75 16.16 30.52 15.75 30.26 15.36 30.15 14.89 29.90 14.49 29.42 14.26 29.18 13.85 28.80 13.56 28.32 13.38 27.96 13.07 27.47 12.94 27.03 12.74 26.58 12.55 26.09 12.42 25.56 12.51 25.06 12.41 24.55 12.50 24.03 12.44 23.52 12.49 23.04 12.72 22.52 12.76 22.02 12.93 21.58 13.18 21.19 13.49 20.76 13.72 20.39 14.02 19.89 14.21 19.57 14.56 19.20 14.86 19.09 15.31 18.80 15.65 18.47 15.98 18.41 16.41 18.33 16.81 18.09 17.19 18.07 17.59 18.08 18.00Z"/>
      <path d="M26.8 48.8 36.2 39.4H47a3 3 0 0 1 3 3V48a3 3 0 0 1-3 3H25Z"/>
    </g>
  </defs>
  <g clip-path="url(#bm-tile)">
    <polygon class="bm-top" points="0,0 64,0 32,32"/>
    <polygon class="bm-left" points="0,0 32,32 0,64"/>
    <polygon class="bm-right" points="64,0 64,64 32,32"/>
    <polygon class="bm-bottom" points="0,64 32,32 64,64"/>
  </g>
  <rect x=".5" y=".5" width="63" height="63" rx="14.5" fill="none" class="bm-rim"/>
  <use href="#bm-letter" class="bm-lit" clip-path="url(#bm-near)"/>
  <use href="#bm-letter" class="bm-shade" clip-path="url(#bm-far)"/>
</svg>"##;

#[component]
pub fn Boot() -> Element {
    let app = use_app();
    let booting = app.ui.read().booting;
    let note = app.ui.read().boot_note.clone();
    // It stays a moment once the window is ready, to fade out.
    let mut shown = use_signal(|| true);
    use_effect(use_reactive!(|booting| {
        if !booting {
            spawn(async move {
                tokio::time::sleep(Duration::from_millis(460)).await;
                shown.set(false);
            });
        }
    }));
    if !shown() {
        return rsx! {};
    }
    rsx! {
        div { class: if booting { "boot" } else { "boot leaving" }, role: "status", "aria-label": "Lary is starting",
            span { class: "boot-mark", dangerous_inner_html: MARK }
            div { class: "boot-note", if note.is_empty() { "Starting Lary…" } else { "{note}" } }
            div { class: "boot-line", span {} }
        }
    }
}
