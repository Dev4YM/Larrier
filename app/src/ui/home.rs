//! New agent screen: a centered composer with the repository and
//! environment pickers above it — Cursor's empty state.

use dioxus::prelude::*;

use super::composer::Composer;
use super::{DragRegion, SidebarToggle};

#[component]
pub fn Home() -> Element {
    rsx! {
        div { class: "main-head",
            SidebarToggle {}
            DragRegion {}
        }
        div { class: "home",
            div { class: "home-inner", style: "margin-top: 0; flex: 1; justify-content: center; padding-bottom: 14vh;",
                Composer { session_id: None, home: true }
            }
        }
    }
}
