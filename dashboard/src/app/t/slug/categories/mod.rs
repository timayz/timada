//! `/t/{slug}/categories`: the category tree of a tenant, for members with
//! `products:read`; those with `products:manage` organise it. The tree comes
//! from the shared read model (a moment behind), a category's own page reads
//! its stream.

pub mod category_id;
pub mod create;

use timada_catalog::{
    CatalogError, CategoryRow, MAX_CATEGORY_NAME_LEN, categories_of_tenant, tree_of,
};
use timada_tenancy::{PERMISSION_PRODUCTS_MANAGE, PERMISSION_PRODUCTS_READ};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params},
    view::{View, view},
};

use super::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::app::auth::Flash;
use crate::db::ReadPool;
use crate::ui::{Shell, document, notice};

#[topcoat::router::query_params(error = bad_request)]
struct CategoriesQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn categories_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, Slug(slug.to_owned()))
        .query(Flash { error, message })
        .resolve(cx)
}

/// The query-string code of a user-facing error on a category: the variants
/// a category shares with a product are worded for a category here, the rest
/// read the same either way.
pub fn error_code(err: &CatalogError) -> &'static str {
    match err {
        CatalogError::InvalidName => "invalid_category_name",
        CatalogError::InvalidSlug => "invalid_category_slug",
        CatalogError::SlugTaken => "category_slug_taken",
        CatalogError::InvalidDescription => "invalid_category_description",
        other => super::products::error_code(other),
    }
}

/// A category and how deep it sits. The two numbers a merchant needs are on
/// the row: `product_count`, filed under it, and `subtree_count`, what the
/// shop's page for it lists.
pub struct Row {
    pub category: CategoryRow,
    pub depth: usize,
}

impl Row {
    /// Whether its page shows more than what is filed directly under it.
    pub fn inherits(&self) -> bool {
        self.category.subtree_count != self.category.product_count
    }
}

/// The live categories of a tenant in tree order, each with its depth.
pub async fn rows_of(pool: &sqlx::SqlitePool, tenant_id: &str) -> Result<Vec<Row>> {
    fn walk(nodes: Vec<timada_catalog::TreeOf<CategoryRow>>, depth: usize, out: &mut Vec<Row>) {
        for node in nodes {
            out.push(Row {
                category: node.item,
                depth,
            });
            walk(node.children, depth + 1, out);
        }
    }
    let mut out = Vec::new();
    walk(
        tree_of(categories_of_tenant(pool, tenant_id).await?),
        0,
        &mut out,
    );
    Ok(out)
}

/// A category as an option of a parent `<select>`: owned, because the view
/// takes what it renders.
pub struct Choice {
    pub id: String,
    /// The name, indented by its depth.
    pub label: String,
}

impl Choice {
    pub fn of(row: &Row) -> Self {
        Self {
            id: row.category.id.clone(),
            label: format!("{}{}", "— ".repeat(row.depth), row.category.name),
        }
    }
}

/// How a row is indented in the table.
fn indent(depth: usize) -> &'static str {
    match depth {
        0 => "",
        1 => "pl-6",
        _ => "pl-12",
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PRODUCTS_READ)
        .await?;
    let can_manage = access.can(cx, PERMISSION_PRODUCTS_MANAGE).await?;
    let query = query_params::<CategoriesQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let TenantAccess { tenant, .. } = access;
    let pool = app_context::<ReadPool>(cx);
    let rows = rows_of(pool, &tenant.id).await?;
    let slug = tenant.slug.clone();
    let title = format!("Categories · {}", tenant.name);
    // A category at the deepest level takes no children of its own.
    let parents: Vec<Choice> = rows
        .iter()
        .filter(|row| row.depth + 2 <= timada_catalog::MAX_CATEGORY_DEPTH)
        .map(Choice::of)
        .collect();

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "categories"),
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">"Categories"</h1>
            </header>
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <p class="mb-4 text-sm text-neutral-500">
                    "Categories are what a shopper browses: a top-level one holds sub-categories, three levels deep at most. A product belongs to as many as fit it."
                </p>
                <p class="mb-4 text-sm text-neutral-500">
                    "A category's page in the shop lists what is filed under it and under every sub-category below it, so a product in a sub-category counts for its parents too."
                </p>
                <table class="w-full text-left text-sm">
                    <thead>
                        <tr class="border-b border-neutral-200 text-neutral-500">
                            <th class="py-2 font-medium">"Name"</th>
                            <th class="py-2 font-medium">"Slug"</th>
                            <th class="py-2 font-medium">"On its page"</th>
                        </tr>
                    </thead>
                    <tbody class="divide-y divide-neutral-100">
                        if rows.is_empty() {
                            <tr><td class="py-2 text-neutral-500" colspan="3">"No category yet."</td></tr>
                        }
                        for row in &rows {
                            <tr data-category=(&row.category.slug) data-category-depth=(row.depth)>
                                <td class=(format!("py-2 {}", indent(row.depth)))>
                                    <a class="underline" href=(href!(category_id::index, Slug(slug.clone()), category_id::CategoryId(row.category.id.clone())))>(&row.category.name)</a>
                                    if !row.category.in_menu {
                                        " " <span class="rounded bg-neutral-100 px-1.5 py-0.5 text-xs text-neutral-600" data-hidden-from-menu="">"Hidden from menu"</span>
                                    }
                                </td>
                                <td class="py-2"><code>(&row.category.slug)</code></td>
                                <td class="py-2">
                                    <span data-category-count=(row.category.subtree_count)>(row.category.subtree_count)</span>
                                    if row.inherits() {
                                        <span class="text-neutral-500" data-category-direct=(row.category.product_count)>
                                            " (" (row.category.product_count) " filed here)"
                                        </span>
                                    }
                                </td>
                            </tr>
                        }
                    </tbody>
                </table>
            </section>

            if can_manage {
                <section class="rounded border border-neutral-200 bg-white p-4">
                    <h2 class="mb-2 text-lg font-medium">"Create a category"</h2>
                    <form method="post" action=(href!(create::create, Slug(slug.clone()))) class="space-y-3">
                        <label class="block text-sm" for="new-category-name">"Name"</label>
                        <input id="new-category-name" name="name" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2"
                            type="text" maxlength=(MAX_CATEGORY_NAME_LEN) required="" placeholder="Cat bedding">
                        <label class="block text-sm" for="new-category-parent">"Parent"</label>
                        <select id="new-category-parent" name="parent_id" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2">
                            <option value="">"Top level"</option>
                            for parent in &parents {
                                <option value=(&parent.id)>(&parent.label)</option>
                            }
                        </select>
                        <label class="block text-sm" for="new-category-slug">"Slug (optional)"</label>
                        <input id="new-category-slug" name="slug" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2"
                            type="text" placeholder="cat-bedding">
                        <button class="rounded bg-neutral-900 px-4 py-2 font-medium text-white hover:bg-neutral-700">"Create category"</button>
                    </form>
                </section>
            }
        )
    })
}
