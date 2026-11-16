//! New agent screen: a centered composer with the project and
//! environment pickers above it.

use dioxus::prelude::*;

use crate::state::use_app;

use super::chat::{DropZones, TabStrip};
use super::composer::Composer;
use super::{DragRegion, SidebarToggle};

#[component]
pub fn Home() -> Element {
    // The picture behind it (Settings → Appearance → Home Background).
    let bg = use_app().config.read().settings.home_bg.clone();
    let has_bg = !bg.image.is_empty();
    let vars = super::home_bg::input_style(&bg);
    rsx! {
        // From the very top, behind the tabs too: nothing cuts it off.
        super::home_bg::HomeBackdrop { bg, id: "home" }
        div { class: "main-head",
            SidebarToggle {}
            TabStrip {}
            DragRegion {}
        }
        DropZones { target: None }
        div { class: if has_bg { "home has-bg" } else { "home" }, style: "{vars}",
            div { class: "home-inner", style: "margin-top: 0; flex: 1; justify-content: center; padding-bottom: 14vh;",
                Composer { session_id: None, home: true }
            }
        }
    }
}
