use timada_core::{Address, Money};
use timada_invoice::{
    Command, InvoiceError, InvoiceStatus, IssueCreditNote, ListInvoices, count_invoices,
    credit_note_list_subscription, credit_notes_from_refunds_subscription, credit_notes_of_invoice,
    credit_notes_of_refunds, invoice_from_orders_subscription, invoice_id,
    invoice_list_subscription, list_invoices, load_credit_note, load_invoice, migrations,
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
        order_number: None,
        tax: None,
    }
}

fn all_migrations() -> Vec<Box<dyn sqlx_migrator::Migration<sqlx::Sqlite>>> {
    let mut all = migrations();
    all.extend(timada_payment::migrations());
    all
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

    // Second order takes the next number. Cancelled once paid, its invoice
    // stays issued: the refund is documented by credit notes instead.
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
    let cancelled = load_invoice(&executor, invoice_id(&order_2))
        .await?
        .ok_or_else(|| anyhow::anyhow!("invoice 2 missing"))?;
    assert_eq!(cancelled.status, InvoiceStatus::Issued);

    // An order cancelled before it is paid only has a draft: it is voided.
    let order_3 = orders.place_order(place_order("cart-3")).await?;
    sync().await?;
    orders.cancel_order(&order_3, "out of stock").await?;
    sync().await?;
    let voided = load_invoice(&executor, invoice_id(&order_3))
        .await?
        .ok_or_else(|| anyhow::anyhow!("invoice 3 missing"))?;
    assert_eq!(voided.status, InvoiceStatus::Voided);
    assert_eq!(voided.voided_reason.as_deref(), Some("order cancelled"));
    assert!(matches!(
        cmd.issue_invoice(invoice_id(&order_3)).await,
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

    // The admin listing: both invoices, filterable by status and number.
    invoice_list_subscription()
        .data(db.clone())
        .run_once(&executor)
        .await?;
    let all = list_invoices(&db, &ListInvoices::default()).await?;
    assert_eq!(all.len(), 3);
    let issued_only = ListInvoices {
        status: Some(InvoiceStatus::Issued),
        ..ListInvoices::default()
    };
    let rows = list_invoices(&db, &issued_only).await?;
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.issued_at.is_some()));
    let first = rows
        .iter()
        .find(|r| r.invoice_id == invoice_id(&order_1))
        .ok_or_else(|| anyhow::anyhow!("invoice 1 not listed"))?;
    assert_eq!(first.invoice_number, issued.invoice_number);
    assert_eq!(first.total_minor, 27_836);
    assert_eq!(count_invoices(&db, &issued_only).await?, 2);
    let by_number = ListInvoices {
        number: Some(format!("F{year}-000002")),
        ..ListInvoices::default()
    };
    let rows = list_invoices(&db, &by_number).await?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "issued");
    assert_eq!(rows[0].total_minor, 27_836 - 2_499);
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
async fn refunds_are_documented_by_credit_notes() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(all_migrations()).await?;
    let orders = timada_order::Command(&executor);
    let payments = timada_payment::Command(&executor);
    let order = orders.place_order(place_order("cart-1")).await?;
    let payment = payments
        .request_payment(timada_payment::RequestPayment {
            order_id: order.clone(),
            amount: Money::eur(27_836),
            method: timada_payment::PaymentMethod::Card,
        })
        .await?;
    payments.capture_payment(&payment, "psp-1".into()).await?;
    orders.mark_paid(&order, &payment).await?;
    sync_invoices(&executor, db.clone()).await?;

    let credit = || async {
        for _ in 0..2 {
            credit_notes_from_refunds_subscription()
                .data(db.clone())
                .run_once(&executor)
                .await?;
            credit_note_list_subscription()
                .data(db.clone())
                .run_once(&executor)
                .await?;
        }
        anyhow::Ok(credit_notes_of_invoice(&db, &invoice_id(&order)).await?)
    };

    // Two partial refunds, two credit notes numbered in their own sequence.
    payments
        .refund_payment(&payment, Money::eur(1_000), "goodwill".into())
        .await?;
    let notes = credit().await?;
    let year = timada_core::time::year_of(timada_core::time::now_unix_secs()?);
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].credit_note_number, format!("A{year}-000001"));
    assert_eq!(
        (notes[0].amount_minor, notes[0].reason.as_str()),
        (1_000, "goodwill")
    );

    payments
        .refund_payment(&payment, Money::eur(26_836), "returned".into())
        .await?;
    let notes = credit().await?;
    assert_eq!(notes.len(), 2);
    assert_eq!(notes[1].credit_note_number, format!("A{year}-000002"));
    let note = load_credit_note(&executor, &notes[1].credit_note_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("credit note missing"))?;
    assert_eq!(note.amount, Money::eur(26_836));
    assert_eq!(note.invoice_number, format!("F{year}-000001"));
    assert_eq!(note.order_id, order);
    let by_refund = credit_notes_of_refunds(&db, &[notes[0].refund_id.clone()]).await?;
    assert_eq!(by_refund.len(), 1);
    assert_eq!(by_refund[0].credit_note_number, notes[0].credit_note_number);

    // The invoice itself never changes.
    let invoice = load_invoice(&executor, invoice_id(&order))
        .await?
        .ok_or_else(|| anyhow::anyhow!("invoice missing"))?;
    assert_eq!(invoice.status, InvoiceStatus::Issued);
    assert_eq!(invoice.total, Money::eur(27_836));

    // Issuing again for the same refund returns the same note; crediting more
    // than the invoice, or a draft, is refused.
    let cmd = Command {
        executor: &executor,
        db: db.clone(),
    };
    let again = cmd
        .issue_credit_note(IssueCreditNote {
            refund_id: notes[0].refund_id.clone(),
            invoice_id: invoice_id(&order),
            amount: Money::eur(1_000),
            reason: "goodwill".into(),
        })
        .await?;
    assert_eq!(again, notes[0].credit_note_number);
    let too_much = cmd
        .issue_credit_note(IssueCreditNote {
            refund_id: "refund-x".into(),
            invoice_id: invoice_id(&order),
            amount: Money::eur(1),
            reason: "oops".into(),
        })
        .await;
    assert!(matches!(too_much, Err(InvoiceError::CreditExceedsInvoice)));
    let draft_order = orders.place_order(place_order("cart-2")).await?;
    sync_invoices(&executor, db.clone()).await?;
    let on_draft = cmd
        .issue_credit_note(IssueCreditNote {
            refund_id: "refund-y".into(),
            invoice_id: invoice_id(&draft_order),
            amount: Money::eur(1),
            reason: "oops".into(),
        })
        .await;
    assert!(matches!(on_draft, Err(InvoiceError::InvoiceNotIssued)));
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
