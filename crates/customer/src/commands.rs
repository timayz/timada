//! Registration and login. Both cross the SQL credentials table and the event
//! store; see the crate docs for why the row comes first.

use sqlx::SqlitePool;
use timada_auth::{AuthState, SessionKind, create_session, hash_password, verify_password};
use timada_core::{Executor, new_id};

use crate::aggregate::CustomerRegistered;

/// How long a customer stays signed in: 30 days, same as an admin.
const CUSTOMER_SESSION_TTL_MILLIS: i64 = 30 * 24 * 60 * 60 * 1000;

/// Why a registration was refused.
#[derive(Debug, thiserror::Error)]
pub enum RegisterError {
    #[error("{0}")]
    Invalid(String),
    #[error("an account with this email already exists")]
    EmailTaken,
    #[error(transparent)]
    Storage(anyhow::Error),
}

/// Why a login was refused. Unknown email and wrong password collapse into
/// one variant on purpose — the form must not leak which emails exist.
#[derive(Debug, thiserror::Error)]
pub enum LoginError {
    #[error("unknown email or wrong password")]
    BadCredentials,
    #[error(transparent)]
    Storage(anyhow::Error),
}

/// Register a new customer and return their id.
///
/// The credentials INSERT goes first: its UNIQUE email is the atomic gate
/// against double registration. The event follows; if that append fails the
/// row is deleted best-effort so the email is not left claimed by a ghost.
#[tracing::instrument(skip(executor, write_pool, password))]
pub async fn register_customer(
    executor: &Executor,
    write_pool: &SqlitePool,
    email: &str,
    full_name: &str,
    password: &str,
) -> Result<String, RegisterError> {
    let email = email.trim().to_lowercase();
    let full_name = full_name.trim();
    if !email.contains('@') {
        return Err(RegisterError::Invalid("enter a valid email address".into()));
    }
    if full_name.is_empty() {
        return Err(RegisterError::Invalid("enter your name".into()));
    }
    if password.chars().count() < 8 {
        return Err(RegisterError::Invalid(
            "the password needs at least 8 characters".into(),
        ));
    }

    let password_hash = hash_password(password).map_err(RegisterError::Storage)?;
    let customer_id = new_id();
    let inserted = sqlx::query(
        "INSERT INTO customer_credentials (customer_id, email, password_hash, created_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT (email) DO NOTHING",
    )
    .bind(&customer_id)
    .bind(&email)
    .bind(&password_hash)
    .bind(timada_auth::now_millis())
    .execute(write_pool)
    .await
    .map_err(|source| RegisterError::Storage(source.into()))?;

    if inserted.rows_affected() == 0 {
        return Err(RegisterError::EmailTaken);
    }

    let event = CustomerRegistered {
        email: email.clone(),
        full_name: full_name.to_owned(),
    };
    if let Err(source) = evento::append(&customer_id)
        .original_version(0)
        .event(&event)
        .commit(executor)
        .await
    {
        // Give the email back; if even this fails the row is orphaned and a
        // re-registration of the email needs manual repair.
        if let Err(cleanup) = sqlx::query("DELETE FROM customer_credentials WHERE customer_id = ?")
            .bind(&customer_id)
            .execute(write_pool)
            .await
        {
            tracing::error!(
                error = ?cleanup,
                %customer_id,
                "failed to clean up credentials after a failed registration event"
            );
        }
        return Err(RegisterError::Storage(source.into()));
    }

    tracing::info!(%customer_id, "customer registered");
    Ok(customer_id)
}

/// Check credentials and mint a customer session.
/// Returns `(customer_id, session_token)`.
#[tracing::instrument(skip(auth, password))]
pub async fn login_customer(
    auth: &AuthState,
    email: &str,
    password: &str,
) -> Result<(String, String), LoginError> {
    let email = email.trim().to_lowercase();
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT customer_id, password_hash FROM customer_credentials WHERE email = ?",
    )
    .bind(&email)
    .fetch_optional(&auth.read_pool)
    .await
    .map_err(|source| LoginError::Storage(source.into()))?;

    let Some((customer_id, password_hash)) = row else {
        return Err(LoginError::BadCredentials);
    };
    if !verify_password(password, &password_hash).map_err(LoginError::Storage)? {
        return Err(LoginError::BadCredentials);
    }

    let token = create_session(
        &auth.write_pool,
        &customer_id,
        SessionKind::Customer,
        CUSTOMER_SESSION_TTL_MILLIS,
    )
    .await
    .map_err(LoginError::Storage)?;

    Ok((customer_id, token))
}
