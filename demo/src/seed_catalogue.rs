//! A shop's worth of products for `--seed`: a few branches of the category
//! tree, a dozen brands, prices from a cable to a graphics card, some out of
//! stock, some reviewed — enough for the listing's filters to have something
//! to say. Safe to run again: what exists is left alone.

use timada_catalog::{
    Brand, CatalogError, Command, CreateCategory, CreateProduct, DescribeProduct, Media, MediaKind,
    Spec, SpecKey, category_id,
};
use timada_core::{Money, slug::slugify};
use timada_inventory::{RegisterStockItem, StockLocation};
use timada_pricing::ListPrice;

use crate::Store;

struct Item {
    sku: &'static str,
    name: &'static str,
    brand: &'static str,
    path: &'static [&'static str],
    cents: i64,
    stock: u32,
    /// The ratings of its published reviews.
    ratings: &'static [u8],
    feature: &'static str,
    /// Its technical sheet: `(group, label, value)`.
    specs: &'static [(&'static str, &'static str, &'static str)],
}

const SCREENS: &[&str] = &[
    "Informatique",
    "Périphériques",
    "Écran ordinateur",
    "Écran PC",
];
const KEYBOARDS: &[&str] = &["Informatique", "Périphériques", "Clavier"];
const MICE: &[&str] = &["Informatique", "Périphériques", "Souris"];
const GPUS: &[&str] = &["Informatique", "Composants", "Carte graphique"];
const SSDS: &[&str] = &["Informatique", "Composants", "SSD"];
const HEADSETS: &[&str] = &["Image & Son", "Casque"];
const SPEAKERS: &[&str] = &["Image & Son", "Enceinte"];
const LIGHTS: &[&str] = &["Maison", "Éclairage"];

/// What shoppers filter each category by: `(category slug, [(group, label)])`.
const FACETS: &[(&str, &[(&str, &str)])] = &[
    (
        "ecran-pc",
        &[
            ("Dalle", "Taille"),
            ("Dalle", "Type"),
            ("Dalle", "Définition"),
            ("Dalle", "Fréquence"),
        ],
    ),
    (
        "clavier",
        &[("Clavier", "Technologie"), ("Connexion", "Liaison")],
    ),
    ("souris", &[("Connexion", "Liaison")]),
    (
        "carte-graphique",
        &[("Puce", "Fabricant"), ("Mémoire", "Capacité")],
    ),
    (
        "ssd",
        &[("Stockage", "Capacité"), ("Stockage", "Interface")],
    ),
    ("image-son", &[("Connexion", "Liaison")]),
];

#[rustfmt::skip]
const ITEMS: &[Item] = &[
    Item { sku: "LG-27GP850", name: "LG 27\" UltraGear 27GP850", brand: "LG", path: SCREENS, cents: 34_995, stock: 8, ratings: &[5, 4, 5], feature: "Dalle Nano IPS QHD 165 Hz, 1 ms, HDR400", specs: &[("Dalle", "Taille", "27 pouces"), ("Dalle", "Type", "IPS"), ("Dalle", "Définition", "QHD"), ("Dalle", "Fréquence", "165 Hz")] },
    Item { sku: "LG-34WP65C", name: "LG 34\" UltraWide incurvé 34WP65C", brand: "LG", path: SCREENS, cents: 39_900, stock: 0, ratings: &[4], feature: "Dalle VA incurvée 21/9, 160 Hz, USB-C", specs: &[("Dalle", "Taille", "34 pouces"), ("Dalle", "Type", "VA"), ("Dalle", "Définition", "UWQHD"), ("Dalle", "Fréquence", "160 Hz")] },
    Item { sku: "SAM-ODY-G5", name: "Samsung 32\" Odyssey G5", brand: "Samsung", path: SCREENS, cents: 27_995, stock: 14, ratings: &[4, 3, 4, 5], feature: "Dalle VA incurvée QHD 144 Hz, FreeSync Premium", specs: &[("Dalle", "Taille", "32 pouces"), ("Dalle", "Type", "VA"), ("Dalle", "Définition", "QHD"), ("Dalle", "Fréquence", "144 Hz")] },
    Item { sku: "SAM-S24R35", name: "Samsung 24\" bureautique S24R35", brand: "Samsung", path: SCREENS, cents: 10_990, stock: 30, ratings: &[3, 4], feature: "Dalle IPS Full HD 75 Hz, bords fins", specs: &[("Dalle", "Taille", "24 pouces"), ("Dalle", "Type", "IPS"), ("Dalle", "Définition", "Full HD"), ("Dalle", "Fréquence", "75 Hz")] },
    Item { sku: "ASUS-VG249", name: "ASUS 24\" TUF Gaming VG249Q", brand: "ASUS", path: SCREENS, cents: 15_995, stock: 6, ratings: &[5, 5], feature: "Dalle IPS Full HD 144 Hz, pied réglable", specs: &[("Dalle", "Taille", "24 pouces"), ("Dalle", "Type", "IPS"), ("Dalle", "Définition", "Full HD"), ("Dalle", "Fréquence", "144 Hz")] },
    Item { sku: "AOC-Q27G2", name: "AOC 27\" Gaming Q27G2S", brand: "AOC", path: SCREENS, cents: 22_990, stock: 3, ratings: &[], feature: "Dalle IPS QHD 165 Hz, 1 ms", specs: &[("Dalle", "Taille", "27 pouces"), ("Dalle", "Type", "IPS"), ("Dalle", "Définition", "QHD"), ("Dalle", "Fréquence", "165 Hz")] },
    Item { sku: "LOG-MXKEYS", name: "Logitech MX Keys S", brand: "Logitech", path: KEYBOARDS, cents: 11_999, stock: 25, ratings: &[5, 5, 4], feature: "Clavier sans fil rétroéclairé, touches concaves", specs: &[("Clavier", "Technologie", "Ciseaux"), ("Connexion", "Liaison", "Sans fil")] },
    Item { sku: "LOG-G915", name: "Logitech G915 TKL", brand: "Logitech", path: KEYBOARDS, cents: 19_999, stock: 4, ratings: &[4], feature: "Clavier mécanique sans fil extra-plat, RGB", specs: &[("Clavier", "Technologie", "Mécanique"), ("Connexion", "Liaison", "Sans fil")] },
    Item { sku: "COR-K70", name: "Corsair K70 RGB Pro", brand: "Corsair", path: KEYBOARDS, cents: 16_990, stock: 9, ratings: &[4, 4, 5], feature: "Clavier mécanique Cherry MX Red, repose-poignet", specs: &[("Clavier", "Technologie", "Mécanique"), ("Connexion", "Liaison", "Filaire")] },
    Item { sku: "COR-K55", name: "Corsair K55 Core", brand: "Corsair", path: KEYBOARDS, cents: 4_999, stock: 0, ratings: &[3], feature: "Clavier à membrane silencieux, RGB dix zones", specs: &[("Clavier", "Technologie", "Membrane"), ("Connexion", "Liaison", "Filaire")] },
    Item { sku: "LOG-MX3S", name: "Logitech MX Master 3S", brand: "Logitech", path: MICE, cents: 10_999, stock: 40, ratings: &[5, 5, 5, 4], feature: "Souris sans fil 8000 dpi, clics silencieux", specs: &[("Capteur", "Résolution", "8000 dpi"), ("Connexion", "Liaison", "Sans fil")] },
    Item { sku: "LOG-G502X", name: "Logitech G502 X", brand: "Logitech", path: MICE, cents: 7_999, stock: 18, ratings: &[4, 5], feature: "Souris gamer filaire, capteur HERO 25K", specs: &[("Capteur", "Résolution", "25600 dpi"), ("Connexion", "Liaison", "Filaire")] },
    Item { sku: "COR-M65", name: "Corsair M65 RGB Ultra", brand: "Corsair", path: MICE, cents: 6_990, stock: 7, ratings: &[], feature: "Souris gamer filaire, châssis aluminium, poids réglables", specs: &[("Capteur", "Résolution", "26000 dpi"), ("Connexion", "Liaison", "Filaire")] },
    Item { sku: "ASUS-4070S", name: "ASUS Dual GeForce RTX 4070 SUPER", brand: "ASUS", path: GPUS, cents: 65_995, stock: 5, ratings: &[5, 4], feature: "12 Go GDDR6X, DLSS 3, double ventilateur", specs: &[("Mémoire", "Capacité", "12 Go"), ("Puce", "Fabricant", "NVIDIA")] },
    Item { sku: "MSI-4060", name: "MSI GeForce RTX 4060 Ventus 2X", brand: "MSI", path: GPUS, cents: 32_995, stock: 11, ratings: &[4, 4, 3], feature: "8 Go GDDR6, DLSS 3, format compact", specs: &[("Mémoire", "Capacité", "8 Go"), ("Puce", "Fabricant", "NVIDIA")] },
    Item { sku: "MSI-7800XT", name: "MSI Radeon RX 7800 XT Gaming Trio", brand: "MSI", path: GPUS, cents: 56_990, stock: 0, ratings: &[5], feature: "16 Go GDDR6, triple ventilateur", specs: &[("Mémoire", "Capacité", "16 Go"), ("Puce", "Fabricant", "AMD")] },
    Item { sku: "CRU-P3-1T", name: "Crucial P3 Plus 1 To", brand: "Crucial", path: SSDS, cents: 7_490, stock: 60, ratings: &[5, 4, 5, 5], feature: "SSD M.2 NVMe PCIe 4.0, 5000 Mo/s", specs: &[("Stockage", "Capacité", "1 To"), ("Stockage", "Interface", "NVMe PCIe 4.0")] },
    Item { sku: "CRU-MX500-2T", name: "Crucial MX500 2 To", brand: "Crucial", path: SSDS, cents: 13_990, stock: 22, ratings: &[4], feature: "SSD 2,5\" SATA, 560 Mo/s", specs: &[("Stockage", "Capacité", "2 To"), ("Stockage", "Interface", "SATA")] },
    Item { sku: "SAM-990P-2T", name: "Samsung 990 PRO 2 To", brand: "Samsung", path: SSDS, cents: 18_995, stock: 16, ratings: &[5, 5], feature: "SSD M.2 NVMe PCIe 4.0, 7450 Mo/s", specs: &[("Stockage", "Capacité", "2 To"), ("Stockage", "Interface", "NVMe PCIe 4.0")] },
    Item { sku: "SONY-XM5", name: "Sony WH-1000XM5", brand: "Sony", path: HEADSETS, cents: 34_900, stock: 13, ratings: &[5, 5, 4, 5], feature: "Casque Bluetooth à réduction de bruit, 30 h d'autonomie", specs: &[("Audio", "Réduction de bruit", "Oui"), ("Connexion", "Liaison", "Sans fil")] },
    Item { sku: "COR-HS80", name: "Corsair HS80 RGB Wireless", brand: "Corsair", path: HEADSETS, cents: 14_990, stock: 2, ratings: &[4, 3], feature: "Casque gamer sans fil, son spatial Dolby Atmos", specs: &[("Audio", "Réduction de bruit", "Non"), ("Connexion", "Liaison", "Sans fil")] },
    Item { sku: "JBL-FLIP6", name: "JBL Flip 6", brand: "JBL", path: SPEAKERS, cents: 12_999, stock: 35, ratings: &[5, 4, 4], feature: "Enceinte Bluetooth étanche IP67, 12 h d'autonomie", specs: &[("Batterie", "Autonomie", "12 h"), ("Résistance", "Étanchéité", "IP67")] },
    Item { sku: "JBL-CHARGE5", name: "JBL Charge 5", brand: "JBL", path: SPEAKERS, cents: 17_999, stock: 0, ratings: &[5], feature: "Enceinte Bluetooth étanche, batterie externe intégrée", specs: &[("Batterie", "Autonomie", "20 h"), ("Résistance", "Étanchéité", "IP67")] },
    Item { sku: "PHI-HUE-GO", name: "Philips Hue Go", brand: "Philips", path: LIGHTS, cents: 7_999, stock: 19, ratings: &[4, 2], feature: "Lampe connectée portable, 16 millions de couleurs", specs: &[("Éclairage", "Couleurs", "16 millions")] },
];

/// The category a breadcrumb names, opened on the way down. The slug is the
/// name's: these names are unique in the demo's tree.
async fn branch(catalog: &Command<'_, evento::Sqlite>, path: &[&str]) -> anyhow::Result<String> {
    let mut parent_id = None;
    for name in path {
        let opened = catalog
            .create_category(CreateCategory {
                name: (*name).to_owned(),
                slug: None,
                parent_id: parent_id.clone(),
            })
            .await;
        parent_id = Some(match opened {
            Ok(id) => id,
            Err(CatalogError::SlugAlreadyExists(slug)) => category_id(&slug),
            Err(err) => return Err(err.into()),
        });
    }
    parent_id.ok_or_else(|| anyhow::anyhow!("empty category path"))
}

/// Gives a product its technical sheet, unless it has one.
async fn specify(
    catalog: &Command<'_, evento::Sqlite>,
    product_id: &str,
    item: &Item,
) -> anyhow::Result<()> {
    let has_sheet = timada_catalog::load_product_page(catalog.0, product_id)
        .await?
        .is_some_and(|product| !product.specs.is_empty());
    if has_sheet || item.specs.is_empty() {
        return Ok(());
    }
    let specs = item
        .specs
        .iter()
        .map(|(group, label, value)| Spec {
            group: (*group).into(),
            label: (*label).into(),
            value: (*value).into(),
        })
        .collect();
    match catalog.specify_product(product_id, specs).await {
        Ok(()) | Err(CatalogError::ProductArchived) => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// The demo's prices in its other currencies: a rule of thumb rounded to
/// `…,90`, as a shop would — decided once, never computed at checkout. Every
/// fifth product has no price in francs: it is simply not sold there.
async fn price_abroad<E: evento::Executor>(
    executor: &E,
    product_id: &str,
    index: usize,
    cents: i64,
) -> anyhow::Result<()> {
    let pricing = timada_pricing::Command(executor);
    let rounded = |percent: i64| ((cents * percent / 100) / 100).max(1) * 100 + 90;
    let mut prices = vec![Money::new(rounded(88), "GBP")];
    if index % 5 != 4 {
        prices.push(Money::new(rounded(97), "CHF"));
    }
    for price in prices {
        match pricing
            .set_currency_price(timada_pricing::price_id(product_id), price)
            .await
        {
            Ok(())
            | Err(
                timada_pricing::PricingError::PriceWithdrawn
                | timada_pricing::PricingError::PriceNotFound,
            ) => {}
            Err(err) => return Err(err.into()),
        }
    }
    Ok(())
}

pub async fn run(store: &Store) -> anyhow::Result<()> {
    let executor = &store.executor;
    let catalog = Command(executor);
    let mut added = 0;
    for (index, item) in ITEMS.iter().enumerate() {
        let created = catalog
            .create_product(CreateProduct {
                sku: item.sku.into(),
                name: item.name.into(),
                brand: Brand {
                    name: item.brand.into(),
                    slug: slugify(item.brand),
                },
                category_path: item.path.iter().map(|s| (*s).to_owned()).collect(),
                short_description: item.feature.into(),
                warranty_months: 24,
            })
            .await;
        let product_id = match created {
            Ok(id) => id,
            // Seeded before: only what later versions of the seed added.
            Err(CatalogError::SkuAlreadyExists(_)) => {
                let known = timada_catalog::product_id(item.sku);
                specify(&catalog, &known, item).await?;
                price_abroad(executor, &known, index, item.cents).await?;
                continue;
            }
            Err(err) => return Err(err.into()),
        };
        specify(&catalog, &product_id, item).await?;
        let category = branch(&catalog, item.path).await?;
        catalog.categorise_product(&product_id, category).await?;
        catalog
            .describe_product(
                &product_id,
                DescribeProduct {
                    long_description: format!("{} — {}.", item.name, item.feature),
                    key_features: item.feature.split(", ").map(str::to_owned).collect(),
                },
            )
            .await?;
        catalog
            .add_product_media(
                &product_id,
                Media {
                    url: format!("/media/demo/{}.svg", item.sku.to_lowercase()),
                    kind: MediaKind::Image,
                    alt: item.name.into(),
                },
            )
            .await?;
        timada_pricing::Command(executor)
            .list_price(ListPrice {
                product_id: product_id.clone(),
                price_incl_tax: Money::eur(item.cents),
                vat_rate_bp: 2_000,
                eco_participation: Money::eur(0),
            })
            .await?;
        price_abroad(executor, &product_id, index, item.cents).await?;
        let inventory = timada_inventory::Command(executor);
        let stock = inventory
            .register_stock_item(RegisterStockItem {
                product_id: product_id.clone(),
                location: StockLocation::Warehouse,
            })
            .await?;
        if item.stock > 0 {
            inventory.receive_stock(&stock, item.stock).await?;
        }
        let reviews = timada_review::Command(executor);
        for (index, rating) in item.ratings.iter().enumerate() {
            let review = reviews
                .submit_review(timada_review::SubmitReview {
                    product_id: product_id.clone(),
                    customer_id: format!("demo-reviewer-{index}"),
                    order_id: None,
                    rating: *rating,
                    title: "Avis de démonstration".into(),
                    body: format!(
                        "{} : noté {rating} sur 5 par un client de démonstration.",
                        item.name
                    ),
                })
                .await?;
            reviews.publish_review(&review).await?;
        }
        added += 1;
    }
    for (slug, facets) in FACETS {
        let facets = facets
            .iter()
            .map(|(group, label)| SpecKey::new(*group, *label))
            .collect();
        match catalog
            .define_category_facets(category_id(slug), facets)
            .await
        {
            Ok(()) | Err(CatalogError::CategoryNotFound | CatalogError::CategoryArchived) => {}
            Err(err) => return Err(err.into()),
        }
    }
    tracing::info!(added, "catalogue seeded");
    Ok(())
}
