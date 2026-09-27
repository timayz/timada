//! A category of the shop, at the slugs of the way down to it —
//! `/informatique/peripheriques/ecran-pc` — where it sits in the tree, the
//! categories under it, and its products, those of its subcategories
//! included. A category that was archived (or sits under one that was) is
//! not found. `/c/{category_slug}`, the address from before, moves
//! visitors on for good.

use timada_catalog::{
    CategoryRow, category_by_slug, category_lineage, effective_facets, is_on_storefront,
    list_categories, listed_counts_by_category,
};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        error::RouterErrorExt, error::redirect_permanent, href, page, path_param,
        path_param as param, request::uri,
    },
    view::{View, component, view},
};

use super::{
    Crumb, Head, breadcrumb, catalog, document,
    listing::{Scope, listing_view, load_listing},
    seo::breadcrumb_json_ld,
};
use crate::Store;

path_param!(pub category_slug: String, error = not_found);

/// The segments of a category's address: the slugs of its lineage, root
/// first.
pub fn path_of(lineage: &[CategoryRow]) -> Vec<String> {
    lineage
        .iter()
        .map(|category| category.slug.clone())
        .collect()
}

/// The address of the category `lineage` leads to.
pub fn link_of(cx: &Cx, lineage: &[CategoryRow]) -> String {
    href!(catalog::browse, catalog::Slugs(path_of(lineage))).resolve(cx)
}

/// The trail down to a category, each step linked but the last — or every
/// step, for a page further down (a product's).
pub fn crumbs(cx: &Cx, lineage: &[CategoryRow], link_last: bool) -> Vec<Crumb> {
    let last = lineage.len().saturating_sub(1);
    let mut trail = vec![Crumb {
        label: "Catalogue".to_owned(),
        link: Some(href!(catalog::home).resolve(cx)),
    }];
    trail.extend(lineage.iter().enumerate().map(|(index, category)| Crumb {
        label: category.name.clone(),
        link: (link_last || index < last).then(|| link_of(cx, &lineage[..=index])),
    }));
    trail
}

/// The address from before: on to the category's, for good. The query
/// string — filters, page — goes along.
#[page("/c/{category_slug}")]
pub async fn show(cx: &Cx) -> Result<()> {
    let slug = param::<CategorySlug>(cx)?.clone();
    let store = app_context::<Store>(cx);
    let category = category_by_slug(&store.db, &slug)
        .await?
        .ok_or_not_found()?;
    let lineage = category_lineage(&store.db, &category.id).await?;
    is_on_storefront(&lineage).then_some(()).ok_or_not_found()?;
    let mut target = link_of(cx, &lineage);
    if let Some(query) = uri(cx).query() {
        target.push('?');
        target.push_str(query);
    }
    Err(redirect_permanent(target).into())
}

/// The page of the category `lineage` leads to, `row` being its last step.
#[component]
pub async fn category_view(
    cx: &Cx,
    row: &CategoryRow,
    lineage: &[CategoryRow],
) -> Result<impl View> {
    let category = row.clone();
    let store = app_context::<Store>(cx);

    // The categories right under this one, each with how much it holds.
    let below: Vec<CategoryRow> = list_categories(&store.db, false)
        .await?
        .into_iter()
        .filter(|c| c.parent_id.as_deref() == Some(category.id.as_str()))
        .collect();
    let below_ids: Vec<String> = below.iter().map(|c| c.id.clone()).collect();
    let currency = crate::currency::shopper_currency(cx).await?;
    let counts = listed_counts_by_category(&store.db, &below_ids, Some(&currency)).await?;
    // `(link, label, picture)`: the drawn stand-in goes by the slug, like a
    // product's does by its reference. A child's address is this page's,
    // one step further.
    let here_path = path_of(lineage);
    let children: Vec<(String, String, String)> = below
        .into_iter()
        .map(|c| {
            let label = match counts.get(&c.id) {
                Some(count) => format!("{} ({count})", c.name),
                None => c.name,
            };
            let picture = format!("/media/demo/{}.svg", c.slug);
            let mut path = here_path.clone();
            path.push(c.slug);
            (
                href!(catalog::browse, catalog::Slugs(path)).resolve(cx),
                label,
                picture,
            )
        })
        .collect();

    let here = link_of(cx, lineage);
    let listing = load_listing(
        cx,
        here.clone(),
        Scope {
            category_id: Some(category.id.clone()),
            brand_slug: None,
            spec_facets: effective_facets(lineage),
        },
    )
    .await?;
    let trail = crumbs(cx, lineage, false);
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
                    <ul class="rail">
                        for (link, name, picture) in &children {
                            <li class="tile round">
                                <a href=(link.clone())>
                                    <span class="disc"><img src=(picture.clone()) alt="" width="160" height="160" loading="lazy" decoding="async"></span>
                                    (name.clone())
                                </a>
                            </li>
                        }
                    </ul>
                </nav>
            }
            listing_view(listing: &listing)
        )
    })
}
