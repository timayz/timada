//! `/t/{slug}/ads`: what the shop's Google Ads account cost and brought
//! over a period, for members with `orders:read`: its totals, then the
//! same by campaign, by kind of device, by ad group, by category, by
//! product and by search, each with its return on ad spend. Read once a
//! day through the shop's Google connection (`crate::ads_passes`), in the
//! account's currency and on the account's days, so a period ends on the
//! last day read: yesterday on the account's clock. Summed from the read
//! model; where the daily read stands is the watch's own stream.
//!
//! The return told here is Google's: the value of the conversions Google
//! Ads counted over what the clicks cost. What the shop's own orders say
//! is the sales report's.

use std::collections::BTreeMap;

use timada_catalog::{FeedItems, feed_items};
use timada_core::format_money;
use timada_google::{
    ACCESS_ADS, AdsFigures, AdsLine, MAX_ADS_PRODUCT_ROWS, ads_by_ad_group, ads_by_campaign,
    ads_by_device, ads_by_product, ads_by_term, ads_totals, day_before, last_ads_day,
};
use timada_tenancy::PERMISSION_ORDERS_READ;
use topcoat::{
    Result,
    context::{Cx, context},
    icon::icon,
    router::{href, page, query_params},
    view::{View, attributes, component, view},
};

use super::google::{customer_number, google};
use super::{TenantAccess, TenantNav, require_tenant, slug};
use crate::components::alert::{AlertVariant, alert, alert_description};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::select::select;
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::db::Db;
use crate::ui::{
    LINK, Shell, document, empty_row, fact, facts, filter_bar, format_time, icons, page_header,
    section_card,
};

/// How many products, and how many searches, a period lists.
const LISTED: usize = 100;
/// The periods offered, in days up to the last one read, with what the
/// menu calls them. The longest is as far back as the products and the
/// searches are kept.
const SPANS: [(u32, &str); 3] = [(7, "7 days"), (28, "28 days"), (90, "90 days")];
/// The span a page without a choice shows.
const DEFAULT_SPAN: u32 = 28;

#[topcoat::router::query_params(error = bad_request)]
struct AdsQuery {
    /// One of [`SPANS`], in days; anything else is the default.
    days: Option<String>,
}

/// The span the query asks for.
fn span_of(days: Option<&str>) -> u32 {
    days.and_then(|days| days.parse::<u32>().ok())
        .filter(|days| SPANS.iter().any(|(span, _)| span == days))
        .unwrap_or(DEFAULT_SPAN)
}

/// A count with its thousands apart: `12 480`.
fn count(n: i64) -> String {
    let digits = n.max(0).to_string();
    let mut out = String::new();
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at).is_multiple_of(3) {
            out.push('\u{202f}');
        }
        out.push(digit);
    }
    out
}

/// Thousandths of a conversion, with one decimal when it has one: `3`,
/// `1.5`.
fn conversions(milli: i64) -> String {
    let tenths = (milli.max(0) + 50) / 100;
    if tenths % 10 == 0 {
        count(tenths / 10)
    } else {
        format!("{}.{}", count(tenths / 10), tenths % 10)
    }
}

/// A return on ad spend in hundredths, as a person writes it: `3.45`.
pub(crate) fn ratio(hundredths: Option<i64>) -> String {
    match hundredths {
        Some(hundredths) => format!("{}.{:02}", hundredths / 100, hundredths % 100),
        None => "None".to_owned(),
    }
}

/// An amount of the account's currency.
fn money(cents: i64, currency: &str) -> String {
    format_money(cents.max(0) as u64, currency)
}

/// How a kind of device reads.
fn device_label(device: &str) -> &'static str {
    match device {
        "mobile" => "Phones",
        "desktop" => "Computers",
        "tablet" => "Tablets",
        "tv" => "Televisions",
        _ => "Other",
    }
}

/// One row of a table: a campaign, a device, an ad group, a category, a
/// product or a search, with what it cost and brought.
struct Row {
    /// What a test finds the row by.
    key: String,
    label: String,
    /// Where the label leads, for a product.
    href: Option<String>,
    figures: AdsFigures,
}

impl Row {
    fn of(line: &AdsLine, label: String) -> Self {
        Self {
            key: line.key.clone(),
            label,
            href: None,
            figures: line.figures(),
        }
    }
}

/// What the products cost and brought, added up by the categories the
/// catalog files them under today, the one that cost the most first. A
/// product in two categories counts in both.
fn by_category(products: &[AdsLine], items: &FeedItems) -> Vec<Row> {
    let mut sums: BTreeMap<(String, String), AdsFigures> = BTreeMap::new();
    let mut add = |key: &str, name: &str, line: &AdsLine| {
        let sum = sums.entry((key.to_owned(), name.to_owned())).or_default();
        sum.cost_micros += line.cost_micros;
        sum.impressions += line.impressions;
        sum.clicks += line.clicks;
        sum.conversions_milli += line.conversions_milli;
        sum.value_micros += line.value_micros;
    };
    for line in products {
        match items.product_of(&line.key) {
            None => add("gone", "No longer in the catalog", line),
            Some((product_id, _)) => {
                let filed = items.categories_of(product_id);
                if filed.is_empty() {
                    add("none", "In no category", line);
                }
                for (id, name) in filed {
                    add(id, name, line);
                }
            }
        }
    }
    let mut rows: Vec<Row> = sums
        .into_iter()
        .map(|((key, label), figures)| Row {
            key,
            label,
            href: None,
            figures,
        })
        .collect();
    rows.sort_by(|a, b| (b.figures.cost_micros, &a.label).cmp(&(a.figures.cost_micros, &b.label)));
    rows
}

/// A table of rows with what each cost and brought.
#[component]
async fn figures_table(
    cx: &Cx,
    /// Names the table for a test: `data-ads-table`.
    name: &str,
    head: &str,
    currency: &str,
    rows: &[Row],
) -> Result<impl View> {
    Ok(view! {
        table(
            attrs: attributes! { data-ads-table=(name) },
            table_header(
                table_row(
                    table_head((head))
                    table_head(attrs: attributes! { class="text-right" }, "Cost")
                    table_head(attrs: attributes! { class="text-right" }, "Impressions")
                    table_head(attrs: attributes! { class="text-right" }, "Clicks")
                    table_head(attrs: attributes! { class="text-right" }, "Conversions")
                    table_head(attrs: attributes! { class="text-right" }, "Conversion value")
                    table_head(attrs: attributes! { class="text-right" }, "Return")
                )
            )
            table_body(
                if rows.is_empty() {
                    empty_row(colspan: 7, text: "Nothing shown over these days.")
                }
                for row in rows {
                    table_row(
                        attrs: attributes! { data-ads-row=(&row.key) },
                        table_cell(
                            if let Some(href) = &row.href {
                                <a class=(LINK) href=(href)>(&row.label)</a>
                            } else {
                                (&row.label)
                            }
                        )
                        table_cell(attrs: attributes! { class="text-right tabular-nums" data-cost=(row.figures.cost_cents()) }, (money(row.figures.cost_cents(), currency)))
                        table_cell(attrs: attributes! { class="text-right tabular-nums" }, (count(row.figures.impressions)))
                        table_cell(attrs: attributes! { class="text-right tabular-nums" data-clicks=(row.figures.clicks) }, (count(row.figures.clicks)))
                        table_cell(attrs: attributes! { class="text-right tabular-nums" }, (conversions(row.figures.conversions_milli)))
                        table_cell(attrs: attributes! { class="text-right tabular-nums" }, (money(row.figures.value_cents(), currency)))
                        table_cell(attrs: attributes! { class="text-right tabular-nums" data-return="" }, (ratio(row.figures.return_hundredths())))
                    )
                }
            )
        )
    })
}

/// Where the shop stands with Google Ads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Standing {
    /// No connection, or one that does not cover Google Ads.
    NotConnected,
    /// Google no longer honours the connection.
    Lapsed,
    /// Connected, with no account chosen.
    NoAccount,
    /// Read, or about to be.
    Read,
}

impl Standing {
    fn key(self) -> &'static str {
        match self {
            Self::NotConnected => "not-connected",
            Self::Lapsed => "lapsed",
            Self::NoAccount => "no-account",
            Self::Read => "read",
        }
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ORDERS_READ)
        .await?;
    let query = query_params::<AdsQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let span = span_of(query.days.as_deref());
    let command = google(cx);
    let connection = command
        .load_connection(access.tenant_id())
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .filter(|connection| connection.covers(ACCESS_ADS));
    let account = connection.as_ref().and_then(|c| c.ads.clone());
    let standing = match &connection {
        None => Standing::NotConnected,
        Some(connection) if connection.lapsed => Standing::Lapsed,
        Some(_) if account.is_none() => Standing::NoAccount,
        Some(_) => Standing::Read,
    };
    // Where the daily read stands, for the account it is about.
    let watch = command
        .load_ads_watch(access.tenant_id())
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .filter(|watch| Some(&watch.customer_id) == account.as_ref().map(|a| &a.customer_id));
    let db = context::<Db>(cx);
    let tenant_id = access.tenant_id().to_owned();
    let slug = access.tenant.slug.clone();
    let customer_id = account.as_ref().map(|account| account.customer_id.clone());
    // What a lapsed connection read before stays readable.
    let last = match &customer_id {
        Some(customer_id) => last_ads_day(db, &tenant_id, customer_id)
            .await
            .map_err(topcoat::Error::from_anyhow)?,
        None => None,
    };
    let period = last
        .as_ref()
        .map(|last| (day_before(last, span - 1), last.clone()));
    let mut totals = None;
    let mut campaigns = Vec::new();
    let mut devices = Vec::new();
    let mut ad_groups = Vec::new();
    let mut categories = Vec::new();
    let mut products = Vec::new();
    let mut terms = Vec::new();
    if let (Some(customer_id), Some((first, last))) = (&customer_id, &period) {
        let failed = topcoat::Error::from_anyhow;
        totals = Some(
            ads_totals(db, &tenant_id, customer_id, first, last)
                .await
                .map_err(failed)?,
        );
        for line in ads_by_campaign(db, &tenant_id, customer_id, first, last)
            .await
            .map_err(failed)?
        {
            campaigns.push(Row::of(&line, line.name.clone()));
        }
        for line in ads_by_device(db, &tenant_id, customer_id, first, last)
            .await
            .map_err(failed)?
        {
            devices.push(Row::of(&line, device_label(&line.key).to_owned()));
        }
        for line in ads_by_ad_group(db, &tenant_id, customer_id, first, last)
            .await
            .map_err(failed)?
        {
            ad_groups.push(Row::of(&line, line.name.clone()));
        }
        // Every product of the period, to add the categories up from; the
        // ones that cost the most are listed.
        let lines = ads_by_product(
            db,
            &tenant_id,
            customer_id,
            first,
            last,
            MAX_ADS_PRODUCT_ROWS as i64,
        )
        .await
        .map_err(failed)?;
        let items = feed_items(db, &tenant_id).await.map_err(failed)?;
        categories = by_category(&lines, &items);
        for line in lines.iter().take(LISTED) {
            let mut row = Row::of(line, line.key.clone());
            if let Some((product_id, name)) = items.product_of(&line.key) {
                row.label = name.to_owned();
                row.href = Some(
                    href!(
                        super::products::product_id::index,
                        slug: slug.clone(),
                        super::products::product_id::product_id: product_id.to_owned()
                    )
                    .resolve(cx),
                );
            }
            products.push(row);
        }
        for line in ads_by_term(db, &tenant_id, customer_id, first, last, LISTED as i64)
            .await
            .map_err(failed)?
        {
            terms.push(Row::of(&line, line.key.clone()));
        }
    }
    let TenantAccess { tenant, .. } = access;
    let title = format!("Google Ads · {}", tenant.name);
    let google_page = href!(super::google::index, slug: slug.clone()).resolve(cx);
    let failure = watch
        .as_ref()
        .filter(|_| standing == Standing::Read)
        .and_then(|watch| Some((watch.last_error.clone()?, watch.next_at)));
    let checked_at = watch.as_ref().map_or(0, |watch| watch.checked_at);
    let behind = watch.as_ref().is_some_and(|watch| watch.behind);
    let (first, last) = period.clone().unwrap_or_default();
    let account_label = account
        .as_ref()
        .map(|account| {
            if account.name.is_empty() {
                customer_number(&account.customer_id)
            } else {
                format!(
                    "{} ({})",
                    account.name,
                    customer_number(&account.customer_id)
                )
            }
        })
        .unwrap_or_default();
    let time_zone = account
        .as_ref()
        .map(|account| account.time_zone.clone())
        .filter(|zone| !zone.is_empty())
        .unwrap_or_else(|| "UTC".to_owned());
    // The currency the days were read in; the account's before any was.
    let currency = totals
        .as_ref()
        .map(|totals| totals.currency.clone())
        .filter(|currency| !currency.is_empty())
        .or(account.as_ref().map(|account| account.currency.clone()))
        .unwrap_or_default();
    let figures = totals.as_ref().map(|totals| totals.figures());
    let days_held = totals.as_ref().map_or(0, |totals| totals.days);
    let products_note = format!(
        "The products of the feed your ads showed, the ones that cost the most first, {LISTED} at most. A product sold in several variants is one item a variant in the feed: each has its row."
    );
    let terms_note = format!(
        "What people typed before clicking an ad, the searches that cost the most first, {LISTED} at most. A search nobody clicked is not kept."
    );

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "ads"),
            page_header(
                title: "Google Ads",
                subtitle: Some("What your campaigns cost and brought, as Google Ads counts it. Read once a day through the shop's Google account; nothing is changed in Google Ads."),
            )

            <div data-ads=(standing.key())>
                if standing == Standing::NotConnected {
                    section_card(
                        title: Some("Not connected"),
                        <p class="text-sm text-muted-foreground">
                            "Connect the Google account that manages your shop's Google Ads account, on the "
                            <a href=(&google_page) class=(LINK)>"Google account"</a>
                            " page, and tick Google Ads: what its campaigns cost and brought is read here from the next pass on."
                        </p>
                    )
                }
                if standing == Standing::Lapsed {
                    alert(
                        variant: AlertVariant::Destructive,
                        attrs: attributes! { role="alert" class="mb-6" data-ads-lapsed="" },
                        icon(data: icons::ERROR)
                        alert_description(
                            "Google no longer honours the shop's connection: nothing new is read until you "
                            <a href=(&google_page) class=(LINK)>"connect again"</a>
                            ". What was read before stays below."
                        )
                    )
                }
                if standing == Standing::NoAccount {
                    section_card(
                        title: Some("No account chosen"),
                        <p class="text-sm text-muted-foreground">
                            "The shop is connected to Google. Say which Google Ads account is this shop's on the "
                            <a href=(&google_page) class=(LINK)>"Google account"</a>
                            " page."
                        </p>
                    )
                }
                if let Some((reason, next_at)) = &failure {
                    alert(
                        variant: AlertVariant::Warning,
                        attrs: attributes! { role="alert" class="mb-6" data-ads-failure="" },
                        icon(data: icons::WARNING)
                        alert_description(
                            "The last read of Google Ads failed: " (reason) ". It is tried again from "
                            <time datetime=(*next_at)>(format_time(*next_at))</time> "."
                        )
                    )
                }
            </div>

            if account.is_some() {
                section_card(
                    filter_bar(
                        <label class="text-sm" for="ads-days">"Period"</label>
                        select(
                            attrs: attributes! { id="ads-days" name="days" class="w-44" },
                            for (days, label) in SPANS {
                                <option value=(days) selected=(span == days)>"Last " (label)</option>
                            }
                        )
                        button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Show")
                    )
                    if period.is_some() {
                        <p class="text-sm text-muted-foreground" data-ads-period="">
                            "From " <time datetime=(&first)>(&first)</time> " to " <time datetime=(&last)>(&last)</time>
                            ", both included, for " (&account_label) ". Days are the account's ("
                            (&time_zone) "). Google counts a conversion on the day of its click, so the last two weeks may still move: they are read again every day."
                        </p>
                    } else if standing == Standing::Read && failure.is_none() {
                        <p class="text-sm text-muted-foreground" data-ads-waiting="">
                            "Nothing read yet for " (&account_label)
                            if checked_at > 0 {
                                ": no ad of the account was shown over the days read."
                            } else {
                                ": the first read is on its way, with the last 90 days."
                            }
                        </p>
                    } else {
                        <p class="text-sm text-muted-foreground">"Nothing read yet for " (&account_label) "."</p>
                    }
                    if checked_at > 0 {
                        <p class="mt-2 text-sm text-muted-foreground">
                            "Last read on " <time datetime=(checked_at) data-ads-checked="">(format_time(checked_at))</time>
                            if behind { ". Older days are still being read" }
                            "."
                        </p>
                    }
                )
            }

            if let Some(figures) = &figures {
                section_card(
                    title: Some("Totals"),
                    description: Some("Every campaign of the account. The return is the value of the conversions Google Ads counted for 1 spent: it depends on what the Google tag reports, and is not the shop's own count of its orders."),
                    attrs: attributes! { data-ads-totals=(&currency) },
                    facts(
                        fact(term: "Cost", attrs: attributes! { data-ads-cost=(figures.cost_cents()) }, (money(figures.cost_cents(), &currency)))
                        fact(term: "Impressions", attrs: attributes! { data-ads-impressions=(figures.impressions) }, (count(figures.impressions)))
                        fact(term: "Clicks", attrs: attributes! { data-ads-clicks=(figures.clicks) }, (count(figures.clicks)))
                        fact(term: "Click rate", (figures.click_rate_bps().map_or_else(|| "None".to_owned(), |bps| format!("{}.{} %", bps / 100, (bps % 100) / 10))))
                        fact(term: "Cost of a click", (figures.click_cost_cents().map_or_else(|| "None".to_owned(), |cents| money(cents, &currency))))
                        fact(term: "Conversions", attrs: attributes! { data-ads-conversions=(figures.conversions_milli) }, (conversions(figures.conversions_milli)))
                        fact(term: "Conversion value", attrs: attributes! { data-ads-value=(figures.value_cents()) }, (money(figures.value_cents(), &currency)))
                        fact(term: "Return on ad spend", attrs: attributes! { data-ads-return="" }, (ratio(figures.return_hundredths())))
                        fact(term: "Days with numbers", attrs: attributes! { data-ads-days=(days_held) }, (days_held) " of " (span))
                    )
                )

                section_card(
                    title: Some("By campaign"),
                    figures_table(name: "campaigns", head: "Campaign", currency: &currency, rows: &campaigns)
                )
                section_card(
                    title: Some("By device"),
                    figures_table(name: "devices", head: "Device", currency: &currency, rows: &devices)
                )
                section_card(
                    title: Some("By ad group"),
                    figures_table(name: "ad-groups", head: "Ad group", currency: &currency, rows: &ad_groups)
                )
                section_card(
                    title: Some("By category"),
                    description: Some("The products below, added up by the categories the catalog files them under today. A product in two categories counts in both, so these do not add up to the totals; neither do campaigns that show no product."),
                    figures_table(name: "categories", head: "Category", currency: &currency, rows: &categories)
                )
                section_card(
                    title: Some("By product"),
                    description: Some(&products_note),
                    figures_table(name: "products", head: "Product", currency: &currency, rows: &products)
                )
                section_card(
                    title: Some("By search"),
                    description: Some(&terms_note),
                    figures_table(name: "terms", head: "Search", currency: &currency, rows: &terms)
                )
            }
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn figures_read_as_a_person_writes_them() {
        assert_eq!(span_of(Some("7")), 7);
        assert_eq!(span_of(Some("365")), DEFAULT_SPAN);
        assert_eq!(span_of(None), DEFAULT_SPAN);
        assert_eq!(count(12_480), "12\u{202f}480");
        assert_eq!(conversions(3_000), "3");
        assert_eq!(conversions(1_500), "1.5");
        assert_eq!(conversions(1_040), "1");
        assert_eq!(conversions(0), "0");
        assert_eq!(ratio(Some(345)), "3.45");
        assert_eq!(ratio(Some(5)), "0.05");
        assert_eq!(ratio(None), "None");
        assert_eq!(device_label("mobile"), "Phones");
        assert_eq!(device_label("fridge"), "Other");
    }
}
