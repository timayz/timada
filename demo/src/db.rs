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
    all.extend(timada_inventory::migrations());
    all.extend(timada_review::migrations());
    all.extend(timada_customer::migrations());
    all.extend(timada_order::migrations());
    all.extend(timada_payment::migrations());
    all.extend(timada_invoice::migrations());
    all.extend(timada_promotion::migrations());
    all.extend(timada_mailer::migrations());
    all.extend(timada_admin::migrations());
    all.extend(crate::auth::migrations());
    all
}

/// What the e-mails say about the shop; `TIMADA_BASE_URL` and
/// `TIMADA_MAIL_FROM` override the development defaults.
pub fn mailer_config() -> timada_mailer::MailerConfig {
    timada_mailer::MailerConfig {
        from: std::env::var("TIMADA_MAIL_FROM")
            .unwrap_or_else(|_| "Timada demo <no-reply@timada.example>".to_owned()),
        shop_name: "Timada demo".to_owned(),
        base_url: std::env::var("TIMADA_BASE_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:3000".to_owned()),
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
        timada_payment::refund_list_subscription()
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
        timada_mailer::mailer_subscription()
            .data(db)
            .data(mailer_config())
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
        timada_payment::refund_list_subscription()
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
        timada_mailer::mailer_subscription()
            .data(db.clone())
            .data(mailer_config())
            .run_once(executor)
            .await?;
    }
    Ok(())
}
