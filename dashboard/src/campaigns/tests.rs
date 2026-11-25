use std::sync::{Arc, Mutex};

use evento::Evento;
use serde_json::{Value, json};
use timada_google::{
    CAMPAIGN_AD_GROUPS, CAMPAIGN_BID, CAMPAIGN_COUNTRIES, CAMPAIGN_DAILY_BUDGET, CAMPAIGN_NAME,
    CAMPAIGN_PHONE, CAMPAIGN_START, CAMPAIGN_TABLET, GoogleError, MerchantAccount, START_LIVE,
};
use timada_proposals::{KIND_ADS_CAMPAIGN, ProposalStatus};
use timada_tenancy::CreateTenant;

use super::*;
use crate::auth::now;
use crate::google::tests::{Request, StandIn, connected, token_answer};
use crate::proposals::apply::settle;
use crate::proposals::{Draft, Proposals, Refusal, approve, prepare};

const ACCOUNT: &str = "1234567890";
const ALICE: &str = "user-alice";
const BOB: &str = "user-bob";

/// A campaign of the stand-in's account: name, number, running, ad groups
/// (number, name).
type HeldCampaign = (String, String, bool, Vec<(String, String)>);

/// What the stand-in's Google Ads holds and how it answers.
#[derive(Default)]
struct Held {
    campaigns: Vec<HeldCampaign>,
    /// The batches of operations it was sent.
    mutations: Vec<Vec<Value>>,
    /// What the next batches are answered with, in place of taking them.
    refusals: Vec<(u16, String)>,
}

fn answer(held: &Mutex<Held>, request: &Request) -> (u16, String) {
    let mut held = held.lock().unwrap();
    if request.line.starts_with("POST /token") {
        return token_answer("access-1");
    }
    assert_eq!(request.authorization, "Bearer access-1");
    let body: Value = serde_json::from_str(&request.body).unwrap();
    if request.line.contains("googleAds:searchStream") {
        let query = body["query"].as_str().unwrap();
        assert!(
            query.contains("FROM ad_group WHERE campaign.name = '"),
            "{query}"
        );
        let results: Vec<Value> = held
            .campaigns
            .iter()
            .filter(|(name, ..)| query.contains(&format!("campaign.name = '{name}'")))
            .flat_map(|(_, id, running, groups)| {
                groups.iter().map(move |(group_id, group)| {
                    json!({
                        "campaign": {"id": id, "status": if *running { "ENABLED" } else { "PAUSED" }},
                        "adGroup": {"id": group_id, "name": group},
                    })
                })
            })
            .collect();
        return (200, json!([{ "results": results }]).to_string());
    }
    assert_eq!(
        request.line,
        format!("POST /ads/v26/customers/{ACCOUNT}/googleAds:mutate HTTP/1.1")
    );
    if !held.refusals.is_empty() {
        return held.refusals.remove(0);
    }
    let operations = body["mutateOperations"].as_array().unwrap().clone();
    let mut answers = Vec::new();
    let mut groups = Vec::new();
    let number = format!("{}", 501 + held.campaigns.len());
    let mut created = None;
    for operation in &operations {
        if let Some(update) = operation["campaignOperation"].get("update") {
            let name = update["resourceName"].as_str().unwrap();
            for campaign in &mut held.campaigns {
                if name.ends_with(&format!("/campaigns/{}", campaign.1)) {
                    campaign.2 = update["status"] == "ENABLED";
                }
            }
            answers.push(json!({"campaignResult": {"resourceName": name}}));
        } else if let Some(campaign) = operation["campaignOperation"].get("create") {
            created = Some((
                campaign["name"].as_str().unwrap().to_owned(),
                campaign["status"] == "ENABLED",
            ));
            answers.push(json!({"campaignResult": {
                "resourceName": format!("customers/{ACCOUNT}/campaigns/{number}"),
            }}));
        } else if let Some(group) = operation["adGroupOperation"].get("create") {
            let id = format!("{}", 601 + groups.len());
            answers.push(json!({"adGroupResult": {
                "resourceName": format!("customers/{ACCOUNT}/adGroups/{id}"),
            }}));
            groups.push((id, group["name"].as_str().unwrap().to_owned()));
        } else if operation.get("campaignBudgetOperation").is_some() {
            answers.push(json!({"campaignBudgetResult": {
                "resourceName": format!("customers/{ACCOUNT}/campaignBudgets/401"),
            }}));
        } else {
            answers.push(json!({}));
        }
    }
    if let Some((name, running)) = created {
        held.campaigns.push((name, number, running, groups));
    }
    held.mutations.push(operations);
    (
        200,
        json!({ "mutateOperationResponses": answers }).to_string(),
    )
}

struct Rig {
    store: Evento,
    shop: String,
    held: Arc<Mutex<Held>>,
    stand_in: StandIn,
    google: Google,
}

impl Rig {
    /// A shop of Alice's connected to Google, its Google Ads and Merchant
    /// Center accounts chosen, its feed's label for campaigns set.
    async fn new() -> Self {
        let turso = evento::Turso::in_memory_concurrent().await.unwrap();
        let store = Evento::new(turso.clone());
        let shop = timada_tenancy::Command(store.clone())
            .create_tenant(CreateTenant {
                name: "Acme".to_owned(),
                owner_user_id: ALICE.to_owned(),
            })
            .await
            .unwrap()
            .id;
        let held = Arc::new(Mutex::new(Held::default()));
        let stand_in = StandIn::start({
            let held = held.clone();
            move |request| answer(&held, request)
        })
        .await;
        let google = stand_in.google();
        connected(&turso, &google, &shop, "refresh-1").await;
        let connections = connections(store.clone());
        connections
            .choose_ads_account(
                ALICE,
                &shop,
                AdsAccount {
                    customer_id: ACCOUNT.to_owned(),
                    login_customer_id: None,
                    name: "Acme".to_owned(),
                    currency: "EUR".to_owned(),
                    time_zone: "Europe/Paris".to_owned(),
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
        timada_catalog::Command::new(store.clone(), &shop)
            .set_feed_ads_label(ALICE, Some(2))
            .await
            .unwrap();
        Self {
            store,
            shop,
            held,
            stand_in,
            google,
        }
    }

    fn shopping(start: &str) -> Draft {
        Draft {
            kind: KIND_ADS_CAMPAIGN.to_owned(),
            subject_id: CAMPAIGN_SHOPPING.to_owned(),
            values: [
                (CAMPAIGN_NAME, "Shopping · Acme"),
                (CAMPAIGN_DAILY_BUDGET, "10"),
                (CAMPAIGN_BID, "0.30"),
                (CAMPAIGN_COUNTRIES, "FR, BE"),
                (CAMPAIGN_PHONE, "-55"),
                (CAMPAIGN_TABLET, "-40"),
                (CAMPAIGN_START, start),
                (CAMPAIGN_AD_GROUPS, "01CAT Arbres à chat\n01TOY Jouets"),
            ]
            .into_iter()
            .map(|(field, value)| (field.to_owned(), value.to_owned()))
            .collect(),
            reason: "The shop's first campaign.".to_owned(),
            source: SOURCE.to_owned(),
            valid_until: None,
        }
    }

    /// Bob prepares, Alice approves with what she changed; the proposal.
    async fn approved(&self, draft: Draft, edits: &[(&str, &str)]) -> String {
        let id = prepare(&self.store, &self.shop, BOB, draft, now())
            .await
            .unwrap();
        let waiting = self.proposal(&id).await;
        let edits = edits
            .iter()
            .map(|(field, after)| ((*field).to_owned(), (*after).to_owned()))
            .collect();
        approve(&self.store, &self.shop, ALICE, &waiting, edits, now())
            .await
            .unwrap();
        id
    }

    async fn proposal(&self, id: &str) -> ProposalState {
        Proposals::new(self.store.clone(), &self.shop)
            .load_proposal(id)
            .await
            .unwrap()
            .unwrap()
    }

    async fn settle(&self, id: &str) {
        settle(&self.store, &self.google, &self.shop, id)
            .await
            .unwrap();
    }

    async fn campaign(&self, kind: &str) -> Option<CampaignState> {
        held(&self.store, &self.shop, kind).await.unwrap()
    }

    fn mutations(&self) -> Vec<Vec<Value>> {
        self.held.lock().unwrap().mutations.clone()
    }
}

/// The operations of a batch that create something of `kind`.
fn creates<'a>(batch: &'a [Value], kind: &str) -> Vec<&'a Value> {
    batch
        .iter()
        .filter_map(|operation| operation[kind].get("create"))
        .collect()
}

#[tokio::test]
async fn an_approved_campaign_is_created_once_as_the_member_left_it() {
    let rig = Rig::new().await;
    // Prepared is not sent: Google Ads hears nothing before the approval.
    let id = prepare(
        &rig.store,
        &rig.shop,
        BOB,
        Rig::shopping(START_PAUSED),
        now(),
    )
    .await
    .unwrap();
    rig.settle(&id).await;
    assert!(rig.stand_in.lines().is_empty());
    let waiting = rig.proposal(&id).await;
    assert_eq!(waiting.subject_name, "First Shopping campaign");
    assert_eq!(waiting.changes.len(), 8);
    assert!(
        waiting
            .changes
            .iter()
            .all(|change| change.before.is_empty())
    );

    // Alice raises the bid and leaves the tablets alone.
    approve(
        &rig.store,
        &rig.shop,
        ALICE,
        &waiting,
        vec![
            (CAMPAIGN_BID.to_owned(), "0,35".to_owned()),
            (CAMPAIGN_TABLET.to_owned(), String::new()),
        ],
        now(),
    )
    .await
    .unwrap();
    rig.settle(&id).await;
    assert_eq!(rig.proposal(&id).await.status, ProposalStatus::Applied);
    let campaign = rig.campaign(CAMPAIGN_SHOPPING).await.unwrap();
    assert_eq!(campaign.campaign_id, "501");
    assert_eq!(campaign.customer_id, ACCOUNT);
    assert_eq!(campaign.proposal_id, id);
    assert_eq!(campaign.setting(CAMPAIGN_BID), "0.35");
    assert_eq!(campaign.setting(CAMPAIGN_TABLET), "");
    assert_eq!(campaign.live_at, None);
    assert_eq!(
        campaign
            .ad_groups
            .iter()
            .map(|group| (group.ad_group_id.as_str(), group.category_id.as_str()))
            .collect::<Vec<_>>(),
        vec![("601", "01CAT"), ("602", "01TOY")]
    );

    let mutations = rig.mutations();
    assert_eq!(mutations.len(), 1);
    let batch = &mutations[0];
    let budget = creates(batch, "campaignBudgetOperation")[0];
    assert_eq!(budget["amountMicros"], "10000000");
    assert_eq!(budget["explicitlyShared"], false);
    let created = creates(batch, "campaignOperation")[0];
    assert_eq!(created["name"], "Shopping · Acme");
    assert_eq!(created["advertisingChannelType"], "SHOPPING");
    assert_eq!(created["status"], "PAUSED");
    assert_eq!(created["shoppingSetting"]["merchantId"], "5550001");
    assert_eq!(
        created["containsEuPoliticalAdvertising"],
        "DOES_NOT_CONTAIN_EU_POLITICAL_ADVERTISING"
    );
    let criteria = creates(batch, "campaignCriterionOperation");
    assert_eq!(
        criteria
            .iter()
            .filter_map(|criterion| criterion["location"]["geoTargetConstant"].as_str())
            .collect::<Vec<_>>(),
        vec!["geoTargetConstants/2250", "geoTargetConstants/2056"]
    );
    // Phones at -55 %, and no word of the tablets.
    let devices: Vec<_> = criteria
        .iter()
        .filter(|criterion| criterion.get("device").is_some())
        .collect();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0]["device"]["type"], "MOBILE");
    assert_eq!(devices[0]["bidModifier"], 0.45);
    let groups = creates(batch, "adGroupOperation");
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0]["name"], "Arbres à chat");
    assert_eq!(groups[0]["type"], "SHOPPING_PRODUCT_ADS");
    assert_eq!(groups[0]["cpcBidMicros"], "350000");
    assert_eq!(creates(batch, "adGroupAdOperation").len(), 2);
    // A tree an ad group: its root, the category's products, and the rest
    // left out.
    let listed = creates(batch, "adGroupCriterionOperation");
    assert_eq!(listed.len(), 6);
    assert_eq!(listed[0]["listingGroup"]["type"], "SUBDIVISION");
    let shown = &listed[1]["listingGroup"];
    assert_eq!(shown["parentAdGroupCriterion"], listed[0]["resourceName"]);
    assert_eq!(
        shown["caseValue"]["productCustomAttribute"],
        json!({"index": "INDEX2", "value": "01cat"})
    );
    assert_eq!(listed[1]["cpcBidMicros"], "350000");
    assert_eq!(listed[2]["negative"], true);
    assert_eq!(
        listed[2]["listingGroup"]["caseValue"]["productCustomAttribute"],
        json!({"index": "INDEX2"})
    );
    // What one operation creates is named below zero, each its own.
    let mut temporary: Vec<&str> = batch
        .iter()
        .filter_map(|operation| {
            operation
                .as_object()?
                .values()
                .next()?
                .get("create")?
                .get("resourceName")?
                .as_str()
        })
        .collect();
    let named = temporary.len();
    temporary.sort_unstable();
    temporary.dedup();
    assert_eq!(temporary.len(), named);

    // The approval seen again, by this node or another: nothing is asked.
    let asked = rig.stand_in.lines().len();
    rig.settle(&id).await;
    assert_eq!(rig.stand_in.lines().len(), asked);
    assert_eq!(rig.mutations().len(), 1);

    // A campaign is created, never changed.
    let again = prepare(&rig.store, &rig.shop, BOB, Rig::shopping(START_LIVE), now()).await;
    assert!(matches!(
        again,
        Err(Refusal::Google(GoogleError::CampaignExists))
    ));
}

/// A node stopped between Google's answer and the record: the next finds
/// the campaign by its name and records it, without a second.
#[tokio::test]
async fn a_campaign_google_ads_holds_already_is_recorded_not_made_twice() {
    let rig = Rig::new().await;
    rig.held.lock().unwrap().campaigns.push((
        "Shopping · Acme".into(),
        "777".into(),
        false,
        vec![
            ("72".into(), "Jouets".into()),
            ("71".into(), "Arbres à chat".into()),
        ],
    ));
    let id = rig.approved(Rig::shopping(START_PAUSED), &[]).await;
    rig.settle(&id).await;
    assert_eq!(rig.proposal(&id).await.status, ProposalStatus::Applied);
    assert!(rig.mutations().is_empty());
    let campaign = rig.campaign(CAMPAIGN_SHOPPING).await.unwrap();
    assert_eq!(campaign.campaign_id, "777");
    assert_eq!(campaign.ad_groups[0].ad_group_id, "71");
    assert_eq!(campaign.ad_groups[0].category_id, "01CAT");
    assert_eq!(campaign.ad_groups[1].ad_group_id, "72");
}

#[tokio::test]
async fn someone_elses_campaign_of_that_name_is_left_alone() {
    let rig = Rig::new().await;
    rig.held.lock().unwrap().campaigns.push((
        "Shopping · Acme".into(),
        "777".into(),
        true,
        vec![("71".into(), "All products".into())],
    ));
    let id = rig.approved(Rig::shopping(START_PAUSED), &[]).await;
    rig.settle(&id).await;
    let failed = rig.proposal(&id).await;
    assert_eq!(failed.status, ProposalStatus::Failed);
    assert!(failed.note.contains("another name"), "{}", failed.note);
    assert!(rig.mutations().is_empty());
    assert!(rig.campaign(CAMPAIGN_SHOPPING).await.is_none());
}

#[tokio::test]
async fn what_google_ads_refuses_is_the_proposals_outcome() {
    let rig = Rig::new().await;
    rig.held.lock().unwrap().refusals.push((
        400,
        r#"{"error":{"code":400,"message":"Request contains an invalid argument.","details":[{"errors":[{"errorCode":{"merchantCenterError":"MERCHANT_ID_CANNOT_BE_ACCESSED"},"message":"Merchant ID is either not found or not linked to the Google Ads customer."}]}]}}"#.into(),
    ));
    let id = rig.approved(Rig::shopping(START_PAUSED), &[]).await;
    rig.settle(&id).await;
    let failed = rig.proposal(&id).await;
    assert_eq!(failed.status, ProposalStatus::Failed);
    assert_eq!(
        failed.note,
        "Google Ads refused it: Merchant ID is either not found or not linked to the Google Ads customer. (MERCHANT_ID_CANNOT_BE_ACCESSED)"
    );
    assert!(rig.campaign(CAMPAIGN_SHOPPING).await.is_none());

    // Put right at Google, it is prepared and approved again.
    let id = rig.approved(Rig::shopping(START_LIVE), &[]).await;
    // Google stumbles once: asked again a moment later.
    rig.held.lock().unwrap().refusals.push((503, "busy".into()));
    rig.settle(&id).await;
    assert_eq!(rig.proposal(&id).await.status, ProposalStatus::Applied);
    let campaign = rig.campaign(CAMPAIGN_SHOPPING).await.unwrap();
    // Created running: live from the start.
    assert_eq!(campaign.live_at, Some(campaign.created_at));
    assert_eq!(rig.mutations().len(), 1);
}

#[tokio::test]
async fn a_shop_with_no_account_or_no_label_fails_the_approved_campaign() {
    let rig = Rig::new().await;
    let id = rig.approved(Rig::shopping(START_PAUSED), &[]).await;
    timada_catalog::Command::new(rig.store.clone(), &rig.shop)
        .set_feed_ads_label(ALICE, None)
        .await
        .unwrap();
    rig.settle(&id).await;
    let failed = rig.proposal(&id).await;
    assert_eq!(failed.status, ProposalStatus::Failed);
    assert_eq!(failed.note, "The feed has no label for campaigns any more.");

    let id = rig.approved(Rig::shopping(START_PAUSED), &[]).await;
    connections(rig.store.clone())
        .disconnect(ALICE, &rig.shop)
        .await
        .unwrap();
    rig.settle(&id).await;
    let failed = rig.proposal(&id).await;
    assert_eq!(failed.status, ProposalStatus::Failed);
    assert_eq!(
        failed.note,
        "The shop has no Google Ads account to create it in."
    );
    assert!(rig.stand_in.lines().is_empty());
}

#[tokio::test]
async fn a_campaign_on_the_shops_name_is_a_search_campaign_with_one_ad() {
    let rig = Rig::new().await;
    timada_orders::Command::new(rig.store.clone(), &rig.shop)
        .set_storefront_address(ALICE, Some("https://acme.example"))
        .await
        .unwrap();
    let draft = Draft {
        kind: KIND_ADS_CAMPAIGN.to_owned(),
        subject_id: CAMPAIGN_BRAND.to_owned(),
        values: [
            (CAMPAIGN_NAME, "Acme · name"),
            (CAMPAIGN_DAILY_BUDGET, "2"),
            (CAMPAIGN_BID, "0.30"),
            (CAMPAIGN_COUNTRIES, "FR"),
            (CAMPAIGN_START, START_PAUSED),
            (CAMPAIGN_KEYWORDS, "acme\nacme moto"),
            (CAMPAIGN_HEADLINES, "Acme\nOfficial shop\nOrder online"),
            (
                CAMPAIGN_DESCRIPTIONS,
                "Acme: the official shop.\nOrder online, delivered to your door.",
            ),
        ]
        .into_iter()
        .map(|(field, value)| (field.to_owned(), value.to_owned()))
        .collect(),
        reason: "A Search campaign on the shop's own name.".to_owned(),
        source: SOURCE.to_owned(),
        valid_until: None,
    };
    let id = rig.approved(draft, &[]).await;
    rig.settle(&id).await;
    assert_eq!(rig.proposal(&id).await.status, ProposalStatus::Applied);
    let campaign = rig.campaign(CAMPAIGN_BRAND).await.unwrap();
    assert_eq!(campaign.ad_groups.len(), 1);
    assert_eq!(campaign.ad_groups[0].category_id, "");
    // The Shopping campaign is another stream.
    assert!(rig.campaign(CAMPAIGN_SHOPPING).await.is_none());

    let batch = &rig.mutations()[0];
    let created = creates(batch, "campaignOperation")[0];
    assert_eq!(created["advertisingChannelType"], "SEARCH");
    assert_eq!(created["networkSettings"]["targetContentNetwork"], false);
    assert!(created.get("shoppingSetting").is_none());
    assert_eq!(
        creates(batch, "adGroupOperation")[0]["type"],
        "SEARCH_STANDARD"
    );
    let keywords = creates(batch, "adGroupCriterionOperation");
    assert_eq!(keywords.len(), 2);
    assert_eq!(
        keywords[1]["keyword"],
        json!({"text": "acme moto", "matchType": "PHRASE"})
    );
    let ad = &creates(batch, "adGroupAdOperation")[0]["ad"];
    assert_eq!(ad["finalUrls"], json!(["https://acme.example"]));
    assert_eq!(
        ad["responsiveSearchAd"]["headlines"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[tokio::test]
async fn a_paused_campaign_is_set_running_when_a_member_asks() {
    let rig = Rig::new().await;
    let id = rig.approved(Rig::shopping(START_PAUSED), &[]).await;
    rig.settle(&id).await;
    let connections = connections(rig.store.clone());
    let stream = rig.campaign(CAMPAIGN_SHOPPING).await.unwrap().id;
    // Nobody asked: nothing is told to Google Ads.
    set_live(&rig.store, &rig.google, &stream).await.unwrap();
    assert_eq!(rig.mutations().len(), 1);

    // Google Ads refuses: the member reads why, and may ask again.
    connections
        .request_live(ALICE, &rig.shop, CAMPAIGN_SHOPPING)
        .await
        .unwrap();
    rig.held.lock().unwrap().refusals.push((
        400,
        r#"{"error":{"message":"Bad","details":[{"errors":[{"errorCode":{"campaignError":"CAMPAIGN_CANNOT_USE_SHARED_BUDGET"},"message":"The budget cannot be used."}]}]}}"#.into(),
    ));
    set_live(&rig.store, &rig.google, &stream).await.unwrap();
    let refused = rig.campaign(CAMPAIGN_SHOPPING).await.unwrap();
    assert!(!refused.live_requested);
    assert_eq!(refused.live_at, None);
    assert_eq!(
        refused.live_error.as_deref(),
        Some(
            "Google Ads refused it: The budget cannot be used. (CAMPAIGN_CANNOT_USE_SHARED_BUDGET)"
        )
    );

    connections
        .request_live(ALICE, &rig.shop, CAMPAIGN_SHOPPING)
        .await
        .unwrap();
    set_live(&rig.store, &rig.google, &stream).await.unwrap();
    let live = rig.campaign(CAMPAIGN_SHOPPING).await.unwrap();
    assert!(live.live_at.is_some());
    assert_eq!(live.live_error, None);
    let update = &rig.mutations()[1][0]["campaignOperation"];
    assert_eq!(update["updateMask"], "status");
    assert_eq!(
        update["update"],
        json!({"resourceName": "customers/1234567890/campaigns/501", "status": "ENABLED"})
    );
    assert!(rig.held.lock().unwrap().campaigns[0].2);
    // Delivered again: nothing more.
    set_live(&rig.store, &rig.google, &stream).await.unwrap();
    assert_eq!(rig.mutations().len(), 2);
}

/// Set running at Google Ads itself: the daily read sees it show an ad.
#[tokio::test]
async fn a_campaign_seen_showing_ads_is_recorded_as_running() {
    let rig = Rig::new().await;
    let id = rig.approved(Rig::shopping(START_PAUSED), &[]).await;
    rig.settle(&id).await;
    let created_at = rig.campaign(CAMPAIGN_SHOPPING).await.unwrap().created_at;
    // Another campaign of the account showed ads: not this one.
    let other = BTreeMap::from([("11".to_owned(), "2999-01-01".to_owned())]);
    note_shown(&rig.store, &rig.shop, ACCOUNT, &other)
        .await
        .unwrap();
    assert_eq!(rig.campaign(CAMPAIGN_SHOPPING).await.unwrap().live_at, None);
    // Another account's read says nothing of it either.
    let shown = BTreeMap::from([("501".to_owned(), "2999-01-01".to_owned())]);
    note_shown(&rig.store, &rig.shop, "9876543210", &shown)
        .await
        .unwrap();
    assert_eq!(rig.campaign(CAMPAIGN_SHOPPING).await.unwrap().live_at, None);

    note_shown(&rig.store, &rig.shop, ACCOUNT, &shown)
        .await
        .unwrap();
    let live_at = rig
        .campaign(CAMPAIGN_SHOPPING)
        .await
        .unwrap()
        .live_at
        .unwrap();
    assert_eq!(
        live_at,
        timada_core::civil::parse_date("2999-01-01").unwrap()
    );
    assert!(live_at > created_at);
    // The first day stands.
    let later = BTreeMap::from([("501".to_owned(), "2999-02-01".to_owned())]);
    note_shown(&rig.store, &rig.shop, ACCOUNT, &later)
        .await
        .unwrap();
    assert_eq!(
        rig.campaign(CAMPAIGN_SHOPPING).await.unwrap().live_at,
        Some(live_at)
    );
}

fn category(id: &str, name: &str, parent: Option<&str>) -> CategoryRow {
    CategoryRow {
        tenant_id: "shop".into(),
        id: id.into(),
        slug: id.to_lowercase(),
        name: name.into(),
        description: String::new(),
        parent_id: parent.map(str::to_owned),
        position: 0,
        archived: false,
        in_menu: true,
        draft: false,
        out_of_ads: false,
        created_by: ALICE.into(),
        created_at: 0,
        updated_at: 0,
        product_count: 3,
        subtree_count: 3,
    }
}

#[test]
fn the_ad_groups_are_the_categories_worth_advertising() {
    let mut empty = category("01EMPTY", "Vide", None);
    empty.product_count = 0;
    let mut draft = category("01DRAFT", "Brouillon", None);
    draft.draft = true;
    let mut gone = category("01GONE", "Ancienne", None);
    gone.archived = true;
    let mut out = category("01OUT", "Litières", Some("01CAT"));
    out.out_of_ads = true;
    let categories = vec![
        category("01CAT", "Chats", None),
        category("01TREE", "Arbres", Some("01CAT")),
        out,
        category("01DOG", "Chiens", None),
        // The same name under another parent: told apart by it.
        category("01DTREE", "Arbres", Some("01DOG")),
        empty,
        draft,
        gone,
    ];
    let (groups, left_out) = ad_groups_of(&categories);
    assert_eq!(
        groups,
        vec![
            ("01CAT".to_owned(), "Chats".to_owned()),
            ("01TREE".to_owned(), "Chats > Arbres".to_owned()),
            ("01DOG".to_owned(), "Chiens".to_owned()),
            ("01DTREE".to_owned(), "Chiens > Arbres".to_owned()),
        ]
    );
    assert_eq!(left_out, vec!["Chats > Litières"]);
    assert_eq!(cut("Acme", 30), "Acme");
    assert_eq!(
        cut("Acme Moto Équipement du Motard Exigeant", 30),
        "Acme Moto Équipement du Motard"
    );
    assert_eq!(
        cut("Acme Moto Équipement des Motards", 30),
        "Acme Moto Équipement des"
    );
}
