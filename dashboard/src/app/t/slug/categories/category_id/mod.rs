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
    view::{View, view},
};

use super::{Choice, error_code, rows_of};
use crate::app::auth::Flash;
use crate::app::t::slug::products::catalog;
use crate::app::t::slug::{Slug, TenantAccess, TenantNav, require_tenant, tenant_nav};
use crate::auth::user_labels;
use crate::db::ReadPool;
use crate::ui::{document, format_time, notice};

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
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold" data-category-id=(&category.id)>
                    (&category.name)
                    if category.archived {
                        " " <span class="rounded bg-neutral-100 px-2 text-sm font-normal text-neutral-700" data-category-archived="">"Archived"</span>
                    }
                </h1>
                tenant_nav(tenant: &tenant, nav: &nav, current: "categories")
            </header>
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            <nav class="mb-4 text-sm text-neutral-500" aria-label="Breadcrumb" data-breadcrumb="">
                <a class="underline" href=(href!(super::index, Slug(slug.clone())))>"Categories"</a>
                for ancestor in &ancestors {
                    " / "
                    <a class="underline" href=(href!(index, Slug(slug.clone()), CategoryId(ancestor.id.clone())))>(&ancestor.name)</a>
                }
                " / " (&category.name)
            </nav>

            if children > 0 {
                <p class="mb-4 text-sm text-neutral-500">
                    "Its page in the shop also lists what is filed under its sub-categories, so taking a product out of this category does not hide it there while it stays in one of them."
                </p>
            }

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <h2 class="mb-2 text-lg font-medium">"Overview"</h2>
                <dl class="grid grid-cols-[10rem_1fr] gap-y-1 text-sm">
                    <dt class="text-neutral-500">"Slug"</dt><dd><code>(&category.slug)</code></dd>
                    <dt class="text-neutral-500">"Description"</dt>
                    <dd data-category-description="" class="whitespace-pre-line">(if category.description.is_empty() { "—" } else { category.description.as_str() })</dd>
                    <dt class="text-neutral-500">"On its page"</dt>
                    <dd data-category-products=(on_its_page)>
                        (on_its_page) " product" (if on_its_page == 1 { "" } else { "s" })
                        if on_its_page != filed_here {
                            <span class="text-neutral-500">", counting its sub-categories"</span>
                        }
                    </dd>
                    <dt class="text-neutral-500">"Filed here"</dt>
                    <dd>
                        <a class="underline" data-category-direct=(filed_here) href=(href!(crate::app::t::slug::products::index, Slug(slug.clone())).query(ProductsOfCategory { category: &category.id }).resolve(cx))>
                            (filed_here) " product" (if filed_here == 1 { "" } else { "s" })
                        </a>
                    </dd>
                    <dt class="text-neutral-500">"Sub-categories"</dt><dd data-category-children=(children)>(children)</dd>
                    <dt class="text-neutral-500">"Position"</dt><dd>(category.position)</dd>
                    <dt class="text-neutral-500">"Shop menu"</dt>
                    <dd data-category-menu=(if category.hidden_from_menu { "hidden" } else { "shown" })>
                        if category.hidden_from_menu {
                            "Hidden, with its sub-categories; its page stays open"
                        } else {
                            "Shown"
                        }
                    </dd>
                    <dt class="text-neutral-500">"Created"</dt>
                    <dd>(&created_by) ", " <time datetime=(category.created_at)>(format_time(category.created_at))</time></dd>
                    <dt class="text-neutral-500">"Changed"</dt>
                    <dd><time datetime=(category.updated_at)>(format_time(category.updated_at))</time></dd>
                </dl>
            </section>

            if category.archived {
                <p class="mb-8 text-sm text-neutral-500">
                    "An archived category cannot change any more. The products that were in it keep the membership until they leave it."
                </p>
            }

            if editable {
                <section class="mb-8 grid gap-4 md:grid-cols-2">
                    <form method="post" action=(href!(rename::rename, Slug(slug.clone()), CategoryId(category.id.clone()))) class="rounded border border-neutral-200 bg-white p-4">
                        <label class="block text-sm font-medium" for="category-name">"Name"</label>
                        <input id="category-name" name="name" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2"
                            type="text" maxlength=(MAX_CATEGORY_NAME_LEN) required="" value=(&category.name)>
                        <button class="mt-3 rounded border border-neutral-300 px-3 py-1 text-sm">"Rename"</button>
                    </form>
                    <form method="post" action=(href!(move_to::move_to, Slug(slug.clone()), CategoryId(category.id.clone()))) class="rounded border border-neutral-200 bg-white p-4">
                        <label class="block text-sm font-medium" for="category-parent">"Parent"</label>
                        <select id="category-parent" name="parent_id" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2">
                            <option value="" selected=(category.parent_id.is_none())>"Top level"</option>
                            for parent in &parents {
                                <option value=(&parent.id) selected=(category.parent_id.as_deref() == Some(parent.id.as_str()))>
                                    (&parent.label)
                                </option>
                            }
                        </select>
                        <button class="mt-3 rounded border border-neutral-300 px-3 py-1 text-sm">"Move"</button>
                    </form>
                    <form method="post" action=(href!(position::position, Slug(slug.clone()), CategoryId(category.id.clone()))) class="rounded border border-neutral-200 bg-white p-4">
                        <label class="block text-sm font-medium" for="category-position">"Position among its siblings"</label>
                        <input id="category-position" name="position" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2"
                            type="number" min="0" max="10000" required="" value=(category.position)>
                        <button class="mt-3 rounded border border-neutral-300 px-3 py-1 text-sm">"Set position"</button>
                    </form>
                    <form method="post" action=(href!(describe::describe, Slug(slug.clone()), CategoryId(category.id.clone()))) class="rounded border border-neutral-200 bg-white p-4">
                        <label class="block text-sm font-medium" for="category-description">"Description"</label>
                        <textarea id="category-description" name="description" rows="4" maxlength=(MAX_CATEGORY_DESCRIPTION_LEN)
                            class="mt-1 w-full rounded border border-neutral-300 px-3 py-2">(&category.description)</textarea>
                        <button class="mt-3 rounded border border-neutral-300 px-3 py-1 text-sm">"Save description"</button>
                    </form>
                    <form method="post" action=(href!(menu::menu, Slug(slug.clone()), CategoryId(category.id.clone()))) class="rounded border border-neutral-200 bg-white p-4">
                        <label class="flex items-center gap-2 text-sm font-medium">
                            <input name="in_menu" type="checkbox" value="on" checked=(!category.hidden_from_menu)>
                            "Show in the shop menu"
                        </label>
                        <p class="mt-1 text-sm text-neutral-500">"A hidden category takes its sub-categories out of the menu too. Its page and products stay reachable."</p>
                        <button class="mt-3 rounded border border-neutral-300 px-3 py-1 text-sm">"Save menu visibility"</button>
                    </form>
                </section>

                <section class="flex flex-wrap items-center gap-3">
                    if children > 0 {
                        <p class="text-sm text-neutral-500">"Archive or move its sub-categories before archiving it."</p>
                    } else {
                        <form method="post" action=(href!(archive::archive, Slug(slug.clone()), CategoryId(category.id.clone())))>
                            <button class="rounded border border-red-700 px-3 py-1 text-sm text-red-700 hover:bg-red-50">"Archive"</button>
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
