mod common;

use common::*;
use timada_catalog::{
    CatalogError, CategoryQuestion, CreateCategory, DraftCategory, MAX_CATEGORY_BODY_LEN,
    MAX_CATEGORY_DEPTH, MAX_CATEGORY_QUESTIONS, MAX_PINNED_PRODUCTS, SLUG_SUFFIX_LEN,
};

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
async fn a_category_slug_taken_gets_a_suffix() {
    let cmd = command().await;
    let category = create_category(&cmd, "Mug").await;
    assert_eq!(category.slug, "mug");

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
async fn what_search_engines_show_of_a_category_is_its_own() {
    let cmd = command().await;
    let category = create_category(&cmd, "Bedding").await;
    assert_eq!(category.meta_title, "");
    assert_eq!(category.meta_description, "");

    cmd.set_category_seo(
        ALICE,
        &category.id,
        " Cat beds and mats | Acme ",
        "Soft beds\nfor  cats. ",
    )
    .await
    .unwrap();
    let set = cmd.load_category(&category.id).await.unwrap().unwrap();
    assert_eq!(set.meta_title, "Cat beds and mats | Acme");
    assert_eq!(set.meta_description, "Soft beds for cats.");
    assert_eq!(set.name, "Bedding");

    let version = set.aggregate_version;
    cmd.set_category_seo(
        ALICE,
        &category.id,
        "Cat beds and mats | Acme",
        "Soft beds for cats.",
    )
    .await
    .unwrap();
    let again = cmd.load_category(&category.id).await.unwrap().unwrap();
    assert_eq!(again.aggregate_version, version, "no event for no change");

    let long = cmd
        .set_category_seo(ALICE, &category.id, &"t".repeat(71), "")
        .await;
    assert!(matches!(long, Err(CatalogError::InvalidSeo)), "{long:?}");
    let missing = cmd.set_category_seo(ALICE, "nope", "Title", "").await;
    assert!(
        matches!(missing, Err(CatalogError::CategoryNotFound)),
        "{missing:?}"
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
async fn a_category_body_is_written_apart_from_its_description() {
    let cmd = command().await;
    let category = create_category(&cmd, "Toys").await;
    assert_eq!(category.body, "", "a new category has no body");

    cmd.describe_category(ALICE, &category.id, "Games for cats.")
        .await
        .unwrap();
    cmd.write_category_body(
        ALICE,
        &category.id,
        "  Why a cat plays.\n\nHow to pick a toy.  ",
    )
    .await
    .unwrap();
    let written = cmd.load_category(&category.id).await.unwrap().unwrap();
    assert_eq!(written.description, "Games for cats.");
    assert_eq!(written.body, "Why a cat plays.\n\nHow to pick a toy.");

    let version = written.aggregate_version;
    cmd.write_category_body(
        ALICE,
        &category.id,
        "Why a cat plays.\n\nHow to pick a toy.",
    )
    .await
    .unwrap();
    let again = cmd.load_category(&category.id).await.unwrap().unwrap();
    assert_eq!(again.aggregate_version, version, "no event for no change");

    let long = "b".repeat(MAX_CATEGORY_BODY_LEN + 1);
    let err = cmd
        .write_category_body(ALICE, &category.id, &long)
        .await
        .unwrap_err();
    assert!(matches!(err, CatalogError::InvalidDescription), "{err}");
    cmd.write_category_body(ALICE, &category.id, &"b".repeat(MAX_CATEGORY_BODY_LEN))
        .await
        .expect("the longest body is taken");
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
        "an archived category answers to its slug no more"
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
        cmd.write_category_body(ALICE, &first.id, "Nope").await,
        cmd.set_category_seo(ALICE, &first.id, "Nope", "").await,
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

#[tokio::test]
async fn a_category_answers_the_questions_it_is_given() {
    let cmd = command().await;
    let mats = create_category(&cmd, "Mats").await;
    let asked = |question: &str, answer: &str| CategoryQuestion {
        question: question.to_owned(),
        answer: answer.to_owned(),
    };
    cmd.set_category_questions(
        ALICE,
        &mats.id,
        &[
            asked("  Which  size?\n", " Measure the cat.\r\nAdd 10 cm. "),
            asked("Machine washable?", "Yes, at 30°."),
        ],
    )
    .await
    .unwrap();
    let category = cmd.load_category(&mats.id).await.unwrap().unwrap();
    assert_eq!(
        category.questions,
        [
            asked("Which size?", "Measure the cat.\nAdd 10 cm."),
            asked("Machine washable?", "Yes, at 30°."),
        ]
    );

    // A question without an answer, or too many of them, is refused whole.
    let empty = cmd
        .set_category_questions(ALICE, &mats.id, &[asked("Why?", "  ")])
        .await;
    assert!(
        matches!(empty, Err(CatalogError::InvalidCategoryQuestions)),
        "{empty:?}"
    );
    let many = vec![asked("Q", "A"); MAX_CATEGORY_QUESTIONS + 1];
    let refused = cmd.set_category_questions(ALICE, &mats.id, &many).await;
    assert!(
        matches!(refused, Err(CatalogError::InvalidCategoryQuestions)),
        "{refused:?}"
    );
    cmd.set_category_questions(ALICE, &mats.id, &[])
        .await
        .unwrap();
    let category = cmd.load_category(&mats.id).await.unwrap().unwrap();
    assert!(category.questions.is_empty());
}

#[tokio::test]
async fn a_category_puts_the_products_it_is_told_first() {
    let cmd = command().await;
    let mats = create_category(&cmd, "Mats").await;
    let blue = create_product(&cmd, "Blue mat").await;
    let pink = create_product(&cmd, "Pink mat").await;
    cmd.pin_category_products(
        ALICE,
        &mats.id,
        &[
            pink.id.clone(),
            blue.id.clone(),
            pink.id.clone(),
            " ".into(),
        ],
    )
    .await
    .unwrap();
    let category = cmd.load_category(&mats.id).await.unwrap().unwrap();
    assert_eq!(category.pinned, [pink.id.clone(), blue.id.clone()]);

    let unknown = cmd
        .pin_category_products(ALICE, &mats.id, &["never-was".into()])
        .await;
    assert!(
        matches!(unknown, Err(CatalogError::NotFound)),
        "{unknown:?}"
    );
    let many: Vec<String> = (0..=MAX_PINNED_PRODUCTS).map(|n| format!("p{n}")).collect();
    let refused = cmd.pin_category_products(ALICE, &mats.id, &many).await;
    assert!(
        matches!(refused, Err(CatalogError::TooManyPinnedProducts)),
        "{refused:?}"
    );
    cmd.pin_category_products(ALICE, &mats.id, &[])
        .await
        .unwrap();
    let category = cmd.load_category(&mats.id).await.unwrap().unwrap();
    assert!(category.pinned.is_empty());
}

fn draft(id: &str, name: &str, slug: &str, parent_id: Option<&str>) -> DraftCategory {
    DraftCategory {
        id: id.to_owned(),
        name: name.to_owned(),
        slug: slug.to_owned(),
        parent_id: parent_id.map(str::to_owned),
        meta_title: format!("{name} for bikers"),
        out_of_ads: false,
        position: 0,
    }
}

#[tokio::test]
async fn a_draft_category_is_made_once_under_the_id_its_maker_chose() {
    let cmd = command().await;
    let gloves = draft("01GLOVES", "Biker gloves", "biker-gloves", None);
    assert!(
        cmd.create_draft_category(ALICE, gloves.clone())
            .await
            .unwrap()
    );
    let made = cmd.load_category("01GLOVES").await.unwrap().unwrap();
    assert!(made.draft);
    assert!(!made.out_of_ads);
    assert_eq!(
        (
            made.name.as_str(),
            made.slug.as_str(),
            made.meta_title.as_str()
        ),
        ("Biker gloves", "biker-gloves", "Biker gloves for bikers")
    );

    // Asked for again, as a saga delivered twice does: nothing more.
    assert!(!cmd.create_draft_category(ALICE, gloves).await.unwrap());
    let same = cmd.load_category("01GLOVES").await.unwrap().unwrap();
    assert_eq!(same.aggregate_version, made.aggregate_version);

    // A child under it, kept out of ads from the start.
    assert!(!cmd.category_slug_taken("summer-gloves").await.unwrap());
    let summer = DraftCategory {
        out_of_ads: true,
        position: 2,
        meta_title: String::new(),
        ..draft(
            "01SUMMER",
            "Summer gloves",
            "summer-gloves",
            Some("01GLOVES"),
        )
    };
    assert!(cmd.create_draft_category(ALICE, summer).await.unwrap());
    let child = cmd.load_category("01SUMMER").await.unwrap().unwrap();
    assert!(child.draft && child.out_of_ads);
    assert_eq!(child.parent_id.as_deref(), Some("01GLOVES"));
    assert_eq!(child.meta_title, "");
    assert_eq!(child.position, 2);
    for taken in ["summer-gloves", "biker-gloves", "blog"] {
        assert!(cmd.category_slug_taken(taken).await.unwrap(), "{taken}");
    }

    // A slug another category holds is refused, and nothing is written.
    assert!(matches!(
        cmd.create_draft_category(ALICE, draft("01OTHER", "Other", "biker-gloves", None))
            .await,
        Err(CatalogError::SlugTaken)
    ));
    assert!(cmd.load_category("01OTHER").await.unwrap().is_none());
    assert!(matches!(
        cmd.create_draft_category(ALICE, draft("01LOST", "Lost", "lost", Some("01NOWHERE")))
            .await,
        Err(CatalogError::CategoryNotFound)
    ));
    assert!(matches!(
        cmd.create_draft_category(ALICE, draft(" ", "No id", "no-id", None))
            .await,
        Err(CatalogError::CategoryNotFound)
    ));
}

#[tokio::test]
async fn a_category_is_published_from_the_top_down_and_kept_back_again() {
    let cmd = command().await;
    let live = create_category(&cmd, "Bedding").await;
    assert!(!live.draft, "a category made by hand is shown to shoppers");
    cmd.create_draft_category(ALICE, draft("01GLOVES", "Gloves", "gloves", None))
        .await
        .unwrap();
    cmd.create_draft_category(
        ALICE,
        draft("01SUMMER", "Summer", "summer", Some("01GLOVES")),
    )
    .await
    .unwrap();

    assert!(matches!(
        cmd.set_category_published(ALICE, "01SUMMER", true).await,
        Err(CatalogError::CategoryParentDraft)
    ));
    cmd.set_category_published(ALICE, "01GLOVES", true)
        .await
        .unwrap();
    cmd.set_category_published(ALICE, "01SUMMER", true)
        .await
        .unwrap();
    let published = cmd.load_category("01SUMMER").await.unwrap().unwrap();
    assert!(!published.draft);
    cmd.set_category_published(ALICE, "01SUMMER", true)
        .await
        .unwrap();
    let same = cmd.load_category("01SUMMER").await.unwrap().unwrap();
    assert_eq!(same.aggregate_version, published.aggregate_version);

    cmd.set_category_published(ALICE, &live.id, false)
        .await
        .unwrap();
    assert!(cmd.load_category(&live.id).await.unwrap().unwrap().draft);
}

#[tokio::test]
async fn a_category_is_kept_out_of_ads_and_let_back_in() {
    let cmd = command().await;
    let category = create_category(&cmd, "Bedding").await;
    assert!(!category.out_of_ads);
    cmd.set_category_in_ads(ALICE, &category.id, true)
        .await
        .unwrap();
    let same = cmd.load_category(&category.id).await.unwrap().unwrap();
    assert_eq!(same.aggregate_version, category.aggregate_version);

    cmd.set_category_in_ads(ALICE, &category.id, false)
        .await
        .unwrap();
    assert!(
        cmd.load_category(&category.id)
            .await
            .unwrap()
            .unwrap()
            .out_of_ads
    );
    cmd.set_category_in_ads(ALICE, &category.id, true)
        .await
        .unwrap();
    assert!(
        !cmd.load_category(&category.id)
            .await
            .unwrap()
            .unwrap()
            .out_of_ads
    );
}
