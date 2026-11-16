//! What an order left the shop, as its costs stood when it was sold.

use timada_catalog::format_money;
use timada_orders::OrderState;
use topcoat::{
    Result,
    context::Cx,
    view::{View, attributes, component, view},
};

use crate::components::card::{card, card_content, card_header};
use crate::ui::{fact, facts, signed_money};

/// The card of an order's costs and what it left; nothing for an order
/// sold before the shop said what any of its goods cost.
#[component]
pub async fn order_costs(cx: &Cx, order: &OrderState) -> Result<impl View> {
    let money = |cents: u64| format_money(cents, &order.currency);
    let rows = order.costs.as_ref().map(|costs| {
        let goods = costs
            .goods_cents(&order.lines)
            .map_or_else(|| "Not known for every item".to_owned(), money);
        [
            ("Goods", goods),
            ("Sending", money(costs.shipping_cents)),
            ("At the border", money(costs.import_cents)),
            ("Fixed costs", money(costs.fixed_cents)),
            ("Payment fee", money(costs.payment_fee_cents)),
            ("VAT expected", money(costs.vat_cents)),
        ]
    });
    let profit = order.profit_cents();
    let profit_label = profit.map(|cents| signed_money(cents, &order.currency));
    Ok(view! {
        if let Some(rows) = &rows {
            card(
                attrs: attributes! { data-order-costs="" },
                card_header(
                    <h2 class="text-base font-semibold">"What it left"</h2>
                    <p class="text-sm text-muted-foreground">"Out of the total, as the costs stood when it was sold. A refund is not taken off."</p>
                )
                card_content(
                    facts(
                        for (term, value) in rows {
                            fact(term: term, (value))
                        }
                        if let Some(label) = &profit_label {
                            fact(term: "Left", attrs: attributes! { class="font-medium" data-order-profit=(profit.unwrap_or_default()) }, (label))
                        } else {
                            fact(term: "Left", attrs: attributes! { data-order-profit="" }, "Not told: the shop had not said what every item cost.")
                        }
                    )
                )
            )
        }
    })
}
