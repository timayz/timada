//! The VAT journal and the quarterly report read out of it: the shop's own
//! VAT, the one-stop-shop return with its corrections, exports.

use timada_core::{Address, Money};
use timada_invoice::{
    Command, IssueCreditNote, VatCorrection, VatPeriod, VatReportLine,
    invoice_from_orders_subscription, invoice_id, load_invoice, migrations, oss_csv,
    vat_journal_subscription, vat_report,
};
use timada_order::{DeliveryChoice, OrderLine, OrderTax, PaymentMode, PlaceOrder, Seller};
use timada_tax::TaxTreatment;

fn address(country: &str) -> Address {
    Address {
        first_name: "Ada".into(),
        last_name: "Lovelace".into(),
        line1: "12 rue des Machines".into(),
        postal_code: "31000".into(),
        city: "Toulouse".into(),
        country_code: country.into(),
        ..Address::default()
    }
}

/// 100,00 of goods and 20,00 of shipping, delivered to `country`.
fn order(
    cart: &str,
    country: &str,
    zone: &str,
    treatment: TaxTreatment,
    rate_bp: u16,
) -> PlaceOrder {
    PlaceOrder {
        cart_id: cart.into(),
        customer_id: "customer-1".into(),
        seller: Seller::Ldlc,
        lines: vec![OrderLine {
            product_id: "sku-1".into(),
            name: "Produit".into(),
            quantity: 1,
            unit_price: Money::eur(10_000),
            warranty_months: 24,
        }],
        delivery_address: address(country),
        // Billed at home: the destination is what counts.
        billing_address: address("FR"),
        delivery: DeliveryChoice {
            method_code: "colissimo".into(),
            pickup_store_id: None,
        },
        payment_mode: PaymentMode::Card,
        shipping_fee: Money::eur(2_000),
        handling_fee: Money::eur(0),
        promo_code: None,
        discount: None,
        order_number: None,
        tax: Some(OrderTax {
            zone_code: zone.into(),
            treatment,
            line_rates: vec![("sku-1".into(), rate_bp)],
            shipping_rate_bp: rate_bp,
        }),
    }
}

fn line(country: &str, rate_bp: u16, base_minor: i64, vat_minor: i64) -> VatReportLine {
    VatReportLine {
        country_code: country.into(),
        rate_bp,
        base_minor,
        vat_minor,
    }
}

#[tokio::test]
async fn a_quarter_is_read_as_three_returns() -> anyhow::Result<()> {
    let mut all = migrations();
    all.extend(timada_order::migrations());
    let (executor, db) = timada_core::testing::memory_executor(all).await?;
    let orders = timada_order::Command(&executor);
    let invoices = Command {
        executor: &executor,
        db: db.clone(),
    };
    let journal = || async {
        vat_journal_subscription()
            .data(db.clone())
            .run_once(&executor)
            .await
    };

    let mut issued = Vec::new();
    for (cart, country, zone, treatment, rate) in [
        ("cart-fr", "FR", "fr", TaxTreatment::Domestic, 2_000),
        ("cart-fr-2", "FR", "fr", TaxTreatment::Domestic, 2_000),
        ("cart-de", "DE", "de", TaxTreatment::DestinationVat, 1_900),
        (
            "cart-de-old",
            "DE",
            "de",
            TaxTreatment::DestinationVat,
            1_900,
        ),
        ("cart-it", "IT", "it", TaxTreatment::DestinationVat, 2_200),
        ("cart-mq", "MQ", "fr-overseas", TaxTreatment::Export, 0),
    ] {
        let order_id = orders
            .place_order(order(cart, country, zone, treatment, rate))
            .await?;
        issued.push(invoice_id(&order_id));
    }
    // One more that stays a draft: no VAT is due on it yet.
    orders
        .place_order(order(
            "cart-draft",
            "ES",
            "es",
            TaxTreatment::DestinationVat,
            2_100,
        ))
        .await?;
    invoice_from_orders_subscription()
        .data(db.clone())
        .run_once(&executor)
        .await?;
    for id in &issued {
        invoices.issue_invoice(id).await?;
    }
    let [_, fr_2, de, de_old, it, mq] = &issued[..] else {
        anyhow::bail!("six invoices expected");
    };
    journal().await?;

    let now = VatPeriod::of(timada_core::time::now_unix_secs()?);
    let report = vat_report(&db, now).await?;
    assert_eq!(report.currency, "EUR");
    // 120,00 all taxes included: 100,00 + 20,00 at 20 %, twice.
    assert_eq!(report.domestic, [line("FR", 2_000, 20_000, 4_000)]);
    // By member state of *delivery*, at its rate; the draft is not there.
    assert_eq!(
        report.oss,
        [
            line("DE", 1_900, 20_168, 3_832),
            line("IT", 2_200, 9_836, 2_164),
        ]
    );
    assert_eq!(report.exports_base_minor, 12_000);
    assert_eq!(report.unbroken, (0, 0));
    assert!(report.oss_corrections.is_empty());
    assert_eq!(report.oss_vat_minor(), 5_996);
    let de_invoice = load_invoice(&executor, de)
        .await?
        .and_then(|invoice| invoice.tax)
        .ok_or_else(|| anyhow::anyhow!("no VAT on the German invoice"))?;
    assert_eq!(de_invoice.vat_lines[0].vat.minor * 2, 3_832);

    // One of the German invoices is from the quarter before.
    let (start, _) = now.bounds();
    sqlx::query(
        "UPDATE invoice_vat_journal SET issued_at = ?1, invoice_issued_at = ?1 WHERE invoice_id = ?2",
    )
    .bind(start - 86_400)
    .bind(de_old)
    .execute(&db)
    .await?;
    let earlier = vat_report(&db, now.previous()).await?;
    assert_eq!(earlier.oss, [line("DE", 1_900, 10_084, 1_916)]);
    assert!(earlier.domestic.is_empty());

    // Credit notes: half of this quarter's German sale, a quarter of the
    // earlier one, part of a domestic sale, part of the export.
    for (refund, invoice, cents) in [
        ("refund-de", de, 6_000),
        ("refund-de-old", de_old, 3_000),
        ("refund-fr", fr_2, 1_200),
        ("refund-mq", mq, 2_000),
    ] {
        invoices
            .issue_credit_note(IssueCreditNote {
                refund_id: refund.into(),
                invoice_id: invoice.clone(),
                amount: Money::eur(cents),
                reason: "retour".into(),
            })
            .await?;
    }
    journal().await?;
    // The journal runs again over the old invoice's credit note only; the
    // date moved by hand is what its correction refers to.
    sqlx::query("UPDATE invoice_vat_journal SET invoice_issued_at = ?1 WHERE invoice_id = ?2")
        .bind(start - 86_400)
        .bind(de_old)
        .execute(&db)
        .await?;

    let report = vat_report(&db, now).await?;
    // Same quarter: netted. 60,00 back → 50,42 + 9,58 at 19 %.
    assert_eq!(
        report.oss,
        [
            line("DE", 1_900, 10_084 - 5_042, 1_916 - 958),
            line("IT", 2_200, 9_836, 2_164),
        ]
    );
    // Earlier quarter: a correction of that quarter, for that member state.
    assert_eq!(
        report.oss_corrections,
        [VatCorrection {
            corrected: now.previous(),
            country_code: "DE".into(),
            vat_minor: -479,
        }]
    );
    assert_eq!(report.oss_vat_minor(), 958 + 2_164 - 479);
    assert_eq!(
        report.domestic,
        [line("FR", 2_000, 20_000 - 1_000, 4_000 - 200)]
    );
    assert_eq!(report.exports_base_minor, 10_000);
    // What was declared for the earlier quarter does not move.
    assert_eq!(vat_report(&db, now.previous()).await?, earlier);
    assert_eq!(vat_report(&db, now.next()).await?.currency, "");

    // Redelivered: the same journal.
    let rows = |db: sqlx::SqlitePool| async move {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM invoice_vat_journal")
            .fetch_one(&db)
            .await
    };
    let before = rows(db.clone()).await?;
    journal().await?;
    assert_eq!(rows(db.clone()).await?, before);

    let csv = oss_csv(now, &report);
    let expected = format!(
        "kind,period,corrected_period,member_state,vat_rate,taxable_amount,vat_amount,currency\n\
         sale,{now},,DE,19.00,50.42,9.58,EUR\n\
         sale,{now},,IT,22.00,98.36,21.64,EUR\n\
         correction,{now},{before},DE,,,-4.79,EUR\n",
        now = now.code(),
        before = now.previous().code(),
    );
    assert_eq!(csv, expected);
    let _ = it;
    Ok(())
}
