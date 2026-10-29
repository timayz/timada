//! A page's body, from the Markdown the merchant writes to the HTML a
//! storefront shows.
//!
//! CommonMark with tables and strikethrough. Raw HTML in the source is shown
//! as text, never passed through; the HTML that comes out is then cleaned by
//! an allow-list sanitiser, so a link to `javascript:` or an attribute the
//! parser let by never reaches a page. A picture is one of the page's own,
//! written by its file name (`![The workshop](01J….webp)`) and served from
//! the shop's media address; any other picture is shown as its description.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd, html};

use crate::value_object::is_image_file;

/// The HTML of a page's body. `media_url` is where the shop's pictures are
/// served from, with no trailing slash (`https://media.example.com/acme`);
/// empty leaves a picture's bare file name, as a storefront with no media
/// address does for its products.
pub fn render_markdown(source: &str, media_url: &str) -> String {
    let mut dropped_image = false;
    let events = Parser::new_ext(source, options()).filter_map(|event| match event {
        Event::Start(Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        }) => {
            if is_image_file(&dest_url) {
                Some(Event::Start(Tag::Image {
                    link_type,
                    dest_url: if media_url.is_empty() {
                        dest_url
                    } else {
                        format!("{media_url}/{dest_url}").into()
                    },
                    title,
                    id,
                }))
            } else {
                // Its description stays, as text.
                dropped_image = true;
                None
            }
        }
        Event::End(TagEnd::Image) if dropped_image => {
            dropped_image = false;
            None
        }
        Event::Html(raw) | Event::InlineHtml(raw) => Some(Event::Text(raw)),
        event => Some(event),
    });
    let mut out = String::with_capacity(source.len() * 3 / 2);
    html::push_html(&mut out, events);
    ammonia::Builder::default()
        .link_rel(Some("noopener noreferrer"))
        .clean(&out)
        .to_string()
}

/// A page's body as plain words, the Markdown taken off: for a search to
/// match and a description to quote.
pub fn markdown_text(source: &str) -> String {
    let mut text = String::with_capacity(source.len());
    for event in Parser::new_ext(source, options()) {
        match event {
            Event::Text(words) | Event::Code(words) => text.push_str(&words),
            Event::SoftBreak | Event::HardBreak => text.push(' '),
            Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::Item
                | TagEnd::TableCell
                | TagEnd::BlockQuote(_),
            ) => text.push(' '),
            _ => {}
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEDIA: &str = "https://media.example.com/acme";

    #[test]
    fn renders_headings_lists_links_and_tables() {
        let html = render_markdown(
            "## Retours\n\n- 14 jours\n- **gratuits**\n\n[Contact](/support-client)\n\n| a | b |\n|---|---|\n| 1 | 2 |",
            MEDIA,
        );
        assert!(html.contains("<h2>Retours</h2>"), "{html}");
        assert!(
            html.contains("<li><strong>gratuits</strong></li>"),
            "{html}"
        );
        assert!(
            html.contains(r#"<a href="/support-client" rel="noopener noreferrer">Contact</a>"#),
            "{html}"
        );
        assert!(html.contains("<td>2</td>"), "{html}");
    }

    #[test]
    fn raw_html_is_text() {
        let html = render_markdown(
            "<script>alert(1)</script>\n\nHi <b onclick=x>there</b>",
            MEDIA,
        );
        assert!(!html.contains("<script"), "{html}");
        assert!(!html.contains("<b"), "{html}");
        assert!(html.contains("&lt;script&gt;"), "{html}");
    }

    #[test]
    fn script_links_are_cleaned() {
        let html = render_markdown("[x](javascript:alert(1))", MEDIA);
        assert!(!html.contains("javascript"), "{html}");
    }

    #[test]
    fn own_pictures_are_served_from_the_media_address() {
        let html = render_markdown("![The workshop](01JBX0V7Y5B6V9H8K3QWERTYAS.webp)", MEDIA);
        assert!(
            html.contains(
                r#"<img src="https://media.example.com/acme/01JBX0V7Y5B6V9H8K3QWERTYAS.webp" alt="The workshop">"#
            ),
            "{html}"
        );
    }

    #[test]
    fn other_pictures_leave_their_description() {
        let html = render_markdown("![A tracker](https://evil.example/pixel.gif)", MEDIA);
        assert!(!html.contains("<img"), "{html}");
        assert!(html.contains("A tracker"), "{html}");
    }

    #[test]
    fn plain_text_drops_the_markup() {
        assert_eq!(
            markdown_text("# Title\n\nSome *words*,\nand [a link](/x).\n\n- one\n- two"),
            "Title Some words, and a link. one two"
        );
    }
}
