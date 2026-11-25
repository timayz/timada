//! Agent keys: a user makes one on their account page, limited to the
//! shops and the permissions they choose, and gets the token once. An agent
//! that presents it to the dashboard's MCP server acts as that user, within
//! what the key covers and never beyond what the user holds when it asks.
//!
//! A user's keys are one stream ([`agent_keys_id`]); the token names its
//! user, so a key is found without a table. What a key asked for is logged
//! a day at a time on a stream of its own ([`agent_calls_id`]), which its
//! user reads; an event a call would otherwise crowd the keys' stream.

use evento::{Executor, Projection, ProjectionAggregate, WriteError, metadata::Event};

use crate::aggregator::agent::{
    AgentCalls, AgentKeyUse, AgentKeys, AgentShop, Called, Minted, UsedOn, Withdrawn,
};
use crate::error::TenancyError;
use crate::value_object::{
    MAX_AGENT_CALLS_A_DAY, MAX_AGENT_KEY_SHOPS, MAX_AGENT_KEYS, PERMISSIONS, agent_calls_id,
    agent_key_hint, agent_key_id, agent_key_use_id, agent_key_user, agent_keys_id,
    generate_agent_key_token, normalize_api_key_label,
};

/// How many times a write is decided again when another write to the
/// stream came in between.
const ATTEMPTS: usize = 5;

/// How many of a day's calls a log keeps to show: the last ones.
pub const AGENT_CALLS_SHOWN: usize = 200;

/// A key of a user's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentKeyState {
    /// The hash of its token.
    pub id: String,
    pub label: String,
    pub hint: String,
    /// The shops it covers, each with what it may do there.
    pub shops: Vec<AgentShop>,
    /// Unix seconds.
    pub minted_at: u64,
    pub withdrawn_at: Option<u64>,
}

impl AgentKeyState {
    pub fn is_active(&self) -> bool {
        self.withdrawn_at.is_none()
    }

    /// Whether the key covers `permission` in the shop `tenant_id`: one of
    /// its permissions there grants it.
    pub fn allows(&self, tenant_id: &str, permission: &str) -> bool {
        self.shops.iter().any(|shop| {
            shop.tenant_id == tenant_id
                && shop
                    .permissions
                    .iter()
                    .any(|held| crate::implies(held, permission))
        })
    }
}

/// The write model of a user's agent keys.
#[evento::projection(id = id, name = "timada-tenancy/AgentKeysState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct AgentKeysState {
    pub id: String,
    pub user_id: String,
    /// Every key made, the oldest first, the withdrawn ones included.
    pub keys: Vec<AgentKeyState>,
}

impl AgentKeysState {
    pub fn key(&self, key_id: &str) -> Option<&AgentKeyState> {
        self.keys.iter().find(|key| key.id == key_id)
    }
}

pub fn agent_keys_projection<E: Executor>() -> Projection<E, AgentKeysState> {
    Projection::new::<AgentKeys>()
        .handler(on_agent_key_minted())
        .handler(on_agent_key_withdrawn())
        .strict()
}

#[evento::handler]
async fn on_agent_key_minted(
    event: Event<Minted>,
    keys: &mut AgentKeysState,
) -> anyhow::Result<()> {
    keys.id = event.aggregate_id.to_owned();
    keys.user_id = event.data.user_id.to_owned();
    keys.keys.push(AgentKeyState {
        id: event.data.key_id.to_owned(),
        label: event.data.label.to_owned(),
        hint: event.data.hint.to_owned(),
        shops: event.data.shops.to_owned(),
        minted_at: event.timestamp,
        withdrawn_at: None,
    });
    Ok(())
}

#[evento::handler]
async fn on_agent_key_withdrawn(
    event: Event<Withdrawn>,
    keys: &mut AgentKeysState,
) -> anyhow::Result<()> {
    if let Some(key) = keys.keys.iter_mut().find(|key| key.id == event.data.key_id) {
        key.withdrawn_at = Some(event.timestamp);
    }
    Ok(())
}

/// The days a key was used on.
#[evento::projection(id = id, name = "timada-tenancy/AgentKeyUseState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct AgentKeyUseState {
    pub id: String,
    /// The last day (`2026-11-25`).
    pub last_day: String,
}

pub fn agent_key_use_projection<E: Executor>() -> Projection<E, AgentKeyUseState> {
    Projection::new::<AgentKeyUse>()
        .handler(on_agent_key_used())
        .strict()
}

#[evento::handler]
async fn on_agent_key_used(
    event: Event<UsedOn>,
    used: &mut AgentKeyUseState,
) -> anyhow::Result<()> {
    used.id = event.aggregate_id.to_owned();
    used.last_day = event.data.day.to_owned();
    Ok(())
}

/// One call of a key's log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCall {
    pub tool: String,
    /// The shop it was about; empty for none.
    pub tenant_id: String,
    pub ok: bool,
    /// Unix seconds.
    pub at: u64,
}

/// What a key asked for on a day.
#[evento::projection(id = id, name = "timada-tenancy/AgentCallsState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct AgentCallsState {
    pub id: String,
    /// How many calls the day holds.
    pub count: u32,
    /// The last [`AGENT_CALLS_SHOWN`] of them, the oldest first.
    pub last: Vec<AgentCall>,
}

pub fn agent_calls_projection<E: Executor>() -> Projection<E, AgentCallsState> {
    Projection::new::<AgentCalls>()
        .handler(on_agent_called())
        .strict()
}

#[evento::handler]
async fn on_agent_called(event: Event<Called>, calls: &mut AgentCallsState) -> anyhow::Result<()> {
    calls.id = event.aggregate_id.to_owned();
    calls.count += 1;
    if calls.last.len() == AGENT_CALLS_SHOWN {
        calls.last.remove(0);
    }
    calls.last.push(AgentCall {
        tool: event.data.tool.to_owned(),
        tenant_id: event.data.tenant_id.to_owned(),
        ok: event.data.ok,
        at: event.timestamp,
    });
    Ok(())
}

/// A key to make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintAgentKey {
    pub label: String,
    /// The shops it covers, each with the permissions it gets there.
    pub shops: Vec<(String, Vec<String>)>,
}

/// A key just made, with its token. The plaintext token exists only in
/// this value (and wherever the person who made it pastes it).
#[derive(Clone, PartialEq, Eq)]
pub struct MintedAgentKey {
    pub token: String,
    pub key: AgentKeyState,
}

impl std::fmt::Debug for MintedAgentKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MintedAgentKey")
            .field("token", &"******")
            .field("key", &self.key)
            .finish()
    }
}

impl<E: Executor> super::Command<E> {
    /// The keys `user_id` made; `None` before the first.
    pub async fn load_agent_keys(&self, user_id: &str) -> anyhow::Result<Option<AgentKeysState>> {
        agent_keys_projection()
            .load(agent_keys_id(user_id))
            .execute(&self.0)
            .await
    }

    /// The user and the key a token is of, while the key stands; `None`
    /// for anything else (no such key, one withdrawn, not a token).
    pub async fn agent_key_of(
        &self,
        token: &str,
    ) -> anyhow::Result<Option<(String, AgentKeyState)>> {
        let Some(user_id) = agent_key_user(token) else {
            return Ok(None);
        };
        let key_id = agent_key_id(token);
        Ok(self
            .load_agent_keys(user_id)
            .await?
            .and_then(|keys| keys.key(&key_id).cloned())
            .filter(AgentKeyState::is_active)
            .map(|key| (user_id.to_owned(), key)))
    }

    /// The last day (`2026-11-25`) the key was used on; `None` for one
    /// never used.
    pub async fn agent_key_last_day(&self, key_id: &str) -> anyhow::Result<Option<String>> {
        Ok(agent_key_use_projection()
            .load(agent_key_use_id(key_id))
            .execute(&self.0)
            .await?
            .map(|used| used.last_day))
    }

    /// What the key asked for on `day`; `None` for a day it asked nothing.
    pub async fn load_agent_calls(
        &self,
        key_id: &str,
        day: &str,
    ) -> anyhow::Result<Option<AgentCallsState>> {
        agent_calls_projection()
            .load(agent_calls_id(key_id, day))
            .execute(&self.0)
            .await
    }
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Makes a key for `actor_user_id`, limited to the shops and
    /// permissions of `cmd`: each a shop the user is a member of that is
    /// not suspended, with permissions the user holds there. Returns the
    /// token, once.
    pub async fn mint_agent_key(
        &self,
        actor_user_id: &str,
        cmd: MintAgentKey,
        routing_key: Option<String>,
    ) -> Result<MintedAgentKey, TenancyError> {
        let label = normalize_api_key_label(&cmd.label)?;
        if cmd.shops.is_empty() || cmd.shops.len() > MAX_AGENT_KEY_SHOPS {
            return Err(TenancyError::InvalidAgentKeyScope);
        }
        let mut shops: Vec<AgentShop> = Vec::with_capacity(cmd.shops.len());
        for (tenant_id, permissions) in &cmd.shops {
            let tenant = self
                .load_tenant(tenant_id)
                .await?
                .filter(|tenant| !tenant.is_suspended())
                .ok_or(TenancyError::InvalidAgentKeyScope)?;
            // In the order the vocabulary lists them, each once.
            let kept: Vec<String> = PERMISSIONS
                .iter()
                .filter(|known| permissions.iter().any(|asked| asked == *known))
                .map(|known| (*known).to_owned())
                .collect();
            let unknown = permissions
                .iter()
                .any(|asked| !PERMISSIONS.contains(&asked.as_str()));
            let wider = kept
                .iter()
                .any(|permission| !tenant.can(actor_user_id, permission));
            if kept.is_empty()
                || unknown
                || wider
                || shops.iter().any(|shop| shop.tenant_id == *tenant_id)
            {
                return Err(TenancyError::InvalidAgentKeyScope);
            }
            shops.push(AgentShop {
                tenant_id: tenant_id.clone(),
                permissions: kept,
            });
        }
        let token = generate_agent_key_token(actor_user_id);
        let minted = Minted {
            user_id: actor_user_id.to_owned(),
            key_id: agent_key_id(&token),
            label,
            hint: agent_key_hint(&token),
            shops,
            minted_by: actor_user_id.to_owned(),
        };
        for _ in 0..ATTEMPTS {
            let loaded = self.load_agent_keys(actor_user_id).await?;
            let live = loaded.as_ref().map_or(0, |keys| {
                keys.keys.iter().filter(|key| key.is_active()).count()
            });
            if live >= MAX_AGENT_KEYS {
                return Err(TenancyError::TooManyAgentKeys);
            }
            let mut write = match &loaded {
                Some(keys) => keys.write()?,
                None => {
                    let mut write = evento::append(agent_keys_id(actor_user_id));
                    write.original_version(0);
                    write
                }
            };
            write.routing_key_opt(routing_key.clone()).event(&minted);
            match write.commit(&self.0).await {
                Ok(_) => {
                    let key = self
                        .load_agent_keys(actor_user_id)
                        .await?
                        .and_then(|keys| keys.key(&minted.key_id).cloned())
                        .ok_or(TenancyError::AgentKeyNotFound)?;
                    return Ok(MintedAgentKey { token, key });
                }
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Err(WriteError::InvalidOriginalVersion.into())
    }

    /// Withdraws the key `key_id` of `actor_user_id`: an agent presenting
    /// its token is turned away from then on. One withdrawn already is a
    /// no-op.
    pub async fn withdraw_agent_key(
        &self,
        actor_user_id: &str,
        key_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        for _ in 0..ATTEMPTS {
            let keys = self
                .load_agent_keys(actor_user_id)
                .await?
                .ok_or(TenancyError::AgentKeyNotFound)?;
            let key = keys.key(key_id).ok_or(TenancyError::AgentKeyNotFound)?;
            if !key.is_active() {
                return Ok(());
            }
            let mut write = keys.write()?;
            write
                .routing_key_opt(routing_key.clone())
                .event(&Withdrawn {
                    key_id: key_id.to_owned(),
                    withdrawn_by: actor_user_id.to_owned(),
                });
            match write.commit(&self.0).await {
                Ok(_) => return Ok(()),
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Err(WriteError::InvalidOriginalVersion.into())
    }

    /// Logs that the key `key_id` asked for `tool` about the shop
    /// `tenant_id` (empty for none) at `now`, answered or refused, and
    /// marks the day as one the key was used on. Past
    /// [`MAX_AGENT_CALLS_A_DAY`] a day the log is full and nothing more is
    /// written: a stream holds so many events.
    pub async fn log_agent_call(
        &self,
        key_id: &str,
        tool: &str,
        tenant_id: &str,
        ok: bool,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let day = timada_core::civil::format_date(now);
        let called = Called {
            tool: tool.chars().take(80).collect(),
            tenant_id: tenant_id.to_owned(),
            ok,
        };
        let mut first = false;
        for _ in 0..ATTEMPTS {
            let loaded = self.load_agent_calls(key_id, &day).await?;
            if loaded
                .as_ref()
                .is_some_and(|calls| calls.count >= MAX_AGENT_CALLS_A_DAY)
            {
                return Ok(());
            }
            first = loaded.is_none();
            let mut write = match &loaded {
                Some(calls) => calls.write()?,
                None => {
                    let mut write = evento::append(agent_calls_id(key_id, &day));
                    write.original_version(0);
                    write
                }
            };
            write.routing_key_opt(routing_key.clone()).event(&called);
            match write.commit(&self.0).await {
                Ok(_) => break,
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
        if first {
            let used = agent_key_use_projection()
                .load(agent_key_use_id(key_id))
                .execute(&self.0)
                .await?;
            if used.as_ref().is_none_or(|used| used.last_day != day) {
                let mut write = match &used {
                    Some(used) => used.write()?,
                    None => {
                        let mut write = evento::append(agent_key_use_id(key_id));
                        write.original_version(0);
                        write
                    }
                };
                write
                    .routing_key_opt(routing_key)
                    .event(&UsedOn { day: day.clone() });
                match write.commit(&self.0).await {
                    // Another node marked the day at the same moment.
                    Ok(_) | Err(WriteError::InvalidOriginalVersion) => {}
                    Err(err) => return Err(err.into()),
                }
            }
        }
        Ok(())
    }
}
