//! `/t/{slug}/carts`: the carts shoppers are filling and have not ordered,
//! the most recently touched first, for members with `orders:read`. One
//! that stays still for days is a sale left behind.

use timada_orders::{count_open_carts, open_carts};
use timada_tenancy::PERMISSION_ORDERS_READ;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use super::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::db::Db;
use crate::ui::{
    Shell, Tone, document, empty_row, format_time, page_header, pager, section_card, status_badge,
};

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
    let pool = app_context::<Db>(cx);
    let total = count_open_carts(pool, &tenant.id)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let rows = open_carts(pool, &tenant.id, (page_number - 1) * PAGE_SIZE, PAGE_SIZE)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
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
            shell: Shell::tenant(&tenant, &nav, "carts"),
            page_header(title: "Open carts", subtitle: Some("What shoppers are filling and have not ordered yet."))

            section_card(
                description: Some("Carts holding something that were not ordered yet. A cart becomes an order at checkout and leaves this list."),
                table(
                    table_header(
                        table_row(
                            table_head("Cart")
                            table_head(attrs: attributes! { class="text-right" }, "Products")
                            table_head(attrs: attributes! { class="text-right" }, "Items")
                            table_head("State")
                            table_head("Started")
                            table_head("Last change")
                        )
                    )
                    table_body(
                        if rows.is_empty() {
                            empty_row(colspan: 6, text: "No open cart.")
                        }
                        for row in &rows {
                            table_row(
                                attrs: attributes! { data-cart=(&row.id) },
                                table_cell(<code class="text-xs">(&row.id)</code>)
                                table_cell(attrs: attributes! { class="text-right tabular-nums" }, (row.line_count))
                                table_cell(attrs: attributes! { class="text-right tabular-nums" data-cart-items="" }, (row.item_count))
                                table_cell(
                                    status_badge(
                                        tone: if row.status == "paying" { Tone::Info } else { Tone::Neutral },
                                        attrs: attributes! { data-cart-status=(&row.status) },
                                        (if row.status == "paying" { "Paying" } else { "Filling" })
                                    )
                                )
                                table_cell(<time datetime=(row.created_at)>(format_time(row.created_at as u64))</time>)
                                table_cell(<time datetime=(row.updated_at)>(format_time(row.updated_at as u64))</time>)
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
