//! The mapping from a WooCommerce shop's Store API listings (trimmed real
//! answers from chatounette.com, October 2026) to a snapshot.

use timada_catalog_import::woocommerce::{SiteOptions, WooCategory, WooProduct, map_catalog};
use timada_catalog_import::{SelectionSeed, Snapshot};

fn fixture<T: serde::de::DeserializeOwned>(name: &str) -> T {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

fn mapped(options: &SiteOptions) -> Snapshot {
    let categories: Vec<WooCategory> = fixture("categories.json");
    let products: Vec<WooProduct> = fixture("products.json");
    let variations: Vec<WooProduct> = fixture("variations.json");
    map_catalog(categories, products, variations, options)
}

#[test]
fn categories_keep_their_tree_order_and_menu() {
    let snapshot = mapped(&SiteOptions {
        menu_order: vec!["jeux-pour-chat".into(), "couchage-chat".into()],
        hidden: vec!["ventes-privees".into()],
    });
    let slugs: Vec<&str> = snapshot
        .categories
        .iter()
        .map(|c| c.slug.as_str())
        .collect();
    assert!(!slugs.contains(&"uncategorized"), "{slugs:?}");
    // The menu's order first, then the API's (alphabetical).
    let roots: Vec<&str> = snapshot
        .categories
        .iter()
        .filter(|c| c.parent.is_none())
        .map(|c| c.slug.as_str())
        .collect();
    assert_eq!(
        roots,
        [
            "jeux-pour-chat",
            "couchage-chat",
            "accessoires-pour-chat",
            "goodies-chat",
            "ventes-privees"
        ]
    );
    let positions: Vec<u32> = snapshot
        .categories
        .iter()
        .filter(|c| c.parent.is_none())
        .map(|c| c.position)
        .collect();
    assert_eq!(positions, [0, 1, 2, 3, 4]);
    let find = |slug: &str| snapshot.categories.iter().find(|c| c.slug == slug).unwrap();
    assert_eq!(find("tapis-chats").parent.as_deref(), Some("couchage-chat"));
    assert_eq!(
        find("tapis-rafraichissant-chat").parent.as_deref(),
        Some("tapis-chats")
    );
    assert!(!find("ventes-privees").in_menu);
    assert!(find("couchage-chat").in_menu);
    assert_eq!(find("couchage-chat").name, "Couchages pour chats");
    let description = &find("tapis-rafraichissant-chat").description;
    assert!(
        description.starts_with("En été, votre chat déserte son panier"),
        "{description}"
    );
    assert!(
        !description.contains('<') && !description.contains("&rsquo;"),
        "{description}"
    );
    assert!(
        find("couchage-chat")
            .image
            .as_ref()
            .unwrap()
            .url
            .ends_with(".jpg")
    );
}

#[test]
fn a_variable_product_becomes_options_and_priced_variants() {
    let snapshot = mapped(&SiteOptions::default());
    let product = snapshot
        .products
        .iter()
        .find(|p| p.slug == "tapis-rafraichissant-chat-orne-tetes-chats-japonais")
        .expect("the variable product");
    assert_eq!(
        product.name,
        "Tapis rafraichissant chat orné de têtes de chats japonais"
    );
    assert_eq!(product.currency, "EUR");
    assert_eq!(product.amount_cents, None, "sold through its variants");
    assert!(
        product
            .description
            .starts_with("Imaginez votre chat étendu"),
        "{}",
        product.description
    );
    assert!(!product.description.contains("<strong>"));
    // Only the categories the snapshot knows, in the shop's order (leaf first).
    assert_eq!(
        product.categories,
        ["tapis-rafraichissant-chat", "couchage-chat", "tapis-chats"]
    );
    assert_eq!(product.images.len(), 5);
    assert!(
        product.images[0]
            .alt
            .starts_with("Un tapis rafraichissant chat")
    );
    // Only the attribute that varies is an option; the others are facts.
    assert_eq!(product.options.len(), 1);
    assert_eq!(product.options[0].name, "Taille");
    assert_eq!(product.options[0].values, ["M", "S"]);
    let facts: Vec<(&str, &str)> = product
        .attributes
        .iter()
        .map(|a| (a.name.as_str(), a.value.as_str()))
        .collect();
    assert_eq!(
        facts,
        [
            ("Couleur", "Beige, Bleu Foncé"),
            ("Matiere", "Rotin tissé (surface), + Mousse (intérieur)"),
            ("Dimensions", "S : 43 x 30 cm, M : 55 x 40 cm"),
        ]
    );
    // Variants carry the term's name (not its slug), the variation's own
    // price and its picture, found among the product's.
    assert_eq!(product.variants.len(), 2);
    let m = &product.variants[0];
    assert_eq!(
        m.selections,
        [SelectionSeed {
            option: "Taille".into(),
            value: "M".into()
        }]
    );
    assert_eq!(m.amount_cents, 2990);
    assert_eq!(m.image, Some(0));
    assert_eq!(m.sku, None);
    let s = &product.variants[1];
    assert_eq!(s.selections[0].value, "S");
    assert_eq!(s.amount_cents, 1990);
}

#[test]
fn a_simple_product_keeps_its_price_code_and_facts() {
    let snapshot = mapped(&SiteOptions::default());
    let product = snapshot
        .products
        .iter()
        .find(|p| p.slug == "pendentif-chat-fier-elegant")
        .expect("the simple product");
    assert_eq!(product.amount_cents, Some(4390));
    assert_eq!(
        product.sku.as_deref(),
        Some("4000210003358-1-1-1-3-2-3-2-3-1-1-1")
    );
    assert!(product.options.is_empty() && product.variants.is_empty());
    assert_eq!(product.categories, ["pendentif-chat", "goodies-chat"]);
    assert_eq!(product.images.len(), 4);
    assert_eq!(product.attributes.len(), 1);
    assert_eq!(product.attributes[0].name, "Matériau");
    assert!(
        product
            .description
            .starts_with("Ce pendentif de chat fier et élégant est un accessoire"),
        "{}",
        product.description
    );
    assert!(
        product.description.contains('\u{2019}'),
        "entities decoded: {}",
        product.description
    );
}
