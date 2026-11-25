//! What is read from Google Ads under a shop's access.
//!
//! Google Ads answers questions written in its own query language, one
//! report at a time. Each is asked as a stream (`googleAds:searchStream`):
//! the whole answer comes in one request, which Google counts as one
//! operation of the Cloud project's daily allowance however many rows it
//! carries, where a paged answer costs a request a page. The access level
//! that sets the allowance belongs to the Cloud project of the OAuth
//! client: no developer token is sent (Google stopped reading it on
//! 2026-09-09).
//!
//! An account is reached by itself, or through a manager account the grant
//! reaches, which is then named in `login-customer-id`.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{Value, json};
use timada_google::aggregator::{
    AdsAdGroupRow, AdsCampaignRow, AdsNumbers, AdsProductRow, AdsTermRow,
};
use timada_google::{AdsAccount, AdsDayReport, CAMPAIGN_SHOPPING, CampaignPlan, geo_target_of};

use super::{Google, GoogleFault, reason, unavailable};

/// How many of the accounts a grant reaches are asked about: each is a
/// question.
const ACCOUNTS_ASKED: usize = 20;

/// The figures every report asks for.
const METRICS: &str = "metrics.cost_micros, metrics.impressions, metrics.clicks, \
     metrics.conversions, metrics.conversions_value";

/// What a period of an account gave, each row with its day.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdsRows {
    /// The account's currency, as the campaigns' report names it.
    pub currency: Option<String>,
    pub campaigns: Vec<(String, AdsCampaignRow)>,
    pub ad_groups: Vec<(String, AdsAdGroupRow)>,
    pub products: Vec<(String, AdsProductRow)>,
    pub terms: Vec<(String, AdsTermRow)>,
}

/// One of the four reports a pass asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdsRead {
    Campaigns,
    AdGroups,
    Products,
    Terms,
}

impl AdsRead {
    pub const ALL: [Self; 4] = [Self::Campaigns, Self::AdGroups, Self::Products, Self::Terms];

    /// The question, over the days from `first` to `last` of the account's
    /// time zone. Google leaves out what was not shown at all.
    fn query(self, first: &str, last: &str) -> String {
        let days = format!("segments.date BETWEEN '{first}' AND '{last}'");
        match self {
            Self::Campaigns => format!(
                "SELECT campaign.id, campaign.name, customer.currency_code, segments.date, \
                 segments.device, {METRICS} FROM campaign WHERE {days}"
            ),
            Self::AdGroups => format!(
                "SELECT campaign.id, ad_group.id, ad_group.name, segments.date, {METRICS} \
                 FROM ad_group WHERE {days}"
            ),
            Self::Products => format!(
                "SELECT campaign.id, ad_group.id, segments.product_item_id, segments.date, \
                 {METRICS} FROM shopping_performance_view WHERE {days}"
            ),
            // A search nobody clicked cost nothing and says nothing.
            Self::Terms => format!(
                "SELECT search_term_view.search_term, campaign.id, ad_group.id, segments.date, \
                 {METRICS} FROM search_term_view WHERE {days} AND metrics.clicks > 0"
            ),
        }
    }
}

/// One batch of a streamed answer.
#[derive(Deserialize)]
struct Batch {
    #[serde(default)]
    results: Vec<Value>,
}

#[derive(Deserialize)]
struct Accessible {
    #[serde(default, rename = "resourceNames")]
    resource_names: Vec<String>,
}

/// The text at `path` in a row; Google writes its ids and counts as text,
/// and a stand-in may write them as numbers.
fn text(row: &Value, path: &[&str]) -> String {
    let mut at = row;
    for key in path {
        at = &at[*key];
    }
    match at {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        _ => String::new(),
    }
}

/// The count at `path` in a row: written as text or as a number.
fn count(row: &Value, path: &[&str]) -> u64 {
    let mut at = row;
    for key in path {
        at = &at[*key];
    }
    match at {
        Value::String(text) => text.parse().unwrap_or(0),
        // `as` saturates, and takes what is no number for zero.
        Value::Number(number) => number
            .as_u64()
            .unwrap_or_else(|| number.as_f64().unwrap_or(0.0).round() as u64),
        _ => 0,
    }
}

/// The decimal at `path` in a row, times `scale`, rounded.
fn scaled(row: &Value, path: &[&str], scale: f64) -> u64 {
    let mut at = row;
    for key in path {
        at = &at[*key];
    }
    let value = match at {
        Value::String(text) => text.parse().unwrap_or(0.0),
        Value::Number(number) => number.as_f64().unwrap_or(0.0),
        _ => 0.0,
    };
    (value * scale).round() as u64
}

fn numbers(row: &Value) -> AdsNumbers {
    AdsNumbers {
        cost_micros: count(row, &["metrics", "costMicros"]),
        impressions: count(row, &["metrics", "impressions"]),
        clicks: count(row, &["metrics", "clicks"]),
        conversions_milli: scaled(row, &["metrics", "conversions"], 1_000.0),
        value_micros: scaled(row, &["metrics", "conversionsValue"], 1_000_000.0),
    }
}

/// A kind of device as it is kept.
fn device(named: &str) -> &'static str {
    match named {
        "MOBILE" => "mobile",
        "DESKTOP" => "desktop",
        "TABLET" => "tablet",
        "CONNECTED_TV" => "tv",
        _ => "other",
    }
}

/// Opens what Google Ads answered a question with. A refusal names what is
/// wrong in a list of errors, each with a code of its own kind.
fn open_ads(status: u16, body: &[u8]) -> Result<Vec<Value>, GoogleFault> {
    if status == 200 {
        // A stream is a list of batches; a stand-in may answer with one.
        let batches: Vec<Batch> = match serde_json::from_slice::<Value>(body) {
            Ok(Value::Array(batches)) => batches
                .into_iter()
                .filter_map(|batch| serde_json::from_value(batch).ok())
                .collect(),
            Ok(batch @ Value::Object(_)) => serde_json::from_value(batch).into_iter().collect(),
            Ok(_) => Vec::new(),
            Err(err) => {
                return Err(GoogleFault::Unavailable(format!(
                    "unreadable answer: {err}"
                )));
            }
        };
        return Ok(batches
            .into_iter()
            .flat_map(|batch| batch.results)
            .collect());
    }
    Err(ads_refusal(status, body))
}

/// Why Google Ads refused, in its words.
fn ads_refusal(status: u16, body: &[u8]) -> GoogleFault {
    let said: Value = serde_json::from_slice(body).unwrap_or_default();
    // A refused stream wraps its error in a list.
    let said = match &said {
        Value::Array(all) => all.first().cloned().unwrap_or_default(),
        _ => said,
    };
    let error = match &said["error"] {
        Value::Null => &said,
        error => error,
    };
    let detail = error["details"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|detail| detail["errors"].as_array().into_iter().flatten())
        .next();
    // `{"quotaError": "RESOURCE_EXHAUSTED"}`: the kind, then which.
    let code = detail
        .and_then(|detail| detail["errorCode"].as_object())
        .and_then(|code| code.iter().next())
        .map(|(kind, which)| (kind.as_str(), which.as_str().unwrap_or_default()));
    let message = detail
        .and_then(|detail| detail["message"].as_str())
        .or(error["message"].as_str())
        .filter(|message| !message.is_empty())
        .map_or_else(|| format!("HTTP {status}"), str::to_owned);
    let message = match code {
        Some((_, which)) if !which.is_empty() => format!("{message} ({which})"),
        _ => message,
    };
    match (status, code) {
        // The access is turned down: [`Google::read`] asks for another.
        (401, _) => GoogleFault::ConnectAgain,
        // The allowance is used up, for the day or for the moment.
        (_, Some(("quotaError", _))) | (429 | 500.., _) => {
            GoogleFault::Unavailable(reason(&message))
        }
        _ => GoogleFault::Refused(reason(&message)),
    }
}

impl Google {
    /// Asks Google Ads one question about `customer_id`, reached through
    /// `login_customer_id` when it is a manager's client; the rows of the
    /// whole answer.
    async fn ads_search(
        &self,
        access_token: &str,
        customer_id: &str,
        login_customer_id: Option<&str>,
        query: &str,
    ) -> Result<Vec<Value>, GoogleFault> {
        let inner = self.inner()?;
        let url = format!(
            "{}/customers/{customer_id}/googleAds:searchStream",
            inner.ads_url
        );
        let mut request = inner
            .client
            .post(url)
            .bearer_auth(access_token)
            .json(&json!({ "query": query }));
        if let Some(login) = login_customer_id {
            request = request.header("login-customer-id", login);
        }
        let response = request.send().await.map_err(unavailable)?;
        let status = response.status().as_u16();
        let body = response.bytes().await.map_err(unavailable)?;
        open_ads(status, &body)
    }

    /// The Google Ads accounts the grant reaches and ads can run in, by
    /// name: the ones it reaches by itself, and the clients of the manager
    /// accounts it reaches. An account that cannot be asked about (closed,
    /// or not open to the platform's access level) is left out. One
    /// question for the list, then one an account.
    pub async fn ads_accounts(&self, access_token: &str) -> Result<Vec<AdsAccount>, GoogleFault> {
        let inner = self.inner()?;
        let url = format!("{}/customers:listAccessibleCustomers", inner.ads_url);
        let url = reqwest::Url::parse(&url).map_err(|err| GoogleFault::Refused(err.to_string()))?;
        let response = inner
            .client
            .get(url)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(unavailable)?;
        let status = response.status().as_u16();
        let body = response.bytes().await.map_err(unavailable)?;
        if status != 200 {
            return Err(ads_refusal(status, &body));
        }
        let reached: Accessible = serde_json::from_slice(&body)
            .map_err(|err| GoogleFault::Unavailable(format!("unreadable answer: {err}")))?;
        let mut found: BTreeMap<String, AdsAccount> = BTreeMap::new();
        for name in reached.resource_names.iter().take(ACCOUNTS_ASKED) {
            let Some(reached_id) = name.strip_prefix("customers/") else {
                continue;
            };
            // The account itself (level 0) and, for a manager, its clients.
            let rows = self
                .ads_search(
                    access_token,
                    reached_id,
                    None,
                    "SELECT customer_client.id, customer_client.level, customer_client.manager, \
                     customer_client.descriptive_name, customer_client.currency_code, \
                     customer_client.time_zone FROM customer_client \
                     WHERE customer_client.level <= 1 AND customer_client.status = 'ENABLED'",
                )
                .await;
            let rows = match rows {
                Ok(rows) => rows,
                Err(GoogleFault::Refused(why)) => {
                    tracing::debug!(account = %reached_id, %why, "a Google Ads account is not read");
                    continue;
                }
                Err(fault) => return Err(fault),
            };
            for row in &rows {
                let client = &row["customerClient"];
                if client["manager"].as_bool().unwrap_or(false) {
                    continue;
                }
                let customer_id = text(client, &["id"]);
                let direct = count(client, &["level"]) == 0;
                let account = AdsAccount {
                    login_customer_id: (!direct).then(|| reached_id.to_owned()),
                    name: text(client, &["descriptiveName"]),
                    currency: text(client, &["currencyCode"]),
                    time_zone: text(client, &["timeZone"]),
                    customer_id: customer_id.clone(),
                };
                // Reached by itself rather than through a manager, when both.
                if direct || !found.contains_key(&customer_id) {
                    found.insert(customer_id, account);
                }
            }
        }
        let mut accounts: Vec<AdsAccount> = found
            .into_values()
            .filter(|account| !account.customer_id.is_empty())
            .collect();
        accounts.sort_by(|a, b| (&a.name, &a.customer_id).cmp(&(&b.name, &b.customer_id)));
        Ok(accounts)
    }

    /// One report of `account` over the days from `first` to `last`
    /// (`2026-11-20`, both included, days of the account's time zone), put
    /// into `rows`. One question, however many days.
    pub async fn ads_report(
        &self,
        access_token: &str,
        account: &AdsAccount,
        what: AdsRead,
        first: &str,
        last: &str,
        rows: &mut AdsRows,
    ) -> Result<(), GoogleFault> {
        let answer = self
            .ads_search(
                access_token,
                &account.customer_id,
                account.login_customer_id.as_deref(),
                &what.query(first, last),
            )
            .await?;
        for row in &answer {
            let day = text(row, &["segments", "date"]);
            let campaign_id = text(row, &["campaign", "id"]);
            let ad_group_id = text(row, &["adGroup", "id"]);
            let numbers = numbers(row);
            match what {
                AdsRead::Campaigns => {
                    let currency = text(row, &["customer", "currencyCode"]);
                    if rows.currency.is_none() && !currency.is_empty() {
                        rows.currency = Some(currency);
                    }
                    rows.campaigns.push((
                        day,
                        AdsCampaignRow {
                            campaign_id,
                            name: text(row, &["campaign", "name"]),
                            device: device(&text(row, &["segments", "device"])).to_owned(),
                            numbers,
                        },
                    ));
                }
                AdsRead::AdGroups => rows.ad_groups.push((
                    day,
                    AdsAdGroupRow {
                        campaign_id,
                        ad_group_id,
                        name: text(row, &["adGroup", "name"]),
                        numbers,
                    },
                )),
                AdsRead::Products => rows.products.push((
                    day,
                    AdsProductRow {
                        item_id: text(row, &["segments", "productItemId"]),
                        campaign_id,
                        ad_group_id,
                        numbers,
                    },
                )),
                AdsRead::Terms => rows.terms.push((
                    day,
                    AdsTermRow {
                        term: text(row, &["searchTermView", "searchTerm"]),
                        campaign_id,
                        ad_group_id,
                        numbers,
                    },
                )),
            }
        }
        Ok(())
    }
}

/// What Google Ads called a campaign it just took, and its ad groups in
/// the order they were asked for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CreatedIds {
    pub campaign_id: String,
    pub ad_group_ids: Vec<String>,
}

/// A campaign found in an account by its name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FoundCampaign {
    pub campaign_id: String,
    /// Whether Google Ads has it running (`ENABLED`) rather than paused.
    pub enabled: bool,
    /// Its ad groups: Google Ads' number, then the name.
    pub ad_groups: Vec<(String, String)>,
}

/// What a campaign needs beside its plan.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CampaignContext {
    /// The Merchant Center account whose products a Shopping campaign
    /// shows.
    pub merchant_id: String,
    /// Which of the feed's custom labels says a product's category (0 to
    /// 4).
    pub label: u8,
    /// Where the ad of a campaign on the shop's name leads.
    pub final_url: String,
}

/// The last part of a resource's name: Google Ads' number for it.
fn number_of(resource_name: &str) -> String {
    resource_name
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// A text inside the quotes of a question.
fn quoted(text: &str) -> String {
    text.replace('\\', "\\\\").replace('\'', "\\'")
}

/// What a device's clicks are paid against the bid, from hundredths more
/// or less: -55 is 0.45.
fn modifier(percent: i32) -> f64 {
    f64::from(100 + percent) / 100.0
}

/// The operations that create the campaign `plan` describes in the account
/// `customer_id`, to send together: Google Ads takes them all or none.
/// What one creates is named by a number below zero until Google Ads gives
/// it its own, so the ones after can refer to it.
pub fn campaign_operations(
    customer_id: &str,
    plan: &CampaignPlan,
    context: &CampaignContext,
) -> Vec<Value> {
    let of = |kind: &str, id: &str| format!("customers/{customer_id}/{kind}/{id}");
    let mut next = 0i64;
    let mut temp = || {
        next -= 1;
        next.to_string()
    };
    let budget = of("campaignBudgets", &temp());
    let campaign = of("campaigns", &temp());
    let shopping = plan.kind == CAMPAIGN_SHOPPING;
    let mut operations = vec![json!({ "campaignBudgetOperation": { "create": {
        "resourceName": budget,
        "amountMicros": plan.daily_budget_micros.to_string(),
        "deliveryMethod": "STANDARD",
        // Its own: a budget is shared between campaigns unless told not to.
        "explicitlyShared": false,
    } } })];
    let mut created = json!({
        "resourceName": campaign,
        "name": plan.name,
        "advertisingChannelType": if shopping { "SHOPPING" } else { "SEARCH" },
        "status": if plan.live { "ENABLED" } else { "PAUSED" },
        "manualCpc": {},
        "campaignBudget": budget,
        "containsEuPoliticalAdvertising": "DOES_NOT_CONTAIN_EU_POLITICAL_ADVERTISING",
    });
    if shopping {
        created["shoppingSetting"] = json!({
            "merchantId": context.merchant_id,
            "campaignPriority": 0,
            "enableLocal": false,
        });
    } else {
        // Google's own results only: the name of a shop is searched there.
        created["networkSettings"] = json!({
            "targetGoogleSearch": true,
            "targetSearchNetwork": false,
            "targetContentNetwork": false,
            "targetPartnerSearchNetwork": false,
        });
    }
    operations.push(json!({ "campaignOperation": { "create": created } }));
    for country in &plan.countries {
        if let Some(number) = geo_target_of(country) {
            operations.push(json!({ "campaignCriterionOperation": { "create": {
                "campaign": campaign,
                "location": { "geoTargetConstant": format!("geoTargetConstants/{number}") },
            } } }));
        }
    }
    for (device, percent) in [
        ("MOBILE", plan.phone_percent),
        ("TABLET", plan.tablet_percent),
    ] {
        if let Some(percent) = percent {
            operations.push(json!({ "campaignCriterionOperation": { "create": {
                "campaign": campaign,
                "device": { "type": device },
                "bidModifier": modifier(percent),
            } } }));
        }
    }
    let bid = plan.bid_micros.to_string();
    if shopping {
        let index = format!("INDEX{}", context.label);
        for group in &plan.ad_groups {
            let group_id = temp();
            let ad_group = of("adGroups", &group_id);
            operations.push(json!({ "adGroupOperation": { "create": {
                "resourceName": ad_group,
                "name": group.name,
                "campaign": campaign,
                "status": "ENABLED",
                "type": "SHOPPING_PRODUCT_ADS",
                "cpcBidMicros": bid,
            } } }));
            operations.push(json!({ "adGroupAdOperation": { "create": {
                "adGroup": ad_group,
                "status": "ENABLED",
                "ad": { "shoppingProductAd": {} },
            } } }));
            // The products shown: the ones whose label says this category,
            // and none of the others. A tree goes up whole, its root first.
            let root = of("adGroupCriteria", &format!("{group_id}~{}", temp()));
            operations.push(json!({ "adGroupCriterionOperation": { "create": {
                "resourceName": root,
                "adGroup": ad_group,
                "status": "ENABLED",
                "listingGroup": { "type": "SUBDIVISION" },
            } } }));
            operations.push(json!({ "adGroupCriterionOperation": { "create": {
                "adGroup": ad_group,
                "status": "ENABLED",
                "cpcBidMicros": bid,
                "listingGroup": {
                    "type": "UNIT",
                    "parentAdGroupCriterion": root,
                    "caseValue": { "productCustomAttribute": {
                        "index": index,
                        "value": timada_catalog::ads_category_label(&group.category_id),
                    } },
                },
            } } }));
            operations.push(json!({ "adGroupCriterionOperation": { "create": {
                "adGroup": ad_group,
                "status": "ENABLED",
                "negative": true,
                "listingGroup": {
                    "type": "UNIT",
                    "parentAdGroupCriterion": root,
                    "caseValue": { "productCustomAttribute": { "index": index } },
                },
            } } }));
        }
    } else {
        let ad_group = of("adGroups", &temp());
        operations.push(json!({ "adGroupOperation": { "create": {
            "resourceName": ad_group,
            "name": plan.name,
            "campaign": campaign,
            "status": "ENABLED",
            "type": "SEARCH_STANDARD",
            "cpcBidMicros": bid,
        } } }));
        for keyword in &plan.keywords {
            operations.push(json!({ "adGroupCriterionOperation": { "create": {
                "adGroup": ad_group,
                "status": "ENABLED",
                // The words in this order, whatever comes around them.
                "keyword": { "text": keyword, "matchType": "PHRASE" },
            } } }));
        }
        let texts = |lines: &[String]| -> Vec<Value> {
            lines.iter().map(|text| json!({ "text": text })).collect()
        };
        operations.push(json!({ "adGroupAdOperation": { "create": {
            "adGroup": ad_group,
            "status": "ENABLED",
            "ad": {
                "finalUrls": [context.final_url],
                "responsiveSearchAd": {
                    "headlines": texts(&plan.headlines),
                    "descriptions": texts(&plan.descriptions),
                },
            },
        } } }));
    }
    operations
}

#[derive(Deserialize)]
struct Mutated {
    #[serde(default, rename = "mutateOperationResponses")]
    responses: Vec<Value>,
}

impl Google {
    /// Sends `operations` to the account together: all are made, or none.
    /// What Google Ads answers each with, in their order.
    async fn ads_mutate(
        &self,
        access_token: &str,
        account: &AdsAccount,
        operations: Vec<Value>,
    ) -> Result<Vec<Value>, GoogleFault> {
        let inner = self.inner()?;
        let url = format!(
            "{}/customers/{}/googleAds:mutate",
            inner.ads_url, account.customer_id
        );
        let mut request = inner
            .client
            .post(url)
            .bearer_auth(access_token)
            .json(&json!({ "mutateOperations": operations }));
        if let Some(login) = account.login_customer_id.as_deref() {
            request = request.header("login-customer-id", login);
        }
        let response = request.send().await.map_err(unavailable)?;
        let status = response.status().as_u16();
        let body = response.bytes().await.map_err(unavailable)?;
        if status != 200 {
            return Err(ads_refusal(status, &body));
        }
        let mutated: Mutated = serde_json::from_slice(&body)
            .map_err(|err| GoogleFault::Unavailable(format!("unreadable answer: {err}")))?;
        Ok(mutated.responses)
    }

    /// Creates the campaign `plan` describes in `account`, whole or not at
    /// all. As many operations of the platform's allowance as the campaign
    /// has parts.
    pub async fn ads_create_campaign(
        &self,
        access_token: &str,
        account: &AdsAccount,
        plan: &CampaignPlan,
        context: &CampaignContext,
    ) -> Result<CreatedIds, GoogleFault> {
        let operations = campaign_operations(&account.customer_id, plan, context);
        let answers = self.ads_mutate(access_token, account, operations).await?;
        let mut created = CreatedIds::default();
        for answer in &answers {
            let campaign = text(answer, &["campaignResult", "resourceName"]);
            if !campaign.is_empty() {
                created.campaign_id = number_of(&campaign);
            }
            let ad_group = text(answer, &["adGroupResult", "resourceName"]);
            if !ad_group.is_empty() {
                created.ad_group_ids.push(number_of(&ad_group));
            }
        }
        let groups = if plan.kind == CAMPAIGN_SHOPPING {
            plan.ad_groups.len()
        } else {
            1
        };
        if created.campaign_id.is_empty() || created.ad_group_ids.len() != groups {
            return Err(GoogleFault::Unavailable(
                "an answer that does not name what was created".into(),
            ));
        }
        Ok(created)
    }

    /// The campaign called `name` in `account`, with its ad groups, when
    /// there is one that was not removed. One question.
    pub async fn ads_find_campaign(
        &self,
        access_token: &str,
        account: &AdsAccount,
        name: &str,
    ) -> Result<Option<FoundCampaign>, GoogleFault> {
        let rows = self
            .ads_search(
                access_token,
                &account.customer_id,
                account.login_customer_id.as_deref(),
                &format!(
                    "SELECT campaign.id, campaign.status, ad_group.id, ad_group.name \
                     FROM ad_group WHERE campaign.name = '{}' \
                     AND campaign.status != 'REMOVED' AND ad_group.status != 'REMOVED'",
                    quoted(name)
                ),
            )
            .await?;
        let Some(first) = rows.first() else {
            return Ok(None);
        };
        let campaign_id = text(first, &["campaign", "id"]);
        Ok(Some(FoundCampaign {
            enabled: text(first, &["campaign", "status"]) == "ENABLED",
            ad_groups: rows
                .iter()
                .filter(|row| text(row, &["campaign", "id"]) == campaign_id)
                .map(|row| {
                    (
                        text(row, &["adGroup", "id"]),
                        text(row, &["adGroup", "name"]),
                    )
                })
                .collect(),
            campaign_id,
        }))
    }

    /// Sets the campaign `campaign_id` of `account` running. Asked of one
    /// that runs already, Google Ads changes nothing.
    pub async fn ads_enable_campaign(
        &self,
        access_token: &str,
        account: &AdsAccount,
        campaign_id: &str,
    ) -> Result<(), GoogleFault> {
        let operation = json!({ "campaignOperation": {
            "updateMask": "status",
            "update": {
                "resourceName": format!(
                    "customers/{}/campaigns/{campaign_id}",
                    account.customer_id
                ),
                "status": "ENABLED",
            },
        } });
        self.ads_mutate(access_token, account, vec![operation])
            .await?;
        Ok(())
    }
}

/// The days of a period, each with its rows, the oldest first: every day
/// one of the four reports has a row for. `currency` is the account's when
/// the campaigns' report did not name it.
pub fn assemble_ads_days(rows: AdsRows, currency: &str) -> Vec<AdsDayReport> {
    let currency = rows.currency.unwrap_or_else(|| currency.to_owned());
    let mut days: BTreeMap<String, AdsDayReport> = BTreeMap::new();
    fn of<'a>(
        days: &'a mut BTreeMap<String, AdsDayReport>,
        day: String,
        currency: &str,
    ) -> &'a mut AdsDayReport {
        days.entry(day.clone()).or_insert_with(|| AdsDayReport {
            day,
            currency: currency.to_owned(),
            ..AdsDayReport::default()
        })
    }
    for (day, row) in rows.campaigns {
        of(&mut days, day, &currency).campaigns.push(row);
    }
    for (day, row) in rows.ad_groups {
        of(&mut days, day, &currency).ad_groups.push(row);
    }
    for (day, row) in rows.products {
        of(&mut days, day, &currency).products.push(row);
    }
    for (day, row) in rows.terms {
        of(&mut days, day, &currency).terms.push(row);
    }
    days.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::google::tests::StandIn;

    fn account(login: Option<&str>) -> AdsAccount {
        AdsAccount {
            customer_id: "1234567890".into(),
            login_customer_id: login.map(str::to_owned),
            name: "Acme".into(),
            currency: "EUR".into(),
            time_zone: "Europe/Paris".into(),
        }
    }

    #[tokio::test]
    async fn the_accounts_a_grant_reaches_are_listed_with_the_managers_clients() {
        let stand_in = StandIn::start(|request| {
            if request.line.starts_with("GET /ads/v26/customers:listAccessibleCustomers") {
                return (
                    200,
                    r#"{"resourceNames":["customers/1111111111","customers/2222222222","customers/3333333333"]}"#.into(),
                );
            }
            assert!(request.body.contains("FROM customer_client"), "{}", request.body);
            let rows = if request.line.contains("/customers/1111111111/") {
                // An account of its own.
                r#"[{"results":[{"customerClient":{"id":"1111111111","level":"0","manager":false,"descriptiveName":"Zèbre","currencyCode":"EUR","timeZone":"Europe/Paris"}}],"fieldMask":"x"}]"#
            } else if request.line.contains("/customers/2222222222/") {
                // A manager, with one client, and the account above again.
                r#"[{"results":[{"customerClient":{"id":"2222222222","level":"0","manager":true,"descriptiveName":"Agency"}}]},{"results":[{"customerClient":{"id":"4444444444","level":"1","manager":false,"descriptiveName":"Acme","currencyCode":"USD","timeZone":"America/New_York"}},{"customerClient":{"id":"1111111111","level":"1","manager":false,"descriptiveName":"Zèbre","currencyCode":"EUR","timeZone":"Europe/Paris"}}]}]"#
            } else {
                // Closed: Google refuses to say anything of it.
                return (
                    403,
                    r#"[{"error":{"code":403,"message":"The caller does not have permission","status":"PERMISSION_DENIED","details":[{"errors":[{"errorCode":{"authorizationError":"CUSTOMER_NOT_ENABLED"},"message":"The customer account can't be accessed because it is not yet enabled or has been deactivated."}]}]}}]"#.into(),
                );
            };
            (200, rows.into())
        })
        .await;
        let google = stand_in.google();
        let accounts = google.ads_accounts("access-1").await.unwrap();
        assert_eq!(
            accounts,
            vec![
                AdsAccount {
                    customer_id: "4444444444".into(),
                    login_customer_id: Some("2222222222".into()),
                    name: "Acme".into(),
                    currency: "USD".into(),
                    time_zone: "America/New_York".into(),
                },
                AdsAccount {
                    customer_id: "1111111111".into(),
                    login_customer_id: None,
                    name: "Zèbre".into(),
                    currency: "EUR".into(),
                    time_zone: "Europe/Paris".into(),
                },
            ]
        );
        let requests = stand_in.requests.lock().unwrap();
        assert_eq!(requests.len(), 4);
        assert!(
            requests
                .iter()
                .all(|r| r.authorization == "Bearer access-1")
        );
        // No developer token: the access level is the Cloud project's.
        assert!(
            requests
                .iter()
                .all(|r| !r.head.to_lowercase().contains("developer-token"))
        );
    }

    #[tokio::test]
    async fn a_report_is_one_question_over_the_days_and_reads_googles_figures() {
        let stand_in = StandIn::start(|request| {
            let body: Value = serde_json::from_str(&request.body).unwrap();
            let query = body["query"].as_str().unwrap();
            assert!(
                query.contains("segments.date BETWEEN '2026-11-01' AND '2026-11-20'"),
                "{query}"
            );
            let rows = if query.contains("FROM campaign ") {
                // Counts as text, decimals as numbers, as Google writes them.
                r#"[{"results":[{"campaign":{"resourceName":"customers/1234567890/campaigns/11","id":"11","name":"Shopping"},"customer":{"currencyCode":"EUR"},"segments":{"date":"2026-11-20","device":"MOBILE"},"metrics":{"costMicros":"9000000","impressions":"1500","clicks":"30","conversions":1.5,"conversionsValue":20.25}},{"campaign":{"id":"11","name":"Shopping"},"customer":{"currencyCode":"EUR"},"segments":{"date":"2026-11-19","device":"CONNECTED_TV"},"metrics":{"impressions":"3"}}]}]"#
            } else if query.contains("FROM ad_group ") {
                r#"[{"results":[{"campaign":{"id":"11"},"adGroup":{"id":"21","name":"Arbres à chat"},"segments":{"date":"2026-11-20"},"metrics":{"costMicros":"9000000","clicks":"30"}}]}]"#
            } else if query.contains("FROM shopping_performance_view ") {
                r#"[{"results":[{"campaign":{"id":"11"},"adGroup":{"id":"21"},"segments":{"date":"2026-11-20","productItemId":"01arbre"},"metrics":{"costMicros":"9000000","clicks":"30"}}]}]"#
            } else {
                assert!(query.contains("FROM search_term_view "), "{query}");
                assert!(query.contains("metrics.clicks > 0"));
                // Nothing: Google answers with an empty stream.
                "[]"
            };
            (200, rows.into())
        })
        .await;
        let google = stand_in.google();
        let mut rows = AdsRows::default();
        for what in AdsRead::ALL {
            google
                .ads_report(
                    "access-1",
                    &account(Some("2222222222")),
                    what,
                    "2026-11-01",
                    "2026-11-20",
                    &mut rows,
                )
                .await
                .unwrap();
        }
        let requests = stand_in.requests.lock().unwrap();
        assert_eq!(requests.len(), 4);
        for request in requests.iter() {
            assert_eq!(
                request.line,
                "POST /ads/v26/customers/1234567890/googleAds:searchStream HTTP/1.1"
            );
            assert!(
                request
                    .head
                    .to_lowercase()
                    .contains("login-customer-id: 2222222222"),
                "{}",
                request.head
            );
        }
        drop(requests);

        let days = assemble_ads_days(rows, "USD");
        assert_eq!(days.len(), 2);
        assert_eq!(days[0].day, "2026-11-19");
        assert_eq!(days[0].campaigns[0].device, "tv");
        assert_eq!(days[0].campaigns[0].numbers.impressions, 3);
        let day = &days[1];
        // The currency the report names, not the one chosen with the account.
        assert_eq!(
            (day.day.as_str(), day.currency.as_str()),
            ("2026-11-20", "EUR")
        );
        assert_eq!(
            day.campaigns[0],
            AdsCampaignRow {
                campaign_id: "11".into(),
                name: "Shopping".into(),
                device: "mobile".into(),
                numbers: AdsNumbers {
                    cost_micros: 9_000_000,
                    impressions: 1_500,
                    clicks: 30,
                    conversions_milli: 1_500,
                    value_micros: 20_250_000,
                },
            }
        );
        assert_eq!(day.ad_groups[0].name, "Arbres à chat");
        assert_eq!(day.products[0].item_id, "01arbre");
        assert!(day.terms.is_empty());
    }

    #[tokio::test]
    async fn a_refusal_is_told_in_google_ads_words() {
        let stand_in = StandIn::start(|request| {
            let body: Value = serde_json::from_str(&request.body).unwrap();
            let query = body["query"].as_str().unwrap();
            if query.contains("FROM campaign ") {
                (
                    429,
                    r#"[{"error":{"code":429,"message":"Resource has been exhausted (e.g. check quota).","status":"RESOURCE_EXHAUSTED","details":[{"@type":"type.googleapis.com/google.ads.googleads.v26.errors.GoogleAdsFailure","errors":[{"errorCode":{"quotaError":"RESOURCE_EXHAUSTED"},"message":"Too many requests. Retry in 900 seconds."}]}]}}]"#.into(),
                )
            } else if query.contains("FROM ad_group ") {
                (
                    403,
                    r#"{"error":{"code":403,"message":"The caller does not have permission","details":[{"errors":[{"errorCode":{"authorizationError":"CLOUD_PROJECT_NOT_APPROVED_FOR_PRODUCTION"},"message":"The Cloud project is only approved for use with test accounts."}]}]}}"#.into(),
                )
            } else if query.contains("FROM shopping_performance_view ") {
                (401, r#"{"error":{"code":401,"message":"Invalid Credentials"}}"#.into())
            } else {
                (503, "busy".into())
            }
        })
        .await;
        let google = stand_in.google();
        let mut rows = AdsRows::default();
        let mut asked = Vec::new();
        for what in AdsRead::ALL {
            asked.push(
                google
                    .ads_report(
                        "access-1",
                        &account(None),
                        what,
                        "2026-11-01",
                        "2026-11-20",
                        &mut rows,
                    )
                    .await,
            );
        }
        assert_eq!(
            asked,
            vec![
                Err(GoogleFault::Unavailable(
                    "Too many requests. Retry in 900 seconds. (RESOURCE_EXHAUSTED)".into()
                )),
                Err(GoogleFault::Refused(
                    "The Cloud project is only approved for use with test accounts. (CLOUD_PROJECT_NOT_APPROVED_FOR_PRODUCTION)".into()
                )),
                Err(GoogleFault::ConnectAgain),
                Err(GoogleFault::Unavailable("HTTP 503".into())),
            ]
        );
        // Reached by itself: no manager is named.
        assert!(
            stand_in
                .requests
                .lock()
                .unwrap()
                .iter()
                .all(|r| !r.head.to_lowercase().contains("login-customer-id"))
        );
    }
}
