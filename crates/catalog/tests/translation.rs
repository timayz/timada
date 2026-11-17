mod common;

use common::*;
use evento::ProjectionAggregate;
use timada_catalog::{CatalogError, CategoryQuestion, TranslationKind, TranslationText};

const PRODUCT: TranslationKind = TranslationKind::Product;
const CATEGORY: TranslationKind = TranslationKind::Category;

fn named(name: &str) -> TranslationText {
    TranslationText {
        name: name.to_owned(),
        ..Default::default()
    }
}

fn at(name: &str, slug: &str) -> TranslationText {
    TranslationText {
        slug: Some(slug.to_owned()),
        ..named(name)
    }
}

#[tokio::test]
async fn a_product_is_worded_in_another_language_beside_its_own() {
    let cmd = command().await;
    let tree = create_product(&cmd, "Arbre à chat").await;
    assert_eq!(
        cmd.load_translation(PRODUCT, &tree.id, "en").await.unwrap(),
        None
    );

    cmd.translate(
        ALICE,
        PRODUCT,
        &tree.id,
        " EN ",
        TranslationText {
            description: " Three floors of sisal. ".to_owned(),
            meta_title: "Cat tree, 3 floors".to_owned(),
            // A product has no buying guide and answers no questions.
            body: "ignored".to_owned(),
            questions: vec![CategoryQuestion {
                question: "ignored".to_owned(),
                answer: "ignored".to_owned(),
            }],
            ..named("  Cat Tree ")
        },
    )
    .await
    .unwrap();
    let english = cmd
        .load_translation(PRODUCT, &tree.id, "en")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(english.kind, "product");
    assert_eq!(english.subject_id, tree.id);
    assert_eq!(english.language, "en");
    assert_eq!(english.name, "Cat Tree");
    assert_eq!(english.slug, "cat-tree");
    assert_eq!(english.description, "Three floors of sisal.");
    assert_eq!(english.meta_title, "Cat tree, 3 floors");
    assert_eq!(english.body, "");
    assert!(english.questions.is_empty());
    // Name, description and search engines' words, in one write.
    assert_eq!(english.aggregate_version().unwrap(), 3);

    // The product's own stream is untouched, and so is its version.
    let after = cmd.load_product(&tree.id).await.unwrap().unwrap();
    assert_eq!(after.name, "Arbre à chat");
    assert_eq!(after.slug, "arbre-a-chat");
    assert_eq!(
        after.aggregate_version().unwrap(),
        tree.aggregate_version().unwrap()
    );

    // The same words write nothing; a new description writes that alone.
    let again = TranslationText {
        description: "Three floors of sisal.".to_owned(),
        meta_title: "Cat tree, 3 floors".to_owned(),
        ..named("Cat Tree")
    };
    cmd.translate(ALICE, PRODUCT, &tree.id, "en", again.clone())
        .await
        .unwrap();
    cmd.translate(
        ALICE,
        PRODUCT,
        &tree.id,
        "en",
        TranslationText {
            description: "Four floors.".to_owned(),
            ..again
        },
    )
    .await
    .unwrap();
    let english = cmd
        .load_translation(PRODUCT, &tree.id, "en")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(english.description, "Four floors.");
    assert_eq!(english.aggregate_version().unwrap(), 4);

    // Each language has its own stream.
    cmd.translate(ALICE, PRODUCT, &tree.id, "de", named("Kratzbaum"))
        .await
        .unwrap();
    let both = cmd
        .load_translations(
            PRODUCT,
            &tree.id,
            &["en".to_owned(), "es".to_owned(), "de".to_owned()],
        )
        .await
        .unwrap();
    let names: Vec<&str> = both.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["Cat Tree", "Kratzbaum"]);
}

#[tokio::test]
async fn a_slug_is_held_once_in_a_language_and_kept_across_renames() {
    let cmd = command().await;
    let tree = create_product(&cmd, "Arbre à chat").await;
    let post = create_product(&cmd, "Griffoir").await;
    cmd.translate(ALICE, PRODUCT, &tree.id, "en", named("Cat Tree"))
        .await
        .unwrap();

    // Another product naming itself the same gets a suffix; asking for the
    // slug itself is refused.
    cmd.translate(ALICE, PRODUCT, &post.id, "en", named("Cat tree"))
        .await
        .unwrap();
    let scratching = cmd
        .load_translation(PRODUCT, &post.id, "en")
        .await
        .unwrap()
        .unwrap();
    assert!(
        scratching.slug.starts_with("cat-tree-"),
        "{}",
        scratching.slug
    );
    let taken = cmd
        .translate(ALICE, PRODUCT, &post.id, "en", at("Cat tree", "cat-tree"))
        .await;
    assert!(matches!(taken, Err(CatalogError::SlugTaken)), "{taken:?}");
    // The same slug in another language, or for a category, is another slug.
    cmd.translate(ALICE, PRODUCT, &post.id, "de", at("Kratzbaum", "cat-tree"))
        .await
        .unwrap();
    let cats = create_category(&cmd, "Chats").await;
    cmd.translate(ALICE, CATEGORY, &cats.id, "en", at("Cats", "cat-tree"))
        .await
        .unwrap();

    // A new name keeps the address; a new slug moves it and remembers the
    // old one, which is free at once.
    cmd.translate(ALICE, PRODUCT, &tree.id, "en", named("Big Cat Tree"))
        .await
        .unwrap();
    let english = cmd
        .load_translation(PRODUCT, &tree.id, "en")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (english.name.as_str(), english.slug.as_str()),
        ("Big Cat Tree", "cat-tree")
    );
    cmd.translate(
        ALICE,
        PRODUCT,
        &tree.id,
        "en",
        at("Big Cat Tree", "Big-Cat-Tree"),
    )
    .await
    .unwrap();
    let english = cmd
        .load_translation(PRODUCT, &tree.id, "en")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(english.slug, "big-cat-tree");
    let former: Vec<&str> = english
        .former_slugs
        .iter()
        .map(|was| was.slug.as_str())
        .collect();
    assert_eq!(former, ["cat-tree"]);
    cmd.translate(ALICE, PRODUCT, &post.id, "en", at("Cat tree", "cat-tree"))
        .await
        .unwrap();

    // A slug that is another product's own in the main language stays its
    // address in every language it is not translated into.
    let taken = cmd
        .translate(ALICE, PRODUCT, &tree.id, "es", at("Árbol", "griffoir"))
        .await;
    assert!(matches!(taken, Err(CatalogError::SlugTaken)), "{taken:?}");
    // Its own main slug is its own to keep (a brand reads the same anywhere).
    cmd.translate(ALICE, PRODUCT, &tree.id, "es", at("Árbol", "arbre-a-chat"))
        .await
        .unwrap();
}

#[tokio::test]
async fn a_dropped_translation_frees_its_slug_and_starts_again_empty() {
    let cmd = command().await;
    let tree = create_product(&cmd, "Arbre à chat").await;
    let post = create_product(&cmd, "Griffoir").await;
    cmd.translate(
        ALICE,
        PRODUCT,
        &tree.id,
        "en",
        TranslationText {
            description: "Three floors.".to_owned(),
            ..named("Cat Tree")
        },
    )
    .await
    .unwrap();
    cmd.drop_translation(ALICE, PRODUCT, &tree.id, "en")
        .await
        .unwrap();
    // Told twice, nothing more happens.
    cmd.drop_translation(ALICE, PRODUCT, &tree.id, "en")
        .await
        .unwrap();
    assert_eq!(
        cmd.load_translation(PRODUCT, &tree.id, "en").await.unwrap(),
        None
    );
    let gone = cmd
        .load_translation_any(PRODUCT, &tree.id, "en")
        .await
        .unwrap()
        .unwrap();
    assert!(gone.dropped);
    assert_eq!(gone.aggregate_version().unwrap(), 3);
    assert_eq!(gone.former_slugs.len(), 1);

    // The slug is another product's to take, and the first comes back
    // under another, with nothing of what it said before.
    cmd.translate(
        ALICE,
        PRODUCT,
        &post.id,
        "en",
        at("Scratching post", "cat-tree"),
    )
    .await
    .unwrap();
    cmd.translate(ALICE, PRODUCT, &tree.id, "en", named("Cat Tree"))
        .await
        .unwrap();
    let back = cmd
        .load_translation(PRODUCT, &tree.id, "en")
        .await
        .unwrap()
        .unwrap();
    assert!(back.slug.starts_with("cat-tree-"), "{}", back.slug);
    assert_eq!(back.description, "");

    // An archived product's slug is free too.
    cmd.archive_product(ALICE, &post.id).await.unwrap();
    cmd.translate(ALICE, PRODUCT, &tree.id, "en", at("Cat Tree", "cat-tree"))
        .await
        .unwrap();
}

#[tokio::test]
async fn a_category_is_worded_with_its_guide_and_its_questions() {
    let cmd = command().await;
    let cats = create_category(&cmd, "Chats").await;
    let asked = vec![CategoryQuestion {
        question: "  Which size? ".to_owned(),
        answer: "Measure your cat.\r\nThen add 20 cm.".to_owned(),
    }];
    cmd.translate(
        ALICE,
        CATEGORY,
        &cats.id,
        "en",
        TranslationText {
            description: "Everything for cats.".to_owned(),
            body: "## How to choose\n\nBy size.".to_owned(),
            questions: asked,
            ..named("Cats")
        },
    )
    .await
    .unwrap();
    let english = cmd
        .load_translation(CATEGORY, &cats.id, "en")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(english.kind, "category");
    assert_eq!(english.slug, "cats");
    assert_eq!(english.body, "## How to choose\n\nBy size.");
    assert_eq!(english.questions[0].question, "Which size?");
    assert_eq!(
        english.questions[0].answer,
        "Measure your cat.\nThen add 20 cm."
    );
    // A product and a category translate apart, even under one id.
    assert_eq!(
        cmd.load_translation(PRODUCT, &cats.id, "en").await.unwrap(),
        None
    );
}

#[tokio::test]
async fn what_cannot_be_worded_is_refused() {
    let cmd = command().await;
    let tree = create_product(&cmd, "Arbre à chat").await;
    let cats = create_category(&cmd, "Chats").await;

    let off_list = cmd
        .translate(ALICE, PRODUCT, &tree.id, "zz", named("X"))
        .await;
    assert!(
        matches!(off_list, Err(CatalogError::InvalidLanguage)),
        "{off_list:?}"
    );
    let nameless = cmd
        .translate(ALICE, PRODUCT, &tree.id, "en", named(" "))
        .await;
    assert!(
        matches!(nameless, Err(CatalogError::InvalidName)),
        "{nameless:?}"
    );
    let bad_slug = cmd
        .translate(ALICE, PRODUCT, &tree.id, "en", at("Cat Tree", "Cat Tree!"))
        .await;
    assert!(
        matches!(bad_slug, Err(CatalogError::InvalidSlug)),
        "{bad_slug:?}"
    );
    let long = cmd
        .translate(
            ALICE,
            CATEGORY,
            &cats.id,
            "en",
            TranslationText {
                meta_title: "x".repeat(500),
                ..named("Cats")
            },
        )
        .await;
    assert!(matches!(long, Err(CatalogError::InvalidSeo)), "{long:?}");
    let missing = cmd
        .translate(ALICE, PRODUCT, "nope", "en", named("X"))
        .await;
    assert!(
        matches!(missing, Err(CatalogError::NotFound)),
        "{missing:?}"
    );
    let no_category = cmd
        .translate(ALICE, CATEGORY, &tree.id, "en", named("X"))
        .await;
    assert!(
        matches!(no_category, Err(CatalogError::CategoryNotFound)),
        "{no_category:?}"
    );

    cmd.archive_product(ALICE, &tree.id).await.unwrap();
    let archived = cmd
        .translate(ALICE, PRODUCT, &tree.id, "en", named("Cat Tree"))
        .await;
    assert!(
        matches!(archived, Err(CatalogError::Archived)),
        "{archived:?}"
    );
    // Nothing was written along the way.
    assert_eq!(
        cmd.load_translation_any(PRODUCT, &tree.id, "en")
            .await
            .unwrap(),
        None
    );
}
