//! A category and the products in it, sub-categories included, narrowed by
//! the price and colour filters.

use timada_storefront::{
    ProductQuery, ProductSort, categories, category_body, category_path, menu_of, option_values,
    products, require_category_in,
};
use topcoat::{
    Result,
    context::Cx,
    router::{href, module_param, page, path_param, query_params},
    view::{View, view},
};

use crate::app::{
    boutique::PER_PAGE,
    catalog_grid, document,
    filters::{COLOUR, Filters, Panel, filter_fields, filter_panel},
};

module_param!(pub slug);

#[topcoat::router::query_params(error = bad_request)]
struct CategoryQuery {
    sort: Option<String>,
    page: Option<u32>,
    min_price: Option<u32>,
    max_price: Option<u32>,
    couleur: Option<String>,
}

#[derive(serde::Serialize)]
struct CategoryLink<'a> {
    sort: &'a str,
    page: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    min_price: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_price: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    couleur: Option<&'a str>,
}

/// The sort menu, as the shop words it.
const SORTS: &[(ProductSort, &str)] = &[
    (ProductSort::Newest, "Tri du plus récent au plus ancien"),
    (ProductSort::TopRated, "Tri par notes moyennes"),
    (ProductSort::PriceAsc, "Tri par tarif croissant"),
    (ProductSort::PriceDesc, "Tri par tarif décroissant"),
];

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    // The tree holds every live category, so the page's own comes out of it
    // even when the merchant left it out of the menu.
    let tree = categories(cx).await?;
    let nav = menu_of(&tree);
    let node = require_category_in(&tree, path_param::<Slug>(cx))?;
    let category = node.category.clone();
    let children = node.children.clone();
    let crumbs = category_path(&tree, &category);
    let query = query_params::<CategoryQuery>(cx)?;
    // The longer text about the category, on its first page only.
    let body = if query.page.unwrap_or(1) <= 1 {
        category_body(cx, &category.id).await?
    } else {
        String::new()
    };
    let sort = query
        .sort
        .as_deref()
        .map(ProductSort::parse)
        .unwrap_or_default();
    let filters = Filters::new(query.min_price, query.max_price, query.couleur.clone());
    // The products of this category and of everything below it, which is
    // what a shopper expects from a top-level entry.
    let wanted = filters.apply(
        ProductQuery::default()
            .in_category(category.id.clone())
            .sort(sort)
            .per_page(PER_PAGE)
            .page(query.page.unwrap_or(1)),
    );
    let page = products(cx, &wanted).await?;
    let colours = option_values(cx, &wanted, COLOUR).await?;
    let link = {
        let sort = sort.as_str();
        let slug = category.slug.clone();
        move |filters: &Filters, number: u32| {
            href!(index, Slug(slug.clone()))
                .query(CategoryLink {
                    sort,
                    page: number,
                    min_price: filters.min_price,
                    max_price: filters.max_price,
                    couleur: filters.couleur.as_deref(),
                })
                .resolve(cx)
        }
    };
    let panel = Panel::build(&filters, &colours, |filters| link(filters, 1));
    let sort_label = SORTS
        .iter()
        .find(|(candidate, _)| *candidate == sort)
        .map(|(_, label)| *label)
        .unwrap_or_default();

    Ok(view! {
        document(
            title: &category.name,
            nav: &nav,
            <div class="wrap">
                <nav class="crumbs" aria-label="Fil d'Ariane" data-crumbs="">
                    <a href=(href!(crate::app::index))>"Accueil"</a>
                    for crumb in &crumbs {
                        " / "
                        <a href=(href!(index, Slug(crumb.slug.clone())))>(&crumb.name)</a>
                    }
                    " / " (&category.name)
                </nav>
                <h1 data-category=(&category.slug)>(&category.name)</h1>
                if !category.description.is_empty() {
                    <p class="description">(&category.description)</p>
                }
                if !children.is_empty() {
                    <nav aria-label="Sous-catégories">
                        <ul class="chips">
                            for child in &children {
                                <li>
                                    <a data-subcategory=(&child.category.slug)
                                        href=(href!(index, Slug(child.category.slug.clone())))>
                                        (&child.category.name) " (" (child.product_count) ")"
                                    </a>
                                </li>
                            }
                        </ul>
                    </nav>
                }
                filter_panel(panel: &panel)
                <div class="listing-head">
                    <p data-results=(page.total)>
                        (page.total) (if page.total == 1 { " résultat affiché" } else { " résultats affichés" })
                        " · " (sort_label)
                    </p>
                    <form class="sort" method="get">
                        filter_fields(filters: &filters)
                        <label for="sort">"Trier"</label>
                        <select id="sort" name="sort" data-sort="">
                            for (candidate, label) in SORTS {
                                <option value=(candidate.as_str()) selected=(*candidate == sort)>(*label)</option>
                            }
                        </select>
                        <button>"Trier"</button>
                    </form>
                </div>
                if page.items.is_empty() {
                    if panel.active {
                        <p data-filters-empty="">
                            "Aucun produit ne correspond à ces filtres. "
                            <a href=(&panel.clear)>"Effacer les filtres"</a>
                        </p>
                    } else {
                        <p>"Rien dans cette catégorie pour le moment."</p>
                    }
                }
                catalog_grid(products: &page.items)
                if page.pages() > 1 {
                    <nav class="pages" aria-label="Pages">
                        if page.has_prev() { <a href=(link(&filters, page.page - 1))>"← Précédent"</a> }
                        <span>"Page " (page.page) " sur " (page.pages())</span>
                        if page.has_next() { <a href=(link(&filters, page.page + 1))>"Suivant →"</a> }
                    </nav>
                }
                if page.page == 1 && !body.is_empty() {
                    <section class="category-body" data-category-body="">
                        <p class="description">(&body)</p>
                    </section>
                }
            </div>
        )
    })
}
