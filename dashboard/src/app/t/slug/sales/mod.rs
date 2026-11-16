//! `/t/{slug}/sales`: what the shop sold over a period, for members with
//! `orders:read`: its revenue, orders, average basket, refunds and profit,
//! then the same by the channel that brought the shopper, by product (the
//! best sellers) and by category. Summed from the orders read model, a
//! moment behind the orders themselves; the days are the shop's own
//! (its time zone).

use jiff::{Timestamp, ToSpan, civil::Date, tz::TimeZone};
use timada_catalog::format_money;
use timada_orders::{Period, best_sellers, sales_by_category, sales_by_source, sales_totals};
use timada_tenancy::PERMISSION_ORDERS_READ;
use topcoat::{
    Result,
    context::{Cx, context},
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use super::{TenantAccess, TenantNav, require_tenant, slug};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::input::input;
use crate::components::select::select;
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::db::Db;
use crate::ui::{
    LINK, Shell, document, empty_row, fact, facts, filter_bar, page_header, section_card,
    signed_money,
};

/// How many products the best sellers list.
const BEST_SELLERS: i64 = 20;
/// The periods offered, in days up to today, with what the menu calls them.
const SPANS: [(u32, &str); 4] = [
    (7, "Last 7 days"),
    (30, "Last 30 days"),
    (90, "Last 90 days"),
    (365, "Last 12 months"),
];
/// The span a page without a choice shows.
const DEFAULT_SPAN: u32 = 30;

#[topcoat::router::query_params(error = bad_request)]
struct SalesQuery {
    /// One of [`SPANS`], in days; anything else is the default, unless the
    /// two dates below are given.
    days: Option<String>,
    /// The first and the last day, `YYYY-MM-DD`, both included.
    from: Option<String>,
    to: Option<String>,
}

/// The days a report covers, in the shop's zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Days {
    first: Date,
    last: Date,
    /// The span picked from the menu; `None` for dates typed in.
    span: Option<u32>,
}

impl Days {
    /// What the query asks for, `today` being the shop's: two dates in
    /// order when both are given, else a span ending today.
    fn of(query: &SalesQuery, today: Date) -> Self {
        let date = |value: &Option<String>| value.as_deref()?.trim().parse::<Date>().ok();
        if let (Some(first), Some(last)) = (date(&query.from), date(&query.to))
            && first <= last
        {
            return Self {
                first,
                last,
                span: None,
            };
        }
        let span = query
            .days
            .as_deref()
            .and_then(|days| days.parse::<u32>().ok())
            .filter(|days| SPANS.iter().any(|(span, _)| span == days))
            .unwrap_or(DEFAULT_SPAN);
        Self {
            first: today
                .checked_sub((i64::from(span) - 1).days())
                .unwrap_or(today),
            last: today,
            span: Some(span),
        }
    }

    fn period(&self, zone: &TimeZone) -> Period {
        period_of_days(zone, self.first, self.last)
    }
}

/// From the first day's midnight to the midnight after the last, as the
/// shop's clocks have them.
pub(crate) fn period_of_days(zone: &TimeZone, first: Date, last: Date) -> Period {
    let midnight = |day: Date| {
        day.to_zoned(zone.clone())
            .map_or(0, |start| start.timestamp().as_second())
    };
    Period {
        from: midnight(first),
        to: midnight(last.tomorrow().unwrap_or(last)),
    }
}

/// What a row left, or that it is not known: `costed` of its `all` (orders
/// or things sold) kept their costs.
fn profit_label(profit_cents: i64, costed: i64, all: i64, currency: &str) -> String {
    match costed {
        0 => "Not told".to_owned(),
        costed if costed == all => signed_money(profit_cents, currency),
        costed => format!(
            "{} on {costed} of {all}",
            signed_money(profit_cents, currency)
        ),
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ORDERS_READ)
        .await?;
    let query = query_params::<SalesQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let TenantAccess { tenant, .. } = access;
    let zone = TimeZone::get(tenant.shop_time_zone()).unwrap_or(TimeZone::UTC);
    let today = Timestamp::now().to_zoned(zone.clone()).date();
    let days = Days::of(query, today);
    let period = days.period(&zone);
    let db = context::<Db>(cx);
    let totals = sales_totals(db, &tenant.id, period)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let sources = sales_by_source(db, &tenant.id, period)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let sellers = best_sellers(db, &tenant.id, period, BEST_SELLERS)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let categories = sales_by_category(db, &tenant.id, period)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let money = |cents: i64, currency: &str| format_money(cents.max(0) as u64, currency);
    let slug = tenant.slug.clone();
    let first = days.first.to_string();
    let last = days.last.to_string();
    let title = format!("Sales report · {}", tenant.name);

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "sales"),
            page_header(
                title: "Sales report",
                subtitle: Some("What the orders placed over a period add up to. An order counts from the day it is placed, on the shop's clock, and for nothing once cancelled; its refunds count with it, whenever they were made."),
            )

            section_card(
                filter_bar(
                    <label class="text-sm" for="sales-days">"Period"</label>
                    select(
                        attrs: attributes! { id="sales-days" name="days" class="w-44" },
                        for (span, label) in SPANS {
                            <option value=(span) selected=(days.span == Some(span))>(label)</option>
                        }
                        if days.span.is_none() {
                            <option value="" selected="">"The dates beside"</option>
                        }
                    )
                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Show")
                )
                filter_bar(
                    <label class="text-sm" for="sales-from">"From"</label>
                    input(attrs: attributes! { id="sales-from" name="from" type="date" class="w-44" value=(&first) required="" })
                    <label class="text-sm" for="sales-to">"To"</label>
                    input(attrs: attributes! { id="sales-to" name="to" type="date" class="w-44" value=(&last) required="" })
                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Show these days")
                )
                <p class="text-sm text-muted-foreground" data-sales-period="">
                    "From " <time datetime=(&first)>(&first)</time> " to " <time datetime=(&last)>(&last)</time> ", both included."
                </p>
            )

            if totals.is_empty() {
                section_card(
                    title: Some("Totals"),
                    <p class="text-sm text-muted-foreground" data-sales-empty="">"No order was placed over these days."</p>
                )
            }
            for total in &totals {
                section_card(
                    title: Some("Totals"),
                    description: Some("Revenue is what the orders were paid: goods, less discounts, plus shipping, VAT included. Profit is told for the orders that kept what each of their goods cost, their refunds taken off."),
                    attrs: attributes! { data-sales-totals=(&total.currency) },
                    facts(
                        fact(term: "Orders", attrs: attributes! { data-sales-orders=(total.orders) }, (total.orders))
                        fact(term: "Revenue", attrs: attributes! { data-sales-revenue=(total.revenue_cents) }, (money(total.revenue_cents, &total.currency)))
                        fact(term: "Average basket", (total.average_basket_cents().map_or_else(|| "None".to_owned(), |cents| money(cents, &total.currency))))
                        fact(term: "Refunds", attrs: attributes! { data-sales-refunds=(total.refunded_cents) }, (money(total.refunded_cents, &total.currency)))
                        fact(term: "After refunds", (signed_money(total.net_cents(), &total.currency)))
                        fact(term: "Items sold", (total.items))
                        fact(term: "Cancelled", (total.cancelled))
                        fact(
                            term: "Profit",
                            attrs: attributes! { data-sales-profit=(if total.costed_orders > 0 { total.profit_cents.to_string() } else { String::new() }) },
                            if total.costed_orders == 0 {
                                "Not told: no order of these days kept what its goods cost. Say it on each product's Cost & margin tab."
                            } else {
                                (profit_label(total.profit_cents, total.costed_orders, total.orders, &total.currency))
                                if total.costed_orders < total.orders { " orders" }
                            }
                        )
                    )
                )
            }

            section_card(
                title: Some("By source"),
                description: Some("What brought the shopper, as the storefront kept it with their consent."),
                table(
                    attrs: attributes! { data-sales-sources="" },
                    table_header(
                        table_row(
                            table_head("Source")
                            table_head(attrs: attributes! { class="text-right" }, "Orders")
                            table_head(attrs: attributes! { class="text-right" }, "Revenue")
                            table_head(attrs: attributes! { class="text-right" }, "Refunds")
                            table_head(attrs: attributes! { class="text-right" }, "Profit")
                        )
                    )
                    table_body(
                        if sources.is_empty() {
                            empty_row(colspan: 5, text: "Nothing sold over these days.")
                        }
                        for row in &sources {
                            table_row(
                                attrs: attributes! { data-sales-source=(row.source.as_deref().unwrap_or("")) },
                                table_cell((row.source.as_deref().unwrap_or("Not said")))
                                table_cell(attrs: attributes! { class="text-right tabular-nums" }, (row.orders))
                                table_cell(attrs: attributes! { class="text-right tabular-nums" }, (money(row.revenue_cents, &row.currency)))
                                table_cell(attrs: attributes! { class="text-right tabular-nums" }, (money(row.refunded_cents, &row.currency)))
                                table_cell(attrs: attributes! { class="text-right tabular-nums" }, (profit_label(row.profit_cents, row.costed_orders, row.orders, &row.currency)))
                            )
                        }
                    )
                )
            )

            section_card(
                title: Some("Best sellers"),
                description: Some("The products that sold for the most, by what their lines were sold for (VAT included, before any discount on the order, without shipping). A refund is an order's and is not taken off here."),
                table(
                    attrs: attributes! { data-sales-products="" },
                    table_header(
                        table_row(
                            table_head("Product")
                            table_head(attrs: attributes! { class="text-right" }, "Sold")
                            table_head(attrs: attributes! { class="text-right" }, "Sales")
                            table_head(attrs: attributes! { class="text-right" }, "Profit")
                        )
                    )
                    table_body(
                        if sellers.is_empty() {
                            empty_row(colspan: 4, text: "Nothing sold over these days.")
                        }
                        for row in &sellers {
                            table_row(
                                attrs: attributes! { data-sales-product=(&row.name) },
                                table_cell(
                                    <a class=(LINK) href=(href!(super::products::product_id::index, slug: slug.clone(), super::products::product_id::product_id: row.product_id.clone()))>(&row.name)</a>
                                )
                                table_cell(attrs: attributes! { class="text-right tabular-nums" data-sales-quantity="" }, (row.quantity))
                                table_cell(attrs: attributes! { class="text-right tabular-nums" }, (money(row.sales_cents, &row.currency)))
                                table_cell(attrs: attributes! { class="text-right tabular-nums" }, (profit_label(row.profit_cents, row.costed_quantity, row.quantity, &row.currency)))
                            )
                        }
                    )
                )
            )

            section_card(
                title: Some("By category"),
                description: Some("As the catalog files its products today. A product in two categories counts in both, so these do not add up to the totals."),
                table(
                    attrs: attributes! { data-sales-categories="" },
                    table_header(
                        table_row(
                            table_head("Category")
                            table_head(attrs: attributes! { class="text-right" }, "Sold")
                            table_head(attrs: attributes! { class="text-right" }, "Sales")
                            table_head(attrs: attributes! { class="text-right" }, "Profit")
                        )
                    )
                    table_body(
                        if categories.is_empty() {
                            empty_row(colspan: 4, text: "Nothing sold over these days.")
                        }
                        for row in &categories {
                            table_row(
                                attrs: attributes! { data-sales-category=(row.name.as_deref().unwrap_or("")) },
                                table_cell((row.name.as_deref().unwrap_or("In no category")))
                                table_cell(attrs: attributes! { class="text-right tabular-nums" }, (row.quantity))
                                table_cell(attrs: attributes! { class="text-right tabular-nums" }, (money(row.sales_cents, &row.currency)))
                                table_cell(attrs: attributes! { class="text-right tabular-nums" }, (profit_label(row.profit_cents, row.costed_quantity, row.quantity, &row.currency)))
                            )
                        }
                    )
                )
            )
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(days: Option<&str>, from: Option<&str>, to: Option<&str>) -> SalesQuery {
        SalesQuery {
            days: days.map(str::to_owned),
            from: from.map(str::to_owned),
            to: to.map(str::to_owned),
        }
    }

    fn day(value: &str) -> Date {
        value.parse().unwrap()
    }

    #[test]
    fn a_span_ends_today_and_dates_are_taken_in_order() {
        let today = day("2026-11-16");
        let month = Days::of(&query(None, None, None), today);
        assert_eq!(month.first, day("2026-10-18"));
        assert_eq!(month.last, today);
        assert_eq!(month.span, Some(30));
        assert_eq!(
            Days::of(&query(Some("7"), None, None), today).first,
            day("2026-11-10")
        );
        // Not one of the spans offered: the default.
        assert_eq!(
            Days::of(&query(Some("5000"), None, None), today).span,
            Some(30)
        );
        let typed = Days::of(
            &query(Some("7"), Some("2026-10-01"), Some("2026-10-31")),
            today,
        );
        assert_eq!(
            (typed.first, typed.last),
            (day("2026-10-01"), day("2026-10-31"))
        );
        assert_eq!(typed.span, None);
        // Backwards, or half given: the span.
        assert_eq!(
            Days::of(&query(None, Some("2026-10-31"), Some("2026-10-01")), today).span,
            Some(30)
        );
        assert_eq!(
            Days::of(&query(None, Some("2026-10-01"), None), today).span,
            Some(30)
        );
    }

    #[test]
    fn a_day_is_the_shop_s_from_midnight_to_midnight() {
        let paris = TimeZone::get("Europe/Paris").unwrap();
        // The day the clocks went back: twenty-five hours.
        let days = Days {
            first: day("2026-10-25"),
            last: day("2026-10-25"),
            span: None,
        };
        let period = days.period(&paris);
        assert_eq!(period.to - period.from, 25 * 3600);
        assert_eq!(
            period.from,
            "2026-10-24T22:00:00Z"
                .parse::<Timestamp>()
                .unwrap()
                .as_second()
        );
    }

    #[test]
    fn profit_says_how_much_of_the_row_it_covers() {
        assert_eq!(profit_label(1200, 0, 3, "EUR"), "Not told");
        assert_eq!(profit_label(1200, 3, 3, "EUR"), "12.00 €");
        assert_eq!(profit_label(-250, 1, 3, "EUR"), "-2.50 € on 1 of 3");
    }
}
