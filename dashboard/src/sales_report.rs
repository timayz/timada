//! What a period's sales left once the ads that brought them are paid: the
//! part of the sales report that sets what an ad channel cost against what
//! the orders it brought were paid, for the shop, by channel, by product
//! and by category. The page shows it and the scheduled export writes it.
//!
//! An order is matched to a channel by the source it kept
//! ([`timada_orders::OrderSource::channel`]): the last arrival before the
//! shopper paid, as `source / medium`. Spend comes from what the daily
//! read of Google Ads recorded (`timada-google`), on the same calendar
//! days, in the account's currency: sales in another currency are left out
//! of the comparison. A second channel that tells its spend is one more
//! [`Paid`] kind and one more row.
//!
//! The break-even of a product or a category is what the period's own
//! sales say: what was sold for each 1 it left before ads, from the lines
//! that kept their costs. A return above it earns, one below costs.

use std::collections::BTreeMap;

use timada_catalog::{FeedItems, feed_items};
use timada_core::format_amount;
use timada_google::{MAX_ADS_PRODUCT_ROWS, ads_account_of, ads_by_product, ads_totals};
use timada_orders::{Period, SourceSales, sales_by_product_source, sales_by_source, sales_totals};

use crate::db::Db;
use crate::export::csv_line;

/// What kind of click brought an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Paid {
    /// An ad of Google Ads.
    GoogleAds,
    /// An ad somewhere timada does not read the spend of.
    Other,
    /// No paid click: a search, a mail, a link, a visit typed in.
    Free,
}

/// The mediums campaigns tag a paid click with.
const PAID_MEDIUMS: [&str; 9] = [
    "cpc",
    "ppc",
    "cpm",
    "paid",
    "paidsearch",
    "paid_search",
    "paidsocial",
    "paid_social",
    "display",
];

/// The kind of click a channel (`source / medium`) is; one the storefront
/// did not say is no paid click.
pub fn paid_of(source: Option<&str>) -> Paid {
    let Some((source, medium)) = source.and_then(|channel| channel.split_once(" / ")) else {
        return Paid::Free;
    };
    if !PAID_MEDIUMS.contains(&medium) {
        return Paid::Free;
    }
    match source {
        "google" | "adwords" | "googleads" | "google_ads" => Paid::GoogleAds,
        _ => Paid::Other,
    }
}

/// `value` for 1 of `over`, in hundredths; `None` when `over` is nothing.
fn ratio(value: i64, over: i64) -> Option<i64> {
    (over > 0).then(|| ((i128::from(value) * 100 + i128::from(over) / 2) / i128::from(over)) as i64)
}

/// The orders one kind of click brought over the period.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelRow {
    pub paid: Paid,
    /// What the channel cost; `None` where it is not known (a channel
    /// timada does not read) or there is none to pay (no paid click).
    pub spend_cents: Option<i64>,
    pub orders: i64,
    pub revenue_cents: i64,
    pub refunded_cents: i64,
    pub costed_orders: i64,
    pub profit_cents: i64,
}

impl ChannelRow {
    pub fn key(&self) -> &'static str {
        match self.paid {
            Paid::GoogleAds => "google-ads",
            Paid::Other => "other-paid",
            Paid::Free => "free",
        }
    }

    pub fn label(&self) -> &'static str {
        match self.paid {
            Paid::GoogleAds => "Google Ads",
            Paid::Other => "Other paid clicks",
            Paid::Free => "Without a paid click",
        }
    }

    /// What the orders were paid for 1 spent, in hundredths.
    pub fn return_hundredths(&self) -> Option<i64> {
        ratio(self.revenue_cents, self.spend_cents?)
    }

    /// What the orders left once the channel is paid; `None` unless every
    /// one of them knows what it left, and the spend is known.
    pub fn after_ads_cents(&self) -> Option<i64> {
        let spend = self.spend_cents?;
        (self.costed_orders == self.orders).then_some(self.profit_cents - spend)
    }
}

/// Where a product or a category stands against its break-even.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Its return is at or above its break-even.
    Earns,
    /// Below it, or ads paid for with nothing sold.
    Costs,
    /// Sold, but none of it says what it left.
    NotKnown,
}

impl Verdict {
    pub fn label(self) -> &'static str {
        match self {
            Self::Earns => "Earns",
            Self::Costs => "Costs",
            Self::NotKnown => "Not known",
        }
    }
}

/// What Google Ads cost for a product, or a category, and what the orders
/// it brought bought of it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ItemRow {
    /// The product's id, the category's, or the feed's item id for an
    /// item the catalog no longer has.
    pub key: String,
    pub name: String,
    /// The product, when the catalog has it.
    pub product_id: Option<String>,
    pub spend_cents: i64,
    pub quantity: i64,
    pub sales_cents: i64,
    pub costed_quantity: i64,
    pub costed_sales_cents: i64,
    pub profit_cents: i64,
}

impl ItemRow {
    /// What was sold for 1 spent, in hundredths; `None` with no spend.
    pub fn return_hundredths(&self) -> Option<i64> {
        ratio(self.sales_cents, self.spend_cents)
    }

    /// What has to be sold for 1 left before ads, in hundredths: from the
    /// lines that kept their costs. `None` when none did, or they left
    /// nothing.
    pub fn break_even_hundredths(&self) -> Option<i64> {
        ratio(self.costed_sales_cents, self.profit_cents)
    }

    /// The return less the break-even, in hundredths.
    pub fn gap_hundredths(&self) -> Option<i64> {
        Some(self.return_hundredths()? - self.break_even_hundredths()?)
    }

    /// What is left of it once its ads are paid; `None` unless everything
    /// sold of it knows what it left.
    pub fn after_ads_cents(&self) -> Option<i64> {
        (self.costed_quantity == self.quantity).then_some(self.profit_cents - self.spend_cents)
    }

    /// Where it stands; `None` for one no ad was paid for.
    pub fn verdict(&self) -> Option<Verdict> {
        if self.spend_cents <= 0 {
            return None;
        }
        if self.sales_cents <= 0 {
            return Some(Verdict::Costs);
        }
        Some(
            match (self.return_hundredths(), self.break_even_hundredths()) {
                (Some(back), Some(even)) if back >= even => Verdict::Earns,
                (Some(_), Some(_)) => Verdict::Costs,
                // Sold at a loss before any ad.
                _ if self.costed_quantity > 0 && self.profit_cents <= 0 => Verdict::Costs,
                _ => Verdict::NotKnown,
            },
        )
    }

    fn add(&mut self, other: &Self) {
        self.spend_cents += other.spend_cents;
        self.quantity += other.quantity;
        self.sales_cents += other.sales_cents;
        self.costed_quantity += other.costed_quantity;
        self.costed_sales_cents += other.costed_sales_cents;
        self.profit_cents += other.profit_cents;
    }
}

/// A period's sales against what the ads cost.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdReport {
    /// The currency of everything below: the Google Ads account's when
    /// spend was read, else the one the shop sold the most in. Empty when
    /// there is neither spend nor sale.
    pub currency: String,
    /// Whether the shop has a Google Ads account chosen.
    pub connected: bool,
    /// What Google Ads cost over the days, every campaign together, and
    /// over how many days it holds numbers.
    pub spend_cents: i64,
    pub spend_days: i64,
    /// By kind of click, the paid ones first; a kind with neither order
    /// nor spend is left out.
    pub channels: Vec<ChannelRow>,
    /// By product, the one that cost the most first: every product Google
    /// Ads was paid for, or that an order it brought bought.
    pub products: Vec<ItemRow>,
    /// The same added up by the categories the catalog files the products
    /// under today; a product in two counts in both.
    pub categories: Vec<ItemRow>,
}

impl AdReport {
    /// The orders of one kind of click.
    pub fn channel(&self, paid: Paid) -> Option<&ChannelRow> {
        self.channels.iter().find(|row| row.paid == paid)
    }

    /// The part of the revenue that came without a paid click, in
    /// hundredths of a percent; `None` when nothing was sold.
    pub fn free_share_bps(&self) -> Option<i64> {
        let all: i64 = self.channels.iter().map(|row| row.revenue_cents).sum();
        let free = self.channel(Paid::Free).map_or(0, |row| row.revenue_cents);
        (all > 0).then(|| (free * 10_000 + all / 2) / all)
    }
}

/// The channels' rows from the period's sales by source, in `currency`.
fn channels(sources: &[SourceSales], currency: &str, spend_cents: Option<i64>) -> Vec<ChannelRow> {
    let mut rows: BTreeMap<Paid, ChannelRow> = BTreeMap::new();
    fn of(rows: &mut BTreeMap<Paid, ChannelRow>, paid: Paid) -> &mut ChannelRow {
        rows.entry(paid).or_insert(ChannelRow {
            paid,
            spend_cents: None,
            orders: 0,
            revenue_cents: 0,
            refunded_cents: 0,
            costed_orders: 0,
            profit_cents: 0,
        })
    }
    if let Some(spend) = spend_cents.filter(|spend| *spend > 0) {
        of(&mut rows, Paid::GoogleAds).spend_cents = Some(spend);
    }
    for source in sources.iter().filter(|row| row.currency == currency) {
        let row = of(&mut rows, paid_of(source.source.as_deref()));
        row.orders += source.orders;
        row.revenue_cents += source.revenue_cents;
        row.refunded_cents += source.refunded_cents;
        row.costed_orders += source.costed_orders;
        row.profit_cents += source.profit_cents;
    }
    // Orders Google Ads brought with no spend read: nothing was paid, as
    // far as is known.
    let mut rows: Vec<ChannelRow> = rows.into_values().collect();
    for row in &mut rows {
        if row.paid == Paid::GoogleAds && row.spend_cents.is_none() {
            row.spend_cents = spend_cents;
        }
    }
    rows
}

/// The categories' rows from the products'.
fn categories(products: &[ItemRow], items: &FeedItems) -> Vec<ItemRow> {
    let mut sums: BTreeMap<String, ItemRow> = BTreeMap::new();
    let mut add = |key: &str, name: &str, row: &ItemRow| {
        sums.entry(key.to_owned())
            .or_insert_with(|| ItemRow {
                key: key.to_owned(),
                name: name.to_owned(),
                ..ItemRow::default()
            })
            .add(row);
    };
    for row in products {
        let Some(product_id) = &row.product_id else {
            add("gone", "No longer in the catalog", row);
            continue;
        };
        let filed = items.categories_of(product_id);
        if filed.is_empty() {
            add("none", "In no category", row);
        }
        for (id, name) in filed {
            add(id, name, row);
        }
    }
    let mut rows: Vec<ItemRow> = sums.into_values().collect();
    sort(&mut rows);
    rows
}

/// The ones that cost the most first, then the ones that sold the most.
fn sort(rows: &mut [ItemRow]) {
    rows.sort_by(|a, b| {
        (b.spend_cents, b.sales_cents, &a.key).cmp(&(a.spend_cents, a.sales_cents, &b.key))
    });
}

/// The sales of `tenant_id` placed over `period` against what its ads cost
/// on the days from `first` to `last` (`2026-11-20`, both included): the
/// same days, as the Google Ads account counts them.
pub async fn ad_report(
    db: &Db,
    tenant_id: &str,
    period: Period,
    first: &str,
    last: &str,
) -> anyhow::Result<AdReport> {
    let sources = sales_by_source(db, tenant_id, period).await?;
    let account = ads_account_of(db, tenant_id).await?;
    let mut report = AdReport {
        connected: account.is_some(),
        ..AdReport::default()
    };
    let mut spend_by_item = Vec::new();
    if let Some(customer_id) = &account {
        let totals = ads_totals(db, tenant_id, customer_id, first, last).await?;
        report.spend_cents = totals.figures().cost_cents();
        report.spend_days = totals.days;
        report.currency = totals.currency;
        spend_by_item = ads_by_product(
            db,
            tenant_id,
            customer_id,
            first,
            last,
            MAX_ADS_PRODUCT_ROWS as i64,
        )
        .await?;
    }
    if report.currency.is_empty() {
        // The sources come the one that brought the most first.
        report.currency = sources
            .first()
            .map(|row| row.currency.clone())
            .unwrap_or_default();
    }
    report.channels = channels(
        &sources,
        &report.currency,
        account.is_some().then_some(report.spend_cents),
    );

    // By product: what Google Ads was paid for each, and what the orders
    // it brought bought of each.
    let items = feed_items(db, tenant_id).await?;
    let mut products: BTreeMap<String, ItemRow> = BTreeMap::new();
    for line in &spend_by_item {
        let spend = line.figures().cost_cents();
        match items.product_of(&line.key) {
            Some((product_id, name)) => {
                let row = products
                    .entry(product_id.to_owned())
                    .or_insert_with(|| ItemRow {
                        key: product_id.to_owned(),
                        name: name.to_owned(),
                        product_id: Some(product_id.to_owned()),
                        ..ItemRow::default()
                    });
                row.spend_cents += spend;
            }
            None => {
                products.insert(
                    format!("item:{}", line.key),
                    ItemRow {
                        key: line.key.clone(),
                        name: line.key.clone(),
                        spend_cents: spend,
                        ..ItemRow::default()
                    },
                );
            }
        }
    }
    for sold in sales_by_product_source(db, tenant_id, period).await? {
        if sold.currency != report.currency || paid_of(sold.source.as_deref()) != Paid::GoogleAds {
            continue;
        }
        let row = products
            .entry(sold.product_id.clone())
            .or_insert_with(|| ItemRow {
                key: sold.product_id.clone(),
                name: sold.name.clone(),
                product_id: Some(sold.product_id.clone()),
                ..ItemRow::default()
            });
        row.quantity += sold.quantity;
        row.sales_cents += sold.sales_cents;
        row.costed_quantity += sold.costed_quantity;
        row.costed_sales_cents += sold.costed_sales_cents;
        row.profit_cents += sold.profit_cents;
    }
    report.products = products.into_values().collect();
    sort(&mut report.products);
    report.categories = categories(&report.products, &items);
    Ok(report)
}

/// The columns of the sales file.
pub const SALES_COLUMNS: [&str; 13] = [
    "section",
    "key",
    "name",
    "currency",
    "orders",
    "quantity",
    "revenue",
    "refunds",
    "profit",
    "ad_spend",
    "return_on_ad_spend",
    "break_even",
    "after_ads",
];

/// A ratio in hundredths as a file writes it: `3.45`.
fn decimal(hundredths: Option<i64>) -> String {
    hundredths
        .map(|value| format!("{}.{:02}", value / 100, value % 100))
        .unwrap_or_default()
}

/// An amount that may be a loss, without its currency: `-3.20`.
fn signed(cents: i64) -> String {
    let amount = format_amount(cents.unsigned_abs());
    if cents < 0 {
        format!("-{amount}")
    } else {
        amount
    }
}

/// The sales report of `tenant_id` over `period` as the lines of a CSV
/// file, without its head: the totals a currency, then each source, each
/// kind of click, each product and each category, told apart by their
/// first cell. A profit is left empty where not every sale says what it
/// left.
pub async fn sales_lines(
    db: &Db,
    tenant_id: &str,
    period: Period,
    first: &str,
    last: &str,
) -> anyhow::Result<String> {
    let mut out = String::new();
    let whole = |profit: i64, costed: i64, all: i64| {
        if all > 0 && costed == all {
            signed(profit)
        } else {
            String::new()
        }
    };
    let cents = |cents: i64| format_amount(cents.max(0) as u64);
    let report = ad_report(db, tenant_id, period, first, last).await?;
    for total in sales_totals(db, tenant_id, period).await? {
        let spend =
            (total.currency == report.currency && report.connected).then_some(report.spend_cents);
        csv_line(
            &mut out,
            &[
                "total".to_owned(),
                String::new(),
                String::new(),
                total.currency.clone(),
                total.orders.to_string(),
                total.items.to_string(),
                cents(total.revenue_cents),
                cents(total.refunded_cents),
                whole(total.profit_cents, total.costed_orders, total.orders),
                spend.map(cents).unwrap_or_default(),
                decimal(spend.and_then(|spend| ratio(total.revenue_cents, spend))),
                String::new(),
                spend
                    .filter(|_| total.orders > 0 && total.costed_orders == total.orders)
                    .map(|spend| signed(total.profit_cents - spend))
                    .unwrap_or_default(),
            ],
        );
    }
    for source in sales_by_source(db, tenant_id, period).await? {
        csv_line(
            &mut out,
            &[
                "source".to_owned(),
                crate::export::typed(source.source.as_deref().unwrap_or("")),
                String::new(),
                source.currency.clone(),
                source.orders.to_string(),
                String::new(),
                cents(source.revenue_cents),
                cents(source.refunded_cents),
                whole(source.profit_cents, source.costed_orders, source.orders),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
            ],
        );
    }
    for row in &report.channels {
        csv_line(
            &mut out,
            &[
                "channel".to_owned(),
                row.key().to_owned(),
                row.label().to_owned(),
                report.currency.clone(),
                row.orders.to_string(),
                String::new(),
                cents(row.revenue_cents),
                cents(row.refunded_cents),
                whole(row.profit_cents, row.costed_orders, row.orders),
                row.spend_cents.map(cents).unwrap_or_default(),
                decimal(row.return_hundredths()),
                String::new(),
                row.after_ads_cents().map(signed).unwrap_or_default(),
            ],
        );
    }
    for (section, rows) in [
        ("product", &report.products),
        ("category", &report.categories),
    ] {
        for row in rows {
            csv_line(
                &mut out,
                &[
                    section.to_owned(),
                    row.key.clone(),
                    crate::export::typed(&row.name),
                    report.currency.clone(),
                    String::new(),
                    row.quantity.to_string(),
                    cents(row.sales_cents),
                    String::new(),
                    whole(row.profit_cents, row.costed_quantity, row.quantity),
                    cents(row.spend_cents),
                    decimal(row.return_hundredths()),
                    decimal(row.break_even_hundredths()),
                    row.after_ads_cents()
                        .filter(|_| row.quantity > 0 || row.spend_cents > 0)
                        .map(signed)
                        .unwrap_or_default(),
                ],
            );
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
