use evento::migrator::Migrate;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

use super::*;
use crate::tax::{Regime, TaxProfile};
use crate::value_object::{PAYMENT_METHOD_CARD, book_id};

async fn command() -> Command<evento::Sqlite> {
    let options = SqliteConnectOptions::new()
        .filename(":memory:")
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let migrator = evento::sql_migrator::new::<sqlx::Sqlite>().unwrap();
    migrator
        .run(&mut *conn, &evento::migrator::Plan::apply_all())
        .await
        .unwrap();
    drop(conn);
    Command(pool.into())
}

/// 2026-10-14T21:00:00Z.
const NOW: u64 = 1_792_011_600;

fn invoice(order_id: &str) -> NewInvoice {
    NewInvoice {
        order_id: order_id.to_owned(),
        order_reference: "K7Q4-MZP2".to_owned(),
        paid_at: NOW - 5,
        payment_method: PAYMENT_METHOD_CARD.to_owned(),
        seller: Seller {
            address: Address {
                name: "Acme SAS".into(),
                ..Default::default()
            },
            siret: "12002701600092".into(),
            vat_number: Some("FR85120027016".into()),
            ..Default::default()
        },
        buyer: Address {
            name: "Ada Lovelace".into(),
            ..Default::default()
        },
        buyer_email: "ada@example.org".into(),
        delivery: Address {
            name: "Ada Lovelace".into(),
            postal_code: "75002".into(),
            city: "Paris".into(),
            country: "FR".into(),
            ..Default::default()
        },
        currency: "EUR".into(),
        lines: vec![
            NewInvoiceLine {
                name: "Mug".into(),
                quantity: 2,
                unit_amount_cents: 1250,
                tax_category: "standard".into(),
                ..Default::default()
            },
            NewInvoiceLine {
                name: "Book".into(),
                quantity: 1,
                unit_amount_cents: 1999,
                tax_category: "books".into(),
                ..Default::default()
            },
        ],
        total_cents: 4499,
        issued_by: "test".into(),
        tax: TaxProfile {
            vat_registered: true,
            oss_registered: false,
        },
        buyer_vat_number: None,
        vies_reference: None,
    }
}

/// An order of `amount_cents` (VAT included, standard goods) shipped to
/// Germany.
fn to_germany(order_id: &str, amount_cents: u64) -> NewInvoice {
    let base = invoice(order_id);
    NewInvoice {
        delivery: Address {
            country: "DE".into(),
            postal_code: "10115".into(),
            ..base.delivery.clone()
        },
        lines: vec![NewInvoiceLine {
            name: "Lamp".into(),
            quantity: 1,
            unit_amount_cents: amount_cents,
            tax_category: "standard".into(),
            ..Default::default()
        }],
        total_cents: amount_cents,
        ..base
    }
}

#[tokio::test]
async fn invoices_are_numbered_in_turn_and_once_per_order() {
    let cmd = command().await;
    let first = cmd.issue_invoice(invoice("o1"), NOW).await.unwrap();
    assert_eq!(first.label, "F2026-000001");
    assert_eq!(first.total_cents, 4499);
    assert_eq!(first.vat.len(), 2);
    assert_eq!(first.total_excl_vat_cents + first.total_vat_cents, 4499);

    // The same order again: the same invoice, no number taken.
    let again = cmd.issue_invoice(invoice("o1"), NOW + 60).await.unwrap();
    assert_eq!(again, first);
    let second = cmd.issue_invoice(invoice("o2"), NOW).await.unwrap();
    assert_eq!(second.label, "F2026-000002");
    assert_eq!(
        cmd.load_invoice_of_order("o2").await.unwrap().unwrap(),
        second
    );
    assert!(cmd.load_invoice_of_order("o3").await.unwrap().is_none());

    let wrong = NewInvoice {
        total_cents: 1,
        ..invoice("o3")
    };
    assert!(matches!(
        cmd.issue_invoice(wrong, NOW).await.unwrap_err(),
        InvoicingError::TotalMismatch
    ));
}

#[tokio::test]
async fn books_fill_and_the_series_carries_on_in_the_next() {
    let cmd = command().await;
    let mut labels = Vec::new();
    for i in 0..8 {
        let document = cmd
            .issue_invoice(invoice(&format!("o{i}")), NOW)
            .await
            .unwrap();
        labels.push(document.label);
    }
    let expected: Vec<String> = (1..=8).map(|n| format!("F2026-{n:06}")).collect();
    assert_eq!(labels, expected);
    // Three books of three, the last with two.
    let third = book_projection()
        .load(book_id("F2026", 2))
        .execute(&cmd.0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(third.documents.len(), 2);
    assert_eq!(third.last_number, 8);
    // An old one is still found through its index.
    let first = cmd.load_invoice_of_order("o0").await.unwrap().unwrap();
    assert_eq!(first.label, "F2026-000001");
}

#[tokio::test]
async fn a_new_year_starts_a_new_series() {
    let cmd = command().await;
    cmd.issue_invoice(invoice("o1"), NOW).await.unwrap();
    let next_year = NOW + 90 * 86_400;
    let document = cmd.issue_invoice(invoice("o2"), next_year).await.unwrap();
    assert_eq!(document.label, "F2027-000001");
}

#[tokio::test]
async fn a_crash_after_the_number_never_takes_a_second_one() {
    let cmd = command().await;
    cmd.issue_invoice(invoice("o1"), NOW).await.unwrap();
    // A writer reserved o2 and wrote it into the book, then stopped before
    // recording the index.
    let document_id = invoice_id("o2");
    evento::append(&document_id)
        .original_version(0)
        .event(&Reserved {
            series: "F2026".into(),
        })
        .commit(&cmd.0)
        .await
        .unwrap();
    let book = book_projection()
        .load(book_id("F2026", 0))
        .execute(&cmd.0)
        .await
        .unwrap()
        .unwrap();
    let data = invoice("o2");
    evento::append(book_id("F2026", 0))
        .original_version(book.aggregate_version().unwrap())
        .event(&InvoiceIssued {
            document_id: document_id.clone(),
            series: "F2026".into(),
            number: 2,
            issued_at: NOW,
            order_id: data.order_id,
            order_reference: data.order_reference,
            paid_at: data.paid_at,
            payment_method: data.payment_method,
            seller: data.seller,
            buyer: data.buyer,
            buyer_email: data.buyer_email,
            delivery: data.delivery,
            currency: data.currency,
            vat: Vec::new(),
            lines: Vec::new(),
            total_excl_vat_cents: 0,
            total_vat_cents: 0,
            total_cents: 4499,
            template: TEMPLATE_VERSION,
            issued_by: data.issued_by,
            regime: "domestic".into(),
            tax_country: "FR".into(),
            buyer_vat_number: None,
            vies_reference: None,
            eu_distance_sales_cents: 0,
            eu_threshold_passed: false,
        })
        .commit(&cmd.0)
        .await
        .unwrap();
    assert!(cmd.load_invoice_of_order("o2").await.unwrap().is_none());

    // The retry finds it and records it, in the series it reserved, even a
    // year later.
    let retried = cmd
        .issue_invoice(invoice("o2"), NOW + 365 * 86_400)
        .await
        .unwrap();
    assert_eq!(retried.label, "F2026-000002");
    let third = cmd.issue_invoice(invoice("o3"), NOW).await.unwrap();
    assert_eq!(third.label, "F2026-000003");
}

#[tokio::test]
async fn writers_racing_never_share_or_skip_a_number() {
    let cmd = command().await;
    let issues = (0..6).map(|i| {
        let cmd = Command(cmd.0.clone());
        async move {
            // Two writers per order, as when two nodes answer the same event.
            let order = format!("o{}", i / 2);
            cmd.issue_invoice(invoice(&order), NOW).await.unwrap()
        }
    });
    let documents = futures_join(issues).await;
    let mut by_order: BTreeMap<String, String> = BTreeMap::new();
    for document in &documents {
        let label = by_order
            .entry(document.order_id.clone())
            .or_insert_with(|| document.label.clone());
        assert_eq!(*label, document.label, "one number per order");
    }
    let mut labels: Vec<&String> = by_order.values().collect();
    labels.sort();
    assert_eq!(labels, ["F2026-000001", "F2026-000002", "F2026-000003"]);
}

async fn futures_join<F>(futures: impl Iterator<Item = F>) -> Vec<F::Output>
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    let handles: Vec<_> = futures.map(tokio::spawn).collect();
    let mut out = Vec::new();
    for handle in handles {
        out.push(handle.await.unwrap());
    }
    out
}

#[tokio::test]
async fn a_refund_is_credited_against_the_invoice() {
    let cmd = command().await;
    let missing = NewCreditNote {
        order_id: "o1".into(),
        refund_id: "re_1".into(),
        amount_cents: 100,
        issued_by: "test".into(),
    };
    assert!(matches!(
        cmd.issue_credit_note(missing.clone(), NOW)
            .await
            .unwrap_err(),
        InvoicingError::InvoiceNotFound
    ));
    let invoice = cmd.issue_invoice(invoice("o1"), NOW).await.unwrap();
    let credit = cmd.issue_credit_note(missing.clone(), NOW).await.unwrap();
    assert_eq!(credit.label, "A2026-000001");
    assert_eq!(credit.kind, DocumentKind::CreditNote);
    assert_eq!(
        credit.invoice_label.as_deref(),
        Some(invoice.label.as_str())
    );
    assert_eq!(credit.total_cents, 100);
    assert_eq!(credit.total_excl_vat_cents + credit.total_vat_cents, 100);
    // The same refund again: the same credit note.
    assert_eq!(cmd.issue_credit_note(missing, NOW).await.unwrap(), credit);

    let whole = NewCreditNote {
        order_id: "o1".into(),
        refund_id: "re_2".into(),
        amount_cents: invoice.total_cents,
        issued_by: "test".into(),
    };
    let whole = cmd.issue_credit_note(whole, NOW).await.unwrap();
    assert_eq!(whole.label, "A2026-000002");
    assert_eq!(whole.vat, invoice.vat);
    let too_much = NewCreditNote {
        order_id: "o1".into(),
        refund_id: "re_3".into(),
        amount_cents: invoice.total_cents + 1,
        issued_by: "test".into(),
    };
    assert!(matches!(
        cmd.issue_credit_note(too_much, NOW).await.unwrap_err(),
        InvoicingError::CreditTooLarge
    ));
}

#[tokio::test]
async fn eu_distance_sales_pass_the_threshold_in_the_series_order() {
    let cmd = command().await;
    // 6,000 € then 4,800 € excluding VAT: French VAT, then the one that
    // passes 10,000 € is German already, and so is every one after.
    let first = cmd
        .issue_invoice(to_germany("o1", 720_000), NOW)
        .await
        .unwrap();
    assert_eq!(first.regime, Regime::DistanceOrigin);
    assert_eq!(first.vat[0].rate_bps, 2000);
    assert_eq!(first.eu_distance_sales_cents, 600_000);
    let domestic = cmd.issue_invoice(invoice("o2"), NOW).await.unwrap();
    assert_eq!(domestic.regime, Regime::Domestic);
    assert_eq!(
        domestic.eu_distance_sales_cents, 600_000,
        "a French sale does not count"
    );
    let second = cmd
        .issue_invoice(to_germany("o3", 600_000), NOW)
        .await
        .unwrap();
    assert_eq!(second.regime, Regime::Oss);
    assert_eq!(second.tax_country, "DE");
    assert_eq!(second.vat[0].rate_bps, 1900);
    assert!(second.eu_threshold_passed);
    let small = cmd
        .issue_invoice(to_germany("o4", 1_000), NOW)
        .await
        .unwrap();
    assert_eq!(small.regime, Regime::Oss);

    // Passed this year: German VAT all next year too, counting from zero.
    let next_year = NOW + 90 * 86_400;
    let january = cmd
        .issue_invoice(to_germany("o5", 1_000), next_year)
        .await
        .unwrap();
    assert!(january.label.starts_with("F2027-"));
    assert_eq!(january.regime, Regime::Oss);
    assert_eq!(january.eu_distance_sales_cents, 840);
}

#[tokio::test]
async fn racing_writers_count_every_distance_sale_once() {
    let cmd = command().await;
    let issues = (0..6).map(|i| {
        let cmd = Command(cmd.0.clone());
        async move {
            cmd.issue_invoice(to_germany(&format!("o{i}"), 240_000), NOW)
                .await
                .unwrap()
        }
    });
    let mut documents = futures_join(issues).await;
    documents.sort_by_key(|document| document.number);
    // 2,000 € each excluding VAT: the sixth passes 10,000 €.
    let totals: Vec<u64> = documents
        .iter()
        .map(|d| d.eu_distance_sales_cents)
        .collect();
    assert_eq!(totals[..4], [200_000, 400_000, 600_000, 800_000]);
    assert_eq!(totals[4], 1_000_000);
    let regimes: Vec<Regime> = documents.iter().map(|d| d.regime).collect();
    assert_eq!(&regimes[..5], &[Regime::DistanceOrigin; 5]);
    assert_eq!(regimes[5], Regime::Oss);
}

#[tokio::test]
async fn a_valid_business_number_reverse_charges_and_its_credit_note_too() {
    let cmd = command().await;
    let business = NewInvoice {
        buyer_vat_number: Some("DE123456789".into()),
        vies_reference: Some("WAPIAAAAW1".into()),
        ..to_germany("o1", 12_000)
    };
    let invoice = cmd.issue_invoice(business, NOW).await.unwrap();
    assert_eq!(invoice.regime, Regime::ReverseCharge);
    assert!(invoice.vat.is_empty());
    assert_eq!(invoice.total_excl_vat_cents, 12_000);
    assert_eq!(invoice.buyer_vat_number.as_deref(), Some("DE123456789"));
    assert_eq!(invoice.vies_reference.as_deref(), Some("WAPIAAAAW1"));
    assert_eq!(invoice.eu_distance_sales_cents, 0);

    let credit = cmd
        .issue_credit_note(
            NewCreditNote {
                order_id: "o1".into(),
                refund_id: "re_1".into(),
                amount_cents: 2_000,
                issued_by: "test".into(),
            },
            NOW,
        )
        .await
        .unwrap();
    assert_eq!(credit.regime, Regime::ReverseCharge);
    assert_eq!(credit.total_excl_vat_cents, 2_000);
    assert_eq!(credit.buyer_vat_number.as_deref(), Some("DE123456789"));

    // Exported: no VAT either.
    let export = NewInvoice {
        delivery: Address {
            country: "US".into(),
            postal_code: "10001".into(),
            ..Address::default()
        },
        ..invoice_for_export()
    };
    let export = cmd.issue_invoice(export, NOW).await.unwrap();
    assert_eq!(export.regime, Regime::Export);
    assert!(export.vat.is_empty());
}

fn invoice_for_export() -> NewInvoice {
    invoice("o2")
}
