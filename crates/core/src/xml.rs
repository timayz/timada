//! Text written into XML: the shops' feeds.

/// Text as XML carries it. Characters XML 1.0 forbids (the control
/// characters but tab and the line ends) are dropped: one of them makes the
/// whole feed unreadable.
pub fn xml_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if c.is_control() || matches!(c, '\u{FFFE}' | '\u{FFFF}') => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_is_escaped_and_forbidden_characters_dropped() {
        assert_eq!(
            xml_text("a < b & \"c\" > d"),
            "a &lt; b &amp; &quot;c&quot; &gt; d"
        );
        assert_eq!(xml_text("a\u{0}b\u{8}c\td\ne"), "abc\td\ne");
    }
}
