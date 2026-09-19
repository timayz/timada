use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::SqlitePool;
use topcoat::session::TokenHash;

/// The signed-in shopper: the customer aggregate the account belongs to.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Account {
    pub customer_id: String,
    pub email: String,
}

/// An email claim with no customer behind it is a sign-up that died between
/// the claim and the registration; it can be retried after this many seconds.
const STALE_CLAIM_SECS: i64 = 300;

pub fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

fn now_secs() -> anyhow::Result<i64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64)
}

/// Reserves `email` before the customer is registered; the primary key is
/// the uniqueness guard the event store cannot give. `Ok(false)` when taken.
pub async fn claim_email(
    db: &SqlitePool,
    email: &str,
    password_hash: &str,
) -> anyhow::Result<bool> {
    let now = now_secs()?;
    sqlx::query(
        "DELETE FROM shop_account WHERE email = ? AND customer_id IS NULL AND created_at < ?",
    )
    .bind(email)
    .bind(now - STALE_CLAIM_SECS)
    .execute(db)
    .await?;

    let result =
        sqlx::query("INSERT INTO shop_account (email, password_hash, created_at) VALUES (?, ?, ?)")
            .bind(email)
            .bind(password_hash)
            .bind(now)
            .execute(db)
            .await;
    match result {
        Ok(_) => Ok(true),
        Err(sqlx::Error::Database(err)) if err.is_unique_violation() => Ok(false),
        Err(err) => Err(err.into()),
    }
}

/// Moves an account to `new_email`. The primary key makes it atomic: either
/// the address is free and now belongs to this account, or nothing changed
/// (`Ok(false)`). A sign-up that died half-way does not keep an address.
pub async fn move_account(
    db: &SqlitePool,
    customer_id: &str,
    new_email: &str,
) -> anyhow::Result<bool> {
    sqlx::query(
        "DELETE FROM shop_account WHERE email = ? AND customer_id IS NULL AND created_at < ?",
    )
    .bind(new_email)
    .bind(now_secs()? - STALE_CLAIM_SECS)
    .execute(db)
    .await?;

    let moved = sqlx::query("UPDATE shop_account SET email = ? WHERE customer_id = ?")
        .bind(new_email)
        .bind(customer_id)
        .execute(db)
        .await;
    match moved {
        Ok(done) => Ok(done.rows_affected() > 0),
        Err(sqlx::Error::Database(err)) if err.is_unique_violation() => Ok(false),
        Err(err) => Err(err.into()),
    }
}

/// Stores the new hash and closes the customer's sessions, except `keep`.
pub async fn replace_password(
    db: &SqlitePool,
    customer_id: &str,
    password_hash: &str,
    keep: Option<&TokenHash>,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE shop_account SET password_hash = ? WHERE customer_id = ?")
        .bind(password_hash)
        .bind(customer_id)
        .execute(db)
        .await?;
    sqlx::query(
        "DELETE FROM shop_session WHERE customer_id = ?1 AND (?2 IS NULL OR token_hash != ?2)",
    )
    .bind(customer_id)
    .bind(keep.map(|hash| hash.as_slice()))
    .execute(db)
    .await?;
    Ok(())
}

pub async fn release_claim(db: &SqlitePool, email: &str) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM shop_account WHERE email = ? AND customer_id IS NULL")
        .bind(email)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn attach_customer(db: &SqlitePool, email: &str, customer_id: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE shop_account SET customer_id = ? WHERE email = ?")
        .bind(customer_id)
        .bind(email)
        .execute(db)
        .await?;
    Ok(())
}

/// The account and its password hash; claims without a customer cannot sign in.
pub async fn find_credentials(
    db: &SqlitePool,
    email: &str,
) -> sqlx::Result<Option<(Account, String)>> {
    let row: Option<(String, String, String)> = sqlx::query_as(
        "SELECT customer_id, email, password_hash FROM shop_account
         WHERE email = ? AND customer_id IS NOT NULL",
    )
    .bind(normalize_email(email))
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(customer_id, email, hash)| (Account { customer_id, email }, hash)))
}

pub async fn find_by_session(
    db: &SqlitePool,
    token_hash: &TokenHash,
) -> anyhow::Result<Option<Account>> {
    let account = sqlx::query_as(
        "SELECT a.customer_id, a.email FROM shop_session s
         JOIN shop_account a ON a.customer_id = s.customer_id
         WHERE s.token_hash = ? AND s.expires_at > ?",
    )
    .bind(token_hash.as_slice())
    .bind(now_secs()?)
    .fetch_optional(db)
    .await?;
    Ok(account)
}

pub async fn insert_session(
    db: &SqlitePool,
    token_hash: &TokenHash,
    customer_id: &str,
    expires_at: SystemTime,
) -> anyhow::Result<()> {
    let expires_at = expires_at.duration_since(UNIX_EPOCH)?.as_secs() as i64;
    sqlx::query("INSERT INTO shop_session (token_hash, customer_id, expires_at) VALUES (?, ?, ?)")
        .bind(token_hash.as_slice())
        .bind(customer_id)
        .bind(expires_at)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn delete_session(db: &SqlitePool, token_hash: &TokenHash) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM shop_session WHERE token_hash = ?")
        .bind(token_hash.as_slice())
        .execute(db)
        .await?;
    Ok(())
}
