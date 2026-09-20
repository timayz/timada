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
        business: None,
        exchange_rate: None,
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

#[tokio::test]
async fn a_reverse_charged_sale_is_neither_an_export_nor_the_one_stop_shops() -> anyhow::Result<()>
{
    use timada_invoice::{IntraCommunityLine, load_invoice_document};
    use timada_tax::{BusinessBuyer, BusinessPurchase, REVERSE_CHARGE_MENTION, ReverseChargeProof};

    let mut all = migrations();
    all.extend(timada_order::migrations());
    let (executor, db) = timada_core::testing::memory_executor(all).await?;
    let orders = timada_order::Command(&executor);
    let invoices = Command {
        executor: &executor,
        db: db.clone(),
    };
    let business = BusinessPurchase {
        buyer: BusinessBuyer {
            company_name: "Hermann GmbH".into(),
            vat_number: "DE123456789".into(),
        },
        reverse_charge: Some(ReverseChargeProof {
            consultation_ref: Some("WAPIAAAAY1X2Z3".into()),
            checked_at: 1_789_000_000,
        }),
    };
    // A reverse charge is a sale without VAT: refused on a taxed order.
    let taxed = orders
        .place_order(PlaceOrder {
            business: Some(business.clone()),
            ..order(
                "cart-wrong",
                "DE",
                "de",
                TaxTreatment::DestinationVat,
                1_900,
            )
        })
        .await;
    assert!(taxed.is_err());

    // To a German business, without VAT; to a French one, with — and named.
    let exempt = orders
        .place_order(PlaceOrder {
            business: Some(business.clone()),
            ..order("cart-b2b", "DE", "de", TaxTreatment::Export, 0)
        })
        .await?;
    let domestic = orders
        .place_order(PlaceOrder {
            business: Some(BusinessPurchase {
                buyer: BusinessBuyer {
                    company_name: "Machines SARL".into(),
                    vat_number: "FR40303265045".into(),
                },
                reverse_charge: None,
            }),
            ..order("cart-b2b-fr", "FR", "fr", TaxTreatment::Domestic, 2_000)
        })
        .await?;
    let export = orders
        .place_order(order(
            "cart-mq",
            "MQ",
            "fr-overseas",
            TaxTreatment::Export,
            0,
        ))
        .await?;
    invoice_from_orders_subscription()
        .data(db.clone())
        .run_once(&executor)
        .await?;
    for order_id in [&exempt, &domestic, &export] {
        invoices.issue_invoice(invoice_id(order_id)).await?;
    }
    vat_journal_subscription()
        .data(db.clone())
        .run_once(&executor)
        .await?;

    // The invoice names the business and says who owes the VAT.
    let invoice = load_invoice(&executor, invoice_id(&exempt))
        .await?
        .ok_or_else(|| anyhow::anyhow!("invoice missing"))?;
    assert_eq!(invoice.company, Some(business.buyer.clone()));
    assert_eq!(invoice.reverse_charge, business.reverse_charge);
    assert_eq!(invoice.regime_mention(), Some(REVERSE_CHARGE_MENTION));
    let issuer = timada_invoice::InvoiceIssuer::default();
    let document = load_invoice_document(&executor, &db, &issuer, &invoice_id(&exempt))
        .await?
        .ok_or_else(|| anyhow::anyhow!("no document"))?;
    assert_eq!(
        document.company_lines(),
        ["Hermann GmbH", "N° TVA : DE123456789"]
    );
    assert_eq!(document.regime_mention, Some(REVERSE_CHARGE_MENTION));
    assert!(!document.amounts_include_vat);
    // A domestic business is named too; its VAT is the ordinary one.
    let document = load_invoice_document(&executor, &db, &issuer, &invoice_id(&domestic))
        .await?
        .ok_or_else(|| anyhow::anyhow!("no document"))?;
    assert_eq!(document.company_lines()[1], "N° TVA : FR40303265045");
    assert_eq!(document.regime_mention, None);

    let now = VatPeriod::of(timada_core::time::now_unix_secs()?);
    let report = vat_report(&db, now).await?;
    assert_eq!(
        report.intra_community,
        [IntraCommunityLine {
            country_code: "DE".into(),
            buyer_vat_number: "DE123456789".into(),
            base_minor: 12_000,
        }]
    );
    assert!(report.oss.is_empty());
    assert_eq!(report.exports_base_minor, 12_000, "the export alone");
    assert_eq!(report.domestic, [line("FR", 2_000, 10_000, 2_000)]);

    // A credit note on the exempt sale comes off the same line.
    invoices
        .issue_credit_note(IssueCreditNote {
            refund_id: "refund-b2b".into(),
            invoice_id: invoice_id(&exempt),
            amount: Money::eur(2_000),
            reason: "retour".into(),
        })
        .await?;
    vat_journal_subscription()
        .data(db.clone())
        .run_once(&executor)
        .await?;
    let report = vat_report(&db, now).await?;
    assert_eq!(report.intra_community[0].base_minor, 10_000);
    assert_eq!(report.exports_base_minor, 12_000);
    Ok(())
}

/// A pound order: 100,00 £ of goods and 20,00 £ of delivery, all at 20 %.
fn pound_order(cart: &str, rate: Option<timada_tax::PinnedRate>) -> PlaceOrder {
    let mut order = order(cart, "FR", "fr", TaxTreatment::Domestic, 2_000);
    order.lines[0].unit_price = Money::new(10_000, "GBP");
    order.shipping_fee = Money::new(2_000, "GBP");
    order.handling_fee = Money::new(0, "GBP");
    order.exchange_rate = rate;
    order
}

fn pounds_per_euro() -> timada_tax::PinnedRate {
    timada_tax::PinnedRate {
        base: "EUR".into(),
        currency: "GBP".into(),
        // 1 EUR = 0,80 GBP: 120,00 £ are 150,00 €.
        per_base_micros: 800_000,
        as_of: 1_789_689_600,
        source: "BCE".into(),
    }
}

#[tokio::test]
async fn a_sale_in_pounds_goes_to_the_books_in_euros_at_its_orders_rate() -> anyhow::Result<()> {
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
    let period = VatPeriod::of(timada_core::time::now_unix_secs()?);

    // A euro sale, a pound sale with its rate, a pound sale without one (the
    // source was down when it was placed).
    let euros = orders
        .place_order(order("cart-eur", "FR", "fr", TaxTreatment::Domestic, 2_000))
        .await?;
    let pinned = orders
        .place_order(pound_order("cart-gbp", Some(pounds_per_euro())))
        .await?;
    let waiting = orders
        .place_order(pound_order("cart-gbp-late", None))
        .await?;
    // A rate is of the order's currency, or it is refused.
    assert!(matches!(
        orders
            .place_order(pound_order(
                "cart-gbp-wrong",
                Some(timada_tax::PinnedRate {
                    currency: "CHF".into(),
                    ..pounds_per_euro()
                })
            ))
            .await,
        Err(timada_order::OrderError::RateOfAnotherCurrency { .. })
    ));
    invoice_from_orders_subscription()
        .data(db.clone())
        .run_once(&executor)
        .await?;
    for order_id in [&euros, &pinned, &waiting] {
        invoices.issue_invoice(invoice_id(order_id)).await?;
    }
    // Half of the pinned sale comes back.
    invoices
        .issue_credit_note(IssueCreditNote {
            refund_id: "refund-gbp".into(),
            invoice_id: invoice_id(&pinned),
            amount: Money::new(6_000, "GBP"),
            reason: "geste".into(),
        })
        .await?;
    journal().await?;

    // Euros and converted pounds add up; the sale without a rate stays out,
    // and the report says so.
    let report = vat_report(&db, period).await?;
    assert_eq!(report.currency, "EUR");
    assert_eq!(report.unconverted, 1);
    // 100,00 € + 20,00 € of VAT, then 125,00 € + 25,00 €, less 62,50 € + 12,50 €.
    assert_eq!(report.domestic, [line("FR", 2_000, 16_250, 3_250)]);

    // The documents say the same, in the same euros.
    let issuer = timada_invoice::InvoiceIssuer::default();
    let document =
        timada_invoice::load_invoice_document(&executor, &db, &issuer, &invoice_id(&pinned))
            .await?
            .ok_or_else(|| anyhow::anyhow!("no invoice document"))?;
    assert_eq!(document.total, Money::new(12_000, "GBP"));
    let base = document
        .base_currency
        .ok_or_else(|| anyhow::anyhow!("not stated in euros"))?;
    assert_eq!(
        (
            base.total_excl_vat.clone(),
            base.vat_total.clone(),
            base.total.clone()
        ),
        (Money::eur(12_500), Money::eur(2_500), Money::eur(15_000))
    );
    assert!(
        base.mention().contains("1 EUR = 0,8000 GBP"),
        "{}",
        base.mention()
    );
    let note = timada_invoice::load_credit_note_document(
        &executor,
        &issuer,
        &timada_invoice::credit_note_id("refund-gbp"),
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("no credit note document"))?;
    assert_eq!(
        note.base_currency.map(|base| (base.vat_total, base.total)),
        Some((Money::eur(1_250), Money::eur(7_500)))
    );
    let silent =
        timada_invoice::load_invoice_document(&executor, &db, &issuer, &invoice_id(&waiting))
            .await?
            .ok_or_else(|| anyhow::anyhow!("no invoice document"))?;
    assert!(silent.base_currency.is_none());

    // The rate is pinned at last — once: the first word stays.
    assert!(
        orders
            .pin_exchange_rate(&waiting, pounds_per_euro())
            .await?
    );
    assert!(
        !orders
            .pin_exchange_rate(
                &waiting,
                timada_tax::PinnedRate {
                    per_base_micros: 900_000,
                    ..pounds_per_euro()
                }
            )
            .await?
    );
    assert!(matches!(
        orders.pin_exchange_rate(&euros, pounds_per_euro()).await,
        Err(timada_order::OrderError::RateOfAnotherCurrency { .. })
    ));
    journal().await?;
    journal().await?;
    let report = vat_report(&db, period).await?;
    assert_eq!(report.unconverted, 0);
    assert_eq!(report.domestic, [line("FR", 2_000, 28_750, 5_750)]);
    Ok(())
}
