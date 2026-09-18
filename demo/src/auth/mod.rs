//! Shopper accounts. Credentials and sessions live in SQL next to the read
//! models (never in events); the account points at the `Customer` aggregate
//! that holds the identity and the address book.

mod migration;
mod password;
mod store;

use timada_customer::{CustomerError, RegisterCustomer};
use topcoat::{
    context::{Cx, app_context, memoize},
    router::{error::see_other, href, request::uri},
    session,
};

use crate::Store;

pub use migration::migrations;
pub use store::Account;

/// Name of the shopper session cookie (served as `__Host-timada_shop`),
/// distinct from the admin's.
pub const SESSION_COOKIE: &str = "timada_shop";

const MIN_PASSWORD_LEN: usize = 8;

#[derive(Debug, thiserror::Error)]
pub enum SignUpError {
    #[error("Un compte existe déjà avec cette adresse email.")]
    EmailTaken,
    #[error("Le mot de passe doit contenir au moins {MIN_PASSWORD_LEN} caractères.")]
    WeakPassword,
    #[error("Adresse email invalide.")]
    InvalidEmail,
    #[error("Le prénom et le nom sont obligatoires.")]
    MissingName,
    #[error(transparent)]
    Server(#[from] anyhow::Error),
}

/// Creates the customer and the account that signs into it.
///
/// The email is claimed in SQL first (its primary key is the uniqueness
/// guard), then the customer is registered and attached; a failed
/// registration releases the claim.
pub async fn sign_up(
    store: &Store,
    customer: RegisterCustomer,
    password: &str,
) -> Result<Account, SignUpError> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(SignUpError::WeakPassword);
    }
    let email = store::normalize_email(&customer.email);
    let hash = password::hash(password)
        .ok_or_else(|| SignUpError::Server(anyhow::anyhow!("password hashing failed")))?;
    if !store::claim_email(&store.db, &email, &hash).await? {
        return Err(SignUpError::EmailTaken);
    }

    let registered = timada_customer::Command(&store.executor)
        .register_customer(RegisterCustomer {
            email: email.clone(),
            ..customer
        })
        .await;
    let customer_id = match registered {
        Ok(id) => id,
        Err(err) => {
            store::release_claim(&store.db, &email)
                .await
                .map_err(anyhow::Error::from)?;
            return Err(match err {
                CustomerError::InvalidEmail(_) => SignUpError::InvalidEmail,
                CustomerError::Required(_) => SignUpError::MissingName,
                other => SignUpError::Server(other.into()),
            });
        }
    };
    store::attach_customer(&store.db, &email, &customer_id)
        .await
        .map_err(anyhow::Error::from)?;
    tracing::info!(%customer_id, "shopper account created");
    Ok(Account { customer_id, email })
}

/// The shopper the request's session belongs to, if any. Memoized per request.
#[memoize(as_ref)]
pub async fn current_account(cx: &Cx) -> topcoat::Result<Option<Account>> {
    let Some(token_hash) = session::token_hash(cx).await? else {
        return Ok(None);
    };
    let store = app_context::<Store>(cx);
    Ok(store::find_by_session(&store.db, &token_hash).await?)
}

/// The signed-in shopper, or a redirect to the login page that comes back here.
pub async fn require_account(cx: &Cx) -> topcoat::Result<Account> {
    match current_account(cx).await {
        Ok(Some(account)) => Ok(account.clone()),
        Ok(None) => {
            let login = href!(crate::app::account::login)
                .query([("next", uri(cx).path())])
                .resolve(cx);
            Err(see_other(login).into())
        }
        Err(err) => Err(anyhow::anyhow!("{err:#}").into()),
    }
}

/// Opens a session for an account (after sign-up or a verified password).
pub async fn start_session(cx: &Cx, account: &Account) -> topcoat::Result<()> {
    let store = app_context::<Store>(cx);
    let started = session::start(cx).await?;
    store::insert_session(
        &store.db,
        &started.token_hash,
        &account.customer_id,
        started.expires_at,
    )
    .await?;
    Ok(())
}

/// Checks the credentials and opens a session. `Ok(false)` on a bad
/// email/password pair (no hint which).
pub async fn sign_in(cx: &Cx, email: &str, password: &str) -> topcoat::Result<bool> {
    let store = app_context::<Store>(cx);
    let Some((account, hash)) = store::find_credentials(&store.db, email).await? else {
        // Burn comparable time so a missing account is not distinguishable.
        password::verify(password, &password::DUMMY_HASH);
        return Ok(false);
    };
    if !password::verify(password, &hash) {
        return Ok(false);
    }
    start_session(cx, &account).await?;
    tracing::info!(customer_id = %account.customer_id, "shopper signed in");
    Ok(true)
}

/// Closes the current session, if any.
pub async fn sign_out(cx: &Cx) -> topcoat::Result<()> {
    let store = app_context::<Store>(cx);
    if let Some(token_hash) = session::stop(cx).await? {
        store::delete_session(&store.db, &token_hash).await?;
    }
    Ok(())
}

/// Gives an already registered customer a password (the seed step).
pub async fn attach_account(
    store: &Store,
    email: &str,
    password: &str,
    customer_id: &str,
) -> anyhow::Result<()> {
    let email = store::normalize_email(email);
    let hash =
        password::hash(password).ok_or_else(|| anyhow::anyhow!("password hashing failed"))?;
    if store::claim_email(&store.db, &email, &hash).await? {
        store::attach_customer(&store.db, &email, customer_id).await?;
    }
    Ok(())
}
