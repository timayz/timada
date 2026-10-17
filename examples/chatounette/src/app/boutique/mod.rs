//! The whole catalogue at `/boutique`, newest first, searched with `?q=`.

use timada_storefront::pages::{ListOptions, catalog_page};
use topcoat::{
    Result,
    context::Cx,
    router::page,
    view::{View, view},
};

use crate::app::{catalog_grid, document};

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let catalog = catalog_page(cx, &ListOptions::default()).await?;
    let title = match &catalog.q {
        Some(q) => format!("Résultats pour « {q} »"),
        None => "Nos produits".to_owned(),
    };

    Ok(view! {
        document(
            title: &title,
            <div class="wrap">
                <nav class="crumbs" aria-label="Fil d'Ariane" data-crumbs="">
                    <a href=(&catalog.home_url)>"Accueil"</a> " / " (&title)
                </nav>
                <h1>(&title)</h1>
                <div class="listing-head">
                    <p data-results=(catalog.products.total)>(catalog.products.total) (if catalog.products.total == 1 { " résultat affiché" } else { " résultats affichés" })</p>
                </div>
                if catalog.products.items.is_empty() {
                    <p>"Aucun produit ne correspond pour le moment."</p>
                }
                catalog_grid(products: &catalog.products.items)
                if catalog.products.pages() > 1 {
                    <nav class="pages" aria-label="Pages">
                        if let Some(prev) = &catalog.prev_url { <a href=(prev)>"← Précédent"</a> }
                        <span>"Page " (catalog.products.page) " sur " (catalog.products.pages())</span>
                        if let Some(next) = &catalog.next_url { <a href=(next)>"Suivant →"</a> }
                    </nav>
                }
            </div>
        )
    })
}
