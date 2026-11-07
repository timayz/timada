mod common;

use common::*;
use evento::{AggregateEvent, Executor};
use timada_core::claim::CLAIM_GRACE_SECS;
use timada_pages::aggregator::Claimed;
use timada_pages::{CreatePage, PageError, page_slug_id};

/// Writes `data` as the first event of the index stream `id`, stamped
/// `age_secs` ago: what a node that died after claiming leaves behind.
async fn claimed_ago<D>(store: &evento::Turso, id: &str, data: &D, age_secs: u64)
where
    D: AggregateEvent + bitcode::Encode,
{
    store
        .replicate(vec![evento::Event {
            id: ulid::Ulid::generate(),
            aggregate_id: id.to_owned(),
            aggregate_type: D::aggregate_type().to_owned(),
            version: 1,
            name: D::event_name().to_owned(),
            tenant: TENANT.to_owned(),
            data: bitcode::encode(data),
            timestamp: timada_core::unix_now() - age_secs,
            ..Default::default()
        }])
        .await
        .unwrap();
}

fn at(slug: &str, title: &str) -> CreatePage {
    CreatePage {
        slug: slug.to_owned(),
        ..draft(title)
    }
}

#[tokio::test]
async fn a_rename_frees_the_old_slug_and_it_stops_resolving() {
    let pages = command().await;
    let id = pages.create_page(ALICE, draft("About us")).await.unwrap();
    pages
        .rename_page(ALICE, &id, "qui-sommes-nous")
        .await
        .unwrap();
    assert!(
        pages.load_page_by_slug("about-us").await.unwrap().is_none(),
        "the old slug stops resolving before its entry is released"
    );
    assert!(
        pages
            .load_page_slug_index("about-us")
            .await
            .unwrap()
            .is_some()
    );

    run_releases(&pages).await;
    assert!(
        pages
            .load_page_slug_index("about-us")
            .await
            .unwrap()
            .is_none()
    );
    let index = pages
        .load_page_slug_index("qui-sommes-nous")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(index.page_id, id);
}

#[tokio::test]
async fn a_deleted_page_leaves_nothing_claimed_once_the_releases_ran() {
    let pages = command().await;
    let id = pages.create_page(ALICE, draft("Old terms")).await.unwrap();
    pages.delete_page(ALICE, &id).await.unwrap();
    // Its slug is free before the release runs.
    let new = pages
        .create_page(ALICE, at("old-terms", "New terms"))
        .await
        .unwrap();

    run_releases(&pages).await;
    let index = pages
        .load_page_slug_index("old-terms")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(index.page_id, new, "the new page keeps what it claimed");
    pages.delete_page(ALICE, &new).await.unwrap();
    run_releases(&pages).await;
    assert!(
        pages
            .load_page_slug_index("old-terms")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn a_slug_taken_back_before_its_release_stays_claimed() {
    let pages = command().await;
    let id = pages.create_page(ALICE, draft("About us")).await.unwrap();
    pages.rename_page(ALICE, &id, "about").await.unwrap();
    pages.rename_page(ALICE, &id, "about-us").await.unwrap();

    run_releases(&pages).await;
    let index = pages
        .load_page_slug_index("about-us")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(index.page_id, id);
    assert!(pages.load_page_slug_index("about").await.unwrap().is_none());
    let taken = pages.create_page(ALICE, draft("About us")).await;
    assert!(matches!(taken, Err(PageError::SlugTaken)), "{taken:?}");
}

#[tokio::test]
async fn a_slug_whose_page_was_never_written_is_held_while_the_claim_is_fresh() {
    let pages = command().await;
    let claimed = Claimed {
        page_id: "never-written".to_owned(),
        page_version: 0,
    };
    claimed_ago(pages.executor(), &page_slug_id("contact"), &claimed, 1).await;
    let taken = pages.create_page(ALICE, draft("Contact")).await;
    assert!(matches!(taken, Err(PageError::SlugTaken)), "{taken:?}");

    let age = CLAIM_GRACE_SECS + 1;
    claimed_ago(pages.executor(), &page_slug_id("about-us"), &claimed, age).await;
    pages
        .create_page(ALICE, draft("About us"))
        .await
        .expect("a node died between the claim and the page: the slug is free");
}
