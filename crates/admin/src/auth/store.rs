use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::SqlitePool;
use topcoat::session::TokenHash;

use crate::error::AdminError;

use super::password;

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AdminUser {
    pub id: String,
    pub email: String,
}

fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

fn now_secs() -> anyhow::Result<i64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64)
}

/// Creates an admin account; the hosting app calls this from its own CLI or
/// seed step. Returns the new admin id.
pub async fn create_admin(
    db: &SqlitePool,
    email: &str,
    password: &str,
) -> Result<String, AdminError> {
    let email = normalize_email(email);
    if email.is_empty() || !email.contains('@') {
        return Err(AdminError::Required("email"));
    }
    if password.trim().is_empty() {
        return Err(AdminError::Required("password"));
    }
    let hash = password::hash(password).ok_or(AdminError::Password)?;
    let id = evento::hash_ids(vec![email.as_str(), "admin"]);
    let created_at = now_secs().map_err(|_| AdminError::Password)?;

    let result = sqlx::query(
        "INSERT INTO admin_user (id, email, password_hash, created_at) VALUES (?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&email)
    .bind(hash)
    .bind(created_at)
    .execute(db)
    .await;
    match result {
        Ok(_) => {
            tracing::info!(admin_id = %id, %email, "admin created");
            Ok(id)
        }
        Err(sqlx::Error::Database(err)) if err.is_unique_violation() => {
            Err(AdminError::EmailTaken(email))
        }
        Err(err) => Err(err.into()),
    }
}

pub async fn find_credentials(
    db: &SqlitePool,
    email: &str,
) -> sqlx::Result<Option<(AdminUser, String)>> {
    let row: Option<(String, String, String)> =
        sqlx::query_as("SELECT id, email, password_hash FROM admin_user WHERE email = ?")
            .bind(normalize_email(email))
            .fetch_optional(db)
            .await?;
    Ok(row.map(|(id, email, hash)| (AdminUser { id, email }, hash)))
}

pub async fn find_by_session(
    db: &SqlitePool,
    token_hash: &TokenHash,
) -> anyhow::Result<Option<AdminUser>> {
    let admin = sqlx::query_as(
        "SELECT u.id, u.email FROM admin_session s
         JOIN admin_user u ON u.id = s.admin_id
         WHERE s.token_hash = ? AND s.expires_at > ?",
    )
    .bind(token_hash.as_slice())
    .bind(now_secs()?)
    .fetch_optional(db)
    .await?;
    Ok(admin)
}

pub async fn insert_session(
    db: &SqlitePool,
    token_hash: &TokenHash,
    admin_id: &str,
    expires_at: SystemTime,
) -> anyhow::Result<()> {
    let expires_at = expires_at.duration_since(UNIX_EPOCH)?.as_secs() as i64;
    sqlx::query("INSERT INTO admin_session (token_hash, admin_id, expires_at) VALUES (?, ?, ?)")
        .bind(token_hash.as_slice())
        .bind(admin_id)
        .bind(expires_at)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn delete_session(db: &SqlitePool, token_hash: &TokenHash) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM admin_session WHERE token_hash = ?")
        .bind(token_hash.as_slice())
        .execute(db)
        .await?;
    Ok(())
}
