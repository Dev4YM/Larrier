//! Markdown → safe HTML for transcript rendering.

use pulldown_cmark::{html, CowStr, Event, Options, Parser, Tag};

pub fn to_html(src: &str) -> String {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    let parser = Parser::new_ext(src, opts).map(|ev| match ev {
        // Never let agent output inject markup or scripts into the window.
        Event::Html(s) | Event::InlineHtml(s) => Event::Text(s),
        Event::Start(Tag::Link { link_type, dest_url, title, id }) => {
            let safe = sanitize_url(&dest_url);
            Event::Start(Tag::Link { link_type, dest_url: CowStr::from(safe), title, id })
        }
        Event::Start(Tag::Image { link_type, dest_url, title, id }) => {
            let safe = sanitize_url(&dest_url);
            Event::Start(Tag::Image { link_type, dest_url: CowStr::from(safe), title, id })
        }
        other => other,
    });
    let mut out = String::with_capacity(src.len() * 3 / 2);
    html::push_html(&mut out, parser);
    out
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
}
