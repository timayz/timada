//! Admin users: a plain credentials table, no aggregate. Staff accounts are
//! operational configuration, not domain history.

use sqlx::SqlitePool;
use timada_core::new_id;

use crate::password::{hash_password, verify_password};
use crate::session::now_millis;

/// Create the admin user, or reset their password when the email already
/// exists — the upsert doubles as the recovery path (`create-admin` CLI).
/// Returns the user's id.
#[tracing::instrument(skip(write_pool, password))]
pub async fn create_admin_user(
    write_pool: &SqlitePool,
    email: &str,
    password: &str,
) -> anyhow::Result<String> {
    let email = email.trim().to_lowercase();
    anyhow::ensure!(email.contains('@'), "admin email must contain '@'");
    anyhow::ensure!(!password.is_empty(), "admin password must not be empty");

    let password_hash = hash_password(password)?;
    let id = new_id();
    sqlx::query(
        "INSERT INTO admin_users (id, email, password_hash, created_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT (email) DO UPDATE SET password_hash = excluded.password_hash",
    )
    .bind(&id)
    .bind(&email)
    .bind(&password_hash)
    .bind(now_millis())
    .execute(write_pool)
    .await?;

    // On conflict the insert's id lost; read back whichever row owns the email.
    let (id,): (String,) = sqlx::query_as("SELECT id FROM admin_users WHERE email = ?")
        .bind(&email)
        .fetch_one(write_pool)
        .await?;
    tracing::info!(%email, "admin user created or password reset");
    Ok(id)
}

/// Check admin credentials; `Some(user_id)` on success. Unknown email and
/// wrong password are indistinguishable to the caller on purpose.
pub async fn verify_admin(
    read_pool: &SqlitePool,
    email: &str,
    password: &str,
) -> anyhow::Result<Option<String>> {
    let email = email.trim().to_lowercase();
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT id, password_hash FROM admin_users WHERE email = ?")
            .bind(&email)
            .fetch_optional(read_pool)
            .await?;

    let Some((id, password_hash)) = row else {
        return Ok(None);
    };
    if verify_password(password, &password_hash)? {
        Ok(Some(id))
    } else {
        Ok(None)
    }
}
