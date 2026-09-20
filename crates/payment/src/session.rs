//! The provider's session for a payment being paid — write-side SQL, provider
//! plumbing rather than a domain fact: `payment_provider_session` keeps the
//! session's reference so a second visit reuses it and a timeout can call it
//! off.

use evento::Executor;
use sqlx::SqlitePool;

use crate::{
    error::PaymentError,
    provider::{CancelOutcome, PaymentProvider, PaymentStart, ReturnUrls},
    query::load_payment,
    value_object::PaymentStatus,
};

async fn session_of(db: &SqlitePool, payment_id: &str) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar(
        "SELECT session_reference FROM payment_provider_session WHERE payment_id = ?",
    )
    .bind(payment_id)
    .fetch_optional(db)
    .await
}

/// What the storefront must show for the shopper to pay `payment_id`. Only a
/// payment still `Requested` can be started ([`PaymentError::NotRequested`]
/// otherwise); asking again reuses the provider's session.
pub async fn start_payment<E: Executor>(
    executor: &E,
    db: &SqlitePool,
    provider: &dyn PaymentProvider,
    payment_id: &str,
    urls: &ReturnUrls,
) -> Result<PaymentStart, PaymentError> {
    let payment = load_payment(executor, payment_id)
        .await?
        .ok_or(PaymentError::PaymentNotFound)?;
    if payment.status != PaymentStatus::Requested {
        return Err(PaymentError::NotRequested);
    }

    let known = session_of(db, payment_id).await?;
    let started = provider.start(&payment, known.as_deref(), urls).await?;
    if let Some(session) = &started.session_reference
        && known.as_ref() != Some(session)
    {
        sqlx::query(
            "INSERT INTO payment_provider_session (payment_id, session_reference, created_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT (payment_id) DO UPDATE SET session_reference = excluded.session_reference",
        )
        .bind(payment_id)
        .bind(session)
        .bind(timada_core::time::now_unix_secs()? as i64)
        .execute(db)
        .await?;
    }
    Ok(started.start)
}

/// Calls off the session a payment was being paid on, if it has one. To do
/// *before* declining a payment that timed out: when the provider answers
/// [`CancelOutcome::AlreadyPaid`], the payment is to be captured instead.
pub async fn cancel_payment_session(
    db: &SqlitePool,
    provider: &dyn PaymentProvider,
    payment_id: &str,
) -> Result<CancelOutcome, PaymentError> {
    let Some(session) = session_of(db, payment_id).await? else {
        return Ok(CancelOutcome::Cancelled);
    };
    let outcome = provider.cancel(&session).await?;
    if outcome == CancelOutcome::Cancelled {
        sqlx::query("DELETE FROM payment_provider_session WHERE payment_id = ?")
            .bind(payment_id)
            .execute(db)
            .await?;
    }
    Ok(outcome)
}
