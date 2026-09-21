//! The team: who operates the shop, as what, and whether they still do.
//! Rows of `admin_user`, managed by the owners from the « Équipe » page.
//!
//! Two rules keep a shop from locking itself out. An operator never demotes
//! or deactivates *themselves* — somebody else does. And, whoever asks, the
//! **last active owner** stays one: that one is checked inside the write's
//! own transaction, so two owners demoting each other at the same moment
//! cannot both succeed.

use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::SqlitePool;
use topcoat::session::TokenHash;

use super::{password, role::Role};

/// For the passwords chosen here — a temporary one, or an operator's own. A
/// host's bootstrap (`create_admin`) is the host's business.
pub const MIN_PASSWORD_LEN: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operator {
    pub id: String,
    pub email: String,
    pub role: Role,
    pub active: bool,
    /// Given a temporary password they have not replaced yet.
    pub must_change_password: bool,
    pub created_at: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum TeamError {
    #[error("Un opérateur existe déjà avec cette adresse e-mail.")]
    EmailTaken,
    #[error("Adresse e-mail invalide.")]
    InvalidEmail,
    #[error("Le mot de passe doit contenir au moins {MIN_PASSWORD_LEN} caractères.")]
    WeakPassword,
    #[error("Cet opérateur n'existe pas.")]
    NotFound,
    #[error("La boutique doit garder au moins un propriétaire actif.")]
    LastOwner,
    #[error("Un autre propriétaire doit le faire pour vous.")]
    Yourself,
    #[error("Mot de passe actuel incorrect.")]
    WrongPassword,
    #[error("Choisissez un mot de passe différent de l'actuel.")]
    SamePassword,
    #[error(transparent)]
    Server(#[from] anyhow::Error),
}

impl From<sqlx::Error> for TeamError {
    fn from(err: sqlx::Error) -> Self {
        Self::Server(err.into())
    }
}

fn now_secs() -> Result<i64, TeamError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| TeamError::Server(err.into()))?;
    Ok(now.as_secs() as i64)
}

fn hashed(password_text: &str) -> Result<String, TeamError> {
    if password_text.chars().count() < MIN_PASSWORD_LEN {
        return Err(TeamError::WeakPassword);
    }
    password::hash(password_text)
        .ok_or_else(|| TeamError::Server(anyhow::anyhow!("password hashing failed")))
}

/// Everybody, the owners first, then by e-mail; who left is listed last.
pub async fn list_operators(db: &SqlitePool) -> Result<Vec<Operator>, TeamError> {
    let rows: Vec<(String, String, String, bool, bool, i64)> = sqlx::query_as(
        "SELECT id, email, role, active, must_change_password, created_at FROM admin_user
         ORDER BY active DESC, role != 'owner', email",
    )
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(
            |(id, email, role, active, must_change_password, created_at)| {
                Some(Operator {
                    role: Role::parse(&role)?,
                    id,
                    email,
                    active,
                    must_change_password,
                    created_at,
                })
            },
        )
        .collect())
}

/// Adds an operator with a temporary password, to be replaced at their first
/// sign-in. Returns the new id.
pub async fn add_operator(
    db: &SqlitePool,
    email: &str,
    temporary_password: &str,
    role: Role,
) -> Result<String, TeamError> {
    let email = email.trim().to_lowercase();
    let well_formed = email
        .split_once('@')
        .is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.'));
    if !well_formed {
        return Err(TeamError::InvalidEmail);
    }
    let hash = hashed(temporary_password)?;
    let id = evento::hash_ids(vec![email.as_str(), "admin"]);
    let added = sqlx::query(
        "INSERT INTO admin_user (id, email, password_hash, created_at, role, must_change_password)
         VALUES (?, ?, ?, ?, ?, 1)",
    )
    .bind(&id)
    .bind(&email)
    .bind(hash)
    .bind(now_secs()?)
    .bind(role.as_str())
    .execute(db)
    .await;
    match added {
        Ok(_) => {
            tracing::info!(admin_id = %id, %email, role = role.as_str(), "operator added");
            Ok(id)
        }
        Err(sqlx::Error::Database(err)) if err.is_unique_violation() => Err(TeamError::EmailTaken),
        Err(err) => Err(err.into()),
    }
}

/// What is about to change for an operator.
enum Change {
    Role(Role),
    Active(bool),
}

/// Applies `change` unless it would leave the shop without an active owner.
/// One immediate transaction: the count and the write see the same team.
async fn apply(db: &SqlitePool, id: &str, change: Change) -> Result<(), TeamError> {
    let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
    let current: Option<(String, bool)> =
        sqlx::query_as("SELECT role, active FROM admin_user WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((role, active)) = current else {
        return Err(TeamError::NotFound);
    };
    let is_active_owner = active && role == Role::Owner.as_str();
    let stays_active_owner = match change {
        Change::Role(role) => active && role == Role::Owner,
        Change::Active(active) => active && role == Role::Owner.as_str(),
    };
    if is_active_owner && !stays_active_owner {
        let others: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM admin_user WHERE role = 'owner' AND active = 1 AND id != ?",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        if others == 0 {
            return Err(TeamError::LastOwner);
        }
    }
    match change {
        Change::Role(role) => {
            sqlx::query("UPDATE admin_user SET role = ? WHERE id = ?")
                .bind(role.as_str())
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        Change::Active(active) => {
            sqlx::query("UPDATE admin_user SET active = ? WHERE id = ?")
                .bind(active)
                .bind(id)
                .execute(&mut *tx)
                .await?;
            // Whoever leaves is out at once, wherever they are signed in.
            if !active {
                sqlx::query("DELETE FROM admin_session WHERE admin_id = ?")
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
            }
        }
    }
    tx.commit().await?;
    Ok(())
}

/// Gives `id` another role — at their next request, signed in or not.
pub async fn change_role(
    db: &SqlitePool,
    acting: &str,
    id: &str,
    role: Role,
) -> Result<(), TeamError> {
    if acting == id {
        return Err(TeamError::Yourself);
    }
    apply(db, id, Change::Role(role)).await?;
    tracing::info!(admin_id = %id, by = %acting, role = role.as_str(), "operator's role changed");
    Ok(())
}

/// The operator no longer signs in, and is signed out everywhere. Nothing is
/// deleted: what they did stays theirs.
pub async fn deactivate(db: &SqlitePool, acting: &str, id: &str) -> Result<(), TeamError> {
    if acting == id {
        return Err(TeamError::Yourself);
    }
    apply(db, id, Change::Active(false)).await?;
    tracing::info!(admin_id = %id, by = %acting, "operator deactivated");
    Ok(())
}

pub async fn reactivate(db: &SqlitePool, acting: &str, id: &str) -> Result<(), TeamError> {
    apply(db, id, Change::Active(true)).await?;
    tracing::info!(admin_id = %id, by = %acting, "operator reactivated");
    Ok(())
}

/// A new temporary password for somebody who lost theirs: signed out
/// everywhere, and asked for their own at the next sign-in.
pub async fn reset_password(
    db: &SqlitePool,
    acting: &str,
    id: &str,
    temporary_password: &str,
) -> Result<(), TeamError> {
    if acting == id {
        return Err(TeamError::Yourself);
    }
    let hash = hashed(temporary_password)?;
    let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
    let changed = sqlx::query(
        "UPDATE admin_user SET password_hash = ?, must_change_password = 1 WHERE id = ?",
    )
    .bind(hash)
    .bind(id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(TeamError::NotFound);
    }
    sqlx::query("DELETE FROM admin_session WHERE admin_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    tracing::info!(admin_id = %id, by = %acting, "operator's password reset");
    Ok(())
}

/// The operator chooses their own password: the temporary one is done with,
/// and every other session of theirs is closed — `keep` is the one asking.
pub async fn change_own_password(
    db: &SqlitePool,
    id: &str,
    current: &str,
    new: &str,
    keep: Option<&TokenHash>,
) -> Result<(), TeamError> {
    let stored: Option<String> =
        sqlx::query_scalar("SELECT password_hash FROM admin_user WHERE id = ? AND active = 1")
            .bind(id)
            .fetch_optional(db)
            .await?;
    let Some(stored) = stored else {
        return Err(TeamError::NotFound);
    };
    if !password::verify(current, &stored) {
        return Err(TeamError::WrongPassword);
    }
    if current == new {
        return Err(TeamError::SamePassword);
    }
    let hash = hashed(new)?;
    let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
    sqlx::query("UPDATE admin_user SET password_hash = ?, must_change_password = 0 WHERE id = ?")
        .bind(hash)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    match keep {
        Some(keep) => {
            sqlx::query("DELETE FROM admin_session WHERE admin_id = ? AND token_hash != ?")
                .bind(id)
                .bind(keep.as_slice())
                .execute(&mut *tx)
                .await?;
        }
        None => {
            sqlx::query("DELETE FROM admin_session WHERE admin_id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    tracing::info!(admin_id = %id, "operator changed their password");
    Ok(())
}
