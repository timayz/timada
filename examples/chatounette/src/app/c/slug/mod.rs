//! A category and the products in it, sub-categories included.

use timada_storefront::{
    ProductSort,
    pages::{ListOptions, category_page},
};
use topcoat::{
    Result,
    context::Cx,
    router::{module_param, page, path_param},
    view::{View, view},
};

use crate::app::{catalog_grid, document};

module_param!(pub slug);

/// The sort menu, as the shop words it.
fn sort_label(sort: ProductSort) -> &'static str {
    match sort {
        ProductSort::Newest => "Tri du plus récent au plus ancien",
        ProductSort::TopRated => "Tri par notes moyennes",
        ProductSort::PriceAsc => "Tri par tarif croissant",
        ProductSort::PriceDesc => "Tri par tarif décroissant",
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let category = category_page(cx, path_param::<Slug>(cx), &ListOptions::default()).await?;
    let sorts: Vec<(ProductSort, bool)> = category.sorts().collect();

    Ok(view! {
        document(
            title: &category.category.name,
            <div class="wrap">
                <nav class="crumbs" aria-label="Fil d'Ariane" data-crumbs="">
                    <a href=(&category.home_url)>"Accueil"</a>
                    for crumb in &category.crumbs {
                        " / "
                        <a href=(&crumb.url)>(&crumb.name)</a>
                    }
                    " / " (&category.category.name)
                </nav>
                <h1 data-category=(&category.category.slug)>(&category.category.name)</h1>
                if !category.category.description.is_empty() {
                    <p class="description">(&category.category.description)</p>
                }
                if !category.children.is_empty() {
                    <nav aria-label="Sous-catégories">
                        <ul class="chips">
                            for child in &category.children {
                                <li>
                                    <a data-subcategory=(&child.category.slug) href=(&child.url)>
                                        (&child.category.name) " (" (child.product_count) ")"
                                    </a>
                                </li>
                            }
                        </ul>
                    </nav>
                }
                <div class="listing-head">
                    <p data-results=(category.products.total)>
                        (category.products.total) (if category.products.total == 1 { " résultat affiché" } else { " résultats affichés" })
                        " · " (sort_label(category.sort))
                    </p>
                    <form class="sort" method="get">
                        <label for="sort">"Trier"</label>
                        <select id="sort" name="sort" data-sort="">
                            for (sort, selected) in &sorts {
                                <option value=(sort.as_str()) selected=(*selected)>(sort_label(*sort))</option>
                            }
                        </select>
                        <button>"Trier"</button>
                    </form>
                </div>
                if category.products.items.is_empty() {
                    <p>"Rien dans cette catégorie pour le moment."</p>
                }
                catalog_grid(products: &category.products.items)
                if category.products.pages() > 1 {
                    <nav class="pages" aria-label="Pages">
                        if let Some(prev) = &category.prev_url { <a href=(prev)>"← Précédent"</a> }
                        <span>"Page " (category.products.page) " sur " (category.products.pages())</span>
                        if let Some(next) = &category.next_url { <a href=(next)>"Suivant →"</a> }
                    </nav>
                }
            </div>
        )
    })
}
