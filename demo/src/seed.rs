//! Sample data: the AOC monitor from the mockups, stock, a customer with the
//! mockup's addresses, one checked-out cart (which the process managers turn
//! into an order), the shopper's login and the admin account.

use timada_cart::{AddLine, Checkout};
use timada_catalog::{Brand, CreateProduct, DescribeProduct, Spec};
use timada_core::{Address, Civility, Money};
use timada_customer::RegisterCustomer;
use timada_inventory::{RegisterStockItem, StockLocation};
use timada_pricing::{InstallmentOffer, ListPrice};

use crate::Store;

pub const PROMO_CODE: &str = "BIENVENUE10";
pub const SHOPPER_EMAIL: &str = "jonathan@example.com";
pub const SHOPPER_PASSWORD: &str = "demo1234";

pub async fn run(store: &Store) -> anyhow::Result<()> {
    let executor = &store.executor;

    match timada_admin::create_admin(&store.db, "admin@timada.example", "admin").await {
        Ok(_) | Err(timada_admin::AdminError::EmailTaken(_)) => {}
        Err(err) => return Err(err.into()),
    }

    // A promo code to try in the cart: 10 % off the goods.
    let promotion = timada_promotion::Command {
        executor,
        db: store.db.clone(),
    };
    match promotion
        .create_discount(timada_promotion::CreateDiscount {
            code: PROMO_CODE.into(),
            kind: timada_promotion::DiscountKind::Percent { bp: 1_000 },
            max_redemptions: None,
            valid_until: None,
        })
        .await
    {
        Ok(_) | Err(timada_promotion::PromotionError::CodeAlreadyExists(_)) => {}
        Err(err) => return Err(err.into()),
    }

    let catalog = timada_catalog::Command(executor);
    let product_id = match catalog
        .create_product(CreateProduct {
            sku: "AOC-24G4XE".into(),
            name: "AOC 23.8\" LED - 24G4XE".into(),
            brand: Brand {
                name: "AOC".into(),
                slug: "aoc".into(),
            },
            category_path: vec![
                "Informatique".into(),
                "Périphériques".into(),
                "Ecran ordinateur".into(),
                "Ecran PC".into(),
            ],
            short_description:
                "Ecran PC Full HD 1080p - 1920 x 1080 pixels - 1 ms (gris à gris) - 16/9 - Dalle IPS - 180 Hz - HDR10 - Adaptive Sync / G-SYNC Compatible - DisplayPort/HDMI - Noir"
                    .into(),
            warranty_months: 60,
        })
        .await
    {
        Ok(id) => id,
        Err(timada_catalog::CatalogError::SkuAlreadyExists(_)) => {
            tracing::info!("already seeded");
            return attach_shopper_login(store).await;
        }
        Err(err) => return Err(err.into()),
    };
    catalog
        .describe_product(
            &product_id,
            DescribeProduct {
                long_description: "Avec le moniteur gaming AOC 24G4XE, vous bénéficiez d'un bon environnement de jeu pour accueillir vos victoires !".into(),
                key_features: vec![
                    "Écran IPS de 23.8 pouces avec résolution Full HD (1920 x 1080 pixels)".into(),
                    "Affichage ultra-fluide avec une fréquence d'affichage de 180 Hz".into(),
                    "Temps de réponse : 1 ms (gris à gris)".into(),
                    "2 connecteurs HDMI 2.0 + 1 connecteur DisplayPort 1.4".into(),
                ],
            },
        )
        .await?;
    catalog
        .specify_product(
            &product_id,
            vec![
                Spec {
                    group: "Dalle".into(),
                    label: "Taille".into(),
                    value: "23.8\"".into(),
                },
                Spec {
                    group: "Dalle".into(),
                    label: "Fréquence".into(),
                    value: "180 Hz".into(),
                },
            ],
        )
        .await?;

    let pricing = timada_pricing::Command(executor);
    pricing
        .list_price(ListPrice {
            product_id: product_id.clone(),
            price_incl_tax: Money::eur(11_995),
            vat_rate_bp: 2_000,
            eco_participation: Money::eur(170),
        })
        .await?;
    pricing
        .attach_installment_offer(
            timada_pricing::price_id(&product_id),
            InstallmentOffer {
                count: 3,
                fee: Money::eur(479),
            },
        )
        .await?;

    let inventory = timada_inventory::Command(executor);
    let stock = inventory
        .register_stock_item(RegisterStockItem {
            product_id: product_id.clone(),
            location: StockLocation::Warehouse,
        })
        .await?;
    inventory.receive_stock(&stock, 5).await?;

    let customers = timada_customer::Command(executor);
    let customer_id = customers
        .register_customer(RegisterCustomer {
            email: SHOPPER_EMAIL.into(),
            civility: Civility::Mr,
            first_name: "Jonathan".into(),
            last_name: "Lapiquonne".into(),
        })
        .await?;
    crate::auth::attach_account(store, SHOPPER_EMAIL, SHOPPER_PASSWORD, &customer_id).await?;
    let billing = Address {
        civility: Civility::Mr,
        first_name: "Jonathan".into(),
        last_name: "Lapiquonne".into(),
        line1: "Bologas".into(),
        line2: None,
        postal_code: "59390".into(),
        city: "Le gros morn".into(),
        country_code: "MQ".into(),
        phone: Some("0596542473".into()),
        mobile: None,
    };
    let delivery = Address {
        line1: "121, Avenue Tolosane".into(),
        line2: Some("Apt A21".into()),
        postal_code: "31520".into(),
        city: "Ramonville-Saint-Agne".into(),
        country_code: "FR".into(),
        phone: Some("0721358738".into()),
        ..billing.clone()
    };
    customers
        .set_billing_address(&customer_id, billing.clone())
        .await?;
    customers
        .add_delivery_address(&customer_id, delivery.clone())
        .await?;

    let cart = timada_cart::Command(executor);
    let cart_id = cart.open_cart(Some(customer_id.clone())).await?;
    cart.add_line(
        &cart_id,
        AddLine {
            product_id: product_id.clone(),
            name: "AOC 23.8\" LED - 24G4XE".into(),
            quantity: 1,
            unit_price: Money::eur(11_995),
            warranty_months: 60,
        },
    )
    .await?;
    cart.checkout(
        &cart_id,
        Checkout {
            customer_id: None,
            delivery_address: delivery,
            billing_address: billing,
            delivery: timada_cart::DeliveryChoice {
                method_code: "chronopost-dom".into(),
                pickup_store_id: None,
            },
            payment_mode: timada_cart::PaymentMode::Installments { count: 3 },
        },
    )
    .await?;

    tracing::info!(%product_id, %customer_id, %cart_id, "seeded");
    Ok(())
}

/// A database seeded before shopper accounts existed has the customer but no
/// login; the customer list read model says which customer it is.
async fn attach_shopper_login(store: &Store) -> anyhow::Result<()> {
    let customers = timada_customer::list_customers(
        &store.db,
        &timada_customer::ListCustomers {
            q: Some(SHOPPER_EMAIL.into()),
            ..Default::default()
        },
    )
    .await?;
    if let Some(customer) = customers.iter().find(|c| c.email == SHOPPER_EMAIL) {
        crate::auth::attach_account(
            store,
            SHOPPER_EMAIL,
            SHOPPER_PASSWORD,
            &customer.customer_id,
        )
        .await?;
    }
    Ok(())
}
