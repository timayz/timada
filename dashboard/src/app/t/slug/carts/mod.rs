//! `/t/{slug}/carts`: the carts shoppers are filling and have not ordered,
//! the most recently touched first, for members with `orders:read`. One
//! that stays still for days is a sale left behind.

use timada_orders::{count_open_carts, open_carts};
use timada_tenancy::PERMISSION_ORDERS_READ;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params},
    view::{View, view},
};

use super::{Slug, TenantAccess, TenantNav, require_tenant, tenant_nav};
use crate::db::ReadPool;
use crate::ui::{document, format_time};

const PAGE_SIZE: i64 = 50;

#[topcoat::router::query_params(error = bad_request)]
struct CartsQuery {
    /// 1-based.
    page: Option<i64>,
}

#[derive(serde::Serialize)]
struct CartsLink {
    page: i64,
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ORDERS_READ)
        .await?;
    let query = query_params::<CartsQuery>(cx)?;
    let page_number = query.page.unwrap_or(1).max(1);
    let nav = TenantNav::of(cx, &access).await?;
    let TenantAccess { tenant, .. } = access;
    let pool = app_context::<ReadPool>(cx);
    let total = count_open_carts(pool, &tenant.id).await?;
    let rows = open_carts(pool, &tenant.id, (page_number - 1) * PAGE_SIZE, PAGE_SIZE).await?;
    let pages = (total + PAGE_SIZE - 1) / PAGE_SIZE;
    let page_link = {
        let slug = tenant.slug.clone();
        move |page: i64| {
            href!(index, Slug(slug.clone()))
                .query(CartsLink { page })
                .resolve(cx)
        }
    };
    let title = format!("Carts · {}", tenant.name);

    Ok(view! {
        document(
            title: &title,
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">"Open carts"</h1>
                tenant_nav(tenant: &tenant, nav: &nav, current: "carts")
            </header>

            <section class="rounded border border-neutral-200 bg-white p-4">
                <p class="mb-3 text-sm text-neutral-500">
                    "Carts holding something that were not ordered yet. A cart becomes an order at checkout and leaves this list."
                </p>
                <table class="w-full text-left text-sm">
                    <thead>
                        <tr class="border-b border-neutral-200 text-neutral-500">
                            <th class="py-2 font-medium">"Cart"</th>
                            <th class="py-2 font-medium">"Products"</th>
                            <th class="py-2 font-medium">"Items"</th>
                            <th class="py-2 font-medium">"Started"</th>
                            <th class="py-2 font-medium">"Last change"</th>
                        </tr>
                    </thead>
                    <tbody class="divide-y divide-neutral-100">
                        if rows.is_empty() {
                            <tr><td class="py-2 text-neutral-500" colspan="5">"No open cart."</td></tr>
                        }
                        for row in &rows {
                            <tr data-cart=(&row.id)>
                                <td class="py-2"><code>(&row.id)</code></td>
                                <td class="py-2">(row.line_count)</td>
                                <td class="py-2" data-cart-items="">(row.item_count)</td>
                                <td class="py-2"><time datetime=(row.created_at)>(format_time(row.created_at as u64))</time></td>
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
        )
    })
}
