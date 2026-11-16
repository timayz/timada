//! A page's body, from the Markdown the merchant writes to the HTML a
//! storefront shows.
//!
//! CommonMark with tables and strikethrough. Raw HTML in the source is shown
//! as text, never passed through; the HTML that comes out is then cleaned by
//! an allow-list sanitiser, so a link to `javascript:` or an attribute the
//! parser let by never reaches a page. A picture is one of the page's own,
//! written by its file name (`![The workshop](01J….webp)`) and served from
//! the shop's media address; any other picture is shown as its description.
//!
//! An article of the blog is the same Markdown, and two things more
//! ([`render_article`]): its headings carry an id each, for a table of
//! contents, and a line of its own may name a product or a category for
//! the storefront to show there.

use std::collections::HashSet;

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd, html};

use timada_core::Rendition;

use crate::value_object::is_image_file;

/// The HTML of a page's body. `media_url` is where the shop's pictures are
/// served from, with no trailing slash (`https://media.example.com/acme`);
/// empty leaves a picture's bare file name, as a storefront with no media
/// address does for its products.
pub fn render_markdown(source: &str, media_url: &str) -> String {
    let mut out = String::with_capacity(source.len() * 3 / 2);
    html::push_html(&mut out, events(source, media_url).into_iter());
    ammonia::Builder::default()
        .link_rel(Some("noopener noreferrer"))
        .clean(&out)
        .to_string()
}

/// What the parser reads in `source`, as a body shows it: raw HTML as
/// text, a picture of the shop's own from its media address, any other
/// picture as its description.
fn events<'a>(source: &'a str, media_url: &str) -> Vec<Event<'a>> {
    let mut dropped_image = false;
    Parser::new_ext(source, options())
        .filter_map(|event| match event {
            Event::Start(Tag::Image {
                link_type,
                dest_url,
                title,
                id,
            }) => {
                if is_image_file(&dest_url) {
                    Some(Event::Start(Tag::Image {
                        link_type,
                        // The body names the picture; what is served is its
                        // page rendition.
                        dest_url: if media_url.is_empty() {
                            dest_url
                        } else {
                            format!("{media_url}/{}/{dest_url}", Rendition::Page.dir()).into()
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
        })
        .collect()
}

/// What an article's body is made of, in order: its text, and the products
/// and categories it shows in between. A line of its own reading
/// `[[product:blue-mug]]` or `[[category:mugs]]` names one by its slug; the
/// storefront draws it as it draws its own cards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArticleBlock {
    /// Text as HTML, cleaned: safe to write into a page unescaped.
    Html(String),
    /// The slug of a product to show.
    Product(String),
    /// The slug of a category to show.
    Category(String),
}

/// A heading of an article, for its table of contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArticleHeading {
    /// 2 for `##`, 3 for `###`.
    pub level: u8,
    /// The `id` the heading carries in the HTML: `#{id}` goes to it.
    pub id: String,
    pub text: String,
}

/// An article's body, ready to show.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenderedArticle {
    pub blocks: Vec<ArticleBlock>,
    /// The `##` and `###` headings, in order.
    pub headings: Vec<ArticleHeading>,
}

/// What every heading id starts with, so none takes an id the page around
/// the article uses for itself.
const HEADING_ID_PREFIX: &str = "s-";

/// An article's body as the storefront shows it: the HTML [`render_markdown`]
/// gives, its `##` and `###` headings carrying an id each for a table of
/// contents, cut where the body shows a product or a category.
pub fn render_article(source: &str, media_url: &str) -> RenderedArticle {
    let mut rendered = RenderedArticle::default();
    let mut taken = HashSet::new();
    for piece in pieces(source) {
        match piece {
            Piece::Markdown(text) => {
                let html = render_piece(text, media_url, &mut rendered.headings, &mut taken);
                if !html.trim().is_empty() {
                    rendered.blocks.push(ArticleBlock::Html(html));
                }
            }
            Piece::Product(slug) => rendered.blocks.push(ArticleBlock::Product(slug.to_owned())),
            Piece::Category(slug) => rendered
                .blocks
                .push(ArticleBlock::Category(slug.to_owned())),
        }
    }
    rendered
}

/// One stretch of Markdown between two of an article's blocks: its
/// headings are given their ids and noted in `headings`.
fn render_piece(
    source: &str,
    media_url: &str,
    headings: &mut Vec<ArticleHeading>,
    taken: &mut HashSet<String>,
) -> String {
    let mut events = events(source, media_url);
    let mut at = 0;
    while at < events.len() {
        let level = match &events[at] {
            Event::Start(Tag::Heading {
                level: HeadingLevel::H2,
                ..
            }) => 2,
            Event::Start(Tag::Heading {
                level: HeadingLevel::H3,
                ..
            }) => 3,
            _ => {
                at += 1;
                continue;
            }
        };
        let mut text = String::new();
        for event in &events[at + 1..] {
            match event {
                Event::End(TagEnd::Heading(_)) => break,
                Event::Text(words) | Event::Code(words) => text.push_str(words),
                Event::SoftBreak | Event::HardBreak => text.push(' '),
                _ => {}
            }
        }
        let base = timada_core::slugify(&text, crate::MAX_SLUG_LEN, "section");
        let mut id = format!("{HEADING_ID_PREFIX}{base}");
        let mut n = 2;
        while !taken.insert(id.clone()) {
            id = format!("{HEADING_ID_PREFIX}{base}-{n}");
            n += 1;
        }
        if let Event::Start(Tag::Heading { id: slot, .. }) = &mut events[at] {
            *slot = Some(id.clone().into());
        }
        headings.push(ArticleHeading { level, id, text });
        at += 1;
    }
    let mut out = String::with_capacity(source.len() * 3 / 2);
    html::push_html(&mut out, events.into_iter());
    ammonia::Builder::default()
        .link_rel(Some("noopener noreferrer"))
        .add_tag_attributes("h2", &["id"])
        .add_tag_attributes("h3", &["id"])
        .clean(&out)
        .to_string()
}

/// A stretch of an article's source.
enum Piece<'a> {
    Markdown(&'a str),
    Product(&'a str),
    Category(&'a str),
}

/// Cuts an article's source at the paragraphs that name a product or a
/// category and nothing else. Only a paragraph of the body itself counts:
/// one quoted, listed or shown as code is text.
fn pieces(source: &str) -> Vec<Piece<'_>> {
    let mut pieces = Vec::new();
    let mut from = 0;
    let mut depth = 0usize;
    for (event, range) in Parser::new_ext(source, options()).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                if depth == 0
                    && matches!(tag, Tag::Paragraph)
                    && let Some(block) = embed(source[range.clone()].trim())
                {
                    pieces.push(Piece::Markdown(&source[from..range.start]));
                    pieces.push(block);
                    from = range.end;
                }
                depth += 1;
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    pieces.push(Piece::Markdown(&source[from..]));
    pieces
}

/// What a paragraph reading `[[product:slug]]` or `[[category:slug]]`
/// names.
fn embed(paragraph: &str) -> Option<Piece<'_>> {
    let (kind, slug) = paragraph
        .strip_prefix("[[")?
        .strip_suffix("]]")?
        .split_once(':')?;
    let slug = slug.trim();
    let is_slug = !slug.is_empty()
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    match (kind.trim(), is_slug) {
        ("product", true) => Some(Piece::Product(slug)),
        ("category", true) => Some(Piece::Category(slug)),
        _ => None,
    }
}

/// An article's body as plain words, as [`markdown_text`] gives a page's,
/// the products and categories it shows left out.
pub fn article_text(source: &str) -> String {
    pieces(source)
        .into_iter()
        .filter_map(|piece| match piece {
            Piece::Markdown(text) => Some(markdown_text(text)),
            _ => None,
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
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
                r#"<img src="https://media.example.com/acme/1200/01JBX0V7Y5B6V9H8K3QWERTYAS.webp" alt="The workshop">"#
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
    fn an_article_is_cut_at_its_products_and_lists_its_headings() {
        let rendered = render_article(
            "Intro.\n\n## Choisir un tapis\n\nTexte.\n\n[[product:tapis-bleu]]\n\n### Les tailles\n\n\
             [[category: tapis ]]\n\n## Choisir un tapis\n\nFin.",
            MEDIA,
        );
        let [
            ArticleBlock::Html(first),
            ArticleBlock::Product(product),
            ArticleBlock::Html(second),
            ArticleBlock::Category(category),
            ArticleBlock::Html(third),
        ] = rendered.blocks.as_slice()
        else {
            panic!("{:#?}", rendered.blocks);
        };
        assert!(
            first.contains(r#"<h2 id="s-choisir-un-tapis">Choisir un tapis</h2>"#),
            "{first}"
        );
        assert_eq!(product, "tapis-bleu");
        assert!(second.contains(r#"<h3 id="s-les-tailles">"#), "{second}");
        assert_eq!(category, "tapis");
        // The same heading twice: two ids.
        assert!(
            third.contains(r#"<h2 id="s-choisir-un-tapis-2">"#),
            "{third}"
        );
        let toc: Vec<(u8, &str, &str)> = rendered
            .headings
            .iter()
            .map(|h| (h.level, h.id.as_str(), h.text.as_str()))
            .collect();
        assert_eq!(
            toc,
            [
                (2, "s-choisir-un-tapis", "Choisir un tapis"),
                (3, "s-les-tailles", "Les tailles"),
                (2, "s-choisir-un-tapis-2", "Choisir un tapis"),
            ]
        );
    }

    #[test]
    fn a_product_named_inside_other_text_is_text() {
        let source = "Voir [[product:tapis]] ici.\n\n```\n[[product:tapis]]\n```\n\n\
                      > [[product:tapis]]\n\n[[produit:tapis]]\n\n[[product:Not a slug]]";
        let rendered = render_article(source, MEDIA);
        assert_eq!(rendered.blocks.len(), 1, "{:#?}", rendered.blocks);
        assert!(
            matches!(&rendered.blocks[0], ArticleBlock::Html(html) if html.contains("[[product:tapis]]"))
        );
    }

    #[test]
    fn an_article_keeps_no_id_the_merchant_wrote() {
        let rendered = render_article("## Titre {#cart}\n\n<h2 id=\"cart\">x</h2>", MEDIA);
        let ArticleBlock::Html(html) = &rendered.blocks[0] else {
            panic!();
        };
        assert!(html.contains(r#"<h2 id="s-titre-cart">"#), "{html}");
        assert!(!html.contains(r#"<h2 id="cart""#), "{html}");
    }

    #[test]
    fn an_articles_text_leaves_its_products_out() {
        assert_eq!(
            article_text("Un *tapis*.\n\n[[product:tapis-bleu]]\n\nEt la suite."),
            "Un tapis. Et la suite."
        );
    }

    #[test]
    fn plain_text_drops_the_markup() {
        assert_eq!(
            markdown_text("# Title\n\nSome *words*,\nand [a link](/x).\n\n- one\n- two"),
            "Title Some words, and a link. one two"
        );
    }
}
