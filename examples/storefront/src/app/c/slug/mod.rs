//! A category and the products in it, sub-categories included.

use timada_storefront::{
    ProductQuery, ProductSort, categories, category_path, products, require_category_in,
};
use topcoat::{
    Result,
    context::Cx,
    router::{href, module_param, page, path_param, query_params},
    view::{View, view},
};

use crate::app::{catalog_grid, document};

module_param!(pub slug);

/// A category page shows more at once than the home page.
const PER_PAGE: u32 = 36;

#[topcoat::router::query_params(error = bad_request)]
struct CategoryQuery {
    sort: Option<String>,
    page: Option<u32>,
}

#[derive(serde::Serialize)]
struct CategoryLink<'a> {
    sort: &'a str,
    page: u32,
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    // The menu holds every live category, so the page's own comes out of it.
    let nav = categories(cx).await?;
    let node = require_category_in(&nav, path_param::<Slug>(cx))?;
    let category = node.category.clone();
    let children = node.children.clone();
    let crumbs = category_path(&nav, &category);
    let query = query_params::<CategoryQuery>(cx)?;
    let sort = query
        .sort
        .as_deref()
        .map(ProductSort::parse)
        .unwrap_or_default();
    // The products of this category and of everything below it, which is
    // what a shopper expects from a top-level entry.
    let page = products(
        cx,
        &ProductQuery::default()
            .in_category(category.id.clone())
            .sort(sort)
            .per_page(PER_PAGE)
            .page(query.page.unwrap_or(1)),
    )
    .await?;
    let link = {
        let sort = sort.as_str();
        let slug = category.slug.clone();
        move |number: u32| {
            href!(index, Slug(slug.clone()))
                .query(CategoryLink { sort, page: number })
                .resolve(cx)
        }
    };

    Ok(view! {
        document(
            title: &category.name,
            nav: &nav,
            <nav class="crumbs" aria-label="Breadcrumb" data-crumbs="">
                <a href=(href!(crate::app::index))>"Catalog"</a>
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
                <nav aria-label="Sub-categories">
                    <ul>
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
            <form class="sort" method="get">
                <label for="sort">"Sort by "</label>
                <select id="sort" name="sort" data-sort="">
                    <option value="newest" selected=(sort == ProductSort::Newest)>"Newest"</option>
                    <option value="price_asc" selected=(sort == ProductSort::PriceAsc)>"Price, low to high"</option>
                    <option value="price_desc" selected=(sort == ProductSort::PriceDesc)>"Price, high to low"</option>
                    <option value="top_rated" selected=(sort == ProductSort::TopRated)>"Top rated"</option>
                </select>
                <button>"Sort"</button>
            </form>
            if page.items.is_empty() {
                <p>"Nothing in this category yet."</p>
            }
            catalog_grid(products: &page.items)
            if page.pages() > 1 {
                <nav class="pages" aria-label="Pages">
                    if page.has_prev() { <a href=(link(page.page - 1))>"Previous"</a> }
                    <span>"Page " (page.page) " of " (page.pages())</span>
                    if page.has_next() { <a href=(link(page.page + 1))>"Next"</a> }
                </nav>
            }
        )
    })
}
