//! Settings → Appearance: theme, presets, colors, fonts, layout and motion,
//! next to a live preview built from Larrier's real UI pieces so every
//! change shows the moment it is made.

use std::collections::{BTreeMap, HashMap};

use dioxus::prelude::*;
use larrier_core::{config::SidebarGroup, Density, Settings, TextSizes, Theme};

use super::{Dropdown, Toggle};
use crate::icons::{Icon, Lucide, I};
use crate::state::use_app;

// ------------------------------------------------------------------ data

const ACCENTS: &[(&str, &str)] = &[
    ("", "Theme"),
    ("#3a6a9f", "Blue"),
    ("#2f6fdc", "Azure"),
    ("#1f8a65", "Green"),
    ("#0e8a9a", "Teal"),
    ("#7c5cd6", "Violet"),
    ("#d14d8a", "Pink"),
    ("#c4442e", "Red"),
    ("#c08532", "Amber"),
    ("#26251e", "Graphite"),
];

const UI_FONTS: &[&str] = &[
    "Inter",
    "Segoe UI Variable Text",
    "Segoe UI",
    "SF Pro Text",
    "Geist",
    "IBM Plex Sans",
    "Roboto",
    "Source Sans 3",
    "Noto Sans",
    "Helvetica Neue",
    "Arial",
];
const MONO_FONTS: &[&str] = &[
    "JetBrains Mono",
    "Cascadia Code",
    "Cascadia Mono",
    "Fira Code",
    "Geist Mono",
    "IBM Plex Mono",
    "SF Mono",
    "Source Code Pro",
    "Consolas",
    "Menlo",
];

/// Colors a user may override per theme: (CSS variable, label, help).
const COLOR_GROUPS: &[(&str, &[(&str, &str, &str)])] = &[
    (
        "Surfaces",
        &[
            ("win", "Window & sidebar", "Chat and sidebar background"),
            ("pane", "Panels", "Side panel, settings and editor"),
            ("card", "Cards", "Prompts, tool and edit cards"),
            ("card-2", "Composer & raised", "Composer, inputs and menus"),
            ("selected", "Selection", "Selected rows and tabs"),
            ("chip", "Chips", "Pills, badges and segmented controls"),
            ("hover", "Hover", "Row and button hover"),
        ],
    ),
    (
        "Text",
        &[
            ("fg", "Text", "Primary text and icons"),
            ("fg-2", "Secondary text", "Labels and descriptions"),
            ("fg-3", "Muted text", "Timestamps and hints"),
        ],
    ),
    (
        "Lines",
        &[("border", "Borders", "Cards, inputs and dividers"), ("border-2", "Strong borders", "Focused and hovered edges")],
    ),
    (
        "Status",
        &[
            ("blue", "Info & links", "Links and the default accent"),
            ("green", "Added", "Additions and success"),
            ("red", "Removed", "Deletions and errors"),
            ("amber", "Warnings", "Needs input and warnings"),
            ("code-bg", "Inline code", "Inline code background"),
        ],
    ),
];

fn all_color_keys() -> Vec<&'static str> {
    COLOR_GROUPS.iter().flat_map(|(_, rows)| rows.iter().map(|r| r.0)).collect()
}

/// The handful of base colors a preset is made from; everything else
/// (muted text, borders, hover, diff tints) is derived.
#[derive(Clone, Copy, PartialEq)]
struct Base {
    win: &'static str,
    pane: &'static str,
    card: &'static str,
    card2: &'static str,
    selected: &'static str,
    chip: &'static str,
    chip2: &'static str,
    fg: &'static str,
    blue: &'static str,
    green: &'static str,
    red: &'static str,
    amber: &'static str,
}

struct Preset {
    name: &'static str,
    note: &'static str,
    light: Base,
    dark: Base,
    contrast: bool,
}

const LARRIER_LIGHT: Base = Base {
    win: "#f2f1ed", pane: "#f7f7f4", card: "#f7f7f4", card2: "#fbfbf9", selected: "#ebeae5", chip: "#e6e5e0", chip2: "#e1e0db",
    fg: "#26251e", blue: "#3a6a9f", green: "#1f8a65", red: "#cf2d56", amber: "#c08532",
};
const LARRIER_DARK: Base = Base {
    win: "#161512", pane: "#1b1a17", card: "#201f1b", card2: "#23221e", selected: "#262520", chip: "#2b2a25", chip2: "#33322c",
    fg: "#e9e7e0", blue: "#7ba7d8", green: "#4cb88a", red: "#ee6a8a", amber: "#d9a149",
};

const PRESETS: &[Preset] = &[
    Preset { name: "Larrier", note: "Warm paper and ink", light: LARRIER_LIGHT, dark: LARRIER_DARK, contrast: false },
    Preset {
        name: "Neutral",
        note: "Clean grays, crisp white cards",
        light: Base { win: "#f3f3f3", pane: "#f8f8f8", card: "#ffffff", card2: "#ffffff", selected: "#e8e8e8", chip: "#e4e4e4", chip2: "#dcdcdc", fg: "#1f1f1f", blue: "#2f6fdc", green: "#1e8a5a", red: "#d0364f", amber: "#b97d18" },
        dark: Base { win: "#141414", pane: "#181818", card: "#1e1e1e", card2: "#222222", selected: "#2a2a2a", chip: "#2c2c2c", chip2: "#353535", fg: "#e6e6e6", blue: "#6ea8fe", green: "#4ec98f", red: "#f2718a", amber: "#e2a93b" },
        contrast: false,
    },
    Preset {
        name: "Graphite",
        note: "Cool slate grays",
        light: Base { win: "#eef0f3", pane: "#f6f7f9", card: "#fbfbfc", card2: "#ffffff", selected: "#e2e5ea", chip: "#dfe2e7", chip2: "#d6dae0", fg: "#1c2026", blue: "#3d6fd1", green: "#23885f", red: "#c93a52", amber: "#b5801f" },
        dark: Base { win: "#111317", pane: "#15181c", card: "#1b1e23", card2: "#1f2328", selected: "#262a31", chip: "#2a2e35", chip2: "#333841", fg: "#e3e6eb", blue: "#7aa5f0", green: "#56c495", red: "#ef7a8d", amber: "#dcae55" },
        contrast: false,
    },
    Preset {
        name: "Nord",
        note: "Arctic blues",
        light: Base { win: "#e5e9f0", pane: "#eceff4", card: "#f2f4f8", card2: "#f7f8fb", selected: "#d8dee9", chip: "#d8dee9", chip2: "#cdd4e0", fg: "#2e3440", blue: "#5e81ac", green: "#5f8a4c", red: "#bf616a", amber: "#c0872f" },
        dark: Base { win: "#2a2f3a", pane: "#2e3440", card: "#343b48", card2: "#3b4252", selected: "#434c5e", chip: "#434c5e", chip2: "#4c566a", fg: "#eceff4", blue: "#88c0d0", green: "#a3be8c", red: "#bf616a", amber: "#ebcb8b" },
        contrast: false,
    },
    Preset {
        name: "Solarized",
        note: "Precision contrast",
        light: Base { win: "#eee8d5", pane: "#fdf6e3", card: "#fdf6e3", card2: "#fffbef", selected: "#e4ddc8", chip: "#e2dbc4", chip2: "#d8d0b8", fg: "#3b4d53", blue: "#268bd2", green: "#859900", red: "#dc322f", amber: "#b58900" },
        dark: Base { win: "#00232c", pane: "#002b36", card: "#06323e", card2: "#073642", selected: "#0b3f4c", chip: "#0e4452", chip2: "#134d5c", fg: "#dcd6c3", blue: "#268bd2", green: "#859900", red: "#dc322f", amber: "#b58900" },
        contrast: false,
    },
    Preset {
        name: "Rosé Pine",
        note: "Soho vibes",
        light: Base { win: "#f4ede8", pane: "#faf4ed", card: "#fffaf3", card2: "#fffaf3", selected: "#f2e9e1", chip: "#ebe0d6", chip2: "#dfdad9", fg: "#575279", blue: "#286983", green: "#56949f", red: "#b4637a", amber: "#ea9d34" },
        dark: Base { win: "#191724", pane: "#1f1d2e", card: "#232136", card2: "#26233a", selected: "#2a273f", chip: "#393552", chip2: "#44415a", fg: "#e0def4", blue: "#c4a7e7", green: "#9ccfd8", red: "#eb6f92", amber: "#f6c177" },
        contrast: false,
    },
    Preset {
        name: "Sepia",
        note: "Easy on the eyes",
        light: Base { win: "#f1e9da", pane: "#f7f0e3", card: "#fbf6ec", card2: "#fdf9f1", selected: "#e8dcc6", chip: "#e6d9c2", chip2: "#dccdb2", fg: "#3b2f22", blue: "#2f6690", green: "#4c7a34", red: "#b5452b", amber: "#b07a1a" },
        dark: Base { win: "#1c1813", pane: "#211c16", card: "#27211a", card2: "#2b241c", selected: "#332b21", chip: "#3a3126", chip2: "#44392c", fg: "#ecdfc8", blue: "#8fb4d6", green: "#a3c57f", red: "#e8846b", amber: "#e0b25c" },
        contrast: false,
    },
    Preset {
        name: "Midnight",
        note: "Deep blue-black",
        light: Base { win: "#edf1f7", pane: "#f5f7fb", card: "#fbfcfe", card2: "#ffffff", selected: "#e1e7f1", chip: "#dde4ef", chip2: "#d2dbe8", fg: "#18202e", blue: "#2d6ae0", green: "#1b8a66", red: "#d23c5c", amber: "#c0841f" },
        dark: Base { win: "#0a0d13", pane: "#0e1219", card: "#141922", card2: "#171d27", selected: "#1c2330", chip: "#212938", chip2: "#2a3344", fg: "#dde5f0", blue: "#6aa1ff", green: "#4fd1a1", red: "#ff7a93", amber: "#f0b45a" },
        contrast: false,
    },
    Preset {
        name: "High Contrast",
        note: "Maximum legibility",
        light: Base { win: "#ffffff", pane: "#ffffff", card: "#ffffff", card2: "#ffffff", selected: "#dbe4ff", chip: "#e6e6e6", chip2: "#d0d0d0", fg: "#000000", blue: "#0047c2", green: "#006b3c", red: "#b0002a", amber: "#8a5a00" },
        dark: Base { win: "#000000", pane: "#000000", card: "#0a0a0a", card2: "#0d0d0d", selected: "#1d2b4d", chip: "#1a1a1a", chip2: "#2a2a2a", fg: "#ffffff", blue: "#7cb4ff", green: "#3fe08f", red: "#ff6b8b", amber: "#ffc04d" },
        contrast: true,
    },
];

/// Full token set for one theme of a preset.
fn derive(b: &Base, dark: bool, contrast: bool) -> Vec<(&'static str, String)> {
    let a = |hex: &str, alpha: f32| with_alpha(hex, alpha);
    let pick = |normal_light: f32, normal_dark: f32, hc: f32| if contrast { hc } else if dark { normal_dark } else { normal_light };
    vec![
        ("win", b.win.into()),
        ("pane", b.pane.into()),
        ("card", b.card.into()),
        ("card-2", b.card2.into()),
        ("input", b.card2.into()),
        ("selected", b.selected.into()),
        ("chip", b.chip.into()),
        ("chip-2", b.chip2.into()),
        ("hover", a(b.fg, pick(0.04, 0.045, 0.09))),
        ("code-bg", a(b.fg, pick(0.055, 0.07, 0.1))),
        ("overlay", if dark { "rgba(0, 0, 0, 0.42)".into() } else { a(b.fg, 0.14) }),
        ("fg", b.fg.into()),
        ("fg-2", a(b.fg, pick(0.6, 0.6, 0.85))),
        ("fg-3", a(b.fg, pick(0.4, 0.4, 0.7))),
        ("fg-4", a(b.fg, pick(0.22, 0.2, 0.45))),
        ("border", a(b.fg, pick(0.1, 0.085, 0.4))),
        ("border-2", a(b.fg, pick(0.16, 0.14, 0.6))),
        ("blue", b.blue.into()),
        ("green", b.green.into()),
        ("red", b.red.into()),
        ("amber", b.amber.into()),
        ("blue-bg", a(b.blue, if dark { 0.12 } else { 0.09 })),
        ("green-bg", a(b.green, if dark { 0.12 } else { 0.1 })),
        ("red-bg", a(b.red, if dark { 0.1 } else { 0.08 })),
        ("amber-bg", a(b.amber, if dark { 0.13 } else { 0.12 })),
        ("diff-add", a(b.green, if dark { 0.13 } else { 0.11 })),
        ("diff-del", a(b.red, if dark { 0.11 } else { 0.09 })),
        ("accent", b.fg.into()),
        ("accent-fg", b.pane.into()),
        ("scroll", a(b.fg, if dark { 0.13 } else { 0.14 })),
    ]
}

/// The `custom_colors` map a preset writes (empty for the built-in theme).
fn preset_colors(p: &Preset) -> BTreeMap<String, String> {
    if p.name == "Larrier" {
        return BTreeMap::new();
    }
    let mut m = BTreeMap::new();
    for (k, v) in derive(&p.light, false, p.contrast) {
        m.insert(format!("light.{k}"), v);
    }
    for (k, v) in derive(&p.dark, true, p.contrast) {
        m.insert(format!("dark.{k}"), v);
    }
    m
}

// ------------------------------------------------------------------ colors

/// (r, g, b, alpha) from `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb()` or `rgba()`.
fn parse_color(v: &str) -> Option<(u8, u8, u8, f32)> {
    let v = v.trim();
    if let Some(h) = v.strip_prefix('#') {
        if !h.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let x = |s: &str| u8::from_str_radix(s, 16).ok();
        let d = |c: char| x(&format!("{c}{c}"));
        let c: Vec<char> = h.chars().collect();
        return match c.len() {
            3 => Some((d(c[0])?, d(c[1])?, d(c[2])?, 1.0)),
            4 => Some((d(c[0])?, d(c[1])?, d(c[2])?, d(c[3])? as f32 / 255.0)),
            6 => Some((x(&h[0..2])?, x(&h[2..4])?, x(&h[4..6])?, 1.0)),
            8 => Some((x(&h[0..2])?, x(&h[2..4])?, x(&h[4..6])?, x(&h[6..8])? as f32 / 255.0)),
            _ => None,
        };
    }
    let lower = v.to_ascii_lowercase();
    let inner = lower.strip_prefix("rgba(").or_else(|| lower.strip_prefix("rgb("))?.strip_suffix(')')?;
    let nums: Vec<f32> = inner
        .split(|c: char| c == ',' || c == '/' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.trim_end_matches('%').parse().ok())
        .collect();
    if nums.len() < 3 {
        return None;
    }
    let ch = |f: f32| f.round().clamp(0.0, 255.0) as u8;
    Some((ch(nums[0]), ch(nums[1]), ch(nums[2]), nums.get(3).copied().unwrap_or(1.0).clamp(0.0, 1.0)))
}

/// `#rrggbb` for a color input (alpha dropped).
fn hex6(v: &str) -> Option<String> {
    parse_color(v).map(|(r, g, b, _)| format!("#{r:02x}{g:02x}{b:02x}"))
}

fn with_alpha(hex: &str, alpha: f32) -> String {
    match parse_color(hex) {
        Some((r, g, b, _)) => format!("rgba({r}, {g}, {b}, {})", (alpha * 1000.0).round() / 1000.0),
        None => hex.to_string(),
    }
}

/// Values written into an inline style must stay a single declaration.
fn valid_css_color(v: &str) -> bool {
    let v = v.trim();
    !v.is_empty() && v.len() <= 64 && v.chars().all(|c| c.is_ascii_alphanumeric() || " #(),.%/-".contains(c))
}

/// CSS variables for a nested themed scope (preview / theme cards), the
/// same way the window root applies the user's settings.

/// One text size: what it changes, a sample at that size and − / +.
#[component]
fn SizeRow(title: &'static str, desc: &'static str, value: u32, min: u32, max: u32, sample: &'static str, onchange: EventHandler<u32>) -> Element {
    let (font, text) = match sample {
        "code" | "term" => ("var(--mono)", "let x = 42;"),
        _ => ("var(--font)", "Aa"),
    };
    rsx! {
        div { class: "set-row",
            div { class: "lbl", div { class: "t", "{title}" } div { class: "d", "{desc}" } }
            div { class: "ap-size",
                span { class: "ap-size-sample", style: "font-family: {font}; font-size: {value}px;", "{text}" }
                super::Stepper { value, min, max, onchange: move |v| onchange.call(v) }
            }
        }
    }
}

/// Key that changes with an inline style, so Dioxus re-creates the element
/// instead of patching it (patching keeps properties the new style dropped).
fn style_key(style: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    style.hash(&mut h);
    h.finish()
}

fn scope_style(s: &Settings, dark: bool) -> String {
    let theme = if dark { "dark" } else { "light" };
    let mut style = super::text_size_vars(s);
    if !s.accent.trim().is_empty() && valid_css_color(&s.accent) {
        style.push_str(&format!(" --accent-c: {};", s.accent.trim()));
    }
    if let Some(f) = s.ui_font.as_ref().filter(|f| !f.trim().is_empty()) {
        style.push_str(&format!(" --font: '{}', system-ui, sans-serif;", f.trim().replace(['\'', ';', '"'], "")));
    }
    if let Some(f) = s.mono_font.as_ref().filter(|f| !f.trim().is_empty()) {
        style.push_str(&format!(" --mono: '{}', ui-monospace, monospace;", f.trim().replace(['\'', ';', '"'], "")));
    }
    let prefix = format!("{theme}.");
    for (k, v) in &s.custom_colors {
        if let Some(var) = k.strip_prefix(&prefix) {
            if valid_css_color(v) && var.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                style.push_str(&format!(" --{var}: {};", v.trim()));
            }
        }
    }
    style
}

fn font_family(f: &str, mono: bool) -> String {
    let f = f.replace(['\'', ';', '"'], "");
    if mono {
        format!("'{f}', ui-monospace, monospace")
    } else {
        format!("'{f}', system-ui, sans-serif")
    }
}

const SAMPLE_MD: &str = "Done — the settings page now **stacks below 760px** and follows your theme. I updated `settings.rs`:\n\n```rust\nfn layout(width: f32) -> Layout {\n    // Stack the columns on narrow windows\n    if width < 760.0 { Layout::Stacked } else { Layout::Split }\n}\n```\n";

// ------------------------------------------------------------------ page

#[component]
pub fn AppearancePage() -> Element {
    let mut app = use_app();
    let s = app.config.read().settings.clone();
    let app_dark = app.ui.read().dark;
    // Which theme the color editor and preview show (starts on the active one).
    let mut edit_dark = use_signal(move || app_dark);
    let mut computed: Signal<HashMap<String, String>> = use_signal(HashMap::new);
    let mut fonts_ok: Signal<HashMap<String, bool>> = use_signal(HashMap::new);
    let seg = |on: bool| if on { "on" } else { "" };
    let variant = if edit_dark() { "dark" } else { "light" };

    // Effective token values of the edited theme, read from the preview.
    let dep = (edit_dark(), s.custom_colors.clone(), app_dark);
    use_effect(use_reactive!(|dep| {
        let _ = &dep;
        let keys = serde_json::to_string(&all_color_keys()).unwrap_or_default();
        spawn(async move {
            let mut ev = document::eval(&format!(
                r#"requestAnimationFrame(() => requestAnimationFrame(() => {{
                    const el = document.querySelector('.ap-mini');
                    const out = {{}};
                    if (el) {{ const cs = getComputedStyle(el); for (const k of {keys}) out[k] = cs.getPropertyValue('--' + k).trim(); }}
                    dioxus.send(out);
                }}));"#
            ));
            if let Ok(v) = ev.recv::<serde_json::Value>().await {
                if let Some(o) = v.as_object() {
                    computed.set(o.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect());
                }
            }
        });
    }));

    // Which fonts are installed (canvas width test).
    use_hook(move || {
        let list: Vec<&str> = UI_FONTS.iter().chain(MONO_FONTS.iter()).copied().collect();
        let fonts = serde_json::to_string(&list).unwrap_or_default();
        spawn(async move {
            let mut ev = document::eval(&format!(
                r#"const c = document.createElement('canvas').getContext('2d');
                const s = 'mmmmmmmmmmlli1WQ@#_ AaBbGg';
                const has = (f) => ['monospace', 'serif', 'sans-serif'].some(b => {{
                    c.font = '72px ' + b; const w0 = c.measureText(s).width;
                    c.font = '72px "' + f + '", ' + b; return c.measureText(s).width !== w0; }});
                dioxus.send(Object.fromEntries({fonts}.map(f => [f, has(f)])));"#
            ));
            if let Ok(v) = ev.recv::<serde_json::Value>().await {
                if let Some(o) = v.as_object() {
                    fonts_ok.set(o.iter().map(|(k, v)| (k.clone(), v.as_bool().unwrap_or(true))).collect());
                }
            }
        });
    });

    let code_themes: Vec<String> = crate::highlight::themes().into_iter().map(|t| t.to_string()).collect();
    let mut code_opts: Vec<(String, String)> = vec![(String::new(), "Default".into())];
    code_opts.extend(code_themes.iter().map(|t| (t.clone(), t.clone())));
    let active_preset = PRESETS.iter().position(|p| preset_colors(p) == s.custom_colors);
    let overrides_here = s.custom_colors.keys().filter(|k| k.starts_with(&format!("{variant}."))).count();
    let custom_accent = !s.accent.is_empty() && valid_css_color(&s.accent) && !ACCENTS.iter().any(|(h, _)| *h == s.accent);

    let export = move |_| {
        let s = app.config.read().settings.clone();
        let v = serde_json::json!({
            "larrier_theme": 1,
            "accent": s.accent,
            "custom_colors": s.custom_colors,
            "code_theme_light": s.code_theme_light,
            "code_theme_dark": s.code_theme_dark,
            "ui_font": s.ui_font,
            "mono_font": s.mono_font,
            "font_size": s.font_size,
            "density": s.density,
        });
        app.copy_text(&serde_json::to_string_pretty(&v).unwrap_or_default());
        app.toast("Theme copied to the clipboard");
    };
    let import = move |_| {
        let text = arboard::Clipboard::new().and_then(|mut c| c.get_text()).unwrap_or_default();
        match import_theme(&text) {
            Ok(f) => {
                app.update_config(f);
                app.toast("Theme imported");
            }
            Err(e) => app.toast_err(format!("Couldn't import: {e}")),
        }
    };
    let reset_all = move |_| {
        let d = Settings::default();
        app.update_config(move |c| {
            let s = &mut c.settings;
            s.accent = d.accent.clone();
            s.custom_colors.clear();
            s.code_theme_light = None;
            s.code_theme_dark = None;
            s.ui_font = None;
            s.mono_font = None;
            s.font_size = d.font_size;
            s.density = d.density;
            s.sidebar_right = d.sidebar_right;
            s.transcript_width = d.transcript_width;
            s.zoom = 1.0;
            s.reduce_motion = false;
        });
        app.toast("Appearance reset to defaults");
    };

    rsx! {
        style { {include_str!("appearance.css")} }
        div { class: "ap-wrap",
            div { class: "ap-head",
                div { class: "grow",
                    h1 { "Appearance" }
                    div { class: "ap-sub", "Make Larrier yours. Every change shows in the preview as you make it." }
                }
                div { class: "ap-head-actions",
                    button { class: "ghost-btn", title: "Copy this look as JSON", onclick: export, Lucide { name: "share", size: 13 } "Export" }
                    button { class: "ghost-btn", title: "Apply a theme JSON from the clipboard", onclick: import, Icon { i: I::ClipboardPaste, size: 13 } "Import" }
                    button { class: "ghost-btn", title: "Back to the default look", onclick: reset_all, Lucide { name: "rotate-ccw", size: 13 } "Reset" }
                }
            }
            div { class: "ap-page",
                div { class: "ap-main",
                    // ---------------------------------------------------- theme
                    div { class: "set-group-title", Lucide { name: "contrast", size: 13 } "Theme" }
                    div { class: "ap-themes",
                        for (t, label, icon) in [(Theme::System, "System", I::Monitor), (Theme::Light, "Light", I::Sun), (Theme::Dark, "Dark", I::Moon)] {
                            button { key: "{label}", class: if s.theme == t { "ap-theme-card on" } else { "ap-theme-card" },
                                onclick: move |_| {
                                    app.update_config(|c| c.settings.theme = t);
                                    app.apply_theme();
                                    let dark = app.ui.read().dark;
                                    edit_dark.set(dark);
                                },
                                div { class: "ap-theme-shot",
                                    match t {
                                        Theme::System => rsx! {
                                            MiniWindow { dark: false }
                                            div { class: "ap-split-dark", MiniWindow { dark: true } }
                                        },
                                        Theme::Light => rsx! { MiniWindow { dark: false } },
                                        Theme::Dark => rsx! { MiniWindow { dark: true } },
                                    }
                                }
                                div { class: "ap-theme-label",
                                    Icon { i: icon, size: 13 }
                                    span { "{label}" }
                                    span { class: "ap-check", Icon { i: I::Check, size: 12, stroke: 2.4 } }
                                }
                            }
                        }
                    }

                    // ---------------------------------------------------- presets
                    div { class: "set-group-title", Lucide { name: "swatch-book", size: 13 } "Presets"
                        span { class: "ap-hint", "Each preset sets both light and dark colors" }
                    }
                    div { class: "ap-presets",
                        for (i, p) in PRESETS.iter().enumerate() {
                            button { key: "{p.name}", class: if active_preset == Some(i) { "ap-preset on" } else { "ap-preset" },
                                title: "{p.note}",
                                onclick: move |_| {
                                    let m = preset_colors(&PRESETS[i]);
                                    app.update_config(move |c| c.settings.custom_colors = m);
                                },
                                div { class: "ap-strips",
                                    PresetStrip { b: p.light }
                                    PresetStrip { b: p.dark }
                                }
                                div { class: "ap-preset-name",
                                    span { class: "grow ellipsis", "{p.name}" }
                                    span { class: "ap-check", Icon { i: I::Check, size: 12, stroke: 2.4 } }
                                }
                                div { class: "ap-preset-note", "{p.note}" }
                            }
                        }
                    }

                    // ---------------------------------------------------- accent
                    div { class: "set-group-title", Lucide { name: "pipette", size: 13 } "Accent" }
                    div { class: "set-card",
                        div { class: "set-row",
                            div { class: "lbl", div { class: "t", "Accent Color" } div { class: "d", "Links, focus rings, toggles and highlights." } }
                            div { class: "ap-accents",
                                for (hex, name) in ACCENTS.iter().copied() {
                                    button { key: "{name}", title: "{name}",
                                        class: if s.accent == hex { "ap-dot on" } else { "ap-dot" },
                                        style: if hex.is_empty() { "background: var(--blue);".to_string() } else { format!("background: {hex};") },
                                        onclick: move |_| app.update_config(|c| c.settings.accent = hex.to_string()),
                                        if hex.is_empty() { span { class: "ap-dot-a", "A" } }
                                    }
                                }
                                label { key: "{custom_accent}", class: if custom_accent { "ap-dot custom on" } else { "ap-dot custom" },
                                    title: "Custom color",
                                    style: if custom_accent { format!("background: {};", s.accent) } else { String::new() },
                                    Lucide { name: "plus", size: 12 }
                                    input { r#type: "color",
                                        value: hex6(&s.accent).unwrap_or_else(|| "#3a6a9f".into()),
                                        oninput: move |e| app.config.write().settings.accent = e.value(),
                                        onchange: move |_| app.save_config(),
                                    }
                                }
                            }
                        }
                        div { class: "set-row",
                            div { class: "lbl", div { class: "t", "Hex" } div { class: "d", "Any CSS color — leave empty to follow the theme." } }
                            HexInput {
                                value: s.accent.clone(),
                                placeholder: "Theme",
                                onset: move |v: String| app.update_config(move |c| c.settings.accent = v),
                            }
                        }
                    }

                    // ---------------------------------------------------- colors
                    div { class: "set-group-title", Lucide { name: "paintbrush", size: 13 } "Colors"
                        div { class: "grow" }
                        div { class: "seg",
                            button { class: seg(!edit_dark()), onclick: move |_| edit_dark.set(false), Icon { i: I::Sun, size: 12 } " Light" }
                            button { class: seg(edit_dark()), onclick: move |_| edit_dark.set(true), Icon { i: I::Moon, size: 12 } " Dark" }
                        }
                        button { class: "ghost-btn", disabled: overrides_here == 0,
                            title: "Remove every {variant} color override",
                            onclick: move |_| {
                                let pre = format!("{variant}.");
                                app.update_config(move |c| c.settings.custom_colors.retain(|k, _| !k.starts_with(&pre)));
                            },
                            "Reset {variant}"
                        }
                    }
                    for (group, rows) in COLOR_GROUPS.iter().copied() {
                        div { key: "{group}", class: "set-card ap-colors",
                            div { class: "ap-color-group", "{group}" }
                            for (key, label, desc) in rows.iter().copied() {
                                ColorRow {
                                    key: "{variant}-{key}",
                                    theme: variant,
                                    var: key,
                                    label,
                                    desc,
                                    overridden: s.custom_colors.get(&format!("{variant}.{key}")).cloned(),
                                    effective: computed.read().get(key).cloned().unwrap_or_default(),
                                }
                            }
                        }
                    }

                    // ---------------------------------------------------- code
                    div { class: "set-group-title", Lucide { name: "code", size: 13 } "Code" }
                    div { class: "set-card",
                        div { class: "set-row",
                            div { class: "lbl", div { class: "t", "Light Code Theme" } div { class: "d", "Syntax colors in code blocks and diffs." } }
                            super::Select { value: s.code_theme_light.clone().unwrap_or_default(), options: code_opts.clone(), wide: true,
                                onchange: move |v: String| { edit_dark.set(false); app.update_config(|c| c.settings.code_theme_light = if v.is_empty() { None } else { Some(v) }) } }
                        }
                        div { class: "set-row",
                            div { class: "lbl", div { class: "t", "Dark Code Theme" } div { class: "d", "Used when Larrier is dark." } }
                            super::Select { value: s.code_theme_dark.clone().unwrap_or_default(), options: code_opts, wide: true,
                                onchange: move |v: String| { edit_dark.set(true); app.update_config(|c| c.settings.code_theme_dark = if v.is_empty() { None } else { Some(v) }) } }
                        }
                    }

                    // ---------------------------------------------------- text
                    div { class: "set-group-title", Lucide { name: "type", size: 13 } "Text" }
                    div { class: "set-card",
                        div { class: "set-row",
                            div { class: "lbl", div { class: "t", "Interface Font" } div { class: "d", "Menus, sidebar and transcripts." } }
                            FontPicker { mono: false, installed: fonts_ok.read().clone() }
                        }
                        div { class: "ap-sample", style: "font-family: var(--font); font-size: {s.font_size}px;",
                            "The quick brown fox jumps over the lazy dog — 0123456789"
                        }
                        div { class: "set-row",
                            div { class: "lbl", div { class: "t", "Code Font" } div { class: "d", "Code blocks, diffs and terminals." } }
                            FontPicker { mono: true, installed: fonts_ok.read().clone() }
                        }
                        div { class: "ap-sample mono", style: "font-family: var(--mono);",
                            "fn main() {{ let ok = 0O1lI != \"{{}}[]\"; }} => -> != <="
                        }
                    }

                    // ---------------------------------------------------- text sizes
                    div { class: "set-group-title", Lucide { name: "a-large-small", size: 13 } "Text Sizes"
                        if s.font_size != 14 || s.text_sizes != TextSizes::default() {
                            button { class: "set-link", onclick: move |_| app.update_config(|c| { c.settings.font_size = 14; c.settings.text_sizes = TextSizes::default(); }), "Reset" }
                        }
                    }
                    div { class: "set-card",
                        SizeRow { title: "Replies", desc: "The agent's answers in the chat.", value: s.font_size, min: 11, max: 22,
                            sample: "fs", onchange: move |v| app.update_config(|c| c.settings.font_size = v) }
                        SizeRow { title: "Prompts", desc: "Your messages in the chat.", value: s.text_sizes.prompts, min: 11, max: 22,
                            sample: "prompt", onchange: move |v| app.update_config(|c| c.settings.text_sizes.prompts = v) }
                        SizeRow { title: "Composer", desc: "What you type before sending.", value: s.text_sizes.composer, min: 11, max: 22,
                            sample: "composer", onchange: move |v| app.update_config(|c| c.settings.text_sizes.composer = v) }
                        SizeRow { title: "Steps & Tools", desc: "Tool steps, commands and cards.", value: s.text_sizes.steps, min: 10, max: 20,
                            sample: "steps", onchange: move |v| app.update_config(|c| c.settings.text_sizes.steps = v) }
                        SizeRow { title: "Code", desc: "Code blocks, diffs and tool output.", value: s.text_sizes.code, min: 10, max: 20,
                            sample: "code", onchange: move |v| app.update_config(|c| c.settings.text_sizes.code = v) }
                        SizeRow { title: "Sidebar", desc: "Chat and repository names.", value: s.text_sizes.sidebar, min: 11, max: 18,
                            sample: "side", onchange: move |v| app.update_config(|c| c.settings.text_sizes.sidebar = v) }
                        SizeRow { title: "Terminal", desc: "The built-in terminal (new terminals).", value: s.text_sizes.terminal, min: 10, max: 22,
                            sample: "term", onchange: move |v| app.update_config(|c| c.settings.text_sizes.terminal = v) }
                        div { class: "set-row",
                            div { class: "lbl", div { class: "t", "Zoom" } div { class: "d", "Scale the whole window — Ctrl + / Ctrl − / Ctrl 0." } }
                            div { class: "seg",
                                for z in [0.8f64, 0.9, 1.0, 1.1, 1.25, 1.5] {
                                    button { key: "{z}", class: seg((s.zoom - z).abs() < 0.01), onclick: move |_| app.update_config(|c| c.settings.zoom = z), "{(z * 100.0).round()}%" }
                                }
                            }
                        }
                    }

                    // ---------------------------------------------------- layout
                    div { class: "set-group-title", Lucide { name: "layout-panel-left", size: 13 } "Layout" }
                    div { class: "ap-layout-cards",
                        for (d, label) in [(Density::Comfortable, "Comfortable"), (Density::Compact, "Compact")] {
                            button { key: "{label}", class: if s.density == d { "ap-lcard on" } else { "ap-lcard" },
                                onclick: move |_| app.update_config(|c| c.settings.density = d),
                                div { class: if d == Density::Compact { "ap-lshot dense" } else { "ap-lshot" },
                                    for i in 0..5 { div { key: "{i}", class: if i == 1 { "ap-lrow sel" } else { "ap-lrow" }, span { class: "pt" } span { class: "ln" } } }
                                }
                                div { class: "ap-lcap", span { "{label}" } span { class: "ap-check", Icon { i: I::Check, size: 12, stroke: 2.4 } } }
                            }
                        }
                        for (right, label) in [(false, "Sidebar left"), (true, "Sidebar right")] {
                            button { key: "{label}", class: if s.sidebar_right == right { "ap-lcard on" } else { "ap-lcard" },
                                onclick: move |_| app.update_config(|c| c.settings.sidebar_right = right),
                                div { class: if right { "ap-lshot side rev" } else { "ap-lshot side" },
                                    div { class: "ap-lside", div { class: "ln" } div { class: "ln sel" } div { class: "ln" } }
                                    div { class: "ap-lbody", div { class: "ln w" } div { class: "ln" } div { class: "ln w" } }
                                }
                                div { class: "ap-lcap", span { "{label}" } span { class: "ap-check", Icon { i: I::Check, size: 12, stroke: 2.4 } } }
                            }
                        }
                    }
                    div { class: "set-card", style: "margin-top: 10px;",
                        div { class: "set-row",
                            div { class: "lbl", div { class: "t", "Transcript Width" } div { class: "d", "Widest the conversation column gets." } }
                            div { class: "ap-range wide",
                                div { class: "ap-tw", div { class: "ap-tw-col", style: "width: {((s.transcript_width.clamp(560, 1600) - 480) as f64 / 11.2).round()}%;" } }
                                super::Stepper { value: s.transcript_width.clamp(560, 1600), min: 560, max: 1600, step: 40,
                                    onchange: move |v| app.update_config(|c| c.settings.transcript_width = v) }
                            }
                        }
                        div { class: "set-row",
                            div { class: "lbl", div { class: "t", "Sidebar Grouping" } div { class: "d", "How agents are grouped in the sidebar." } }
                            div { class: "seg",
                                for (g, l) in [(SidebarGroup::Status, "Repositories"), (SidebarGroup::Agent, "Agent")] {
                                    button { key: "{l}", class: seg(s.sidebar_group == g || (l == "Repositories" && s.sidebar_group == SidebarGroup::Workspace)),
                                        onclick: move |_| app.update_config(|c| c.settings.sidebar_group = g), "{l}" }
                                }
                            }
                        }
                    }

                    // ---------------------------------------------------- motion
                    div { class: "set-group-title", Lucide { name: "wind", size: 13 } "Motion" }
                    div { class: "set-card",
                        div { class: "set-row",
                            div { class: "lbl", div { class: "t", "Reduce Motion" } div { class: "d", "Minimize animations and transitions across Larrier." } }
                            Toggle { on: s.reduce_motion, onchange: move |v: bool| app.update_config(|c| c.settings.reduce_motion = v) }
                        }
                    }
                }

                // -------------------------------------------------------- preview
                div { class: "ap-preview-col",
                    div { class: "ap-preview-head",
                        span { class: "ap-live", span { class: "ap-live-dot" } "Live preview" }
                        div { class: "grow" }
                        div { class: "seg",
                            button { class: seg(!edit_dark()), title: "Preview light", onclick: move |_| edit_dark.set(false), Icon { i: I::Sun, size: 12 } }
                            button { class: seg(edit_dark()), title: "Preview dark", onclick: move |_| edit_dark.set(true), Icon { i: I::Moon, size: 12 } }
                        }
                    }
                    PreviewPane { dark: edit_dark() }
                    div { class: "ap-preview-foot",
                        if overrides_here > 0 {
                            "{overrides_here} custom {variant} colors"
                        } else {
                            "Default {variant} colors"
                        }
                        if let Some(i) = active_preset { " · {PRESETS[i].name} preset" }
                    }
                }
            }
        }
    }
}

/// Parse an exported theme and return the settings change it makes.
fn import_theme(text: &str) -> anyhow::Result<impl FnOnce(&mut larrier_core::Config)> {
    let v: serde_json::Value = serde_json::from_str(text.trim()).map_err(|_| anyhow::anyhow!("the clipboard doesn't hold a Larrier theme"))?;
    if v.get("larrier_theme").is_none() {
        anyhow::bail!("the clipboard doesn't hold a Larrier theme");
    }
    let colors: BTreeMap<String, String> = v
        .get("custom_colors")
        .and_then(|c| c.as_object())
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .filter(|(k, v)| (k.starts_with("light.") || k.starts_with("dark.")) && valid_css_color(v))
                .collect()
        })
        .unwrap_or_default();
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(|x| x.to_string());
    let accent = s("accent").filter(|a| a.is_empty() || valid_css_color(a));
    let (light, dark, ui, mono) = (s("code_theme_light"), s("code_theme_dark"), s("ui_font"), s("mono_font"));
    let size = v.get("font_size").and_then(|x| x.as_u64()).map(|n| n.clamp(10, 20) as u32);
    let density: Option<Density> = v.get("density").and_then(|d| serde_json::from_value(d.clone()).ok());
    Ok(move |c: &mut larrier_core::Config| {
        let st = &mut c.settings;
        st.custom_colors = colors;
        if let Some(a) = accent {
            st.accent = a;
        }
        st.code_theme_light = light;
        st.code_theme_dark = dark;
        st.ui_font = ui;
        st.mono_font = mono;
        if let Some(n) = size {
            st.font_size = n;
        }
        if let Some(d) = density {
            st.density = d;
        }
    })
}

// ------------------------------------------------------------------ pieces

/// Six chips of a preset theme.
#[component]
fn PresetStrip(b: Base) -> Element {
    rsx! {
        div { class: "ap-strip", style: "background: {b.win};",
            span { style: "background: {b.card}; border-color: {b.selected};" }
            span { style: "background: {b.selected};" }
            span { style: "background: {b.fg};" }
            span { class: "dot", style: "background: {b.blue};" }
            span { class: "dot", style: "background: {b.green};" }
            span { class: "dot", style: "background: {b.red};" }
        }
    }
}

/// A tiny window drawn with the real theme tokens (overrides included).
#[component]
fn MiniWindow(dark: bool) -> Element {
    let app = use_app();
    let style = scope_style(&app.config.read().settings, dark);
    rsx! {
        div { key: "{style_key(&style)}", class: "app ap-swatch", "data-theme": if dark { "dark" } else { "light" }, style: "{style}",
            div { class: "ap-sw-side",
                div { class: "ap-sw-line w60" }
                div { class: "ap-sw-line sel" }
                div { class: "ap-sw-line w40" }
                div { class: "ap-sw-line w50" }
            }
            div { class: "ap-sw-main",
                div { class: "ap-sw-card", div { class: "ap-sw-line w70 fg" } }
                div { class: "ap-sw-line w80" }
                div { class: "ap-sw-line w50" }
                div { class: "ap-sw-composer", span { class: "ap-sw-dot" } }
            }
        }
    }
}

/// Text field that applies a CSS color on Enter / blur.
#[component]
fn HexInput(value: String, #[props(default = "")] placeholder: &'static str, onset: EventHandler<String>) -> Element {
    let mut bad = use_signal(|| false);
    rsx! {
        input { class: if bad() { "input ap-hex bad" } else { "input ap-hex" },
            spellcheck: false,
            placeholder: "{placeholder}",
            value: "{value}",
            oninput: move |_| bad.set(false),
            onchange: move |e| {
                let v = e.value().trim().to_string();
                if v.is_empty() || valid_css_color(&v) {
                    bad.set(false);
                    onset.call(v);
                } else {
                    bad.set(true);
                }
            },
        }
    }
}

#[component]
fn ColorRow(
    theme: &'static str,
    var: &'static str,
    label: &'static str,
    desc: &'static str,
    overridden: Option<String>,
    effective: String,
) -> Element {
    let mut app = use_app();
    let key = format!("{theme}.{var}");
    let shown = overridden.clone().unwrap_or_else(|| effective.clone());
    let alpha = parse_color(&shown).map(|c| c.3).unwrap_or(1.0);
    let picker = hex6(&shown).unwrap_or_else(|| "#808080".into());
    let is_set = overridden.is_some();
    let (k1, k2, k3) = (key.clone(), key.clone(), key.clone());
    rsx! {
        div { class: if is_set { "ap-color-row set" } else { "ap-color-row" },
            label { class: "ap-swatch-btn", title: "Pick a color",
                span { class: "ap-chip", style: "background: {shown};" }
                input { r#type: "color", value: "{picker}",
                    oninput: move |e| {
                        let v = e.value();
                        // Keep the token's transparency (hover, borders, muted text).
                        let v = if alpha < 0.999 { with_alpha(&v, alpha) } else { v };
                        app.config.write().settings.custom_colors.insert(k1.clone(), v);
                    },
                    onchange: move |_| app.save_config(),
                }
            }
            div { class: "lbl",
                div { class: "t", "{label}" if is_set { span { class: "ap-mod", "Custom" } } }
                div { class: "d", "{desc}" }
            }
            HexInput {
                value: shown.clone(),
                onset: move |v: String| {
                    let k = k2.clone();
                    app.update_config(move |c| {
                        if v.is_empty() {
                            c.settings.custom_colors.remove(&k);
                        } else {
                            c.settings.custom_colors.insert(k, v);
                        }
                    });
                },
            }
            button { class: "icon-btn sm ap-reset", title: "Reset to the theme color", disabled: !is_set,
                style: if is_set { "visibility: visible;" } else { "visibility: hidden;" },
                onclick: move |_| {
                    let k = k3.clone();
                    app.update_config(move |c| { c.settings.custom_colors.remove(&k); });
                },
                Lucide { name: "rotate-ccw", size: 12 }
            }
        }
    }
}

/// Font chooser whose options render in their own typeface.
#[component]
fn FontPicker(mono: bool, installed: HashMap<String, bool>) -> Element {
    let app = use_app();
    let open = use_signal(|| false);
    let mut custom = use_signal(String::new);
    let cur = {
        let s = &app.config.read().settings;
        if mono { s.mono_font.clone() } else { s.ui_font.clone() }
    }
    .filter(|f| !f.trim().is_empty());
    let default_label = if mono { "System monospace" } else { "System default" };
    let label = cur.clone().unwrap_or_else(|| default_label.to_string());
    let btn_font = match &cur {
        Some(f) => font_family(f, mono),
        None => if mono { "var(--mono)".into() } else { "var(--font)".into() },
    };
    let list: &'static [&'static str] = if mono { MONO_FONTS } else { UI_FONTS };
    let set = move |v: Option<String>| {
        let mut open = open;
        open.set(false);
        app.update_config(move |c| if mono { c.settings.mono_font = v } else { c.settings.ui_font = v });
    };
    let mut open_btn = open;
    rsx! {
        div { class: "menu-anchor",
            button { class: if open() { "select-btn open ap-font-btn" } else { "select-btn ap-font-btn" }, style: "font-family: {btn_font};",
                onclick: move |_| open_btn.set(true),
                span { class: "ellipsis", "{label}" }
                span { class: "chev", Icon { i: I::ChevronDown, size: 12 } }
            }
            Dropdown { open, up: false, right: true, class: "ap-font-menu",
                button { class: "menu-item", onclick: move |_| set(None),
                    span { class: "ellipsis", style: if mono { "font-family: ui-monospace, monospace;" } else { "font-family: system-ui, sans-serif;" }, "{default_label}" }
                    if cur.is_none() { span { class: "check", Icon { i: I::Check, size: 13 } } }
                }
                div { class: "menu-sep" }
                for f in list.iter().copied() {
                    {
                        let ok = installed.get(f).copied().unwrap_or(true);
                        let on = cur.as_deref() == Some(f);
                        let fam = font_family(f, mono);
                        rsx! {
                            button { key: "{f}", class: if ok { "menu-item ap-font-item" } else { "menu-item ap-font-item missing" },
                                title: if ok { "" } else { "Not installed on this PC — falls back to the system font" },
                                onclick: move |_| set(Some(f.to_string())),
                                span { class: "ellipsis", style: "font-family: {fam};", "{f}" }
                                span { class: "ap-font-aa", style: "font-family: {fam};", "Aa" }
                                if !ok { span { class: "sub", "not installed" } }
                                if on { span { class: "check", Icon { i: I::Check, size: 13 } } }
                            }
                        }
                    }
                }
                div { class: "menu-sep" }
                div { class: "ap-font-custom",
                    input { class: "input", placeholder: "Any installed font…", value: "{custom}",
                        oninput: move |e| custom.set(e.value()),
                        onkeydown: move |e| if e.key() == Key::Enter {
                            let v = custom().trim().to_string();
                            if !v.is_empty() { set(Some(v)); custom.set(String::new()); }
                        },
                    }
                    button { class: "ghost-btn",
                        onclick: move |_| {
                            let v = custom().trim().to_string();
                            if !v.is_empty() { set(Some(v)); custom.set(String::new()); }
                        },
                        "Use"
                    }
                }
            }
        }
    }
}

/// A miniature Larrier window made of the app's own components and
/// classes, themed exactly like the real thing.
#[component]
fn PreviewPane(dark: bool) -> Element {
    let app = use_app();
    let s = app.config.read().settings.clone();
    let style = scope_style(&s, dark);
    let theme = if dark { "dark" } else { "light" };
    let code_theme = if dark { s.code_theme_dark.clone() } else { s.code_theme_light.clone() };
    let code_css = use_memo(use_reactive!(|code_theme, dark| crate::highlight::scoped_theme_css(code_theme.as_deref(), dark, ".ap-mini.ap-mini .ap-code.ap-code")));
    let md = use_hook(|| crate::markdown::to_html(SAMPLE_MD));
    let mut cls = String::from("app ap-mini");
    if s.density == Density::Compact {
        cls.push_str(" compact");
    }
    rsx! {
        style { "{code_css}" }
        div { class: "ap-frame",
            div { key: "{style_key(&style)}", class: "{cls}", "data-theme": "{theme}", style: "{style}",
                div { class: if s.sidebar_right { "ap-m-body rev" } else { "ap-m-body" },
                    div { class: "ap-m-side",
                        div { class: "ap-m-top",
                            span { class: "icon-btn", Icon { i: I::PanelLeft, size: 15, stroke: 1.5 } }
                            span { class: "icon-btn", Icon { i: I::Search, size: 14, stroke: 1.5 } }
                        }
                        div { class: "side-nav",
                            div { class: "nav-row", span { class: "ic", Icon { i: I::SquarePen, size: 15, stroke: 1.5 } } "New Agent" }
                            div { class: "nav-row", span { class: "ic", Icon { i: I::CalendarClock, size: 15, stroke: 1.5 } } "Automations" }
                        }
                        div { class: "ap-m-sec", "Repositories" }
                        div { class: "repo-row", span { class: "ic", Icon { i: I::FolderOpen, size: 15, stroke: 1.5 } } span { class: "nm", "larrier" } }
                        div { class: "chat-row selected", span { class: "bullet", span { class: "pt" } } span { class: "t", "Refactor parser" } span { class: "tm", "2m" } }
                        div { class: "chat-row", span { class: "bullet", super::Spinner { size: 13 } } span { class: "t", "Add dark mode" } span { class: "tm", "now" } }
                        div { class: "chat-sub", "Editing theme.rs" }
                        div { class: "chat-row unread", span { class: "bullet", span { class: "pt" } } span { class: "t", "Fix flaky test" } span { class: "tm", "1h" } }
                        div { class: "chat-row review unread", span { class: "bullet", span { class: "pt" } } span { class: "t", "Ship v0.4" } span { class: "tm", "3h" } }
                    }
                    div { class: "ap-m-main",
                        div { class: "ap-m-head",
                            div { class: "title-chip", span { class: "t", "Refactor parser" } span { class: "ic", Icon { i: I::Laptop, size: 13, stroke: 1.5 } } }
                            div { class: "grow" }
                            span { class: "head-link", "IDE" Icon { i: I::ArrowUpRight, size: 11 } }
                        }
                        div { class: "ap-m-scroll",
                            div { class: "user-msg", div { class: "user-card expanded", "Make the settings page responsive and follow the theme" } }
                            div { class: "step", span { class: "verb", "Explored" } span { class: "target", "4 files, 2 searches" } }
                            div { class: "a-text md ap-code", dangerous_inner_html: "{md}" }
                            div { class: "tool-card",
                                div { class: "tool-card-head",
                                    span { class: "ic", Icon { i: I::Lines, size: 13 } }
                                    span { class: "name", "settings.rs" }
                                    span { class: "stats", span { class: "add", "+12" } span { class: "del", "-3" } }
                                }
                                div { class: "preview open",
                                    div { class: "pv-line ctx", "  fn render(&self) -> Element {{" }
                                    div { class: "pv-line del", "-     let cols = 2;" }
                                    div { class: "pv-line add", "+     let cols = if self.narrow {{ 1 }} else {{ 2 }};" }
                                    div { class: "pv-line add", "+     let theme = self.theme();" }
                                }
                            }
                            div { class: "ap-m-actions",
                                span { class: "pill-btn", "Changes" span { class: "add num", "+12" } span { class: "del num", "-3" } }
                                a { "View diff" }
                            }
                        }
                        div { class: "ap-m-composer",
                            div { class: "composer",
                                div { class: "composer-row",
                                    span { class: "plus-btn", Icon { i: I::Plus, size: 15 } }
                                    span { class: "ap-m-ph", "Send follow-up" }
                                    span { class: "picker", span { class: "lb sub", "Claude" } span { class: "lb", "Auto" } span { class: "chev", Icon { i: I::ChevronDown, size: 11 } } }
                                    span { class: "send-btn ready", Icon { i: I::ArrowUp, size: 13, stroke: 2.2 } }
                                }
                            }
                            div { class: "ap-m-env",
                                span { class: "env-chip", Icon { i: I::GitBranch, size: 12 } "main" }
                                span { class: "env-chip", Icon { i: I::Laptop, size: 12 } "This PC" }
                                div { class: "grow" }
                                span { class: "toggle on" }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_css_colors() {
        assert_eq!(parse_color("#fff"), Some((255, 255, 255, 1.0)));
        assert_eq!(hex6("#26251E").as_deref(), Some("#26251e"));
        assert_eq!(hex6("rgba(38, 37, 30, 0.1)").as_deref(), Some("#26251e"));
        let (_, _, _, a) = parse_color("rgb(1 2 3 / 0.5)").unwrap();
        assert!((a - 0.5).abs() < 1e-6);
        assert!(parse_color("#ggg").is_none());
        assert!(parse_color("#ééé").is_none());
    }

    #[test]
    fn rejects_unsafe_values() {
        assert!(valid_css_color("rgba(0, 0, 0, 0.4)"));
        assert!(!valid_css_color("red; background: url(x)"));
        assert!(!valid_css_color(""));
    }

    #[test]
    fn presets_cover_both_themes_with_valid_values() {
        assert!(preset_colors(&PRESETS[0]).is_empty(), "the built-in preset clears overrides");
        for p in &PRESETS[1..] {
            let m = preset_colors(p);
            assert!(m.contains_key("light.win") && m.contains_key("dark.fg"), "{}", p.name);
            assert!(m.values().all(|v| valid_css_color(v)), "{}", p.name);
        }
    }
}
