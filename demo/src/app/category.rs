//! `/c/{category_slug}`: a category of the shop — where it sits in the tree,
//! the categories under it, and its products, those of its subcategories
//! included. A category that was archived (or sits under one that was) is
//! not found.

use timada_catalog::{
    CategoryRow, ProductListRow, category_by_slug, category_lineage, category_subtree_ids,
    count_products_in_categories, is_on_storefront, list_categories, products_in_categories,
};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        error::RouterErrorExt, href, page, path_param, path_param as param, query_params,
        query_params as query,
    },
    view::{View, component, view},
};

use super::{Crumb, breadcrumb, catalog, document};
use crate::Store;

path_param!(pub category_slug: String, error = not_found);

const PRODUCTS_PER_PAGE: u32 = 24;

#[query_params(error = bad_request)]
struct CategoryQuery {
    page: Option<u32>,
}

/// The trail down to a category, each step linked but the last — or every
/// step, for a page further down (a product's).
pub fn crumbs(cx: &Cx, lineage: &[CategoryRow], link_last: bool) -> Vec<Crumb> {
    let last = lineage.len().saturating_sub(1);
    let mut trail = vec![Crumb {
        label: "Catalogue".to_owned(),
        link: Some(href!(catalog::home).resolve(cx)),
    }];
    trail.extend(lineage.iter().enumerate().map(|(index, category)| {
        Crumb {
            label: category.name.clone(),
            link: (link_last || index < last)
                .then(|| href!(show, CategorySlug(category.slug.clone())).resolve(cx)),
        }
    }));
    trail
}

#[page("/c/{category_slug}")]
pub async fn show(cx: &Cx) -> Result<impl View> {
    let slug = param::<CategorySlug>(cx)?.clone();
    let store = app_context::<Store>(cx);
    let category = category_by_slug(&store.db, &slug)
        .await?
        .ok_or_not_found()?;
    let lineage = category_lineage(&store.db, &category.id).await?;
    is_on_storefront(&lineage).then_some(()).ok_or_not_found()?;

    let children: Vec<(String, String)> = list_categories(&store.db, false)
        .await?
        .into_iter()
        .filter(|c| c.parent_id.as_deref() == Some(category.id.as_str()))
        .map(|c| (href!(show, CategorySlug(c.slug)).resolve(cx), c.name))
        .collect();

    let under = category_subtree_ids(&store.db, &category.id).await?;
    let total = count_products_in_categories(&store.db, &under).await?;
    let pages = (total.max(0) as u32).div_ceil(PRODUCTS_PER_PAGE).max(1);
    let page = query::<CategoryQuery>(cx)?
        .page
        .unwrap_or(1)
        .clamp(1, pages);
    let products = products_in_categories(
        &store.db,
        &under,
        PRODUCTS_PER_PAGE,
        (page - 1) * PRODUCTS_PER_PAGE,
    )
    .await?;
    let here = href!(show, CategorySlug(slug.clone())).resolve(cx);
    let page_link = |n: u32| match n {
        1 => here.clone(),
        n => format!("{here}?page={n}"),
    };
    let previous = (page > 1).then(|| page_link(page - 1));
    let next = (page < pages).then(|| page_link(page + 1));
    let trail = crumbs(cx, &lineage, false);

    Ok(view! {
        document(
            title: &category.name,
            breadcrumb(trail: &trail)
            <h1>(category.name.clone())</h1>
            if !category.description.is_empty() {
                <p>(category.description.clone())</p>
            }
            if !children.is_empty() {
                <nav aria-label="Sous-catégories">
                    <ul class="tags">
                        for (link, name) in &children {
                            <li><a href=(link.clone())>(name.clone())</a></li>
                        }
                    </ul>
                </nav>
            }
            if products.is_empty() {
                <p class="muted">"Aucun produit dans cette catégorie pour le moment."</p>
            } else {
                <p class="muted">(total.to_string()) " produit(s)"</p>
                product_list(products: &products)
                if pages > 1 {
                    <nav aria-label="Pages" class="pager">
                        if let Some(previous) = &previous { <a href=(previous.clone()) rel="prev">"← Précédents"</a> }
                        <span>"Page " (page.to_string()) " / " (pages.to_string())</span>
                        if let Some(next) = &next { <a href=(next.clone()) rel="next">"Suivants →"</a> }
                    </nav>
                }
            }
        )
    })
}

/// Products by name, each linked to its page.
#[component]
pub async fn product_list(cx: &Cx, products: &[ProductListRow]) -> Result<impl View> {
    let lines: Vec<(String, &ProductListRow)> = products
        .iter()
        .map(|product| {
            let link = href!(
                catalog::product_page,
                catalog::ProductId(product.id.clone())
            )
            .resolve(cx);
            (link, product)
        })
        .collect();
    Ok(view! {
        <ul>
            for (link, product) in &lines {
                <li><a href=(link.clone())>(product.name.clone())</a> " " <span class="muted">(product.sku.clone())</span></li>
            }
        </ul>
    })
}
