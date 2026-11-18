//! File → Import Session: one of the harnesses' sessions on this computer
//! becomes Lary's own (or a file: an LSF session, a ChatGPT or Claude.ai
//! export). Searchable; the newest first.

use dioxus::prelude::*;

use super::AgentGlyph;
use crate::native_sessions::{self, FOUND};
use crate::state::use_app;

#[component]
pub fn ImportSessionModal() -> Element {
    let mut app = use_app();
    let mut query = use_signal(String::new);
    // The session being imported, by its harness and id.
    let mut busy = use_signal(|| None::<String>);
    let found = FOUND.read().clone();
    let ql = query().trim().to_lowercase();
    let sessions = app.sessions.read().clone();
    rsx! {
        div { class: "modal-head", "Import Session" }
        div { class: "modal-body imp-body",
            input { class: "input full", autofocus: true, spellcheck: "false", placeholder: "Search the harnesses' sessions on this computer", value: "{query}", oninput: move |e| query.set(e.value()) }
            div { class: "imp-list",
                match found {
                    None => rsx! { div { class: "imp-empty", "Looking for sessions…" } },
                    Some(list) => {
                        let rows: Vec<_> = list.into_iter().filter(|i| native_sessions::matches(i, &ql)).take(200).collect();
                        if rows.is_empty() {
                            rsx! { div { class: "imp-empty", if ql.is_empty() { "No harness sessions on this computer" } else { "No sessions match" } } }
                        } else {
                            rsx! {
                                for info in rows {
                                    {
                                        let key = format!("{}:{}", info.agent.id(), info.native_id);
                                        let here = native_sessions::in_lary(&info, &sessions);
                                        let importing = busy().as_deref() == Some(key.as_str());
                                        let (k2, i2) = (key.clone(), info.clone());
                                        rsx! {
                                            button { key: "{key}", class: "imp-row", disabled: busy().is_some(),
                                                onclick: move |_| {
                                                    // Already Lary's: open it.
                                                    if let Some(id) = here.clone() {
                                                        app.ui.write().modal = None;
                                                        app.open_session(&id);
                                                        return;
                                                    }
                                                    busy.set(Some(k2.clone()));
                                                    let info = i2.clone();
                                                    spawn(async move {
                                                        if !app.import_native_session(info).await {
                                                            busy.set(None);
                                                        }
                                                    });
                                                },
                                                AgentGlyph { kind: info.agent.clone(), class: "md" }
                                                div { class: "lbl",
                                                    div { class: "t", "{native_sessions::title(&info)}" }
                                                    div { class: "d", "{native_sessions::detail(&info)}" }
                                                }
                                                if importing {
                                                    span { class: "imp-tag", "Importing…" }
                                                } else if here.is_some() {
                                                    span { class: "imp-tag", "In Lary" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        div { class: "modal-foot imp-foot",
            button { class: "ghost-btn", title: "A Lary session file, or a ChatGPT or Claude.ai export",
                onclick: move |_| {
                    app.ui.write().modal = None;
                    app.import_files();
                },
                "Choose a file…"
            }
            button { class: "ghost-btn", onclick: move |_| app.ui.write().modal = None, "Cancel" }
        }
    }
}
