//! The text of a storefront announcement, from what the merchant types to
//! the HTML the top bar shows.
//!
//! One line of plain words with two marks: `**bold**`, and at most one
//! `[link](/path)` to a page of the shop or to an `https://` address.
//! Anything else is text, escaped: no Markdown blocks (a message starting
//! with `-` or `#` stays as typed) and no raw HTML.

use crate::error::PageError;

/// One run of an announcement's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Piece<'a> {
    Text(&'a str),
    Bold(&'a str),
    Link { label: &'a str, href: &'a str },
}

/// Splits a text into its runs. A mark that does not close is text.
pub fn announcement_pieces(text: &str) -> Vec<Piece<'_>> {
    let mut pieces = Vec::new();
    let mut rest = text;
    let mut plain = 0;
    while plain < rest.len() {
        let tail = &rest[plain..];
        let mark = if let Some(inner) = tail.strip_prefix("**") {
            inner
                .find("**")
                .filter(|end| *end > 0)
                .map(|end| (Piece::Bold(&inner[..end]), 2 + end + 2))
        } else if let Some(inner) = tail.strip_prefix('[') {
            inner.find("](").and_then(|label_end| {
                let after = &inner[label_end + 2..];
                after.find(')').filter(|_| label_end > 0).map(|href_end| {
                    (
                        Piece::Link {
                            label: &inner[..label_end],
                            href: &after[..href_end],
                        },
                        1 + label_end + 2 + href_end + 1,
                    )
                })
            })
        } else {
            None
        };
        match mark {
            Some((piece, len)) => {
                if plain > 0 {
                    pieces.push(Piece::Text(&rest[..plain]));
                }
                pieces.push(piece);
                rest = &rest[plain + len..];
                plain = 0;
            }
            None => plain += tail.chars().next().map_or(1, char::len_utf8),
        }
    }
    if !rest.is_empty() {
        pieces.push(Piece::Text(rest));
    }
    pieces
}

/// Whether a link may leave the bar for there: a path of the shop (`/…`,
/// not `//host`) or an `https://` address.
pub fn is_announcement_href(href: &str) -> bool {
    let shop_path = href.starts_with('/') && !href.starts_with("//");
    let https = href
        .strip_prefix("https://")
        .is_some_and(|host| !host.is_empty());
    (shop_path || https) && !href.chars().any(|c| c.is_whitespace() || c == '"')
}

/// Checks the marks of a text: at most one link, to a place
/// [`is_announcement_href`] allows.
pub fn check_announcement_marks(text: &str) -> Result<(), PageError> {
    let links: Vec<&str> = announcement_pieces(text)
        .into_iter()
        .filter_map(|piece| match piece {
            Piece::Link { href, .. } => Some(href),
            _ => None,
        })
        .collect();
    match links.as_slice() {
        [] => Ok(()),
        [href] if is_announcement_href(href) => Ok(()),
        _ => Err(PageError::InvalidAnnouncementLink),
    }
}

/// The HTML of an announcement: safe to write into a page unescaped.
pub fn render_announcement(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 32);
    for piece in announcement_pieces(text) {
        match piece {
            Piece::Text(words) => escape_into(&mut out, words),
            Piece::Bold(words) => {
                out.push_str("<strong>");
                escape_into(&mut out, words);
                out.push_str("</strong>");
            }
            Piece::Link { label, href } if is_announcement_href(href) => {
                out.push_str("<a href=\"");
                escape_into(&mut out, href);
                if href.starts_with('/') {
                    out.push_str("\">");
                } else {
                    out.push_str("\" rel=\"noopener noreferrer\">");
                }
                escape_into(&mut out, label);
                out.push_str("</a>");
            }
            Piece::Link { label, .. } => escape_into(&mut out, label),
        }
    }
    out
}

/// The words of an announcement, the marks taken off.
pub fn announcement_text(text: &str) -> String {
    announcement_pieces(text)
        .into_iter()
        .map(|piece| match piece {
            Piece::Text(words) | Piece::Bold(words) => words,
            Piece::Link { label, .. } => label,
        })
        .collect()
}

fn escape_into(out: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bold_and_one_link() {
        assert_eq!(
            render_announcement("-10% maintenant avec le code **BIENVENUE10** [Voir](/boutique)"),
            "-10% maintenant avec le code <strong>BIENVENUE10</strong> <a href=\"/boutique\">Voir</a>"
        );
        assert_eq!(
            render_announcement("[Soldes](https://example.com/soldes)"),
            "<a href=\"https://example.com/soldes\" rel=\"noopener noreferrer\">Soldes</a>"
        );
    }

    #[test]
    fn markdown_blocks_and_html_stay_text() {
        assert_eq!(render_announcement("- 10% # promo"), "- 10% # promo");
        assert_eq!(
            render_announcement("<b>x</b> **<i>**"),
            "&lt;b&gt;x&lt;/b&gt; <strong>&lt;i&gt;</strong>"
        );
    }

    #[test]
    fn unclosed_marks_are_text() {
        assert_eq!(render_announcement("**x"), "**x");
        assert_eq!(render_announcement("****"), "****");
        assert_eq!(render_announcement("[a](b"), "[a](b");
        assert_eq!(render_announcement("é **ü** ["), "é <strong>ü</strong> [");
    }

    #[test]
    fn a_link_goes_to_the_shop_or_https() {
        assert!(check_announcement_marks("[a](/livraison)").is_ok());
        assert!(check_announcement_marks("[a](https://x.fr)").is_ok());
        for bad in [
            "[a](javascript:alert(1))",
            "[a](//evil.example)",
            "[a](http://x.fr)",
            "[a](/x) [b](/y)",
        ] {
            assert!(check_announcement_marks(bad).is_err(), "{bad}");
        }
        assert_eq!(render_announcement("[a](javascript:x)"), "a");
    }

    #[test]
    fn plain_words() {
        assert_eq!(announcement_text("Code **X** [ici](/a)"), "Code X ici");
    }
}
