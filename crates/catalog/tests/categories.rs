mod common;

use common::*;
use timada_catalog::{CatalogError, CreateCategory, MAX_CATEGORY_DEPTH, SLUG_SUFFIX_LEN};

#[tokio::test]
async fn creating_a_category_derives_its_slug_and_starts_at_the_top() {
    let cmd = command().await;
    let category = create_category(&cmd, "  Cat bedding  ").await;
    assert_eq!(category.slug, "cat-bedding");
    assert_eq!(category.name, "Cat bedding");
    assert_eq!(category.description, "");
    assert_eq!(category.parent_id, None);
    assert_eq!(category.position, 0);
    assert!(!category.archived);
    assert_eq!(category.created_by, ALICE);
    assert_eq!(category.created_at, category.updated_at);

    let by_slug = cmd
        .load_category_by_slug("cat-bedding")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(by_slug, category);
    assert!(cmd.ancestors(&category.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_category_slug_is_independent_of_a_product_slug() {
    let cmd = command().await;
    let product = create_product(&cmd, "Mug").await;
    let category = create_category(&cmd, "Mug").await;
    assert_eq!(product.slug, "mug");
    assert_eq!(category.slug, "mug", "the two indexes do not collide");

    let second = create_category(&cmd, "MUG!").await;
    assert!(second.slug.starts_with("mug-"), "{}", second.slug);
    assert_eq!(second.slug.len(), "mug-".len() + SLUG_SUFFIX_LEN);
}

#[tokio::test]
async fn a_given_category_slug_is_kept_or_refused() {
    let cmd = command().await;
    let given = cmd
        .create_category(
            ALICE,
            CreateCategory {
                name: "Cat bedding".into(),
                slug: Some(" Bedding-Cat ".into()),
                parent_id: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(given.slug, "bedding-cat");

    let taken = cmd
        .create_category(
            ALICE,
            CreateCategory {
                name: "Other".into(),
                slug: Some("bedding-cat".into()),
                parent_id: None,
            },
        )
        .await;
    assert!(matches!(taken, Err(CatalogError::SlugTaken)), "{taken:?}");

    for bad in ["", "   ", &"n".repeat(81)] {
        let err = cmd
            .create_category(ALICE, new_category(bad))
            .await
            .unwrap_err();
        assert!(matches!(err, CatalogError::InvalidName), "{bad:?}: {err}");
        assert!(err.is_user_facing());
    }
}

#[tokio::test]
async fn categories_nest_three_levels_deep_and_no_further() {
    let cmd = command().await;
    let bedding = create_category(&cmd, "Bedding").await;
    let mats = create_child(&cmd, "Mats", &bedding.id).await;
    let cooling = create_child(&cmd, "Cooling mats", &mats.id).await;
    assert_eq!(cooling.parent_id.as_deref(), Some(mats.id.as_str()));
    assert_eq!(
        cmd.ancestors(&cooling.id)
            .await
            .unwrap()
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["Bedding", "Mats"],
        "ancestors read top-level first"
    );
    assert_eq!(MAX_CATEGORY_DEPTH, 3);

    let too_deep = cmd
        .create_category(
            ALICE,
            CreateCategory {
                name: "Deeper".into(),
                slug: None,
                parent_id: Some(cooling.id.clone()),
            },
        )
        .await;
    assert!(
        matches!(too_deep, Err(CatalogError::CategoryTooDeep)),
        "{too_deep:?}"
    );

    let unknown = cmd
        .create_category(
            ALICE,
            CreateCategory {
                name: "Nowhere".into(),
                slug: None,
                parent_id: Some("no-such-category".into()),
            },
        )
        .await;
    assert!(
        matches!(unknown, Err(CatalogError::CategoryNotFound)),
        "{unknown:?}"
    );
}

#[tokio::test]
async fn editing_a_category_is_idempotent() {
    let cmd = command().await;
    let category = create_category(&cmd, "Bedding").await;
    cmd.rename_category(ALICE, &category.id, " Cat bedding ")
        .await
        .unwrap();
    cmd.describe_category(ALICE, &category.id, "Where a cat sleeps.")
        .await
        .unwrap();
    cmd.position_category(ALICE, &category.id, 3).await.unwrap();
    let edited = cmd.load_category(&category.id).await.unwrap().unwrap();
    assert_eq!(edited.name, "Cat bedding");
    assert_eq!(edited.description, "Where a cat sleeps.");
    assert_eq!(edited.position, 3);
    assert_eq!(edited.slug, "bedding", "renaming leaves the slug alone");

    let version = edited.aggregate_version;
    cmd.rename_category(ALICE, &category.id, "Cat bedding")
        .await
        .unwrap();
    cmd.describe_category(ALICE, &category.id, "Where a cat sleeps.")
        .await
        .unwrap();
    cmd.position_category(ALICE, &category.id, 3).await.unwrap();
    let again = cmd.load_category(&category.id).await.unwrap().unwrap();
    assert_eq!(again.aggregate_version, version, "no event for no change");

    let long = "d".repeat(2001);
    let err = cmd
        .describe_category(ALICE, &category.id, &long)
        .await
        .unwrap_err();
    assert!(matches!(err, CatalogError::InvalidDescription), "{err}");
    let missing = cmd.rename_category(ALICE, "nope", "Name").await;
    assert!(
        matches!(missing, Err(CatalogError::CategoryNotFound)),
        "{missing:?}"
    );
}

#[tokio::test]
async fn moving_a_category_refuses_loops_and_depth() {
    let cmd = command().await;
    let bedding = create_category(&cmd, "Bedding").await;
    let mats = create_child(&cmd, "Mats", &bedding.id).await;
    let cooling = create_child(&cmd, "Cooling mats", &mats.id).await;
    let toys = create_category(&cmd, "Toys").await;

    let under_itself = cmd
        .move_category(ALICE, &bedding.id, Some(&bedding.id))
        .await;
    assert!(
        matches!(under_itself, Err(CatalogError::CategoryCycle)),
        "{under_itself:?}"
    );
    let under_child = cmd.move_category(ALICE, &bedding.id, Some(&mats.id)).await;
    assert!(
        matches!(under_child, Err(CatalogError::CategoryCycle)),
        "{under_child:?}"
    );
    let under_grandchild = cmd
        .move_category(ALICE, &bedding.id, Some(&cooling.id))
        .await;
    assert!(
        matches!(under_grandchild, Err(CatalogError::CategoryCycle)),
        "{under_grandchild:?}"
    );

    // Toys is at the top, so `mats` would put `cooling`'s level at four.
    let too_deep = cmd.move_category(ALICE, &toys.id, Some(&cooling.id)).await;
    assert!(
        matches!(too_deep, Err(CatalogError::CategoryTooDeep)),
        "{too_deep:?}"
    );

    cmd.move_category(ALICE, &cooling.id, Some(&toys.id))
        .await
        .unwrap();
    let moved = cmd.load_category(&cooling.id).await.unwrap().unwrap();
    assert_eq!(moved.parent_id.as_deref(), Some(toys.id.as_str()));

    cmd.move_category(ALICE, &cooling.id, None).await.unwrap();
    let at_top = cmd.load_category(&cooling.id).await.unwrap().unwrap();
    assert_eq!(at_top.parent_id, None);
    let version = at_top.aggregate_version;
    cmd.move_category(ALICE, &cooling.id, None).await.unwrap();
    assert_eq!(
        cmd.load_category(&cooling.id)
            .await
            .unwrap()
            .unwrap()
            .aggregate_version,
        version,
        "the same parent writes nothing"
    );
}

#[tokio::test]
async fn an_archived_category_leaves_its_slug_and_takes_no_edits() {
    let cmd = command().await;
    let first = create_category(&cmd, "Bedding").await;
    cmd.archive_category(ALICE, &first.id).await.unwrap();
    let archived = cmd.load_category(&first.id).await.unwrap().unwrap();
    assert!(archived.archived);
    assert!(
        cmd.load_category_by_slug("bedding")
            .await
            .unwrap()
            .is_none(),
        "the index entry is released"
    );

    let second = create_category(&cmd, "Bedding").await;
    assert_eq!(second.slug, "bedding");
    assert_ne!(second.id, first.id);

    cmd.archive_category(ALICE, &first.id)
        .await
        .expect("archiving twice is a no-op");
    for err in [
        cmd.rename_category(ALICE, &first.id, "Nope").await,
        cmd.describe_category(ALICE, &first.id, "Nope").await,
        cmd.move_category(ALICE, &first.id, None).await,
        cmd.position_category(ALICE, &first.id, 1).await,
        cmd.set_category_in_menu(ALICE, &first.id, false).await,
    ] {
        assert!(
            matches!(err, Err(CatalogError::CategoryArchived)),
            "{err:?}"
        );
    }
    assert!(
        cmd.load_category_by_slug("bedding")
            .await
            .unwrap()
            .is_some_and(|c| c.id == second.id),
        "the new category owns the slug"
    );
}

#[tokio::test]
async fn a_category_leaves_the_menu_and_comes_back() {
    let cmd = command().await;
    let category = create_category(&cmd, "Bedding").await;
    assert!(!category.hidden_from_menu, "a new category is in the menu");

    let version = category.aggregate_version;
    cmd.set_category_in_menu(ALICE, &category.id, true)
        .await
        .unwrap();
    let same = cmd.load_category(&category.id).await.unwrap().unwrap();
    assert_eq!(same.aggregate_version, version, "already shown: no event");

    cmd.set_category_in_menu(ALICE, &category.id, false)
        .await
        .unwrap();
    let hidden = cmd.load_category(&category.id).await.unwrap().unwrap();
    assert!(hidden.hidden_from_menu);
    cmd.set_category_in_menu(ALICE, &category.id, false)
        .await
        .unwrap();
    let again = cmd.load_category(&category.id).await.unwrap().unwrap();
    assert_eq!(
        again.aggregate_version, hidden.aggregate_version,
        "hiding twice is a no-op"
    );

    cmd.set_category_in_menu(ALICE, &category.id, true)
        .await
        .unwrap();
    let shown = cmd.load_category(&category.id).await.unwrap().unwrap();
    assert!(!shown.hidden_from_menu);
}
