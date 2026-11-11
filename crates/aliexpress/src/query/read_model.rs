//! The subscription that keeps `aliexpress_products` and
//! `aliexpress_forwards` current for every tenant, and the queries over
//! them. One subscription follows every tenant's streams (its cursor lives
//! in the store); each row carries the tenant of the event that made it. It
//! also follows what makes an imported product matter (shoppers wanting it, the
//! merchant selling it), which decides what the budget of reads goes to first.
//! When this node last read an item or an order is no event's to tell: the
//! queries join the node's own record of it ([`super::read_times`]).

use evento::{
    Executor,
    metadata::{Event, RawEvent},
    subscription::{Context, SubscriptionBuilder},
};
use timada_catalog::aggregator::{Published, Unpublished};
use timada_core::db::{Db, FromRow, Value, params_from_iter};
use timada_orders::OrderLine;
use timada_orders::aggregator::{ItemAdded, Placed};

use crate::aggregator::forward::Forward;
use crate::aggregator::{AliexpressProduct, SkuLink};
use crate::command::{
    ForwardState, ForwardStatus, LinkState, LinkStatus, forward_projection, link_projection,
};
use crate::forwarding::{ForwardPlan, LinkedSkus, plan_items};
use crate::value_object::{Share, shard_of};

/// The cursor identity of the read-model subscription, in the store.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-aliexpress/read-models";

/// Builds the subscription over every tenant and routing key. `db` is the
/// read-model database and must be allowed to write.
pub fn read_model_subscription<E: Executor>(db: Db) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(db)
        .handler(on_link_event())
        .handler(on_forward_event())
        .handler(on_item_added())
        .handler(on_order_placed())
        .handler(on_product_published())
        .handler(on_product_unpublished())
        .any_routing_key()
}

#[evento::subscription_all]
async fn on_link_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<AliexpressProduct>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(link) = link_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_link(&db, &event.tenant, &link).await
}

#[evento::subscription_all]
async fn on_forward_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Forward>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(forward) = forward_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_forward(&db, &event.tenant, &forward).await
}

#[evento::subscription]
async fn on_item_added<E: Executor>(
    context: &Context<'_, E>,
    event: Event<ItemAdded>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    note_wanted(&db, &event.tenant, &event.data.product_id, event.timestamp).await
}

#[evento::subscription]
async fn on_order_placed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Placed>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    for line in &event.data.lines {
        note_wanted(&db, &event.tenant, &line.product_id, event.timestamp).await?;
    }
    Ok(())
}

#[evento::subscription]
async fn on_product_published<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Published>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    note_published(&db, &event.tenant, &event.aggregate_id, true).await
}

#[evento::subscription]
async fn on_product_unpublished<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Unpublished>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    note_published(&db, &event.tenant, &event.aggregate_id, false).await
}

/// A shopper put the catalog product in a cart or bought it at `at` (unix
/// seconds). Most products are no import of any shop: this then changes nothing.
/// Public for the integration tests, which seed rows; only the subscription
/// writes one otherwise.
pub async fn note_wanted(
    db: &Db,
    tenant_id: &str,
    product_id: &str,
    at: u64,
) -> anyhow::Result<()> {
    db.execute(
        "UPDATE aliexpress_products SET wanted_at = MAX(COALESCE(wanted_at, 0), ?3)
         WHERE tenant_id = ?1 AND product_id = ?2",
        (tenant_id, product_id, at as i64),
    )
    .await?;
    Ok(())
}

/// The catalog product went on or off sale. Public for the integration
/// tests, which seed rows; only the subscription writes one otherwise.
pub async fn note_published(
    db: &Db,
    tenant_id: &str,
    product_id: &str,
    published: bool,
) -> anyhow::Result<()> {
    db.execute(
        "UPDATE aliexpress_products SET published = ?3
         WHERE tenant_id = ?1 AND product_id = ?2",
        (tenant_id, product_id, published),
    )
    .await?;
    Ok(())
}

/// Writes the row of a forward as the write model sees it. When this node
/// last read it is not in the row: a read that changed nothing records no
/// event, so the node keeps that for itself ([`super::read_times::note_forward_read`])
/// and drops it once the order is no longer read again (done, or nothing
/// placed). The integration tests seed rows through this; nothing else
/// outside the subscription writes one.
pub async fn upsert_forward(
    db: &Db,
    tenant_id: &str,
    forward: &ForwardState,
) -> anyhow::Result<()> {
    let ae_order_ids = forward
        .ae_orders
        .iter()
        .map(|order| order.id.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let state = serde_json::to_string(forward)?;
    db.execute(
        "INSERT INTO aliexpress_forwards (tenant_id, id, order_id, status, ae_order_ids,
            tracking, last_error, placed_at, updated_at, shard, settling, state)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            status = excluded.status, ae_order_ids = excluded.ae_order_ids,
            tracking = excluded.tracking, last_error = excluded.last_error,
            placed_at = excluded.placed_at, updated_at = excluded.updated_at,
            settling = excluded.settling, state = excluded.state",
        (
            tenant_id,
            forward.id.as_str(),
            forward.order_id.as_str(),
            forward.status.as_str(),
            ae_order_ids,
            forward.tracking().join(" "),
            forward.last_error.as_deref(),
            forward.placed_at.map(|at| at as i64),
            forward.updated_at as i64,
            i64::from(shard_of(&forward.id)),
            forward.complete && forward.all_shipped(),
            state.as_str(),
        ),
    )
    .await?;
    if matches!(forward.status, ForwardStatus::Done | ForwardStatus::Failed) {
        super::read_times::forget(db, tenant_id, &forward.id).await?;
    }
    // Its parcels are followed from `Done` on; the first follow may well be
    // noted before this row hears of the closing.
    if forward.status == ForwardStatus::Failed {
        super::read_times::forget(
            db,
            tenant_id,
            &super::read_times::parcels_read_id(&forward.id),
        )
        .await?;
    }
    Ok(())
}

/// The forward of a shop's order as the read model has it, whole, or `None`
/// while none was asked for. An order has one forward at most (its id is
/// made of the order's, [`forward_id`](crate::forward_id)). A moment behind
/// its stream, for the order's page; what acts on a forward loads its stream.
pub async fn forward_of_order(
    db: &Db,
    tenant_id: &str,
    order_id: &str,
) -> anyhow::Result<Option<ForwardState>> {
    let row: Option<(String,)> = db
        .fetch_optional(
            "SELECT state FROM aliexpress_forwards WHERE tenant_id = ?1 AND order_id = ?2",
            (tenant_id, order_id),
        )
        .await?;
    row.map(|(state,)| serde_json::from_str(&state).map_err(anyhow::Error::from))
        .transpose()
}

/// One row of `aliexpress_forwards`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct ForwardRow {
    pub tenant_id: String,
    pub id: String,
    pub order_id: String,
    /// `requested`, `started`, `placed`, `failed` or `done`.
    pub status: String,
    /// Space-separated.
    pub ae_order_ids: String,
    /// Space-separated parcel numbers.
    pub tracking: String,
    pub last_error: Option<String>,
    pub placed_at: Option<i64>,
    pub updated_at: i64,
}

/// The orders of every shop placed on AliExpress and not settled yet that
/// `share` holds, last read or changed before `before` (unix seconds): what a sweep
/// reads, in case a push from AliExpress never came. One whose parcels are all
/// known is due sooner, from `settling_before`: a read settles such an order in
/// one go, so only a node that stopped half-way leaves it so, and the shopper is
/// not left waiting a period for it. The shops take turns, each its longest
/// unread first, so one shop's open orders never crowd another shop out. Grouped
/// by tenant, as the sweep paces one shop's reads. When one was last read is
/// the later of its last change and this node's own record of reading it
/// ([`super::read_times::note_forward_read`]).
pub async fn forwards_due(
    db: &Db,
    before: i64,
    settling_before: i64,
    share: Share,
    limit: i64,
) -> anyhow::Result<Vec<ForwardRow>> {
    db.fetch_all(
        "WITH read AS (
            SELECT f.tenant_id, f.id, f.settling, f.shard,
                MAX(f.updated_at, COALESCE(r.tried_at, 0)) AS last_read_at
            FROM aliexpress_forwards f
            LEFT JOIN aliexpress_read_times r ON r.tenant_id = f.tenant_id AND r.id = f.id
            WHERE f.status = 'placed'
         ),
         due AS (
            SELECT tenant_id, id, last_read_at FROM (
                SELECT tenant_id, id, last_read_at, ROW_NUMBER() OVER (
                    PARTITION BY tenant_id ORDER BY last_read_at, id
                ) AS turn
                FROM read
                WHERE (last_read_at < ?1 OR (settling = 1 AND last_read_at < ?5))
                    AND shard % ?3 = ?4
            ) ORDER BY turn, last_read_at, tenant_id LIMIT ?2
         )
         SELECT f.tenant_id, f.id, f.order_id, f.status, f.ae_order_ids, f.tracking,
            f.last_error, f.placed_at, f.updated_at
         FROM aliexpress_forwards f
         JOIN due ON due.tenant_id = f.tenant_id AND due.id = f.id
         ORDER BY f.tenant_id, due.last_read_at, f.id",
        (
            before,
            limit,
            share.of as i64,
            share.index as i64,
            settling_before,
        ),
    )
    .await
}

/// The forwards of every shop that `share` holds whose order was fulfilled
/// with AliExpress's parcels and closed since `since` (unix seconds), whose
/// parcels are due to be asked about at `now`: what the sweep asks AliExpress
/// where the parcels are. Followed from when the forward closed (its last
/// change), for as long as an order is read, so a parcel that takes its time
/// is followed to the door and one AliExpress stopped telling of is let go;
/// no further once this node found them delivered
/// ([`super::read_times::note_parcels_delivered`]). Asked about again
/// `every` seconds after the last time while they move, and after as long as
/// they have been still once they stop, up to `at_most` seconds: a parcel
/// waiting at customs for a week costs a call every `at_most`, not every
/// `every`. The shops take turns, each its longest unasked first, so one
/// shop's season never crowds another shop's parcels out. Grouped by tenant,
/// as the sweep paces one shop's reads.
pub async fn parcels_due(
    db: &Db,
    now: i64,
    every: i64,
    at_most: i64,
    since: i64,
    share: Share,
    limit: i64,
) -> anyhow::Result<Vec<ForwardRow>> {
    db.fetch_all(
        "WITH followed AS (
            SELECT f.tenant_id, f.id, f.shard, COALESCE(r.tried_at, 0) AS tried_at,
                COALESCE(r.tried_at, 0) - COALESCE(r.synced_at, f.updated_at) AS still_for
            FROM aliexpress_forwards f
            LEFT JOIN aliexpress_read_times r
                ON r.tenant_id = f.tenant_id AND r.id = 'parcels:' || f.id
            WHERE f.status = 'done' AND f.tracking != '' AND f.updated_at >= ?4
                AND COALESCE(r.delivered, 0) = 0
         ),
         due AS (
            SELECT tenant_id, id, tried_at FROM (
                SELECT tenant_id, id, tried_at, ROW_NUMBER() OVER (
                    PARTITION BY tenant_id ORDER BY tried_at, id
                ) AS turn
                FROM followed
                WHERE tried_at + MAX(?2, MIN(?3, still_for)) <= ?1 AND shard % ?6 = ?7
            ) ORDER BY turn, tried_at, tenant_id LIMIT ?5
         )
         SELECT f.tenant_id, f.id, f.order_id, f.status, f.ae_order_ids, f.tracking,
            f.last_error, f.placed_at, f.updated_at
         FROM aliexpress_forwards f
         JOIN due ON due.tenant_id = f.tenant_id AND due.id = f.id
         ORDER BY f.tenant_id, due.tried_at, f.id",
        (
            now,
            every,
            at_most,
            since,
            limit,
            share.of as i64,
            share.index as i64,
        ),
    )
    .await
}

/// The forwards of every shop that `share` holds, taken on before `before`
/// (unix seconds) and not heard of since: what the sweep says AliExpress may
/// have, the node placing them having stopped or failed to record the outcome.
/// The longest silent first; the write model decides.
pub async fn forwards_stalled(
    db: &Db,
    before: i64,
    share: Share,
    limit: i64,
) -> anyhow::Result<Vec<ForwardRow>> {
    db.fetch_all(
        "SELECT tenant_id, id, order_id, status, ae_order_ids, tracking, last_error,
            placed_at, updated_at
         FROM aliexpress_forwards
         WHERE status = 'started' AND updated_at < ?1 AND shard % ?3 = ?4
         ORDER BY updated_at, tenant_id, id LIMIT ?2",
        (before, limit, share.of as i64, share.index as i64),
    )
    .await
}

/// Writes the row of a link as the write model sees it: the reads it holds
/// are the ones recorded; a read that changed nothing records no event, so
/// this node keeps when it last read the item for itself ([`super::read_times::note_read`]),
/// and the queries below take the later of the two; once the item is no
/// longer read again (unlinked, or never imported) that record goes. What
/// makes the product matter stays with the product: an item imported anew
/// starts over. The integration tests seed rows through this; nothing else
/// outside the subscription writes one.
pub async fn upsert_link(db: &Db, tenant_id: &str, link: &LinkState) -> anyhow::Result<()> {
    let tried = link.last_requested_at.max(link.last_synced_at);
    let skus = serde_json::to_string(&link.skus)?;
    db.execute(
        "INSERT INTO aliexpress_products (tenant_id, id, ae_product_id, product_id, status,
            offline, own_pricing, last_synced_at, last_error, requested_at, updated_at,
            new_combinations, last_tried_at, shard, skus)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            product_id = excluded.product_id, status = excluded.status,
            offline = excluded.offline, own_pricing = excluded.own_pricing,
            last_synced_at = excluded.last_synced_at,
            last_error = excluded.last_error,
            requested_at = excluded.requested_at, updated_at = excluded.updated_at,
            new_combinations = excluded.new_combinations,
            last_tried_at = excluded.last_tried_at, skus = excluded.skus,
            wanted_at = CASE WHEN product_id IS excluded.product_id THEN wanted_at END,
            published = CASE WHEN product_id IS excluded.product_id THEN published ELSE 0 END",
        (
            tenant_id,
            link.id.as_str(),
            link.ae_product_id.as_str(),
            link.product_id.as_deref(),
            link.status.as_str(),
            link.offline,
            link.pricing.is_some(),
            link.last_synced_at.map(|at| at as i64),
            link.last_error.as_deref(),
            link.requested_at as i64,
            link.updated_at as i64,
            link.offered().count() as i64,
            tried.map(|at| at as i64),
            i64::from(shard_of(&link.id)),
            skus.as_str(),
        ),
    )
    .await?;
    if matches!(link.status, LinkStatus::Unlinked | LinkStatus::Failed) {
        super::read_times::forget(db, tenant_id, &link.id).await?;
        super::deliveries::forget_delivery(db, tenant_id, &link.id).await?;
    }
    Ok(())
}

/// A linked item as the read model has it, with what a plan needs of it:
/// the catalog product it filled and the combinations its variants are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedProduct {
    pub ae_product_id: String,
    pub product_id: String,
    pub skus: Vec<SkuLink>,
}

impl LinkedProduct {
    pub fn as_linked(&self) -> LinkedSkus<'_> {
        LinkedSkus {
            ae_product_id: &self.ae_product_id,
            product_id: &self.product_id,
            skus: &self.skus,
        }
    }
}

/// The imports that filled the catalog products `product_ids`, one per
/// product (its latest, as [`link_of_product`] picks it), the products no
/// import filled left out.
pub async fn linked_products(
    db: &Db,
    tenant_id: &str,
    product_ids: &[&str],
) -> anyhow::Result<Vec<LinkedProduct>> {
    let mut ids: Vec<&str> = product_ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT ae_product_id, product_id, skus FROM (
            SELECT ae_product_id, product_id, skus, ROW_NUMBER() OVER (
                PARTITION BY product_id ORDER BY updated_at DESC, id DESC
            ) AS latest
            FROM aliexpress_products
            WHERE tenant_id = ?1 AND status = 'linked' AND product_id IN ({})
         ) WHERE latest = 1 ORDER BY product_id",
        // `?2` on: the tenant is `?1`.
        (2..=ids.len() + 1)
            .map(|n| format!("?{n}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let mut params = vec![Value::from(tenant_id)];
    params.extend(ids.iter().map(|id| Value::from(*id)));
    let rows: Vec<(String, String, String)> = db.fetch_all(&sql, params_from_iter(params)).await?;
    rows.into_iter()
        .map(|(ae_product_id, product_id, skus)| {
            Ok(LinkedProduct {
                ae_product_id,
                product_id,
                skus: serde_json::from_str(&skus)?,
            })
        })
        .collect()
}

/// What of an order's lines AliExpress sells, as the read model has the
/// shop's imports: what a page says of the order. The forward itself plans
/// from the store ([`forward_items`](crate::forward_items) over the links'
/// own streams), as every command does; this is the same plan a moment
/// behind, at no cost to the store.
pub async fn forward_plan_shown(
    db: &Db,
    tenant_id: &str,
    lines: &[OrderLine],
) -> anyhow::Result<ForwardPlan> {
    let product_ids: Vec<&str> = lines.iter().map(|line| line.product_id.as_str()).collect();
    let linked = linked_products(db, tenant_id, &product_ids).await?;
    let linked: Vec<LinkedSkus<'_>> = linked.iter().map(LinkedProduct::as_linked).collect();
    Ok(plan_items(lines, &linked))
}

/// One row of `aliexpress_products`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct LinkRow {
    pub tenant_id: String,
    pub id: String,
    pub ae_product_id: String,
    pub product_id: Option<String>,
    /// `importing`, `linked`, `failed` or `unlinked`.
    pub status: String,
    pub offline: bool,
    /// Whether the product has its own price rule.
    pub own_pricing: bool,
    /// Unix seconds of the last read that worked, as far as this node knows:
    /// the later of the last one recorded and this node's own
    /// ([`super::read_times::note_read`]).
    pub last_synced_at: Option<i64>,
    pub last_error: Option<String>,
    pub requested_at: i64,
    pub updated_at: i64,
    /// Combinations a read found that the shop could add (ignored aside).
    pub new_combinations: i64,
    /// Unix seconds of the last read asked for or made, as far as this node
    /// knows, whether or not it worked.
    pub last_tried_at: Option<i64>,
}

/// The columns of a [`LinkRow`], shared by the queries below: the two read
/// times are the later of the row's (recorded in the item's stream) and this
/// node's own (`aliexpress_read_times`, joined as `r`).
macro_rules! link_columns {
    () => {
        "p.tenant_id, p.id, p.ae_product_id, p.product_id, p.status, p.offline, p.own_pricing,
            NULLIF(MAX(COALESCE(p.last_synced_at, 0), COALESCE(r.synced_at, 0)), 0)
                AS last_synced_at,
            p.last_error, p.requested_at, p.updated_at, p.new_combinations,
            NULLIF(MAX(COALESCE(p.last_tried_at, 0), COALESCE(r.tried_at, 0)), 0)
                AS last_tried_at"
    };
}

/// The rows the columns are read from.
macro_rules! link_tables {
    () => {
        " FROM aliexpress_products p
          LEFT JOIN aliexpress_read_times r ON r.tenant_id = p.tenant_id AND r.id = p.id "
    };
}

/// A [`LinkRow`] from the table, shared by the queries below.
macro_rules! columns {
    () => {
        concat!("SELECT ", link_columns!(), link_tables!())
    };
}

/// A shop's imports, most recently asked for first.
pub async fn links_of_tenant(db: &Db, tenant_id: &str, limit: i64) -> anyhow::Result<Vec<LinkRow>> {
    db.fetch_all(
        concat!(
            columns!(),
            "WHERE p.tenant_id = ?1 ORDER BY p.requested_at DESC, p.id DESC LIMIT ?2"
        ),
        (tenant_id, limit),
    )
    .await
}

/// The import that filled a catalog product, if one did.
pub async fn link_of_product(
    db: &Db,
    tenant_id: &str,
    product_id: &str,
) -> anyhow::Result<Option<LinkRow>> {
    db.fetch_optional(
        concat!(
            columns!(),
            "WHERE p.tenant_id = ?1 AND p.product_id = ?2 AND p.status = 'linked'
         ORDER BY p.updated_at DESC LIMIT 1"
        ),
        (tenant_id, product_id),
    )
    .await
}

/// The linked items of every shop that `share` holds, last tried before
/// `before` (unix seconds): what the sweep asks to read next, the most useful
/// first. First what shoppers wanted since `wanted_since` (in a cart, or bought),
/// then what is on sale, then the rest. Within each of those the shops take
/// turns, each its oldest first, so one huge catalogue never crowds another shop
/// out.
pub async fn links_due(
    db: &Db,
    before: i64,
    wanted_since: i64,
    share: Share,
    limit: i64,
) -> anyhow::Result<Vec<LinkRow>> {
    db.fetch_all(
        concat!(
            "WITH due AS (
                SELECT tenant_id, id, rank, last_tried_at, ROW_NUMBER() OVER (
                    PARTITION BY tenant_id, rank ORDER BY last_tried_at, id
                ) AS turn
                FROM (
                    SELECT p.tenant_id, p.id, CASE
                        WHEN p.wanted_at >= ?2 THEN 0
                        WHEN p.published = 1 THEN 1
                        ELSE 2
                    END AS rank,
                    MAX(COALESCE(p.last_tried_at, 0), COALESCE(r.tried_at, 0)) AS last_tried_at",
            link_tables!(),
            "WHERE p.status = 'linked' AND p.shard % ?3 = ?4
                )
                WHERE last_tried_at < ?1
            )
            SELECT ",
            link_columns!(),
            link_tables!(),
            "JOIN due ON due.tenant_id = p.tenant_id AND due.id = p.id
            ORDER BY due.rank, due.turn, due.last_tried_at, p.tenant_id, p.id
            LIMIT ?5"
        ),
        (
            before,
            wanted_since,
            share.of as i64,
            share.index as i64,
            limit,
        ),
    )
    .await
}
