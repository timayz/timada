#![allow(dead_code)]

use evento::Executor;
use evento::migrator::Migrate;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx_migrator::Info;
use timada_identity::{
    AddPasskey, ChallengePurpose, Command, CompleteOAuth2, LoginContext, OAuth2Mode,
    RegisterWithPassword, SignInPasskey, VerifiedPasskey, hash_password,
};

/// A fixed "now" for the commands that take one (unix seconds).
pub const NOW: u64 = 1_700_000_000;

/// A password that passes the policy.
pub const PASSWORD: &str = "correct horse battery";

/// A fresh in-memory event store, as in evento's `bank-axum-sqlite` example.
pub async fn executor() -> evento::Sqlite {
    executor_with_pool().await.0
}

/// The store plus the pool it runs on, with the read-model schema applied,
/// for tests that drive the subscription.
pub async fn executor_with_pool() -> (evento::Sqlite, sqlx::SqlitePool) {
    let options = SqliteConnectOptions::new()
        .filename(":memory:")
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let mut migrator = evento::sql_migrator::new::<sqlx::Sqlite>().unwrap();
    migrator
        .add_migrations(timada_identity::migrations())
        .unwrap();
    migrator
        .run(&mut *conn, &evento::migrator::Plan::apply_all())
        .await
        .unwrap();
    drop(conn);
    (pool.clone().into(), pool)
}

pub async fn command() -> Command<evento::Sqlite> {
    Command::new(executor().await, codes())
}

/// The verifier the tests' codes are tagged under.
pub fn codes() -> std::sync::Arc<dyn timada_identity::CodeVerifier> {
    std::sync::Arc::new(timada_identity::HmacCodeVerifier::new([7; 32]))
}

pub fn identity(provider: &str, sub: &str) -> timada_identity::ProviderIdentity {
    timada_identity::ProviderIdentity {
        provider: provider.to_owned(),
        provider_user_id: sub.to_owned(),
        email: Some(format!("{sub}@example.com")),
        name: Some(format!("User {sub}")),
        picture: None,
    }
}

pub fn context() -> LoginContext {
    LoginContext {
        user_agent: Some("tests".to_owned()),
        ip: Some("127.0.0.1".to_owned()),
    }
}

pub fn oauth2(
    mode: OAuth2Mode,
    session_user_id: Option<&str>,
    provider: &str,
    sub: &str,
) -> CompleteOAuth2 {
    CompleteOAuth2 {
        mode,
        session_user_id: session_user_id.map(str::to_owned),
        identity: identity(provider, sub),
        context: context(),
    }
}

pub fn passkey(credential_id: &str) -> VerifiedPasskey {
    VerifiedPasskey {
        credential_id: credential_id.to_owned(),
        passkey_json: format!("{{\"cred_id\":\"{credential_id}\",\"counter\":0}}"),
        aaguid: "00000000-0000-0000-0000-000000000000".to_owned(),
        rp_id: "timada.localhost".to_owned(),
        name: "Test key".to_owned(),
    }
}

pub fn add_passkey(
    session_user_id: Option<&str>,
    bound_user_id: &str,
    credential_id: &str,
) -> AddPasskey {
    AddPasskey {
        session_user_id: session_user_id.map(str::to_owned),
        bound_user_id: bound_user_id.to_owned(),
        passkey: passkey(credential_id),
    }
}

/// Registers a user with an e-mail address and [`PASSWORD`], proving the
/// address with the code the domain issued, and returns the user id.
pub async fn register_via_email<E: Executor>(cmd: &Command<E>, email: &str) -> String {
    let issued = cmd
        .issue_email_code(email, ChallengePurpose::Register, NOW)
        .await
        .unwrap();
    cmd.register_with_password(
        RegisterWithPassword {
            email: email.to_owned(),
            code: issued.code,
            password_hash: hash_password(PASSWORD).await.unwrap(),
            context: context(),
        },
        NOW,
    )
    .await
    .unwrap()
    .user_id
}

/// Registers a user through the login page's default flow and returns its id.
pub async fn register_via_oauth2<E: Executor>(
    cmd: &Command<E>,
    provider: &str,
    sub: &str,
) -> String {
    cmd.complete_oauth2(oauth2(OAuth2Mode::CreateUserOrLogin, None, provider, sub))
        .await
        .unwrap()
        .user_id
}

pub fn new_id() -> String {
    ulid::Ulid::generate().to_string()
}

pub fn sign_in_passkey(
    credential_id: &str,
    updated_passkey_json: Option<&str>,
    session_user_id: Option<&str>,
) -> SignInPasskey {
    SignInPasskey {
        credential_id: credential_id.to_owned(),
        updated_passkey_json: updated_passkey_json.map(str::to_owned),
        session_user_id: session_user_id.map(str::to_owned),
        context: context(),
    }
}
