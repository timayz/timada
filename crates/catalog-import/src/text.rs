//! What a shop's HTML becomes in a catalog that keeps plain text.

/// Turns a fragment of HTML into readable text: paragraphs, headings, list
/// items and table rows end a line, `<br>` breaks one, every other tag is
/// dropped, entities are decoded, whitespace is collapsed, and blank lines
/// come at most two in a row. A script's or style's content is dropped too.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    let mut skip_until: Option<&str> = None;
    while let Some(start) = rest.find('<') {
        let text = &rest[..start];
        if skip_until.is_none() {
            push_text(&mut out, text);
        }
        rest = &rest[start..];
        let Some(end) = rest.find('>') else {
            break;
        };
        let tag = &rest[1..end];
        rest = &rest[end + 1..];
        let name = tag
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let closing = tag.starts_with('/');
        if let Some(wanted) = skip_until {
            if closing && name == wanted {
                skip_until = None;
            }
            continue;
        }
        match name.as_str() {
            "script" | "style" if !closing => {
                skip_until = Some(if name == "script" { "script" } else { "style" })
            }
            "br" => out.push('\n'),
            "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "table" | "blockquote" | "ul"
            | "ol" | "pre" | "hr" => {
                if closing || name == "hr" {
                    break_paragraph(&mut out);
                } else {
                    break_line(&mut out);
                }
            }
            "div" | "li" | "tr" | "section" | "article" | "header" | "footer" | "figure"
            | "figcaption" => break_line(&mut out),
            "td" | "th" if closing => out.push(' '),
            _ => {}
        }
    }
    if skip_until.is_none() {
        push_text(&mut out, rest);
    }
    collapse(&out)
}

/// Appends text with its entities decoded and its runs of whitespace
/// folded into one space.
fn push_text(out: &mut String, text: &str) {
    let decoded = decode_entities(text);
    let mut pending_space = false;
    for c in decoded.chars() {
        if c.is_whitespace() && c != '\u{a0}' {
            pending_space = true;
            continue;
        }
        if pending_space && !out.is_empty() && !out.ends_with('\n') {
            out.push(' ');
        }
        pending_space = false;
        out.push(if c == '\u{a0}' { ' ' } else { c });
    }
    if pending_space && !out.is_empty() && !out.ends_with('\n') {
        out.push(' ');
    }
}

/// Ends the line being written, unless it is already ended.
fn break_line(out: &mut String) {
    let trimmed = out.trim_end_matches(' ').len();
    out.truncate(trimmed);
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
}

/// Ends the paragraph being written with a blank line, unless there is one.
fn break_paragraph(out: &mut String) {
    break_line(out);
    if !out.is_empty() && !out.ends_with("\n\n") {
        out.push('\n');
    }
}

/// Trims every line, drops leading and trailing blank lines, and keeps at
/// most one blank line between paragraphs.
fn collapse(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank = 0;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            blank += 1;
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
            if blank > 0 {
                out.push('\n');
            }
        }
        blank = 0;
        out.push_str(line);
    }
    out
}

const ENTITIES: &[(&str, &str)] = &[
    ("amp", "&"),
    ("lt", "<"),
    ("gt", ">"),
    ("quot", "\""),
    ("apos", "'"),
    ("nbsp", "\u{a0}"),
    ("rsquo", "\u{2019}"),
    ("lsquo", "\u{2018}"),
    ("rdquo", "\u{201d}"),
    ("ldquo", "\u{201c}"),
    ("laquo", "\u{ab}"),
    ("raquo", "\u{bb}"),
    ("hellip", "\u{2026}"),
    ("ndash", "\u{2013}"),
    ("mdash", "\u{2014}"),
    ("euro", "\u{20ac}"),
    ("deg", "\u{b0}"),
    ("copy", "\u{a9}"),
    ("reg", "\u{ae}"),
    ("trade", "\u{2122}"),
    ("times", "\u{d7}"),
    ("eacute", "\u{e9}"),
    ("egrave", "\u{e8}"),
    ("ecirc", "\u{ea}"),
    ("agrave", "\u{e0}"),
    ("acirc", "\u{e2}"),
    ("ccedil", "\u{e7}"),
    ("ocirc", "\u{f4}"),
    ("ucirc", "\u{fb}"),
    ("ugrave", "\u{f9}"),
    ("icirc", "\u{ee}"),
    ("iuml", "\u{ef}"),
    ("euml", "\u{eb}"),
    ("Eacute", "\u{c9}"),
    ("Egrave", "\u{c8}"),
    ("Agrave", "\u{c0}"),
    ("Ccedil", "\u{c7}"),
    ("oelig", "\u{153}"),
    ("OElig", "\u{152}"),
    ("frac12", "\u{bd}"),
    ("middot", "\u{b7}"),
    ("bull", "\u{2022}"),
];

/// Decodes the named entities above and every numeric one.
fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        // An entity is short; look for its end within the next few
        // characters (not bytes: the text after a stray `&` may be anything).
        let Some(end) = rest
            .char_indices()
            .take(12)
            .find(|(_, c)| *c == ';')
            .map(|(index, _)| index)
        else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let decoded = if let Some(code) = entity.strip_prefix('#') {
            let value = if let Some(hex) = code.strip_prefix(['x', 'X']) {
                u32::from_str_radix(hex, 16).ok()
            } else {
                code.parse::<u32>().ok()
            };
            value.and_then(char::from_u32).map(String::from)
        } else {
            ENTITIES
                .iter()
                .find(|(name, _)| *name == entity)
                .map(|(_, value)| (*value).to_owned())
        };
        match decoded {
            Some(decoded) => {
                out.push_str(&decoded);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Cuts text to at most `max` characters, at the last paragraph break
/// before the limit when there is one past the halfway mark, else at the
/// last space, else at the limit. Trailing whitespace is trimmed.
pub fn truncate_chars(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let limit = text
        .char_indices()
        .nth(max)
        .map(|(index, _)| index)
        .unwrap_or(text.len());
    let head = &text[..limit];
    let cut = head
        .rfind("\n\n")
        .filter(|&at| at >= limit / 2)
        .or_else(|| head.rfind(char::is_whitespace))
        .unwrap_or(limit);
    head[..cut].trim_end().to_owned()
}

/// Cuts a short field (a name, a value) to `max` characters, keeping the
/// text whole where it fits and never splitting a character.
pub(crate) fn clamp_chars(text: &str, max: usize) -> String {
    let text = text.trim();
    match text.char_indices().nth(max) {
        Some((index, _)) => text[..index].trim_end().to_owned(),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paragraphs_become_lines_and_entities_text() {
        let html = "<p>Imaginez votre <strong>chat</strong> étendu&nbsp;ici, loin de l&rsquo;été.</p>\n<p class=\"x\">Second&#8217;s &amp; third &#x27;quoted&#x27;.</p><ul><li>un</li><li>deux</li></ul>";
        assert_eq!(
            html_to_text(html),
            "Imaginez votre chat étendu ici, loin de l\u{2019}été.\n\nSecond\u{2019}s & third 'quoted'.\n\nun\ndeux"
        );
    }

    #[test]
    fn breaks_scripts_and_stray_ampersands() {
        assert_eq!(html_to_text("a<br>b<br/>c"), "a\nb\nc");
        assert_eq!(
            html_to_text("<script>alert(1)</script>Tom & Jerry <style>p{}</style>end"),
            "Tom & Jerry end"
        );
        assert_eq!(
            html_to_text("<p><br data-mce-bogus=\"1\"></p><p>text</p>"),
            "text"
        );
        assert_eq!(html_to_text("  plain   text  "), "plain text");
        assert_eq!(html_to_text("Tom & Jérôme ; Zoé &eacute;"), "Tom & Jérôme ; Zoé é");
    }

    #[test]
    fn tables_read_across() {
        assert_eq!(
            html_to_text(
                "<table><tr><th>Taille</th><td>S</td></tr><tr><th>Poids</th><td>1 kg</td></tr></table>"
            ),
            "Taille S\nPoids 1 kg"
        );
    }

    #[test]
    fn truncates_at_a_paragraph_then_a_word() {
        let text = "first paragraph here\n\nsecond paragraph here\n\nthird";
        assert_eq!(truncate_chars(text, 100), text);
        assert_eq!(
            truncate_chars(text, 45),
            "first paragraph here\n\nsecond paragraph here"
        );
        assert_eq!(truncate_chars("one two three four", 10), "one two");
        assert_eq!(truncate_chars("ééééé", 3), "ééé");
        assert_eq!(clamp_chars("  héllo world ", 4), "héll");
        assert_eq!(clamp_chars("ok", 4), "ok");
    }
}
