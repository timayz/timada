//! The tuning rounds of a shop's first campaign: what reads its last
//! weeks and prepares proposals, and what makes the changes a member
//! approved.
//!
//! A round is asked for on the campaign's stream of thresholds
//! (`RoundRequested`): by a member, or by this saga when the daily read of
//! Google Ads ends and the shop's own schedule says one is due. The round
//! reads what the campaign did from the read model, what the shop sold
//! from its orders, decides with [`timada_google::tune`] and prepares one
//! proposal a change. Google is asked nothing: a bid is the one timada
//! last set.
//!
//! Nothing is changed until a member approves. An approved bid or device
//! adjustment is then checked against what Google Ads holds: one changed
//! there by hand since is left alone, and the proposal says so. A product
//! to keep out of ads is marked in the catalog, and the feed says so: it
//! is never removed from the shop.

use std::collections::BTreeMap;

use evento::{
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_google::aggregator::{AdsChecked, RoundRequested};
use timada_google::{
    AdsLine, CAMPAIGN_BID, CAMPAIGN_COUNTRIES, CAMPAIGN_DAILY_BUDGET, CAMPAIGN_NAME,
    CAMPAIGN_PRODUCTS, CAMPAIGN_SHOPPING, CAMPAIGN_START, CAMPAIGN_WINNERS, CampaignState,
    DEVICE_PREFIX, DeviceFigures, EXCLUDED, GOOGLE_ACTOR, GROUP_PREFIX, GroupFigures, KEPT_OUT,
    MAX_ADS_PRODUCT_ROWS, MAX_ADS_TERM_ROWS, MAX_CAMPAIGN_AD_GROUPS, PreparedRound, ProductFigures,
    SHOWN, START_PAUSED, TUNED_ADJUSTMENT, TUNED_ADS, TUNED_BID, TUNED_DEVICES, TUNED_SEARCH,
    TUNING_ACTOR, TermFigures, Within, ad_group_line, adjustment_percent, ads_within_campaign,
    bid_cents, day_before, last_ads_day, shares_a_word, tune,
};
use timada_proposals::aggregator::Change;
use timada_proposals::{
    KIND_ADS_BID, KIND_ADS_CAMPAIGN, KIND_ADS_EXCLUSION, KIND_ADS_NEGATIVE_TERM, ProposalError,
    ProposalState, waiting_about,
};

use crate::campaigns::{Made, asked, held, reach, refused, said};
use crate::cluster::Executor;
use crate::db::Db;
use crate::google::Google;
use crate::proposals::{Catalog, Draft, Refusal, Subject, prepare};

/// The cursor identity of the subscription, in the store.
pub const SUBSCRIPTION: &str = "timada-dashboard/ads-tuning";

/// What prepares a round's proposals.
pub const SOURCE: &str = "tuning round";

/// A kind of device's name for a person.
pub fn device_label(device: &str) -> &'static str {
    match device {
        "mobile" => "Phones",
        "tablet" => "Tablets",
        "desktop" => "Computers",
        _ => "Other devices",
    }
}

/// What Google Ads calls a kind of device.
fn device_named(device: &str) -> &'static str {
    match device {
        "mobile" => "MOBILE",
        "tablet" => "TABLET",
        _ => "DESKTOP",
    }
}

/// A field of a tuning proposal, for a person.
pub fn field_label(field: &str) -> Option<&'static str> {
    Some(match field {
        TUNED_BID => "Bid",
        TUNED_ADJUSTMENT => "Adjustment, in %",
        TUNED_ADS => "In ads",
        TUNED_SEARCH => "Search",
        _ => return None,
    })
}

/// `value` as a tuning proposal keeps it in `field`.
pub fn normalized(field: &str, value: &str) -> Result<String, timada_google::GoogleError> {
    use timada_google::GoogleError;
    let value = value.trim();
    match field {
        TUNED_BID => bid_cents(&value.replace(',', "."))
            .map(timada_core::format_amount)
            .ok_or(GoogleError::InvalidCampaignAmount),
        TUNED_ADJUSTMENT => adjustment_percent(value.trim_end_matches('%').trim())
            .map(|percent| match percent {
                0 => String::new(),
                percent => percent.to_string(),
            })
            .ok_or(GoogleError::InvalidAdjustment),
        TUNED_ADS if value == SHOWN || value == KEPT_OUT => Ok(value.to_owned()),
        TUNED_SEARCH if value == SHOWN || value == EXCLUDED => Ok(value.to_owned()),
        _ => Err(GoogleError::InvalidTuned),
    }
}

/// What a tuning proposal of `kind` is about, as the shop holds it now;
/// `None` when it is not there.
pub async fn subject(
    store: &Executor,
    tenant_id: &str,
    kind: &str,
    subject_id: &str,
) -> anyhow::Result<Option<Subject>> {
    let one = |name: String, field: &str, value: String| {
        Some(Subject {
            name,
            values: BTreeMap::from([(field.to_owned(), value)]),
            currency: None,
        })
    };
    if kind == KIND_ADS_EXCLUSION {
        let product = Catalog::new(store.clone(), tenant_id)
            .load_product(subject_id)
            .await?;
        return Ok(product.and_then(|product| {
            let held = if product.out_of_ads { KEPT_OUT } else { SHOWN };
            one(product.name, TUNED_ADS, held.to_owned())
        }));
    }
    let Some(campaign) = held(store, tenant_id, CAMPAIGN_SHOPPING).await? else {
        return Ok(None);
    };
    Ok(match kind {
        KIND_ADS_BID => {
            if let Some(ad_group_id) = subject_id.strip_prefix(GROUP_PREFIX) {
                campaign
                    .ad_groups
                    .iter()
                    .find(|group| group.ad_group_id == ad_group_id)
                    .and_then(|group| {
                        one(
                            group.name.clone(),
                            TUNED_BID,
                            campaign.bid_of(ad_group_id).to_owned(),
                        )
                    })
            } else {
                subject_id
                    .strip_prefix(DEVICE_PREFIX)
                    .filter(|device| TUNED_DEVICES.contains(device))
                    .and_then(|device| {
                        one(
                            device_label(device).to_owned(),
                            TUNED_ADJUSTMENT,
                            campaign.adjustment_of(device).to_owned(),
                        )
                    })
            }
        }
        KIND_ADS_NEGATIVE_TERM if !subject_id.trim().is_empty() => {
            let held = if campaign.excludes(subject_id) {
                EXCLUDED
            } else {
                SHOWN
            };
            one(subject_id.to_owned(), TUNED_SEARCH, held.to_owned())
        }
        _ => None,
    })
}

/// A whole amount of hundredths as millionths.
fn micros(cents: u64) -> u64 {
    cents * 10_000
}

/// Makes the change the approved tuning `proposal` describes, as the
/// member who approved, and records it. `change` is the one field its
/// subject does not hold yet. `Err` only when the store failed.
pub async fn apply(
    store: &Executor,
    google: &Google,
    tenant_id: &str,
    proposal: &ProposalState,
    change: &Change,
) -> anyhow::Result<Made> {
    let actor = proposal.answered_by.as_str();
    let subject_id = proposal.subject_id.as_str();
    if proposal.kind == KIND_ADS_EXCLUSION {
        let done = Catalog::new(store.clone(), tenant_id)
            .set_product_in_ads(actor, subject_id, change.after == SHOWN)
            .await;
        return match done {
            Ok(()) => Ok(Made::Done),
            Err(err) if err.is_store_fault() => Err(err.into()),
            Err(err) => refused(format!("The shop refused it: {err}.")),
        };
    }
    let Some(campaign) = held(store, tenant_id, CAMPAIGN_SHOPPING).await? else {
        return refused("The shop has no campaign to change.");
    };
    let Some((connection, account)) = reach(store, tenant_id, &campaign.customer_id).await? else {
        return refused("The shop is no longer connected to the campaign's Google Ads account.");
    };
    let connections = timada_google::Command(store.clone());
    let campaign_id = campaign.campaign_id.as_str();
    let recorded = match (proposal.kind.as_str(), change.field.as_str()) {
        (KIND_ADS_BID, TUNED_BID) => {
            let ad_group_id = subject_id.strip_prefix(GROUP_PREFIX).unwrap_or_default();
            let (Some(before), Some(after)) = (bid_cents(&change.before), bid_cents(&change.after))
            else {
                return refused("A bid is an amount above zero.");
            };
            let units = asked(google, store, &connection, |access| {
                let account = &account;
                async move { google.ads_group_bids(&access, account, ad_group_id).await }
            })
            .await;
            let units = match units {
                Ok(units) => units,
                Err(fault) => return refused(said(&fault)),
            };
            // Someone changed it at Google Ads since the round read it:
            // their change stands.
            let other = units
                .iter()
                .map(|(_, bid)| *bid)
                .find(|bid| *bid != micros(before) && *bid != micros(after));
            match other {
                Some(bid) => {
                    return refused(format!(
                        "It was changed at Google Ads since: a click is paid {} there. Nothing was changed.",
                        timada_core::format_amount(bid / 10_000)
                    ));
                }
                None if units.is_empty() => {
                    return refused(
                        "The ad group has no product group that bids any more: it was changed at Google Ads.",
                    );
                }
                None => {}
            }
            if units.iter().any(|(_, bid)| *bid != micros(after)) {
                let names: Vec<String> = units.into_iter().map(|(name, _)| name).collect();
                let set = asked(google, store, &connection, |access| {
                    let (account, names) = (&account, &names);
                    async move {
                        google
                            .ads_set_group_bid(&access, account, ad_group_id, names, micros(after))
                            .await
                    }
                })
                .await;
                if let Err(fault) = set {
                    return refused(said(&fault));
                }
            }
            connections
                .record_bid(
                    actor,
                    tenant_id,
                    CAMPAIGN_SHOPPING,
                    ad_group_id,
                    &change.after,
                    &proposal.id,
                )
                .await
        }
        (KIND_ADS_BID, TUNED_ADJUSTMENT) => {
            let device = subject_id.strip_prefix(DEVICE_PREFIX).unwrap_or_default();
            let (Some(before), Some(after)) = (
                adjustment_percent(&change.before),
                adjustment_percent(&change.after),
            ) else {
                return refused("An adjustment is a whole percentage.");
            };
            let read = asked(google, store, &connection, |access| {
                let account = &account;
                async move {
                    google
                        .ads_device_adjustments(&access, account, campaign_id)
                        .await
                }
            })
            .await;
            let read = match read {
                Ok(read) => read,
                Err(fault) => return refused(said(&fault)),
            };
            let there = read.iter().find(|(named, ..)| named == device);
            let now = there.map_or(0, |(.., percent)| *percent);
            if now != before && now != after {
                return refused(format!(
                    "It was changed at Google Ads since: the adjustment is {now} % there. Nothing was changed."
                ));
            }
            if now != after {
                let criterion = there.map(|(_, name, _)| name.clone());
                let set = asked(google, store, &connection, |access| {
                    let (account, criterion) = (&account, criterion.as_deref());
                    async move {
                        google
                            .ads_set_device_adjustment(
                                &access,
                                account,
                                campaign_id,
                                (device_named(device), criterion),
                                after,
                            )
                            .await
                    }
                })
                .await;
                if let Err(fault) = set {
                    return refused(said(&fault));
                }
            }
            connections
                .record_adjustment(
                    actor,
                    tenant_id,
                    CAMPAIGN_SHOPPING,
                    device,
                    &change.after,
                    &proposal.id,
                )
                .await
        }
        (KIND_ADS_NEGATIVE_TERM, TUNED_SEARCH) => {
            if change.after != EXCLUDED {
                return refused("A search is brought back at Google Ads, not from here.");
            }
            let set = asked(google, store, &connection, |access| {
                let account = &account;
                async move {
                    google
                        .ads_exclude_term(&access, account, campaign_id, subject_id)
                        .await
                }
            })
            .await;
            if let Err(fault) = set {
                return refused(said(&fault));
            }
            connections
                .record_excluded_term(
                    actor,
                    tenant_id,
                    CAMPAIGN_SHOPPING,
                    subject_id,
                    &proposal.id,
                )
                .await
        }
        _ => return refused("Nothing applies this change."),
    };
    match recorded {
        Ok(_) => {
            tracing::info!(tenant = %tenant_id, kind = %proposal.kind, subject = %subject_id, "a tuning change was made at Google Ads");
            Ok(Made::Done)
        }
        Err(err) if err.is_store_fault() => Err(err.into()),
        Err(err) => refused(format!("The change could not be recorded: {err}.")),
    }
}

/// Whether a member may ask for a round now, and why not: the first
/// campaign runs and has had its first weeks.
pub fn may_tune(
    campaign: Option<&CampaignState>,
    now: u64,
) -> Result<(), timada_google::GoogleError> {
    use timada_google::GoogleError;
    let campaign = campaign.ok_or(GoogleError::NoCampaign)?;
    match campaign.settles_at() {
        Some(settled) if now >= settled => Ok(()),
        _ => Err(GoogleError::CampaignSettling),
    }
}

/// The store and the read models, to prepare rounds with.
#[derive(Clone)]
struct Parts {
    store: Executor,
    db: Db,
}

/// Prepares the rounds asked for, and asks for the ones due by a shop's
/// own schedule once Google Ads was read. A saga: it follows the node's
/// own work group; the streams are the platform's.
pub fn subscription<E: evento::Executor>(store: Executor, db: Db) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(Parts { store, db })
        .handler(on_round_requested())
        .handler(on_ads_checked())
        .tenant("")
        .retry(5)
}

#[evento::subscription]
async fn on_round_requested<E: evento::Executor>(
    context: &Context<'_, E>,
    event: Event<RoundRequested>,
) -> anyhow::Result<()> {
    let parts: Parts = context.extract();
    prepare_round(
        &parts.store,
        &parts.db,
        &event.data.tenant_id,
        crate::auth::now(),
    )
    .await
}

#[evento::subscription]
async fn on_ads_checked<E: evento::Executor>(
    context: &Context<'_, E>,
    event: Event<AdsChecked>,
) -> anyhow::Result<()> {
    // Older days are still owed: the numbers are not whole yet.
    if event.data.behind {
        return Ok(());
    }
    let parts: Parts = context.extract();
    ask_when_due(&parts.store, &event.data.tenant_id, crate::auth::now()).await
}

/// Asks for a round for the shop when its own schedule says one is due at
/// `now` and its campaign may be tuned.
pub(crate) async fn ask_when_due(
    store: &Executor,
    tenant_id: &str,
    now: u64,
) -> anyhow::Result<()> {
    let connections = timada_google::Command(store.clone());
    let Some(tuning) = connections.load_tuning(tenant_id).await? else {
        return Ok(());
    };
    if !tuning.round_due(now) {
        return Ok(());
    }
    let campaign = held(store, tenant_id, CAMPAIGN_SHOPPING).await?;
    if may_tune(campaign.as_ref(), now).is_err() {
        return Ok(());
    }
    if connections.request_round(TUNING_ACTOR, tenant_id).await? {
        tracing::info!(tenant = %tenant_id, "a tuning round is due: asked for");
    }
    Ok(())
}

/// A count read from the read model, never below zero.
fn count(n: i64) -> u64 {
    n.max(0) as u64
}

/// Prepares the round the shop asked for, if none prepared it yet: reads
/// the campaign's last weeks, decides, prepares a proposal a change and
/// records what was done.
pub(crate) async fn prepare_round(
    store: &Executor,
    db: &Db,
    tenant_id: &str,
    now: u64,
) -> anyhow::Result<()> {
    let connections = timada_google::Command(store.clone());
    let Some(tuning) = connections.load_tuning(tenant_id).await? else {
        return Ok(());
    };
    // Another node prepared it.
    if !tuning.round_requested {
        return Ok(());
    }
    let fail = async |reason: &str| -> anyhow::Result<()> {
        tracing::info!(tenant = %tenant_id, %reason, "a tuning round was not prepared");
        connections
            .record_round_failure(GOOGLE_ACTOR, tenant_id, reason)
            .await?;
        Ok(())
    };
    let Some(campaign) = held(store, tenant_id, CAMPAIGN_SHOPPING).await? else {
        return fail("The shop has no first campaign to tune.").await;
    };
    let Some((_, account)) = reach(store, tenant_id, &campaign.customer_id).await? else {
        return fail("The shop is no longer connected to the campaign's Google Ads account.").await;
    };
    let customer_id = account.customer_id.as_str();
    let Some(last) = last_ads_day(db, tenant_id, customer_id).await? else {
        return fail("Google Ads was not read yet: its page says when it will be.").await;
    };
    let settings = tuning.thresholds();
    let first = day_before(&last, settings.days - 1);
    let days = (first.as_str(), last.as_str());
    let within = async |within: Within, limit: usize| {
        ads_within_campaign(
            db,
            within,
            tenant_id,
            (customer_id, &campaign.campaign_id),
            days,
            limit as i64,
        )
        .await
    };
    let by_group = within(Within::AdGroups, MAX_CAMPAIGN_AD_GROUPS * 2).await?;
    let by_item = within(Within::Products, MAX_ADS_PRODUCT_ROWS).await?;
    let by_term = within(Within::Terms, MAX_ADS_TERM_ROWS).await?;
    let by_device = within(Within::Devices, 10).await?;

    // What the shop sold over the same days, as its own clocks count them.
    let zone = jiff::tz::TimeZone::get(&account.time_zone).unwrap_or(jiff::tz::TimeZone::UTC);
    let (Ok(from), Ok(to)) = (
        first.parse::<jiff::civil::Date>(),
        last.parse::<jiff::civil::Date>(),
    ) else {
        return fail("The days Google Ads was read on do not read as days.").await;
    };
    let period = crate::app::t::slug::sales::period_of_days(&zone, from, to);
    let report = crate::sales_report::ad_report(db, tenant_id, period, &first, &last).await?;
    let items = timada_catalog::feed_items(db, tenant_id).await?;

    let resting = |subject: &str| {
        campaign
            .changed_at(subject)
            .is_some_and(|at| now < at + settings.rest_days * 86_400)
    };
    let line_of = |lines: &[AdsLine], key: &str| lines.iter().find(|line| line.key == key).cloned();
    let groups: Vec<GroupFigures> = campaign
        .ad_groups
        .iter()
        .map(|group| {
            let line = line_of(&by_group, &group.ad_group_id);
            GroupFigures {
                ad_group_id: group.ad_group_id.clone(),
                name: group.name.clone(),
                bid_cents: bid_cents(campaign.bid_of(&group.ad_group_id)).unwrap_or(1),
                impressions: line.as_ref().map_or(0, |line| count(line.impressions)),
                clicks: line.as_ref().map_or(0, |line| count(line.clicks)),
                cost_micros: line.as_ref().map_or(0, |line| count(line.cost_micros)),
                value_micros: line.as_ref().map_or(0, |line| count(line.value_micros)),
                break_even: report
                    .categories
                    .iter()
                    .find(|row| row.key == group.category_id)
                    .and_then(|row| row.break_even_hundredths()),
                resting: resting(&group.ad_group_id),
            }
        })
        .collect();

    // A product sold in several variants is an item a variant: added up.
    let mut by_product: BTreeMap<String, ProductFigures> = BTreeMap::new();
    // The items of each product, and the names shown in each ad group.
    let mut items_of: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut names_in: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for line in &by_item {
        let Some((product_id, name)) = items.product_of(&line.key) else {
            continue;
        };
        let sold = report.products.iter().find(|row| row.key == product_id);
        let figures = by_product
            .entry(product_id.to_owned())
            .or_insert_with(|| ProductFigures {
                product_id: product_id.to_owned(),
                name: name.to_owned(),
                sold: sold.map_or(0, |row| row.quantity),
                break_even: sold.and_then(|row| row.break_even_hundredths()),
                ..ProductFigures::default()
            });
        figures.clicks += count(line.clicks);
        figures.cost_micros += count(line.cost_micros);
        figures.conversions_milli += count(line.conversions_milli);
        figures.value_micros += count(line.value_micros);
        items_of
            .entry(product_id.to_owned())
            .or_default()
            .push(line.key.clone());
        // `name` is the ad group the item showed in.
        names_in
            .entry(line.name.clone())
            .or_default()
            .push(name.to_owned());
    }
    let products: Vec<ProductFigures> = by_product.into_values().collect();

    let terms: Vec<TermFigures> = by_term
        .iter()
        .map(|line| {
            let group = campaign
                .ad_groups
                .iter()
                .find(|group| group.ad_group_id == line.name)
                .map(|group| group.name.clone())
                .unwrap_or_default();
            let shown = names_in.get(&line.name).map(Vec::as_slice).unwrap_or(&[]);
            TermFigures {
                related: shares_a_word(
                    &line.key,
                    shown.iter().map(String::as_str).chain([group.as_str()]),
                ),
                excluded: campaign.excludes(&line.key),
                term: line.key.clone(),
                group,
                clicks: count(line.clicks),
                cost_micros: count(line.cost_micros),
                conversions_milli: count(line.conversions_milli),
            }
        })
        .collect();

    let devices: Vec<DeviceFigures> = by_device
        .iter()
        .map(|line| DeviceFigures {
            device: line.key.clone(),
            clicks: count(line.clicks),
            cost_micros: count(line.cost_micros),
            value_micros: count(line.value_micros),
            percent: adjustment_percent(campaign.adjustment_of(&line.key)).unwrap_or(0),
            resting: resting(&line.key),
        })
        .collect();

    let mut round = tune(
        &settings,
        &account.currency,
        &groups,
        &products,
        &terms,
        &devices,
    );

    // Prepared under the name of who asked; a round nobody asked for is
    // the host's, and the members who may approve are told.
    let actor = tuning.round_requested_by.as_str();
    let mut proposals = Vec::new();
    let mut drafts: Vec<(&str, Draft)> = round
        .suggestions
        .iter()
        .map(|suggestion| {
            let kind = match suggestion.field {
                TUNED_ADS => KIND_ADS_EXCLUSION,
                TUNED_SEARCH => KIND_ADS_NEGATIVE_TERM,
                _ => KIND_ADS_BID,
            };
            (
                kind,
                Draft {
                    kind: kind.to_owned(),
                    subject_id: suggestion.subject_id.clone(),
                    values: vec![(suggestion.field.to_owned(), suggestion.after.clone())],
                    reason: suggestion.reason.clone(),
                    source: SOURCE.to_owned(),
                    valid_until: None,
                },
            )
        })
        .collect();

    // The products that sell well: a campaign of their own, proposed once.
    if !round.winners.is_empty() {
        let has = held(store, tenant_id, CAMPAIGN_WINNERS).await?.is_some();
        if has {
            for (_, name) in &round.winners {
                round.flags.push(format!(
                    "{name} sells well above its break-even: add it to the winners' campaign at Google Ads."
                ));
            }
        } else {
            let shop = timada_tenancy::Command(store.clone())
                .load_tenant(tenant_id)
                .await?
                .map(|shop| shop.name)
                .unwrap_or_default();
            let lines: Vec<String> = round
                .winners
                .iter()
                .flat_map(|(product_id, name)| {
                    items_of
                        .get(product_id)
                        .into_iter()
                        .flatten()
                        .map(move |item| ad_group_line(item, name))
                })
                .take(MAX_CAMPAIGN_AD_GROUPS)
                .collect();
            let bid =
                bid_cents(campaign.setting(CAMPAIGN_BID)).unwrap_or(1) + 2 * settings.step_cents;
            drafts.push((
                KIND_ADS_CAMPAIGN,
                Draft {
                    kind: KIND_ADS_CAMPAIGN.to_owned(),
                    subject_id: CAMPAIGN_WINNERS.to_owned(),
                    values: vec![
                        (CAMPAIGN_NAME.to_owned(), format!("Winners · {shop}")),
                        (
                            CAMPAIGN_DAILY_BUDGET.to_owned(),
                            campaign.setting(CAMPAIGN_DAILY_BUDGET).to_owned(),
                        ),
                        (CAMPAIGN_BID.to_owned(), timada_core::format_amount(bid)),
                        (
                            CAMPAIGN_COUNTRIES.to_owned(),
                            campaign.setting(CAMPAIGN_COUNTRIES).to_owned(),
                        ),
                        (CAMPAIGN_START.to_owned(), START_PAUSED.to_owned()),
                        (CAMPAIGN_PRODUCTS.to_owned(), lines.join("\n")),
                    ],
                    reason: format!(
                        "{} products with at least {} clicks and a return half again above their break-even in {} days: a Shopping campaign of their own that outranks the first, two steps above its starting bid.",
                        round.winners.len(),
                        settings.winner_clicks,
                        settings.days
                    ),
                    source: SOURCE.to_owned(),
                    valid_until: None,
                },
            ));
        }
    }

    for (kind, draft) in drafts {
        // One that waits already says the same, or will be answered first.
        if waiting_about(db, tenant_id, kind, &draft.subject_id)
            .await?
            .is_some()
        {
            continue;
        }
        match prepare(store, tenant_id, actor, draft, now).await {
            Ok(id) => proposals.push(id),
            Err(Refusal::Other(err)) => return Err(err),
            Err(Refusal::Proposal(ProposalError::Write(err))) => return Err(err.into()),
            // Changed since the numbers were read (a product kept out
            // meanwhile, a search excluded): nothing to propose.
            Err(refusal) => {
                tracing::debug!(tenant = %tenant_id, %refusal, "a tuning suggestion was not prepared");
            }
        }
    }
    let prepared = proposals.len();
    connections
        .record_round(
            GOOGLE_ACTOR,
            tenant_id,
            PreparedRound {
                first,
                last,
                proposals,
                notes: round.notes,
                flags: round.flags,
            },
        )
        .await?;
    tracing::info!(tenant = %tenant_id, proposals = prepared, "a tuning round was prepared");
    Ok(())
}

#[cfg(test)]
mod tests;
