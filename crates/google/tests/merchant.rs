mod common;

use common::*;
use timada_core::db::Db;
use timada_core::{Claimant, Sending};
use timada_google::aggregator::{MerchantAccountIssue, MerchantIssue, MerchantProductRow};
use timada_google::{
    ACCESS_MERCHANT_CENTER, ACCESS_SEARCH_CONSOLE, CHECK_EVERY_SECS, Connect, DueMerchant,
    GOOGLE_ACTOR, GoogleError, MAX_MERCHANT_PRODUCT_ROWS, MERCHANT_ABSENT, MERCHANT_ACCEPTED,
    MERCHANT_PENDING, MERCHANT_REFUSED, MerchantAccount, MerchantRecorded, MerchantReport,
    merchant_account_issues, merchant_account_of, merchant_due, merchant_issues,
    merchant_issues_of, merchant_problems, merchant_product, merchant_standing,
    read_model_subscription,
};

const ACCOUNT: &str = "5550001";
const OTHER: &str = "5550002";

async fn catch_up(executor: &evento::Turso, db: &Db) {
    read_model_subscription::<evento::Turso>(db.clone())
        .run_once(executor)
        .await
        .unwrap();
}

fn account(account_id: &str) -> MerchantAccount {
    MerchantAccount {
        account_id: account_id.to_owned(),
        name: "Acme".to_owned(),
    }
}

/// What `account` granted: Search Console and Merchant Center.
fn wide(account: &str, token: &[u8]) -> Connect {
    Connect {
        refresh_token_sealed: token.to_vec(),
        account: account.to_owned(),
        accesses: vec![
            ACCESS_MERCHANT_CENTER.to_owned(),
            ACCESS_SEARCH_CONSOLE.to_owned(),
        ],
    }
}

fn product(item_id: &str, ads: &str, listings: &str) -> MerchantProductRow {
    MerchantProductRow {
        item_id: item_id.to_owned(),
        ads: ads.to_owned(),
        listings: listings.to_owned(),
        issues: Vec::new(),
    }
}

fn issue(code: &str, attribute: &str) -> MerchantIssue {
    MerchantIssue {
        code: code.to_owned(),
        severity: "refused".to_owned(),
        attribute: attribute.to_owned(),
        context: "ads".to_owned(),
        description: "Image too small".to_owned(),
        detail: "Use an image of at least 100 by 100 pixels".to_owned(),
        documentation: "https://support.google.com/merchants/answer/6324350".to_owned(),
    }
}

fn report(products: Vec<MerchantProductRow>) -> MerchantReport {
    MerchantReport {
        products,
        account_issues: Vec::new(),
    }
}

#[tokio::test]
async fn a_shop_chooses_its_merchant_center_account() {
    let google = command().await;
    google
        .connect(ALICE, SHOP, grant("alice@example.com", b"sealed-1"))
        .await
        .unwrap();
    assert!(matches!(
        google
            .choose_merchant_account(ALICE, SHOP, account(ACCOUNT))
            .await,
        Err(GoogleError::NoMerchantCenter)
    ));
    google
        .connect(ALICE, SHOP, wide("alice@example.com", b"sealed-2"))
        .await
        .unwrap();
    for bad in ["", "accounts/5550001", "55 50"] {
        assert!(matches!(
            google
                .choose_merchant_account(ALICE, SHOP, account(bad))
                .await,
            Err(GoogleError::InvalidMerchantAccount)
        ));
    }
    google
        .choose_merchant_account(ALICE, SHOP, account(ACCOUNT))
        .await
        .unwrap();
    google
        .choose_merchant_account(ALICE, SHOP, account(ACCOUNT))
        .await
        .unwrap();
    let connection = google.load_connection(SHOP).await.unwrap().unwrap();
    assert_eq!(connection.aggregate_version, 3);
    assert_eq!(connection.merchant, Some(account(ACCOUNT)));
    // The site and the account are each their own choice.
    assert_eq!(connection.site, None);

    // The same Google account again keeps it; another does not.
    google
        .connect(ALICE, SHOP, wide("alice@example.com", b"sealed-3"))
        .await
        .unwrap();
    assert!(
        google
            .load_connection(SHOP)
            .await
            .unwrap()
            .unwrap()
            .merchant
            .is_some()
    );
    google
        .connect(ALICE, SHOP, wide("bob@example.com", b"sealed-4"))
        .await
        .unwrap();
    assert_eq!(
        google
            .load_connection(SHOP)
            .await
            .unwrap()
            .unwrap()
            .merchant,
        None
    );
}

#[tokio::test]
async fn a_read_is_recorded_when_something_moved_and_says_what_turned_for_the_worse() {
    let (executor, db) = executor_with_pool().await;
    let google = timada_google::Command(executor.clone());
    let record = |report: MerchantReport| {
        let google = &google;
        async move {
            google
                .record_merchant_read(GOOGLE_ACTOR, SHOP, ACCOUNT, report)
                .await
                .unwrap()
        }
    };
    let mut small = product("01TREE", MERCHANT_REFUSED, MERCHANT_ACCEPTED);
    small.issues.push(issue("image_too_small", "image_link"));
    let first = report(vec![
        product("01POST", MERCHANT_ACCEPTED, MERCHANT_ACCEPTED),
        small.clone(),
        product("01BOWL", MERCHANT_PENDING, MERCHANT_PENDING),
        product("01BED", MERCHANT_ACCEPTED, MERCHANT_ABSENT),
    ]);
    // A first read turns nothing: there is no before.
    assert_eq!(
        record(first.clone()).await,
        MerchantRecorded {
            written: true,
            newly_refused: 0,
            new_account_issues: 0
        }
    );
    // Read the same, in another order: left as it is.
    let mut same = first.clone();
    same.products.reverse();
    assert_eq!(record(same).await, MerchantRecorded::default());

    catch_up(&executor, &db).await;
    let standing = merchant_standing(&db, SHOP, ACCOUNT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(standing.products(), 4);
    assert_eq!(
        (
            standing.ads.accepted,
            standing.ads.pending,
            standing.ads.refused
        ),
        (2, 1, 1)
    );
    assert_eq!(
        (
            standing.listings.accepted,
            standing.listings.pending,
            standing.listings.absent
        ),
        (2, 1, 1)
    );
    assert!(!standing.all_accepted_for_ads());
    assert!(standing.read_at > 0);
    assert_eq!(merchant_standing(&db, SHOP, OTHER).await.unwrap(), None);

    // The ones to look at, the refused first.
    let problems = merchant_problems(&db, SHOP, ACCOUNT, 10).await.unwrap();
    let read: Vec<(&str, i64)> = problems
        .iter()
        .map(|row| (row.item_id.as_str(), row.rank))
        .collect();
    assert_eq!(read, vec![("01TREE", 0), ("01BOWL", 1)]);
    let issues = merchant_issues_of(&db, SHOP, ACCOUNT, "01TREE")
        .await
        .unwrap();
    assert_eq!(issues.len(), 1);
    assert_eq!(
        (issues[0].code.as_str(), issues[0].attribute.as_str()),
        ("image_too_small", "image_link")
    );
    assert_eq!(merchant_issues(&db, SHOP, ACCOUNT).await.unwrap(), issues);
    let post = merchant_product(&db, SHOP, ACCOUNT, "01POST")
        .await
        .unwrap()
        .unwrap();
    assert_eq!((post.rank, post.ads.as_str()), (3, MERCHANT_ACCEPTED));
    assert_eq!(
        merchant_product(&db, SHOP, ACCOUNT, "01NONE")
            .await
            .unwrap(),
        None
    );

    // The tree's picture was fixed; the post is refused now, and the
    // account has something held against it.
    let mut second = report(vec![
        product("01POST", MERCHANT_REFUSED, MERCHANT_REFUSED),
        product("01TREE", MERCHANT_ACCEPTED, MERCHANT_ACCEPTED),
        product("01BOWL", MERCHANT_ACCEPTED, MERCHANT_ACCEPTED),
        product("01BED", MERCHANT_ACCEPTED, MERCHANT_ABSENT),
        // New and refused at once: it never was accepted.
        product("01NEW", MERCHANT_REFUSED, MERCHANT_REFUSED),
    ]);
    second.account_issues.push(MerchantAccountIssue {
        title: "Store ownership not verified".to_owned(),
        severity: "CRITICAL".to_owned(),
        detail: "Verify your store".to_owned(),
        documentation: "https://support.google.com/merchants/answer/176793".to_owned(),
    });
    assert_eq!(
        record(second.clone()).await,
        MerchantRecorded {
            written: true,
            newly_refused: 1,
            new_account_issues: 1
        }
    );
    let feed = google
        .load_merchant_feed(SHOP, ACCOUNT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(feed.aggregate_version, 2);
    // A severity that is not one of the three reads as the mildest.
    assert_eq!(feed.last.account_issues[0].severity, "suggestion");
    catch_up(&executor, &db).await;
    // The read replaced the one before, whole.
    assert!(
        merchant_issues(&db, SHOP, ACCOUNT)
            .await
            .unwrap()
            .is_empty()
    );
    let problems = merchant_problems(&db, SHOP, ACCOUNT, 10).await.unwrap();
    let read: Vec<&str> = problems.iter().map(|row| row.item_id.as_str()).collect();
    assert_eq!(read, vec!["01NEW", "01POST"]);
    let held = merchant_account_issues(&db, SHOP, ACCOUNT).await.unwrap();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].title, "Store ownership not verified");

    // The same issue the next day is no news.
    let mut third = second.clone();
    third.products[0] = product("01POST", MERCHANT_ACCEPTED, MERCHANT_ACCEPTED);
    third.products.pop();
    assert_eq!(
        record(third).await,
        MerchantRecorded {
            written: true,
            newly_refused: 0,
            new_account_issues: 0
        }
    );
    catch_up(&executor, &db).await;
    let standing = merchant_standing(&db, SHOP, ACCOUNT)
        .await
        .unwrap()
        .unwrap();
    assert!(standing.all_accepted_for_ads());
    assert_eq!(standing.listings.absent, 1);

    assert!(matches!(
        google
            .record_merchant_read(GOOGLE_ACTOR, SHOP, "accounts/1", report(Vec::new()))
            .await,
        Err(GoogleError::InvalidMerchantAccount)
    ));
}

#[tokio::test]
async fn a_large_feed_keeps_what_needs_looking_at_and_counts_the_rest() {
    let (executor, db) = executor_with_pool().await;
    let google = timada_google::Command(executor.clone());
    let mut products: Vec<MerchantProductRow> = (0..MAX_MERCHANT_PRODUCT_ROWS + 20)
        .map(|n| {
            product(
                &format!("item-{n:05}"),
                MERCHANT_ACCEPTED,
                MERCHANT_ACCEPTED,
            )
        })
        .collect();
    // The last one, by id, is refused: it is kept all the same.
    products.last_mut().unwrap().ads = MERCHANT_REFUSED.to_owned();
    google
        .record_merchant_read(GOOGLE_ACTOR, SHOP, ACCOUNT, report(products.clone()))
        .await
        .unwrap();
    let feed = google
        .load_merchant_feed(SHOP, ACCOUNT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(feed.last.products.len(), MAX_MERCHANT_PRODUCT_ROWS);
    assert_eq!(feed.more, 20);
    assert_eq!(
        feed.last.products[0].item_id,
        products.last().unwrap().item_id
    );
    catch_up(&executor, &db).await;
    let standing = merchant_standing(&db, SHOP, ACCOUNT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(standing.products(), MAX_MERCHANT_PRODUCT_ROWS as i64 + 20);
    assert_eq!(standing.ads.refused, 1);

    // One of the accepted ones left out is refused: it was accepted before.
    products[MAX_MERCHANT_PRODUCT_ROWS + 10].ads = MERCHANT_REFUSED.to_owned();
    let recorded = google
        .record_merchant_read(GOOGLE_ACTOR, SHOP, ACCOUNT, report(products))
        .await
        .unwrap();
    assert_eq!(recorded.newly_refused, 1);
}

#[tokio::test]
async fn a_pass_says_when_the_next_one_is_due() {
    let (executor, db) = executor_with_pool().await;
    let google = timada_google::Command(executor.clone());
    let due = |now: i64| {
        let db = db.clone();
        async move { merchant_due(&db, now, "", 100).await.unwrap() }
    };
    let shop = |account_id: &str| DueMerchant {
        tenant_id: SHOP.to_owned(),
        account_id: account_id.to_owned(),
    };
    google
        .connect(ALICE, SHOP, wide("alice@example.com", b"sealed-1"))
        .await
        .unwrap();
    catch_up(&executor, &db).await;
    assert!(due(i64::MAX).await.is_empty());
    assert_eq!(merchant_account_of(&db, SHOP).await.unwrap(), None);
    google
        .choose_merchant_account(ALICE, SHOP, account(ACCOUNT))
        .await
        .unwrap();
    catch_up(&executor, &db).await;
    assert_eq!(due(0).await, vec![shop(ACCOUNT)]);
    assert_eq!(
        merchant_account_of(&db, SHOP).await.unwrap().as_deref(),
        Some(ACCOUNT)
    );

    google
        .record_merchant_check(GOOGLE_ACTOR, SHOP, ACCOUNT, 4)
        .await
        .unwrap();
    catch_up(&executor, &db).await;
    let watch = google.load_merchant_watch(SHOP).await.unwrap().unwrap();
    assert_eq!(watch.products, 4);
    assert_eq!(watch.next_at, watch.checked_at + CHECK_EVERY_SECS);
    assert!(due(watch.next_at as i64 - 1).await.is_empty());
    assert_eq!(due(watch.next_at as i64).await, vec![shop(ACCOUNT)]);
    let checked_at = watch.checked_at;

    google
        .record_merchant_failure(GOOGLE_ACTOR, SHOP, ACCOUNT, " Google is unavailable: 503 ")
        .await
        .unwrap();
    google
        .record_merchant_failure(GOOGLE_ACTOR, SHOP, ACCOUNT, "again")
        .await
        .unwrap();
    let watch = google.load_merchant_watch(SHOP).await.unwrap().unwrap();
    assert_eq!(watch.attempts, 2);
    assert_eq!(watch.last_error.as_deref(), Some("again"));
    assert_eq!(watch.checked_at, checked_at);

    // Another account: due at once, and a failure on it starts from one.
    google
        .choose_merchant_account(ALICE, SHOP, account(OTHER))
        .await
        .unwrap();
    catch_up(&executor, &db).await;
    assert_eq!(due(0).await, vec![shop(OTHER)]);
    google
        .record_merchant_failure(GOOGLE_ACTOR, SHOP, OTHER, "refused")
        .await
        .unwrap();
    let watch = google.load_merchant_watch(SHOP).await.unwrap().unwrap();
    assert_eq!((watch.attempts, watch.checked_at), (1, 0));

    google.disconnect(ALICE, SHOP).await.unwrap();
    catch_up(&executor, &db).await;
    assert!(due(i64::MAX).await.is_empty());
}

#[tokio::test]
async fn a_day_s_notice_is_claimed_by_one_node_and_recorded_once() {
    let google = command().await;
    let day = "2026-11-24";
    let now = 1_795_500_000;
    let node = |node: &'static str| Claimant { node, now };
    assert!(
        google
            .load_merchant_notice(SHOP, day)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        google
            .claim_merchant_notice(SHOP, day, now + 86_400, node("node-a"))
            .await
            .unwrap()
    );
    // Another node leaves it; the one that holds it may go on.
    assert!(
        !google
            .claim_merchant_notice(SHOP, day, now + 86_400, node("node-b"))
            .await
            .unwrap()
    );
    assert!(
        google
            .claim_merchant_notice(SHOP, day, now + 86_400, node("node-a"))
            .await
            .unwrap()
    );
    let notice = google
        .load_merchant_notice(SHOP, day)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (notice.tenant_id.as_str(), notice.day.as_str()),
        (SHOP, day)
    );

    // A server that does not answer: deferred, then sent.
    google
        .record_merchant_notice_not_sent(SHOP, day, "node-a", "connection refused", now)
        .await
        .unwrap();
    let notice = google
        .load_merchant_notice(SHOP, day)
        .await
        .unwrap()
        .unwrap();
    let Sending::Deferred { retry_at, .. } = notice.sending else {
        panic!("{:?}", notice.sending);
    };
    assert!(
        !google
            .claim_merchant_notice(SHOP, day, now + 86_400, node("node-b"))
            .await
            .unwrap()
    );
    assert!(
        google
            .claim_merchant_notice(
                SHOP,
                day,
                now + 86_400,
                Claimant {
                    node: "node-b",
                    now: retry_at
                }
            )
            .await
            .unwrap()
    );
    google.record_merchant_notice_sent(SHOP, day).await.unwrap();
    google.record_merchant_notice_sent(SHOP, day).await.unwrap();
    let notice = google
        .load_merchant_notice(SHOP, day)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(notice.sending, Sending::Sent);
    assert!(
        !google
            .claim_merchant_notice(SHOP, day, now + 86_400, node("node-a"))
            .await
            .unwrap()
    );
    // Another shop's day, and another day, are notices of their own.
    assert!(
        google
            .claim_merchant_notice("tenant-other", day, now + 86_400, node("node-a"))
            .await
            .unwrap()
    );
    assert!(
        google
            .claim_merchant_notice(SHOP, "2026-11-25", now + 86_400, node("node-a"))
            .await
            .unwrap()
    );
}
