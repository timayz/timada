//! What the model is paid, and the day's caps on it.
//!
//! Every call is charged to the platform's account, whichever shop asks,
//! so a day (UTC) has two caps: one per shop, one for every shop together.
//! They count what each call cost, which is only known once the model
//! answered. So room is held for a call before it is made, at the most it
//! may cost; a call the caps have no room for is refused before the model
//! is asked; and the charge that follows puts the real cost in the hold's
//! place. A cap is therefore never passed, and the last calls before it are
//! refused while a little of it is left.
//!
//! A day is one stream, written against the version it was read at: two
//! nodes holding room at once cannot both take the last of it, so the count
//! holds over a cluster. A node that held room and stopped before charging
//! leaves the hold there; it counts for
//! [`DRAFT_STALE_SECS`](crate::DRAFT_STALE_SECS), as long as its call may
//! still be on its way.

use std::collections::BTreeMap;

use evento::{
    Executor, Projection, ProjectionAggregate, WriteBuilder, WriteError, metadata::Event,
};

use crate::aggregator::{Charged, Held, Released, SpendDay};
use crate::error::WriterError;
use crate::value_object::{
    DRAFT_STALE_SECS, MAX_DAY_HOLDS, SpendCap, SpendCaps, spend_day, spend_day_ends, spend_day_id,
};

/// The gateway over the platform's own streams, where the days of
/// spending live.
pub struct Spend<E: Executor>(pub E);

/// How many times a write to the day decides again when another came in
/// between: every paid call of the platform writes the one stream.
const SPEND_WRITE_ATTEMPTS: usize = 12;

/// Room held for a call that is not charged yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoldState {
    pub id: String,
    pub tenant_id: String,
    pub draft_id: String,
    pub most_usd_micros: u64,
    /// Unix seconds of `Held`.
    pub held_at: u64,
}

impl HoldState {
    /// Whether the hold still counts at `now`: its call may be on its way.
    fn counts(&self, now: u64) -> bool {
        now.saturating_sub(self.held_at) < DRAFT_STALE_SECS
    }
}

/// What one shop's calls cost on a day.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ShopSpend {
    pub usd_micros: u64,
    /// How many calls were charged.
    pub calls: u32,
}

/// The write model of a day of spending.
#[evento::projection(id = id, name = "timada-writer/SpendDayState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct SpendDayState {
    pub id: String,
    /// What was charged, by shop.
    pub shops: BTreeMap<String, ShopSpend>,
    /// The holds neither charged nor released.
    pub holds: Vec<HoldState>,
    /// How many holds the day took in all.
    pub held: u32,
}

impl SpendDayState {
    /// What the calls of the shop `tenant_id` were charged on the day.
    pub fn charged_to(&self, tenant_id: &str) -> ShopSpend {
        self.shops.get(tenant_id).copied().unwrap_or_default()
    }

    /// What every call of the day was charged.
    pub fn charged(&self) -> u64 {
        self.shops
            .values()
            .fold(0, |sum, shop| sum.saturating_add(shop.usd_micros))
    }

    /// What is held at `now` for the calls on their way, for one shop or
    /// for all.
    fn held_at(&self, tenant_id: Option<&str>, now: u64) -> u64 {
        self.holds
            .iter()
            .filter(|hold| hold.counts(now))
            .filter(|hold| tenant_id.is_none_or(|tenant_id| hold.tenant_id == tenant_id))
            .fold(0, |sum, hold| sum.saturating_add(hold.most_usd_micros))
    }

    /// The cap a call for the shop `tenant_id` that may cost
    /// `most_usd_micros` would pass at `now`, the shop's own first; `None`
    /// when there is room.
    pub fn refusal(
        &self,
        caps: SpendCaps,
        tenant_id: &str,
        most_usd_micros: u64,
        now: u64,
    ) -> Option<SpendCap> {
        let shop = self
            .charged_to(tenant_id)
            .usd_micros
            .saturating_add(self.held_at(Some(tenant_id), now))
            .saturating_add(most_usd_micros);
        if shop > caps.shop_usd_micros {
            return Some(SpendCap::Shop);
        }
        let platform = self
            .charged()
            .saturating_add(self.held_at(None, now))
            .saturating_add(most_usd_micros);
        if platform > caps.platform_usd_micros || self.held >= MAX_DAY_HOLDS {
            return Some(SpendCap::Platform);
        }
        None
    }

    fn hold(&self, hold: &str) -> Option<&HoldState> {
        self.holds.iter().find(|held| held.id == hold)
    }
}

pub fn spend_day_projection<E: Executor>() -> Projection<E, SpendDayState> {
    Projection::new::<SpendDay>()
        .handler(on_held())
        .handler(on_charged())
        .handler(on_released())
        .strict()
}

#[evento::handler]
async fn on_held(event: Event<Held>, day: &mut SpendDayState) -> anyhow::Result<()> {
    day.id = event.aggregate_id.to_owned();
    day.held = day.held.saturating_add(1);
    day.holds.push(HoldState {
        id: event.data.hold.to_owned(),
        tenant_id: event.data.tenant_id.to_owned(),
        draft_id: event.data.draft_id.to_owned(),
        most_usd_micros: event.data.most_usd_micros,
        held_at: event.timestamp,
    });
    Ok(())
}

#[evento::handler]
async fn on_charged(event: Event<Charged>, day: &mut SpendDayState) -> anyhow::Result<()> {
    day.holds.retain(|hold| hold.id != event.data.hold);
    let shop = day
        .shops
        .entry(event.data.tenant_id.to_owned())
        .or_default();
    shop.usd_micros = shop.usd_micros.saturating_add(event.data.cost_usd_micros);
    shop.calls = shop.calls.saturating_add(1);
    Ok(())
}

#[evento::handler]
async fn on_released(event: Event<Released>, day: &mut SpendDayState) -> anyhow::Result<()> {
    day.holds.retain(|hold| hold.id != event.data.hold);
    Ok(())
}

/// A call room is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Call<'a> {
    /// The shop whose cap it counts against.
    pub tenant_id: &'a str,
    pub draft_id: &'a str,
    /// The most it may cost, in millionths of a US dollar.
    pub most_usd_micros: u64,
}

/// Room held on a day for one call: what its charge or its release names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hold {
    /// The day the room was held on, and the call counts on.
    pub day: String,
    pub id: String,
}

/// What asking for room gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Room {
    /// The call may be made, then charged or released.
    Held(Hold),
    /// The call would pass `cap`, which frees at `frees_at` (unix seconds).
    Refused { cap: SpendCap, frees_at: u64 },
}

/// Whether a decision writes, and what it gives either way.
enum Step<T> {
    Leave(T),
    Write(T),
}

fn empty_day(day: &str) -> SpendDayState {
    SpendDayState {
        id: spend_day_id(day),
        ..SpendDayState::default()
    }
}

impl<E: Executor> Spend<E> {
    /// A day (`2026-11-24`) of spending as it stands; an empty one before
    /// its first call.
    pub async fn load_spend_day(&self, day: &str) -> anyhow::Result<SpendDayState> {
        Ok(spend_day_projection()
            .load(spend_day_id(day))
            .execute(&self.0)
            .await?
            .unwrap_or_else(|| empty_day(day)))
    }

    /// Loads the day, lets `decide` add its events to a write of it, and
    /// commits; decided again from a fresh load when another write came in
    /// between.
    async fn settle_day<T>(
        &self,
        day: &str,
        routing_key: Option<String>,
        mut decide: impl FnMut(&SpendDayState, &mut WriteBuilder) -> Step<T> + Send,
    ) -> Result<T, WriterError> {
        for _ in 0..SPEND_WRITE_ATTEMPTS {
            let loaded = spend_day_projection()
                .load(spend_day_id(day))
                .execute(&self.0)
                .await?;
            let (state, mut write) = match loaded {
                Some(state) => {
                    let write = state.write()?;
                    (state, write)
                }
                None => {
                    let mut write = evento::append(spend_day_id(day));
                    write.original_version(0);
                    (empty_day(day), write)
                }
            };
            write.routing_key_opt(routing_key.clone());
            let value = match decide(&state, &mut write) {
                Step::Leave(value) => return Ok(value),
                Step::Write(value) => value,
            };
            match write.commit(&self.0).await {
                Ok(_) => return Ok(value),
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Err(WriteError::InvalidOriginalVersion.into())
    }
}

#[evento::command]
impl<E: Executor> Spend<E> {
    /// Holds room on the day of `now` (unix seconds) for `call`; refused
    /// when it would pass one of `caps`.
    pub async fn hold_spend(
        &self,
        actor: &str,
        call: Call<'_>,
        caps: SpendCaps,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<Room, WriterError> {
        let day = spend_day(now);
        let id = ulid::Ulid::generate().to_string();
        self.settle_day(&day, routing_key, |state, write| {
            if let Some(cap) = state.refusal(caps, call.tenant_id, call.most_usd_micros, now) {
                return Step::Leave(Room::Refused {
                    cap,
                    frees_at: spend_day_ends(now),
                });
            }
            write.event(&Held {
                hold: id.clone(),
                tenant_id: call.tenant_id.to_owned(),
                draft_id: call.draft_id.to_owned(),
                most_usd_micros: call.most_usd_micros,
                held_by: actor.to_owned(),
            });
            Step::Write(Room::Held(Hold {
                day: day.clone(),
                id: id.clone(),
            }))
        })
        .await
    }

    /// Charges the call `hold` kept room for with what it cost, on the day
    /// the room was held; `false` when the hold is settled already.
    pub async fn charge_spend(
        &self,
        actor: &str,
        hold: &Hold,
        cost_usd_micros: u64,
        routing_key: Option<String>,
    ) -> Result<bool, WriterError> {
        self.settle_day(&hold.day, routing_key, |state, write| {
            let Some(held) = state.hold(&hold.id) else {
                return Step::Leave(false);
            };
            write.event(&Charged {
                hold: held.id.clone(),
                tenant_id: held.tenant_id.clone(),
                draft_id: held.draft_id.clone(),
                cost_usd_micros,
                charged_by: actor.to_owned(),
            });
            Step::Write(true)
        })
        .await
    }

    /// Frees the room of a call that gave nothing and cost nothing;
    /// `false` when the hold is settled already.
    pub async fn release_spend(
        &self,
        actor: &str,
        hold: &Hold,
        routing_key: Option<String>,
    ) -> Result<bool, WriterError> {
        self.settle_day(&hold.day, routing_key, |state, write| {
            if state.hold(&hold.id).is_none() {
                return Step::Leave(false);
            }
            write.event(&Released {
                hold: hold.id.clone(),
                released_by: actor.to_owned(),
            });
            Step::Write(true)
        })
        .await
    }
}
