//! `/t/{slug}/costs`: what a sale costs the shop beyond the goods, and how
//! its margins are graded, for members with `products:read`; those with
//! `products:manage` change them. Read from the tenant's own stream, so a
//! change shows at once. What each product costs is said on its own page,
//! where its margin is worked out from both.

pub mod set;
pub mod tiers;

use timada_catalog::{CURRENCIES, FEED_LABELS, parse_amount};
use timada_tenancy::{PERMISSION_PRODUCTS_MANAGE, PERMISSION_PRODUCTS_READ};
use topcoat::{
    Result,
    context::Cx,
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use super::products::catalog;
use super::{TenantAccess, TenantNav, require_tenant, slug};
use crate::app::auth::Flash;
use crate::components::button::button;
use crate::components::field::{field, field_description, field_label};
use crate::components::input::input;
use crate::components::select::select;
use crate::ui::{LINK, Shell, document, notice, page_header, section_card};

#[topcoat::router::query_params(error = bad_request)]
struct CostsQuery {
    error: Option<String>,
    message: Option<String>,
}

/// The page, with a flash error code or message.
pub fn costs_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, slug: slug.to_owned())
        .query(Flash { error, message })
        .resolve(cx)
}

/// A share typed as a percentage (`1.5`), in basis points; `None` for
/// what is not a number with at most two decimals.
pub fn parse_percent(text: &str) -> Option<u32> {
    parse_amount(text)
        .ok()
        .and_then(|bps| u32::try_from(bps).ok())
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PRODUCTS_READ)
        .await?;
    let can_manage = access.can(cx, PERMISSION_PRODUCTS_MANAGE).await?;
    let query = query_params::<CostsQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let state = catalog(&access)?
        .load_sale_costs()
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let tiers = state.tiers();
    let costs = state.costs.unwrap_or_default();
    let amount = timada_core::format_amount;
    let currency = if costs.currency.is_empty() {
        CURRENCIES[0].to_owned()
    } else {
        costs.currency.clone()
    };
    let shipping = amount(costs.shipping_cents);
    let import = amount(costs.import_cents);
    let fixed = amount(costs.fixed_cents);
    let fee_percent = amount(u64::from(costs.payment_fee_bps));
    let fee_fixed = amount(costs.payment_fee_cents);
    let medium_from = amount(u64::from(tiers.medium_from_bps));
    let high_from = amount(u64::from(tiers.high_from_bps));
    let feed_label = tiers.feed_label.map(usize::from);
    let TenantAccess { tenant, .. } = access;
    let slug = tenant.slug.clone();
    let title = format!("Costs · {}", tenant.name);

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "costs"),
            page_header(
                title: "Costs",
                subtitle: Some("What a sale costs the shop beyond the goods. With what each product costs, said on its own page, it tells what a sale leaves: the margin of every product, and the profit of every order from now on."),
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                title: Some("What a sale costs"),
                description: Some("Counted for every sale in this currency. A product that is sent or imported another way says so on its own page."),
                <form method="post" action=(href!(set::set, slug: slug.clone())) data-sale-costs="">
                    <fieldset disabled=(!can_manage) class="flex max-w-xl flex-col gap-4">
                        field(
                            field_label(attrs: attributes! { for="costs-currency" }, "Currency")
                            select(
                                attrs: attributes! { id="costs-currency" name="currency" class="w-32" },
                                for code in CURRENCIES {
                                    <option value=(*code) selected=(*code == currency)>(*code)</option>
                                }
                            )
                        )
                        field(
                            field_label(attrs: attributes! { for="costs-shipping" }, "Sending a parcel")
                            field_description(attrs: attributes! { id="costs-shipping-hint" }, "What the carrier or the supplier charges the shop, whatever the shopper pays for delivery.")
                            input(attrs: attributes! {
                                id="costs-shipping" name="shipping" type="text" inputmode="decimal" class="w-40"
                                aria-describedby="costs-shipping-hint" value=(&shipping)
                            })
                        )
                        field(
                            field_label(attrs: attributes! { for="costs-import" }, "At the border, for each kind of item")
                            field_description(attrs: attributes! { id="costs-import-hint" }, "Goods shipped from outside the EU to a private buyer pay a customs duty of 3.00 € for each kind of item in a parcel worth up to 150 € (since July 2026) and a processing fee of 2.00 € (since November 2026): 5.00 € where the supplier passes both on. Leave 0 for goods that are in the EU already.")
                            input(attrs: attributes! {
                                id="costs-import" name="import" type="text" inputmode="decimal" class="w-40"
                                aria-describedby="costs-import-hint" value=(&import)
                            })
                        )
                        field(
                            field_label(attrs: attributes! { for="costs-fixed" }, "Fixed costs, for each sale")
                            field_description(attrs: attributes! { id="costs-fixed-hint" }, "The share of what the shop pays whatever it sells (subscriptions, accounting, packaging): a month of them over a month's orders.")
                            input(attrs: attributes! {
                                id="costs-fixed" name="fixed" type="text" inputmode="decimal" class="w-40"
                                aria-describedby="costs-fixed-hint" value=(&fixed)
                            })
                        )
                        field(
                            field_label(attrs: attributes! { for="costs-fee-percent" }, "Payment fee, in % of the amount paid")
                            input(attrs: attributes! {
                                id="costs-fee-percent" name="fee_percent" type="text" inputmode="decimal" class="w-40"
                                value=(&fee_percent)
                            })
                        )
                        field(
                            field_label(attrs: attributes! { for="costs-fee-fixed" }, "Payment fee, for each payment")
                            input(attrs: attributes! {
                                id="costs-fee-fixed" name="fee_fixed" type="text" inputmode="decimal" class="w-40"
                                value=(&fee_fixed)
                            })
                        )
                        if can_manage {
                            <div>button(attrs: attributes! { type="submit" }, "Save costs")</div>
                        }
                    </fieldset>
                </form>
            )

            section_card(
                title: Some("Margin tiers"),
                description: Some("A product's margin is graded by the share of its price a sale leaves: loss when it leaves nothing, then low, medium and high. One of the shopping feed's labels can say the tier, so ad campaigns bid by it."),
                <form method="post" action=(href!(tiers::tiers, slug: slug.clone())) data-margin-tiers="">
                    <fieldset disabled=(!can_manage) class="flex max-w-xl flex-col gap-4">
                        field(
                            field_label(attrs: attributes! { for="tiers-medium" }, "Medium from, in % of the price")
                            input(attrs: attributes! {
                                id="tiers-medium" name="medium_from" type="text" inputmode="decimal" class="w-40"
                                value=(&medium_from)
                            })
                        )
                        field(
                            field_label(attrs: attributes! { for="tiers-high" }, "High from, in % of the price")
                            input(attrs: attributes! {
                                id="tiers-high" name="high_from" type="text" inputmode="decimal" class="w-40"
                                value=(&high_from)
                            })
                        )
                        field(
                            field_label(attrs: attributes! { for="tiers-label" }, "Feed label that says the tier")
                            field_description(attrs: attributes! { id="tiers-label-hint" },
                                "The feed then writes loss, low, medium or high there for every item whose cost is known, in place of what the product's own "
                                <a class=(LINK) href=(href!(super::feed::index, slug: slug.clone()))>"Shopping feed"</a>
                                " tab says for that label."
                            )
                            select(
                                attrs: attributes! { id="tiers-label" name="feed_label" class="w-40" aria-describedby="tiers-label-hint" },
                                <option value="" selected=(feed_label.is_none())>"None"</option>
                                for n in 0..FEED_LABELS {
                                    <option value=(n) selected=(feed_label == Some(n))>"Label " (n)</option>
                                }
                            )
                        )
                        if can_manage {
                            <div>button(attrs: attributes! { type="submit" }, "Save tiers")</div>
                        }
                    </fieldset>
                </form>
            )
        )
    })
}
