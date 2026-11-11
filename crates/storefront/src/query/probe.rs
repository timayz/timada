//! What the pages read, timed over a shop of 20,000 products (#474): a home,
//! a product with its related products, the cart drawer's offers, the whole
//! catalogue and a category with and without filters, and searches. The
//! catalogue is made up but shaped like the Chatounette one (names of a few
//! words, colours on half the products, sizes on a third, a two-level tree
//! of 108 categories), written as the subscription would. Run by hand:
//! `cargo test -p timada-storefront --lib probe -- --ignored --nocapture`.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use timada_core::db::{Connection, Turso, execute, search_key};

use super::*;
use crate::migrations::migrate;

const PRODUCTS: usize = 20_000;
const REVIEWS: usize = 20_000;
const TOP: usize = 12;
const CHILDREN: usize = 8;
const RUNS: u32 = 10;

/// The same numbers on every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

const NOUNS: &[&str] = &[
    "Tapis",
    "Arbre à chat",
    "Griffoir",
    "Coussin",
    "Panier",
    "Jouet",
    "Balle",
    "Canne à plumes",
    "Souris",
    "Collier",
    "Harnais",
    "Laisse",
    "Gamelle",
    "Fontaine",
    "Litière",
    "Bac à litière",
    "Brosse",
    "Peigne",
    "Couverture",
    "Niche",
    "Hamac",
    "Tunnel",
    "Pointeur laser",
    "Distributeur",
    "Croquettes",
    "Friandises",
    "Sac de transport",
    "Cage",
    "Tour",
    "Lit",
    "Matelas",
    "Pelle",
    "Shampoing",
    "Coupe-griffes",
    "Gant",
    "Grattoir",
    "Cabane",
    "Poupée",
    "Peluche",
    "Bol",
];
const ADJECTIVES: &[&str] = &[
    "interactif",
    "automatique",
    "rechargeable USB",
    "en bois",
    "en sisal",
    "en coton",
    "lavable",
    "pliable",
    "design",
    "mignon",
    "XXL",
    "pour chaton",
    "anti-stress",
    "réutilisable",
    "en peluche",
    "imperméable",
    "chauffant",
    "lumineux",
    "rafraîchissant",
    "autonettoyant",
    "anti-poils",
    "silencieux",
    "japonais",
    "scandinave",
    "en céramique",
    "en inox",
    "orthopédique",
    "suspendu",
    "mural",
    "à ventouse",
];
const MOTIFS: &[&str] = &[
    "motif dinosaures",
    "têtes de chats",
    "flocons de neige",
    "lapins",
    "carottes",
    "animaux arctiques",
    "étoiles",
    "nuages",
    "fraises",
    "pattes",
    "poissons",
    "cactus",
    "arc-en-ciel",
    "licornes",
    "pois",
];
const COLOURS: &[&str] = &[
    "Rose",
    "Bleu",
    "Gris",
    "Noir",
    "Blanc",
    "Rouge",
    "Vert",
    "Jaune",
    "Orange",
    "Violet",
    "Beige",
    "Marron",
    "Rose foncé",
    "Bleu ciel",
    "Gris clair",
    "Kaki",
    "Turquoise",
    "Bordeaux",
    "Corail",
    "Lavande",
    "Menthe",
    "Moutarde",
    "Taupe",
    "Crème",
    "Anthracite",
];
const SIZES: &[&str] = &["XS", "S", "M", "L", "XL", "XXL", "40 cm", "60 cm"];
const TEXT: &str = "Un produit pensé pour le confort de votre compagnon au quotidien, \
    fabriqué avec des matériaux durables et faciles à entretenir. ";

/// Writes `rows` of `width` values each, a few hundred to a statement: each
/// statement naming the full-text columns builds a segment of the index.
async fn insert(conn: &Connection, head: &str, width: usize, rows: Vec<Vec<Value>>) {
    for chunk in rows.chunks(100) {
        let mut values = Vec::new();
        let mut tuples = Vec::new();
        for row in chunk {
            let at = values.len();
            let marks: Vec<String> = (1..=width).map(|k| format!("?{}", at + k)).collect();
            tuples.push(format!("({})", marks.join(", ")));
            values.extend(row.iter().cloned());
        }
        execute(
            conn,
            &format!("{head} {}", tuples.join(", ")),
            params_from_iter(values),
        )
        .await
        .unwrap();
    }
}

/// The shop, and ids worth asking about: the categories, and a product.
struct Shop {
    db: Db,
    top: Vec<String>,
    leaves: Vec<String>,
    product: String,
}

async fn shop() -> Shop {
    let db = Db::from(Turso::temporary_concurrent().await.unwrap());
    migrate(&db).await.unwrap();
    let conn = db.connect().unwrap();
    let mut rng = Rng(0x5eed_1234_abcd_ef01);
    let top: Vec<String> = (0..TOP).map(|t| format!("t{t:02}")).collect();
    let leaves: Vec<String> = (0..TOP * CHILDREN).map(|l| format!("l{l:03}")).collect();
    let mut categories = Vec::new();
    for (position, id) in top.iter().enumerate() {
        categories.push(vec![
            Value::from(id.as_str()),
            Value::Null,
            Value::from(position as i64),
        ]);
    }
    for (position, id) in leaves.iter().enumerate() {
        categories.push(vec![
            Value::from(id.as_str()),
            Value::from(top[position / CHILDREN].as_str()),
            Value::from(position as i64),
        ]);
    }
    for row in categories {
        execute(
            &conn,
            "INSERT INTO storefront_categories (id, parent_id, position, slug, name,
                description, body, archived, in_menu, updated_at)
             VALUES (?1, ?2, ?3, ?1, ?1, '', '', 0, 1, 1)",
            params_from_iter(row),
        )
        .await
        .unwrap();
    }
    let mut products = Vec::new();
    let mut filed = Vec::new();
    let mut listed = Vec::new();
    let mut options = Vec::new();
    for n in 0..PRODUCTS {
        let id = format!("p{n:06}");
        let mut name = format!("{} {}", rng.pick(NOUNS), rng.pick(ADJECTIVES));
        if rng.below(2) == 0 {
            name = format!("{name} {}", rng.pick(ADJECTIVES));
        }
        if rng.below(3) == 0 {
            name = format!("{name} {}", rng.pick(MOTIFS));
        }
        let name = format!("{name} {}", n % 1000);
        let slug = format!("{}-{n}", facet_key(&name));
        let status = match rng.below(100) {
            0..90 => "published",
            90..95 => "draft",
            _ => "archived",
        };
        let updated_at = 1_700_000_000 + rng.below(30_000_000) as i64;
        let min = 300 + rng.below(30_000) as i64;
        let max = if rng.below(2) == 0 {
            min
        } else {
            min + rng.below(10_000) as i64
        };
        let (review_count, rating) = if rng.below(4) == 0 {
            (1 + rng.below(40) as i64, 300 + rng.below(201) as i64)
        } else {
            (0, 0)
        };
        let mut colours: BTreeSet<&str> = BTreeSet::new();
        if rng.below(100) < 56 {
            for _ in 0..1 + rng.below(6) {
                colours.insert(rng.pick(COLOURS));
            }
        }
        let mut sizes: BTreeSet<&str> = BTreeSet::new();
        if rng.below(100) < 31 {
            for _ in 0..2 + rng.below(4) {
                sizes.insert(rng.pick(SIZES));
            }
        }
        let mut shown = Vec::new();
        if !colours.is_empty() {
            shown.push(serde_json::json!({"id": "c", "name": "Couleur", "values": colours}));
        }
        if !sizes.is_empty() {
            shown.push(serde_json::json!({"id": "s", "name": "Taille", "values": sizes}));
        }
        let variants: Vec<_> = (0..colours.len().max(1) * sizes.len().max(1))
            .filter(|_| !shown.is_empty())
            .map(|v| {
                serde_json::json!({"id": format!("v{v}"), "selections": [],
                    "sku": format!("SKU-{n}-{v}"), "amount_cents": min, "currency": "EUR",
                    "image_id": null})
            })
            .collect();
        let images: Vec<_> = (0..5)
            .map(|i| {
                serde_json::json!({"id": format!("01JC5Z8Q7M3N4P5R6S7T{n:06}{i}"), "alt": name,
                    "width": 1200, "height": 1200, "thumb_width": 400, "thumb_height": 400})
            })
            .collect();
        products.push(vec![
            Value::from(id.as_str()),
            Value::from(slug.as_str()),
            Value::from(name.as_str()),
            Value::from(TEXT.repeat(1 + rng.below(12))),
            Value::from(min),
            Value::from("EUR"),
            Value::from(status),
            Value::from(updated_at),
            Value::from(serde_json::to_string(&images).unwrap()),
            Value::from(serde_json::to_string(&shown).unwrap()),
            Value::from(serde_json::to_string(&variants).unwrap()),
            Value::from("[]"),
            Value::from(min),
            Value::from(max),
            Value::from(review_count),
            Value::from(rating),
            Value::from(search_key(&slug)),
            Value::from(search_key(&name)),
        ]);
        // Under one leaf, half the time a sibling too; listed on theirs and
        // their parent's pages, and on the whole shop's.
        let first = rng.below(leaves.len());
        let mut under = BTreeSet::from([first]);
        if rng.below(2) == 0 {
            under.insert(first / CHILDREN * CHILDREN + rng.below(CHILDREN));
        }
        let mut pages = BTreeSet::from([WHOLE_SHOP.to_owned()]);
        for leaf in under {
            filed.push(vec![
                Value::from(id.as_str()),
                Value::from(leaves[leaf].as_str()),
            ]);
            pages.insert(leaves[leaf].clone());
            pages.insert(top[leaf / CHILDREN].clone());
        }
        if status == "published" {
            for page in pages {
                listed.push(vec![
                    Value::from(page),
                    Value::from(id.as_str()),
                    Value::from(updated_at),
                    Value::from(min),
                    Value::from(max),
                    Value::from(review_count),
                    Value::from(rating),
                ]);
            }
        }
        for (name, values) in [("Couleur", &colours), ("Taille", &sizes)] {
            for value in values {
                options.push(vec![
                    Value::from(id.as_str()),
                    Value::from(facet_key(name)),
                    Value::from(facet_key(value)),
                    Value::from(name),
                    Value::from(*value),
                ]);
            }
        }
    }
    insert(
        &conn,
        "INSERT INTO storefront_products (id, slug, name, description, amount_cents, currency,
            status, updated_at, images_json, options_json, variants_json, attributes_json,
            min_amount_cents, max_amount_cents, review_count, rating_x100, slug_search,
            name_search) VALUES",
        18,
        products,
    )
    .await;
    insert(
        &conn,
        "INSERT INTO storefront_product_categories (product_id, category_id) VALUES",
        2,
        filed,
    )
    .await;
    insert(
        &conn,
        "INSERT INTO storefront_category_products (category_id, product_id, updated_at,
            min_amount_cents, max_amount_cents, review_count, rating_x100) VALUES",
        7,
        listed,
    )
    .await;
    insert(
        &conn,
        "INSERT INTO storefront_product_options (product_id, name_key, value_key, name, value)
         VALUES",
        5,
        options,
    )
    .await;
    let reviews = (0..REVIEWS)
        .map(|n| {
            vec![
                Value::from(format!("r{n:06}")),
                Value::from(format!("p{:06}", (n * 7919) % PRODUCTS)),
                Value::from(1 + (n % 5) as i64),
                Value::from("Mon chat adore, je recommande."),
                Value::from("Marie"),
                Value::from("[]"),
                Value::from(1_700_000_000 + ((n * 104_729) % 30_000_000) as i64),
            ]
        })
        .collect();
    insert(
        &conn,
        "INSERT INTO storefront_reviews (id, product_id, rating, body, author_name, photos_json,
            published_at) VALUES",
        7,
        reviews,
    )
    .await;
    for statement in [
        "UPDATE storefront_categories SET subtree_count = (SELECT count(*)
            FROM storefront_category_products cp WHERE cp.category_id = storefront_categories.id)",
        "INSERT INTO storefront_shop (id, products) SELECT 1, count(*)
         FROM storefront_category_products WHERE category_id = ''",
        "INSERT INTO storefront_category_facets (category_id, name_key, value_key, value,
            products)
         SELECT cp.category_id, po.name_key, po.value_key, min(po.value), count(*)
         FROM storefront_category_products cp
         JOIN storefront_product_options po ON po.product_id = cp.product_id
         GROUP BY cp.category_id, po.name_key, po.value_key",
    ] {
        execute(&conn, statement, ()).await.unwrap();
    }
    let product: String = db
        .fetch_scalar(
            "SELECT slug FROM storefront_products WHERE status = 'published' LIMIT 1 OFFSET 1234",
            (),
        )
        .await
        .unwrap();
    Shop {
        db,
        top,
        leaves,
        product,
    }
}

/// Runs `run` once, then [`RUNS`] times, and prints the mean time.
async fn timed<F, Fut, T>(name: &str, mut run: F) -> T
where
    F: FnMut() -> Fut,
    Fut: Future<Output = T>,
{
    run().await;
    let started = Instant::now();
    let mut last = None;
    for _ in 0..RUNS {
        last = Some(run().await);
    }
    let each: Duration = started.elapsed() / RUNS;
    println!("  {name:<52} {each:>12.3?}");
    last.unwrap()
}

/// A listing page as `/boutique` and `/c/{slug}` read it: its products and
/// its colours.
async fn listing(db: &Db, query: &ProductQuery) -> (i64, usize) {
    let page = published_products(db, query).await.unwrap();
    let colours = published_option_values(db, query, "Couleur").await.unwrap();
    (page.total, colours.len())
}

#[tokio::test]
#[ignore = "seeds 20,000 products and times every page; run by hand with --ignored --nocapture"]
async fn pages_of_a_large_shop() {
    let Shop {
        db,
        top,
        leaves,
        product,
    } = shop().await;
    let top = top[3].clone();
    let leaf = leaves[13].clone();

    println!("home");
    timed("newest, picks, eight tiles, latest reviews", || async {
        let newest = published_products(
            &db,
            &ProductQuery::default()
                .sort(ProductSort::Newest)
                .per_page(8),
        )
        .await
        .unwrap();
        let picks = ProductQuery::default()
            .sort(ProductSort::TopRated)
            .exclude_all(newest.items.iter().map(|p| p.id.clone()))
            .per_page(6);
        published_products(&db, &picks).await.unwrap();
        for tile in &leaves[..8] {
            published_products(&db, &ProductQuery::default().in_category(tile).per_page(8))
                .await
                .unwrap();
        }
        crate::latest_reviews(&db, 3).await.unwrap();
    })
    .await;

    let newest = ProductQuery::default()
        .sort(ProductSort::Newest)
        .per_page(8);
    let items = timed("  newest", || published_products(&db, &newest))
        .await
        .unwrap()
        .items;
    let picks = ProductQuery::default()
        .sort(ProductSort::TopRated)
        .exclude_all(items.iter().map(|p| p.id.clone()))
        .per_page(6);
    timed("  picks", || published_products(&db, &picks))
        .await
        .unwrap();
    let tile = ProductQuery::default().in_category(&leaves[0]).per_page(8);
    timed("  one tile", || published_products(&db, &tile))
        .await
        .unwrap();
    timed("  latest reviews", || crate::latest_reviews(&db, 3))
        .await
        .unwrap();

    println!("a product");
    let found = timed("by slug", || published_product_by_slug(&db, &product))
        .await
        .unwrap()
        .unwrap();
    timed("related products", || {
        published_related_products(&db, &found, 4)
    })
    .await
    .unwrap();
    let cart: Vec<String> = vec![found.id.clone(), "p000010".into(), "p000020".into()];
    timed("cart drawer offers (three in the cart)", || {
        published_cart_recommendations(&db, &cart, 3)
    })
    .await
    .unwrap();

    println!("listings, page and colours");
    let shop = ProductQuery::default().per_page(36);
    let cases = [
        ("whole shop", shop.clone()),
        ("whole shop, page 20", shop.clone().page(20)),
        (
            "whole shop, 50 to 70",
            shop.clone().priced_between(Some(5_000), Some(7_000)),
        ),
        (
            "whole shop, rose",
            shop.clone().with_option("Couleur", "Rose"),
        ),
        ("top category", shop.clone().in_category(&top)),
        ("leaf category", shop.clone().in_category(&leaf)),
        (
            "top category, 50 to 70",
            shop.clone()
                .in_category(&top)
                .priced_between(Some(5_000), Some(7_000)),
        ),
        (
            "top category, gris",
            shop.clone()
                .in_category(&top)
                .with_option("Couleur", "Gris"),
        ),
        (
            "top category, gris, top rated",
            shop.clone()
                .in_category(&top)
                .with_option("Couleur", "Gris")
                .sort(ProductSort::TopRated),
        ),
    ];
    for (name, query) in cases {
        let (total, colours) = timed(name, || listing(&db, &query)).await;
        println!("      {total} products, {colours} colours");
        timed("  the page alone", || published_products(&db, &query))
            .await
            .unwrap();
    }

    println!("searches");
    for q in ["tapis", "chat", "dinosaures", "rafraichissant", "ta"] {
        let dropdown = ProductQuery::default().search(q).per_page(4);
        let page = timed(&format!("dropdown {q:?}"), || {
            published_products(&db, &dropdown)
        })
        .await
        .unwrap();
        println!("      {} found", page.total);
        let results = ProductQuery::default().search(q).per_page(36);
        timed(&format!("results page {q:?}, with colours"), || {
            listing(&db, &results)
        })
        .await;
    }
}
