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
    view::{View, attributes, class, view},
};

use super::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::app::auth::Flash;
use crate::cluster::Executor;
use crate::components::button::button;
use crate::components::checkbox::checkbox;
use crate::components::field::{field, field_label};
use crate::components::input::input;
use crate::components::select::select;
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::db::{Db, ReadPool};
use crate::ui::{
    LINK, Shell, Tone, document, empty_row, format_date, notice, page_header, section_card,
    status_badge,
};

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

/// The discounts gateway in the tenant's streams (403 while suspended).
pub fn discounts(access: &TenantAccess) -> Result<Command<Executor>> {
    Ok(Command::new(access.executor()?.clone(), access.tenant_id()))
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
    let rows = discounts_of_tenant(app_context::<Db>(cx), &tenant.id, false)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    // How many paid orders used each code, from the orders' read model.
    let uses = timada_orders::discount_use_counts(pool, &tenant.id).await?;
    let slug = tenant.slug.clone();
    let title = format!("Discounts · {}", tenant.name);

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "discounts"),
            page_header(title: "Discounts", subtitle: Some("Codes shoppers enter in the cart for something off the whole order."))
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                description: Some("A shopper enters a code in the cart: it takes a percentage or an amount off the items of the whole order, one code per order. The invoice shows it as a discount, spread over the VAT rates of what was bought."),
                table(
                    table_header(
                        table_row(
                            table_head("Code")
                            table_head("Off")
                            table_head("When")
                            table_head("Limits")
                            table_head(attrs: attributes! { class="text-right" }, "Orders")
                            table_head("Status")
                        )
                    )
                    table_body(
                        if rows.is_empty() {
                            empty_row(colspan: 6, text: "No discount yet.")
                        }
                        for row in &rows {
                            table_row(
                                attrs: attributes! { data-discount=(&row.code) },
                                table_cell(
                                    <a class=(class!(LINK, "font-mono")) href=(href!(discount_id::index, Slug(slug.clone()), discount_id::DiscountId(row.id.clone())))><code>(&row.code)</code></a>
                                )
                                table_cell(attrs: attributes! { data-discount-value="" }, (row.value().to_string()))
                                table_cell((window_of(row)))
                                table_cell((limits_of(row)))
                                table_cell(attrs: attributes! { class="text-right tabular-nums" data-discount-orders="" }, (uses.get(&row.id).copied().unwrap_or(0)))
                                table_cell(
                                    attrs: attributes! { data-discount-status=(&row.status) },
                                    status_badge(tone: discount_tone(&row.status), (&row.status))
                                )
                            )
                        }
                    )
                )
            )

            if can_manage {
                section_card(
                    title: Some("Create a discount"),
                    <form method="post" action=(href!(create::create, Slug(slug.clone()))) class="flex max-w-xl flex-col gap-4">
                        field(
                            field_label(attrs: attributes! { for="new-discount-code" }, "Code")
                            input(attrs: attributes! {
                                id="new-discount-code" name="code" class="uppercase"
                                type="text" maxlength=(MAX_CODE_LEN) required="" placeholder="SUMMER10"
                            })
                        )
                        <fieldset class="flex flex-col gap-2">
                            <legend class="text-sm font-medium">"Takes off"</legend>
                            <div class="flex flex-wrap gap-4">
                                <label class="flex items-center gap-2 text-sm"><input type="radio" name="kind" value="percent" checked="" class="accent-primary"> " a percentage"</label>
                                <label class="flex items-center gap-2 text-sm"><input type="radio" name="kind" value="amount" class="accent-primary"> " an amount"</label>
                            </div>
                        </fieldset>
                        field(
                            field_label(attrs: attributes! { for="new-discount-value" }, "Percentage or amount")
                            <div class="flex gap-2">
                                input(attrs: attributes! {
                                    id="new-discount-value" name="value"
                                    type="text" inputmode="decimal" required="" placeholder="10"
                                })
                                select(
                                    attrs: attributes! { name="currency" aria-label="Currency of an amount" class="w-28 shrink-0" },
                                    for currency in CURRENCIES {
                                        <option value=(*currency)>(*currency)</option>
                                    }
                                )
                            </div>
                        )
                        <div class="grid gap-4 sm:grid-cols-2">
                            field(
                                field_label(attrs: attributes! { for="new-discount-starts" }, "First day (optional)")
                                input(attrs: attributes! { id="new-discount-starts" name="starts_on" type="date" })
                            )
                            field(
                                field_label(attrs: attributes! { for="new-discount-ends" }, "Last day (optional)")
                                input(attrs: attributes! { id="new-discount-ends" name="ends_on" type="date" })
                            )
                        </div>
                        <p class="text-sm text-muted-foreground">"Days are counted in UTC."</p>
                        field(
                            field_label(attrs: attributes! { for="new-discount-max" }, "Orders it serves at most (optional)")
                            input(attrs: attributes! { id="new-discount-max" name="max_uses" type="number" min="1" max=(MAX_USES) })
                        )
                        <label class="flex items-center gap-2 text-sm">
                            checkbox(attrs: attributes! { name="once_per_email" value="on" })
                            " Once per shopper e-mail"
                        </label>
                        <div>
                            button(attrs: attributes! { type="submit" }, "Create discount")
                        </div>
                    </form>
                )
            }
        )
    })
}

/// The colour of a discount's stored status.
fn discount_tone(status: &str) -> Tone {
    match status {
        "active" => Tone::Success,
        "inactive" => Tone::Warning,
        _ => Tone::Neutral,
    }
}
