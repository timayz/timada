//! The whole catalogue at `/boutique`, newest first, searched with `?q=`.

use timada_storefront::{ProductQuery, ProductSort, menu, products};
use topcoat::{
    Result,
    context::Cx,
    router::{href, page, query_params},
    view::{View, view},
};

use crate::app::{catalog_grid, document};

/// A listing shows this many at once.
pub const PER_PAGE: u32 = 36;

#[topcoat::router::query_params(error = bad_request)]
struct CatalogQuery {
    q: Option<String>,
    page: Option<u32>,
}

#[derive(serde::Serialize)]
struct CatalogLink<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    q: Option<&'a str>,
    page: u32,
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let query = query_params::<CatalogQuery>(cx)?;
    let q = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .map(str::to_owned);
    let mut wanted = ProductQuery::default()
        .page(query.page.unwrap_or(1))
        .per_page(PER_PAGE)
        .sort(ProductSort::Newest);
    if let Some(q) = &q {
        wanted = wanted.search(q.as_str());
    }
    let page = products(cx, &wanted).await?;
    let nav = menu(cx).await?;
    let link = move |number: u32| {
        href!(index)
            .query(CatalogLink {
                q: wanted.q.as_deref(),
                page: number,
            })
            .resolve(cx)
    };
    let title = match &q {
        Some(q) => format!("Résultats pour « {q} »"),
        None => "Nos produits".to_owned(),
    };

    Ok(view! {
        document(
            title: &title,
            nav: &nav,
            <div class="wrap">
                <nav class="crumbs" aria-label="Fil d'Ariane" data-crumbs="">
                    <a href=(href!(crate::app::index))>"Accueil"</a> " / " (&title)
                </nav>
                <h1>(&title)</h1>
                <div class="listing-head">
                    <p data-results=(page.total)>(page.total) (if page.total == 1 { " résultat affiché" } else { " résultats affichés" })</p>
                </div>
                if page.items.is_empty() {
                    <p>"Aucun produit ne correspond pour le moment."</p>
                }
                catalog_grid(products: &page.items)
                if page.pages() > 1 {
                    <nav class="pages" aria-label="Pages">
                        if page.has_prev() { <a href=(link(page.page - 1))>"← Précédent"</a> }
                        <span>"Page " (page.page) " sur " (page.pages())</span>
                        if page.has_next() { <a href=(link(page.page + 1))>"Suivant →"</a> }
                    </nav>
                }
            </div>
        )
    })
}
