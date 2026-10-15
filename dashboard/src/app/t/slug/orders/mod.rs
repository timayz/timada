//! `/t/{slug}/orders`: the orders shoppers placed on the tenant's
//! storefronts, for members with `orders:read`; those with `orders:manage`
//! move them along on an order's own page. The list comes from the shared
//! read model (a moment behind), an order's page reads its stream.

pub mod order_id;

use timada_orders::{Command, OrderFilter, OrderStatus, count_orders, orders_of_tenant};
use timada_tenancy::PERMISSION_ORDERS_READ;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params},
    view::{View, view},
};

use super::{Slug, TenantAccess, TenantNav, require_tenant, tenant_nav};
use crate::cluster::Executor;
use crate::db::ReadPool;
use crate::ui::{document, format_time, notice};

const PAGE_SIZE: i64 = 50;

#[topcoat::router::query_params(error = bad_request)]
struct OrdersQuery {
    /// One of the stored status values; anything else lists every order.
    status: Option<String>,
    /// Matches the reference, the e-mail address or the name it ships to.
    q: Option<String>,
    /// 1-based.
    page: Option<i64>,
    error: Option<String>,
    message: Option<String>,
}

#[derive(serde::Serialize)]
struct OrdersLink<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    q: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    page: Option<i64>,
}

/// The list of the orders matching an e-mail address.
pub fn orders_of_email_url(cx: &Cx, slug: &str, email: &str) -> String {
    href!(index, Slug(slug.to_owned()))
        .query(OrdersLink {
            status: None,
            q: Some(email),
            page: None,
        })
        .resolve(cx)
}

/// The orders gateway over the tenant's own store (403 while suspended).
pub fn orders(access: &TenantAccess) -> Result<Command<Executor>> {
    Ok(Command(access.executor()?.clone()))
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ORDERS_READ)
        .await?;
    let query = query_params::<OrdersQuery>(cx)?;
    let q = query.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    let status = query.status.as_deref().and_then(OrderStatus::parse);
    let page_number = query.page.unwrap_or(1).max(1);
    let nav = TenantNav::of(cx, &access).await?;
    let TenantAccess { tenant, .. } = access;
    let pool = app_context::<ReadPool>(cx);
    let filter = OrderFilter::default().status(status).search(q);
    let total = count_orders(pool, &tenant.id, &filter).await?;
    let rows = orders_of_tenant(
        pool,
        &tenant.id,
        &filter,
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
                .query(OrdersLink {
                    status: status.map(OrderStatus::as_str),
                    q,
                    page: Some(page),
                })
                .resolve(cx)
        }
    };
    let title = format!("Orders · {}", tenant.name);

    Ok(view! {
        document(
            title: &title,
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">"Orders"</h1>
                tenant_nav(tenant: &tenant, nav: &nav, current: "orders")
            </header>
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <form method="get" class="mb-4 flex gap-2">
                    <label class="sr-only" for="orders-search">"Search"</label>
                    <input id="orders-search" name="q" type="search" value=(q.unwrap_or(""))
                        class="w-full rounded border border-neutral-300 px-3 py-2" placeholder="Reference, e-mail or name">
                    <label class="sr-only" for="orders-status">"Status"</label>
                    <select id="orders-status" name="status" class="rounded border border-neutral-300 px-3 py-2">
                        <option value="">"Every status"</option>
                        for option in OrderStatus::ALL {
                            <option value=(option.as_str()) selected=(status == Some(option))>(option.label())</option>
                        }
                    </select>
                    <button class="rounded border border-neutral-300 px-3 py-2">"Filter"</button>
                </form>
                <table class="w-full text-left text-sm">
                    <thead>
                        <tr class="border-b border-neutral-200 text-neutral-500">
                            <th class="py-2 font-medium">"Reference"</th>
                            <th class="py-2 font-medium">"Placed"</th>
                            <th class="py-2 font-medium">"Customer"</th>
                            <th class="py-2 font-medium">"Items"</th>
                            <th class="py-2 font-medium">"Total"</th>
                            <th class="py-2 font-medium">"Status"</th>
                        </tr>
                    </thead>
                    <tbody class="divide-y divide-neutral-100">
                        if rows.is_empty() {
                            <tr><td class="py-2 text-neutral-500" colspan="6">"No order yet."</td></tr>
                        }
                        for row in &rows {
                            <tr data-order=(&row.reference) data-order-status=(&row.status)>
                                <td class="py-2">
                                    <a class="font-mono underline" href=(href!(order_id::index, Slug(slug.clone()), order_id::OrderId(row.id.clone())))>(&row.reference)</a>
                                </td>
                                <td class="py-2"><time datetime=(row.placed_at)>(format_time(row.placed_at as u64))</time></td>
                                <td class="py-2">(&row.name) <span class="text-neutral-500">" · " (&row.email)</span></td>
                                <td class="py-2">(row.item_count)</td>
                                <td class="py-2" data-order-total="">(row.total_label())</td>
                                <td class="py-2">(row.status().label())</td>
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
