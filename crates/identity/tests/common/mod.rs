#![allow(dead_code)]

use evento::Executor;
use evento::migrator::Migrate;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use timada_identity::{
    Command, CompleteOAuth2, LoginContext, OAuth2Mode, RegisterPasskey, RegistrationMode,
    VerifiedPasskey,
};

/// A fresh in-memory event store, as in evento's `bank-axum-sqlite` example.
pub async fn executor() -> evento::Sqlite {
    let options = SqliteConnectOptions::new()
        .filename(":memory:")
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    evento::sql_migrator::new::<sqlx::Sqlite>()
        .unwrap()
        .run(&mut *conn, &evento::migrator::Plan::apply_all())
        .await
        .unwrap();
    drop(conn);
    pool.into()
}

pub async fn command() -> Command<evento::Sqlite> {
    Command(executor().await)
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

pub fn register_passkey(
    mode: RegistrationMode,
    session_user_id: Option<&str>,
    bound_user_id: &str,
    credential_id: &str,
) -> RegisterPasskey {
    RegisterPasskey {
        mode,
        session_user_id: session_user_id.map(str::to_owned),
        bound_user_id: bound_user_id.to_owned(),
        passkey: passkey(credential_id),
        account: "alice".to_owned(),
        label: "Alice".to_owned(),
        context: context(),
    }
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
