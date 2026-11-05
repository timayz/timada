//! The shop's own pages, each at an address of its own (`/mentions-legales`,
//! `/politique-de-livraison`…): which pages the shop has, and where, is
//! decided here; their text is the merchant's, written and published in the
//! back office, and read by slug. A page whose text is not published yet is
//! a 404.

use timada_storefront::{CategoryNode, PublicPage};
use topcoat::{
    Result,
    context::Cx,
    router::href,
    view::{Unescaped, View, component, view},
};

use crate::app::{
    conditions_generales_de_vente, document, mentions_legales, politique_de_confidentialite,
    politique_de_livraison, politique_de_remboursement, qui_sommes_nous,
};

/// The slug of each page's text in the back office, which is also the
/// address the shop serves it at.
pub const MENTIONS_LEGALES: &str = "mentions-legales";
pub const CONDITIONS_GENERALES_DE_VENTE: &str = "conditions-generales-de-vente";
pub const POLITIQUE_DE_CONFIDENTIALITE: &str = "politique-de-confidentialite";
pub const POLITIQUE_DE_REMBOURSEMENT: &str = "politique-de-remboursement";
pub const POLITIQUE_DE_LIVRAISON: &str = "politique-de-livraison";
pub const QUI_SOMMES_NOUS: &str = "qui-sommes-nous";

/// Where the shop serves the page whose text has `slug`, if it has one: a
/// search finds only the pages the shop links to.
pub fn url(cx: &Cx, slug: &str) -> Option<String> {
    Some(match slug {
        MENTIONS_LEGALES => href!(mentions_legales::index).resolve(cx),
        CONDITIONS_GENERALES_DE_VENTE => href!(conditions_generales_de_vente::index).resolve(cx),
        POLITIQUE_DE_CONFIDENTIALITE => href!(politique_de_confidentialite::index).resolve(cx),
        POLITIQUE_DE_REMBOURSEMENT => href!(politique_de_remboursement::index).resolve(cx),
        POLITIQUE_DE_LIVRAISON => href!(politique_de_livraison::index).resolve(cx),
        QUI_SOMMES_NOUS => href!(qui_sommes_nous::index).resolve(cx),
        _ => return None,
    })
}

/// A page in the shop's layout; its route loads it with
/// [`require_page`](timada_storefront::require_page), a 404 when the text is
/// not published.
#[component]
pub async fn shop_page(page: &PublicPage, nav: &[CategoryNode]) -> Result<impl View> {
    Ok(view! {
        document(
            title: &page.meta_title,
            description: Some(page.meta_description.as_str()),
            nav: nav,
            <div class="wrap">
                <nav class="crumbs" aria-label="Fil d'Ariane" data-crumbs="">
                    <a href=(href!(crate::app::index))>"Accueil"</a> " / " (&page.title)
                </nav>
                <article class="page" data-page=(&page.slug)>
                    <h1>(&page.title)</h1>
                    // Cleaned by the library: no script, no attribute it did
                    // not allow, no picture from elsewhere.
                    (Unescaped::new_unchecked(page.html.clone()))
                </article>
            </div>
        )
    })
}
