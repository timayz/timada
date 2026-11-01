//! `/t/{slug}/customers`: the shoppers who signed in to the shop, for members
//! with `customers:read`. The list comes from the shared read model (a moment
//! behind); a customer's order count is every order placed with their
//! address, guest ones included. Members with `customers:manage` delete a
//! shopper's account.

pub mod delete;

use timada_shoppers::{ShopperError, count_shoppers, shoppers_of_tenant};
use timada_tenancy::{PERMISSION_CUSTOMERS_MANAGE, PERMISSION_CUSTOMERS_READ};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use super::orders::orders_of_email_url;
use super::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::app::auth::Flash;
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::db::Db;
use crate::ui::{
    LINK, Shell, document, empty_row, format_time, notice, page_header, pager, section_card,
};

const PAGE_SIZE: u32 = 50;

#[topcoat::router::query_params(error = bad_request)]
struct CustomersQuery {
    /// 1-based.
    page: Option<u32>,
    error: Option<String>,
    message: Option<String>,
}

#[derive(serde::Serialize)]
struct CustomersLink {
    page: u32,
}

/// The customers page, with a flash error code or message.
pub fn customers_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, Slug(slug.to_owned()))
        .query(Flash { error, message })
        .resolve(cx)
}

/// The query-string code of a user-facing shopper error.
pub fn error_code(err: &ShopperError) -> &'static str {
    err.code().unwrap_or("unexpected")
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_CUSTOMERS_READ)
        .await?;
    let query = query_params::<CustomersQuery>(cx)?;
    let page_number = query.page.unwrap_or(1).max(1);
    let can_manage = access.can(cx, PERMISSION_CUSTOMERS_MANAGE).await?;
    let nav = TenantNav::of(cx, &access).await?;
    let TenantAccess { tenant, .. } = access;
    let pool = app_context::<Db>(cx);
    let total = count_shoppers(pool, &tenant.id)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let rows = shoppers_of_tenant(pool, &tenant.id, PAGE_SIZE, (page_number - 1) * PAGE_SIZE)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let pages = total.div_ceil(u64::from(PAGE_SIZE)) as u32;
    let slug = tenant.slug.clone();
    let page_link = {
        let slug = slug.clone();
        move |page: u32| {
            href!(index, Slug(slug.clone()))
                .query(CustomersLink { page })
                .resolve(cx)
        }
    };
    let title = format!("Customers · {}", tenant.name);

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "customers"),
            page_header(title: "Customers", subtitle: Some("The shoppers who signed in to the shop."))
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                <p class="mb-4 text-sm text-muted-foreground">
                    "A shopper gets an account the first time they sign in to the shop with a mailed code. Their orders are every order placed with their address, those placed before they signed in included."
                </p>
                if can_manage {
                    <p class="mb-4 text-sm text-muted-foreground">
                        "Deleting a customer forgets their saved addresses and signs them out everywhere; the orders they placed stay with the shop."
                    </p>
                }
                <p class="mb-4 text-sm font-medium" data-customer-count=(total)>(total) " customer(s)"</p>
                table(
                    table_header(
                        table_row(
                            table_head("E-mail")
                            table_head("Customer since")
                            table_head(attrs: attributes! { class="text-right" }, "Orders")
                            table_head(attrs: attributes! { class="text-right" }, "Saved addresses")
                            if can_manage {
                                table_head(attrs: attributes! { class="text-right" }, "Actions")
                            }
                        )
                    )
                    table_body(
                        if rows.is_empty() {
                            empty_row(colspan: if can_manage { 5 } else { 4 }, text: "No customer here.")
                        }
                        for row in &rows {
                            table_row(
                                attrs: attributes! { data-customer=(&row.email) },
                                table_cell(attrs: attributes! { class="font-medium" }, (&row.email))
                                table_cell(<time datetime=(row.registered_at)>(format_time(row.registered_at as u64))</time>)
                                table_cell(
                                    attrs: attributes! { class="text-right tabular-nums" data-customer-orders=(row.order_count) },
                                    if nav.orders && row.order_count > 0 {
                                        <a class=(LINK) href=(orders_of_email_url(cx, &slug, &row.email))>(row.order_count)</a>
                                    } else {
                                        (row.order_count)
                                    }
                                )
                                table_cell(attrs: attributes! { class="text-right tabular-nums" data-customer-addresses=(row.address_count) }, (row.address_count))
                                if can_manage {
                                    table_cell(
                                        attrs: attributes! { class="text-right" },
                                        <form method="post" action=(href!(delete::delete, Slug(slug.clone())))>
                                            <input type="hidden" name="shopper_id" value=(&row.id)>
                                            button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Delete")
                                        </form>
                                    )
                                }
                            )
                        }
                    )
                )
                pager(
                    page: i64::from(page_number),
                    pages: i64::from(pages),
                    previous: (page_number > 1).then(|| page_link(page_number - 1)),
                    next: (page_number < pages).then(|| page_link(page_number + 1)),
                )
            )
        )
    })
}
