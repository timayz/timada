use std::sync::{Arc, Mutex};

use evento::Evento;
use serde_json::{Value, json};
use timada_catalog::CreateProduct;
use timada_google::aggregator::{
    AdsAdGroupRow, AdsCampaignRow, AdsNumbers, AdsProductRow, AdsTermRow, CampaignAdGroup,
};
use timada_google::{
    AdsAccount, AdsDayReport, CAMPAIGN_AD_GROUPS, CAMPAIGN_PHONE, CAMPAIGN_TABLET, CreatedCampaign,
    MerchantAccount, START_LIVE, TUNING_EVERY_DAYS,
};
use timada_proposals::{ProposalStatus, waiting_proposals};
use timada_tenancy::CreateTenant;

use super::*;
use crate::google::tests::{Request, StandIn, connected, token_answer};
use crate::proposals::apply::settle;
use crate::proposals::{Proposals, approve};

const ACCOUNT: &str = "1234567890";
const ALICE: &str = "user-alice";
const DAY: u64 = 86_400;

/// The Google connections of the platform.
type Connections = timada_google::Command<Evento>;

/// What the stand-in's Google Ads holds and how it answers.
#[derive(Default)]
struct Held {
    /// What a click is paid in each product group that bids, by ad group.
    bids: BTreeMap<String, u64>,
    /// The devices the campaign says something of: device, modifier.
    devices: Vec<(String, f64)>,
    /// The batches of operations it was sent.
    mutations: Vec<Vec<Value>>,
}

fn answer(held: &Mutex<Held>, request: &Request) -> (u16, String) {
    let mut held = held.lock().unwrap();
    if request.line.starts_with("POST /token") {
        return token_answer("access-1");
    }
    let body: Value = serde_json::from_str(&request.body).unwrap();
    if request.line.contains("googleAds:searchStream") {
        let query = body["query"].as_str().unwrap();
        let results: Vec<Value> = if query.contains("FROM ad_group_criterion") {
            held.bids
                .iter()
                .filter(|(group, _)| query.contains(&format!("ad_group.id = {group} ")))
                .map(|(group, bid)| {
                    json!({"adGroupCriterion": {
                        "resourceName": format!("customers/{ACCOUNT}/adGroupCriteria/{group}~9"),
                        "cpcBidMicros": bid.to_string(),
                    }})
                })
                .collect()
        } else {
            assert!(query.contains("FROM campaign_criterion"), "{query}");
            held.devices
                .iter()
                .map(|(device, modifier)| {
                    json!({"campaignCriterion": {
                        "resourceName": format!("customers/{ACCOUNT}/campaignCriteria/501~{device}"),
                        "device": {"type": device},
                        "bidModifier": modifier,
                    }})
                })
                .collect()
        };
        return (200, json!([{ "results": results }]).to_string());
    }
    assert!(
        request.line.contains("googleAds:mutate"),
        "{}",
        request.line
    );
    let operations = body["mutateOperations"].as_array().unwrap().clone();
    for operation in &operations {
        if let Some(update) = operation["adGroupCriterionOperation"].get("update") {
            let name = update["resourceName"].as_str().unwrap();
            let group = name.rsplit('/').next().unwrap().split('~').next().unwrap();
            let bid = update["cpcBidMicros"].as_str().unwrap().parse().unwrap();
            held.bids.insert(group.to_owned(), bid);
        }
    }
    let answers: Vec<Value> = operations.iter().map(|_| json!({})).collect();
    held.mutations.push(operations);
    (
        200,
        json!({ "mutateOperationResponses": answers }).to_string(),
    )
}

fn numbers(clicks: u64, cost: u64, value: u64) -> AdsNumbers {
    AdsNumbers {
        cost_micros: cost * 1_000_000,
        impressions: clicks * 40,
        clicks,
        conversions_milli: if value > 0 { 2_000 } else { 0 },
        value_micros: value * 1_000_000,
    }
}

struct Rig {
    store: Evento,
    db: Db,
    shop: String,
    held: Arc<Mutex<Held>>,
    stand_in: StandIn,
    google: Google,
    /// The product the ads of "Arbres" show.
    tree: String,
    now: u64,
}

impl Rig {
    /// A shop whose first campaign runs, with two ad groups ("Arbres",
    /// number 601, and "Jouets", 602), phones at -55 %, and one day of
    /// numbers: Arbres brought 60.00 for 10.00 over 120 clicks, on a
    /// product that got 160 clicks and sold nothing by the shop's own
    /// orders; Jouets got 12 clicks; "niche chien" was clicked five times.
    async fn new() -> Self {
        let turso = evento::Turso::in_memory_concurrent().await.unwrap();
        crate::db::migrate_store_read_models(&turso).await.unwrap();
        let db = Db::from(turso.clone());
        let store = Evento::new(turso.clone());
        let shop = timada_tenancy::Command(store.clone())
            .create_tenant(CreateTenant {
                name: "Acme".to_owned(),
                owner_user_id: ALICE.to_owned(),
            })
            .await
            .unwrap()
            .id;
        let held = Arc::new(Mutex::new(Held {
            bids: BTreeMap::from([("601".to_owned(), 300_000), ("602".to_owned(), 300_000)]),
            devices: vec![("MOBILE".to_owned(), 0.45)],
            ..Held::default()
        }));
        let stand_in = StandIn::start({
            let held = held.clone();
            move |request| answer(&held, request)
        })
        .await;
        let google = stand_in.google();
        connected(&turso, &google, &shop, "refresh-1").await;
        let connections = timada_google::Command(store.clone());
        connections
            .choose_ads_account(
                ALICE,
                &shop,
                AdsAccount {
                    customer_id: ACCOUNT.to_owned(),
                    login_customer_id: None,
                    name: "Acme".to_owned(),
                    currency: "EUR".to_owned(),
                    time_zone: "UTC".to_owned(),
                },
            )
            .await
            .unwrap();
        connections
            .choose_merchant_account(
                ALICE,
                &shop,
                MerchantAccount {
                    account_id: "5550001".to_owned(),
                    name: "Acme".to_owned(),
                },
            )
            .await
            .unwrap();
        let tree = Catalog::new(store.clone(), &shop)
            .create_product(
                ALICE,
                CreateProduct {
                    name: "Arbre à chat XXL".to_owned(),
                    slug: None,
                },
            )
            .await
            .unwrap()
            .id;
        // The catalog's read model, as its subscription would keep it.
        db.execute(
            "INSERT INTO catalog_products (tenant_id, id, slug, name, status, created_by,
                created_at, updated_at, image_count, variant_count, tax_category,
                slug_search, name_search)
             VALUES (?1, ?2, ?2, 'Arbre à chat XXL', 'published', 'alice', 0, 0, 0, 0,
                'standard', ?2, 'arbre')",
            (shop.as_str(), tree.as_str()),
        )
        .await
        .unwrap();
        let settings = [
            (timada_google::CAMPAIGN_NAME, "Shopping · Acme"),
            (CAMPAIGN_DAILY_BUDGET, "10"),
            (CAMPAIGN_BID, "0.30"),
            (CAMPAIGN_COUNTRIES, "FR"),
            (CAMPAIGN_PHONE, "-55"),
            (CAMPAIGN_TABLET, ""),
            (CAMPAIGN_START, START_LIVE),
            (CAMPAIGN_AD_GROUPS, "01CAT Arbres\n01TOY Jouets"),
        ];
        connections
            .record_campaign(
                ALICE,
                &shop,
                CreatedCampaign {
                    kind: CAMPAIGN_SHOPPING.to_owned(),
                    customer_id: ACCOUNT.to_owned(),
                    campaign_id: "501".to_owned(),
                    settings: settings
                        .into_iter()
                        .map(|(field, value)| (field.to_owned(), value.to_owned()))
                        .collect(),
                    ad_groups: vec![
                        CampaignAdGroup {
                            ad_group_id: "601".into(),
                            category_id: "01CAT".into(),
                            name: "Arbres".into(),
                        },
                        CampaignAdGroup {
                            ad_group_id: "602".into(),
                            category_id: "01TOY".into(),
                            name: "Jouets".into(),
                        },
                    ],
                    proposal_id: "proposal-0".to_owned(),
                },
            )
            .await
            .unwrap();
        let now = crate::auth::now();
        let day = timada_core::civil::format_date(now - DAY);
        let row = |device: &str, clicks, cost, value| AdsCampaignRow {
            campaign_id: "501".into(),
            name: "Shopping · Acme".into(),
            device: device.into(),
            numbers: numbers(clicks, cost, value),
        };
        let group = |id: &str, name: &str, clicks, cost, value| AdsAdGroupRow {
            campaign_id: "501".into(),
            ad_group_id: id.into(),
            name: name.into(),
            numbers: numbers(clicks, cost, value),
        };
        let term = |term: &str, clicks| AdsTermRow {
            term: term.into(),
            campaign_id: "501".into(),
            ad_group_id: "601".into(),
            numbers: numbers(clicks, 2, 0),
        };
        connections
            .record_ads_day(
                "reader",
                &shop,
                ACCOUNT,
                AdsDayReport {
                    day,
                    currency: "EUR".into(),
                    // Phones bring a quarter of what the campaign does.
                    campaigns: vec![
                        row("mobile", 110, 6, 9),
                        row("desktop", 110, 6, 51),
                        // Another campaign of the account is not this one's.
                        AdsCampaignRow {
                            campaign_id: "77".into(),
                            name: "By hand".into(),
                            device: "mobile".into(),
                            numbers: numbers(900, 90, 0),
                        },
                    ],
                    ad_groups: vec![
                        group("601", "Arbres", 120, 10, 60),
                        group("602", "Jouets", 12, 2, 0),
                    ],
                    products: vec![AdsProductRow {
                        item_id: tree.to_lowercase(),
                        campaign_id: "501".into(),
                        ad_group_id: "601".into(),
                        numbers: numbers(160, 8, 0),
                    }],
                    terms: vec![term("niche chien", 5), term("arbre a chat pas cher", 9)],
                },
            )
            .await
            .unwrap();
        let rig = Self {
            store,
            db,
            shop,
            held,
            stand_in,
            google,
            tree,
            now,
        };
        rig.catch_up().await;
        rig
    }

    fn connections(&self) -> Connections {
        timada_google::Command(self.store.clone())
    }

    /// The read models a round reads, brought up to date.
    async fn catch_up(&self) {
        timada_google::read_model_subscription::<Evento>(self.db.clone())
            .run_once(&self.store)
            .await
            .unwrap();
        timada_proposals::read_model_subscription::<Evento>(self.db.clone())
            .run_once(&self.store)
            .await
            .unwrap();
    }

    /// Alice asks for a round and it is prepared; the proposals that wait.
    async fn round(&self, now: u64) -> Vec<ProposalState> {
        self.connections()
            .request_round(ALICE, &self.shop)
            .await
            .unwrap();
        prepare_round(&self.store, &self.db, &self.shop, now)
            .await
            .unwrap();
        self.catch_up().await;
        let waiting = waiting_proposals(
            &self.db,
            &self.shop,
            timada_core::page::PageFrom::First,
            100,
        )
        .await
        .unwrap();
        let mut proposals = Vec::new();
        for row in waiting.rows {
            proposals.push(self.proposal(&row.id).await);
        }
        proposals
    }

    async fn proposal(&self, id: &str) -> ProposalState {
        Proposals::new(self.store.clone(), &self.shop)
            .load_proposal(id)
            .await
            .unwrap()
            .unwrap()
    }

    /// Alice approves `proposal` and the saga makes the change.
    async fn approved(&self, proposal: &ProposalState) -> ProposalState {
        approve(&self.store, &self.shop, ALICE, proposal, vec![], self.now)
            .await
            .unwrap();
        settle(&self.store, &self.google, &self.shop, &proposal.id)
            .await
            .unwrap();
        self.catch_up().await;
        self.proposal(&proposal.id).await
    }

    async fn campaign(&self) -> CampaignState {
        held(&self.store, &self.shop, CAMPAIGN_SHOPPING)
            .await
            .unwrap()
            .unwrap()
    }

    fn mutations(&self) -> Vec<Vec<Value>> {
        self.held.lock().unwrap().mutations.clone()
    }
}

fn about<'a>(proposals: &'a [ProposalState], subject: &str) -> &'a ProposalState {
    proposals
        .iter()
        .find(|proposal| proposal.subject_id == subject)
        .unwrap_or_else(|| {
            let held: Vec<&str> = proposals
                .iter()
                .map(|proposal| proposal.subject_id.as_str())
                .collect();
            panic!("no proposal about {subject} among {held:?}")
        })
}

#[tokio::test]
async fn a_round_prepares_a_proposal_a_change_and_asks_google_nothing() {
    let rig = Rig::new().await;
    let proposals = rig.round(rig.now).await;
    assert_eq!(proposals.len(), 5, "{proposals:#?}");
    assert!(rig.stand_in.lines().is_empty());
    assert!(
        proposals
            .iter()
            .all(|proposal| proposal.source == SOURCE && proposal.prepared_by == ALICE)
    );

    // 6.00 back for 1 against the shop's target of 3.00: two steps up.
    let bid = about(&proposals, "group:601");
    assert_eq!(bid.kind, KIND_ADS_BID);
    assert_eq!(bid.subject_name, "Arbres");
    let change = &bid.changes[0];
    assert_eq!(
        (
            change.field.as_str(),
            change.before.as_str(),
            change.after.as_str()
        ),
        (TUNED_BID, "0.30", "0.34")
    );
    assert!(bid.reason.starts_with("A return of 6.00"), "{}", bid.reason);

    // 160 clicks and not one sale: out of the ads, not out of the shop.
    let drained = about(&proposals, &rig.tree);
    assert_eq!(drained.kind, KIND_ADS_EXCLUSION);
    assert_eq!(drained.subject_name, "Arbre à chat XXL");
    assert_eq!(
        (
            drained.changes[0].before.as_str(),
            drained.changes[0].after.as_str()
        ),
        (SHOWN, KEPT_OUT)
    );

    // A search with nothing in common with what the ad group shows; the
    // one that names the product is left.
    let term = about(&proposals, "niche chien");
    assert_eq!(term.kind, KIND_ADS_NEGATIVE_TERM);
    assert_eq!(term.changes[0].after, EXCLUDED);
    assert!(
        proposals
            .iter()
            .all(|proposal| proposal.subject_id != "arbre a chat pas cher")
    );

    // Phones bring a quarter of the campaign's return: ten points less.
    let phones = about(&proposals, "device:mobile");
    assert_eq!(phones.subject_name, "Phones");
    assert_eq!(
        (
            phones.changes[0].before.as_str(),
            phones.changes[0].after.as_str()
        ),
        ("-55", "-65")
    );

    // Computers bring well over it: ten more.
    let computers = about(&proposals, "device:desktop");
    assert_eq!(
        (
            computers.changes[0].before.as_str(),
            computers.changes[0].after.as_str()
        ),
        ("", "10")
    );

    let tuning = rig
        .connections()
        .load_tuning(&rig.shop)
        .await
        .unwrap()
        .unwrap();
    assert!(!tuning.round_requested);
    let last = tuning.last_round.unwrap();
    assert_eq!(last.proposals.len(), 5);
    assert_eq!(last.error, None);
    assert_eq!(last.first, day_before(&last.last, 20));
    assert_eq!(
        last.notes,
        vec!["Jouets: 12 clicks of the 100 a decision takes."]
    );

    // Asked again while they wait: nothing is prepared twice.
    let again = rig.round(rig.now).await;
    assert_eq!(again.len(), 5);
    // Delivered again with no round asked for: nothing.
    prepare_round(&rig.store, &rig.db, &rig.shop, rig.now)
        .await
        .unwrap();
}

#[tokio::test]
async fn an_approved_bid_is_set_at_google_ads_and_left_to_show() {
    let rig = Rig::new().await;
    let proposals = rig.round(rig.now).await;
    let applied = rig.approved(about(&proposals, "group:601")).await;
    assert_eq!(applied.status, ProposalStatus::Applied, "{}", applied.note);
    assert_eq!(rig.campaign().await.bid_of("601"), "0.34");
    assert_eq!(rig.campaign().await.bid_of("602"), "0.30");
    let mutations = rig.mutations();
    assert_eq!(mutations.len(), 1);
    // The ad group and its product group, together.
    assert_eq!(
        mutations[0][0]["adGroupOperation"],
        json!({
            "updateMask": "cpc_bid_micros",
            "update": {"resourceName": "customers/1234567890/adGroups/601", "cpcBidMicros": "340000"},
        })
    );
    assert_eq!(
        mutations[0][1]["adGroupCriterionOperation"]["update"],
        json!({
            "resourceName": "customers/1234567890/adGroupCriteria/601~9",
            "cpcBidMicros": "340000",
        })
    );
    // The approval seen again: Google is asked nothing more.
    let asked = rig.stand_in.lines().len();
    settle(&rig.store, &rig.google, &rig.shop, &applied.id)
        .await
        .unwrap();
    assert_eq!(rig.stand_in.lines().len(), asked);

    // The next round leaves the ad group to show, and says so.
    let next = rig.round(rig.now + DAY).await;
    assert!(
        next.iter()
            .all(|proposal| proposal.subject_id != "group:601")
    );
    let tuning = rig
        .connections()
        .load_tuning(&rig.shop)
        .await
        .unwrap()
        .unwrap();
    assert!(
        tuning
            .last_round
            .unwrap()
            .notes
            .contains(&"Arbres: its bid changed less than 14 days ago, left to show.".to_owned())
    );
    // Rested, it is looked at again, from the bid it now has.
    let later = rig.round(rig.now + 15 * DAY).await;
    assert_eq!(about(&later, "group:601").changes[0].before, "0.34");
    assert_eq!(about(&later, "group:601").changes[0].after, "0.38");
}

#[tokio::test]
async fn a_bid_changed_at_google_ads_since_is_left_alone() {
    let rig = Rig::new().await;
    let proposals = rig.round(rig.now).await;
    rig.held
        .lock()
        .unwrap()
        .bids
        .insert("601".to_owned(), 420_000);
    let failed = rig.approved(about(&proposals, "group:601")).await;
    assert_eq!(failed.status, ProposalStatus::Failed);
    assert_eq!(
        failed.note,
        "It was changed at Google Ads since: a click is paid 0.42 there. Nothing was changed."
    );
    assert!(rig.mutations().is_empty());
    assert_eq!(rig.campaign().await.bid_of("601"), "0.30");
}

#[tokio::test]
async fn an_approved_exclusion_marks_the_product_and_a_search_is_excluded_once() {
    let rig = Rig::new().await;
    let proposals = rig.round(rig.now).await;
    let catalog = Catalog::new(rig.store.clone(), &rig.shop);

    let applied = rig.approved(about(&proposals, &rig.tree)).await;
    assert_eq!(applied.status, ProposalStatus::Applied, "{}", applied.note);
    let product = catalog.load_product(&rig.tree).await.unwrap().unwrap();
    assert!(product.out_of_ads);
    // Marked in the catalog: Google Ads hears of it through the feed.
    assert!(rig.mutations().is_empty());

    let applied = rig.approved(about(&proposals, "niche chien")).await;
    assert_eq!(applied.status, ProposalStatus::Applied, "{}", applied.note);
    assert!(rig.campaign().await.excludes("niche chien"));
    assert_eq!(
        rig.mutations()[0][0]["campaignCriterionOperation"]["create"],
        json!({
            "campaign": "customers/1234567890/campaigns/501",
            "negative": true,
            "keyword": {"text": "niche chien", "matchType": "EXACT"},
        })
    );

    // The numbers still hold both: the next round proposes neither again.
    let next = rig.round(rig.now).await;
    assert!(
        next.iter()
            .all(|proposal| proposal.subject_id != rig.tree && proposal.subject_id != "niche chien"),
        "{next:#?}"
    );
}

#[tokio::test]
async fn an_approved_adjustment_changes_the_devices_own_criterion() {
    let rig = Rig::new().await;
    let proposals = rig.round(rig.now).await;
    let applied = rig.approved(about(&proposals, "device:mobile")).await;
    assert_eq!(applied.status, ProposalStatus::Applied, "{}", applied.note);
    assert_eq!(rig.campaign().await.adjustment_of("mobile"), "-65");
    assert_eq!(
        rig.mutations()[0][0]["campaignCriterionOperation"],
        json!({
            "updateMask": "bid_modifier",
            "update": {
                "resourceName": "customers/1234567890/campaignCriteria/501~MOBILE",
                "bidModifier": 0.35,
            },
        })
    );
}

#[tokio::test]
async fn a_round_comes_by_itself_only_when_the_shop_asked_and_the_campaign_settled() {
    let rig = Rig::new().await;
    let connections = rig.connections();
    let asked = async || {
        connections
            .load_tuning(&rig.shop)
            .await
            .unwrap()
            .is_some_and(|tuning| tuning.round_requested)
    };
    let settled = rig.now + 22 * DAY;
    // Nothing set: only when asked.
    ask_when_due(&rig.store, &rig.shop, settled).await.unwrap();
    assert!(!asked().await);
    connections
        .set_tuning(
            ALICE,
            &rig.shop,
            vec![(TUNING_EVERY_DAYS.to_owned(), "7".to_owned())],
        )
        .await
        .unwrap();
    // In its first three weeks the campaign is left alone.
    ask_when_due(&rig.store, &rig.shop, rig.now + DAY)
        .await
        .unwrap();
    assert!(!asked().await);
    assert!(may_tune(Some(&rig.campaign().await), rig.now + DAY).is_err());
    assert!(may_tune(None, settled).is_err());

    ask_when_due(&rig.store, &rig.shop, settled).await.unwrap();
    assert!(asked().await);
    prepare_round(&rig.store, &rig.db, &rig.shop, settled)
        .await
        .unwrap();
    let tuning = connections.load_tuning(&rig.shop).await.unwrap().unwrap();
    // Prepared for nobody in particular: the host's.
    assert_eq!(tuning.round_requested_by, TUNING_ACTOR);
    assert_eq!(tuning.last_round.as_ref().unwrap().proposals.len(), 5);
    let prepared = rig
        .proposal(&tuning.last_round.as_ref().unwrap().proposals[0])
        .await;
    assert_eq!(prepared.prepared_by, TUNING_ACTOR);
    // Not due again before a week went by since: the round was recorded
    // at the real clock's now.
    ask_when_due(&rig.store, &rig.shop, rig.now + 3 * DAY)
        .await
        .unwrap();
    assert!(!asked().await);
}

#[tokio::test]
async fn a_round_with_nothing_to_read_says_why() {
    let rig = Rig::new().await;
    // The shop moved to another Google Ads account: the campaign is not
    // its own any more.
    rig.connections()
        .choose_ads_account(
            ALICE,
            &rig.shop,
            AdsAccount {
                customer_id: "9876543210".to_owned(),
                login_customer_id: None,
                name: "Other".to_owned(),
                currency: "EUR".to_owned(),
                time_zone: "UTC".to_owned(),
            },
        )
        .await
        .unwrap();
    let proposals = rig.round(rig.now).await;
    assert!(proposals.is_empty());
    let tuning = rig
        .connections()
        .load_tuning(&rig.shop)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        tuning.last_round.unwrap().error.as_deref(),
        Some("The shop has no first campaign to tune.")
    );
}
