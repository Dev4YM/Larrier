//! The window's menu bar: app mark, File / Edit / View / Help, drag area and
//! caption buttons, drawn by Larrier itself (the window has no system frame).

use dioxus::prelude::*;
use larrier_core::Theme;
use larrier_formats::ExportFormat;

use super::{open_tool, zoom, DragRegion, WindowControls};
use crate::state::{use_app, App, RightTab, SettingsTab, View};

/// Edit commands work on whatever is focused in the webview.
pub fn edit_command(cmd: &str) {
    let js = match cmd {
        "paste" => return paste_into_focused(),
        "selectAll" => "const el = window.__larrierCtx || document.activeElement; if (el && el.select) { el.focus(); el.select(); } else { document.execCommand('selectAll'); }".to_string(),
        other => format!("(window.__larrierCtx || document.activeElement)?.focus?.(); document.execCommand('{other}');"),
    };
    document::eval(&js);
}

/// Paste from the OS clipboard into the focused input (the webview can't read
/// the clipboard on its own without a permission prompt).
pub fn paste_into_focused() {
    let text = arboard::Clipboard::new().and_then(|mut c| c.get_text()).unwrap_or_default();
    let js = format!(
        "const el = window.__larrierCtx || document.activeElement; if (el) {{ el.focus(); document.execCommand('insertText', false, {}); }}",
        serde_json::to_string(&text).unwrap_or_default()
    );
    document::eval(&js);
}

/// The app's commands for the system menu bar (macOS), with the same ids
/// [`system_command`] reads back.
pub fn system_menu() -> dioxus::desktop::muda::Menu {
    use dioxus::desktop::muda::accelerator::{Accelerator, Code, Modifiers};
    use dioxus::desktop::muda::{Menu, MenuItem, PredefinedMenuItem as P, Submenu};
    let cmd = Modifiers::SUPER;
    let item = |id: &str, label: &str, key: Option<(Modifiers, Code)>| MenuItem::with_id(id, label, true, key.map(|(m, c)| Accelerator::new(Some(m), c)));
    let sep = P::separator;
    let menu = |title: &str, items: &[&dyn dioxus::desktop::muda::IsMenuItem]| Submenu::with_items(title, true, items).expect("menu");
    let app = menu(
        "Larrier",
        &[
            &item("about", "About Larrier", None),
            &sep(),
            &item("settings", "Settings…", Some((cmd, Code::Comma))),
            &sep(),
            &P::services(None),
            &sep(),
            &P::hide(None),
            &P::hide_others(None),
            &P::show_all(None),
            &sep(),
            &item("quit", "Quit Larrier", Some((cmd, Code::KeyQ))),
        ],
    );
    let file = menu(
        "File",
        &[
            &item("new-agent", "New Agent", Some((cmd, Code::KeyN))),
            &item("open-folder", "Open Folder…", Some((cmd, Code::KeyO))),
            &sep(),
            &item("import-all", "Import Sessions from All Agents", Some((cmd | Modifiers::SHIFT, Code::KeyI))),
            &item("import-file", "Import File…", None),
            &item("export", "Export Agent…", None),
            &sep(),
            &P::close_window(None),
        ],
    );
    let edit = menu("Edit", &[&P::undo(None), &P::redo(None), &sep(), &P::cut(None), &P::copy(None), &P::paste(None), &P::select_all(None)]);
    let view = menu(
        "View",
        &[
            &item("palette", "Command Palette", Some((cmd, Code::KeyK))),
            &sep(),
            &item("toggle-sidebar", "Toggle Sidebar", Some((cmd, Code::KeyB))),
            &item("toggle-panel", "Toggle Side Panel", Some((cmd | Modifiers::SHIFT, Code::KeyJ))),
            &sep(),
            &item("zoom-in", "Zoom In", Some((cmd, Code::Equal))),
            &item("zoom-out", "Zoom Out", Some((cmd, Code::Minus))),
            &item("zoom-reset", "Actual Size", Some((cmd, Code::Digit0))),
            &sep(),
            &P::fullscreen(None),
        ],
    );
    let window = menu("Window", &[&P::minimize(None), &P::maximize(None), &sep(), &P::bring_all_to_front(None)]);
    let help = menu(
        "Help",
        &[&item("docs", "Documentation", None), &item("spec", "Larrier Session Format", None), &item("shortcuts", "Keyboard Shortcuts", None), &sep(), &item("issue", "Report an Issue", None)],
    );
    #[cfg(target_os = "macos")]
    {
        window.set_as_windows_menu_for_nsapp();
        help.set_as_help_menu_for_nsapp();
    }
    Menu::with_items(&[&app, &file, &edit, &view, &window, &help]).expect("menu bar")
}

/// The command a system menu item stands for.
fn system_command(id: &str) -> Option<Cmd> {
    Some(match id {
        "about" => Cmd::About,
        "settings" => Cmd::Settings,
        "quit" => Cmd::Exit,
        "new-agent" => Cmd::NewAgent,
        "open-folder" => Cmd::OpenFolder,
        "import-all" => Cmd::ImportAll,
        "import-file" => Cmd::ImportFile,
        "export" => Cmd::Export,
        "palette" => Cmd::Palette,
        "toggle-sidebar" => Cmd::ToggleSidebar,
        "toggle-panel" => Cmd::TogglePanel,
        "zoom-in" => Cmd::Zoom(1),
        "zoom-out" => Cmd::Zoom(-1),
        "zoom-reset" => Cmd::Zoom(0),
        "docs" => Cmd::Docs,
        "spec" => Cmd::Spec,
        "shortcuts" => Cmd::Shortcuts,
        "issue" => Cmd::Issue,
        _ => return None,
    })
}

#[derive(Clone, Copy, PartialEq)]
enum Cmd {
    NewAgent,
    OpenFolder,
    ImportAll,
    ImportFile,
    Export,
    Settings,
    Exit,
    Edit(&'static str),
    Palette,
    ToggleSidebar,
    TogglePanel,
    Tool(RightTab),
    Theme(Theme),
    Zoom(i8),
    DevTools,
    Back,
    Forward,
    Automations,
    Customize,
    Docs,
    Spec,
    Shortcuts,
    Issue,
    About,
}

fn run(mut app: App, c: Cmd) {
    app.ui.write().menubar = None;
    match c {
        Cmd::NewAgent => app.new_agent(),
        Cmd::OpenFolder => app.pick_workspace(true),
        Cmd::ImportAll => app.sync_imports(false),
        Cmd::ImportFile => app.import_files(),
        Cmd::Export => {
            if let Some(id) = app.current_id() {
                app.export(&id, ExportFormat::LsfJsonl);
            } else {
                app.toast("Open an agent to export it");
            }
        }
        Cmd::Settings => app.open_settings(SettingsTab::General),
        Cmd::Exit => {
            app.install_on_exit();
            dioxus::desktop::window().close()
        }
        Cmd::Edit(e) => edit_command(e),
        Cmd::Palette => {
            let mut ui = app.ui.write();
            ui.palette_open = true;
            ui.palette_query.clear();
        }
        Cmd::ToggleSidebar => {
            let v = !app.ui.read().sidebar_open;
            app.ui.write().sidebar_open = v;
        }
        Cmd::TogglePanel => {
            let v = !app.ui.read().right_open;
            app.ui.write().right_open = v;
        }
        Cmd::Tool(t) => open_tool(app, t),
        Cmd::Theme(t) => {
            app.update_config(|c| c.settings.theme = t);
            app.apply_theme();
        }
        Cmd::Zoom(d) => zoom(app, d as f64 / 10.0),
        Cmd::DevTools => dioxus::desktop::window().devtool(),
        Cmd::Back => app.go(-1),
        Cmd::Forward => app.go(1),
        Cmd::Automations => app.navigate(View::Automations),
        Cmd::Customize => app.navigate(View::Customize),
        Cmd::Docs => {
            let _ = open::that("https://github.com/Dev4YM/Larrier#readme");
        }
        Cmd::Spec => {
            let _ = open::that("https://github.com/Dev4YM/Larrier/blob/main/docs/LSF.md");
        }
        Cmd::Shortcuts => app.open_settings(SettingsTab::Shortcuts),
        Cmd::Issue => {
            let _ = open::that("https://github.com/Dev4YM/Larrier/issues/new");
        }
        Cmd::About => app.open_settings(SettingsTab::About),
    }
}

type Entry = Option<(&'static str, &'static str, Cmd)>;

fn entries(menu: &str) -> Vec<Entry> {
    match menu {
        "File" => vec![
            Some(("New Agent", "Ctrl+N", Cmd::NewAgent)),
            Some(("Open Folder…", "", Cmd::OpenFolder)),
            None,
            Some(("Import Sessions from All Agents", "Ctrl+Shift+I", Cmd::ImportAll)),
            Some(("Import File…", "", Cmd::ImportFile)),
            Some(("Export Agent…", "", Cmd::Export)),
            None,
            Some(("Automations", "", Cmd::Automations)),
            Some(("Customize", "", Cmd::Customize)),
            Some(("Settings", "Ctrl+,", Cmd::Settings)),
            None,
            Some(("Exit", "Alt+F4", Cmd::Exit)),
        ],
        "Edit" => vec![
            Some(("Undo", "Ctrl+Z", Cmd::Edit("undo"))),
            Some(("Redo", "Ctrl+Y", Cmd::Edit("redo"))),
            None,
            Some(("Cut", "Ctrl+X", Cmd::Edit("cut"))),
            Some(("Copy", "Ctrl+C", Cmd::Edit("copy"))),
            Some(("Paste", "Ctrl+V", Cmd::Edit("paste"))),
            Some(("Paste as Text", "Ctrl+Shift+V", Cmd::Edit("paste"))),
            Some(("Select All", "Ctrl+A", Cmd::Edit("selectAll"))),
            None,
            Some(("Search Agents…", "Ctrl+K", Cmd::Palette)),
        ],
        "View" => vec![
            Some(("Command Palette", "Ctrl+Shift+P", Cmd::Palette)),
            None,
            Some(("Toggle Sidebar", "Ctrl+B", Cmd::ToggleSidebar)),
            Some(("Toggle Side Panel", "Ctrl+Shift+J", Cmd::TogglePanel)),
            Some(("Changes", "Ctrl+E", Cmd::Tool(RightTab::Changes))),
            Some(("Browser", "Ctrl+Shift+B", Cmd::Tool(RightTab::Browser))),
            Some(("Terminal", "Ctrl+J", Cmd::Tool(RightTab::Terminal))),
            Some(("File", "Ctrl+G", Cmd::Tool(RightTab::Files))),
            None,
            Some(("Back", "Alt+←", Cmd::Back)),
            Some(("Forward", "Alt+→", Cmd::Forward)),
            None,
            Some(("Light Theme", "", Cmd::Theme(Theme::Light))),
            Some(("Dark Theme", "", Cmd::Theme(Theme::Dark))),
            Some(("System Theme", "", Cmd::Theme(Theme::System))),
            None,
            Some(("Zoom In", "Ctrl+=", Cmd::Zoom(1))),
            Some(("Zoom Out", "Ctrl+-", Cmd::Zoom(-1))),
            Some(("Reset Zoom", "Ctrl+0", Cmd::Zoom(0))),
            Some(("Toggle Developer Tools", "", Cmd::DevTools)),
        ],
        _ => vec![
            Some(("Documentation", "", Cmd::Docs)),
            Some(("Larrier Session Format", "", Cmd::Spec)),
            Some(("Keyboard Shortcuts", "", Cmd::Shortcuts)),
            None,
            Some(("Report an Issue", "", Cmd::Issue)),
            Some(("About Larrier", "", Cmd::About)),
        ],
    }
}

#[component]
pub fn MenuBar() -> Element {
    let mut app = use_app();
    let open = app.ui.read().menubar;
    // macOS: the menus are in the system menu bar; this bar only holds the
    // traffic lights and moves the window.
    let names: &[&'static str] = if crate::platform::SYSTEM_MENU_BAR { &[] } else { &["File", "Edit", "View", "Help"] };
    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
    dioxus::desktop::use_muda_event_handler(move |e| {
        if let Some(c) = system_command(e.id.0.as_str()) {
            run(app, c);
        }
    });
    rsx! {
        div { class: if crate::platform::SYSTEM_MENU_BAR { "menubar system" } else { "menubar" },
            if !crate::platform::SYSTEM_MENU_BAR {
                div { class: "app-mark", title: "Larrier",
                    span { class: "mark ink", dangerous_inner_html: crate::logo::MARK_SVG }
                    span { class: "mark paper", dangerous_inner_html: crate::logo::MARK_LIGHT_SVG }
                }
            }
            for (i, n) in names.iter().copied().enumerate() {
                div { key: "{n}", style: "position: relative; display: flex;",
                    button {
                        class: if open == Some(n) { "mb-item open" } else { "mb-item" },
                        onclick: move |_| {
                            let cur = app.ui.read().menubar;
                            app.ui.write().menubar = if cur == Some(n) { None } else { Some(n) };
                        },
                        // Hovering across the bar while a menu is open switches menus.
                        onmouseenter: move |_| {
                            if app.ui.read().menubar.is_some() {
                                app.ui.write().menubar = Some(n);
                            }
                        },
                        "{n}"
                    }
                    if open == Some(n) {
                        div { class: "menu mb-menu", style: if i == 0 { "left: 0;" } else { "left: 0;" },
                            onclick: move |e| e.stop_propagation(),
                            for (j, e) in entries(n).into_iter().enumerate() {
                                match e {
                                    None => rsx! { div { key: "{j}", class: "menu-sep" } },
                                    Some((label, kb, cmd)) => rsx! {
                                        button { key: "{j}", class: "menu-item", onclick: move |_| run(app, cmd),
                                            span { "{label}" }
                                            if !kb.is_empty() { span { class: "kb", {crate::platform::keys(kb)} } }
                                        }
                                    },
                                }
                            }
                        }
                    }
                }
            }
            DragRegion {}
            WindowControls {}
            if open.is_some() {
                div { class: "menu-backdrop", style: "top: 30px;", onclick: move |_| app.ui.write().menubar = None }
            }
        }
    }
}
