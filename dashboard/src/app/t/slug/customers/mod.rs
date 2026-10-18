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
    view::{View, view},
};

use super::orders::orders_of_email_url;
use super::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::app::auth::Flash;
use crate::db::ReadPool;
use crate::ui::{Shell, document, format_time, notice};

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
    let pool = app_context::<ReadPool>(cx);
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
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">"Customers"</h1>
            </header>
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <p class="mb-4 text-sm text-neutral-500">
                    "A shopper gets an account the first time they sign in to the shop with a mailed code. Their orders are every order placed with their address, those placed before they signed in included."
                </p>
                if can_manage {
                    <p class="mb-4 text-sm text-neutral-500">
                        "Deleting a customer forgets their saved addresses and signs them out everywhere; the orders they placed stay with the shop."
                    </p>
                }
                <p class="mb-4 text-sm" data-customer-count=(total)>(total) " customer(s)"</p>
                <table class="w-full text-left text-sm">
                    <thead>
                        <tr class="border-b border-neutral-200 text-neutral-500">
                            <th class="py-2 font-medium">"E-mail"</th>
                            <th class="py-2 font-medium">"Customer since"</th>
                            <th class="py-2 font-medium">"Orders"</th>
                            <th class="py-2 font-medium">"Saved addresses"</th>
                            if can_manage {
                                <th class="py-2 text-right font-medium">"Actions"</th>
                            }
                        </tr>
                    </thead>
                    <tbody class="divide-y divide-neutral-100">
                        if rows.is_empty() {
                            <tr><td class="py-2 text-neutral-500" colspan=(if can_manage { 5 } else { 4 })>"No customer here."</td></tr>
                        }
                        for row in &rows {
                            <tr data-customer=(&row.email)>
                                <td class="py-2">(&row.email)</td>
                                <td class="py-2"><time datetime=(row.registered_at)>(format_time(row.registered_at as u64))</time></td>
                                <td class="py-2" data-customer-orders=(row.order_count)>
                                    if nav.orders && row.order_count > 0 {
                                        <a class="underline" href=(orders_of_email_url(cx, &slug, &row.email))>(row.order_count)</a>
                                    } else {
                                        (row.order_count)
                                    }
                                </td>
                                <td class="py-2" data-customer-addresses=(row.address_count)>(row.address_count)</td>
                                if can_manage {
                                    <td class="py-2 text-right">
                                        <form method="post" action=(href!(delete::delete, Slug(slug.clone())))>
                                            <input type="hidden" name="shopper_id" value=(&row.id)>
                                            <button class="rounded border border-red-700 px-2 py-1 text-xs text-red-700 hover:bg-red-50">"Delete"</button>
                                        </form>
                                    </td>
                                }
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
