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

/// How long the link of a « mot de passe oublié » e-mail works.
pub const RESET_LINK_TTL_SECS: i64 = 3600;
/// An account is sent one such e-mail a minute at most, whoever asks.
const RESET_REQUEST_INTERVAL_SECS: i64 = 60;

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

/// Gives somebody who ordered without an account one: a password for the
/// customer they already are, their orders with it. Same order as a sign-up —
/// the address is claimed in SQL first, its primary key being the uniqueness
/// guard — then the customer is told; a refusal there releases the claim.
///
/// An address that already signs into another customer is refused: the two
/// stay two customers, and the guest keeps the link of their e-mails.
pub async fn open_guest_account(
    store: &Store,
    customer_id: &str,
    password: &str,
) -> Result<Account, SignUpError> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(SignUpError::WeakPassword);
    }
    let guest = timada_customer::load_address_book(&store.executor, customer_id)
        .await?
        .filter(|customer| customer.guest)
        .ok_or_else(|| SignUpError::Server(anyhow::anyhow!("{customer_id} is not a guest")))?;
    let email = store::normalize_email(&guest.email);
    let hash = password::hash(password)
        .ok_or_else(|| SignUpError::Server(anyhow::anyhow!("password hashing failed")))?;
    if !store::claim_email(&store.db, &email, &hash).await? {
        return Err(SignUpError::EmailTaken);
    }
    let opened = timada_customer::Command(&store.executor)
        .open_account(customer_id)
        .await;
    if let Err(err) = opened {
        store::release_claim(&store.db, &email)
            .await
            .map_err(anyhow::Error::from)?;
        return Err(SignUpError::Server(err.into()));
    }
    store::attach_customer(&store.db, &email, customer_id)
        .await
        .map_err(anyhow::Error::from)?;
    tracing::info!(%customer_id, "a guest opened an account");
    Ok(Account {
        customer_id: customer_id.to_owned(),
        email,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum ChangeEmailError {
    #[error("Mot de passe incorrect.")]
    WrongPassword,
    #[error("Adresse email invalide.")]
    InvalidEmail,
    #[error("Un compte existe déjà avec cette adresse email.")]
    EmailTaken,
    #[error("C'est déjà l'adresse email de votre compte.")]
    Unchanged,
    #[error(transparent)]
    Server(#[from] anyhow::Error),
}

/// Moves the signed-in shopper to another e-mail address, which is both their
/// login (SQL) and the customer's address (event). Same order as a sign-up:
/// the address is taken in SQL first — its primary key is the uniqueness
/// guard — then recorded on the customer; if that fails the login moves back.
/// The old address is told, in case it was not the shopper who asked.
pub async fn change_email(
    store: &Store,
    account: &Account,
    new_email: &str,
    password: &str,
) -> Result<Account, ChangeEmailError> {
    let credentials = store::find_credentials(&store.db, &account.email)
        .await
        .map_err(anyhow::Error::from)?;
    let verified = credentials.is_some_and(|(_, hash)| password::verify(password, &hash));
    if !verified {
        return Err(ChangeEmailError::WrongPassword);
    }
    let new_email = store::normalize_email(new_email);
    if new_email == account.email {
        return Err(ChangeEmailError::Unchanged);
    }
    if !store::move_account(&store.db, &account.customer_id, &new_email).await? {
        return Err(ChangeEmailError::EmailTaken);
    }

    let changed = timada_customer::Command(&store.executor)
        .change_email(&account.customer_id, new_email.clone())
        .await;
    if let Err(err) = changed {
        store::move_account(&store.db, &account.customer_id, &account.email).await?;
        return Err(match err {
            CustomerError::InvalidEmail(_) => ChangeEmailError::InvalidEmail,
            other => ChangeEmailError::Server(other.into()),
        });
    }

    // A reset link sent to the address being left opens nothing any more.
    store::forget_resets(&store.db, &account.customer_id)
        .await
        .map_err(anyhow::Error::from)?;

    // Straight into the outbox: only the host knows the address being left.
    let config = crate::db::mailer_config();
    let notice = timada_mailer::Email {
        from: config.from.clone(),
        to: account.email.clone(),
        subject: format!("Votre adresse e-mail {} a été modifiée", config.shop_name),
        body: format!(
            "Bonjour,\n\nL'adresse e-mail de votre compte {} est désormais {new_email}. Vous ne \
             recevrez plus nos messages à cette adresse.\n\nSi vous n'êtes pas à l'origine de ce \
             changement, contactez-nous sans attendre.\n\nÀ bientôt,\n{}\n",
            config.shop_name, config.shop_name
        ),
        html_body: None,
        attachments: Vec::new(),
    };
    let message_id = timada_core::id::derived(
        &[&account.customer_id, &account.email, &new_email],
        "email-changed",
    );
    timada_mailer::enqueue(&store.db, &message_id, "email-changed", &notice)
        .await
        .map_err(anyhow::Error::from)?;

    tracing::info!(customer_id = %account.customer_id, "shopper changed their e-mail");
    Ok(Account {
        customer_id: account.customer_id.clone(),
        email: new_email,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum ChangePasswordError {
    #[error("Mot de passe actuel incorrect.")]
    WrongPassword,
    #[error("Le mot de passe doit contenir au moins {MIN_PASSWORD_LEN} caractères.")]
    WeakPassword,
    #[error(transparent)]
    Server(#[from] anyhow::Error),
}

/// Replaces the signed-in shopper's password after checking the current one,
/// signs every *other* browser out, and tells the shopper by e-mail.
pub async fn change_password(
    cx: &Cx,
    account: &Account,
    current: &str,
    new: &str,
) -> Result<(), ChangePasswordError> {
    let store = app_context::<Store>(cx);
    let credentials = store::find_credentials(&store.db, &account.email)
        .await
        .map_err(anyhow::Error::from)?;
    if !credentials.is_some_and(|(_, hash)| password::verify(current, &hash)) {
        return Err(ChangePasswordError::WrongPassword);
    }
    if new.chars().count() < MIN_PASSWORD_LEN {
        return Err(ChangePasswordError::WeakPassword);
    }
    let hash = password::hash(new).ok_or_else(|| anyhow::anyhow!("password hashing failed"))?;
    let keep = session::token_hash(cx)
        .await
        .map_err(|err| anyhow::anyhow!("{err:#}"))?;
    store::replace_password(&store.db, &account.customer_id, &hash, keep.as_ref()).await?;

    password_changed_notice(store, account, "vos autres appareils ont été déconnectés").await?;
    tracing::info!(customer_id = %account.customer_id, "shopper changed their password");
    Ok(())
}

/// Tells the shopper their password changed — `signed_out` says which of
/// their devices were signed out — in case it was not them.
async fn password_changed_notice(
    store: &Store,
    account: &Account,
    signed_out: &str,
) -> anyhow::Result<()> {
    let config = crate::db::mailer_config();
    let notice = timada_mailer::Email {
        from: config.from.clone(),
        to: account.email.clone(),
        subject: format!("Votre mot de passe {} a été modifié", config.shop_name),
        body: format!(
            "Bonjour,\n\nLe mot de passe de votre compte {} vient d'être modifié, et {signed_out}.\n\n\
             Si vous n'êtes pas à l'origine de ce changement, contactez-nous sans attendre.\n\n\
             À bientôt,\n{}\n",
            config.shop_name, config.shop_name
        ),
        html_body: None,
        attachments: Vec::new(),
    };
    let now = timada_core::time::now_unix_secs()?.to_string();
    let message_id = timada_core::id::derived(&[&account.customer_id, &now], "password-changed");
    timada_mailer::enqueue(&store.db, &message_id, "password-changed", &notice).await?;
    Ok(())
}

/// What is kept of a reset link's token: its SHA-256. The token itself only
/// exists in the e-mail.
fn reset_token_hash(token: &str) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    Sha256::digest(token.trim().as_bytes()).to_vec()
}

/// E-mails the account of `email` a link to choose a new password. Says
/// nothing about whether there is such an account: the caller answers the
/// same either way, and so does this.
pub async fn request_password_reset(store: &Store, email: &str) -> anyhow::Result<()> {
    use argon2::password_hash::rand_core::{OsRng, RngCore};

    let Some((account, _)) = store::find_credentials(&store.db, email).await? else {
        tracing::info!("password reset asked for an address without an account");
        return Ok(());
    };
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let token = hex::encode(bytes);
    let token_hash = reset_token_hash(&token);
    let recorded = store::insert_reset(
        &store.db,
        &token_hash,
        &account.customer_id,
        RESET_LINK_TTL_SECS,
        RESET_REQUEST_INTERVAL_SECS,
    )
    .await?;
    if !recorded {
        tracing::info!(customer_id = %account.customer_id, "password reset asked again too soon");
        return Ok(());
    }

    let config = crate::db::mailer_config();
    let link = format!(
        "{}/password/reset?token={token}",
        config.base_url.trim_end_matches('/')
    );
    let message = timada_mailer::Email {
        from: config.from.clone(),
        to: account.email.clone(),
        subject: format!("Choisissez un nouveau mot de passe {}", config.shop_name),
        body: format!(
            "Bonjour,\n\nVous avez demandé à choisir un nouveau mot de passe pour votre compte {}. \
             Ce lien est valable une heure et ne sert qu'une fois :\n\n{link}\n\n\
             Si vous n'êtes pas à l'origine de cette demande, ignorez ce message : votre mot de \
             passe reste inchangé.\n\nÀ bientôt,\n{}\n",
            config.shop_name, config.shop_name
        ),
        html_body: None,
        attachments: Vec::new(),
    };
    let message_id = timada_core::id::derived(
        &[&account.customer_id, &hex::encode(&token_hash)],
        "password-reset",
    );
    timada_mailer::enqueue(&store.db, &message_id, "password-reset", &message).await?;
    tracing::info!(customer_id = %account.customer_id, "password reset link sent");
    Ok(())
}

/// Whether a reset link still opens an account (unused, not run out).
pub async fn reset_link_is_valid(store: &Store, token: &str) -> anyhow::Result<bool> {
    Ok(store::find_reset(&store.db, &reset_token_hash(token))
        .await?
        .is_some())
}

#[derive(Debug, thiserror::Error)]
pub enum ResetPasswordError {
    #[error("Ce lien n'est plus valable.")]
    InvalidLink,
    #[error("Le mot de passe doit contenir au moins {MIN_PASSWORD_LEN} caractères.")]
    WeakPassword,
    #[error(transparent)]
    Server(#[from] anyhow::Error),
}

/// Gives the account behind a reset link a new password. The link is used up,
/// every browser signed in to the account is signed out — whoever had the old
/// password is out too — and the shopper is told by e-mail. Nobody is signed
/// in: the new password is what proves who is there.
pub async fn reset_password(
    store: &Store,
    token: &str,
    new: &str,
) -> Result<(), ResetPasswordError> {
    let token_hash = reset_token_hash(token);
    let Some(account) = store::find_reset(&store.db, &token_hash).await? else {
        return Err(ResetPasswordError::InvalidLink);
    };
    if new.chars().count() < MIN_PASSWORD_LEN {
        return Err(ResetPasswordError::WeakPassword);
    }
    let hash = password::hash(new).ok_or_else(|| anyhow::anyhow!("password hashing failed"))?;
    if !store::consume_reset(&store.db, &token_hash).await? {
        return Err(ResetPasswordError::InvalidLink);
    }
    store::replace_password(&store.db, &account.customer_id, &hash, None).await?;
    password_changed_notice(store, &account, "tous vos appareils ont été déconnectés").await?;
    tracing::info!(customer_id = %account.customer_id, "shopper reset their password");
    Ok(())
}

/// The shopper the request's session belongs to, if any. Memoized per request.
#[memoize(as_ref)]
pub async fn current_account(cx: &Cx) -> topcoat::Result<Option<Account>> {
    let Some(token_hash) = session::token_hash(cx).await? else {
        return Ok(None);
    };
    let store = app_context::<Store>(cx);
    store::find_by_session(&store.db, &token_hash)
        .await
        .map_err(topcoat::Error::from_anyhow)
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
        Err(err) => Err(topcoat::Error::msg(format!("{err:#}"))),
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
    .await
    .map_err(topcoat::Error::from_anyhow)?;
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
