//! New agent screen: a centered composer with the project and
//! environment pickers above it.

use dioxus::prelude::*;

use super::chat::{DropZones, TabStrip};
use super::composer::Composer;
use super::{DragRegion, SidebarToggle};

#[component]
pub fn Home() -> Element {
    rsx! {
        div { class: "main-head",
            SidebarToggle {}
            TabStrip {}
            DragRegion {}
        }
        DropZones { target: None }
        div { class: "home",
            div { class: "home-inner", style: "margin-top: 0; flex: 1; justify-content: center; padding-bottom: 14vh;",
                Composer { session_id: None, home: true }
            }
        }
    }
}
