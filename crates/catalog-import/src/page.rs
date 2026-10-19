//! What a shop's pages say that its Store API does not: the text a theme
//! shows below a category's products, and the text a page builder lays out
//! in a product's "Description" tab.

use crate::text::html_to_text;

/// The block Shoptimizer shows below a category's products.
const BELOW_CATEGORY: &str = r#"class="below-woocommerce-category""#;
/// The panel of a product's "Description" tab.
const DESCRIPTION_TAB: &str = r#"id="tab-description""#;
/// The marker of an Elementor widget, followed by its type.
const WIDGET: &str = r#"data-widget_type=""#;
/// The widgets whose text is the product's own: headings, text blocks and
/// repeated fields (a size chart). Pictures, product grids, testimonials and
/// spacers are left out.
const TEXT_WIDGETS: [&str; 3] = ["heading.", "text-editor.", "jet-listing-dynamic-repeater."];

/// The text below a category's products, or nothing when the page has none.
pub fn below_category(html: &str) -> String {
    element_at(html, BELOW_CATEGORY)
        .map(|(inner, _)| html_to_text(inner))
        .unwrap_or_default()
}

/// The text of a product's "Description" tab: its text widgets in page
/// order, up to the first heading named in `stop_at` (where the shop's own
/// blocks, the same on every product, begin). A widget inside one left out
/// is left out with it. Nothing when the page has no such tab.
pub fn product_tab(html: &str, stop_at: &[String]) -> String {
    let Some((panel, _)) = element_at(html, DESCRIPTION_TAB) else {
        return String::new();
    };
    let mut parts: Vec<String> = Vec::new();
    let mut rest = panel;
    while let Some(at) = rest.find(WIDGET) {
        let kind = &rest[at + WIDGET.len()..];
        let kind = &kind[..kind.find('"').unwrap_or(kind.len())];
        let Some((inner, after)) = element_at(rest, &rest[at..at + WIDGET.len() + kind.len()])
        else {
            break;
        };
        if TEXT_WIDGETS.iter().any(|prefix| kind.starts_with(prefix)) {
            let text = html_to_text(inner);
            if kind.starts_with("heading.") && stop_at.iter().any(|stop| text.trim() == stop) {
                break;
            }
            if !text.is_empty() {
                parts.push(text);
            }
        }
        rest = &rest[after..];
    }
    parts.join("\n\n")
}

/// The inside of the `<div>` whose opening tag holds `marker`, and the
/// offset just past its closing tag, matched by counting the `<div>`s
/// opened and closed in between.
fn element_at<'a>(html: &'a str, marker: &str) -> Option<(&'a str, usize)> {
    let at = html.find(marker)?;
    let open = html[..at].rfind('<')?;
    let start = open + html[open..].find('>')? + 1;
    let mut depth = 1usize;
    let mut cursor = start;
    while depth > 0 {
        let next = html[cursor..].find("<div").map(|i| (cursor + i, true));
        let close = html[cursor..].find("</div").map(|i| (cursor + i, false));
        let (index, opens) = match (next, close) {
            (Some(n), Some(c)) => {
                if n.0 < c.0 {
                    n
                } else {
                    c
                }
            }
            (None, Some(c)) => c,
            _ => return None,
        };
        if opens {
            depth += 1;
        } else {
            depth -= 1;
        }
        let end = index + html[index..].find('>')? + 1;
        if depth == 0 {
            return Some((&html[start..index], end));
        }
        cursor = end;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const CATEGORY: &str = r#"<main><div class="products"><div>Mug</div></div>
        <nav><a href="/page/2/">2</a></nav>
        <div class="below-woocommerce-category"> <p>Le chat dort <strong>beaucoup</strong>.<br /> Il joue aussi.</p>
        <h2>Comment choisir&nbsp;?</h2><div><p>Observez-le.</p></div></div>
        <footer>Contact</footer></main>"#;

    #[test]
    fn the_text_below_a_category_stops_at_its_block() {
        assert_eq!(
            below_category(CATEGORY),
            "Le chat dort beaucoup.\nIl joue aussi.\n\nComment choisir ?\n\nObservez-le."
        );
        assert_eq!(below_category("<main><p>Nothing</p></main>"), "");
    }

    const PRODUCT: &str = r#"<div class="summary"><div class="woocommerce-product-details__short-description"><p>Court.</p></div></div>
        <div class="woocommerce-Tabs-panel" id="tab-description" role="tabpanel">
          <div class="elementor"><section><div class="widget-wrap"></div></section>
            <div class="elementor-element elementor-widget" data-widget_type="jet-listing-dynamic-repeater.default"><div class="container">
              <h2 style="text-align:center">Tableau des tailles</h2><div class="item"><span>S : 40x30cm | M : 50x40cm</span></div>
            </div></div>
            <div class="elementor-element" data-widget_type="heading.default"><h2 class="title">Un cocon confortable</h2></div>
            <div class="elementor-element" data-widget_type="text-editor.default"><p>Votre chat a son territoire.</p><p>Doux.</p></div>
            <div class="elementor-element" data-widget_type="image.default"><img alt="Tapis" src="t.jpg"></div>
            <div class="elementor-element" data-widget_type="jet-listing-grid.default"><div class="item">
              <div data-widget_type="heading.default"><h2>Gant toilettage chat</h2></div>
              <div data-widget_type="text-editor.default"><p>Un autre produit.</p></div>
            </div></div>
            <div class="elementor-element" data-widget_type="heading.default"><h2>Une place fraîche</h2></div>
            <div class="elementor-element" data-widget_type="heading.default"><h2>À propos</h2></div>
            <div class="elementor-element" data-widget_type="text-editor.default"><p>Antoine et Léa.</p></div>
          </div>
        </div>
        <div class="woocommerce-Tabs-panel" id="tab-additional_information"><p>Poids</p></div>"#;

    #[test]
    fn a_product_tab_keeps_its_own_text_up_to_the_shop_blocks() {
        assert_eq!(
            product_tab(PRODUCT, &["À propos".to_owned()]),
            "Tableau des tailles\n\nS : 40x30cm | M : 50x40cm\n\nUn cocon confortable\n\n\
             Votre chat a son territoire.\n\nDoux.\n\nUne place fraîche"
        );
    }

    #[test]
    fn without_a_stop_the_whole_tab_is_read_but_never_what_follows() {
        let text = product_tab(PRODUCT, &[]);
        assert!(text.ends_with("À propos\n\nAntoine et Léa."), "{text}");
        assert!(!text.contains("Gant"), "a product grid is left out: {text}");
        assert!(!text.contains("Poids"), "the next tab is not read: {text}");
        assert_eq!(product_tab("<main></main>", &[]), "");
    }
}
