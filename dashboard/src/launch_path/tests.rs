use evento::Evento;
use timada_launch::STEP_CONSENT;
use timada_tenancy::CreateTenant;

use super::*;

const ALICE: &str = "user-alice";

fn category(id: &str, name: &str, parent: Option<&str>, products: i64) -> CategoryRow {
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
        product_count: products,
        subtree_count: products,
    }
}

#[test]
fn only_the_bottom_of_the_tree_is_held_to_a_number_of_products() {
    let mut draft = category("DRAFT", "Brouillon", None, 0);
    draft.draft = true;
    let categories = vec![
        // Above others: its products are theirs.
        category("CAT", "Chats", None, 0),
        category("TREE", "Arbres", Some("CAT"), 12),
        category("TOY", "Jouets", Some("CAT"), 9),
        category("DOG", "Chiens", None, 10),
        draft,
    ];
    assert_eq!(thin_leaves(&categories, 10), vec!["Jouets (9)"]);
    assert!(thin_leaves(&categories, 9).is_empty());
}

#[test]
fn categories_that_hold_the_same_products_are_told() {
    let filed = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(category, product)| ((*category).to_owned(), (*product).to_owned()))
            .collect()
    };
    let same = twins(&filed(&[
        ("TREE", "p1"),
        ("TREE", "p2"),
        ("BIG", "p2"),
        ("BIG", "p1"),
        ("TOY", "p1"),
        ("DOG", "p3"),
    ]));
    assert_eq!(same, vec![vec!["BIG".to_owned(), "TREE".to_owned()]]);
    assert!(twins(&filed(&[("TREE", "p1"), ("TOY", "p2")])).is_empty());
    assert_eq!(listed(&["a".into(), "b".into()]), "a, b");
    let many: Vec<String> = (1..=7).map(|n| n.to_string()).collect();
    assert_eq!(listed(&many), "1, 2, 3, 4, 5 and 2 more");
}

#[tokio::test]
async fn a_new_shop_has_everything_to_do_and_a_members_word_is_kept() {
    let turso = evento::Turso::in_memory_concurrent().await.unwrap();
    crate::db::migrate_store_read_models(&turso).await.unwrap();
    let db = Db::from(turso.clone());
    let store = Evento::new(turso);
    let tenant = timada_tenancy::Command(store.clone())
        .create_tenant(CreateTenant {
            name: "Acme".to_owned(),
            owner_user_id: ALICE.to_owned(),
        })
        .await
        .unwrap();
    let steps = path(&store, &db, &tenant).await.unwrap();
    let state = |steps: &[Step], key: &str| {
        steps
            .iter()
            .find(|step| step.key == key)
            .unwrap_or_else(|| panic!("no step {key}"))
            .state
    };
    // In the order of the path, each key once.
    let keys: Vec<&str> = steps.iter().map(|step| step.key).collect();
    assert_eq!(
        keys,
        vec![
            "study",
            "categories",
            "category_products",
            "category_overlap",
            "product_count",
            "slug_clash",
            "margins",
            "supplier_text",
            "company",
            "legal",
            "consent",
            "contact",
            "shipping_returns",
            "sitemap",
            "sitemap_submitted",
            "tag",
            "test_order",
            "speed",
            "feed",
            "campaign_prepared",
            "campaign_live",
        ]
    );
    for key in [
        "study",
        "categories",
        "category_products",
        "product_count",
        "margins",
        "company",
        // No page is published, no address, no Search Console: a word
        // would not make up for it.
        "legal",
        "shipping_returns",
        "sitemap",
        "sitemap_submitted",
        "tag",
        "feed",
        "campaign_prepared",
        "campaign_live",
    ] {
        assert_eq!(state(&steps, key), State::ToDo, "{key}");
    }
    // Nothing to clash or to overlap yet.
    assert_eq!(state(&steps, "slug_clash"), State::Done);
    assert_eq!(state(&steps, "category_overlap"), State::Done);
    // What only a person can tell.
    for key in ["supplier_text", "consent", "contact", "test_order", "speed"] {
        assert_eq!(state(&steps, key), State::NotChecked, "{key}");
    }
    assert_eq!(
        blocking(&steps),
        vec![
            "Every product leaves something, and its break-even is known",
            "The company's details are filled",
            "The legal pages are written for this company",
            "The storefront asks before it measures",
            "Delivery and returns are stated",
            "The Google tag is set and counts the sales",
            "A real test order went through, its purchase counted",
            "Merchant Center accepts every product",
        ]
    );

    let launches = timada_launch::Command::new(store.clone(), &tenant.id);
    launches
        .confirm_step(ALICE, STEP_CONSENT, true)
        .await
        .unwrap();
    // A word on what timada can tell is missing changes nothing.
    launches
        .confirm_step(ALICE, timada_launch::STEP_LEGAL, true)
        .await
        .unwrap();
    let steps = path(&store, &db, &tenant).await.unwrap();
    assert_eq!(state(&steps, "consent"), State::Done);
    let consent = steps.iter().find(|step| step.key == "consent").unwrap();
    assert_eq!(consent.confirmed.as_ref().unwrap().0, ALICE);
    assert_eq!(state(&steps, "legal"), State::ToDo);
    assert_eq!(blocking(&steps).len(), 7);
    launches
        .confirm_step(ALICE, STEP_CONSENT, false)
        .await
        .unwrap();
    let steps = path(&store, &db, &tenant).await.unwrap();
    assert_eq!(state(&steps, "consent"), State::NotChecked);

    // A smaller catalog planned: an empty shop still has none.
    launches.set_path_targets(ALICE, 3, 20).await.unwrap();
    let steps = path(&store, &db, &tenant).await.unwrap();
    let count = steps
        .iter()
        .find(|step| step.key == "product_count")
        .unwrap();
    assert_eq!(count.detail, "0 products are published, of the 20 planned.");
}
