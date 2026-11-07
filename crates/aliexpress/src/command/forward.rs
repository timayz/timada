//! Write side of placing a shop's paid orders on AliExpress, and of reading
//! them again until they ship.

use evento::{Executor, Projection, ProjectionAggregate, WriteError, metadata::Event};

use super::claim_ae_order;
use crate::aggregator::forward::{
    AlsoPlaced, Asking, Checked, Done, Failed, Forward, ForwardItem, Placed, Requested, Started,
};
use crate::error::AliexpressError;
use crate::value_object::{FORWARD_STALL_SECS, forward_id};

/// Where placing an order on AliExpress stands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ForwardStatus {
    /// Asked for, not taken on yet.
    #[default]
    Requested,
    /// A node took it on and has not said how it went: if it stopped, the
    /// order may or may not be on AliExpress.
    Started,
    /// AliExpress took it; read again until it ships.
    Placed,
    /// Nothing was placed.
    Failed,
    /// No longer read again.
    Done,
}

impl ForwardStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Started => "started",
            Self::Placed => "placed",
            Self::Failed => "failed",
            Self::Done => "done",
        }
    }
}

/// One AliExpress order of a forward, as last read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AeOrderState {
    pub id: String,
    /// Empty until read.
    pub status: String,
    pub logistics_status: String,
    pub tracking: Vec<String>,
    /// AliExpress closed it without shipping it.
    pub closed: bool,
}

/// A shop's order on AliExpress.
#[evento::projection(id = id, name = "timada-aliexpress/ForwardState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct ForwardState {
    pub id: String,
    pub order_id: String,
    pub status: ForwardStatus,
    pub requested_by: String,
    /// Unix seconds of the last `Requested`.
    pub requested_at: u64,
    pub items: Vec<ForwardItem>,
    pub ae_orders: Vec<AeOrderState>,
    /// AliExpress was asked to charge the account.
    pub pay_requested: bool,
    /// Every line of the shop's order was bought on AliExpress.
    pub complete: bool,
    /// Unix seconds AliExpress took it.
    pub placed_at: Option<u64>,
    /// Why the last attempt placed nothing.
    pub last_error: Option<String>,
    /// The tracking the shop's order was fulfilled with, if it was.
    pub fulfilled_tracking: Option<String>,
    /// AliExpress orders of an attempt that overlapped the one recorded: the
    /// order was bought twice, and these are to cancel there.
    pub also_placed: Vec<String>,
    /// Unix seconds of the last event.
    pub updated_at: u64,
}

impl ForwardState {
    /// Every parcel number AliExpress gave, in order.
    pub fn tracking(&self) -> Vec<String> {
        let mut all: Vec<String> = Vec::new();
        for number in self.ae_orders.iter().flat_map(|order| &order.tracking) {
            if !all.contains(number) {
                all.push(number.clone());
            }
        }
        all
    }

    /// Whether every AliExpress order has a parcel number.
    pub fn all_shipped(&self) -> bool {
        !self.ae_orders.is_empty()
            && self
                .ae_orders
                .iter()
                .all(|order| !order.tracking.is_empty())
    }

    /// Whether AliExpress closed every one of its orders, shipping none.
    pub fn all_closed(&self) -> bool {
        !self.ae_orders.is_empty() && self.ae_orders.iter().all(|order| order.closed)
    }
}

pub fn forward_projection<E: Executor>() -> Projection<E, ForwardState> {
    Projection::new::<Forward>()
        .handler(on_requested())
        .handler(on_started())
        .handler(on_asking())
        .handler(on_placed())
        .handler(on_also_placed())
        .handler(on_failed())
        .handler(on_checked())
        .handler(on_done())
        .strict()
}

#[evento::handler]
async fn on_asking(event: Event<Asking>, forward: &mut ForwardState) -> anyhow::Result<()> {
    forward.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_also_placed(
    event: Event<AlsoPlaced>,
    forward: &mut ForwardState,
) -> anyhow::Result<()> {
    for id in &event.data.ae_order_ids {
        if !forward.also_placed.contains(id) {
            forward.also_placed.push(id.to_owned());
        }
    }
    forward.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_requested(event: Event<Requested>, forward: &mut ForwardState) -> anyhow::Result<()> {
    forward.id = event.aggregate_id.to_owned();
    forward.order_id = event.data.order_id.to_owned();
    forward.status = ForwardStatus::Requested;
    forward.requested_by = event.data.requested_by.to_owned();
    forward.requested_at = event.timestamp;
    forward.last_error = None;
    forward.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_started(event: Event<Started>, forward: &mut ForwardState) -> anyhow::Result<()> {
    forward.status = ForwardStatus::Started;
    forward.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_placed(event: Event<Placed>, forward: &mut ForwardState) -> anyhow::Result<()> {
    forward.status = ForwardStatus::Placed;
    forward.items = event.data.items.to_owned();
    forward.ae_orders = event
        .data
        .ae_order_ids
        .iter()
        .map(|id| AeOrderState {
            id: id.to_owned(),
            ..Default::default()
        })
        .collect();
    forward.pay_requested = event.data.pay_requested;
    forward.complete = event.data.complete;
    forward.placed_at = Some(event.timestamp);
    forward.last_error = None;
    forward.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_failed(event: Event<Failed>, forward: &mut ForwardState) -> anyhow::Result<()> {
    forward.status = ForwardStatus::Failed;
    forward.last_error = Some(event.data.reason.to_owned());
    forward.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_checked(event: Event<Checked>, forward: &mut ForwardState) -> anyhow::Result<()> {
    let data = &event.data;
    if let Some(order) = forward
        .ae_orders
        .iter_mut()
        .find(|order| order.id == data.ae_order_id)
    {
        order.status = data.status.to_owned();
        order.logistics_status = data.logistics_status.to_owned();
        order.tracking = data.tracking.to_owned();
        order.closed = data.closed;
    }
    forward.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_done(event: Event<Done>, forward: &mut ForwardState) -> anyhow::Result<()> {
    forward.status = ForwardStatus::Done;
    if event.data.fulfilled {
        forward.fulfilled_tracking = event.data.tracking.to_owned();
    }
    forward.updated_at = event.timestamp;
    Ok(())
}

/// What a placement recorded.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Placement {
    pub ae_order_ids: Vec<String>,
    pub items: Vec<ForwardItem>,
    pub pay_requested: bool,
    pub complete: bool,
}

/// What recording the orders AliExpress took came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlacedRecord {
    /// The forward is placed as these orders (now, or already).
    Placed,
    /// The forward was placed as other orders, by an attempt that overlapped
    /// this one: these are recorded beside them (`AlsoPlaced`), for the
    /// merchant to cancel on AliExpress (now, or already).
    AlsoPlaced,
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Asks for `order_id` to be placed on AliExpress; the forward's id. A
    /// placed one is `AlreadyForwarded`. One asked for or taken on is
    /// `Forwarding` unless `again`: a node that stopped half-way leaves no
    /// word of whether AliExpress took it, so only the merchant, having
    /// looked there, asks again. A failed one is asked for again.
    pub async fn request_forward(
        &self,
        actor: &str,
        order_id: &str,
        again: bool,
        routing_key: Option<String>,
    ) -> Result<String, AliexpressError> {
        let requested = Requested {
            order_id: order_id.to_owned(),
            requested_by: actor.to_owned(),
        };
        let id = forward_id(order_id);
        let lost = |err: WriteError| match err {
            WriteError::InvalidOriginalVersion => AliexpressError::Forwarding,
            other => other.into(),
        };
        match self.load_forward(order_id).await? {
            None => {
                evento::append(&id)
                    .original_version(0)
                    .routing_key_opt(routing_key)
                    .event(&requested)
                    .tenant(&self.tenant)
                    .commit(&self.executor)
                    .await
                    .map_err(lost)?;
            }
            Some(forward) => {
                match forward.status {
                    ForwardStatus::Placed | ForwardStatus::Done => {
                        return Err(AliexpressError::AlreadyForwarded);
                    }
                    ForwardStatus::Requested | ForwardStatus::Started if !again => {
                        return Err(AliexpressError::Forwarding);
                    }
                    _ => {}
                }
                forward
                    .write()?
                    .routing_key_opt(routing_key)
                    .event(&requested)
                    .tenant(&self.tenant)
                    .commit(&self.executor)
                    .await
                    .map_err(lost)?;
            }
        }
        Ok(id)
    }

    /// Takes a requested forward on; `false` when another node did first
    /// (or it is no longer to place).
    pub async fn start_forward(
        &self,
        actor: &str,
        forward_id: &str,
        routing_key: Option<String>,
    ) -> Result<bool, AliexpressError> {
        let Some(forward) = self.load_forward_by_id(forward_id).await? else {
            return Ok(false);
        };
        if forward.status != ForwardStatus::Requested {
            return Ok(false);
        }
        let written = forward
            .write()?
            .routing_key_opt(routing_key)
            .event(&Started {
                started_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await;
        match written {
            Ok(_) => Ok(true),
            Err(WriteError::InvalidOriginalVersion) => Ok(false),
            Err(err) => Err(err.into()),
        }
    }

    /// Fences the forward right before AliExpress is asked for it: written
    /// at `version`, the one the attempt took it on at, so an attempt
    /// overtaken in between (asked for again, given up as stalled, placed by
    /// another) asks nothing. Whether this attempt may ask.
    pub async fn begin_asking(
        &self,
        actor: &str,
        forward_id: &str,
        version: u16,
        routing_key: Option<String>,
    ) -> Result<bool, AliexpressError> {
        let written = evento::append(forward_id)
            .original_version(version)
            .routing_key_opt(routing_key)
            .event(&Asking {
                asked_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await;
        match written {
            Ok(_) => Ok(true),
            Err(WriteError::InvalidOriginalVersion) => Ok(false),
            Err(err) => Err(err.into()),
        }
    }

    /// Records the orders AliExpress took, then each in the platform's
    /// streams ([`claim_ae_order`]), so a push naming one finds the forward.
    /// AliExpress has them whatever the forward says meanwhile: another write
    /// in between is read again and written over, and a forward given up as
    /// stalled is placed after all. Recorded already (a retry after a write
    /// that landed), only what is left of the platform's record is written.
    /// A forward placed as other orders, by an attempt that overlapped this
    /// one, gets these beside them ([`PlacedRecord::AlsoPlaced`]): AliExpress
    /// bought it twice, and the merchant cancels these there. An order the
    /// platform holds for another forward refuses its record
    /// (`ClaimedElsewhere`), after the forward and the others are recorded.
    pub async fn record_placed(
        &self,
        actor: &str,
        forward_id: &str,
        placement: Placement,
        routing_key: Option<String>,
    ) -> Result<PlacedRecord, AliexpressError> {
        let placed = Placed {
            ae_order_ids: placement.ae_order_ids,
            items: placement.items,
            pay_requested: placement.pay_requested,
            complete: placement.complete,
            placed_by: actor.to_owned(),
        };
        let mut record = PlacedRecord::Placed;
        for attempt in 1..=super::WRITE_ATTEMPTS {
            let forward = self.require_forward(forward_id).await?;
            let mut write = forward.write()?;
            write.routing_key_opt(routing_key.clone());
            if matches!(forward.status, ForwardStatus::Placed | ForwardStatus::Done) {
                let same = forward
                    .ae_orders
                    .iter()
                    .map(|order| &order.id)
                    .eq(placed.ae_order_ids.iter());
                if same {
                    break;
                }
                record = PlacedRecord::AlsoPlaced;
                let known = placed
                    .ae_order_ids
                    .iter()
                    .all(|id| forward.also_placed.contains(id));
                if known {
                    break;
                }
                write.event(&AlsoPlaced {
                    ae_order_ids: placed.ae_order_ids.clone(),
                    placed_by: actor.to_owned(),
                });
            } else {
                write.event(&placed);
            }
            match write.tenant(&self.tenant).commit(&self.executor).await {
                Ok(_) => break,
                Err(WriteError::InvalidOriginalVersion) if attempt < super::WRITE_ATTEMPTS => {
                    record = PlacedRecord::Placed;
                }
                Err(err) => return Err(err.into()),
            }
        }
        let mut elsewhere = None;
        for ae_order_id in &placed.ae_order_ids {
            match claim_ae_order(&self.executor, ae_order_id, &self.tenant, forward_id).await {
                Ok(()) => {}
                Err(err @ AliexpressError::ClaimedElsewhere { .. }) => elsewhere = Some(err),
                Err(err) => return Err(err),
            }
        }
        match elsewhere {
            Some(err) => Err(err),
            None => Ok(record),
        }
    }

    /// Says AliExpress may have the order of a forward taken on and not
    /// heard of for [`FORWARD_STALL_SECS`] before `now` (unix seconds): the
    /// node placing it stopped, or could not record what AliExpress
    /// answered. `reason` is what the merchant reads, who looks there before
    /// placing it again. Whether it was said: `false` for a forward that
    /// moved on meanwhile, or not stalled yet.
    pub async fn fail_stalled_forward(
        &self,
        actor: &str,
        forward_id: &str,
        reason: &str,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<bool, AliexpressError> {
        let Some(forward) = self.load_forward_by_id(forward_id).await? else {
            return Ok(false);
        };
        let stalled = forward.status == ForwardStatus::Started
            && now.saturating_sub(forward.updated_at) > FORWARD_STALL_SECS;
        if !stalled {
            return Ok(false);
        }
        let written = forward
            .write()?
            .routing_key_opt(routing_key)
            .event(&Failed {
                reason: reason.chars().take(500).collect(),
                failed_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await;
        match written {
            Ok(_) => Ok(true),
            Err(WriteError::InvalidOriginalVersion) => Ok(false),
            Err(err) => Err(err.into()),
        }
    }

    /// Records why nothing was placed.
    pub async fn record_forward_failure(
        &self,
        actor: &str,
        forward_id: &str,
        reason: &str,
        routing_key: Option<String>,
    ) -> Result<(), AliexpressError> {
        let forward = self.require_forward(forward_id).await?;
        forward
            .write()?
            .routing_key_opt(routing_key)
            .event(&Failed {
                reason: reason.chars().take(500).collect(),
                failed_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Records what reading the forward's AliExpress orders again changed
    /// (`checked`: one per order read), then `done` when it is no longer to
    /// read; written against `version`, the one the read started from, so
    /// another node's read in between wins (`Write(InvalidOriginalVersion)`).
    /// Returns the version the forward is at afterwards.
    pub async fn record_checked(
        &self,
        forward_id: &str,
        version: u16,
        checked: Vec<Checked>,
        done: Option<Done>,
        routing_key: Option<String>,
    ) -> Result<u16, AliexpressError> {
        if checked.is_empty() && done.is_none() {
            return Ok(version);
        }
        let mut write = evento::append(forward_id);
        write.original_version(version).routing_key_opt(routing_key);
        for read in &checked {
            write.event(read);
        }
        if let Some(done) = &done {
            write.event(done);
        }
        write.tenant(&self.tenant).commit(&self.executor).await?;
        Ok(version + checked.len() as u16 + u16::from(done.is_some()))
    }
}

impl<E: Executor> super::Command<E> {
    /// The forward of a shop's order, if it was ever asked for.
    pub async fn load_forward(&self, order_id: &str) -> anyhow::Result<Option<ForwardState>> {
        self.load_forward_by_id(&forward_id(order_id)).await
    }

    pub async fn load_forward_by_id(
        &self,
        forward_id: &str,
    ) -> anyhow::Result<Option<ForwardState>> {
        forward_projection()
            .load(forward_id)
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    /// The forward and the version it was read at.
    pub async fn load_forward_versioned(
        &self,
        forward_id: &str,
    ) -> anyhow::Result<Option<(ForwardState, u16)>> {
        let Some(forward) = self.load_forward_by_id(forward_id).await? else {
            return Ok(None);
        };
        let version = forward.aggregate_version()?;
        Ok(Some((forward, version)))
    }

    async fn require_forward(&self, forward_id: &str) -> Result<ForwardState, AliexpressError> {
        self.load_forward_by_id(forward_id)
            .await?
            .ok_or(AliexpressError::NotFound)
    }
}
