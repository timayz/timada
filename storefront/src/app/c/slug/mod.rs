//! A category and the products in it, sub-categories included, narrowed by
//! the price and colour filters. Its plain listing opens on the products
//! the merchant put first, and its first page ends on the buying guide and
//! the questions the merchant answered.

use timada_storefront::{
    ProductQuery, ProductSort, breadcrumb_ld, categories, category_hub, category_path,
    category_seo, category_slugs, find_category, menu_of, option_values, products,
    require_category,
};
use topcoat::{
    Result,
    context::Cx,
    router::{error::redirect_permanent, href, module_param, page, query_params},
    view::{Unescaped, View, view},
};

use crate::app::{
    boutique::PER_PAGE,
    catalog_grid, document,
    filters::{Filters, Panel, filter_fields, filter_panel, shop_filters},
};

module_param!(pub slug);

#[topcoat::router::query_params(error = bad_request)]
struct CategoryQuery {
    sort: Option<String>,
    page: Option<u32>,
    min_price: Option<u32>,
    max_price: Option<u32>,
}

/// A link into the listing. The default sort and the first page are left
/// out, so the plain listing and each page after it have one address.
#[derive(serde::Serialize)]
struct CategoryLink<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    sort: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    page: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    min_price: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_price: Option<u32>,
}

/// How a category lists its products until a shopper asks otherwise: what
/// sells best first, then the newest, which is the whole order of a shop
/// that has sold nothing yet.
const DEFAULT_SORT: ProductSort = ProductSort::BestSelling;

/// The sort menu, as the shop words it.
const SORTS: &[(ProductSort, &str)] = &[
    (ProductSort::BestSelling, "Tri par popularité"),
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
    let Some(node) = find_category(&tree, slug(cx)) else {
        // One address a language: asked by its main slug where it is
        // translated, it moves to the one it goes by here.
        let category = require_category(cx, slug(cx)).await?;
        return Err(redirect_permanent(href!(index, slug: category.slug).resolve(cx)).into());
    };
    let category = node.category.clone();
    let children = node.children.clone();
    let crumbs = category_path(&tree, &category);
    let seo = category_seo(cx, &category).await?;
    let query = query_params::<CategoryQuery>(cx)?;
    let hub = category_hub(cx, &category.id).await?;
    let sort = query
        .sort
        .as_deref()
        .map(ProductSort::parse)
        .unwrap_or(DEFAULT_SORT);
    let offered = shop_filters(cx).await?;
    let filters = Filters::read(cx, query.min_price, query.max_price, &offered);
    // The products of this category and of everything below it, which is
    // what a shopper expects from a top-level entry.
    let wanted = filters.apply(
        ProductQuery::default()
            .in_category(category.id.clone())
            .sort(sort)
            .per_page(PER_PAGE)
            .page(query.page.unwrap_or(1)),
    );
    // Filtered or sorted another way, it shows the category's products
    // again.
    let narrowed = filters.is_active() || sort != DEFAULT_SORT;
    // The plain listing opens on the products the merchant put first, and
    // lists the others after them: its first page is longer by those.
    let pinned = if narrowed { Vec::new() } else { hub.pinned };
    let mut listed = wanted.clone();
    for product in &pinned {
        listed = listed.exclude(product.id.clone());
    }
    let mut page = products(cx, &listed).await?;
    let total = page.total + pinned.len() as i64;
    if page.page == 1 {
        page.items.splice(0..0, pinned);
    }
    // The values each filter offers, counted over the whole listing: the
    // products put first are of it.
    let mut values = Vec::with_capacity(offered.len());
    for filter in offered {
        let found = option_values(cx, &wanted, &filter.name).await?;
        values.push((filter, found));
    }
    let link = {
        let sort = (sort != DEFAULT_SORT).then(|| sort.as_str());
        let slug = category.slug.clone();
        move |filters: &Filters, number: u32| {
            filters.onto(
                href!(index, slug: slug.clone())
                    .query(CategoryLink {
                        sort,
                        page: (number > 1).then_some(number),
                        min_price: filters.min_price,
                        max_price: filters.max_price,
                    })
                    .resolve(cx),
            )
        }
    };
    let panel = Panel::build(&filters, &values, |filters| link(filters, 1));
    let sort_label = SORTS
        .iter()
        .find(|(candidate, _)| *candidate == sort)
        .map(|(_, label)| *label)
        .unwrap_or_default();

    // Search engines keep the plain listing for those, and each page of it
    // for itself.
    let canonical = if narrowed {
        href!(index, slug: category.slug.clone()).resolve(cx)
    } else {
        link(&Filters::default(), page.page)
    };
    let speaking = crate::app::lang::speaking(cx).await;
    let slugs = category_slugs(cx, &category.id).await?;
    let elsewhere = speaking.elsewhere(&canonical, |language| {
        href!(index, slug: slugs.in_language(language).to_owned()).resolve(cx)
    });
    let home = speaking.absolute(cx, &href!(crate::app::index).resolve(cx));
    let trail: Vec<(String, String)> = crumbs
        .iter()
        .chain([&category])
        .map(|crumb| {
            let path = href!(index, slug: crumb.slug.clone()).resolve(cx);
            (crumb.name.clone(), speaking.absolute(cx, &path))
        })
        .collect();
    let structured = [breadcrumb_ld(
        [("Accueil", home.as_str())].into_iter().chain(
            trail
                .iter()
                .map(|(name, url)| (name.as_str(), url.as_str())),
        ),
    )];

    Ok(view! {
        document(
            title: &seo.title,
            noindex: narrowed,
            canonical: Some(canonical.as_str()),
            elsewhere: &elsewhere,
            structured: &structured,
            description: Some(seo.description.as_str()).filter(|text| !text.is_empty()),
            nav: &nav,
            <div class="wrap">
                <nav class="crumbs" aria-label="Fil d'Ariane" data-crumbs="">
                    <a href=(href!(crate::app::index))>"Accueil"</a>
                    for crumb in &crumbs {
                        " / "
                        <a href=(href!(index, slug: crumb.slug.clone()))>(&crumb.name)</a>
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
                                        href=(href!(index, slug: child.category.slug.clone()))>
                                        (&child.category.name) " (" (child.product_count) ")"
                                    </a>
                                </li>
                            }
                        </ul>
                    </nav>
                }
                filter_panel(panel: &panel)
                <div class="listing-head">
                    <p data-results=(total)>
                        (total) (if total == 1 { " résultat affiché" } else { " résultats affichés" })
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
                if page.page == 1 && !hub.guide_html.is_empty() {
                    // Cleaned by the library: no script, no attribute it did
                    // not allow, no picture from elsewhere.
                    <section class="category-body" data-category-body="">
                        (Unescaped::new_unchecked(hub.guide_html.clone()))
                    </section>
                }
                if page.page == 1 && !hub.questions.is_empty() {
                    <section class="category-questions" aria-labelledby="questions-title" data-category-questions="">
                        <h2 id="questions-title">"Questions fréquentes"</h2>
                        for asked in &hub.questions {
                            <details>
                                <summary>(&asked.question)</summary>
                                <p>(&asked.answer)</p>
                            </details>
                        }
                    </section>
                }
            </div>
        )
    })
}
