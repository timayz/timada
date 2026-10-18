//! `/t/{slug}/categories/{category_id}`: one category, read from its own
//! stream (so an edit shows at once), with the forms that change it.

pub mod archive;
pub mod describe;
pub mod menu;
pub mod move_to;
pub mod position;
pub mod rename;

use timada_catalog::{
    CategoryRow, CategoryState, MAX_CATEGORY_DESCRIPTION_LEN, MAX_CATEGORY_NAME_LEN,
    category_by_id, live_child_count,
};
use timada_tenancy::{PERMISSION_PRODUCTS_MANAGE, PERMISSION_PRODUCTS_READ};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        error::{SeeOther, not_found, see_other},
        href, module_param, page, path_param, query_params,
    },
    view::{View, attributes, class, view},
};

use super::{Choice, error_code, rows_of};
use crate::app::auth::Flash;
use crate::app::t::slug::products::catalog;
use crate::app::t::slug::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::auth::user_labels;
use crate::components::badge::{BadgeVariant, badge};
use crate::components::breadcrumb::{
    breadcrumb, breadcrumb_item, breadcrumb_link, breadcrumb_list, breadcrumb_page,
    breadcrumb_separator,
};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::card::CARD;
use crate::components::checkbox::checkbox;
use crate::components::field::{field, field_label};
use crate::components::input::input;
use crate::components::label::label;
use crate::components::select::select;
use crate::components::textarea::textarea;
use crate::db::ReadPool;
use crate::ui::{
    LINK, Shell, document, fact, facts, format_time, notice, page_header, section_card,
};

module_param!(pub category_id);

#[topcoat::router::query_params(error = bad_request)]
struct CategoryQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn category_url(
    cx: &Cx,
    slug: &str,
    category_id: &str,
    error: Option<&str>,
    message: Option<&str>,
) -> String {
    href!(
        index,
        Slug(slug.to_owned()),
        CategoryId(category_id.to_owned())
    )
    .query(Flash { error, message })
    .resolve(cx)
}

/// The category named in the URL, or 404.
async fn require_category(cx: &Cx, access: &TenantAccess) -> Result<CategoryState> {
    let id = path_param::<CategoryId>(cx);
    catalog(access)?
        .load_category(id)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .ok_or_else(|| not_found().into())
}

/// What every action on a category shares: the member may manage products,
/// the category exists, and the outcome goes back to the category page.
async fn act<F>(cx: &Cx, message: &'static str, run: F) -> Result<SeeOther>
where
    F: AsyncFnOnce(
        &timada_catalog::Command<crate::cluster::Executor>,
        &str,
        &str,
    ) -> std::result::Result<(), timada_catalog::CatalogError>,
{
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PRODUCTS_MANAGE)
        .await?;
    let category_id = path_param::<CategoryId>(cx).to_owned();
    let slug = access.tenant.slug.clone();
    let gateway = catalog(&access)?;
    match run(&gateway, &access.user_id, &category_id).await {
        Ok(()) => Ok(see_other(category_url(
            cx,
            &slug,
            &category_id,
            None,
            Some(message),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(category_url(
            cx,
            &slug,
            &category_id,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PRODUCTS_READ)
        .await?;
    let can_manage = access.can(cx, PERMISSION_PRODUCTS_MANAGE).await?;
    let query = query_params::<CategoryQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let category = require_category(cx, &access).await?;
    let labels = user_labels(cx, std::slice::from_ref(&category.created_by)).await?;
    let created_by = labels
        .get(&category.created_by)
        .cloned()
        .unwrap_or_else(|| category.created_by.clone());
    let TenantAccess { tenant, .. } = access;
    let pool = app_context::<ReadPool>(cx);
    // The whole tree: it carries the counts, and it says where this category
    // may move to (anything live that is not itself or one of its
    // descendants, with room for it below).
    let rows = rows_of(pool, &tenant.id).await?;
    // Filed directly under it, and what its page in the shop lists, as the
    // subscription stored them (the same numbers the tree shows).
    let (filed_here, on_its_page) = category_by_id(pool, &tenant.id, &category.id)
        .await?
        .map(|row| (row.product_count, row.subtree_count))
        .unwrap_or_default();
    // The breadcrumb, read off the tree already loaded. The parent comes
    // from the category itself, so a move shows at once.
    let ancestors: Vec<CategoryRow> = {
        let mut chain: Vec<CategoryRow> = Vec::new();
        let mut next = category.parent_id.as_deref();
        while let Some(parent_id) = next {
            if chain.len() > timada_catalog::MAX_CATEGORY_DEPTH
                || parent_id == category.id
                || chain.iter().any(|seen| seen.id == parent_id)
            {
                break;
            }
            let Some(parent) = rows.iter().find(|row| row.category.id == parent_id) else {
                break;
            };
            chain.push(parent.category.clone());
            next = parent.category.parent_id.as_deref();
        }
        chain.reverse();
        chain
    };
    let children = live_child_count(pool, &tenant.id, &category.id).await?;
    let descendants: Vec<&str> = {
        let mut ids = vec![category.id.as_str()];
        for row in &rows {
            if let Some(parent) = row.category.parent_id.as_deref()
                && ids.contains(&parent)
            {
                ids.push(row.category.id.as_str());
            }
        }
        ids
    };
    let parents: Vec<Choice> = rows
        .iter()
        .filter(|row| {
            !descendants.contains(&row.category.id.as_str())
                && row.depth + 2 <= timada_catalog::MAX_CATEGORY_DEPTH
        })
        .map(Choice::of)
        .collect();
    let slug = tenant.slug.clone();
    let editable = can_manage && !category.archived;
    let title = format!("{} · Categories · {}", category.name, tenant.name);

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "categories"),
            breadcrumb(
                attrs: attributes! { class="mb-4" data-breadcrumb="" },
                breadcrumb_list(
                    breadcrumb_item(breadcrumb_link(attrs: attributes! { href=(href!(super::index, Slug(slug.clone()))) }, "All categories"))
                    for ancestor in &ancestors {
                        breadcrumb_separator()
                        breadcrumb_item(breadcrumb_link(attrs: attributes! { href=(href!(index, Slug(slug.clone()), CategoryId(ancestor.id.clone()))) }, (&ancestor.name)))
                    }
                    breadcrumb_separator()
                    breadcrumb_item(breadcrumb_page((&category.name)))
                )
            )
            page_header(
                title: &category.name,
                h1_attrs: attributes! { data-category-id=(&category.id) },
                if category.archived {
                    badge(variant: BadgeVariant::Secondary, attrs: attributes! { data-category-archived="" }, "Archived")
                }
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            if children > 0 {
                <p class="mb-4 text-sm text-muted-foreground">
                    "Its page in the shop also lists what is filed under its sub-categories, so taking a product out of this category does not hide it there while it stays in one of them."
                </p>
            }

            section_card(
                title: Some("Overview"),
                facts(
                    fact(term: "Slug", <code class="text-xs">(&category.slug)</code>)
                    fact(
                        term: "Description",
                        attrs: attributes! { data-category-description="" class="whitespace-pre-line" },
                        (if category.description.is_empty() { "—" } else { category.description.as_str() })
                    )
                    fact(
                        term: "On its page",
                        attrs: attributes! { data-category-products=(on_its_page) },
                        (on_its_page) " product" (if on_its_page == 1 { "" } else { "s" })
                        if on_its_page != filed_here {
                            <span class="text-muted-foreground">", counting its sub-categories"</span>
                        }
                    )
                    fact(
                        term: "Filed here",
                        <a class=(LINK) data-category-direct=(filed_here) href=(href!(crate::app::t::slug::products::index, Slug(slug.clone())).query(ProductsOfCategory { category: &category.id }).resolve(cx))>
                            (filed_here) " product" (if filed_here == 1 { "" } else { "s" })
                        </a>
                    )
                    fact(term: "Sub-categories", attrs: attributes! { data-category-children=(children) }, (children))
                    fact(term: "Position", (category.position))
                    fact(
                        term: "Shop menu",
                        attrs: attributes! { data-category-menu=(if category.hidden_from_menu { "hidden" } else { "shown" }) },
                        if category.hidden_from_menu {
                            "Hidden, with its sub-categories; its page stays open"
                        } else {
                            "Shown"
                        }
                    )
                    fact(term: "Created", (&created_by) ", " <time datetime=(category.created_at)>(format_time(category.created_at))</time>)
                    fact(term: "Changed", <time datetime=(category.updated_at)>(format_time(category.updated_at))</time>)
                )
            )

            if category.archived {
                <p class="mb-6 text-sm text-muted-foreground">
                    "An archived category cannot change any more. The products that were in it keep the membership until they leave it."
                </p>
            }

            if editable {
                <section class="mb-6 grid gap-4 md:grid-cols-2">
                    <form method="post" action=(href!(rename::rename, Slug(slug.clone()), CategoryId(category.id.clone()))) class=(class!(CARD, "gap-4 px-6"))>
                        field(
                            field_label(attrs: attributes! { for="category-name" }, "Name")
                            input(attrs: attributes! {
                                id="category-name" name="name" type="text"
                                maxlength=(MAX_CATEGORY_NAME_LEN) required="" value=(&category.name)
                            })
                        )
                        <div>button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Rename")</div>
                    </form>
                    <form method="post" action=(href!(move_to::move_to, Slug(slug.clone()), CategoryId(category.id.clone()))) class=(class!(CARD, "gap-4 px-6"))>
                        field(
                            field_label(attrs: attributes! { for="category-parent" }, "Parent")
                            select(
                                attrs: attributes! { id="category-parent" name="parent_id" },
                                <option value="" selected=(category.parent_id.is_none())>"Top level"</option>
                                for parent in &parents {
                                    <option value=(&parent.id) selected=(category.parent_id.as_deref() == Some(parent.id.as_str()))>
                                        (&parent.label)
                                    </option>
                                }
                            )
                        )
                        <div>button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Move")</div>
                    </form>
                    <form method="post" action=(href!(position::position, Slug(slug.clone()), CategoryId(category.id.clone()))) class=(class!(CARD, "gap-4 px-6"))>
                        field(
                            field_label(attrs: attributes! { for="category-position" }, "Position among its siblings")
                            input(attrs: attributes! {
                                id="category-position" name="position" type="number"
                                min="0" max="10000" required="" value=(category.position)
                            })
                        )
                        <div>button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Set position")</div>
                    </form>
                    <form method="post" action=(href!(describe::describe, Slug(slug.clone()), CategoryId(category.id.clone()))) class=(class!(CARD, "gap-4 px-6"))>
                        field(
                            field_label(attrs: attributes! { for="category-description" }, "Description")
                            textarea(
                                attrs: attributes! { id="category-description" name="description" rows="4" maxlength=(MAX_CATEGORY_DESCRIPTION_LEN) },
                                (&category.description)
                            )
                        )
                        <div>button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Save description")</div>
                    </form>
                    <form method="post" action=(href!(menu::menu, Slug(slug.clone()), CategoryId(category.id.clone()))) class=(class!(CARD, "gap-4 px-6"))>
                        label(
                            checkbox(attrs: attributes! { name="in_menu" value="on" checked=(!category.hidden_from_menu) })
                            "Show in the shop menu"
                        )
                        <p class="text-sm text-muted-foreground">"A hidden category takes its sub-categories out of the menu too. Its page and products stay reachable."</p>
                        <div>button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Save menu visibility")</div>
                    </form>
                </section>

                <section class="flex flex-wrap items-center gap-3">
                    if children > 0 {
                        <p class="text-sm text-muted-foreground">"Archive or move its sub-categories before archiving it."</p>
                    } else {
                        <form method="post" action=(href!(archive::archive, Slug(slug.clone()), CategoryId(category.id.clone())))>
                            button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Archive")
                        </form>
                    }
                </section>
            }
        )
    })
}

/// The products link of a category page.
#[derive(serde::Serialize)]
struct ProductsOfCategory<'a> {
    category: &'a str,
}

#[derive(serde::Deserialize)]
pub struct RenameForm {
    pub name: String,
}

#[derive(serde::Deserialize)]
pub struct DescribeForm {
    #[serde(default)]
    pub description: String,
}

#[derive(serde::Deserialize)]
pub struct MoveForm {
    #[serde(default)]
    pub parent_id: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct PositionForm {
    pub position: u32,
}

#[derive(serde::Deserialize)]
pub struct MenuForm {
    /// An unticked checkbox is not sent at all.
    #[serde(default)]
    pub in_menu: Option<String>,
}
