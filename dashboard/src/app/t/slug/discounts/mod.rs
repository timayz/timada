//! `/t/{slug}/discounts`: the discount codes of a tenant, for members with
//! `discounts:read`; those with `discounts:manage` create them. The list comes
//! from the shared read model (a moment behind), a discount's own page reads
//! its stream.

pub mod create;
pub mod discount_id;

use timada_catalog::CURRENCIES;
use timada_discounts::{
    Command, DiscountError, DiscountRow, MAX_CODE_LEN, MAX_USES, discounts_of_tenant,
};
use timada_tenancy::{PERMISSION_DISCOUNTS_MANAGE, PERMISSION_DISCOUNTS_READ};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params},
    view::{View, view},
};

use super::{Slug, TenantAccess, TenantNav, require_tenant, tenant_nav};
use crate::app::auth::Flash;
use crate::cluster::Executor;
use crate::db::ReadPool;
use crate::ui::{document, format_date, notice};

#[topcoat::router::query_params(error = bad_request)]
struct DiscountsQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn discounts_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, Slug(slug.to_owned()))
        .query(Flash { error, message })
        .resolve(cx)
}

/// The discounts gateway over the tenant's own store (403 while suspended).
pub fn discounts(access: &TenantAccess) -> Result<Command<Executor>> {
    Ok(Command(access.executor()?.clone()))
}

/// The query-string code of a user-facing discount error.
pub fn error_code(err: &DiscountError) -> &'static str {
    match err {
        // These codes already mean something else on other pages (a sign-in
        // code, a price).
        DiscountError::InvalidCode => "invalid_discount_code",
        DiscountError::CodeTaken => "discount_code_taken",
        DiscountError::InvalidAmount => "invalid_discount_amount",
        other => other.code().unwrap_or("unexpected"),
    }
}

/// When a discount can be used, in days: the last day is the one before its
/// (excluded) end.
pub fn describe_window(starts_at: Option<u64>, ends_at: Option<u64>) -> String {
    let last_day = |end: u64| format_date(end.saturating_sub(1));
    match (starts_at, ends_at) {
        (None, None) => "Always".to_owned(),
        (Some(start), None) => format!("From {}", format_date(start)),
        (None, Some(end)) => format!("Until {}", last_day(end)),
        (Some(start), Some(end)) => format!("{} to {}", format_date(start), last_day(end)),
    }
}

/// What limits a discount besides its window.
pub fn describe_limits(max_uses: Option<u32>, once_per_email: bool) -> String {
    match (max_uses, once_per_email) {
        (None, false) => "None".to_owned(),
        (Some(max), false) => format!("{max} orders"),
        (None, true) => "Once per e-mail".to_owned(),
        (Some(max), true) => format!("{max} orders, once per e-mail"),
    }
}

fn window_of(row: &DiscountRow) -> String {
    describe_window(
        row.starts_at.map(|at| at as u64),
        row.ends_at.map(|at| at as u64),
    )
}

fn limits_of(row: &DiscountRow) -> String {
    describe_limits(row.max_uses.map(|max| max as u32), row.once_per_email)
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_DISCOUNTS_READ)
        .await?;
    let can_manage = access.can(cx, PERMISSION_DISCOUNTS_MANAGE).await?;
    let query = query_params::<DiscountsQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let TenantAccess { tenant, .. } = access;
    let pool = app_context::<ReadPool>(cx);
    let rows = discounts_of_tenant(pool, &tenant.id, false)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let slug = tenant.slug.clone();
    let title = format!("Discounts · {}", tenant.name);

    Ok(view! {
        document(
            title: &title,
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">"Discounts"</h1>
                tenant_nav(tenant: &tenant, nav: &nav, current: "discounts")
            </header>
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <p class="mb-4 text-sm text-neutral-500">
                    "A shopper enters a code in the cart: it takes a percentage or an amount off the items of the whole order, one code per order. The invoice shows it as a discount, spread over the VAT rates of what was bought."
                </p>
                <table class="w-full text-left text-sm">
                    <thead>
                        <tr class="border-b border-neutral-200 text-neutral-500">
                            <th class="py-2 font-medium">"Code"</th>
                            <th class="py-2 font-medium">"Off"</th>
                            <th class="py-2 font-medium">"When"</th>
                            <th class="py-2 font-medium">"Limits"</th>
                            <th class="py-2 font-medium">"Status"</th>
                        </tr>
                    </thead>
                    <tbody class="divide-y divide-neutral-100">
                        if rows.is_empty() {
                            <tr><td class="py-2 text-neutral-500" colspan="5">"No discount yet."</td></tr>
                        }
                        for row in &rows {
                            <tr data-discount=(&row.code)>
                                <td class="py-2">
                                    <a class="underline" href=(href!(discount_id::index, Slug(slug.clone()), discount_id::DiscountId(row.id.clone())))><code>(&row.code)</code></a>
                                </td>
                                <td class="py-2" data-discount-value="">(row.value().to_string())</td>
                                <td class="py-2">(window_of(row))</td>
                                <td class="py-2">(limits_of(row))</td>
                                <td class="py-2" data-discount-status=(&row.status)>(&row.status)</td>
                            </tr>
                        }
                    </tbody>
                </table>
            </section>

            if can_manage {
                <section class="rounded border border-neutral-200 bg-white p-4">
                    <h2 class="mb-2 text-lg font-medium">"Create a discount"</h2>
                    <form method="post" action=(href!(create::create, Slug(slug.clone()))) class="space-y-3">
                        <label class="block text-sm" for="new-discount-code">"Code"</label>
                        <input id="new-discount-code" name="code" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2 uppercase"
                            type="text" maxlength=(MAX_CODE_LEN) required="" placeholder="SUMMER10">
                        <fieldset class="space-y-2">
                            <legend class="text-sm">"Takes off"</legend>
                            <label class="mr-4 text-sm"><input type="radio" name="kind" value="percent" checked=""> " a percentage"</label>
                            <label class="text-sm"><input type="radio" name="kind" value="amount"> " an amount"</label>
                        </fieldset>
                        <label class="block text-sm" for="new-discount-value">"Percentage or amount"</label>
                        <div class="flex gap-2">
                            <input id="new-discount-value" name="value" class="w-full rounded border border-neutral-300 px-3 py-2"
                                type="text" inputmode="decimal" required="" placeholder="10">
                            <select name="currency" aria-label="Currency of an amount" class="rounded border border-neutral-300 px-3 py-2">
                                for currency in CURRENCIES {
                                    <option value=(*currency)>(*currency)</option>
                                }
                            </select>
                        </div>
                        <div class="flex gap-4">
                            <div class="flex-1">
                                <label class="block text-sm" for="new-discount-starts">"First day (optional)"</label>
                                <input id="new-discount-starts" name="starts_on" type="date" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2">
                            </div>
                            <div class="flex-1">
                                <label class="block text-sm" for="new-discount-ends">"Last day (optional)"</label>
                                <input id="new-discount-ends" name="ends_on" type="date" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2">
                            </div>
                        </div>
                        <p class="text-sm text-neutral-500">"Days are counted in UTC."</p>
                        <label class="block text-sm" for="new-discount-max">"Orders it serves at most (optional)"</label>
                        <input id="new-discount-max" name="max_uses" type="number" min="1" max=(MAX_USES) class="mt-1 w-full rounded border border-neutral-300 px-3 py-2">
                        <label class="block text-sm">
                            <input type="checkbox" name="once_per_email" value="on"> " Once per shopper e-mail"
                        </label>
                        <button class="rounded bg-neutral-900 px-4 py-2 font-medium text-white hover:bg-neutral-700">"Create discount"</button>
                    </form>
                </section>
            }
        )
    })
}
