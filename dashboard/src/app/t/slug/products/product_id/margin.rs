//! The "Cost & margin" tab of a product's page: what the shop pays for
//! each thing the product sells, what sending and importing it cost when
//! they are not the shop's usual, and what a sale of each leaves.

use timada_catalog::{
    Margin, MarginTiers, ProductCosts, ProductState, ShopCosts, format_money, tax_category_label,
};
use timada_invoicing::format_rate_fr;
use topcoat::{
    Result,
    context::Cx,
    router::href,
    view::{View, attributes, component, view},
};

use super::{Tab, cost, own_costs, product_id};
use crate::app::t::slug::slug;
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::field::{field, field_description, field_label};
use crate::components::input::input;
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::ui::{LINK, fact, facts, section_card};

/// The form field of one combination's cost: `cost:<variant id>`.
pub const COST_FIELD_PREFIX: &str = "cost:";

/// One thing the product sells: the product itself, or a combination.
pub struct MarginRow {
    /// The combination's id; empty for a product sold as is.
    pub id: String,
    pub label: String,
    /// Whether shoppers can buy it: a hidden option may leave it off sale.
    pub on_sale: bool,
    /// What the shop pays for one, as the form wants it typed; empty when
    /// it has not said.
    pub cost: String,
    /// What a sale of it leaves at the price asked about; `None` without a
    /// cost.
    pub margin: Option<Margin>,
    pub tier: &'static str,
}

/// Everything the tab shows.
pub struct MarginView {
    pub currency: String,
    pub has_variants: bool,
    pub rows: Vec<MarginRow>,
    /// What sending and importing one cost for this product, as the forms
    /// want them typed; empty for the shop's.
    pub own_shipping: String,
    pub own_import: String,
    /// The shop's usual, said under each field.
    pub shop_shipping: String,
    pub shop_import: String,
    /// What a sale carries whatever the thing: (what it is, how much).
    pub shared: Vec<(&'static str, String)>,
    /// The price asked about with "what if", as typed; empty for each
    /// thing's own.
    pub what_if: String,
    /// The shop's costs are in another currency than the product: no margin
    /// can be told.
    pub other_currency: Option<String>,
}

impl MarginView {
    /// The tab for `product`: each thing at its own price, or all of them
    /// at `what_if` when the merchant asks about another one. `costs` is
    /// what the shop pays for them, `vat_bps` the VAT in its price where the
    /// shop sells.
    pub fn of(
        product: &ProductState,
        costs: &ProductCosts,
        shop: Option<&ShopCosts>,
        tiers: MarginTiers,
        vat_bps: u32,
        what_if: Option<(u64, String)>,
    ) -> Self {
        let currency = product
            .currency()
            .unwrap_or(timada_catalog::CURRENCIES[0])
            .to_owned();
        let amount = |cents: Option<u64>| cents.map(timada_core::format_amount).unwrap_or_default();
        let row = |id: Option<&str>, label: String, price: Option<u64>, on_sale: bool| {
            let unit = costs.unit_costs(id);
            let price = what_if.as_ref().map(|(cents, _)| *cents).or(price);
            let margin = price.and_then(|price| Margin::of(price, &currency, vat_bps, unit, shop));
            MarginRow {
                id: id.unwrap_or_default().to_owned(),
                label,
                on_sale,
                cost: amount(unit.cost_cents),
                tier: margin
                    .as_ref()
                    .map_or("", |margin| tiers.tier(margin).as_str()),
                margin,
            }
        };
        let rows = if product.variants.is_empty() {
            vec![row(
                None,
                product.name.clone(),
                product.amount_of(None),
                true,
            )]
        } else {
            product
                .variants
                .iter()
                .map(|variant| {
                    let label = variant
                        .selections
                        .iter()
                        .map(|selection| selection.value.as_str())
                        .collect::<Vec<_>>()
                        .join(" / ");
                    row(
                        Some(&variant.id),
                        label,
                        Some(variant.price.amount_cents),
                        product.sells(variant),
                    )
                })
                .collect()
        };
        let money = |cents: u64| format_money(cents, &currency);
        let usable = shop.filter(|shop| shop.currency == currency);
        let shop_shipping = money(usable.map_or(0, |shop| shop.shipping_cents));
        let shop_import = money(usable.map_or(0, |shop| shop.import_cents));
        let vat = if vat_bps == 0 {
            "None".to_owned()
        } else {
            format!(
                "{} ({})",
                format_rate_fr(vat_bps),
                tax_category_label(product.tax_category())
            )
        };
        let mut shared = vec![
            ("VAT in the price", vat),
            (
                "Sending one",
                costs
                    .own_shipping_cents
                    .map_or_else(|| format!("{shop_shipping} (the shop's)"), money),
            ),
            (
                "At the border",
                costs
                    .own_import_cents
                    .map_or_else(|| format!("{shop_import} (the shop's)"), money),
            ),
        ];
        if let Some(shop) = usable {
            shared.push(("Fixed costs", money(shop.fixed_cents)));
            shared.push((
                "Payment fee",
                format!(
                    "{} % of the price and {}",
                    timada_core::format_amount(u64::from(shop.payment_fee_bps)),
                    money(shop.payment_fee_cents)
                ),
            ));
        }
        Self {
            has_variants: !product.variants.is_empty(),
            rows,
            own_shipping: amount(costs.own_shipping_cents),
            own_import: amount(costs.own_import_cents),
            shop_shipping,
            shop_import,
            shared,
            what_if: what_if.map(|(_, typed)| typed).unwrap_or_default(),
            other_currency: shop
                .filter(|shop| shop.currency != currency)
                .map(|shop| shop.currency.clone()),
            currency,
        }
    }
}

/// An amount that may be a loss, `-3.20 €`.
fn signed_money(cents: i64, currency: &str) -> String {
    let money = format_money(cents.unsigned_abs(), currency);
    if cents < 0 {
        format!("-{money}")
    } else {
        money
    }
}

/// A share in basis points as a percentage with one decimal, `39.7 %`.
fn share_label(bps: i64) -> String {
    let tenths = bps / 10;
    let sign = if tenths < 0 { "-" } else { "" };
    format!("{sign}{}.{} %", tenths.abs() / 10, tenths.abs() % 10)
}

/// A return on ad spend in hundredths, `2.52`.
fn roas_label(hundredths: u64) -> String {
    format!("{}.{:02}", hundredths / 100, hundredths % 100)
}

#[component]
pub async fn margin_tab(cx: &Cx, editable: bool, margin: &MarginView) -> Result<impl View> {
    let slug = slug(cx).to_owned();
    let product_id = product_id(cx).to_owned();
    let currency = margin.currency.as_str();
    let costs_page = href!(crate::app::t::slug::costs::index, slug: slug.clone()).resolve(cx);
    Ok(view! {
        section_card(
            title: Some("What it costs"),
            description: Some("What the shop pays its supplier for one, in the product's currency and as the shop bears it (without VAT for a shop that gets it back). No shopper ever sees it: the margin below and the profit of each order are worked out from it."),
            <form method="post" action=(href!(cost::cost, slug: slug.clone(), product_id: product_id.clone())) data-costs="">
                <fieldset disabled=(!editable) class="flex flex-col gap-4">
                    if margin.has_variants {
                        field(
                            attrs: attributes! { class="max-w-xl" },
                            field_label(attrs: attributes! { for="cost-every" }, "One cost for every combination")
                            field_description(attrs: attributes! { id="cost-every-hint" }, "Fill it in to give them all the same cost; leave it empty to set each below.")
                            input(attrs: attributes! {
                                id="cost-every" name="every" type="text" inputmode="decimal" class="w-40"
                                aria-describedby="cost-every-hint"
                            })
                        )
                        table(
                            attrs: attributes! { data-variant-costs="" },
                            table_header(
                                table_row(
                                    table_head("Combination")
                                    table_head("Cost of one (" (currency) ")")
                                )
                            )
                            table_body(
                                for row in &margin.rows {
                                    table_row(
                                        attrs: attributes! { data-variant=(&row.id) },
                                        table_cell(
                                            (&row.label)
                                            if !row.on_sale {
                                                <span class="text-muted-foreground">" (off sale)"</span>
                                            }
                                        )
                                        table_cell(
                                            <label class="sr-only" for=(format!("cost-{}", row.id))>"Cost of " (&row.label)</label>
                                            input(attrs: attributes! {
                                                id=(format!("cost-{}", row.id)) name=(format!("{COST_FIELD_PREFIX}{}", row.id))
                                                type="text" inputmode="decimal" class="h-8 w-32 px-2" value=(&row.cost)
                                            })
                                        )
                                    )
                                }
                            )
                        )
                    } else {
                        for row in &margin.rows {
                            field(
                                attrs: attributes! { class="max-w-xl" },
                                field_label(attrs: attributes! { for="cost-own" }, "Cost of one (" (currency) ")")
                                field_description(attrs: attributes! { id="cost-own-hint" }, "Leave it empty while it is not known.")
                                input(attrs: attributes! {
                                    id="cost-own" name="cost" type="text" inputmode="decimal" class="w-40"
                                    aria-describedby="cost-own-hint" value=(&row.cost)
                                })
                            )
                        }
                    }
                    if editable {
                        <div>button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Record what it costs")</div>
                    }
                </fieldset>
            </form>
        )

        section_card(
            title: Some("Sending and importing it"),
            description: Some("Only where this product differs from the rest of the shop: a supplier who sends it apart, goods already in the EU that pay nothing at the border. Left empty, it counts what the shop's Costs page says."),
            <form method="post" action=(href!(own_costs::own_costs, slug: slug.clone(), product_id: product_id.clone())) data-own-costs="">
                <fieldset disabled=(!editable) class="flex max-w-xl flex-col gap-4">
                    field(
                        field_label(attrs: attributes! { for="own-shipping" }, "Sending one (" (currency) ")")
                        field_description(attrs: attributes! { id="own-shipping-hint" }, "The shop's: " (&margin.shop_shipping) ".")
                        input(attrs: attributes! {
                            id="own-shipping" name="shipping" type="text" inputmode="decimal" class="w-40"
                            aria-describedby="own-shipping-hint" value=(&margin.own_shipping)
                        })
                    )
                    field(
                        field_label(attrs: attributes! { for="own-import" }, "At the border (" (currency) ")")
                        field_description(attrs: attributes! { id="own-import-hint" }, "The shop's: " (&margin.shop_import) ". Enter 0 for goods that pay nothing there.")
                        input(attrs: attributes! {
                            id="own-import" name="import" type="text" inputmode="decimal" class="w-40"
                            aria-describedby="own-import-hint" value=(&margin.own_import)
                        })
                    )
                    if editable {
                        <div>button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Record these two")</div>
                    }
                </fieldset>
            </form>
        )

        section_card(
            title: Some("What a sale leaves"),
            description: Some("One sold on its own, at home: its price less the VAT in it, its cost, and what the sale costs the shop. The break-even return is what an ad must bring back for each unit of money spent on it before it loses money: the price over the margin."),
            attrs: attributes! { data-margins="" },
            if let Some(other) = &margin.other_currency {
                <p class="mb-4 text-sm text-destructive-text" data-margin-other-currency="">
                    "The shop's costs are in " (other) " and this product is priced in " (currency) ": no margin can be told. Change one of them on the "
                    <a class=(LINK) href=(&costs_page)>"Costs"</a> " page."
                </p>
            }
            facts(
                for (term, value) in &margin.shared {
                    fact(term: term, (value))
                }
            )
            <p class="mt-2 text-sm text-muted-foreground">
                "The shop's own costs are set on the " <a class=(LINK) href=(&costs_page)>"Costs"</a> " page."
            </p>
            <form method="get" class="mt-4 flex flex-wrap items-end gap-2" data-what-if="">
                <input type="hidden" name="tab" value=(Tab::Margin.key().unwrap_or_default()) />
                field(
                    field_label(attrs: attributes! { for="what-if" }, "What if it sold at (" (currency) ")")
                    input(attrs: attributes! {
                        id="what-if" name="what_if" type="text" inputmode="decimal" class="w-40" value=(&margin.what_if)
                    })
                )
                button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Work it out")
            </form>
            if !margin.what_if.is_empty() {
                <p class="mt-2 text-sm text-muted-foreground" data-what-if-shown="">"Worked out as if everything below sold at " (&margin.what_if) " " (currency) "."</p>
            }
            table(
                attrs: attributes! { class="mt-4" data-margin-rows="" },
                table_header(
                    table_row(
                        table_head("Sold")
                        table_head(attrs: attributes! { class="text-right" }, "Price")
                        table_head(attrs: attributes! { class="text-right" }, "VAT")
                        table_head(attrs: attributes! { class="text-right" }, "Cost")
                        table_head(attrs: attributes! { class="text-right" }, "Sale costs")
                        table_head(attrs: attributes! { class="text-right" }, "Margin")
                        table_head(attrs: attributes! { class="text-right" }, "Of the price")
                        table_head(attrs: attributes! { class="text-right" }, "Break-even return")
                        table_head("Tier")
                    )
                )
                table_body(
                    for row in &margin.rows {
                        table_row(
                            attrs: attributes! { data-margin-row=(&row.id) data-tier=(row.tier) },
                            table_cell(
                                (&row.label)
                                if !row.on_sale {
                                    <span class="text-muted-foreground">" (off sale)"</span>
                                }
                            )
                            if let Some(sale) = &row.margin {
                                table_cell(attrs: attributes! { class="text-right tabular-nums" }, (format_money(sale.price_cents, currency)))
                                table_cell(attrs: attributes! { class="text-right tabular-nums" }, (format_money(sale.vat_cents, currency)))
                                table_cell(attrs: attributes! { class="text-right tabular-nums" }, (format_money(sale.cost_cents, currency)))
                                table_cell(attrs: attributes! { class="text-right tabular-nums" }, (format_money(sale.shipping_cents + sale.import_cents + sale.fixed_cents + sale.payment_fee_cents, currency)))
                                table_cell(attrs: attributes! { class="text-right font-medium tabular-nums" data-margin=(sale.margin_cents()) }, (signed_money(sale.margin_cents(), currency)))
                                table_cell(attrs: attributes! { class="text-right tabular-nums" }, (sale.rate_bps().map(share_label).unwrap_or_default()))
                                table_cell(attrs: attributes! { class="text-right tabular-nums" data-break-even="" },
                                    if let Some(roas) = sale.break_even_roas() {
                                        (roas_label(roas))
                                    } else {
                                        "None: it loses money"
                                    }
                                )
                                table_cell((row.tier))
                            } else {
                                <td class="p-3 text-muted-foreground" colspan="8">
                                    if margin.other_currency.is_some() {
                                        "Not told: another currency than the shop's costs."
                                    } else if row.cost.is_empty() {
                                        "Not told: its cost is not known."
                                    } else {
                                        "Not told: it has no price yet."
                                    }
                                </td>
                            }
                        )
                    }
                )
            )
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_read_as_a_merchant_writes_them() {
        assert_eq!(signed_money(-320, "EUR"), "-3.20 €");
        assert_eq!(signed_money(2385, "EUR"), "23.85 €");
        assert_eq!(share_label(3_975), "39.7 %");
        assert_eq!(share_label(-250), "-2.5 %");
        assert_eq!(share_label(0), "0.0 %");
        assert_eq!(roas_label(252), "2.52");
        assert_eq!(roas_label(1_000), "10.00");
    }
}
