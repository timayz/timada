//! The shop's menu: one bar of top-level categories, each opening a panel
//! that starts with "All <category>" and lists what is under it, two more
//! levels deep. The merchant leaves a category out of it from the back
//! office; the page builds it from the library's menu.
//!
//! Without JS each category is a `<details>` and the small-screen menu opens
//! on `#menu`. The library's menu script adds hover, one panel at a time,
//! Escape and a click outside; it reads `details[data-menu]` and the
//! `[data-menu-item]` around each.

use timada_storefront::CategoryNode;
use topcoat::{
    Result,
    router::href,
    view::{View, view},
};

use super::{boutique, c};

/// The menu under the header. `nav` holds only what the menu shows.
#[topcoat::view::component]
pub async fn site_menu(nav: &[CategoryNode]) -> Result<impl View> {
    Ok(view! {
        <nav id="menu" class="sections" aria-label="Catégories">
            <a class="menu-close" href="#">"Fermer le menu"</a>
            <ul class="menu-bar">
                <li class="menu-item" data-menu-item=""><a class="menu-top" href=(href!(boutique::index)) data-menu-all="">"Nos produits"</a></li>
                for node in nav {
                    <li class="menu-item" data-menu-item="">
                        if node.children.is_empty() {
                            <a class="menu-top" data-category=(&node.category.slug) data-products=(node.product_count)
                                href=(href!(c::slug::index, c::slug::Slug(node.category.slug.clone())))>
                                (&node.category.name)
                            </a>
                        } else {
                            <details data-menu=(&node.category.slug)>
                                <summary class="menu-top">(&node.category.name)</summary>
                                <div class="menu-panel">
                                    <a class="menu-all" data-category=(&node.category.slug) data-products=(node.product_count)
                                        href=(href!(c::slug::index, c::slug::Slug(node.category.slug.clone())))>
                                        "Tout " (&node.category.name)
                                    </a>
                                    <ul class="menu-columns">
                                        for child in &node.children {
                                            <li>
                                                <a class="menu-heading" data-category=(&child.category.slug) data-products=(child.product_count)
                                                    href=(href!(c::slug::index, c::slug::Slug(child.category.slug.clone())))>
                                                    (&child.category.name)
                                                </a>
                                                if !child.children.is_empty() {
                                                    <ul>
                                                        for leaf in &child.children {
                                                            <li>
                                                                <a data-category=(&leaf.category.slug) data-products=(leaf.product_count)
                                                                    href=(href!(c::slug::index, c::slug::Slug(leaf.category.slug.clone())))>
                                                                    (&leaf.category.name)
                                                                </a>
                                                            </li>
                                                        }
                                                    </ul>
                                                }
                                            </li>
                                        }
                                    </ul>
                                </div>
                            </details>
                        }
                    </li>
                }
            </ul>
        </nav>
    })
}

/// The menu's look: a bar with full-width panels on a wide screen, a sheet
/// opened by the header's "Menu" link on a narrow one. (The toggle itself
/// is styled with the masthead, in `theme.css`.)
pub const MENU_CSS: &str = r#"
.menu-close { display: none; }
nav.sections { position: relative; border-bottom: 1px solid var(--line); background: var(--paper); }
nav.sections a { color: inherit; text-decoration: none; }
nav.sections a:hover, nav.sections a:focus-visible { color: var(--coral-dark); }
ul.menu-bar { list-style: none; padding: 0; margin: 0 auto; width: min(1200px, 100% - 2rem); display: flex; flex-wrap: wrap; justify-content: center; gap: 0 1.75rem; }
.menu-top { display: inline-block; padding: .75rem 0; font-size: .85rem; font-weight: 800; letter-spacing: .04em; text-transform: uppercase; color: var(--ink); cursor: pointer; }
summary.menu-top { list-style: none; }
summary.menu-top::-webkit-details-marker { display: none; }
summary.menu-top::after { content: '▾'; margin-left: .3rem; font-size: .7rem; color: var(--muted); }
details[open] > summary.menu-top { color: var(--coral-dark); }
.menu-panel { position: absolute; left: 0; right: 0; top: 100%; z-index: 30; background: var(--paper); border: 1px solid var(--line); border-top: 2px solid var(--coral); box-shadow: var(--shadow); padding: 1.25rem max(1.5rem, calc((100% - 1200px) / 2)); }
nav.sections a.menu-all { display: inline-block; margin-bottom: 1rem; font-weight: 700; color: var(--coral-dark); }
ul.menu-columns { list-style: none; padding: 0; margin: 0; columns: 12rem; column-gap: 2rem; }
ul.menu-columns > li { break-inside: avoid; margin-bottom: .9rem; }
nav.sections a.menu-heading { font-weight: 700; color: var(--ink); }
ul.menu-columns ul { list-style: none; padding: 0 0 0 .75rem; margin: .25rem 0 0; font-size: .9rem; color: var(--muted); }
ul.menu-columns ul li { margin: .15rem 0; }
@media (max-width: 900px) {
  nav.sections { display: none; border: 0; margin: 0; }
  nav.sections:target { display: block; position: fixed; inset: 0; z-index: 40; overflow-y: auto; background: var(--paper); padding: 1rem; }
  nav.sections .menu-close { display: block; margin-bottom: 1rem; color: var(--coral-dark); }
  ul.menu-bar { display: block; width: auto; }
  .menu-item { border-bottom: 1px solid var(--line); }
  .menu-top { display: block; }
  .menu-panel { position: static; border: 0; box-shadow: none; padding: 0 0 1rem 1rem; }
  ul.menu-columns { columns: auto; }
}
"#;
