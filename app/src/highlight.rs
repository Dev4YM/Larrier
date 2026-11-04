//! Syntax highlighting with syntect: classed HTML (`hl-*` classes) plus CSS
//! for the light and dark app themes.

use std::sync::OnceLock;

use syntect::highlighting::ThemeSet;
use syntect::html::{css_for_theme_with_class_style, line_tokens_to_classed_spans, ClassStyle};
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};

const STYLE: ClassStyle = ClassStyle::SpacedPrefixed { prefix: "hl-" };
const PREFIX: &str = "hl-";
/// Don't highlight more than this (the viewer falls back to plain text).
const MAX_BYTES: usize = 400 * 1024;
const MAX_LINE: usize = 2000;

pub const DEFAULT_LIGHT: &str = "InspiredGitHub";
pub const DEFAULT_DARK: &str = "base16-ocean.dark";

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn themes_set() -> &'static ThemeSet {
    static SET: OnceLock<ThemeSet> = OnceLock::new();
    SET.get_or_init(ThemeSet::load_defaults)
}

/// Load the syntax and theme sets on a background thread so the first
/// highlighted block doesn't stall the UI.
pub fn warm_up() {
    std::thread::spawn(|| {
        let _ = syntaxes();
        let _ = themes_set();
    });
}

/// Names of the bundled code themes (for the appearance settings).
pub fn themes() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = themes_set().themes.keys().map(|k| k.as_str()).collect();
    v.sort_unstable();
    v
}

/// Map a fence info string, file name or extension to a bundled syntax.
fn find_syntax(lang: &str) -> Option<&'static SyntaxReference> {
    let ss = syntaxes();
    let raw = lang.trim();
    if raw.is_empty() {
        return None;
    }
    // Fence info strings may carry extras: "rust,ignore", "js title=x".
    let first = raw.split(|c: char| c == ',' || c.is_whitespace()).next().unwrap_or(raw);
    let name = first.rsplit(['/', '\\']).next().unwrap_or(first);
    let lower = name.to_ascii_lowercase();

    // Whole file names.
    let by_name = match lower.as_str() {
        "dockerfile" | "containerfile" | ".env" | ".bashrc" | ".zshrc" | ".profile" | "procfile" => Some("sh"),
        "makefile" | "gnumakefile" => Some("makefile"),
        "cmakelists.txt" => Some("cmake"),
        "cargo.lock" | ".gitconfig" | ".editorconfig" => Some("ini"),
        "gemfile" | "rakefile" => Some("rb"),
        _ if lower.starts_with(".env.") || lower.ends_with(".env") => Some("sh"),
        _ => None,
    };
    let ext = by_name.map(|s| s.to_string()).unwrap_or_else(|| match lower.rsplit_once('.') {
        Some((_, e)) => e.to_string(),
        None => lower.clone(),
    });

    // Aliases for languages the default set lacks or names differently.
    let alias = match ext.as_str() {
        "ts" | "tsx" | "typescript" | "mts" | "cts" => "js",
        "jsx" | "mjs" | "cjs" | "javascript" | "node" => "js",
        "csproj" | "fsproj" | "vbproj" | "props" | "targets" | "config" | "xaml" | "resx" | "svg" | "plist" | "xsd" | "nuspec" => "xml",
        "zsh" | "bash" | "shell" | "console" | "shellscript" | "env" => "sh",
        "ps1" | "psm1" | "psd1" | "powershell" | "pwsh" => "ps1",
        "kt" | "kts" | "kotlin" => "java",
        "scss" | "sass" | "less" => "css",
        "vue" | "svelte" | "htm" | "xhtml" => "html",
        "yml" => "yaml",
        "rs" => "rs",
        "py" | "pyw" | "pyi" | "python" => "py",
        "rb" | "ruby" => "rb",
        "golang" => "go",
        "c#" | "csharp" | "cs" => "cs",
        "c++" | "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => "cpp",
        "objc" | "objective-c" => "m",
        "md" | "markdown" | "mdx" => "md",
        "jsonc" | "json5" | "geojson" | "ipynb" => "json",
        "toml" | "ini" | "cfg" | "conf" | "properties" => "ini",
        "sql" | "psql" | "mysql" | "tsql" => "sql",
        "bat" | "cmd" => "bat",
        "diff" | "patch" => "diff",
        "tex" | "latex" => "tex",
        other => other,
    };
    ss.find_syntax_by_token(alias)
        .or_else(|| ss.find_syntax_by_extension(alias))
        .or_else(|| ss.find_syntax_by_token(&ext))
        .or_else(|| ss.find_syntax_by_extension(&ext))
        .or_else(|| ss.find_syntax_by_name(first))
        .or_else(|| fallback_syntax(alias))
        .filter(|s| s.name != "Plain Text")
}

/// Closest available grammar for languages the defaults don't ship.
fn fallback_syntax(alias: &str) -> Option<&'static SyntaxReference> {
    let ss = syntaxes();
    let near = match alias {
        "ini" | "toml" => "yaml",
        "ps1" => "sh",
        "swift" | "dart" => "java",
        "cmake" => "makefile",
        _ => return None,
    };
    ss.find_syntax_by_token(near)
}

/// Whether `lang` (fence info / file name) maps to a known syntax.
pub fn knows(lang: &str) -> bool {
    find_syntax(lang).is_some()
}

/// Human name of the syntax used for `lang` (e.g. "Rust").
pub fn syntax_name(lang: &str) -> Option<&'static str> {
    find_syntax(lang).map(|s| s.name.as_str())
}

fn open_spans_for(stack: &ScopeStack) -> String {
    let mut out = String::new();
    for scope in stack.as_slice() {
        out.push_str("<span class=\"");
        push_classes(&mut out, *scope);
        out.push_str("\">");
    }
    out
}

fn push_classes(out: &mut String, scope: Scope) {
    let s = scope.build_string();
    for (i, atom) in s.split('.').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(PREFIX);
        out.push_str(atom);
    }
}

/// One self-contained HTML string per source line (every line opens and
/// closes its own spans), so viewers can render line numbers and only the
/// visible window. `None` when the language is unknown or the input is too
/// large to highlight.
pub fn highlight_lines(code: &str, lang: Option<&str>) -> Option<Vec<String>> {
    if code.len() > MAX_BYTES {
        return None;
    }
    let syntax = find_syntax(lang?)?;
    if code.lines().any(|l| l.len() > MAX_LINE) {
        return None;
    }
    let ss = syntaxes();
    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut out = Vec::with_capacity(code.lines().count() + 1);
    for line in code.split_inclusive('\n') {
        let ops = state.parse_line(line, ss).ok()?;
        let prefix = open_spans_for(&stack);
        let before = stack.len() as isize;
        let (html, delta) = line_tokens_to_classed_spans(line, &ops, STYLE, &mut stack).ok()?;
        let mut s = prefix;
        s.push_str(html.trim_end_matches(['\n', '\r']));
        let open = (before + delta).max(0);
        for _ in 0..open {
            s.push_str("</span>");
        }
        out.push(s);
    }
    if code.is_empty() || code.ends_with('\n') {
        // Keep the line count consistent with `str::lines`.
        if code.ends_with('\n') && out.len() > code.lines().count() {
            out.truncate(code.lines().count());
        }
    }
    Some(out)
}

/// Highlighted HTML for a whole block (lines joined with `\n`), or `None`.
pub fn highlight_html(code: &str, lang: Option<&str>) -> Option<String> {
    highlight_lines(code, lang).map(|l| l.join("\n"))
}

/// Escape text for HTML (used for plain fallbacks).
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// Theme CSS scoped under `scope`, without background colors (panels keep
/// the app's own backgrounds).
fn scoped_css(theme_name: &str, scope: &str) -> String {
    let ts = themes_set();
    let Some(theme) = ts.themes.get(theme_name) else { return String::new() };
    let Ok(raw) = css_for_theme_with_class_style(theme, STYLE) else { return String::new() };
    let mut out = String::with_capacity(raw.len() + raw.len() / 3);
    // Strip comments, then rewrite every rule `sel1, sel2 { … }`.
    let mut text = String::with_capacity(raw.len());
    let mut rest = raw.as_str();
    while let Some(i) = rest.find("/*") {
        text.push_str(&rest[..i]);
        match rest[i..].find("*/") {
            Some(j) => rest = &rest[i + j + 2..],
            None => {
                rest = "";
                break;
            }
        }
    }
    text.push_str(rest);
    for rule in text.split('}') {
        let Some((sel, body)) = rule.split_once('{') else { continue };
        let sel = sel.trim();
        if sel.is_empty() {
            continue;
        }
        let body: String = body
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty() && !l.starts_with("background-color"))
            .collect::<Vec<_>>()
            .join(" ");
        if body.is_empty() {
            continue;
        }
        let scoped: Vec<String> = sel.split(',').map(|s| format!("{scope} {}", s.trim())).collect();
        out.push_str(&scoped.join(", "));
        out.push_str(" { ");
        out.push_str(&body);
        out.push_str(" }\n");
    }
    out
}

/// CSS for the default light + dark code themes.
pub fn css() -> String {
    css_for(None, None)
}

/// CSS for the given code themes (falls back to the defaults for unknown
/// or missing names).
pub fn css_for(light: Option<&str>, dark: Option<&str>) -> String {
    let ts = themes_set();
    let pick = |name: Option<&str>, default: &'static str| -> String {
        match name {
            Some(n) if ts.themes.contains_key(n) => n.to_string(),
            _ => default.to_string(),
        }
    };
    let mut css = scoped_css(&pick(light, DEFAULT_LIGHT), ".app[data-theme=\"light\"]");
    css.push_str(&scoped_css(&pick(dark, DEFAULT_DARK), ".app[data-theme=\"dark\"]"));
    css
}

/// The code editor's token colors from code theme `name` (the light or
/// dark one): each kind of token Monaco's colorers name gets the color the
/// theme gives its TextMate scope, so the editor reads like the chat's code.
pub fn monaco_rules(name: Option<&str>, dark: bool) -> serde_json::Value {
    use syntect::highlighting::{Color, FontStyle, Highlighter};
    const KINDS: &[(&str, &str)] = &[
        ("comment", "comment.line"),
        ("comment.doc", "comment.block.documentation"),
        ("string", "string.quoted.double"),
        ("string.escape", "constant.character.escape"),
        ("string.key.json", "support.type.property-name.json"),
        ("string.value.json", "string.quoted.double.json"),
        ("regexp", "string.regexp"),
        ("number", "constant.numeric"),
        ("constant", "constant.language"),
        ("keyword", "keyword.control"),
        ("keyword.json", "constant.language.json"),
        ("operator", "keyword.operator"),
        ("type", "entity.name.type"),
        ("type.identifier", "entity.name.type"),
        ("tag", "entity.name.tag"),
        ("metatag", "meta.tag.preprocessor"),
        ("annotation", "storage.type.annotation"),
        ("attribute.name", "entity.other.attribute-name"),
        ("attribute.value", "string.quoted.double"),
        ("predefined", "support.function"),
        ("variable", "variable.other"),
        ("variable.predefined", "variable.language"),
        ("namespace", "entity.name.namespace"),
        ("key", "entity.name.tag.yaml"),
        ("delimiter", "punctuation.separator"),
    ];
    let ts = themes_set();
    let pick = match name {
        Some(n) if ts.themes.contains_key(n) => n,
        _ if dark => DEFAULT_DARK,
        _ => DEFAULT_LIGHT,
    };
    let theme = &ts.themes[pick];
    let h = Highlighter::new(theme);
    let hex = |c: Color| format!("{:02x}{:02x}{:02x}", c.r, c.g, c.b);
    let fg = theme.settings.foreground.map(hex);
    let mut rules = vec![serde_json::json!({ "token": "", "foreground": fg })];
    for (token, scope) in KINDS {
        let stack: Vec<Scope> = Scope::new(scope).into_iter().collect();
        let st = h.style_for_stack(&stack);
        let mut style = Vec::new();
        if st.font_style.contains(FontStyle::ITALIC) {
            style.push("italic");
        }
        if st.font_style.contains(FontStyle::BOLD) {
            style.push("bold");
        }
        rules.push(serde_json::json!({ "token": token, "foreground": hex(st.foreground), "fontStyle": style.join(" ") }));
    }
    serde_json::json!({ "rules": rules, "selection": theme.settings.selection.map(hex) })
}

/// One code theme scoped under `scope` — lets the Appearance preview show
/// the light or dark code theme regardless of the window's current theme.
pub fn scoped_theme_css(name: Option<&str>, dark: bool, scope: &str) -> String {
    let ts = themes_set();
    let pick = match name {
        Some(n) if ts.themes.contains_key(n) => n,
        _ if dark => DEFAULT_DARK,
        _ => DEFAULT_LIGHT,
    };
    scoped_css(pick, scope)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_rust_lines() {
        let code = "fn main() {\n    let s = \"hi\"; // greet\n}\n";
        let lines = highlight_lines(code, Some("rust")).expect("rust is known");
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("hl-"));
        // Every line balances its own spans.
        for l in &lines {
            assert_eq!(l.matches("<span").count(), l.matches("</span>").count(), "{l}");
        }
        assert!(lines[1].contains("hl-comment"));
    }

    #[test]
    fn multiline_comment_spans_close_per_line() {
        let code = "/* a\n b\n c */ let x = 1;";
        let lines = highlight_lines(code, Some("js")).unwrap();
        assert_eq!(lines.len(), 3);
        for l in &lines {
            assert_eq!(l.matches("<span").count(), l.matches("</span>").count(), "{l}");
        }
        assert!(lines[1].contains("hl-comment"));
    }

    #[test]
    fn languages_and_files() {
        for l in ["rust", "rs", "py", "python", "ts", "tsx", "jsx", "json", "yaml", "toml", "sql", "cs", "go", "java", "kt", "html", "css", "scss", "md", "sh", "bash", "c", "cpp", "php", "rb", "Dockerfile", "Makefile", ".env", "Web.config", "app.csproj", "src/main.rs", "ps1"] {
            assert!(knows(l), "no syntax for {l}");
        }
        assert!(!knows("text"));
        assert!(!knows(""));
        assert_eq!(syntax_name("main.rs"), Some("Rust"));
        assert_eq!(syntax_name("Web.config"), Some("XML"));
    }

    #[test]
    fn escapes_source_text() {
        let lines = highlight_lines("<script>alert(1)</script>", Some("rust")).unwrap();
        assert!(!lines[0].contains("<script"));
        assert!(lines[0].contains("&lt;"));
        assert!(lines[0].contains("&gt;"));
    }

    #[test]
    fn limits_and_unknowns() {
        assert!(highlight_lines("x", Some("not-a-language")).is_none());
        assert!(highlight_lines("x", None).is_none());
        let long = "a".repeat(MAX_LINE + 1);
        assert!(highlight_lines(&long, Some("rust")).is_none());
    }

    #[test]
    fn css_is_scoped_without_backgrounds() {
        let c = css();
        assert!(c.contains(".app[data-theme=\"light\"] .hl-"));
        assert!(c.contains(".app[data-theme=\"dark\"] .hl-"));
        assert!(!c.contains("background-color"));
        assert!(themes().contains(&"InspiredGitHub"));
        assert!(themes().contains(&"base16-ocean.dark"));
        let custom = css_for(Some("Solarized (light)"), Some("nope"));
        assert!(custom.contains(".app[data-theme=\"dark\"]"));
    }
}
