use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::SqlitePool;
use topcoat::session::TokenHash;

use crate::error::AdminError;

use super::{password, role::Role};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminUser {
    pub id: String,
    pub email: String,
    pub role: Role,
}

/// A stored role nobody knows is nobody's: the operator cannot sign in.
fn operator(id: String, email: String, role: &str) -> Option<AdminUser> {
    match Role::parse(role) {
        Some(role) => Some(AdminUser { id, email, role }),
        None => {
            tracing::error!(admin_id = %id, %role, "unknown role: the operator is refused");
            None
        }
    }
}

fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

fn now_secs() -> anyhow::Result<i64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64)
}

/// Creates the shop's owner — an operator who may do everything, the team
/// included; the hosting app calls this from its own CLI or seed step.
/// Returns the new admin id.
pub async fn create_admin(
    db: &SqlitePool,
    email: &str,
    password: &str,
) -> Result<String, AdminError> {
    create_operator(db, email, password, Role::Owner).await
}

/// Creates an operator with a role. Returns the new admin id.
pub async fn create_operator(
    db: &SqlitePool,
    email: &str,
    password: &str,
    role: Role,
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
        "INSERT INTO admin_user (id, email, password_hash, created_at, role)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&email)
    .bind(hash)
    .bind(created_at)
    .bind(role.as_str())
    .execute(db)
    .await;
    match result {
        Ok(_) => {
            tracing::info!(admin_id = %id, %email, role = role.as_str(), "admin created");
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
    let row: Option<(String, String, String, String)> =
        sqlx::query_as("SELECT id, email, role, password_hash FROM admin_user WHERE email = ?")
            .bind(normalize_email(email))
            .fetch_optional(db)
            .await?;
    Ok(row.and_then(|(id, email, role, hash)| Some((operator(id, email, &role)?, hash))))
}

pub async fn find_by_session(
    db: &SqlitePool,
    token_hash: &TokenHash,
) -> anyhow::Result<Option<AdminUser>> {
    let row: Option<(String, String, String)> = sqlx::query_as(
        "SELECT u.id, u.email, u.role FROM admin_session s
         JOIN admin_user u ON u.id = s.admin_id
         WHERE s.token_hash = ? AND s.expires_at > ?",
    )
    .bind(token_hash.as_slice())
    .bind(now_secs()?)
    .fetch_optional(db)
    .await?;
    Ok(row.and_then(|(id, email, role)| operator(id, email, &role)))
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
