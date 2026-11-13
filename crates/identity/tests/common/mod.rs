#![allow(dead_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use evento::cursor::{Args, ReadResult, Value};
use evento::{
    Aggregate, AggregateEvent, EventFilter, Executor, RoutingKey, TenantFilter, WriteError,
};
use timada_core::db::Db;
use timada_identity::{
    AddPasskey, ChallengePurpose, Command, CompleteOAuth2, LoginContext, OAuth2Mode,
    RegisterWithPassword, SignInPasskey, VerifiedPasskey, hash_password,
};
use tokio::sync::Notify;
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
/// It can also hold a read, or a write, back until another write lands, to
/// stage a race.
#[derive(Clone)]
pub struct Refusing {
    pub store: evento::Turso,
    refused: Vec<(&'static str, &'static str, Refusal)>,
    held_read: Option<Arc<Held>>,
    held_write: Option<Arc<Held>>,
}

/// How a refused write fails.
#[derive(Clone, Copy)]
enum Refusal {
    /// The store could not tell whether it landed.
    Unknown,
    /// It lost a version race: surely not written.
    LostRace,
}

/// The first read, or write, of one aggregate type waits until another write
/// to that type has landed.
struct Held {
    aggregate_type: &'static str,
    came: AtomicBool,
    parked: Notify,
    written: Notify,
}

impl Held {
    fn of<A: Aggregate>() -> Arc<Self> {
        Arc::new(Self {
            aggregate_type: A::aggregate_type(),
            came: AtomicBool::new(false),
            parked: Notify::new(),
            written: Notify::new(),
        })
    }

    fn written_by(&self, events: &[evento::Event]) -> bool {
        events
            .iter()
            .any(|event| event.aggregate_type == self.aggregate_type)
    }
}

impl Refusing {
    pub fn new(store: evento::Turso) -> Self {
        Self {
            store,
            refused: Vec::new(),
            held_read: None,
            held_write: None,
        }
    }

    /// Also fails every write of `D`, its outcome unknown.
    pub fn refuse<D: AggregateEvent>(mut self) -> Self {
        self.refused
            .push((D::aggregate_type(), D::event_name(), Refusal::Unknown));
        self
    }

    /// Also fails every write of `D` as a lost version race.
    pub fn lose_race<D: AggregateEvent>(mut self) -> Self {
        self.refused
            .push((D::aggregate_type(), D::event_name(), Refusal::LostRace));
        self
    }

    /// Holds the first read of an `A` stream until a write to one lands.
    pub fn hold_first_read<A: Aggregate>(mut self) -> Self {
        self.held_read = Some(Held::of::<A>());
        self
    }

    /// Holds the first write to an `A` stream until another write to one
    /// lands: the two race, and the held one comes second.
    pub fn hold_first_write<A: Aggregate>(mut self) -> Self {
        self.held_write = Some(Held::of::<A>());
        self
    }

    /// Resolves once the read [`Self::hold_first_read`] names, or the write
    /// [`Self::hold_first_write`] names, is held.
    pub async fn parked(&self) {
        if let Some(held) = self.held_read.as_ref().or(self.held_write.as_ref()) {
            held.parked.notified().await;
        }
    }
}

#[async_trait::async_trait]
impl Executor for Refusing {
    async fn write(&self, events: Vec<evento::Event>) -> Result<(), WriteError> {
        for event in &events {
            let refusal = self
                .refused
                .iter()
                .find(|(kind, name, _)| event.aggregate_type == *kind && event.name == *name);
            match refusal {
                Some((_, _, Refusal::Unknown)) => {
                    return Err(WriteError::Unknown(anyhow::anyhow!(
                        "refused {}::{}",
                        event.aggregate_type,
                        event.name
                    )));
                }
                Some((_, _, Refusal::LostRace)) => return Err(WriteError::InvalidOriginalVersion),
                None => {}
            }
        }
        if let Some(held) = self
            .held_write
            .as_ref()
            .filter(|held| held.written_by(&events))
            && !held.came.swap(true, Ordering::SeqCst)
        {
            held.parked.notify_one();
            held.written.notified().await;
            return self.store.write(events).await;
        }
        // What is held, once it came, waits for this write.
        let waiting: Vec<Arc<Held>> = [&self.held_read, &self.held_write]
            .into_iter()
            .flatten()
            .filter(|held| held.came.load(Ordering::SeqCst) && held.written_by(&events))
            .cloned()
            .collect();
        let written = self.store.write(events).await;
        if written.is_ok() {
            for held in waiting {
                held.written.notify_one();
            }
        }
        written
    }

    async fn read(
        &self,
        aggregators: Option<Arc<[EventFilter]>>,
        routing_key: Option<RoutingKey>,
        tenant: TenantFilter,
        args: Args,
        to_micros: Option<u64>,
    ) -> anyhow::Result<ReadResult<evento::Event>> {
        if let Some(held) = &self.held_read {
            let of_held_type = aggregators.as_deref().is_some_and(|filters| {
                filters
                    .iter()
                    .any(|filter| filter.aggregate_type == held.aggregate_type)
            });
            if of_held_type && !held.came.swap(true, Ordering::SeqCst) {
                held.parked.notify_one();
                held.written.notified().await;
            }
        }
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
