//! `/c/{category_slug}`: a category of the shop — where it sits in the tree,
//! the categories under it, and its products, those of its subcategories
//! included. A category that was archived (or sits under one that was) is
//! not found.

use timada_catalog::{
    CategoryRow, category_by_slug, category_lineage, is_on_storefront, list_categories,
};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{error::RouterErrorExt, href, page, path_param, path_param as param},
    view::{View, view},
};

use super::{
    Crumb, Head, breadcrumb, catalog, document,
    listing::{Scope, listing_view, load_listing},
    seo::breadcrumb_json_ld,
};
use crate::Store;

path_param!(pub category_slug: String, error = not_found);

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

    let here = href!(show, CategorySlug(slug.clone())).resolve(cx);
    let listing = load_listing(
        cx,
        here.clone(),
        Scope {
            category_id: Some(category.id.clone()),
            brand_slug: None,
        },
    )
    .await?;
    let trail = crumbs(cx, &lineage, false);
    let head = Head {
        description: Some(category.description.clone()).filter(|text| !text.is_empty()),
        canonical: listing.canonical(),
        robots: listing.robots(),
        previous: listing.previous(),
        next: listing.next(),
        json_ld: Some(breadcrumb_json_ld(&trail, &here)),
    };

    Ok(view! {
        document(
            title: &category.name,
            head: Some(&head),
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
            listing_view(listing: &listing)
        )
    })
}
