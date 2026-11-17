//! The whole catalogue at `/boutique`, newest first, searched with `?q=` and
//! narrowed by the price band and the shop's filters.

use timada_storefront::{ProductQuery, ProductSort, menu, option_values, products};
use topcoat::{
    Result,
    context::Cx,
    router::{href, page, query_params},
    view::{View, view},
};

use crate::app::{
    catalog_grid, document,
    filters::{Filters, Panel, filter_panel, shop_filters},
    words::t,
};

/// A listing shows this many at once.
pub const PER_PAGE: u32 = 36;

#[topcoat::router::query_params(error = bad_request)]
struct CatalogQuery {
    q: Option<String>,
    page: Option<u32>,
    min_price: Option<u32>,
    max_price: Option<u32>,
}

#[derive(serde::Serialize)]
struct CatalogLink<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    q: Option<&'a str>,
    /// Left out for the first page, so it has one address.
    #[serde(skip_serializing_if = "Option::is_none")]
    page: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    min_price: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_price: Option<u32>,
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
    let offered = shop_filters(cx).await?;
    let filters = Filters::read(cx, query.min_price, query.max_price, &offered);
    let wanted = filters.apply(wanted);
    let page = products(cx, &wanted).await?;
    let mut values = Vec::with_capacity(offered.len());
    for filter in offered {
        let found = option_values(cx, &wanted, &filter.name).await?;
        values.push((filter, found));
    }
    let nav = menu(cx).await?;
    let link = move |filters: &Filters, number: u32| {
        filters.onto(
            href!(index)
                .query(CatalogLink {
                    q: wanted.q.as_deref(),
                    page: (number > 1).then_some(number),
                    min_price: filters.min_price,
                    max_price: filters.max_price,
                })
                .resolve(cx),
        )
    };
    let panel = Panel::build(cx, &filters, &values, |filters| link(filters, 1));
    let title = match &q {
        Some(q) => t!(listing_results_for, q).resolve(cx).into_owned(),
        None => t!(footer_products).resolve(cx).into_owned(),
    };

    // A search or a filter shows products the plain catalogue lists too.
    let narrowed = q.is_some() || filters.is_active();
    // Search engines keep the plain catalogue for those, and each page of
    // it for itself.
    let canonical = if narrowed {
        href!(index).resolve(cx)
    } else {
        link(&Filters::default(), page.page)
    };

    Ok(view! {
        document(
            title: &title,
            noindex: narrowed,
            canonical: Some(canonical.as_str()),
            nav: &nav,
            <div class="wrap">
                <nav class="crumbs" aria-label=(t!(shell_crumbs)) data-crumbs="">
                    <a href=(href!(crate::app::index))>(t!(shell_home))</a> " / " (&title)
                </nav>
                <h1>(&title)</h1>
                filter_panel(panel: &panel)
                <div class="listing-head">
                    <p data-results=(page.total)>(t!(listing_results, count: page.total))</p>
                </div>
                if page.items.is_empty() {
                    if panel.active {
                        <p data-filters-empty="">
                            (t!(listing_none_filtered)) " "
                            <a href=(&panel.clear)>(t!(listing_filters_clear))</a>
                        </p>
                    } else {
                        <p>(t!(listing_none))</p>
                    }
                }
                catalog_grid(products: &page.items)
                if page.pages() > 1 {
                    <nav class="pages" aria-label=(t!(listing_pages))>
                        if page.has_prev() { <a href=(link(&filters, page.page - 1))>(t!(listing_previous))</a> }
                        <span>(t!(listing_page, page: page.page, pages: page.pages()))</span>
                        if page.has_next() { <a href=(link(&filters, page.page + 1))>(t!(listing_next))</a> }
                    </nav>
                }
            </div>
        )
    })
}
