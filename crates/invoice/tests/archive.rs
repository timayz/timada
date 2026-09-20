//! The archive: the PDF of an invoice — and of each credit note — filed once
//! when it is issued, served unchanged afterwards, and checked against the
//! hash it had then.
#![cfg(feature = "pdf")]

use timada_core::{Address, Money};
use timada_invoice::{
    ArchiveCheck, ArchiveError, ArchivePolicy, ArchiveStore, Command, DirectoryArchiveStore,
    InvoiceArchive, InvoiceIssuer, IssueCreditNote, SqliteArchiveStore, archive_credit_note,
    archive_invoice, archived_document, credit_note_archive_subscription, credit_note_id,
    invoice_archive_subscription, invoice_from_orders_subscription, invoice_id,
    load_credit_note_document, migrations, read_archived, sha256_hex, verify_archived,
};
use timada_order::{DeliveryChoice, OrderLine, PaymentMode, PlaceOrder, Seller};

fn address() -> Address {
    Address {
        first_name: "Ada".into(),
        last_name: "Lovelace".into(),
        line1: "12 rue des Machines".into(),
        postal_code: "31000".into(),
        city: "Toulouse".into(),
        country_code: "FR".into(),
        ..Address::default()
    }
}

fn order(cart: &str) -> PlaceOrder {
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
        delivery_address: address(),
        billing_address: address(),
        delivery: DeliveryChoice {
            method_code: "colissimo".into(),
            pickup_store_id: None,
        },
        payment_mode: PaymentMode::Card,
        shipping_fee: Money::eur(590),
        handling_fee: Money::eur(0),
        promo_code: None,
        discount: None,
        order_number: Some("C2026-000001".into()),
        tax: None,
        business: None,
    }
}

fn issuer(name: &str) -> InvoiceIssuer {
    InvoiceIssuer {
        name: name.into(),
        address_lines: vec!["1 rue de l'Entrepôt".into(), "31000 Toulouse".into()],
        registration: "SIRET 000 000 000 00000".into(),
        vat_number: "FR00 000000000".into(),
        contact: "facturation@timada.example".into(),
    }
}

/// An issued invoice, its id.
async fn issued(
    executor: &evento::Sqlite,
    db: &sqlx::SqlitePool,
    cart: &str,
) -> anyhow::Result<String> {
    let order_id = timada_order::Command(executor)
        .place_order(order(cart))
        .await?;
    invoice_from_orders_subscription()
        .data(db.clone())
        .run_once(executor)
        .await?;
    let id = invoice_id(&order_id);
    Command {
        executor,
        db: db.clone(),
    }
    .issue_invoice(&id)
    .await?;
    Ok(id)
}

#[tokio::test]
async fn an_issued_invoice_is_filed_once_and_stays_what_it_was() -> anyhow::Result<()> {
    let mut all = migrations();
    all.extend(timada_order::migrations());
    let (executor, db) = timada_core::testing::memory_executor(all).await?;
    let store = SqliteArchiveStore::new(db.clone());
    let id = issued(&executor, &db, "cart-1").await?;
    // A draft is nobody's document yet.
    timada_order::Command(&executor)
        .place_order(order("cart-draft"))
        .await?;
    invoice_from_orders_subscription()
        .data(db.clone())
        .run_once(&executor)
        .await?;

    let archive = |issuer: InvoiceIssuer| {
        let (executor, db, store) = (&executor, db.clone(), store.clone());
        async move {
            invoice_archive_subscription()
                .data(db)
                .data(InvoiceArchive::new(store))
                .data(issuer)
                .run_once(executor)
                .await
        }
    };
    archive(issuer("Timada SAS")).await?;

    let (entry, bytes) = read_archived(&db, &store, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("not archived"))?;
    assert!(bytes.starts_with(b"%PDF-"));
    assert_eq!(entry.kind, "invoice");
    assert!(entry.number.starts_with('F'), "{}", entry.number);
    assert!(
        entry.storage_key.starts_with("invoice/20") && entry.storage_key.ends_with(".pdf"),
        "{}",
        entry.storage_key
    );
    assert_eq!(entry.sha256, sha256_hex(&bytes));
    assert_eq!(entry.size as usize, bytes.len());
    assert!(!entry.reconstituted);
    assert_eq!(
        verify_archived(&db, &store, &id).await?,
        Some(ArchiveCheck::Intact)
    );
    let drafts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM invoice_archive")
        .fetch_one(&db)
        .await?;
    assert_eq!(drafts, 1, "the draft is not archived");

    // The shop moves, a credit note is issued: the file does not change —
    // whoever asks for it gets the bytes of the day.
    Command {
        executor: &executor,
        db: db.clone(),
    }
    .issue_credit_note(IssueCreditNote {
        refund_id: "refund-1".into(),
        invoice_id: id.clone(),
        amount: Money::eur(1_000),
        reason: "geste".into(),
    })
    .await?;
    archive(issuer("Timada SAS — nouvelle adresse")).await?;
    let again = archive_invoice(
        &executor,
        &db,
        &store,
        &issuer("Quelqu'un d'autre"),
        &id,
        &ArchivePolicy::default(),
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("not archived"))?;
    assert_eq!(again, (entry.clone(), bytes.clone()));

    // An archive never replaces a file.
    assert!(matches!(
        store.put(&entry.storage_key, b"something else").await,
        Err(ArchiveError::Conflict(_))
    ));
    store.put(&entry.storage_key, &bytes).await?;

    // Tampered with, then gone: the index remembers what it was.
    sqlx::query("UPDATE invoice_archive_blob SET content = x'00' WHERE key = ?")
        .bind(&entry.storage_key)
        .execute(&db)
        .await?;
    assert_eq!(
        verify_archived(&db, &store, &id).await?,
        Some(ArchiveCheck::Altered)
    );
    sqlx::query("DELETE FROM invoice_archive_blob")
        .execute(&db)
        .await?;
    assert_eq!(
        verify_archived(&db, &store, &id).await?,
        Some(ArchiveCheck::Missing)
    );
    assert_eq!(verify_archived(&db, &store, "unknown").await?, None);
    assert!(archived_document(&db, "unknown").await?.is_none());
    Ok(())
}

#[tokio::test]
async fn a_credit_note_is_a_document_of_its_own_filed_like_its_invoice() -> anyhow::Result<()> {
    let mut all = migrations();
    all.extend(timada_order::migrations());
    let (executor, db) = timada_core::testing::memory_executor(all).await?;
    let store = SqliteArchiveStore::new(db.clone());

    // 100,00 of goods and 5,90 of delivery, all at 20 %.
    let mut taxed = order("cart-taxed");
    taxed.tax = Some(timada_order::OrderTax {
        zone_code: "fr".into(),
        treatment: timada_tax::TaxTreatment::Domestic,
        line_rates: vec![("sku-1".into(), 2_000)],
        shipping_rate_bp: 2_000,
    });
    let order_id = timada_order::Command(&executor).place_order(taxed).await?;
    invoice_from_orders_subscription()
        .data(db.clone())
        .run_once(&executor)
        .await?;
    let invoice = invoice_id(&order_id);
    let command = Command {
        executor: &executor,
        db: db.clone(),
    };
    command.issue_invoice(&invoice).await?;
    let number = command
        .issue_credit_note(IssueCreditNote {
            refund_id: "refund-1".into(),
            invoice_id: invoice.clone(),
            amount: Money::eur(6_000),
            reason: "return R2026-000003".into(),
        })
        .await?;
    let note = credit_note_id("refund-1");

    let document = load_credit_note_document(&executor, &issuer("Timada SAS"), &note)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no document"))?;
    assert_eq!(document.number, number);
    assert!(
        document.invoice_number.starts_with('F'),
        "{}",
        document.invoice_number
    );
    assert!(document.invoice_issued_at.is_some());
    assert_eq!(document.order_label, "C2026-000001");
    assert_eq!(document.reason, "Retour R2026-000003");
    assert_eq!(document.buyer, address());
    assert_eq!(document.amount, Money::eur(6_000));
    assert!(document.amounts_include_vat);
    assert_eq!(document.total_excl_vat(), Some(Money::eur(5_000)));
    assert_eq!(document.vat_total(), Some(Money::eur(1_000)));
    assert!(
        load_credit_note_document(&executor, &issuer("x"), "unknown")
            .await?
            .is_none()
    );

    credit_note_archive_subscription()
        .data(db.clone())
        .data(InvoiceArchive::new(store.clone()))
        .data(issuer("Timada SAS"))
        .run_once(&executor)
        .await?;
    let (entry, bytes) = read_archived(&db, &store, &note)
        .await?
        .ok_or_else(|| anyhow::anyhow!("not archived"))?;
    assert!(bytes.starts_with(b"%PDF-"));
    assert_eq!(entry.kind, "credit_note");
    assert_eq!(entry.number, document.number);
    assert_eq!(
        entry.storage_key,
        format!(
            "credit-note/{}/{}.pdf",
            timada_core::time::year_of(document.issued_at),
            document.number
        )
    );
    assert_eq!(entry.sha256, sha256_hex(&bytes));
    assert!(!entry.reconstituted);
    assert_eq!(
        verify_archived(&db, &store, &note).await?,
        Some(ArchiveCheck::Intact)
    );
    // The invoice's own archive is another subscription's business.
    assert!(archived_document(&db, &invoice).await?.is_none());

    // Asked for again by an issuer who moved since: the file of the day.
    let again = archive_credit_note(
        &executor,
        &db,
        &store,
        &issuer("Quelqu'un d'autre"),
        &note,
        &ArchivePolicy::default(),
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("not archived"))?;
    assert_eq!(again, (entry, bytes));
    assert!(
        archive_credit_note(
            &executor,
            &db,
            &store,
            &issuer("x"),
            "unknown",
            &ArchivePolicy::default()
        )
        .await?
        .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn an_invoice_from_before_the_archive_is_filed_as_reconstituted() -> anyhow::Result<()> {
    let mut all = migrations();
    all.extend(timada_order::migrations());
    let (executor, db) = timada_core::testing::memory_executor(all).await?;
    let store = SqliteArchiveStore::new(db.clone());
    let id = issued(&executor, &db, "cart-old").await?;
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;

    // Issued "long" before it is archived.
    let immediate = ArchivePolicy {
        reconstituted_after: std::time::Duration::ZERO,
    };
    let (entry, _) = archive_invoice(
        &executor,
        &db,
        &store,
        &issuer("Timada SAS"),
        &id,
        &immediate,
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("not archived"))?;
    assert!(entry.reconstituted);
    // Nothing to archive for an invoice that does not exist.
    assert!(
        archive_invoice(&executor, &db, &store, &issuer("x"), "unknown", &immediate)
            .await?
            .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn a_directory_keeps_files_write_once() -> anyhow::Result<()> {
    let root = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("archive-{}", timada_core::time::now_unix_secs()?));
    let store = DirectoryArchiveStore::new(&root);

    assert_eq!(store.get("invoice/2026/F2026-000001.pdf").await?, None);
    store
        .put("invoice/2026/F2026-000001.pdf", b"%PDF-1")
        .await?;
    store
        .put("invoice/2026/F2026-000001.pdf", b"%PDF-1")
        .await?;
    assert_eq!(
        store.get("invoice/2026/F2026-000001.pdf").await?.as_deref(),
        Some(b"%PDF-1".as_slice())
    );
    assert!(root.join("invoice/2026/F2026-000001.pdf").is_file());
    assert!(matches!(
        store.put("invoice/2026/F2026-000001.pdf", b"%PDF-2").await,
        Err(ArchiveError::Conflict(_))
    ));
    // Nothing left aside.
    let left: Vec<_> = std::fs::read_dir(root.join("invoice/2026"))?.collect();
    assert_eq!(left.len(), 1);

    // A key never leaves the directory.
    for bad in [
        "",
        "../outside.pdf",
        "/etc/passwd",
        "invoice//x.pdf",
        "invoice/./x.pdf",
        "a b.pdf",
        "invoice\\x.pdf",
    ] {
        assert!(
            matches!(store.put(bad, b"x").await, Err(ArchiveError::InvalidKey(_))),
            "{bad}"
        );
        assert!(
            matches!(store.get(bad).await, Err(ArchiveError::InvalidKey(_))),
            "{bad}"
        );
    }
    std::fs::remove_dir_all(&root)?;
    Ok(())
}
