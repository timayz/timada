//! Admin surface for the payment context, mounted by the umbrella crate at
//! `/admin/payments`.

use askama::Template;
use axum::Router;
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::get;
use sqlx::SqlitePool;
use timada_core::{AppResult, Currency, Money};
use timada_web::HtmlTemplate;

use crate::state::PaymentState;

pub fn admin_router(state: PaymentState) -> Router {
    Router::new().route("/", get(index)).with_state(state)
}

/// One row of `admin_payment_list`.
#[derive(sqlx::FromRow)]
struct PaymentRow {
    id: String,
    order_id: String,
    provider: String,
    amount_cents: i64,
    currency: String,
    provider_charge_ref: Option<String>,
    status: String,
    reason: Option<String>,
}

impl PaymentRow {
    /// Falls back to raw minor units if the stored currency code is one this
    /// build doesn't know — an unrecognised code shouldn't blank the page.
    fn amount(&self) -> String {
        match Currency::from_code(&self.currency) {
            Ok(currency) => Money::new(self.amount_cents, currency).to_string(),
            Err(_) => format!("{} {}", self.amount_cents, self.currency),
        }
    }

    fn status_class(&self) -> &'static str {
        match self.status.as_str() {
            "captured" => "bg-emerald-100 text-emerald-800",
            "failed" => "bg-red-100 text-red-800",
            "refunded" => "bg-amber-100 text-amber-800",
            _ => "bg-stone-200 text-stone-700",
        }
    }
}

#[derive(Template)]
#[template(path = "admin/payments/index.html")]
struct IndexTemplate {
    payments: Vec<PaymentRow>,
}

async fn index(State(state): State<PaymentState>) -> AppResult<impl IntoResponse> {
    let payments = recent_payments(&state.ctx.read_pool).await?;

    Ok(HtmlTemplate(IndexTemplate { payments }))
}

/// Reads the eventually-consistent admin projection — a charge that was just
/// committed may take a beat to appear here.
async fn recent_payments(read_pool: &SqlitePool) -> Result<Vec<PaymentRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, order_id, provider, amount_cents, currency, provider_charge_ref, status, reason
           FROM admin_payment_list
          ORDER BY created_at DESC, id DESC
          LIMIT 50",
    )
    .fetch_all(read_pool)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use timada_core::{Currency, Money};

    #[tokio::test]
    async fn lists_the_newest_payments_first() {
        let db = crate::test_support::temp_db().await;
        let provider: Arc<dyn crate::PaymentProvider> = Arc::new(crate::FakePaymentProvider);

        crate::request_charge(
            &db.ctx.executor,
            &provider,
            "order-1",
            Money::new(2500, Currency::Eur),
        )
        .await
        .unwrap();

        // Drain the read-model subscription rather than racing a spawned one.
        let mut subscription =
            crate::projection::admin_subscription(db.ctx.write_pool.clone()).no_retry();
        subscription.run_once(&db.ctx.executor).await.unwrap();

        let payments = recent_payments(&db.ctx.read_pool).await.unwrap();
        let payment = payments.first().unwrap();
        assert_eq!(payment.order_id, "order-1");
        assert_eq!(payment.provider, "fake");
        assert_eq!(payment.amount(), "25.00 EUR");
        assert_eq!(payment.status_class(), "bg-emerald-100 text-emerald-800");

        let html = IndexTemplate { payments }.render().unwrap();
        assert!(html.contains("25.00 EUR"));
        assert!(html.contains("order-1"));
    }
}
