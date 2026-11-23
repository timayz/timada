//! What the keyword source is paid, and the day's caps on it.
//!
//! Every call is charged to the platform's account, whoever asks, so a day
//! (UTC) has two caps: one per user, one for everyone together. They count
//! what the source says each call cost, which is only known once it
//! answered. So room is held for a call before it is made, at the most it
//! may cost; a call the caps have no room for is refused before the source
//! is asked; and the charge that follows puts the real cost in the hold's
//! place. A cap is therefore never passed, and the last calls before it are
//! refused while a little of it is left.
//!
//! A day is one stream, written against the version it was read at: two
//! nodes holding room at once cannot both take the last of it, so the count
//! holds over a cluster. A node that held room and stopped before charging
//! leaves the hold there; it counts for
//! [`FETCH_STALE_SECS`](crate::FETCH_STALE_SECS), as long as its fetch may
//! still be on its way.

use std::collections::BTreeMap;

use evento::{
    Executor, Projection, ProjectionAggregate, WriteBuilder, WriteError, metadata::Event,
};
use timada_core::unix_now;

use crate::aggregator::{Charged, Held, Released, SpendDay};
use crate::error::ResearchError;
use crate::value_object::{
    FETCH_STALE_SECS, MAX_DAY_HOLDS, SpendCap, SpendCaps, spend_day, spend_day_ends,
};

use super::{Command, Step};

/// How many times a write to the day decides again when another came in
/// between: every paid call of the platform writes the one stream.
const SPEND_WRITE_ATTEMPTS: usize = 12;

/// Room held for a call that is not charged yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoldState {
    pub id: String,
    pub user_id: String,
    pub study_id: String,
    pub seed: String,
    pub most_usd_micros: u64,
    /// Unix seconds of `Held`.
    pub held_at: u64,
}

impl HoldState {
    /// Whether the hold still counts at `now`: its call may be on its way.
    fn counts(&self, now: u64) -> bool {
        now.saturating_sub(self.held_at) < FETCH_STALE_SECS
    }
}

/// What one user's calls cost on a day.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UserSpend {
    pub usd_micros: u64,
    /// How many calls were charged.
    pub calls: u32,
}

/// The write model of a day of spending.
#[evento::projection(id = id, name = "timada-research/SpendDayState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct SpendDayState {
    /// The day, as `2026-11-23`.
    pub id: String,
    /// What was charged, by user.
    pub users: BTreeMap<String, UserSpend>,
    /// The holds neither charged nor released.
    pub holds: Vec<HoldState>,
    /// How many holds the day took in all.
    pub held: u32,
}

impl SpendDayState {
    /// What `user_id`'s calls were charged on the day.
    pub fn charged_to(&self, user_id: &str) -> u64 {
        self.users.get(user_id).map_or(0, |user| user.usd_micros)
    }

    /// What every call of the day was charged.
    pub fn charged(&self) -> u64 {
        self.users
            .values()
            .fold(0, |sum, user| sum.saturating_add(user.usd_micros))
    }

    /// What is held at `now` for the calls on their way, for one user or
    /// for all.
    fn held_at(&self, user_id: Option<&str>, now: u64) -> u64 {
        self.holds
            .iter()
            .filter(|hold| hold.counts(now))
            .filter(|hold| user_id.is_none_or(|user_id| hold.user_id == user_id))
            .fold(0, |sum, hold| sum.saturating_add(hold.most_usd_micros))
    }

    /// The cap a call for `user_id` that may cost `most_usd_micros` would
    /// pass at `now`, the user's own first; `None` when there is room.
    pub fn refusal(
        &self,
        caps: SpendCaps,
        user_id: &str,
        most_usd_micros: u64,
        now: u64,
    ) -> Option<SpendCap> {
        let user = self
            .charged_to(user_id)
            .saturating_add(self.held_at(Some(user_id), now))
            .saturating_add(most_usd_micros);
        if user > caps.user_usd_micros {
            return Some(SpendCap::User);
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
        user_id: event.data.user_id.to_owned(),
        study_id: event.data.study_id.to_owned(),
        seed: event.data.seed.to_owned(),
        most_usd_micros: event.data.most_usd_micros,
        held_at: event.timestamp,
    });
    Ok(())
}

#[evento::handler]
async fn on_charged(event: Event<Charged>, day: &mut SpendDayState) -> anyhow::Result<()> {
    day.holds.retain(|hold| hold.id != event.data.hold);
    let user = day.users.entry(event.data.user_id.to_owned()).or_default();
    user.usd_micros = user.usd_micros.saturating_add(event.data.cost_usd_micros);
    user.calls = user.calls.saturating_add(1);
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
    /// Who asked for it: the user whose cap it counts against.
    pub user_id: &'a str,
    pub study_id: &'a str,
    pub seed: &'a str,
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

impl<E: Executor> Command<E> {
    /// A day of spending as it stands; an empty one before its first call.
    pub async fn load_spend_day(&self, day: &str) -> anyhow::Result<SpendDayState> {
        Ok(spend_day_projection()
            .load(day)
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
        mut decide: impl FnMut(&SpendDayState, &mut WriteBuilder) -> Result<Step<T>, ResearchError>
        + Send,
    ) -> Result<T, ResearchError> {
        for _ in 0..SPEND_WRITE_ATTEMPTS {
            let loaded = spend_day_projection().load(day).execute(&self.0).await?;
            let (state, mut write) = match loaded {
                Some(state) => {
                    let write = state.write()?;
                    (state, write)
                }
                None => {
                    let mut write = evento::append(day);
                    write.original_version(0);
                    (empty_day(day), write)
                }
            };
            write.routing_key_opt(routing_key.clone());
            let value = match decide(&state, &mut write)? {
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

fn empty_day(day: &str) -> SpendDayState {
    SpendDayState {
        id: day.to_owned(),
        ..SpendDayState::default()
    }
}

#[evento::command]
impl<E: Executor> Command<E> {
    /// Holds room today for `call`; refused when it would pass one of
    /// `caps`.
    pub async fn hold_spend(
        &self,
        actor: &str,
        call: Call<'_>,
        caps: SpendCaps,
        routing_key: Option<String>,
    ) -> Result<Room, ResearchError> {
        let now = unix_now();
        let day = spend_day(now);
        let id = ulid::Ulid::generate().to_string();
        self.settle_day(&day, routing_key, |state, write| {
            if let Some(cap) = state.refusal(caps, call.user_id, call.most_usd_micros, now) {
                return Ok(Step::Leave(Room::Refused {
                    cap,
                    frees_at: spend_day_ends(now),
                }));
            }
            write.event(&Held {
                hold: id.clone(),
                user_id: call.user_id.to_owned(),
                study_id: call.study_id.to_owned(),
                seed: call.seed.to_owned(),
                most_usd_micros: call.most_usd_micros,
                held_by: actor.to_owned(),
            });
            Ok(Step::Write(Room::Held(Hold {
                day: day.clone(),
                id: id.clone(),
            })))
        })
        .await
    }

    /// Charges the call `hold` kept room for with what the source says it
    /// cost, on the day the room was held; `false` when the hold is settled
    /// already.
    pub async fn charge_spend(
        &self,
        actor: &str,
        hold: &Hold,
        cost_usd_micros: u64,
        routing_key: Option<String>,
    ) -> Result<bool, ResearchError> {
        self.settle_day(&hold.day, routing_key, |state, write| {
            let Some(held) = state.hold(&hold.id) else {
                return Ok(Step::Leave(false));
            };
            write.event(&Charged {
                hold: held.id.clone(),
                user_id: held.user_id.clone(),
                study_id: held.study_id.clone(),
                seed: held.seed.clone(),
                cost_usd_micros,
                charged_by: actor.to_owned(),
            });
            Ok(Step::Write(true))
        })
        .await
    }

    /// Frees the room of a call that gave nothing and cost nothing; `false`
    /// when the hold is settled already.
    pub async fn release_spend(
        &self,
        actor: &str,
        hold: &Hold,
        routing_key: Option<String>,
    ) -> Result<bool, ResearchError> {
        self.settle_day(&hold.day, routing_key, |state, write| {
            if state.hold(&hold.id).is_none() {
                return Ok(Step::Leave(false));
            }
            write.event(&Released {
                hold: hold.id.clone(),
                released_by: actor.to_owned(),
            });
            Ok(Step::Write(true))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAPS: SpendCaps = SpendCaps {
        user_usd_micros: 300_000,
        platform_usd_micros: 500_000,
    };
    const MOST: u64 = 132_000;
    const NOW: u64 = 1_700_000_000;

    fn hold(id: &str, user_id: &str, held_at: u64) -> HoldState {
        HoldState {
            id: id.to_owned(),
            user_id: user_id.to_owned(),
            study_id: "study".to_owned(),
            seed: "griffoir".to_owned(),
            most_usd_micros: MOST,
            held_at,
        }
    }

    fn charged(day: &mut SpendDayState, user_id: &str, usd_micros: u64) {
        let user = day.users.entry(user_id.to_owned()).or_default();
        user.usd_micros += usd_micros;
        user.calls += 1;
    }

    #[test]
    fn a_call_is_refused_when_its_most_would_pass_a_cap() {
        let mut day = empty_day("2023-11-14");
        assert_eq!(day.refusal(CAPS, "alice", MOST, NOW), None);
        // 168,000 charged leaves room for exactly one call more.
        charged(&mut day, "alice", 168_000);
        assert_eq!(day.refusal(CAPS, "alice", MOST, NOW), None);
        charged(&mut day, "alice", 1);
        assert_eq!(day.refusal(CAPS, "alice", MOST, NOW), Some(SpendCap::User));
        // Another user has their own cap, within the platform's.
        assert_eq!(day.refusal(CAPS, "bob", MOST, NOW), None);
        charged(&mut day, "bob", 200_000);
        assert_eq!(
            day.refusal(CAPS, "carol", MOST, NOW),
            Some(SpendCap::Platform)
        );
        // A cap of nothing lets no call through.
        let none = SpendCaps {
            user_usd_micros: 0,
            ..CAPS
        };
        assert_eq!(
            empty_day("2023-11-14").refusal(none, "alice", MOST, NOW),
            Some(SpendCap::User)
        );
    }

    #[test]
    fn a_call_on_its_way_counts_at_its_most_until_its_fetch_is_stale() {
        let mut day = empty_day("2023-11-14");
        day.holds.push(hold("h1", "alice", NOW));
        day.holds.push(hold("h2", "alice", NOW));
        // Two on their way hold 264,000 of Alice's 300,000.
        assert_eq!(
            day.refusal(CAPS, "alice", MOST, NOW + 1),
            Some(SpendCap::User)
        );
        assert_eq!(day.refusal(CAPS, "bob", MOST, NOW + 1), None);
        day.holds.push(hold("h3", "bob", NOW));
        assert_eq!(
            day.refusal(CAPS, "carol", MOST, NOW + 1),
            Some(SpendCap::Platform)
        );
        // The nodes that held them stopped: the room is free again.
        let later = NOW + FETCH_STALE_SECS;
        assert_eq!(day.refusal(CAPS, "alice", MOST, later), None);
        assert_eq!(day.refusal(CAPS, "carol", MOST, later), None);
    }

    #[test]
    fn a_day_holds_room_for_so_many_calls_whatever_the_caps() {
        let caps = SpendCaps {
            user_usd_micros: u64::MAX,
            platform_usd_micros: u64::MAX,
        };
        let mut day = empty_day("2023-11-14");
        day.held = MAX_DAY_HOLDS - 1;
        assert_eq!(day.refusal(caps, "alice", MOST, NOW), None);
        day.held = MAX_DAY_HOLDS;
        assert_eq!(
            day.refusal(caps, "alice", MOST, NOW),
            Some(SpendCap::Platform)
        );
    }
}
