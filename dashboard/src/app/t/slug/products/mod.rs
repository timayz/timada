//! `/t/{slug}/products`: the catalog of a tenant, for members with
//! `products:read`; those with `products:manage` create and edit. The list
//! comes from the shared read model (a moment behind), a product's own page
//! reads its stream.

pub mod create;
pub mod product_id;

use timada_catalog::{CatalogError, Command, MAX_NAME_LEN, count_products, products_of_tenant};
use timada_tenancy::{PERMISSION_PRODUCTS_MANAGE, PERMISSION_PRODUCTS_READ};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params},
    view::{View, view},
};

use super::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::app::auth::Flash;
use crate::cluster::Executor;
use crate::db::ReadPool;
use crate::ui::{document, format_time, notice};

const PAGE_SIZE: i64 = 50;

#[topcoat::router::query_params(error = bad_request)]
struct ProductsQuery {
    /// Matches the slug or the name.
    q: Option<String>,
    /// 1-based.
    page: Option<i64>,
    error: Option<String>,
    message: Option<String>,
}

#[derive(serde::Serialize)]
struct ProductsLink<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    q: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    page: Option<i64>,
}

pub fn products_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, Slug(slug.to_owned()))
        .query(Flash { error, message })
        .resolve(cx)
}

/// The catalog gateway over the tenant's own store (403 while suspended).
pub fn catalog(access: &TenantAccess) -> Result<Command<Executor>> {
    Ok(Command(access.executor()?.clone()))
}

/// The query-string code of a user-facing catalog error.
pub fn error_code(err: &CatalogError) -> &'static str {
    match err {
        CatalogError::InvalidName => "invalid_product_name",
        CatalogError::InvalidSlug => "invalid_product_slug",
        CatalogError::SlugTaken => "product_slug_taken",
        CatalogError::InvalidDescription => "invalid_description",
        CatalogError::InvalidAmount => "invalid_amount",
        CatalogError::InvalidCurrency => "invalid_currency",
        CatalogError::NotFound => "product_not_found",
        CatalogError::Archived => "product_archived",
        CatalogError::NotPriced => "product_not_priced",
        CatalogError::Write(_) | CatalogError::Other(_) => "failed",
    }
}

/// The word for a product's state on the pages.
pub fn status_label(status: timada_catalog::ProductStatus) -> &'static str {
    match status {
        timada_catalog::ProductStatus::Draft => "Draft",
        timada_catalog::ProductStatus::Published => "Published",
        timada_catalog::ProductStatus::Archived => "Archived",
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PRODUCTS_READ)
        .await?;
    let can_manage = access.can(cx, PERMISSION_PRODUCTS_MANAGE).await?;
    let query = query_params::<ProductsQuery>(cx)?;
    let q = query.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    let page_number = query.page.unwrap_or(1).max(1);
    let nav = TenantNav::of(cx, &access).await?;
    let TenantAccess { tenant, .. } = access;
    let pool = app_context::<ReadPool>(cx);
    let total = count_products(pool, &tenant.id, q).await?;
    let rows = products_of_tenant(
        pool,
        &tenant.id,
        q,
        (page_number - 1) * PAGE_SIZE,
        PAGE_SIZE,
    )
    .await?;
    let pages = (total + PAGE_SIZE - 1) / PAGE_SIZE;
    let slug = tenant.slug.clone();
    let page_link = {
        let slug = slug.clone();
        move |page: i64| {
            href!(index, Slug(slug.clone()))
                .query(ProductsLink {
                    q,
                    page: Some(page),
                })
                .resolve(cx)
        }
    };
    let title = format!("Products · {}", tenant.name);

    Ok(view! {
        document(
            title: &title,
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">"Products"</h1>
                <nav class="flex items-center gap-4 text-sm">
                    <a class="underline" href=(href!(super::index, Slug(slug.clone())))>(&tenant.name)</a>
                    if nav.members {
                        <a class="underline" href=(href!(super::members::index, Slug(slug.clone())))>"Members"</a>
                    }
                    if nav.roles {
                        <a class="underline" href=(href!(super::roles::index, Slug(slug.clone())))>"Roles"</a>
                    }
                    if nav.api_keys {
                        <a class="underline" href=(href!(super::api_keys::index, Slug(slug.clone())))>"API keys"</a>
                    }
                    <a class="underline" href=(href!(crate::app::home))>"Dashboard"</a>
                </nav>
            </header>
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <form method="get" class="mb-4 flex gap-2">
                    <label class="sr-only" for="products-search">"Search"</label>
                    <input id="products-search" name="q" type="search" value=(q.unwrap_or(""))
                        class="w-full rounded border border-neutral-300 px-3 py-2" placeholder="Name or slug">
                    <button class="rounded border border-neutral-300 px-3 py-2">"Search"</button>
                </form>
                <table class="w-full text-left text-sm">
                    <thead>
                        <tr class="border-b border-neutral-200 text-neutral-500">
                            <th class="py-2 font-medium">"Name"</th>
                            <th class="py-2 font-medium">"Slug"</th>
                            <th class="py-2 font-medium">"Price"</th>
                            <th class="py-2 font-medium">"Status"</th>
                            <th class="py-2 font-medium">"Changed"</th>
                        </tr>
                    </thead>
                    <tbody class="divide-y divide-neutral-100">
                        if rows.is_empty() {
                            <tr><td class="py-2 text-neutral-500" colspan="5">"No product yet."</td></tr>
                        }
                        for row in &rows {
                            <tr data-product=(&row.slug) data-product-status=(&row.status)>
                                <td class="py-2">
                                    <a class="underline" href=(href!(product_id::index, Slug(slug.clone()), product_id::ProductId(row.id.clone())))>(&row.name)</a>
                                </td>
                                <td class="py-2"><code>(&row.slug)</code></td>
                                <td class="py-2">(row.price().map(|p| p.to_string()).unwrap_or_else(|| "—".to_owned()))</td>
                                <td class="py-2">(status_label(row.status()))</td>
                                <td class="py-2"><time datetime=(row.updated_at)>(format_time(row.updated_at as u64))</time></td>
                            </tr>
                        }
                    </tbody>
                </table>
                if pages > 1 {
                    <p class="mt-4 flex items-center gap-4 text-sm">
                        if page_number > 1 { <a class="underline" href=(page_link(page_number - 1))>"Previous"</a> }
                        <span>"Page " (page_number) " of " (pages)</span>
                        if page_number < pages { <a class="underline" href=(page_link(page_number + 1))>"Next"</a> }
                    </p>
                }
            </section>

            if can_manage {
                <section class="rounded border border-neutral-200 bg-white p-4">
                    <h2 class="mb-2 text-lg font-medium">"Create a product"</h2>
                    <p class="mb-3 text-sm text-neutral-500">
                        "A new product is a draft: set its price and publish it from its page. The slug derives from the name unless given."
                    </p>
                    <form method="post" action=(href!(create::create, Slug(slug.clone()))) class="space-y-3">
                        <label class="block text-sm" for="new-product-name">"Name"</label>
                        <input id="new-product-name" name="name" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2"
                            type="text" maxlength=(MAX_NAME_LEN) required="" placeholder="Blue mug">
                        <label class="block text-sm" for="new-product-slug">"Slug (optional)"</label>
                        <input id="new-product-slug" name="slug" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2"
                            type="text" placeholder="blue-mug">
                        <button class="rounded bg-neutral-900 px-4 py-2 font-medium text-white hover:bg-neutral-700">"Create product"</button>
                    </form>
                </section>
            }
        )
    })
}
