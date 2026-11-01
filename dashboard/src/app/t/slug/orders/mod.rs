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
    view::{View, attributes, class, view},
};

use super::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::cluster::Executor;
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::input::input;
use crate::components::select::select;
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::db::Db;
use crate::ui::{
    LINK, Shell, Tone, document, empty_row, filter_bar, format_time, notice, page_header, pager,
    section_card, status_badge,
};

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

/// The orders gateway in the tenant's streams (403 while suspended).
pub fn orders(access: &TenantAccess) -> Result<Command<Executor>> {
    Ok(Command::new(access.executor()?.clone(), access.tenant_id()))
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
    let pool = app_context::<Db>(cx);
    let filter = OrderFilter::default().status(status).search(q);
    let total = count_orders(pool, &tenant.id, &filter)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let rows = orders_of_tenant(
        pool,
        &tenant.id,
        &filter,
        (page_number - 1) * PAGE_SIZE,
        PAGE_SIZE,
    )
    .await
    .map_err(topcoat::Error::from_anyhow)?;
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
            shell: Shell::tenant(&tenant, &nav, "orders"),
            page_header(title: "Orders", subtitle: Some("What shoppers placed on the storefronts."))
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                filter_bar(
                    <label class="sr-only" for="orders-search">"Search"</label>
                    input(attrs: attributes! {
                        id="orders-search" name="q" type="search" value=(q.unwrap_or(""))
                        class="sm:max-w-xs" placeholder="Reference, e-mail or name"
                    })
                    <label class="sr-only" for="orders-status">"Status"</label>
                    select(
                        attrs: attributes! { id="orders-status" name="status" class="w-44" },
                        <option value="">"Every status"</option>
                        for option in OrderStatus::ALL {
                            <option value=(option.as_str()) selected=(status == Some(option))>(option.label())</option>
                        }
                    )
                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Filter")
                )
                table(
                    table_header(
                        table_row(
                            table_head("Reference")
                            table_head("Placed")
                            table_head("Customer")
                            table_head(attrs: attributes! { class="text-right" }, "Items")
                            table_head(attrs: attributes! { class="text-right" }, "Total")
                            table_head("Status")
                        )
                    )
                    table_body(
                        if rows.is_empty() {
                            empty_row(colspan: 6, text: "No order yet.")
                        }
                        for row in &rows {
                            table_row(
                                attrs: attributes! { data-order=(&row.reference) data-order-status=(&row.status) },
                                table_cell(
                                    <a class=(class!(LINK, "font-mono")) href=(href!(order_id::index, Slug(slug.clone()), order_id::OrderId(row.id.clone())))>(&row.reference)</a>
                                )
                                table_cell(<time datetime=(row.placed_at)>(format_time(row.placed_at as u64))</time>)
                                table_cell(
                                    <span class="block font-medium">(&row.name)</span>
                                    <span class="block text-xs text-muted-foreground">(&row.email)</span>
                                )
                                table_cell(attrs: attributes! { class="text-right tabular-nums" }, (row.item_count))
                                table_cell(attrs: attributes! { class="text-right tabular-nums" data-order-total="" }, (row.total_label()))
                                table_cell(status_badge(tone: order_tone(row.status()), (row.status().label())))
                            )
                        }
                    )
                )
                pager(
                    page: page_number,
                    pages: pages,
                    previous: (page_number > 1).then(|| page_link(page_number - 1)),
                    next: (page_number < pages).then(|| page_link(page_number + 1)),
                )
            )
        )
    })
}

/// The colour of an order's status.
pub(crate) fn order_tone(status: OrderStatus) -> Tone {
    match status {
        OrderStatus::Paid => Tone::Info,
        OrderStatus::Fulfilled => Tone::Success,
        OrderStatus::Cancelled => Tone::Danger,
    }
}
