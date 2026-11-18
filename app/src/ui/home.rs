//! New agent screen: a centered composer with the project and
//! environment pickers above it.

use dioxus::prelude::*;

use crate::icons::{Icon, I};
use crate::state::use_app;

use super::chat::{DropZones, TabStrip};
use super::composer::Composer;
use super::{DragRegion, SidebarToggle};

#[component]
pub fn Home() -> Element {
    let mut app = use_app();
    // The picture behind it (Settings → Appearance → Home Background).
    let bg = app.config.read().settings.home_bg.clone();
    let has_bg = !bg.image.is_empty();
    let vars = super::home_bg::input_style(&bg);
    // The last chat, not opened again at start: which, and why.
    let issue = app.ui.read().startup_issue.clone().map(|(id, why)| {
        let title = app.sessions.read().iter().find(|s| s.id == id).map(|s| s.title.clone()).unwrap_or_else(|| "your last chat".into());
        (id, title, why)
    });
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
                if let Some((id, title, why)) = issue {
                    div { class: "home-issue", title: "{why}",
                        span { class: "ic", Icon { i: I::Alert, size: 13 } }
                        span { class: "tx", "Didn't reopen “{title}”: {why}" }
                        button { class: "ghost-btn sm", onclick: move |_| { app.ui.write().startup_issue = None; app.open_session(&id); }, "Open it" }
                        button { class: "icon-btn sm", title: "Dismiss", onclick: move |_| app.ui.write().startup_issue = None, Icon { i: I::X, size: 12 } }
                    }
                }
                Composer { session_id: None, home: true }
            }
        }
    }
}
