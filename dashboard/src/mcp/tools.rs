//! The tools of the MCP server: what each is called, what it takes, what
//! it needs of the key, and what it answers.
//!
//! A reading tool needs the permission the page it mirrors asks for. A
//! writing tool needs `proposals:prepare` and the permission to see what
//! it would change, and answers with the proposal it prepared: the change
//! itself waits for a person.

use serde_json::{Value, json};
use timada_catalog::{ProductFilter, ProductStatus};
use timada_core::format_amount;
use timada_core::page::PageFrom;
use timada_google::{
    ACCESS_MERCHANT_CENTER, ACCESS_SEARCH_CONSOLE, AdsLine, KEPT_OUT, TUNED_ADS, ads_account_of,
    ads_by_ad_group, ads_by_campaign, ads_by_product, ads_by_term, ads_totals, day_before,
    last_ads_day, last_search_day, merchant_standing, search_totals, top_pages, top_queries,
};
use timada_proposals::{
    FIELD_BODY, FIELD_DESCRIPTION, FIELD_META_DESCRIPTION, FIELD_META_TITLE, FIELD_NAME,
    FIELD_PRICE, KIND_ADS_EXCLUSION, KIND_CATEGORY, KIND_NEW_CATEGORY, KIND_PRODUCT,
    waiting_proposals,
};
use timada_tenancy::{
    PERMISSION_ORDERS_READ, PERMISSION_PAGES_READ, PERMISSION_PRODUCTS_READ,
    PERMISSION_PROPOSALS_PREPARE,
};

use super::{Caller, Refused};
use crate::proposals::{Draft, Refusal, prepare};

/// How many rows a listing answers with at most.
const LISTED: i64 = 100;
/// How many products, searches and pages a report names.
const TOP: i64 = 50;

/// One tool as a client is told of it.
pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    /// What it needs of the key in the shop it is asked about.
    pub permissions: &'static [&'static str],
    /// Whether it prepares a proposal rather than only reading.
    pub writes: bool,
    /// Its arguments: name, kind, whether it must be given, what it says.
    pub arguments: &'static [(&'static str, &'static str, bool, &'static str)],
}

const SHOP: (&str, &str, bool, &str) = (
    "shop",
    "string",
    true,
    "The shop's slug, as list_shops gives it.",
);
const DAYS: (&str, &str, bool, &str) = (
    "days",
    "integer",
    false,
    "How many days back, 1 to 365. 28 when left out.",
);
const REASON: (&str, &str, bool, &str) = (
    "reason",
    "string",
    true,
    "Why, for the person who reads the proposal: what you read and what you expect of the change.",
);

impl Tool {
    /// The tool as `tools/list` names it.
    pub fn listed(&self) -> Value {
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();
        for (name, kind, needed, says) in self.arguments {
            properties.insert(
                (*name).to_owned(),
                json!({ "type": kind, "description": says }),
            );
            if *needed {
                required.push(*name);
            }
        }
        json!({
            "name": self.name,
            "description": self.description,
            "inputSchema": { "type": "object", "properties": properties, "required": required },
            "annotations": { "readOnlyHint": !self.writes },
        })
    }
}

/// Every tool, the reading ones first.
pub const TOOLS: &[Tool] = &[
    Tool {
        name: "list_shops",
        description: "The shops this key covers, each with what the key may do there.",
        permissions: &[],
        writes: false,
        arguments: &[],
    },
    Tool {
        name: "list_categories",
        description: "A shop's categories: id, name, slug, the category above, how many products each holds, whether it is a draft or kept out of ads.",
        permissions: &[PERMISSION_PRODUCTS_READ],
        writes: false,
        arguments: &[SHOP],
    },
    Tool {
        name: "list_products",
        description: "A shop's products, the most recently changed first, 100 at most: id, name, slug, status, price. Narrow with a search, a category or a status.",
        permissions: &[PERMISSION_PRODUCTS_READ],
        writes: false,
        arguments: &[
            SHOP,
            (
                "search",
                "string",
                false,
                "Matched anywhere in the name or the slug.",
            ),
            (
                "category_id",
                "string",
                false,
                "Only the products of this category.",
            ),
            ("status", "string", false, "draft, published or archived."),
        ],
    },
    Tool {
        name: "get_product",
        description: "One product as the shop holds it: its texts, what search engines show, its price and its variants', whether it is kept out of ads, and for each thing it sells what it leaves and the return on ad spend at which an ad pays for itself, when its cost is known.",
        permissions: &[PERMISSION_PRODUCTS_READ],
        writes: false,
        arguments: &[SHOP, ("product_id", "string", true, "The product's id.")],
    },
    Tool {
        name: "sales_report",
        description: "What a shop sold over the last days, a row a currency: orders, revenue, refunds, profit where every sale says what it left, and what Google Ads cost over the same days.",
        permissions: &[PERMISSION_ORDERS_READ],
        writes: false,
        arguments: &[SHOP, DAYS],
    },
    Tool {
        name: "ads_report",
        description: "What a shop's Google Ads account cost and brought over the last days read: totals, then by campaign, by ad group, by product and by search, each with cost, clicks, conversions and conversion value.",
        permissions: &[PERMISSION_ORDERS_READ],
        writes: false,
        arguments: &[SHOP, DAYS],
    },
    Tool {
        name: "search_console",
        description: "What Search Console says of a shop's site over the last days read: clicks, impressions and average position, then the most clicked queries and pages.",
        permissions: &[PERMISSION_PAGES_READ],
        writes: false,
        arguments: &[SHOP, DAYS],
    },
    Tool {
        name: "merchant_center",
        description: "Where a shop's products stand in Merchant Center as of its last read: how many are accepted, still looked at, refused or not sent, for Shopping ads and for the free listings.",
        permissions: &[PERMISSION_PAGES_READ],
        writes: false,
        arguments: &[SHOP],
    },
    Tool {
        name: "launch_path",
        description: "A shop's launch path: every step between a shop with a name and a shop that is advertised, each done, to do or not checked, with what was found and whether it must hold before ads.",
        permissions: &[PERMISSION_ORDERS_READ],
        writes: false,
        arguments: &[SHOP],
    },
    Tool {
        name: "checkpoints",
        description: "A shop's target and its checkpoints (days 21, 45, 90 and 120 of its first campaign): what each read, what its numbers say, what was decided, and which comes next.",
        permissions: &[PERMISSION_ORDERS_READ],
        writes: false,
        arguments: &[SHOP],
    },
    Tool {
        name: "list_studies",
        description: "The niche studies of the key's user, the latest first: name, country, verdict, keywords kept, search volume and break-even return.",
        permissions: &[],
        writes: false,
        arguments: &[],
    },
    Tool {
        name: "list_proposals",
        description: "The proposals that wait for a person on a shop, 100 at most: what each would change, why, and what prepared it.",
        permissions: &[PERMISSION_PROPOSALS_PREPARE],
        writes: false,
        arguments: &[SHOP],
    },
    Tool {
        name: "propose_product_change",
        description: "Prepares a change to a product for a person to approve: any of its name, description, search engines' title and description, and price. Nothing changes until it is approved. Give only the fields to change.",
        permissions: &[PERMISSION_PROPOSALS_PREPARE, PERMISSION_PRODUCTS_READ],
        writes: true,
        arguments: &[
            SHOP,
            ("product_id", "string", true, "The product's id."),
            ("name", "string", false, "Its new name."),
            ("description", "string", false, "Its new description."),
            (
                "meta_title",
                "string",
                false,
                "The title search engines show.",
            ),
            (
                "meta_description",
                "string",
                false,
                "The description search engines show.",
            ),
            (
                "price",
                "string",
                false,
                "Its new price, as 19.90, in the product's currency. Only for a product sold as is.",
            ),
            REASON,
        ],
    },
    Tool {
        name: "propose_category_change",
        description: "Prepares a change to a category for a person to approve: any of its name, description, the text below its products, and search engines' title and description.",
        permissions: &[PERMISSION_PROPOSALS_PREPARE, PERMISSION_PRODUCTS_READ],
        writes: true,
        arguments: &[
            SHOP,
            ("category_id", "string", true, "The category's id."),
            ("name", "string", false, "Its new name."),
            ("description", "string", false, "Its new description."),
            ("body", "string", false, "The text below its products."),
            (
                "meta_title",
                "string",
                false,
                "The title search engines show.",
            ),
            (
                "meta_description",
                "string",
                false,
                "The description search engines show.",
            ),
            REASON,
        ],
    },
    Tool {
        name: "propose_category",
        description: "Prepares a new category for a person to approve. Once approved it is created as a draft, kept back from shoppers until someone publishes it.",
        permissions: &[PERMISSION_PROPOSALS_PREPARE, PERMISSION_PRODUCTS_READ],
        writes: true,
        arguments: &[
            SHOP,
            ("name", "string", true, "The category's name."),
            (
                "parent_id",
                "string",
                false,
                "The category above it; left out for the top level.",
            ),
            (
                "meta_title",
                "string",
                false,
                "The title search engines show.",
            ),
            REASON,
        ],
    },
    Tool {
        name: "propose_ads_exclusion",
        description: "Prepares keeping a product out of ads for a person to approve. The product stays in the shop; once approved the shopping feed says campaigns leave it out.",
        permissions: &[PERMISSION_PROPOSALS_PREPARE, PERMISSION_ORDERS_READ],
        writes: true,
        arguments: &[
            SHOP,
            ("product_id", "string", true, "The product's id."),
            REASON,
        ],
    },
];

/// The text argument `name`, when it was given.
fn text<'a>(arguments: &'a Value, name: &str) -> Option<&'a str> {
    arguments[name].as_str()
}

/// The text argument `name`, which the tool cannot do without.
fn needed<'a>(arguments: &'a Value, name: &str) -> Result<&'a str, Refused> {
    text(arguments, name)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Refused::Arguments(format!("Give « {name} ».")))
}

/// The days a report looks back over.
fn days(arguments: &Value) -> Result<u32, Refused> {
    match &arguments["days"] {
        Value::Null => Ok(28),
        Value::Number(days) => days
            .as_u64()
            .filter(|days| (1..=365).contains(days))
            .map(|days| days as u32)
            .ok_or_else(|| Refused::Arguments("« days » is 1 to 365.".to_owned())),
        _ => Err(Refused::Arguments("« days » is a number.".to_owned())),
    }
}

/// A count of hundredths as an amount, kept apart from its currency.
fn amount(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    format!("{sign}{}", format_amount(cents.unsigned_abs()))
}

/// What Google Ads counted for one line.
fn ads_figures(line: &AdsLine) -> Value {
    let figures = line.figures();
    json!({
        "cost": amount(figures.cost_cents()),
        "impressions": figures.impressions,
        "clicks": figures.clicks,
        "conversions": figures.conversions_milli as f64 / 1_000.0,
        "conversion_value": amount(figures.value_cents()),
        "return_on_ad_spend": figures.return_hundredths().map(|back| back as f64 / 100.0),
    })
}

/// Prepares `draft` for the caller and answers with the proposal.
async fn proposed(
    caller: &Caller<'_>,
    tenant_id: &str,
    mut draft: Draft,
) -> Result<Value, Refused> {
    // The key, beside the user the proposal says prepared it.
    draft.source = format!("agent {}", caller.key.label)
        .chars()
        .take(timada_proposals::MAX_SOURCE_LEN)
        .collect();
    match prepare(caller.store, tenant_id, caller.user_id, draft, caller.now).await {
        Ok(id) => Ok(json!({
            "proposal_id": id,
            "status": "waiting",
            "note": "Prepared. Nothing changes until a person approves it on the dashboard.",
        })),
        Err(Refusal::Other(err)) => Err(Refused::Fault(err)),
        Err(refusal) => Err(Refused::Tool(format!("Not prepared: {refusal}."))),
    }
}

/// The fields of `named` the arguments give, each with its value.
fn given(arguments: &Value, named: &[&str]) -> Vec<(String, String)> {
    named
        .iter()
        .filter_map(|field| Some(((*field).to_owned(), text(arguments, field)?.to_owned())))
        .collect()
}

/// Answers the tool `name` for `caller`.
pub async fn call(caller: &Caller<'_>, name: &str, arguments: &Value) -> Result<Value, Refused> {
    let Some(tool) = TOOLS.iter().find(|tool| tool.name == name) else {
        return Err(Refused::Arguments("No such tool.".to_owned()));
    };
    let (store, db) = (caller.store, caller.db);
    // The shop asked about, with what the tool needs of the key there.
    let shop = match tool.arguments.first() {
        Some(first) if first.0 == "shop" => Some(
            caller
                .shop(needed(arguments, "shop")?, tool.permissions)
                .await?,
        ),
        _ => None,
    };
    let tenant_id = shop.as_ref().map_or("", |shop| shop.id.as_str());
    Ok(match name {
        "list_shops" => {
            let shops: Vec<Value> = caller
                .shops()
                .await?
                .iter()
                .map(|shop| {
                    let held: Vec<&str> = caller
                        .key
                        .shops
                        .iter()
                        .filter(|covered| covered.tenant_id == shop.id)
                        .flat_map(|covered| &covered.permissions)
                        .map(String::as_str)
                        // What the user no longer holds, the key does not.
                        .filter(|permission| shop.can(caller.user_id, permission))
                        .collect();
                    json!({ "slug": shop.slug, "name": shop.name, "permissions": held })
                })
                .collect();
            json!({ "shops": shops })
        }
        "list_categories" => {
            let categories: Vec<Value> = timada_catalog::categories_of_tenant(db, tenant_id)
                .await?
                .iter()
                .map(|category| {
                    json!({
                        "id": category.id,
                        "name": category.name,
                        "slug": category.slug,
                        "parent_id": category.parent_id,
                        "products": category.product_count,
                        "draft": category.draft,
                        "kept_out_of_ads": category.out_of_ads,
                    })
                })
                .collect();
            json!({ "categories": categories })
        }
        "list_products" => {
            let status = match text(arguments, "status") {
                None => None,
                Some(status) => Some(ProductStatus::parse(status).ok_or_else(|| {
                    Refused::Arguments("« status » is draft, published or archived.".to_owned())
                })?),
            };
            let filter = ProductFilter::default()
                .search(text(arguments, "search"))
                .category(text(arguments, "category_id"))
                .status(status);
            let page =
                timada_catalog::products_of_tenant(db, tenant_id, &filter, PageFrom::First, LISTED)
                    .await?;
            let products: Vec<Value> = page
                .rows
                .iter()
                .map(|product| {
                    json!({
                        "id": product.id,
                        "name": product.name,
                        "slug": product.slug,
                        "status": product.status,
                        "price_from": product.min_amount_cents.map(amount),
                        "price_to": product.max_amount_cents.map(amount),
                        "currency": product.currency,
                        "variants": product.variant_count,
                    })
                })
                .collect();
            json!({ "products": products, "more": page.older.is_some() })
        }
        "get_product" => {
            let product_id = needed(arguments, "product_id")?;
            let catalog = timada_catalog::Command::new(store.clone(), tenant_id);
            let Some(product) = catalog.load_product(product_id).await? else {
                return Err(Refused::Tool("No such product.".to_owned()));
            };
            // What each thing on sale leaves, from the feed's own row.
            let shop = shop.as_ref().expect("a tool that takes a shop");
            let margins = match timada_catalog::feed_product(db, tenant_id, product_id).await? {
                Some(row) => {
                    let costs = timada_catalog::load_sale_costs(store, tenant_id).await?;
                    let vat =
                        crate::margins::home_vat(crate::margins::tax_profile(store, shop).await?);
                    timada_catalog::item_margins(&row, &vat, costs.costs.as_ref())
                        .iter()
                        .map(|margin| match margin {
                            Some(margin) => json!({
                                "margin": amount(margin.margin_cents()),
                                "break_even_return_on_ad_spend": margin
                                    .break_even_roas()
                                    .map(|even| even as f64 / 100.0),
                            }),
                            None => json!({ "margin": null, "note": "Its cost is not known." }),
                        })
                        .collect()
                }
                None => Vec::new(),
            };
            json!({
                "id": product.id,
                "name": product.name,
                "slug": product.slug,
                "status": product.status.as_str(),
                "description": product.description,
                "meta_title": product.meta_title,
                "meta_description": product.meta_description,
                "price": product.price.as_ref().map(|price| format_amount(price.amount_cents)),
                "currency": product.currency(),
                "variants": product.variants.iter().map(|variant| json!({
                    "id": variant.id,
                    "price": format_amount(variant.price.amount_cents),
                })).collect::<Vec<_>>(),
                "kept_out_of_ads": product.out_of_ads,
                // In the order of what it sells: itself, or its variants.
                "margins": margins,
            })
        }
        "sales_report" => {
            let days = days(arguments)?;
            let period = timada_orders::Period {
                from: caller.now.saturating_sub(u64::from(days) * 86_400) as i64,
                to: caller.now as i64 + 1,
            };
            let totals: Vec<Value> = timada_orders::sales_totals(db, tenant_id, period)
                .await?
                .iter()
                .map(|total| {
                    json!({
                        "currency": total.currency,
                        "orders": total.orders,
                        "revenue": amount(total.revenue_cents),
                        "refunded": amount(total.refunded_cents),
                        "items": total.items,
                        // Known only when every order says what it left.
                        "profit": (total.orders > 0 && total.costed_orders == total.orders)
                            .then(|| amount(total.profit_cents)),
                    })
                })
                .collect();
            let last = timada_core::civil::format_date(caller.now);
            let first = day_before(&last, days - 1);
            let ads = match ads_account_of(db, tenant_id).await? {
                Some(customer_id) => {
                    let spent = ads_totals(db, tenant_id, &customer_id, &first, &last).await?;
                    json!({ "currency": spent.currency, "cost": amount(spent.figures().cost_cents()), "days_read": spent.days })
                }
                None => Value::Null,
            };
            json!({ "days": days, "sales": totals, "google_ads": ads })
        }
        "ads_report" => {
            let days = days(arguments)?;
            let Some(customer_id) = ads_account_of(db, tenant_id).await? else {
                return Err(Refused::Tool(
                    "The shop has no Google Ads account chosen.".to_owned(),
                ));
            };
            let Some(last) = last_ads_day(db, tenant_id, &customer_id).await? else {
                return Err(Refused::Tool("Google Ads was not read yet.".to_owned()));
            };
            let first = day_before(&last, days - 1);
            let totals = ads_totals(db, tenant_id, &customer_id, &first, &last).await?;
            let figures = totals.figures();
            let items = timada_catalog::feed_items(db, tenant_id).await?;
            let lines = |lines: Vec<AdsLine>, named: bool| -> Vec<Value> {
                lines
                    .iter()
                    .map(|line| {
                        let mut row = ads_figures(line);
                        row["id"] = json!(line.key);
                        if named {
                            row["name"] = json!(line.name);
                        }
                        row
                    })
                    .collect()
            };
            let products: Vec<Value> =
                ads_by_product(db, tenant_id, &customer_id, &first, &last, TOP)
                    .await?
                    .iter()
                    .map(|line| {
                        let mut row = ads_figures(line);
                        row["item"] = json!(line.key);
                        if let Some((product_id, name)) = items.product_of(&line.key) {
                            row["product_id"] = json!(product_id);
                            row["name"] = json!(name);
                        }
                        row
                    })
                    .collect();
            let terms: Vec<Value> = ads_by_term(db, tenant_id, &customer_id, &first, &last, TOP)
                .await?
                .iter()
                .map(|line| {
                    let mut row = ads_figures(line);
                    row["search"] = json!(line.key);
                    row
                })
                .collect();
            json!({
                "first_day": first,
                "last_day": last,
                "currency": totals.currency,
                "totals": {
                    "cost": amount(figures.cost_cents()),
                    "impressions": figures.impressions,
                    "clicks": figures.clicks,
                    "conversions": figures.conversions_milli as f64 / 1_000.0,
                    "conversion_value": amount(figures.value_cents()),
                    "return_on_ad_spend": figures.return_hundredths().map(|back| back as f64 / 100.0),
                },
                "campaigns": lines(ads_by_campaign(db, tenant_id, &customer_id, &first, &last).await?, true),
                "ad_groups": lines(ads_by_ad_group(db, tenant_id, &customer_id, &first, &last).await?, true),
                "products": products,
                "searches": terms,
            })
        }
        "search_console" => {
            let days = days(arguments)?;
            let site = timada_google::Command(store.clone())
                .load_connection(tenant_id)
                .await?
                .filter(|connection| connection.covers(ACCESS_SEARCH_CONSOLE))
                .and_then(|connection| connection.site);
            let Some(site) = site else {
                return Err(Refused::Tool(
                    "Search Console is not connected for the shop.".to_owned(),
                ));
            };
            let Some(last) = last_search_day(db, tenant_id, &site).await? else {
                return Err(Refused::Tool("Search Console was not read yet.".to_owned()));
            };
            let first = day_before(&last, days - 1);
            let totals = search_totals(db, tenant_id, &site, &first, &last).await?;
            let row = |key: &str, line: &timada_google::SearchLine| {
                json!({
                    key: line.key,
                    "clicks": line.clicks,
                    "impressions": line.impressions,
                    "position": line.figures().position_tenths().map(|tenths| tenths as f64 / 10.0),
                })
            };
            let queries: Vec<Value> = top_queries(db, tenant_id, &site, &first, &last, TOP)
                .await?
                .iter()
                .map(|line| row("query", line))
                .collect();
            let pages: Vec<Value> = top_pages(db, tenant_id, &site, &first, &last, TOP)
                .await?
                .iter()
                .map(|line| row("page", line))
                .collect();
            json!({
                "site": site,
                "first_day": first,
                "last_day": last,
                "clicks": totals.clicks,
                "impressions": totals.impressions,
                "position": totals.figures().position_tenths().map(|tenths| tenths as f64 / 10.0),
                "queries": queries,
                "pages": pages,
            })
        }
        "merchant_center" => {
            let account = timada_google::Command(store.clone())
                .load_connection(tenant_id)
                .await?
                .filter(|connection| connection.covers(ACCESS_MERCHANT_CENTER))
                .and_then(|connection| connection.merchant);
            let Some(account) = account else {
                return Err(Refused::Tool(
                    "Merchant Center is not connected for the shop.".to_owned(),
                ));
            };
            let Some(standing) = merchant_standing(db, tenant_id, &account.account_id).await?
            else {
                return Err(Refused::Tool(
                    "Merchant Center was not read yet.".to_owned(),
                ));
            };
            let tally = |tally: timada_google::MerchantTally| {
                json!({
                    "accepted": tally.accepted,
                    "pending": tally.pending,
                    "refused": tally.refused,
                    "not_sent": tally.absent,
                })
            };
            json!({
                "account": account.account_id,
                "read_at": standing.read_at,
                "shopping_ads": tally(standing.ads),
                "free_listings": tally(standing.listings),
                "every_product_accepted_for_ads": standing.all_accepted_for_ads(),
            })
        }
        "launch_path" => {
            let shop = shop.as_ref().expect("a tool that takes a shop");
            let steps: Vec<Value> = crate::launch_path::path(store, db, shop)
                .await?
                .iter()
                .map(|step| {
                    json!({
                        "step": step.key,
                        "part": step.group,
                        "label": step.label,
                        "state": step.state.key(),
                        "found": step.detail,
                        "before_ads": step.before_ads,
                        "a_member_confirms": step.confirms,
                    })
                })
                .collect();
            json!({ "steps": steps })
        }
        "checkpoints" => {
            let launch = timada_launch::Command::new(store.clone(), tenant_id)
                .load_launch()
                .await?;
            let standing = crate::checkpoints::standing(store, db, tenant_id, caller.now).await?;
            let target = launch
                .as_ref()
                .and_then(|launch| launch.target())
                .map(|target| {
                    json!({
                        "monthly_profit_after_ads": format_amount(target.monthly_profit_cents),
                        "currency": target.currency,
                        "by": target.by_day,
                    })
                });
            let reached: Vec<Value> = launch
                .iter()
                .flat_map(|launch| &launch.reached)
                .map(|reached| {
                    json!({
                        "day": reached.checkpoint,
                        "read_at": reached.reached_at,
                        "numbers": reached.figures.iter().map(|(name, value)| json!({ "name": name, "value": value })).collect::<Vec<_>>(),
                        "says": reached.advice,
                        "why": reached.why,
                        "decided": reached.decision.as_ref().map(|decision| json!({ "decision": decision.decision, "note": decision.note })),
                    })
                })
                .collect();
            json!({
                "target": target,
                "first_campaign_live_since": standing.started_at,
                "next": standing.ahead.map(|(day, at)| json!({ "day": day, "at": at })),
                "last_thirty_days_profit_after_ads": standing.month_cents.map(amount),
                "currency": standing.currency,
                "read": reached,
            })
        }
        "list_studies" => {
            let studies: Vec<Value> = timada_research::studies_of_user(db, caller.user_id, TOP)
                .await?
                .iter()
                .map(|study| {
                    json!({
                        "id": study.id,
                        "name": study.name,
                        "country": study.country,
                        "language": study.language,
                        "verdict": study.verdict,
                        "keywords_scored": study.scored_keywords,
                        "searches_a_month": study.kept_volume,
                        "break_even_return_on_ad_spend": study.break_even_roas.map(|even| even as f64 / 100.0),
                    })
                })
                .collect();
            json!({ "studies": studies })
        }
        "list_proposals" => {
            let waiting = waiting_proposals(db, tenant_id, PageFrom::First, LISTED).await?;
            let proposals: Vec<Value> = waiting
                .rows
                .iter()
                .map(|row| {
                    json!({
                        "id": row.id,
                        "kind": row.kind,
                        "about": row.subject_name,
                        "subject_id": row.subject_id,
                        "prepared_by": row.source,
                        "reason": row.reason,
                        "changes": row.changes.0.iter().map(|change| json!({
                            "field": change.field,
                            "from": change.before,
                            "to": change.after,
                        })).collect::<Vec<_>>(),
                    })
                })
                .collect();
            json!({ "waiting": proposals })
        }
        "propose_product_change" | "propose_category_change" => {
            let product = name == "propose_product_change";
            let (kind, id, fields): (&str, &str, &[&str]) = if product {
                (
                    KIND_PRODUCT,
                    "product_id",
                    &[
                        FIELD_NAME,
                        FIELD_DESCRIPTION,
                        FIELD_META_TITLE,
                        FIELD_META_DESCRIPTION,
                        FIELD_PRICE,
                    ],
                )
            } else {
                (
                    KIND_CATEGORY,
                    "category_id",
                    &[
                        FIELD_NAME,
                        FIELD_DESCRIPTION,
                        FIELD_BODY,
                        FIELD_META_TITLE,
                        FIELD_META_DESCRIPTION,
                    ],
                )
            };
            let values = given(arguments, fields);
            if values.is_empty() {
                return Err(Refused::Arguments(
                    "Give at least one field to change.".to_owned(),
                ));
            }
            let draft = Draft {
                kind: kind.to_owned(),
                subject_id: needed(arguments, id)?.to_owned(),
                values,
                reason: needed(arguments, "reason")?.to_owned(),
                ..Draft::default()
            };
            proposed(caller, tenant_id, draft).await?
        }
        "propose_category" => {
            let mut values = vec![(FIELD_NAME.to_owned(), needed(arguments, "name")?.to_owned())];
            values.extend(given(arguments, &[FIELD_META_TITLE]));
            let draft = Draft {
                kind: KIND_NEW_CATEGORY.to_owned(),
                subject_id: text(arguments, "parent_id").unwrap_or_default().to_owned(),
                values,
                reason: needed(arguments, "reason")?.to_owned(),
                ..Draft::default()
            };
            proposed(caller, tenant_id, draft).await?
        }
        "propose_ads_exclusion" => {
            let draft = Draft {
                kind: KIND_ADS_EXCLUSION.to_owned(),
                subject_id: needed(arguments, "product_id")?.to_owned(),
                values: vec![(TUNED_ADS.to_owned(), KEPT_OUT.to_owned())],
                reason: needed(arguments, "reason")?.to_owned(),
                ..Draft::default()
            };
            proposed(caller, tenant_id, draft).await?
        }
        _ => return Err(Refused::Arguments("No such tool.".to_owned())),
    })
}
