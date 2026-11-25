mod common;

use common::*;
use timada_google::aggregator::CampaignAdGroup;
use timada_google::{
    CAMPAIGN_AD_GROUPS, CAMPAIGN_BID, CAMPAIGN_BRAND, CAMPAIGN_COUNTRIES, CAMPAIGN_DAILY_BUDGET,
    CAMPAIGN_NAME, CAMPAIGN_PHONE, CAMPAIGN_SHOPPING, CAMPAIGN_START, CAMPAIGN_TABLET,
    CreatedCampaign, GOOGLE_ACTOR, GoogleError, SETTLE_DAYS, START_LIVE, START_PAUSED,
};

const ACCOUNT: &str = "1234567890";

fn settings(start: &str) -> Vec<(String, String)> {
    [
        (CAMPAIGN_NAME, " Shopping · Acme "),
        (CAMPAIGN_DAILY_BUDGET, "10"),
        (CAMPAIGN_BID, "0,3"),
        (CAMPAIGN_COUNTRIES, "fr, be"),
        (CAMPAIGN_PHONE, "-55%"),
        (CAMPAIGN_TABLET, ""),
        (CAMPAIGN_START, start),
        (CAMPAIGN_AD_GROUPS, "01CAT Arbres à chat\n01TOY Jouets"),
    ]
    .into_iter()
    .map(|(field, value)| (field.to_owned(), value.to_owned()))
    .collect()
}

fn created(campaign_id: &str, start: &str) -> CreatedCampaign {
    CreatedCampaign {
        kind: CAMPAIGN_SHOPPING.to_owned(),
        customer_id: "123-456-7890".to_owned(),
        campaign_id: campaign_id.to_owned(),
        settings: settings(start),
        ad_groups: vec![
            CampaignAdGroup {
                ad_group_id: "21".into(),
                category_id: "01CAT".into(),
                name: "Arbres à chat".into(),
            },
            CampaignAdGroup {
                ad_group_id: "22".into(),
                category_id: "01TOY".into(),
                name: "Jouets".into(),
            },
        ],
        proposal_id: "proposal-1".to_owned(),
    }
}

#[tokio::test]
async fn a_campaign_is_recorded_once_with_what_was_approved() {
    let google = command().await;
    assert!(
        google
            .load_campaign(SHOP, CAMPAIGN_SHOPPING)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        google
            .record_campaign(ALICE, SHOP, created("11", START_PAUSED))
            .await
            .unwrap()
    );
    let campaign = google
        .load_campaign(SHOP, CAMPAIGN_SHOPPING)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(campaign.tenant_id, SHOP);
    assert_eq!(campaign.customer_id, ACCOUNT);
    assert_eq!(campaign.campaign_id, "11");
    assert_eq!(campaign.name, "Shopping · Acme");
    // Kept as a proposal keeps them.
    assert_eq!(campaign.setting(CAMPAIGN_BID), "0.30");
    assert_eq!(campaign.setting(CAMPAIGN_COUNTRIES), "FR, BE");
    assert_eq!(campaign.setting(CAMPAIGN_PHONE), "-55");
    assert_eq!(campaign.setting(CAMPAIGN_TABLET), "");
    assert_eq!(campaign.ad_groups.len(), 2);
    assert_eq!(campaign.proposal_id, "proposal-1");
    assert_eq!(campaign.live_at, None);
    assert_eq!(campaign.settles_at(), None);
    let plan = campaign.plan().unwrap();
    assert_eq!(plan.daily_budget_micros, 10_000_000);
    assert_eq!(plan.bid_micros, 300_000);
    assert_eq!(plan.phone_percent, Some(-55));
    assert_eq!(plan.tablet_percent, None);
    assert!(!plan.live);

    // The same approval seen by another node: nothing more.
    assert!(
        !google
            .record_campaign(ALICE, SHOP, created("11", START_PAUSED))
            .await
            .unwrap()
    );
    let again = google
        .load_campaign(SHOP, CAMPAIGN_SHOPPING)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(again.aggregate_version, campaign.aggregate_version);
    // The other kind has a stream of its own.
    assert!(
        google
            .load_campaign(SHOP, CAMPAIGN_BRAND)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn what_google_ads_would_not_call_a_campaign_is_refused() {
    let google = command().await;
    let mut unnamed = created("11", START_PAUSED);
    unnamed.campaign_id = "customers/1/campaigns/11".into();
    assert!(matches!(
        google.record_campaign(ALICE, SHOP, unnamed).await,
        Err(GoogleError::InvalidCampaign)
    ));
    let mut bid = created("11", START_PAUSED);
    bid.settings[2].1 = "12".into();
    assert!(matches!(
        google.record_campaign(ALICE, SHOP, bid).await,
        Err(GoogleError::BidOverBudget)
    ));
    let mut kind = created("11", START_PAUSED);
    kind.kind = "display".into();
    assert!(matches!(
        google.record_campaign(ALICE, SHOP, kind).await,
        Err(GoogleError::InvalidCampaign)
    ));
}

#[tokio::test]
async fn a_campaign_created_live_runs_from_the_start() {
    let google = command().await;
    google
        .record_campaign(ALICE, SHOP, created("11", START_LIVE))
        .await
        .unwrap();
    let campaign = google
        .load_campaign(SHOP, CAMPAIGN_SHOPPING)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(campaign.live_at, Some(campaign.created_at));
    assert_eq!(
        campaign.settles_at(),
        Some(campaign.created_at + SETTLE_DAYS * 86_400)
    );
    assert!(matches!(
        google.request_live(ALICE, SHOP, CAMPAIGN_SHOPPING).await,
        Err(GoogleError::CampaignLive)
    ));
    // Seen showing ads by the daily read: it runs already.
    assert!(
        !google
            .record_live(GOOGLE_ACTOR, SHOP, CAMPAIGN_SHOPPING, "11", 1)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn a_paused_campaign_is_asked_to_run_then_runs_or_says_why_not() {
    let google = command().await;
    assert!(matches!(
        google.request_live(ALICE, SHOP, CAMPAIGN_SHOPPING).await,
        Err(GoogleError::NoCampaign)
    ));
    google
        .record_campaign(ALICE, SHOP, created("11", START_PAUSED))
        .await
        .unwrap();
    let load = async || {
        google
            .load_campaign(SHOP, CAMPAIGN_SHOPPING)
            .await
            .unwrap()
            .unwrap()
    };
    // Nobody asked: a failure has nothing to answer.
    google
        .record_live_failure(GOOGLE_ACTOR, SHOP, CAMPAIGN_SHOPPING, "late")
        .await
        .unwrap();
    assert_eq!(load().await.live_error, None);

    google
        .request_live(ALICE, SHOP, CAMPAIGN_SHOPPING)
        .await
        .unwrap();
    let asked = load().await;
    assert!(asked.live_requested);
    assert_eq!(asked.live_requested_by, ALICE);
    // Asked twice: once.
    google
        .request_live(ALICE, SHOP, CAMPAIGN_SHOPPING)
        .await
        .unwrap();
    assert_eq!(load().await.aggregate_version, asked.aggregate_version);

    google
        .record_live_failure(
            GOOGLE_ACTOR,
            SHOP,
            CAMPAIGN_SHOPPING,
            " Billing is not set up. ",
        )
        .await
        .unwrap();
    let failed = load().await;
    assert!(!failed.live_requested);
    assert_eq!(failed.live_error.as_deref(), Some("Billing is not set up."));

    google
        .request_live(ALICE, SHOP, CAMPAIGN_SHOPPING)
        .await
        .unwrap();
    assert_eq!(load().await.live_error, None);
    // Another campaign's number is not this one.
    assert!(
        !google
            .record_live(ALICE, SHOP, CAMPAIGN_SHOPPING, "99", 1)
            .await
            .unwrap()
    );
    let at = asked.created_at + 3_600;
    assert!(
        google
            .record_live(ALICE, SHOP, CAMPAIGN_SHOPPING, "11", at)
            .await
            .unwrap()
    );
    let live = load().await;
    assert!(!live.live_requested);
    assert_eq!(live.live_at, Some(at));
    assert!(
        !google
            .record_live(ALICE, SHOP, CAMPAIGN_SHOPPING, "11", at + 1)
            .await
            .unwrap()
    );
}

/// A day read by the daily pass may start before the campaign was made
/// (the account's midnight): it did not run before it existed.
#[tokio::test]
async fn a_campaign_never_ran_before_it_was_created() {
    let google = command().await;
    google
        .record_campaign(ALICE, SHOP, created("11", START_PAUSED))
        .await
        .unwrap();
    google
        .record_live(GOOGLE_ACTOR, SHOP, CAMPAIGN_SHOPPING, "11", 1_000)
        .await
        .unwrap();
    let campaign = google
        .load_campaign(SHOP, CAMPAIGN_SHOPPING)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(campaign.live_at, Some(campaign.created_at));
}

/// A shop that moved to another Google Ads account has its next campaign
/// stand in place of the first.
#[tokio::test]
async fn another_accounts_campaign_replaces_the_one_before() {
    let google = command().await;
    google
        .record_campaign(ALICE, SHOP, created("11", START_LIVE))
        .await
        .unwrap();
    let mut other = created("77", START_PAUSED);
    other.customer_id = "9876543210".into();
    assert!(google.record_campaign(ALICE, SHOP, other).await.unwrap());
    let campaign = google
        .load_campaign(SHOP, CAMPAIGN_SHOPPING)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(campaign.customer_id, "9876543210");
    assert_eq!(campaign.campaign_id, "77");
    assert_eq!(campaign.live_at, None);
}
