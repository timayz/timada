//! Loading a snapshot into a catalog: the tree, the products, their
//! pictures through the host's `Images`, idempotency and the limit.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use evento::migrator::Migrate;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx_migrator::Info;
use timada_catalog::{Command, ProductStatus};
use timada_catalog_import::{
    AttributeSeed, BoxFuture, CategorySeed, ImageSeed, Images, ImportOptions, NoImages, OptionSeed,
    ProductSeed, SelectionSeed, Snapshot, StoredImage, VariantSeed, import,
};

const ALICE: &str = "user-alice";

async fn catalog() -> Command<evento::Sqlite> {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(":memory:")
                .create_if_missing(true),
        )
        .await
        .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let mut migrator = evento::sql_migrator::new::<sqlx::Sqlite>().unwrap();
    migrator
        .add_migrations(timada_catalog::migrations())
        .unwrap();
    migrator
        .run(&mut *conn, &evento::migrator::Plan::apply_all())
        .await
        .unwrap();
    drop(conn);
    Command(pool.into())
}

/// Hands out a picture for every URL but the one that says "missing",
/// and counts what it is asked to drop.
#[derive(Default)]
struct FakeImages {
    fetched: AtomicUsize,
    discarded: AtomicUsize,
}

impl Images for FakeImages {
    fn fetch<'a>(&'a self, url: &'a str) -> BoxFuture<'a, anyhow::Result<Option<StoredImage>>> {
        Box::pin(async move {
            self.fetched.fetch_add(1, Ordering::SeqCst);
            if url.contains("missing") {
                return Ok(None);
            }
            if url.contains("broken") {
                anyhow::bail!("connection reset");
            }
            Ok(Some(StoredImage {
                image_id: format!("img-{}", url.rsplit('/').next().unwrap()),
                ext: "jpg".into(),
                size: 1234,
                width: Some(600),
                height: Some(400),
            }))
        })
    }

    fn discard<'a>(&'a self, _image: &'a StoredImage) -> BoxFuture<'a, ()> {
        self.discarded.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {})
    }
}

fn snapshot() -> Snapshot {
    Snapshot {
        source: "test".into(),
        fetched_at: 1,
        categories: vec![
            // A child listed before its parent: the import orders them.
            CategorySeed {
                slug: "tapis-chats".into(),
                name: "Tapis pour chats".into(),
                parent: Some("couchage-chat".into()),
                position: 2,
                ..Default::default()
            },
            CategorySeed {
                slug: "couchage-chat".into(),
                name: "Couchages pour chats".into(),
                description: "Où il dort.".into(),
                position: 1,
                ..Default::default()
            },
            CategorySeed {
                slug: "ventes-privees".into(),
                name: "Ventes privées".into(),
                in_menu: false,
                ..Default::default()
            },
            CategorySeed {
                slug: "orphan".into(),
                name: "Orphan".into(),
                parent: Some("nowhere".into()),
                ..Default::default()
            },
        ],
        products: vec![
            ProductSeed {
                slug: "tapis-japonais".into(),
                name: "Tapis japonais".into(),
                description: "Frais.\n\nDoux.".into(),
                currency: "EUR".into(),
                categories: vec![
                    "tapis-chats".into(),
                    "couchage-chat".into(),
                    "unknown".into(),
                ],
                images: vec![
                    ImageSeed {
                        url: "https://pics.example/a.jpg".into(),
                        alt: "Le tapis".into(),
                    },
                    ImageSeed {
                        url: "https://pics.example/missing.jpg".into(),
                        alt: String::new(),
                    },
                    ImageSeed {
                        url: "https://pics.example/b.jpg".into(),
                        alt: "De côté".into(),
                    },
                    ImageSeed {
                        url: "https://pics.example/broken.jpg".into(),
                        alt: String::new(),
                    },
                ],
                options: vec![
                    OptionSeed {
                        name: "Taille".into(),
                        values: vec!["M".into(), "S".into()],
                    },
                    OptionSeed {
                        name: "Couleur".into(),
                        values: vec!["Beige".into(), "Bleu".into()],
                    },
                ],
                variants: vec![
                    VariantSeed {
                        selections: vec![sel("Taille", "M"), sel("Couleur", "Beige")],
                        sku: Some("TJ-M-BEIGE".into()),
                        amount_cents: 2990,
                        image: Some(2),
                    },
                    VariantSeed {
                        selections: vec![sel("Taille", "S"), sel("Couleur", "Beige")],
                        sku: Some("bad sku/".into()),
                        amount_cents: 1990,
                        image: Some(1),
                    },
                    VariantSeed {
                        selections: vec![sel("Taille", "S"), sel("Couleur", "Bleu")],
                        sku: Some("TJ-M-BEIGE".into()),
                        amount_cents: 1990,
                        image: None,
                    },
                ],
                attributes: vec![AttributeSeed {
                    name: "Matière".into(),
                    value: "Rotin".into(),
                }],
                ..Default::default()
            },
            ProductSeed {
                slug: "pendentif".into(),
                name: "Pendentif".into(),
                currency: "EUR".into(),
                amount_cents: Some(4390),
                sku: Some("P-1".into()),
                categories: vec!["ventes-privees".into()],
                ..Default::default()
            },
            ProductSeed {
                slug: "unpriced".into(),
                name: "Unpriced".into(),
                currency: "EUR".into(),
                ..Default::default()
            },
        ],
    }
}

fn sel(option: &str, value: &str) -> SelectionSeed {
    SelectionSeed {
        option: option.into(),
        value: value.into(),
    }
}

#[tokio::test]
async fn loads_the_tree_and_the_products_then_skips_them_next_time() {
    let catalog = catalog().await;
    let images = Arc::new(FakeImages::default());
    let report = import(
        &catalog,
        ALICE,
        &snapshot(),
        images.clone(),
        &ImportOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(report.categories_created, 3);
    assert_eq!(report.products_created, 2);
    assert_eq!(report.products_skipped, 0);
    assert_eq!(report.variants, 3);
    assert_eq!(report.images, 2, "one missing, one broken");
    assert!(
        report.warnings.iter().any(|w| w.contains("orphan")),
        "{:?}",
        report.warnings
    );
    assert!(
        report.warnings.iter().any(|w| w.starts_with("unpriced:")),
        "{:?}",
        report.warnings
    );
    assert!(
        report.warnings.iter().any(|w| w.contains("broken.jpg")),
        "{:?}",
        report.warnings
    );
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("category unknown unknown")),
        "{:?}",
        report.warnings
    );
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("code bad sku/ refused"))
            && report
                .warnings
                .iter()
                .any(|w| w.contains("code TJ-M-BEIGE refused")),
        "{:?}",
        report.warnings
    );
    assert_eq!(images.fetched.load(Ordering::SeqCst), 4);
    assert_eq!(images.discarded.load(Ordering::SeqCst), 0);

    // The tree.
    let couchage = catalog
        .load_category_by_slug("couchage-chat")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(couchage.description, "Où il dort.");
    assert_eq!(couchage.position, 1);
    assert!(!couchage.hidden_from_menu);
    let tapis = catalog
        .load_category_by_slug("tapis-chats")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(tapis.parent_id.as_deref(), Some(couchage.id.as_str()));
    assert_eq!(tapis.position, 2);
    let ventes = catalog
        .load_category_by_slug("ventes-privees")
        .await
        .unwrap()
        .unwrap();
    assert!(ventes.hidden_from_menu);
    assert!(
        catalog
            .load_category_by_slug("orphan")
            .await
            .unwrap()
            .is_none()
    );

    // The variable product.
    let product = catalog
        .load_product_by_slug("tapis-japonais")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(product.status, ProductStatus::Published);
    assert_eq!(product.description, "Frais.\n\nDoux.");
    assert_eq!(product.price, None);
    assert_eq!(product.categories.len(), 2);
    assert!(product.categories.contains(&couchage.id) && product.categories.contains(&tapis.id));
    let ids: Vec<&str> = product.images.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(
        ids,
        ["img-a.jpg", "img-b.jpg"],
        "in the snapshot's order, the missing ones left out"
    );
    assert_eq!(product.images[1].alt, "De côté");
    assert_eq!(product.images[0].width, Some(600));
    let names: Vec<&str> = product.options.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, ["Taille", "Couleur"]);
    assert_eq!(product.variants.len(), 3);
    let m_beige = product
        .variants
        .iter()
        .find(|v| v.sku.as_deref() == Some("TJ-M-BEIGE"))
        .expect("the variant that kept its code");
    assert_eq!(m_beige.price.amount_cents, 2990);
    assert_eq!(
        m_beige.image_id.as_deref(),
        Some("img-b.jpg"),
        "the third picture, which is the second recorded"
    );
    assert_eq!(m_beige.selections[0].value, "M");
    assert_eq!(m_beige.selections[1].value, "Beige");
    let without_code = product.variants.iter().filter(|v| v.sku.is_none()).count();
    assert_eq!(
        without_code, 2,
        "a bad code and a taken one are dropped, the variants kept"
    );
    let s_beige = product
        .variants
        .iter()
        .find(|v| v.selections[0].value == "S" && v.selections[1].value == "Beige")
        .unwrap();
    assert_eq!(s_beige.image_id, None, "its picture was missing");
    assert_eq!(product.attributes.len(), 1);

    // The simple product.
    let pendentif = catalog
        .load_product_by_slug("pendentif")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pendentif.status, ProductStatus::Published);
    assert_eq!(pendentif.price.as_ref().unwrap().amount_cents, 4390);
    assert!(pendentif.categories.contains(&ventes.id));

    // The one that failed is not there, so a next run tries again.
    assert!(
        catalog
            .load_product_by_slug("unpriced")
            .await
            .unwrap()
            .is_none()
    );

    // Second run: nothing new.
    let again = import(
        &catalog,
        ALICE,
        &snapshot(),
        images.clone(),
        &ImportOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(again.categories_created, 0);
    assert_eq!(again.categories_skipped, 3);
    assert_eq!(again.products_created, 0);
    assert_eq!(again.products_skipped, 2);
    assert_eq!(again.images, 0);
    assert_eq!(
        images.fetched.load(Ordering::SeqCst),
        4,
        "no picture fetched again"
    );
    let product_again = catalog
        .load_product_by_slug("tapis-japonais")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(product_again.variants.len(), 3);
}

#[tokio::test]
async fn a_limit_takes_the_first_products_only_and_no_images_means_none() {
    let catalog = catalog().await;
    let report = import(
        &catalog,
        ALICE,
        &snapshot(),
        Arc::new(NoImages),
        &ImportOptions {
            limit: Some(1),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(report.products_created, 1);
    assert_eq!(report.images, 0);
    assert_eq!(report.categories_created, 3);
    let product = catalog
        .load_product_by_slug("tapis-japonais")
        .await
        .unwrap()
        .unwrap();
    assert!(product.images.is_empty());
    assert_eq!(product.variants.len(), 3);
    assert!(product.variants.iter().all(|v| v.image_id.is_none()));
    assert!(
        catalog
            .load_product_by_slug("pendentif")
            .await
            .unwrap()
            .is_none()
    );
}
