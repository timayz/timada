mod common;

use common::*;
use evento::{AggregateEvent, Executor, ProjectionAggregate};
use timada_catalog::aggregator::Claimed;
use timada_catalog::{
    CatalogError, CreateCategory, CreateProduct, DefineVariant, OptionSelection, product_slug_id,
};
use timada_core::claim::CLAIM_GRACE_SECS;

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

fn pick(option_id: &str, value: &str) -> OptionSelection {
    OptionSelection {
        option_id: option_id.to_owned(),
        value: value.to_owned(),
    }
}

/// A product named `name`, varying by colour, with a variant for each
/// `(colour, code)`; its id and the variants' ids, in order.
async fn with_codes(
    cmd: &timada_catalog::Command<evento::Turso>,
    name: &str,
    codes: &[(&str, &str)],
) -> (String, Vec<String>) {
    let product = create_product(cmd, name).await;
    let colours = codes.iter().map(|(colour, _)| colour.to_string()).collect();
    let colour = cmd
        .define_option(ALICE, &product.id, "Colour", colours)
        .await
        .unwrap();
    let mut variants = Vec::new();
    for (value, sku) in codes {
        let variant = cmd
            .define_variant(
                ALICE,
                &product.id,
                DefineVariant {
                    selections: vec![pick(&colour, value)],
                    sku: Some(sku.to_string()),
                    amount_cents: 1250,
                    currency: "EUR".into(),
                    image_id: None,
                },
            )
            .await
            .unwrap();
        variants.push(variant);
    }
    (product.id, variants)
}

fn given(name: &str, slug: &str) -> CreateProduct {
    CreateProduct {
        name: name.to_owned(),
        slug: Some(slug.to_owned()),
    }
}

#[tokio::test]
async fn what_an_archived_product_held_is_claimed_at_once() {
    let cmd = command().await;
    let (mug, _) = with_codes(&cmd, "Blue Mug", &[("Blue", "MUG-1")]).await;
    cmd.archive_product(ALICE, &mug).await.unwrap();
    assert!(
        cmd.load_slug_index("blue-mug").await.unwrap().is_some(),
        "the entry waits for the release subscription"
    );

    // No release has run: the claims take the values over.
    let cup = cmd
        .create_product(ALICE, given("Blue Cup", "blue-mug"))
        .await
        .expect("the slug of an archived product is free");
    assert_eq!(cup.slug, "blue-mug");
    let (other, _) = with_codes(&cmd, "Tea Cup", &[("Blue", "mug-1")]).await;
    let index = cmd.load_sku_index("MUG-1").await.unwrap().unwrap();
    assert_eq!(
        index.product_id, other,
        "the code moved to the live product"
    );

    // The release that comes later leaves the new holders alone.
    run_releases(&cmd).await;
    assert_eq!(
        cmd.load_slug_index("blue-mug")
            .await
            .unwrap()
            .unwrap()
            .product_id,
        cup.id
    );
    assert_eq!(
        cmd.load_sku_index("mug-1")
            .await
            .unwrap()
            .unwrap()
            .product_id,
        other
    );
}

#[tokio::test]
async fn a_retried_archive_leaves_nothing_claimed_once_the_releases_ran() {
    let cmd = command().await;
    let (mug, _) = with_codes(&cmd, "Blue Mug", &[("Blue", "MUG-1"), ("Red", "MUG-2")]).await;
    cmd.archive_product(ALICE, &mug).await.unwrap();
    // Asked again, as after a node died: nothing more is written.
    cmd.archive_product(ALICE, &mug).await.unwrap();

    run_releases(&cmd).await;
    assert!(cmd.load_slug_index("blue-mug").await.unwrap().is_none());
    assert!(cmd.load_sku_index("MUG-1").await.unwrap().is_none());
    assert!(cmd.load_sku_index("MUG-2").await.unwrap().is_none());

    // Deleting it after is one more event to release for: nothing is left.
    cmd.delete_product(ALICE, &mug).await.unwrap();
    run_releases(&cmd).await;
    assert!(cmd.load_slug_index("blue-mug").await.unwrap().is_none());
}

#[tokio::test]
async fn a_deleted_product_frees_everything_it_held() {
    let cmd = command().await;
    let (mug, _) = with_codes(&cmd, "Blue Mug", &[("Blue", "MUG-1")]).await;
    cmd.delete_product(ALICE, &mug).await.unwrap();
    run_releases(&cmd).await;
    assert!(cmd.load_slug_index("blue-mug").await.unwrap().is_none());
    assert!(cmd.load_sku_index("MUG-1").await.unwrap().is_none());
}

#[tokio::test]
async fn a_rename_frees_the_old_slug() {
    let cmd = command().await;
    let mug = create_product(&cmd, "Blue Mug").await;
    cmd.rename_product(ALICE, &mug.id, "Tea Cup").await.unwrap();
    assert!(
        cmd.load_product_by_slug("blue-mug")
            .await
            .unwrap()
            .is_none(),
        "the old slug stops answering before its entry is released"
    );

    run_releases(&cmd).await;
    assert!(cmd.load_slug_index("blue-mug").await.unwrap().is_none());
    assert_eq!(
        cmd.load_slug_index("tea-cup")
            .await
            .unwrap()
            .unwrap()
            .product_id,
        mug.id
    );
    let other = cmd
        .create_product(ALICE, given("Blue Mug", "blue-mug"))
        .await
        .expect("the old slug is free");
    assert_eq!(other.slug, "blue-mug");
}

#[tokio::test]
async fn a_slug_taken_back_before_its_release_stays_claimed() {
    let cmd = command().await;
    let mug = create_product(&cmd, "Blue Mug").await;
    cmd.rename_product(ALICE, &mug.id, "Tea Cup").await.unwrap();
    cmd.rename_product(ALICE, &mug.id, "Blue Mug")
        .await
        .unwrap();
    let back = cmd.load_product(&mug.id).await.unwrap().unwrap();
    assert_eq!(back.slug, "blue-mug", "its own claim is renewed");

    // The release of the first rename comes after the product took the
    // slug back: it frees what the product left, not what it holds.
    run_releases(&cmd).await;
    let index = cmd.load_slug_index("blue-mug").await.unwrap().unwrap();
    assert_eq!(index.product_id, mug.id);
    assert!(cmd.load_slug_index("tea-cup").await.unwrap().is_none());
    let other = create_product(&cmd, "Blue Mug").await;
    assert_ne!(other.slug, "blue-mug", "the slug is still taken");
}

#[tokio::test]
async fn a_slug_being_taken_back_is_not_released_under_it() {
    let cmd = command().await;
    let mug = create_product(&cmd, "Blue Mug").await;
    cmd.rename_product(ALICE, &mug.id, "Tea Cup").await.unwrap();
    // The product claims its old slug back at its version 2, as a rename
    // does before it commits; the rename has not landed yet.
    let index = cmd.load_slug_index("blue-mug").await.unwrap().unwrap();
    index
        .write()
        .unwrap()
        .event(&Claimed {
            product_id: mug.id.clone(),
            product_version: 2,
        })
        .tenant(TENANT)
        .commit(cmd.executor())
        .await
        .unwrap();

    run_releases(&cmd).await;
    let index = cmd.load_slug_index("blue-mug").await.unwrap().unwrap();
    assert_eq!(index.product_id, mug.id);
    assert_eq!(
        index.product_version, 2,
        "a claim made after the rename is not the rename's to free"
    );
}

#[tokio::test]
async fn a_slug_a_product_is_moving_to_is_held_until_it_writes_past_the_claim() {
    let cmd = command().await;
    let mug = create_product(&cmd, "Blue Mug").await;
    // The product claims a new slug at its version 1, as a rename does
    // before it commits.
    evento::append(product_slug_id("tea-cup"))
        .original_version(0)
        .event(&Claimed {
            product_id: mug.id.clone(),
            product_version: 1,
        })
        .tenant(TENANT)
        .commit(cmd.executor())
        .await
        .unwrap();
    let taken = cmd.create_product(ALICE, given("Tea Cup", "tea-cup")).await;
    assert!(matches!(taken, Err(CatalogError::SlugTaken)), "{taken:?}");

    // Any write of the product past the claim settles it: the rename lost,
    // and the slug is free.
    cmd.describe_product(ALICE, &mug.id, "Holds tea.")
        .await
        .unwrap();
    let cup = cmd
        .create_product(ALICE, given("Tea Cup", "tea-cup"))
        .await
        .expect("a claim its product wrote past is free");
    assert_eq!(cup.slug, "tea-cup");
}

#[tokio::test]
async fn a_code_changed_or_removed_is_freed_and_one_taken_back_is_kept() {
    let cmd = command().await;
    let (mug, variants) =
        with_codes(&cmd, "Blue Mug", &[("Blue", "MUG-1"), ("Red", "MUG-2")]).await;
    // Changed away and back: the variant keeps its code.
    cmd.change_variant_sku(ALICE, &mug, &variants[0], Some("MUG-9"))
        .await
        .unwrap();
    cmd.change_variant_sku(ALICE, &mug, &variants[0], Some("MUG-1"))
        .await
        .unwrap();
    // Only the case changes: the same code, still the variant's.
    cmd.change_variant_sku(ALICE, &mug, &variants[0], Some("mug-1"))
        .await
        .unwrap();
    cmd.remove_variant(ALICE, &mug, &variants[1]).await.unwrap();

    run_releases(&cmd).await;
    let kept = cmd.load_sku_index("MUG-1").await.unwrap().unwrap();
    assert_eq!(kept.variant_id, variants[0]);
    assert!(cmd.load_sku_index("MUG-9").await.unwrap().is_none());
    assert!(
        cmd.load_sku_index("MUG-2").await.unwrap().is_none(),
        "the removed variant's code is free"
    );
}

#[tokio::test]
async fn a_slug_whose_product_was_never_written_is_held_while_the_claim_is_fresh() {
    let cmd = command().await;
    // The product may as well be committing right now.
    let claimed = Claimed {
        product_id: "never-written".to_owned(),
        product_version: 0,
    };
    claimed_ago(cmd.executor(), &product_slug_id("blue-mug"), &claimed, 1).await;

    let taken = cmd
        .create_product(ALICE, given("Blue Mug", "blue-mug"))
        .await;
    assert!(matches!(taken, Err(CatalogError::SlugTaken)), "{taken:?}");
    let derived = create_product(&cmd, "Blue Mug").await;
    assert_ne!(derived.slug, "blue-mug");
}

#[tokio::test]
async fn a_slug_whose_product_was_never_written_is_free_once_the_claim_is_stale() {
    let cmd = command().await;
    let claimed = Claimed {
        product_id: "never-written".to_owned(),
        product_version: 0,
    };
    let age = CLAIM_GRACE_SECS + 1;
    claimed_ago(cmd.executor(), &product_slug_id("blue-mug"), &claimed, age).await;

    let mug = cmd
        .create_product(ALICE, given("Blue Mug", "blue-mug"))
        .await
        .expect("a node died between the claim and the product: the slug is free");
    assert_eq!(mug.slug, "blue-mug");
}

#[tokio::test]
async fn an_archived_category_frees_its_slug() {
    let cmd = command().await;
    let bedding = create_category(&cmd, "Bedding").await;
    cmd.archive_category(ALICE, &bedding.id).await.unwrap();
    cmd.archive_category(ALICE, &bedding.id).await.unwrap();

    run_releases(&cmd).await;
    assert!(
        cmd.load_category_slug_index("bedding")
            .await
            .unwrap()
            .is_none()
    );
    let again = cmd
        .create_category(
            ALICE,
            CreateCategory {
                name: "Bedding".into(),
                slug: Some("bedding".into()),
                parent_id: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(again.slug, "bedding");
}
