use timada_core::{Address, Money};
use timada_invoice::{
    Command, InvoiceError, InvoiceStatus, invoice_from_orders_subscription, invoice_id,
    load_invoice, migrations,
};
use timada_order::{DeliveryChoice, OrderLine, PaymentMode, PlaceOrder, Seller, order_id};

fn address() -> Address {
    Address {
        first_name: "Jonathan".into(),
        last_name: "Lapiquonne".into(),
        line1: "121, Avenue Tolosane".into(),
        postal_code: "31520".into(),
        city: "Ramonville-Saint-Agne".into(),
        country_code: "FR".into(),
        ..Address::default()
    }
}

fn place_order(cart_id: &str) -> PlaceOrder {
    PlaceOrder {
        cart_id: cart_id.into(),
        customer_id: "customer-1".into(),
        seller: Seller::Ldlc,
        lines: vec![OrderLine {
            product_id: "aoc-24g4xe".into(),
            name: "AOC 23.8\" LED - 24G4XE".into(),
            quantity: 2,
            unit_price: Money::eur(12_496),
            warranty_months: 60,
        }],
        delivery_address: address(),
        billing_address: address(),
        delivery: DeliveryChoice {
            method_code: "chronopost-dom".into(),
            pickup_store_id: None,
        },
        payment_mode: PaymentMode::Installments { count: 3 },
        shipping_fee: Money::eur(2_395),
        handling_fee: Money::eur(449),
        promo_code: None,
        discount: None,
    }
}

async fn sync_invoices(executor: &evento::Sqlite, db: sqlx::SqlitePool) -> anyhow::Result<()> {
    invoice_from_orders_subscription()
        .data(db)
        .run_once(executor)
        .await
}

#[tokio::test]
async fn orders_drive_drafting_numbering_and_voiding() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let orders = timada_order::Command(&executor);
    let order_1 = orders.place_order(place_order("cart-1")).await?;
    let order_2 = orders
        .place_order(PlaceOrder {
            promo_code: Some("WELCOME10".into()),
            discount: Some(timada_order::OrderDiscount {
                code: "WELCOME10".into(),
                kind: timada_order::PromoKind::Discount,
                amount: Money::eur(2_499),
            }),
            ..place_order("cart-2")
        })
        .await?;
    assert_eq!(order_1, order_id("cart-1"));

    let sync = || sync_invoices(&executor, db.clone());

    // Placed → drafted, unnumbered.
    sync().await?;
    let draft = load_invoice(&executor, invoice_id(&order_1))
        .await?
        .ok_or_else(|| anyhow::anyhow!("invoice 1 not drafted"))?;
    assert_eq!(draft.status, InvoiceStatus::Draft);
    assert_eq!(draft.invoice_number, None);
    assert_eq!(draft.subtotal, Money::eur(24_992));
    assert_eq!(draft.total, Money::eur(27_836));
    assert_eq!(draft.discount, None);
    // The order's code is a reduction line: the invoice bills what was paid.
    let discounted = load_invoice(&executor, invoice_id(&order_2))
        .await?
        .ok_or_else(|| anyhow::anyhow!("invoice 2 not drafted"))?;
    assert_eq!(
        discounted.discount.as_ref().map(|d| d.label.as_str()),
        Some("Code promo WELCOME10")
    );
    assert_eq!(discounted.total, Money::eur(27_836 - 2_499));

    // Paid → issued with the first number of the year.
    orders.mark_paid(&order_1, "pay-1").await?;
    sync().await?;
    let year = timada_core::time::year_of(timada_core::time::now_unix_secs()?);
    let issued = load_invoice(&executor, invoice_id(&order_1))
        .await?
        .ok_or_else(|| anyhow::anyhow!("invoice 1 missing"))?;
    assert_eq!(issued.status, InvoiceStatus::Issued);
    assert_eq!(
        issued.invoice_number.as_deref(),
        Some(format!("F{year}-000001").as_str())
    );

    // Issuing again returns the same number and writes nothing.
    let cmd = Command {
        executor: &executor,
        db: db.clone(),
    };
    let again = cmd.issue_invoice(invoice_id(&order_1)).await?;
    assert_eq!(again, format!("F{year}-000001"));
    assert_eq!(
        load_invoice(&executor, invoice_id(&order_1))
            .await?
            .as_ref(),
        Some(&issued)
    );

    // Second order takes the next number, then its cancellation voids it.
    orders.mark_paid(&order_2, "pay-2").await?;
    sync().await?;
    let second = load_invoice(&executor, invoice_id(&order_2))
        .await?
        .ok_or_else(|| anyhow::anyhow!("invoice 2 missing"))?;
    assert_eq!(
        second.invoice_number.as_deref(),
        Some(format!("F{year}-000002").as_str())
    );
    orders
        .cancel_order(&order_2, "customer changed mind")
        .await?;
    sync().await?;
    let voided = load_invoice(&executor, invoice_id(&order_2))
        .await?
        .ok_or_else(|| anyhow::anyhow!("invoice 2 missing"))?;
    assert_eq!(voided.status, InvoiceStatus::Voided);
    assert_eq!(voided.voided_reason.as_deref(), Some("order cancelled"));
    assert!(matches!(
        cmd.issue_invoice(invoice_id(&order_2)).await,
        Err(InvoiceError::InvoiceVoided)
    ));

    // Redelivery changes nothing.
    sync().await?;
    assert_eq!(
        load_invoice(&executor, invoice_id(&order_1))
            .await?
            .as_ref(),
        Some(&issued)
    );
    Ok(())
}

#[tokio::test]
async fn order_settled_without_payment_is_invoiced_at_zero() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let orders = timada_order::Command(&executor);
    // Collected in store and entirely covered by a voucher.
    let order = orders
        .place_order(PlaceOrder {
            delivery: DeliveryChoice {
                method_code: "store-pickup".into(),
                pickup_store_id: Some("store-toulouse".into()),
            },
            payment_mode: PaymentMode::Card,
            shipping_fee: Money::eur(0),
            handling_fee: Money::eur(0),
            promo_code: Some("GIFT250".into()),
            discount: Some(timada_order::OrderDiscount {
                code: "GIFT250".into(),
                kind: timada_order::PromoKind::Voucher,
                amount: Money::eur(24_992),
            }),
            ..place_order("cart-free")
        })
        .await?;
    orders.settle_order(&order).await?;
    sync_invoices(&executor, db.clone()).await?;

    let invoice = load_invoice(&executor, invoice_id(&order))
        .await?
        .ok_or_else(|| anyhow::anyhow!("invoice not drafted"))?;
    assert_eq!(invoice.status, InvoiceStatus::Issued);
    assert!(invoice.invoice_number.is_some());
    assert_eq!(invoice.total, Money::eur(0));
    Ok(())
}

#[tokio::test]
async fn drafting_twice_returns_the_same_invoice() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command {
        executor: &executor,
        db,
    };
    let draft = timada_invoice::DraftInvoice {
        order_id: "order-1".into(),
        customer_id: "customer-1".into(),
        billing_address: address(),
        lines: vec![timada_invoice::InvoiceLine {
            product_id: "p1".into(),
            label: "Thing".into(),
            quantity: 1,
            unit_price: Money::eur(1_000),
        }],
        shipping_fee: Money::eur(0),
        handling_fee: Money::eur(0),
        discount: None,
    };
    let first = cmd.draft_invoice(draft.clone()).await?;
    let second = cmd.draft_invoice(draft.clone()).await?;
    assert_eq!(first, second);
    assert_eq!(first, invoice_id("order-1"));

    let empty = timada_invoice::DraftInvoice {
        lines: vec![],
        ..draft
    };
    assert!(matches!(
        cmd.draft_invoice(empty).await,
        Err(InvoiceError::NoLines)
    ));
    Ok(())
}
