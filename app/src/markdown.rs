//! Markdown → safe HTML for transcript rendering, with highlighted,
//! copyable code blocks.

use pulldown_cmark::{html, CodeBlockKind, CowStr, Event, Options, Parser, Tag, TagEnd};

use crate::highlight;

/// Styles for the code blocks produced by [`to_html`].
pub const CSS: &str = r#"
.code-block { margin: 6px 0 10px; border: 1px solid var(--border); border-radius: 8px; background: var(--card); overflow: hidden; }
.code-block .code-head { display: flex; align-items: center; justify-content: space-between; height: 28px; padding: 0 6px 0 10px; border-bottom: 1px solid var(--border); font-size: 11px; color: var(--fg-3); user-select: none; }
.code-block .code-lang { font-family: var(--mono); text-transform: lowercase; }
.code-block .code-copy { width: 24px; height: 22px; display: inline-flex; align-items: center; justify-content: center; border-radius: 5px; color: var(--fg-3); background: none; border: none; cursor: pointer; opacity: 0; transition: opacity .12s, color .12s; }
.code-block .code-copy.done { opacity: 1; color: var(--green); }
.code-block:hover .code-copy { opacity: 1; }
.code-block .code-copy:hover { background: var(--hover); color: var(--fg-2); }
.md .code-block pre { margin: 0; border: none; border-radius: 0; background: transparent; padding: 8px 12px; overflow-x: auto; }
.md .code-block pre code { font-family: var(--mono); font-size: var(--fs-code, 12px); line-height: var(--lh-code, 19px); background: none; padding: 0; white-space: pre; }
"#;


pub fn to_html(src: &str) -> String {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    let sanitized = Parser::new_ext(src, opts).map(|ev| match ev {
        // Never let agent output inject markup or scripts into the window.
        Event::Html(s) | Event::InlineHtml(s) => Event::Text(s),
        Event::Start(Tag::Link { link_type, dest_url, title, id }) => {
            let safe = sanitize_url(&dest_url);
            Event::Start(Tag::Link { link_type, dest_url: CowStr::from(safe), title, id })
        }
        Event::Start(Tag::Image { link_type, dest_url, title, id }) => {
            let safe = local_image(&dest_url).unwrap_or_else(|| sanitize_url(&dest_url));
            Event::Start(Tag::Image { link_type, dest_url: CowStr::from(safe), title, id })
        }
        other => other,
    });

    // Replace each code block with our own (escaped / highlighted) markup.
    // This runs after sanitizing, so the only raw HTML that reaches the
    // output is what we generate here.
    let mut events: Vec<Event> = Vec::new();
    let mut code: Option<(String, String)> = None; // (info, text)
    for ev in sanitized {
        match ev {
            Event::Start(Tag::CodeBlock(kind)) => {
                let info = match kind {
                    CodeBlockKind::Fenced(i) => i.to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                code = Some((info, String::new()));
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((info, text)) = code.take() {
                    events.push(Event::Html(CowStr::from(code_block(&info, &text))));
                }
            }
            Event::Text(t) if code.is_some() => {
                if let Some((_, buf)) = code.as_mut() {
                    buf.push_str(&t);
                }
            }
            other => {
                if code.is_none() {
                    events.push(other);
                }
            }
        }
    }
    let mut out = String::with_capacity(src.len() * 3 / 2);
    html::push_html(&mut out, events.into_iter());
    out
}

fn code_block(info: &str, text: &str) -> String {
    let lang = info.split(|c: char| c == ',' || c.is_whitespace()).next().unwrap_or("").trim();
    let body = if lang.is_empty() { None } else { highlight::highlight_html(text.trim_end_matches('\n'), Some(lang)) };
    let body = body.unwrap_or_else(|| highlight::escape(text.trim_end_matches('\n')));
    let label = if lang.is_empty() { "text".to_string() } else { highlight::escape(lang) };
    format!(
        "<div class=\"code-block\"><div class=\"code-head\"><span class=\"code-lang\">{label}</span><button class=\"code-copy\" type=\"button\" title=\"Copy\"><svg width=\"13\" height=\"13\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.8\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><use href=\"#lucide-copy\"/></svg></button></div><pre><code class=\"hl-code\">{body}</code></pre></div>\n"
    )
}

/// An image on this computer (`file:///C:/…/chart.png`, `C:\…\shot.png`,
/// `/home/…/plot.png`), inlined: the window may not load local files.
fn local_image(url: &str) -> Option<String> {
    let u = url.trim();
    let path = if u.starts_with("file://") {
        larrier_formats::util::file_url_path(u)
    } else if u.len() > 2 && u.as_bytes()[1] == b':' && u.as_bytes()[0].is_ascii_alphabetic() || u.starts_with('/') && !u.starts_with("//") {
        u.to_string()
    } else {
        return None;
    };
    let mime = larrier_formats::util::image_mime(&path)?;
    let part = larrier_core::Part::Image { mime: Some(mime), source: larrier_core::lsf::MediaSource::Path { path } };
    crate::state::image_src(&part)
}

fn sanitize_url(url: &str) -> String {
    let lower = url.trim().to_ascii_lowercase();
    if lower.starts_with("javascript:") || lower.starts_with("data:text") || lower.starts_with("vbscript:") {
        "#".into()
    } else {
        url.to_string()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn escapes_html() {
        let h = super::to_html("hi <script>alert(1)</script> [x](javascript:alert(1))");
        assert!(!h.contains("<script>"));
        assert!(!h.contains("javascript:"));
    }

    #[test]
    fn markdown_code_blocks_are_highlighted_and_copyable() {
        let h = super::to_html("text\n\n```rust\nfn main() { let x = 1; }\n```\n\nafter");
        assert!(h.contains("class=\"code-block\""));
        assert!(h.contains("<span class=\"code-lang\">rust</span>"));
        assert!(h.contains("code-copy"));
        assert!(h.contains("hl-"));
        assert!(h.contains("after"));
    }

    #[test]
    fn markdown_code_blocks_escape_unknown_and_raw_html() {
        let h = super::to_html("```\n<script>alert(1)</script>\n```");
        assert!(!h.contains("<script>"));
        assert!(h.contains("&lt;script&gt;"));
        assert!(h.contains("<span class=\"code-lang\">text</span>"));
        let h2 = super::to_html("```\"><img src=x onerror=alert(1)>\nx\n```");
        assert!(!h2.contains("<img"));
    }

    #[test]
    fn markdown_inline_code_unchanged() {
        let h = super::to_html("use `cargo build` now");
        assert!(h.contains("<code>cargo build</code>"));
        assert!(!h.contains("code-block"));
    }
}
