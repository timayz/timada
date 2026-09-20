//! SQLite setup, migrations and the subscriptions that keep read models and
//! process managers running.

use std::path::Path;
use std::str::FromStr;

use evento::subscription::Subscription;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Sqlite, SqlitePool};
use sqlx_migrator::{Info, Migrate, Migration, Plan};

use crate::Store;

/// One pool in WAL mode with a generous busy timeout (a single-process demo).
pub async fn open(path: &str) -> anyhow::Result<(evento::Sqlite, SqlitePool)> {
    if let Some(dir) = Path::new(path).parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{path}"))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(std::time::Duration::from_secs(60))
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await?;

    let mut migrator = evento::sql_migrator::new::<Sqlite>()?;
    migrator.add_migrations(migrations())?;
    let mut conn = pool.acquire().await?;
    migrator.run(&mut *conn, &Plan::apply_all()).await?;
    drop(conn);

    Ok((pool.clone().into(), pool))
}

pub fn migrations() -> Vec<Box<dyn Migration<Sqlite>>> {
    let mut all = Vec::new();
    all.extend(timada_catalog::migrations());
    all.extend(timada_cart::migrations());
    all.extend(timada_inventory::migrations());
    all.extend(timada_review::migrations());
    all.extend(timada_customer::migrations());
    all.extend(timada_order::migrations());
    all.extend(timada_payment::migrations());
    all.extend(timada_invoice::migrations());
    all.extend(timada_promotion::migrations());
    all.extend(timada_mailer::migrations());
    all.extend(timada_returns::migrations());
    all.extend(timada_admin::migrations());
    all.extend(crate::auth::migrations());
    all
}

/// Where the shop delivers and how each destination is taxed: metropolitan
/// France as listed, the overseas territories without French VAT, and the
/// rest of the EU with the VAT of the destination (one-stop shop). Everything
/// the demo sells is at the standard rate; a shop with reduced-rate products
/// maps them per country with `TaxZones::with_mapped_rate`.
pub fn tax_zones() -> timada_tax::TaxZones {
    timada_tax::TaxZones::france_with_eu_oss()
}

/// The currencies the demo shop sells in: euros first — the base currency,
/// the one the books are kept in — then pounds and Swiss francs.
pub fn shop_currencies() -> timada_core::ShopCurrencies {
    timada_core::ShopCurrencies::new("EUR", &["GBP", "CHF"]).unwrap_or_default()
}

/// How the demo shop takes articles back: 14 days, and a prepaid label at
/// 6,90 € — free when the shop is at fault.
pub fn return_policy() -> timada_returns::ReturnPolicy {
    timada_returns::ReturnPolicy {
        label_fee_minor: 690,
        ..timada_returns::ReturnPolicy::default()
    }
}

/// What delivery costs: the built-in euro fees, and the demo shop's own for
/// the other currencies it sells in. Overseas Chronopost is a euro matter.
pub fn delivery_fees() -> timada_shipping::DeliveryFees {
    use timada_core::Money;
    timada_shipping::DeliveryFees::default()
        .with_fee("colissimo", Money::new(490, "GBP"))
        .with_fee("colissimo-europe", Money::new(1_090, "GBP"))
        .with_fee("store-pickup", Money::new(0, "GBP"))
        .with_fee("colissimo", Money::new(590, "CHF"))
        .with_fee("colissimo-europe", Money::new(1_290, "CHF"))
        .with_fee("store-pickup", Money::new(0, "CHF"))
}

/// Who issues the demo shop's invoices.
pub fn invoice_issuer() -> timada_invoice::InvoiceIssuer {
    timada_invoice::InvoiceIssuer {
        name: "Timada demo SAS".to_owned(),
        address_lines: vec![
            "1 rue de l'Entrepôt".to_owned(),
            "31000 Toulouse".to_owned(),
            "France".to_owned(),
        ],
        registration: "SIRET 000 000 000 00000".to_owned(),
        vat_number: "FR00 000000000".to_owned(),
        contact: "facturation@timada.example".to_owned(),
    }
}

/// Where shoppers send their returns: on the return slip and in the e-mail.
pub const RETURNS_ADDRESS: &str =
    "Timada demo — Service retours\n1 rue de l'Entrepôt\n31000 Toulouse\nFrance";

/// What the e-mails say about the shop; `TIMADA_BASE_URL` and
/// `TIMADA_MAIL_FROM` override the development defaults.
pub fn mailer_config() -> timada_mailer::MailerConfig {
    timada_mailer::MailerConfig {
        from: std::env::var("TIMADA_MAIL_FROM")
            .unwrap_or_else(|_| "Timada demo <no-reply@timada.example>".to_owned()),
        shop_name: "Timada demo".to_owned(),
        base_url: std::env::var("TIMADA_BASE_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:3000".to_owned()),
        returns_address: RETURNS_ADDRESS.to_owned(),
        alerts_to: Some(
            std::env::var("TIMADA_ALERTS_TO")
                .unwrap_or_else(|_| "boutique@timada.example".to_owned()),
        ),
        max_event_age_secs: timada_mailer::MailerConfig::DEFAULT_MAX_EVENT_AGE_SECS,
    }
}

/// Every read-model subscription and process manager, running in the background.
pub async fn start_subscriptions(store: &Store) -> anyhow::Result<Vec<Subscription>> {
    let (executor, db) = (&store.executor, store.db.clone());
    Ok(vec![
        timada_catalog::product_list_subscription()
            .data(db.clone())
            .start(executor)
            .await?,
        timada_catalog::category_list_subscription()
            .data(db.clone())
            .start(executor)
            .await?,
        timada_catalog::listing_subscription()
            .data(db.clone())
            .start(executor)
            .await?,
        timada_cart::saved_cart_list_subscription()
            .data(db.clone())
            .start(executor)
            .await?,
        timada_inventory::back_in_stock_subscription()
            .data(db.clone())
            .start(executor)
            .await?,
        timada_inventory::stock_list_subscription()
            .data(db.clone())
            .start(executor)
            .await?,
        timada_inventory::alert_list_subscription()
            .data(db.clone())
            .start(executor)
            .await?,
        timada_review::product_summary_subscription()
            .data(db.clone())
            .start(executor)
            .await?,
        timada_review::review_list_subscription()
            .data(db.clone())
            .start(executor)
            .await?,
        timada_review::question_list_subscription()
            .data(db.clone())
            .start(executor)
            .await?,
        timada_customer::customer_list_subscription()
            .data(db.clone())
            .start(executor)
            .await?,
        timada_order::order_history_subscription()
            .data(db.clone())
            .start(executor)
            .await?,
        timada_order::order_checkout_subscription()
            .data(db.clone())
            .data(tax_zones())
            // Asked again about a business's VAT number before its order is
            // placed without VAT.
            .data(timada_tax::VatRegistry(store.vat_validator.clone()))
            .data(delivery_fees())
            .start(executor)
            .await?,
        timada_order::order_fulfillment_subscription()
            .start(executor)
            .await?,
        timada_order::payment_deadline_subscription()
            .data(db.clone())
            .start(executor)
            .await?,
        timada_invoice::invoice_from_orders_subscription()
            .data(db.clone())
            .start(executor)
            .await?,
        timada_invoice::invoice_list_subscription()
            .data(db.clone())
            .start(executor)
            .await?,
        timada_invoice::credit_notes_from_refunds_subscription()
            .data(db.clone())
            .start(executor)
            .await?,
        timada_invoice::credit_note_list_subscription()
            .data(db.clone())
            .start(executor)
            .await?,
        timada_invoice::vat_journal_subscription()
            .data(db.clone())
            .start(executor)
            .await?,
        timada_invoice::invoice_archive_subscription()
            .data(db.clone())
            .data(store.archive.clone())
            .data(invoice_issuer())
            .start(executor)
            .await?,
        timada_invoice::credit_note_archive_subscription()
            .data(db.clone())
            .data(store.archive.clone())
            .data(invoice_issuer())
            .start(executor)
            .await?,
        timada_payment::refund_list_subscription()
            .data(db.clone())
            .start(executor)
            .await?,
        timada_payment::dispute_list_subscription()
            .data(db.clone())
            .start(executor)
            .await?,
        timada_order::payment_hold_subscription()
            .data(db.clone())
            .start(executor)
            .await?,
        timada_payment::refund_execution_subscription()
            .data(db.clone())
            .start(executor)
            .await?,
        timada_order::order_promo_release_subscription()
            .data(db.clone())
            .start(executor)
            .await?,
        timada_promotion::code_list_subscription()
            .data(db.clone())
            .start(executor)
            .await?,
        timada_returns::return_processing_subscription()
            .data(db.clone())
            .start(executor)
            .await?,
        timada_returns::return_list_subscription()
            .data(db.clone())
            .start(executor)
            .await?,
        timada_mailer::mailer_subscription()
            .data(db)
            .data(mailer_config())
            // Who issues the invoices: turns on the e-mail that carries them —
            // the archived file, the one the account serves.
            .data(invoice_issuer())
            .data(store.archive.clone())
            .start(executor)
            .await?,
    ])
}

/// Drains the same subscriptions once (after seeding, before the server runs).
pub async fn run_subscriptions_once(store: &Store) -> anyhow::Result<()> {
    let (executor, db) = (&store.executor, store.db.clone());
    for _ in 0..4 {
        timada_catalog::product_list_subscription()
            .data(db.clone())
            .run_once(executor)
            .await?;
        timada_catalog::category_list_subscription()
            .data(db.clone())
            .run_once(executor)
            .await?;
        timada_catalog::listing_subscription()
            .data(db.clone())
            .run_once(executor)
            .await?;
        timada_cart::saved_cart_list_subscription()
            .data(db.clone())
            .run_once(executor)
            .await?;
        timada_inventory::back_in_stock_subscription()
            .data(db.clone())
            .run_once(executor)
            .await?;
        timada_inventory::stock_list_subscription()
            .data(db.clone())
            .run_once(executor)
            .await?;
        timada_inventory::alert_list_subscription()
            .data(db.clone())
            .run_once(executor)
            .await?;
        timada_review::product_summary_subscription()
            .data(db.clone())
            .run_once(executor)
            .await?;
        timada_review::review_list_subscription()
            .data(db.clone())
            .run_once(executor)
            .await?;
        timada_review::question_list_subscription()
            .data(db.clone())
            .run_once(executor)
            .await?;
        timada_customer::customer_list_subscription()
            .data(db.clone())
            .run_once(executor)
            .await?;
        timada_order::order_history_subscription()
            .data(db.clone())
            .run_once(executor)
            .await?;
        timada_order::order_checkout_subscription()
            .data(db.clone())
            .data(tax_zones())
            // Asked again about a business's VAT number before its order is
            // placed without VAT.
            .data(timada_tax::VatRegistry(store.vat_validator.clone()))
            .data(delivery_fees())
            .run_once(executor)
            .await?;
        timada_order::order_fulfillment_subscription()
            .run_once(executor)
            .await?;
        timada_order::payment_deadline_subscription()
            .data(db.clone())
            .run_once(executor)
            .await?;
        timada_invoice::invoice_from_orders_subscription()
            .data(db.clone())
            .run_once(executor)
            .await?;
        timada_invoice::invoice_list_subscription()
            .data(db.clone())
            .run_once(executor)
            .await?;
        timada_invoice::credit_notes_from_refunds_subscription()
            .data(db.clone())
            .run_once(executor)
            .await?;
        timada_invoice::credit_note_list_subscription()
            .data(db.clone())
            .run_once(executor)
            .await?;
        timada_invoice::vat_journal_subscription()
            .data(db.clone())
            .run_once(executor)
            .await?;
        timada_invoice::invoice_archive_subscription()
            .data(db.clone())
            .data(store.archive.clone())
            .data(invoice_issuer())
            .run_once(executor)
            .await?;
        timada_invoice::credit_note_archive_subscription()
            .data(db.clone())
            .data(store.archive.clone())
            .data(invoice_issuer())
            .run_once(executor)
            .await?;
        // Refunds asked for are handed to the provider right away here; the
        // running shop has `run_provider_refunds` for that.
        timada_payment::refund_execution_subscription()
            .data(db.clone())
            .run_once(executor)
            .await?;
        timada_payment::execute_pending_refunds(
            executor,
            &db,
            store.provider.as_ref(),
            &timada_payment::RefundPolicy::without_delays(),
        )
        .await?;
        timada_payment::refund_list_subscription()
            .data(db.clone())
            .run_once(executor)
            .await?;
        timada_payment::dispute_list_subscription()
            .data(db.clone())
            .run_once(executor)
            .await?;
        timada_order::payment_hold_subscription()
            .data(db.clone())
            .run_once(executor)
            .await?;
        timada_order::order_promo_release_subscription()
            .data(db.clone())
            .run_once(executor)
            .await?;
        timada_promotion::code_list_subscription()
            .data(db.clone())
            .run_once(executor)
            .await?;
        timada_returns::return_processing_subscription()
            .data(db.clone())
            .run_once(executor)
            .await?;
        timada_returns::return_list_subscription()
            .data(db.clone())
            .run_once(executor)
            .await?;
        timada_mailer::mailer_subscription()
            .data(db.clone())
            .data(mailer_config())
            .data(invoice_issuer())
            .data(store.archive.clone())
            .run_once(executor)
            .await?;
    }
    Ok(())
}
