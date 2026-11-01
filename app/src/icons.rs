//! Icons: the complete Lucide set (ISC) as one inline SVG sprite, plus a
//! typed shortlist (`I`) for the icons the app uses by name.
//!
//! Render `IconSprite` once near the root; `Icon` and `Lucide` then
//! reference symbols with `<use href="#lucide-…">`, inheriting color,
//! stroke width, caps and joins from the referencing `<svg>`.

use dioxus::prelude::*;

/// Every Lucide icon as `<symbol id="lucide-<name>">` (see
/// `assets/tools/build_lucide_sprite.py`).
pub const SPRITE: &str = include_str!("../assets/lucide-sprite.svg");

/// Mounts the sprite (hidden). Render exactly once.
#[component]
pub fn IconSprite() -> Element {
    rsx! {
        div { style: "display:none", "aria-hidden": "true", dangerous_inner_html: SPRITE }
    }
}

macro_rules! icons {
    ($($v:ident => $name:literal),* $(,)?) => {
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub enum I {
            $($v,)*
            // Windows caption glyphs (hand-drawn to match the OS buttons).
            Minimize,
            Maximize,
            Restore,
        }

        impl I {
            /// All sprite-backed icons.
            pub const ALL: &'static [I] = &[$(I::$v,)*];

            /// Lucide icon name.
            pub fn name(self) -> &'static str {
                match self {
                    $(I::$v => $name,)*
                    I::Minimize => "minus",
                    I::Maximize => "square",
                    I::Restore => "copy",
                }
            }
        }
    };
}

icons! {
    Plus => "plus",
    Columns => "columns-2",
    AppWindow => "app-window",
    Link => "link-2",
    Merge => "git-merge",
    PinOff => "pin-off",
    CopyPlus => "copy-plus",
    Search => "search",
    PanelLeft => "panel-left",
    PanelRight => "panel-right",
    X => "x",
    ChevronDown => "chevron-down",
    ChevronRight => "chevron-right",
    ChevronLeft => "chevron-left",
    ChevronsRight => "chevrons-right",
    ChevronsLeft => "chevrons-left",
    Clock => "clock",
    Settings => "settings",
    GitBranch => "git-branch",
    Globe => "globe",
    Terminal => "square-terminal",
    File => "file",
    Folder => "folder",
    FolderOpen => "folder-open",
    Check => "check",
    Spell => "spell-check",
    Code => "code-xml",
    Annotate => "square-dashed-mouse-pointer",
    BookPlus => "book-plus",
    CircleCheck => "circle-check",
    Circle => "circle",
    ArrowUp => "arrow-up",
    ArrowDown => "arrow-down",
    ArrowLeft => "arrow-left",
    ArrowRight => "arrow-right",
    Stop => "square",
    More => "ellipsis",
    Image => "image",
    Paperclip => "paperclip",
    Infinity => "infinity",
    Archive => "archive",
    Trash => "trash-2",
    Pin => "pin",
    Fork => "git-fork",
    Swap => "arrow-right-left",
    Download => "download",
    Upload => "upload",
    Refresh => "rotate-cw",
    External => "external-link",
    Copy => "copy",
    User => "user",
    Key => "key",
    Bot => "bot",
    Message => "message-square",
    Alert => "triangle-alert",
    Mic => "mic",
    List => "list",
    Sun => "sun",
    Moon => "moon",
    PullRequest => "git-pull-request",
    Commit => "git-commit-horizontal",
    Play => "play",
    Pencil => "pencil",
    Plug => "plug",
    Command => "command",
    Info => "info",
    Cpu => "cpu",
    History => "history",
    Undo => "undo-2",
    Layers => "layers",
    Brain => "brain",
    Sparkle => "sparkle",
    Lock => "lock",
    Monitor => "monitor",
    Eye => "eye",
    Loader => "loader",
    Lines => "text",
    Sliders => "sliders-horizontal",
    Home => "house",
    Laptop => "laptop",
    FolderPlus => "folder-plus",
    Filter => "list-filter",
    ArrowUpRight => "arrow-up-right",
    ThumbsUp => "thumbs-up",
    ThumbsDown => "thumbs-down",
    Diff => "diff",
    Files => "files",
    SquarePen => "square-pen",
    Blocks => "blocks",
    CalendarClock => "calendar-clock",
    CalendarDays => "calendar-days",
    CircleUser => "circle-user",
    Camera => "camera",
    MapPin => "map-pin",
    Briefcase => "briefcase",
    Flame => "flame",
    Trophy => "trophy",
    AudioLines => "audio-lines",
    Quote => "message-square-quote",
    Scissors => "scissors",
    ClipboardPaste => "clipboard-paste",
    TextSelect => "text-select",
    CornerDownLeft => "corner-down-left",
    Server => "server",
    Cloud => "cloud",
    Zap => "zap",
    Box => "box",
    Workflow => "workflow",
    Puzzle => "puzzle",
    Wand => "wand-sparkles",
    Share => "share",
    Star => "star",
    Hash => "hash",
    AtSign => "at-sign",
    Slash => "slash",
    Mail => "mail",
    LogOut => "log-out",
    CircleDot => "circle-dot",
    Dot => "dot",
    PanelBottom => "panel-bottom",
    Maximize2 => "maximize-2",
    Minimize2 => "minimize-2",
    Orbit => "orbit",
    Prompt => "terminal",
    Minus => "minus",
    Gauge => "gauge",
    Timer => "timer",
    CheckCheck => "check-check",
    Grip => "grip-vertical",
    Send => "send",
}

/// Hand-drawn Windows caption glyphs.
fn caption_paths(i: I) -> Option<&'static [&'static str]> {
    match i {
        I::Minimize => Some(&["M5 12h14"]),
        I::Maximize => Some(&["M6 5h12a1 1 0 0 1 1 1v12a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1z"]),
        I::Restore => Some(&["M8 8h10a1 1 0 0 1 1 1v10a1 1 0 0 1-1 1H8a1 1 0 0 1-1-1V9a1 1 0 0 1 1-1z", "M10 5h9a1 1 0 0 1 1 1v9"]),
        _ => None,
    }
}

#[component]
pub fn Icon(i: I, #[props(default = 14)] size: u32, #[props(default = 1.6)] stroke: f32) -> Element {
    let filled = matches!(i, I::Stop | I::Play);
    if let Some(paths) = caption_paths(i) {
        return rsx! {
            svg {
                width: "{size}",
                height: "{size}",
                view_box: "0 0 24 24",
                fill: "none",
                stroke: "currentColor",
                stroke_width: "{stroke}",
                stroke_linecap: "round",
                stroke_linejoin: "round",
                for d in paths.iter() {
                    path { d: "{d}" }
                }
            }
        };
    }
    let href = format!("#lucide-{}", i.name());
    rsx! {
        svg {
            width: "{size}",
            height: "{size}",
            view_box: "0 0 24 24",
            fill: if filled { "currentColor" } else { "none" },
            stroke: "currentColor",
            stroke_width: "{stroke}",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            r#use { href: "{href}" }
        }
    }
}

/// Any icon of the full Lucide library by name (e.g. `"git-merge"`).
#[component]
pub fn Lucide(name: String, #[props(default = 14)] size: u32, #[props(default = 1.6)] stroke: f32) -> Element {
    let href = format!("#lucide-{name}");
    rsx! {
        svg {
            width: "{size}",
            height: "{size}",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "{stroke}",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            r#use { href: "{href}" }
        }
    }
}

/// Whether the bundled library has an icon with this name.
pub fn has_icon(name: &str) -> bool {
    SPRITE.contains(&format!("id=\"lucide-{name}\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_in_the_sprite() {
        for i in I::ALL {
            assert!(has_icon(i.name()), "missing lucide-{} for {:?}", i.name(), i);
        }
        assert!(SPRITE.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
        assert!(SPRITE.matches("<symbol ").count() > 1500);
    }
}
