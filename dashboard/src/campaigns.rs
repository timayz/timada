//! The campaigns timada creates in a shop's Google Ads account: the first
//! Shopping campaign, an ad group a category, and a small Search campaign
//! on the shop's own name.
//!
//! Nothing is sent to Google Ads by itself. A member fills a campaign's
//! settings on its page; they are prepared as a proposal
//! ([`crate::proposals`]), which a member reads whole, edits and approves.
//! Only then does the saga that applies proposals call [`create`], under
//! the name of the member who approved.
//!
//! Google Ads takes a campaign whole or not at all (one request), and
//! knows it by its name: before creating, [`create`] looks for a campaign
//! of that name in the account, and records the one it finds in place of
//! making a second. So an approval seen by two nodes, or by one that
//! stopped between Google's answer and the record, ends with one campaign.
//!
//! A campaign created paused is set running by a member:
//! `LiveRequested` is the intent, and [`subscription`] tells Google Ads,
//! then records since when it runs. One set running at Google Ads itself is
//! noticed by the daily read ([`note_shown`]), on the first day it showed
//! an ad.

use std::collections::BTreeMap;
use std::time::Duration;

use evento::{
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_catalog::CategoryRow;
use timada_google::aggregator::{CampaignAdGroup, LiveRequested};
use timada_google::{
    ACCESS_ADS, ACCESS_MERCHANT_CENTER, AdsAccount, CAMPAIGN_AD_GROUPS, CAMPAIGN_BID,
    CAMPAIGN_BRAND, CAMPAIGN_COUNTRIES, CAMPAIGN_DAILY_BUDGET, CAMPAIGN_DESCRIPTIONS,
    CAMPAIGN_HEADLINES, CAMPAIGN_KEYWORDS, CAMPAIGN_KINDS, CAMPAIGN_NAME, CAMPAIGN_PHONE,
    CAMPAIGN_SHOPPING, CAMPAIGN_START, CAMPAIGN_TABLET, CampaignPlan, CampaignState,
    ConnectionState, CreatedCampaign, DEFAULT_BID, DEFAULT_PHONE, DEFAULT_TABLET, GOOGLE_ACTOR,
    MAX_ADS_NAME_LEN, MAX_CAMPAIGN_AD_GROUPS, MAX_CAMPAIGN_COUNTRIES, MAX_HEADLINE_LEN,
    START_PAUSED, ad_group_line, geo_target_of, plan_of,
};
use timada_proposals::ProposalState;

use crate::cluster::Executor;
use crate::db::Db;
use crate::google::{CampaignContext, FoundCampaign, Google, GoogleFault};

/// The cursor identity of the subscription, in the store.
pub const SUBSCRIPTION: &str = "timada-dashboard/ads-campaigns";

/// What prepares a campaign's proposal.
pub const SOURCE: &str = "first campaign";

/// How long Google Ads is waited for between two tries of a call it could
/// not answer.
const RETRY_AFTER: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(4)];

/// The Google connections of the platform.
type Connections = timada_google::Command<Executor>;

fn connections(store: Executor) -> Connections {
    timada_google::Command(store)
}

/// A kind of campaign's name for a person.
pub fn kind_label(kind: &str) -> &'static str {
    match kind {
        CAMPAIGN_SHOPPING => "First Shopping campaign",
        CAMPAIGN_BRAND => "Search campaign on the shop's name",
        _ => "Campaign",
    }
}

/// A setting's name for a person.
pub fn field_label(field: &str) -> &'static str {
    match field {
        CAMPAIGN_NAME => "Campaign name",
        CAMPAIGN_DAILY_BUDGET => "Daily budget",
        CAMPAIGN_BID => "Starting bid",
        CAMPAIGN_COUNTRIES => "Countries",
        CAMPAIGN_PHONE => "Phones, in %",
        CAMPAIGN_TABLET => "Tablets, in %",
        CAMPAIGN_START => "Starts",
        CAMPAIGN_AD_GROUPS => "Ad groups",
        CAMPAIGN_KEYWORDS => "Searches",
        CAMPAIGN_HEADLINES => "Headlines",
        CAMPAIGN_DESCRIPTIONS => "Descriptions",
        _ => "Setting",
    }
}

/// Whether a setting is a text of several lines.
pub fn is_long(field: &str) -> bool {
    matches!(
        field,
        CAMPAIGN_AD_GROUPS | CAMPAIGN_KEYWORDS | CAMPAIGN_HEADLINES | CAMPAIGN_DESCRIPTIONS
    )
}

/// The shop's campaign of `kind` in the Google Ads account its connection
/// names now; `None` before one was created there. A campaign of an
/// account the shop left is not the shop's any more.
pub async fn held(
    store: &Executor,
    tenant_id: &str,
    kind: &str,
) -> anyhow::Result<Option<CampaignState>> {
    let connections = connections(store.clone());
    let Some(campaign) = connections.load_campaign(tenant_id, kind).await? else {
        return Ok(None);
    };
    let account = connections
        .load_connection(tenant_id)
        .await?
        .and_then(|connection| connection.ads);
    Ok(Some(campaign).filter(|campaign| {
        account.is_some_and(|account| account.customer_id == campaign.customer_id)
    }))
}

/// One thing a campaign waits for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// What a test finds it by.
    pub key: &'static str,
    pub label: &'static str,
    pub met: bool,
    /// What is missing, for the merchant; empty once met.
    pub missing: String,
}

impl Check {
    fn of(key: &'static str, label: &'static str, missing: Option<String>) -> Self {
        Self {
            key,
            label,
            met: missing.is_none(),
            missing: missing.unwrap_or_default(),
        }
    }
}

/// What preparing a campaign is given: whether the shop is ready, and what
/// its form starts from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ready {
    pub checks: Vec<Check>,
    /// The account's currency, for the amounts.
    pub currency: String,
    /// Each setting with the value its form starts from.
    pub defaults: Vec<(&'static str, String)>,
    /// The categories left out for being kept out of ads, by name.
    pub left_out: Vec<String>,
    /// How many categories came after the most a campaign takes.
    pub over: usize,
}

impl Ready {
    /// Whether nothing is missing.
    pub fn is_ready(&self) -> bool {
        self.checks.iter().all(|check| check.met)
    }
}

/// The ad groups of a first campaign: the shop's categories that shoppers
/// see, hold products and are not kept out of ads, in the order of the
/// tree. A category under another is named with it, so no two ad groups
/// share a name. Then the names of the ones kept out.
pub fn ad_groups_of(categories: &[CategoryRow]) -> (Vec<(String, String)>, Vec<String>) {
    let live: BTreeMap<&str, &CategoryRow> = categories
        .iter()
        .filter(|category| !category.archived)
        .map(|category| (category.id.as_str(), category))
        .collect();
    let mut groups: Vec<(String, String)> = Vec::new();
    let mut left_out = Vec::new();
    for category in categories {
        if category.archived || category.draft || category.product_count == 0 {
            continue;
        }
        let mut name = category.name.clone();
        let mut above = category.parent_id.as_deref();
        // A tree is two or three deep; a loop in bad data ends here too.
        for _ in 0..8 {
            let Some(parent) = above.and_then(|id| live.get(id)) else {
                break;
            };
            name = format!("{} > {name}", parent.name);
            above = parent.parent_id.as_deref();
        }
        if category.out_of_ads {
            left_out.push(name);
            continue;
        }
        let mut name: String = name.chars().take(MAX_ADS_NAME_LEN).collect();
        // Two branches cut to the same name: told apart by a number.
        let mut n = 2;
        while groups
            .iter()
            .any(|(_, held)| held.eq_ignore_ascii_case(&name))
        {
            name = format!("{} ({n})", category.name);
            n += 1;
        }
        groups.push((category.id.clone(), name));
    }
    (groups, left_out)
}

/// A text cut to `max` characters at a word.
fn cut(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let cut: String = text.chars().take(max).collect();
    // The cut fell between two words.
    if text.chars().nth(max).is_some_and(char::is_whitespace) {
        return cut;
    }
    match cut.rfind(' ') {
        Some(at) if at > max / 2 => cut[..at].trim_end().to_owned(),
        _ => cut,
    }
}

/// Whether the shop is ready for its campaign of `kind`, and what the
/// form to prepare it starts from. Read from what timada holds: nothing is
/// asked of Google.
pub async fn ready(
    store: &Executor,
    db: &Db,
    tenant: &timada_tenancy::TenantState,
    kind: &str,
) -> anyhow::Result<Ready> {
    let tenant_id = tenant.id.as_str();
    let connection = connections(store.clone())
        .load_connection(tenant_id)
        .await?;
    let account = connection
        .as_ref()
        .filter(|connection| connection.reads(ACCESS_ADS))
        .and_then(|connection| connection.ads.clone());
    let mut checks = vec![Check::of(
        "account",
        "A Google Ads account is chosen",
        account.is_none().then(|| {
            "Connect Google with Google Ads turned on, and choose the shop's Google Ads account."
                .to_owned()
        }),
    )];
    let tag = timada_pages::Command::new(store.clone(), tenant_id)
        .load_google_tag()
        .await?
        .map(|tag| tag.settings);
    let counted = tag
        .as_ref()
        .is_some_and(|tag| tag.ads_id.is_some() && tag.purchase_label.is_some());
    checks.push(Check::of(
        "conversion",
        "The Google tag counts the sales",
        (!counted).then(|| {
            "Give the Google tag the shop's Google Ads id and the label of its purchase conversion: a campaign that cannot count its sales cannot be judged."
                .to_owned()
        }),
    ));
    let orders = timada_orders::Command::new(store.clone(), tenant_id);
    let mut countries: Vec<String> = Vec::new();
    for method in orders.shipping_methods().await? {
        for country in method.countries {
            if geo_target_of(&country).is_some() && !countries.contains(&country) {
                countries.push(country);
            }
        }
    }
    countries.truncate(MAX_CAMPAIGN_COUNTRIES);
    let mut ready = Ready {
        currency: account
            .as_ref()
            .map(|account| account.currency.clone())
            .unwrap_or_default(),
        ..Ready::default()
    };
    let shared = |name: String, budget: &str| {
        vec![
            (CAMPAIGN_NAME, name),
            (CAMPAIGN_DAILY_BUDGET, budget.to_owned()),
            (CAMPAIGN_BID, DEFAULT_BID.to_owned()),
            (CAMPAIGN_COUNTRIES, countries.join(", ")),
        ]
    };
    match kind {
        CAMPAIGN_SHOPPING => {
            let merchant = connection
                .as_ref()
                .filter(|connection| connection.reads(ACCESS_MERCHANT_CENTER))
                .and_then(|connection| connection.merchant.clone());
            let standing = match &merchant {
                Some(merchant) => {
                    timada_google::merchant_standing(db, tenant_id, &merchant.account_id).await?
                }
                None => None,
            };
            checks.push(Check::of(
                "products",
                "Merchant Center accepts every product",
                match (&merchant, &standing) {
                    (None, _) => Some(
                        "Connect Google with Merchant Center turned on, and choose the shop's Merchant Center account."
                            .to_owned(),
                    ),
                    (_, None) => Some(
                        "Merchant Center was not read yet: its page says when it will be."
                            .to_owned(),
                    ),
                    (_, Some(standing)) if !standing.all_accepted_for_ads() => Some(format!(
                        "Merchant Center does not accept every product for Shopping ads yet: {} refused, {} still looked at, {} not sent there.",
                        standing.ads.refused, standing.ads.pending, standing.ads.absent
                    )),
                    _ => None,
                },
            ));
            let label = timada_catalog::Command::new(store.clone(), tenant_id)
                .load_feed_ads_label()
                .await?;
            checks.push(Check::of(
                "label",
                "The feed tells campaigns each product's category",
                label.is_none().then(|| {
                    "Choose the feed's label for campaigns, on the Shopping feed's page: the campaign makes an ad group a category from it."
                        .to_owned()
                }),
            ));
            let categories = timada_catalog::categories_of_tenant(db, tenant_id).await?;
            let (mut groups, left_out) = ad_groups_of(&categories);
            ready.over = groups.len().saturating_sub(MAX_CAMPAIGN_AD_GROUPS);
            groups.truncate(MAX_CAMPAIGN_AD_GROUPS);
            checks.push(Check::of(
                "categories",
                "A category holds products to advertise",
                groups.is_empty().then(|| {
                    "No published category holds a product and is allowed in ads.".to_owned()
                }),
            ));
            ready.left_out = left_out;
            ready.defaults = shared(format!("Shopping · {}", tenant.name), "10.00");
            ready.defaults.extend([
                (CAMPAIGN_PHONE, DEFAULT_PHONE.to_owned()),
                (CAMPAIGN_TABLET, DEFAULT_TABLET.to_owned()),
                (CAMPAIGN_START, START_PAUSED.to_owned()),
                (
                    CAMPAIGN_AD_GROUPS,
                    groups
                        .iter()
                        .map(|(id, name)| ad_group_line(id, name))
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
            ]);
        }
        _ => {
            let address = orders.storefront_address().await?;
            checks.push(Check::of(
                "address",
                "The shop's address is known",
                address
                    .is_none()
                    .then(|| "Say where shoppers find the shop: the ad leads there.".to_owned()),
            ));
            ready.defaults = shared(format!("{} · name", tenant.name), "2.00");
            ready.defaults.extend([
                (CAMPAIGN_START, START_PAUSED.to_owned()),
                (CAMPAIGN_KEYWORDS, tenant.name.to_lowercase()),
                (
                    CAMPAIGN_HEADLINES,
                    [
                        cut(&tenant.name, MAX_HEADLINE_LEN),
                        "Official shop".to_owned(),
                        "Order online".to_owned(),
                    ]
                    .join("\n"),
                ),
                (
                    CAMPAIGN_DESCRIPTIONS,
                    [
                        cut(&format!("{}: the official shop.", tenant.name), 90),
                        "Order online, delivered to your door.".to_owned(),
                    ]
                    .join("\n"),
                ),
            ]);
        }
    }
    ready.checks = checks;
    Ok(ready)
}

/// How creating an approved campaign went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Made {
    /// Google Ads holds it and the shop's stream says so.
    Done,
    /// Not created; the reason is the member's to read.
    Refused(String),
}

fn refused(reason: impl Into<String>) -> anyhow::Result<Made> {
    Ok(Made::Refused(reason.into()))
}

/// Asks Google `call` under the shop's grant, again after a moment when
/// Google could not answer.
async fn asked<T, F, Fut>(
    google: &Google,
    store: &Executor,
    connection: &ConnectionState,
    call: F,
) -> Result<T, GoogleFault>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<T, GoogleFault>>,
{
    let mut waits = RETRY_AFTER.iter();
    loop {
        match google.read(store, connection, &call).await {
            Err(GoogleFault::Unavailable(why)) => match waits.next() {
                Some(wait) => {
                    tracing::debug!(%why, "Google Ads could not answer: asked again");
                    tokio::time::sleep(*wait).await;
                }
                None => return Err(GoogleFault::Unavailable(why)),
            },
            answer => return answer,
        }
    }
}

/// What a member reads of a call Google did not take.
fn said(fault: &GoogleFault) -> String {
    match fault {
        GoogleFault::Off => "No Google application is set on this platform.".to_owned(),
        GoogleFault::ConnectAgain => {
            "Google no longer honours the shop's connection: connect again.".to_owned()
        }
        GoogleFault::Unavailable(why) => {
            format!("Google Ads could not answer ({why}). Nothing was created: prepare it again.")
        }
        GoogleFault::Refused(why) => format!("Google Ads refused it: {why}"),
    }
}

/// The ad groups of `plan` as `found` holds them, when it holds them all:
/// a campaign of the same name that is this one.
fn adopted(plan: &CampaignPlan, found: &FoundCampaign) -> Option<Vec<CampaignAdGroup>> {
    if plan.kind != CAMPAIGN_SHOPPING {
        let (ad_group_id, name) = found.ad_groups.first()?;
        return Some(vec![CampaignAdGroup {
            ad_group_id: ad_group_id.clone(),
            category_id: String::new(),
            name: name.clone(),
        }]);
    }
    plan.ad_groups
        .iter()
        .map(|group| {
            found
                .ad_groups
                .iter()
                .find(|(_, name)| *name == group.name)
                .map(|(ad_group_id, _)| CampaignAdGroup {
                    ad_group_id: ad_group_id.clone(),
                    category_id: group.category_id.clone(),
                    name: group.name.clone(),
                })
        })
        .collect()
}

/// Creates at Google Ads the campaign the approved `proposal` describes,
/// and records it on the shop, as the member who approved. `Err` only when
/// the store failed.
pub async fn create(
    store: &Executor,
    google: &Google,
    tenant_id: &str,
    proposal: &ProposalState,
) -> anyhow::Result<Made> {
    let kind = proposal.subject_id.as_str();
    let settings: Vec<(String, String)> = proposal
        .changes
        .iter()
        .map(|change| (change.field.clone(), change.after.clone()))
        .collect();
    let plan = match plan_of(
        kind,
        settings
            .iter()
            .map(|(field, value)| (field.as_str(), value.as_str())),
    ) {
        Ok(plan) => plan,
        Err(err) => return refused(format!("The campaign cannot be created: {err}.")),
    };
    let connections = connections(store.clone());
    let connection = connections.load_connection(tenant_id).await?;
    let Some((connection, account)) = connection
        .filter(|connection| connection.reads(ACCESS_ADS))
        .and_then(|connection| {
            let account = connection.ads.clone()?;
            Some((connection, account))
        })
    else {
        return refused("The shop has no Google Ads account to create it in.");
    };
    let mut context = CampaignContext::default();
    if kind == CAMPAIGN_SHOPPING {
        let Some(merchant) = connection
            .merchant
            .as_ref()
            .filter(|_| connection.covers(ACCESS_MERCHANT_CENTER))
        else {
            return refused("The shop has no Merchant Center account whose products to show.");
        };
        context.merchant_id = merchant.account_id.clone();
        let label = timada_catalog::Command::new(store.clone(), tenant_id)
            .load_feed_ads_label()
            .await?;
        let Some(label) = label else {
            return refused("The feed has no label for campaigns any more.");
        };
        context.label = label;
    } else {
        let address = timada_orders::Command::new(store.clone(), tenant_id)
            .storefront_address()
            .await?;
        let Some(address) = address else {
            return refused("The shop has no address for the ad to lead to.");
        };
        context.final_url = address;
    }
    // A campaign of that name already there is this one, created by an
    // attempt that did not get to record it, or it is someone else's.
    let found = asked(google, store, &connection, |access| {
        let (account, name) = (&account, plan.name.as_str());
        async move { google.ads_find_campaign(&access, account, name).await }
    })
    .await;
    let (campaign_id, ad_groups) = match found {
        Err(fault) => return refused(said(&fault)),
        Ok(Some(found)) => match adopted(&plan, &found) {
            Some(ad_groups) => (found.campaign_id, ad_groups),
            None => {
                return refused(format!(
                    "The account already has a campaign named « {} » that is not this one: prepare it again under another name.",
                    plan.name
                ));
            }
        },
        Ok(None) => {
            let created = asked(google, store, &connection, |access| {
                let (account, plan, context) = (&account, &plan, &context);
                async move {
                    google
                        .ads_create_campaign(&access, account, plan, context)
                        .await
                }
            })
            .await;
            let created = match created {
                Ok(created) => created,
                Err(fault) => return refused(said(&fault)),
            };
            let ad_groups = if kind == CAMPAIGN_SHOPPING {
                plan.ad_groups
                    .iter()
                    .zip(&created.ad_group_ids)
                    .map(|(group, ad_group_id)| CampaignAdGroup {
                        ad_group_id: ad_group_id.clone(),
                        category_id: group.category_id.clone(),
                        name: group.name.clone(),
                    })
                    .collect()
            } else {
                created
                    .ad_group_ids
                    .iter()
                    .map(|ad_group_id| CampaignAdGroup {
                        ad_group_id: ad_group_id.clone(),
                        category_id: String::new(),
                        name: plan.name.clone(),
                    })
                    .collect()
            };
            (created.campaign_id, ad_groups)
        }
    };
    let recorded = connections
        .record_campaign(
            &proposal.answered_by,
            tenant_id,
            CreatedCampaign {
                kind: kind.to_owned(),
                customer_id: account.customer_id.clone(),
                campaign_id: campaign_id.clone(),
                settings,
                ad_groups,
                proposal_id: proposal.id.clone(),
            },
        )
        .await;
    match recorded {
        Ok(_) => {
            tracing::info!(tenant = %tenant_id, %kind, campaign = %campaign_id, live = plan.live, "a campaign was created at Google Ads");
            Ok(Made::Done)
        }
        Err(err) if err.is_store_fault() => Err(err.into()),
        Err(err) => refused(format!("The campaign could not be recorded: {err}.")),
    }
}

/// The store and Google, to set campaigns running with.
#[derive(Clone)]
struct Parts {
    store: Executor,
    google: Google,
}

/// Follows the campaigns members ask to set running. A saga: it follows
/// the node's own work group; the streams are the platform's.
pub fn subscription<E: evento::Executor>(
    store: Executor,
    google: Google,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(Parts { store, google })
        .handler(on_live_requested())
        .tenant("")
        .retry(5)
}

#[evento::subscription]
async fn on_live_requested<E: evento::Executor>(
    context: &Context<'_, E>,
    event: Event<LiveRequested>,
) -> anyhow::Result<()> {
    let parts: Parts = context.extract();
    set_live(&parts.store, &parts.google, &event.aggregate_id).await
}

/// Tells Google Ads to run the campaign of the stream `id`, when a member
/// asked and nobody answered yet, and records how it went.
pub(crate) async fn set_live(store: &Executor, google: &Google, id: &str) -> anyhow::Result<()> {
    let connections = connections(store.clone());
    let Some(campaign) = connections.load_campaign_by_id(id).await? else {
        return Ok(());
    };
    // Another node answered.
    if !campaign.live_requested {
        return Ok(());
    }
    let (tenant_id, kind) = (campaign.tenant_id.as_str(), campaign.kind.as_str());
    let fail = async |reason: String| -> anyhow::Result<()> {
        tracing::info!(tenant = %tenant_id, %kind, %reason, "a campaign was not set running");
        connections
            .record_live_failure(GOOGLE_ACTOR, tenant_id, kind, &reason)
            .await?;
        Ok(())
    };
    let connection = connections.load_connection(tenant_id).await?;
    let Some((connection, account)) = connection
        .filter(|connection| connection.reads(ACCESS_ADS))
        .and_then(|connection| {
            let account: AdsAccount = connection.ads.clone()?;
            (account.customer_id == campaign.customer_id).then_some((connection, account))
        })
    else {
        return fail(
            "The shop is no longer connected to the campaign's Google Ads account.".into(),
        )
        .await;
    };
    let enabled = asked(google, store, &connection, |access| {
        let (account, campaign_id) = (&account, campaign.campaign_id.as_str());
        async move {
            google
                .ads_enable_campaign(&access, account, campaign_id)
                .await
        }
    })
    .await;
    match enabled {
        Ok(()) => {
            connections
                .record_live(
                    &campaign.live_requested_by,
                    tenant_id,
                    kind,
                    &campaign.campaign_id,
                    crate::auth::now(),
                )
                .await?;
            tracing::info!(tenant = %tenant_id, %kind, "a campaign was set running");
            Ok(())
        }
        Err(GoogleFault::Unavailable(why)) => {
            fail(format!("Google Ads could not answer ({why}): ask again.")).await
        }
        Err(fault) => fail(said(&fault)).await,
    }
}

/// Records as running the shop's paused campaigns that the daily read saw
/// showing ads: `shown` is, by Google Ads' number of a campaign, the first
/// day (`2026-11-20`) among the ones just read on which it was shown. A
/// member set them running at Google Ads itself.
pub async fn note_shown(
    store: &Executor,
    tenant_id: &str,
    customer_id: &str,
    shown: &BTreeMap<String, String>,
) -> anyhow::Result<()> {
    if shown.is_empty() {
        return Ok(());
    }
    let connections = connections(store.clone());
    for kind in CAMPAIGN_KINDS {
        let Some(campaign) = connections.load_campaign(tenant_id, kind).await? else {
            continue;
        };
        if campaign.live_at.is_some() || campaign.customer_id != customer_id {
            continue;
        }
        let Some(at) = shown
            .get(&campaign.campaign_id)
            .and_then(|day| timada_core::civil::parse_date(day))
        else {
            continue;
        };
        let noted = connections
            .record_live(GOOGLE_ACTOR, tenant_id, kind, &campaign.campaign_id, at)
            .await?;
        if noted {
            tracing::info!(tenant = %tenant_id, %kind, "a campaign was seen running at Google Ads");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
