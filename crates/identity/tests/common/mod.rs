#![allow(dead_code)]

use std::sync::Arc;

use evento::cursor::{Args, ReadResult, Value};
use evento::{AggregateEvent, EventFilter, Executor, RoutingKey, TenantFilter, WriteError};
use timada_core::db::Db;
use timada_identity::{
    AddPasskey, ChallengePurpose, Command, CompleteOAuth2, LoginContext, OAuth2Mode,
    RegisterWithPassword, SignInPasskey, VerifiedPasskey, hash_password,
};
use ulid::Ulid;

/// A fixed "now" for the commands that take one (unix seconds).
pub const NOW: u64 = 1_700_000_000;

/// A password that passes the policy.
pub const PASSWORD: &str = "correct horse battery";

/// A fresh in-memory event store.
pub async fn executor() -> evento::Turso {
    executor_with_pool().await.0
}

/// The store plus the read-model database in it, with the read-model schema
/// applied, for tests that drive the subscription.
pub async fn executor_with_pool() -> (evento::Turso, Db) {
    let store = evento::Turso::in_memory_concurrent().await.unwrap();
    let mut conn = store.connect().unwrap();
    timada_identity::MIGRATIONS
        .to_latest(&mut conn)
        .await
        .map_err(|(_, err)| err)
        .unwrap();
    (store.clone(), Db::from(store))
}

pub async fn command() -> Command<evento::Turso> {
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

/// The store of a node whose writes of some events fail, as if it stopped
/// right there: a write holding one of them never lands, every other does.
#[derive(Clone)]
pub struct Refusing {
    pub store: evento::Turso,
    refused: Vec<(&'static str, &'static str)>,
}

impl Refusing {
    pub fn new(store: evento::Turso) -> Self {
        Self {
            store,
            refused: Vec::new(),
        }
    }

    /// Also refuses every write of `D`.
    pub fn refuse<D: AggregateEvent>(mut self) -> Self {
        self.refused.push((D::aggregate_type(), D::event_name()));
        self
    }
}

#[async_trait::async_trait]
impl Executor for Refusing {
    async fn write(&self, events: Vec<evento::Event>) -> Result<(), WriteError> {
        if let Some(event) = events.iter().find(|event| {
            self.refused
                .iter()
                .any(|(kind, name)| event.aggregate_type == *kind && event.name == *name)
        }) {
            return Err(WriteError::Unknown(anyhow::anyhow!(
                "refused {}::{}",
                event.aggregate_type,
                event.name
            )));
        }
        self.store.write(events).await
    }

    async fn read(
        &self,
        aggregators: Option<Arc<[EventFilter]>>,
        routing_key: Option<RoutingKey>,
        tenant: TenantFilter,
        args: Args,
        to_micros: Option<u64>,
    ) -> anyhow::Result<ReadResult<evento::Event>> {
        self.store
            .read(aggregators, routing_key, tenant, args, to_micros)
            .await
    }

    async fn latest_timestamp(
        &self,
        aggregators: Option<Arc<[EventFilter]>>,
        routing_key: Option<RoutingKey>,
        tenant: TenantFilter,
    ) -> anyhow::Result<u64> {
        self.store
            .latest_timestamp(aggregators, routing_key, tenant)
            .await
    }

    async fn get_subscriber_cursor(&self, key: String) -> anyhow::Result<Option<Value>> {
        self.store.get_subscriber_cursor(key).await
    }

    async fn is_subscriber_running(&self, key: String, worker_id: Ulid) -> anyhow::Result<bool> {
        self.store.is_subscriber_running(key, worker_id).await
    }

    async fn upsert_subscriber(&self, key: String, worker_id: Ulid) -> anyhow::Result<()> {
        self.store.upsert_subscriber(key, worker_id).await
    }

    async fn acknowledge(
        &self,
        key: String,
        worker_id: Ulid,
        cursor: Value,
        lag: u64,
    ) -> anyhow::Result<bool> {
        self.store.acknowledge(key, worker_id, cursor, lag).await
    }

    async fn get_snapshot(
        &self,
        tenant: String,
        aggregate_type: String,
        projection: String,
        aggregate_revision: String,
        id: String,
    ) -> anyhow::Result<Option<(Vec<u8>, Value)>> {
        self.store
            .get_snapshot(tenant, aggregate_type, projection, aggregate_revision, id)
            .await
    }

    async fn save_snapshot(
        &self,
        tenant: String,
        aggregate_type: String,
        projection: String,
        aggregate_revision: String,
        id: String,
        data: Vec<u8>,
        cursor: Value,
    ) -> anyhow::Result<()> {
        self.store
            .save_snapshot(
                tenant,
                aggregate_type,
                projection,
                aggregate_revision,
                id,
                data,
                cursor,
            )
            .await
    }

    async fn delete_snapshot(
        &self,
        tenant: String,
        aggregate_type: String,
        projection: String,
        id: String,
    ) -> anyhow::Result<()> {
        self.store
            .delete_snapshot(tenant, aggregate_type, projection, id)
            .await
    }
}

/// The identity gateway over a store that refuses what `refusing` says.
pub fn refusing_command(refusing: Refusing) -> Command<Refusing> {
    Command::new(refusing, codes())
}

/// Frees whatever index entries the subscription finds to free, once.
pub async fn release_indexes(executor: &evento::Turso) {
    timada_identity::index_release_subscription::<evento::Turso>()
        .run_once(executor)
        .await
        .unwrap();
}
