//! The subscription that keeps `newsletter_subscribers`,
//! `newsletter_campaigns` and `newsletter_deliveries` current for every
//! tenant.
//!
//! Each handler reloads the strict write model of the stream and rewrites
//! its row (a subscriber's opened with their key), so delivery may repeat (the
//! subscription is at-least-once) without the row drifting; an erased
//! subscriber loses their row. A key not readable yet (on its way from
//! another node) fails the handler, which is retried. One subscription
//! follows every tenant's streams (its cursor lives in the store): each row
//! carries the tenant of the event that made it, and it writes into the
//! read-model database it is given.
//!
//! The counts move with the rows, in the same transaction: a handler reads
//! the row as it stood, writes it as the write model sees it, and moves the
//! counts by the difference, so a repeated event moves nothing. A subscriber
//! is counted in the audience of their language and time zone while
//! confirmed; a delivery in its campaign's, and its edition's, count of its
//! outcome, and once mailed (or tried) in its edition's count of the
//! subscriber's time zone.

use std::sync::Arc;

use evento::{
    Executor,
    metadata::RawEvent,
    subscription::{Context, SubscriptionBuilder},
};
use timada_core::db::{Connection, Db, execute, fetch_optional};
use timada_core::keys::KeyRing;
use timada_core::shard_of;

use crate::aggregator::{
    Campaign as CampaignStream, Delivery as DeliveryStream, Edition as EditionStream,
    Subscriber as SubscriberStream,
};
use crate::command::{
    CampaignState, DeliveryState, EditionState, Subscriber, campaign_projection,
    delivery_projection, edition_projection, subscriber_projection,
};
use crate::value_object::SubscriberStatus;

/// The cursor identity of the read-model subscription, in the store.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-newsletter/read-models";

/// Where every tenant's keys are.
#[derive(Clone)]
struct Ring(Arc<dyn KeyRing>);

/// Builds the subscription over every tenant and routing key. `db` is the
/// read-model database and must be allowed to write; `keys` holds each
/// tenant's keys, which open the addresses. Start it with `.start(&store)`
/// and keep the handle for shutdown.
pub fn read_model_subscription<E: Executor>(
    db: Db,
    keys: Arc<dyn KeyRing>,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(db)
        .data(Ring(keys))
        .handler(on_subscriber_event())
        .handler(on_campaign_event())
        .handler(on_delivery_event())
        .handler(on_edition_event())
        .any_routing_key()
}

#[evento::subscription_all]
async fn on_subscriber_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<SubscriberStream>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let keys: Ring = context.extract();
    let Some(state) = subscriber_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    if state.forgotten {
        return delete_subscriber(&db, &event.tenant, &state.id).await;
    }
    let keys = keys.0.for_tenant(&event.tenant);
    let subscriber = state
        .open_with(&*keys)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the key of subscriber {} is not readable yet", state.id))?;
    upsert_subscriber(&db, &event.tenant, &subscriber).await
}

#[evento::subscription_all]
async fn on_campaign_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<CampaignStream>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(state) = campaign_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_campaign(&db, &event.tenant, &state).await
}

#[evento::subscription_all]
async fn on_delivery_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<DeliveryStream>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(state) = delivery_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_delivery(&db, &event.tenant, &state).await
}

/// What became of one mail, as the counts tell it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// Handed to the mail server.
    Delivered,
    /// Not taken by the mail server.
    Failed,
    /// Never sent: the subscriber had left the list.
    Skipped,
}

impl Outcome {
    fn of(failed: bool, skipped: bool) -> Self {
        if skipped {
            Self::Skipped
        } else if failed {
            Self::Failed
        } else {
            Self::Delivered
        }
    }

    /// Its part of the counts: `(delivered, failed, skipped)`.
    fn counts(this: Option<Self>) -> (i64, i64, i64) {
        match this {
            None => (0, 0, 0),
            Some(Self::Delivered) => (1, 0, 0),
            Some(Self::Failed) => (0, 1, 0),
            Some(Self::Skipped) => (0, 0, 1),
        }
    }

    /// Whether it counts in its edition's time zone: mailed, or tried.
    fn mailed(this: Option<Self>) -> i64 {
        i64::from(this.is_some_and(|outcome| outcome != Self::Skipped))
    }
}

/// Writes the row of a delivery as the write model sees it and, when its
/// outcome changed, the counts of its campaign, of its edition and of the
/// edition's time zone, in one transaction. The time zone is the one the
/// subscriber has when the delivery is written: the one they had when
/// mailed, near enough.
pub(crate) async fn upsert_delivery(
    db: &Db,
    tenant_id: &str,
    delivery: &DeliveryState,
) -> anyhow::Result<()> {
    let env = (tenant_id, delivery);
    db.write(&env, |tx, (tenant_id, delivery)| {
        Box::pin(async move {
            let tenant_id: &str = tenant_id;
            // A campaign sent at once keys its deliveries by its own id.
            let list_id = delivery
                .edition_id
                .as_deref()
                .unwrap_or(&delivery.campaign_id);
            let stored: Option<(bool, bool)> = fetch_optional(
                tx,
                "SELECT failed, skipped FROM newsletter_deliveries
                    INDEXED BY sqlite_autoindex_newsletter_deliveries_1
                 WHERE tenant_id = ?1 AND edition_id = ?2 AND subscriber_id = ?3",
                (tenant_id, list_id, delivery.subscriber_id.as_str()),
            )
            .await?;
            execute(
                tx,
                "INSERT INTO newsletter_deliveries
                    (tenant_id, campaign_id, edition_id, subscriber_id, failed, skipped)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT (tenant_id, edition_id, subscriber_id) DO UPDATE SET
                    failed = excluded.failed, skipped = excluded.skipped",
                (
                    tenant_id,
                    delivery.campaign_id.as_str(),
                    list_id,
                    delivery.subscriber_id.as_str(),
                    delivery.failed.is_some(),
                    delivery.skipped.is_some(),
                ),
            )
            .await?;
            let before = stored.map(|(failed, skipped)| Outcome::of(failed, skipped));
            let after = Some(Outcome::of(
                delivery.failed.is_some(),
                delivery.skipped.is_some(),
            ));
            if before == after {
                return Ok(());
            }
            let (was, now) = (Outcome::counts(before), Outcome::counts(after));
            let moved = (now.0 - was.0, now.1 - was.1, now.2 - was.2);
            execute(
                tx,
                "UPDATE newsletter_campaigns SET delivered = delivered + ?3,
                    failed = failed + ?4, skipped = skipped + ?5
                 WHERE tenant_id = ?1 AND id = ?2",
                (
                    tenant_id,
                    delivery.campaign_id.as_str(),
                    moved.0,
                    moved.1,
                    moved.2,
                ),
            )
            .await?;
            let Some(edition_id) = delivery.edition_id.as_deref() else {
                return Ok(());
            };
            execute(
                tx,
                "UPDATE newsletter_editions SET delivered = delivered + ?3,
                    failed = failed + ?4, skipped = skipped + ?5
                 WHERE tenant_id = ?1 AND id = ?2",
                (tenant_id, edition_id, moved.0, moved.1, moved.2),
            )
            .await?;
            let mailed = Outcome::mailed(after) - Outcome::mailed(before);
            if mailed == 0 {
                return Ok(());
            }
            let zone: Option<(Option<String>,)> = fetch_optional(
                tx,
                "SELECT time_zone FROM newsletter_subscribers WHERE tenant_id = ?1 AND id = ?2",
                (tenant_id, delivery.subscriber_id.as_str()),
            )
            .await?;
            let zone = zone.and_then(|(zone,)| zone).unwrap_or_default();
            execute(
                tx,
                "INSERT INTO newsletter_edition_zones (tenant_id, edition_id, time_zone, mailed)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (tenant_id, edition_id, time_zone) DO UPDATE SET
                    mailed = mailed + excluded.mailed",
                (tenant_id, edition_id, zone.as_str(), mailed),
            )
            .await?;
            Ok(())
        })
    })
    .await
}

#[evento::subscription_all]
async fn on_edition_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<EditionStream>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(state) = edition_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_edition(&db, &event.tenant, &state).await
}

/// Writes the row of an edition as the write model sees it.
pub(crate) async fn upsert_edition(
    db: &Db,
    tenant_id: &str,
    edition: &EditionState,
) -> anyhow::Result<()> {
    db.execute(
        "INSERT INTO newsletter_editions (tenant_id, id, campaign_id, cycle, status,
            opened_at, closed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            cycle = excluded.cycle, status = excluded.status,
            opened_at = excluded.opened_at, closed_at = excluded.closed_at",
        (
            tenant_id,
            edition.id.as_str(),
            edition.campaign_id.as_str(),
            edition.cycle.as_str(),
            if edition.is_open() { "open" } else { "closed" },
            edition.opened_at as i64,
            edition.closed_at.map(|at| at as i64),
        ),
    )
    .await?;
    Ok(())
}

/// Writes the row of a campaign as the write model sees it.
pub(crate) async fn upsert_campaign(
    db: &Db,
    tenant_id: &str,
    campaign: &CampaignState,
) -> anyhow::Result<()> {
    db.execute(
        "INSERT INTO newsletter_campaigns (tenant_id, id, subject, language, status,
            created_at, sent_at, finished_at, updated_at, cadence)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            subject = excluded.subject, language = excluded.language,
            cadence = excluded.cadence,
            status = excluded.status, sent_at = excluded.sent_at,
            finished_at = excluded.finished_at, updated_at = excluded.updated_at",
        (
            tenant_id,
            campaign.id.as_str(),
            campaign.subject.as_str(),
            campaign.language.as_deref(),
            campaign.status.as_str(),
            campaign.created_at as i64,
            campaign.sent_at.map(|at| at as i64),
            campaign.finished_at.map(|at| at as i64),
            campaign.updated_at as i64,
            campaign
                .schedule
                .as_ref()
                .map(|schedule| schedule.cadence.as_str()),
        ),
    )
    .await?;
    Ok(())
}

/// Drops the row of an erased subscriber, and their count in the audience.
pub(crate) async fn delete_subscriber(
    db: &Db,
    tenant_id: &str,
    subscriber_id: &str,
) -> anyhow::Result<()> {
    let env = (tenant_id, subscriber_id);
    db.write(&env, |tx, (tenant_id, subscriber_id)| {
        Box::pin(async move {
            let before = counted_in(tx, tenant_id, subscriber_id).await?;
            execute(
                tx,
                "DELETE FROM newsletter_subscribers WHERE tenant_id = ?1 AND id = ?2",
                (*tenant_id, *subscriber_id),
            )
            .await?;
            move_in_audience(tx, tenant_id, before, None).await
        })
    })
    .await
}

/// Writes the row of a subscriber as the write model sees them, opened, and
/// moves their count in the audience, in one transaction.
pub(crate) async fn upsert_subscriber(
    db: &Db,
    tenant_id: &str,
    subscriber: &Subscriber,
) -> anyhow::Result<()> {
    let env = (tenant_id, subscriber);
    db.write(&env, |tx, (tenant_id, subscriber)| {
        Box::pin(async move {
            let before = counted_in(tx, tenant_id, &subscriber.id).await?;
            execute(
                tx,
                "INSERT INTO newsletter_subscribers (tenant_id, id, email, language, source,
                    status, requested_at, confirmed_at, unsubscribed_at, updated_at, time_zone,
                    shard)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT (tenant_id, id) DO UPDATE SET
                    email = excluded.email, language = excluded.language,
                    time_zone = excluded.time_zone,
                    source = excluded.source, status = excluded.status,
                    requested_at = excluded.requested_at, confirmed_at = excluded.confirmed_at,
                    unsubscribed_at = excluded.unsubscribed_at, updated_at = excluded.updated_at",
                (
                    *tenant_id,
                    subscriber.id.as_str(),
                    subscriber.email.as_str(),
                    subscriber.language.as_deref(),
                    subscriber.source.as_str(),
                    subscriber.status.as_str(),
                    subscriber.requested_at as i64,
                    subscriber.confirmed_at.map(|at| at as i64),
                    subscriber.unsubscribed_at.map(|at| at as i64),
                    subscriber.updated_at as i64,
                    subscriber.time_zone.as_deref(),
                    i64::from(shard_of(&subscriber.id)),
                ),
            )
            .await?;
            let after = audience_group(
                subscriber.status.as_str(),
                subscriber.language.as_deref(),
                subscriber.time_zone.as_deref(),
            );
            move_in_audience(tx, tenant_id, before, after).await
        })
    })
    .await
}

/// The group of `newsletter_audience` a subscriber is counted in: their
/// language and their time zone, `""` for none.
type AudienceGroup = (String, String);

/// The group a subscriber with this status, language and time zone is
/// counted in: none unless confirmed.
fn audience_group(
    status: &str,
    language: Option<&str>,
    time_zone: Option<&str>,
) -> Option<AudienceGroup> {
    (status == SubscriberStatus::Confirmed.as_str()).then(|| {
        (
            language.unwrap_or_default().to_owned(),
            time_zone.unwrap_or_default().to_owned(),
        )
    })
}

/// The group the stored row of a subscriber is counted in.
async fn counted_in(
    tx: &Connection,
    tenant_id: &str,
    subscriber_id: &str,
) -> anyhow::Result<Option<AudienceGroup>> {
    let row: Option<(String, Option<String>, Option<String>)> = fetch_optional(
        tx,
        "SELECT status, language, time_zone FROM newsletter_subscribers
         WHERE tenant_id = ?1 AND id = ?2",
        (tenant_id, subscriber_id),
    )
    .await?;
    Ok(row.and_then(|(status, language, time_zone)| {
        audience_group(&status, language.as_deref(), time_zone.as_deref())
    }))
}

/// Moves a subscriber's count from the group they were counted in to the
/// one they are counted in now.
async fn move_in_audience(
    tx: &Connection,
    tenant_id: &str,
    before: Option<AudienceGroup>,
    after: Option<AudienceGroup>,
) -> anyhow::Result<()> {
    if before == after {
        return Ok(());
    }
    for ((language, time_zone), by) in [(before, -1i64), (after, 1)]
        .into_iter()
        .filter_map(|(group, by)| group.map(|group| (group, by)))
    {
        execute(
            tx,
            "INSERT INTO newsletter_audience (tenant_id, language, time_zone, confirmed)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (tenant_id, language, time_zone) DO UPDATE SET
                confirmed = confirmed + excluded.confirmed",
            (tenant_id, language.as_str(), time_zone.as_str(), by),
        )
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::{
        MIGRATIONS, campaign_row, count_audience, edition_progress, editions_of_campaign,
    };
    use crate::value_object::CampaignStatus;

    const TENANT: &str = "acme";

    async fn db() -> Db {
        let store = evento::Turso::in_memory_concurrent().await.unwrap();
        let mut conn = store.connect().unwrap();
        MIGRATIONS
            .to_latest(&mut conn)
            .await
            .map_err(|(_, err)| err)
            .unwrap();
        Db::from(store)
    }

    fn subscriber(
        id: &str,
        status: SubscriberStatus,
        language: Option<&str>,
        time_zone: Option<&str>,
    ) -> Subscriber {
        Subscriber {
            id: id.to_owned(),
            email: format!("{id}@example.com"),
            language: language.map(str::to_owned),
            time_zone: time_zone.map(str::to_owned),
            source: "form".to_owned(),
            status,
            ..Subscriber::default()
        }
    }

    fn delivery(edition_id: Option<&str>, subscriber_id: &str) -> DeliveryState {
        DeliveryState {
            id: format!("{}-{subscriber_id}", edition_id.unwrap_or("c1")),
            campaign_id: "c1".to_owned(),
            edition_id: edition_id.map(str::to_owned),
            subscriber_id: subscriber_id.to_owned(),
            ..DeliveryState::default()
        }
    }

    async fn audience(db: &Db) -> Vec<(String, String, i64)> {
        db.fetch_all(
            "SELECT language, time_zone, confirmed FROM newsletter_audience
             WHERE tenant_id = ?1 AND confirmed != 0 ORDER BY language, time_zone",
            (TENANT,),
        )
        .await
        .unwrap()
    }

    fn group(language: &str, time_zone: &str, confirmed: i64) -> (String, String, i64) {
        (language.to_owned(), time_zone.to_owned(), confirmed)
    }

    /// A subscriber counts once, in the group of their language and time
    /// zone, while confirmed; written again, they count the same.
    #[tokio::test]
    async fn the_audience_follows_each_subscriber() {
        let db = db().await;
        use SubscriberStatus::*;
        let ada = |status, time_zone| subscriber("ada", status, Some("fr"), time_zone);

        upsert_subscriber(&db, TENANT, &ada(Pending, None))
            .await
            .unwrap();
        assert_eq!(audience(&db).await, []);
        for _ in 0..2 {
            upsert_subscriber(&db, TENANT, &ada(Confirmed, None))
                .await
                .unwrap();
        }
        assert_eq!(audience(&db).await, [group("fr", "", 1)]);
        upsert_subscriber(&db, TENANT, &ada(Confirmed, Some("Europe/Paris")))
            .await
            .unwrap();
        assert_eq!(audience(&db).await, [group("fr", "Europe/Paris", 1)]);
        upsert_subscriber(&db, TENANT, &subscriber("alan", Confirmed, None, None))
            .await
            .unwrap();
        assert_eq!(count_audience(&db, TENANT, None).await.unwrap(), 2);
        assert_eq!(count_audience(&db, TENANT, Some("fr")).await.unwrap(), 1);
        assert_eq!(count_audience(&db, TENANT, Some("en")).await.unwrap(), 0);

        upsert_subscriber(&db, TENANT, &ada(Unsubscribed, Some("Europe/Paris")))
            .await
            .unwrap();
        assert_eq!(audience(&db).await, [group("", "", 1)]);
        upsert_subscriber(&db, TENANT, &ada(Confirmed, Some("Europe/Paris")))
            .await
            .unwrap();
        // Erased, twice: counted out once.
        for _ in 0..2 {
            delete_subscriber(&db, TENANT, "ada").await.unwrap();
        }
        assert_eq!(audience(&db).await, [group("", "", 1)]);
        assert_eq!(count_audience(&db, "other", None).await.unwrap(), 0);
    }

    /// A delivery counts once in its campaign's and its edition's count of
    /// its outcome, moving when the outcome does, and once mailed in its
    /// edition's count of the subscriber's time zone.
    #[tokio::test]
    async fn a_delivery_counts_once_where_its_outcome_says() {
        let db = db().await;
        let campaign = CampaignState {
            id: "c1".to_owned(),
            subject: "News".to_owned(),
            status: CampaignStatus::Scheduled,
            ..CampaignState::default()
        };
        upsert_campaign(&db, TENANT, &campaign).await.unwrap();
        let edition = EditionState {
            id: "e1".to_owned(),
            campaign_id: "c1".to_owned(),
            cycle: "2030-01-08".to_owned(),
            ..EditionState::default()
        };
        upsert_edition(&db, TENANT, &edition).await.unwrap();
        for (id, time_zone) in [
            ("ada", Some("Europe/Paris")),
            ("alan", None),
            ("grace", Some("Europe/Paris")),
        ] {
            upsert_subscriber(
                &db,
                TENANT,
                &subscriber(id, SubscriberStatus::Confirmed, None, time_zone),
            )
            .await
            .unwrap();
        }
        let counts = async || {
            let row = campaign_row(&db, TENANT, "c1").await.unwrap().unwrap();
            let [edition] = editions_of_campaign(&db, TENANT, "c1", 10)
                .await
                .unwrap()
                .try_into()
                .unwrap();
            (
                (row.delivered, row.failed, row.skipped),
                (edition.delivered, edition.failed, edition.skipped),
            )
        };

        // Each written twice, as a repeated event would.
        for _ in 0..2 {
            upsert_delivery(&db, TENANT, &delivery(Some("e1"), "ada"))
                .await
                .unwrap();
            upsert_delivery(&db, TENANT, &delivery(Some("e1"), "alan"))
                .await
                .unwrap();
        }
        assert_eq!(counts().await, ((2, 0, 0), (2, 0, 0)));
        // Alan's mail failed: moved, not added.
        let failed = DeliveryState {
            failed: Some("550 no such user".to_owned()),
            ..delivery(Some("e1"), "alan")
        };
        upsert_delivery(&db, TENANT, &failed).await.unwrap();
        upsert_delivery(&db, TENANT, &failed).await.unwrap();
        assert_eq!(counts().await, ((1, 1, 0), (1, 1, 0)));
        // Grace left before her mail: skipped, not mailed.
        let skipped = DeliveryState {
            skipped: Some("left the list".to_owned()),
            ..delivery(Some("e1"), "grace")
        };
        upsert_delivery(&db, TENANT, &skipped).await.unwrap();
        assert_eq!(counts().await, ((1, 1, 1), (1, 1, 1)));
        // A delivery of the campaign sent at once counts on the campaign.
        upsert_delivery(&db, TENANT, &delivery(None, "grace"))
            .await
            .unwrap();
        assert_eq!(counts().await, ((2, 1, 1), (1, 1, 1)));

        // By zone: Ada in Paris, Alan in the shop's zone (Paris too here, so
        // one group); Grace was never mailed.
        let progress = edition_progress(&db, TENANT, "e1", None, "Europe/Paris")
            .await
            .unwrap();
        assert_eq!(progress.len(), 1);
        assert_eq!(
            (
                progress[0].zone.as_str(),
                progress[0].delivered,
                progress[0].audience
            ),
            ("Europe/Paris", 2, 3)
        );
        let progress = edition_progress(&db, TENANT, "e1", None, "UTC")
            .await
            .unwrap();
        let by_zone: Vec<(&str, i64, i64)> = progress
            .iter()
            .map(|zone| (zone.zone.as_str(), zone.delivered, zone.audience))
            .collect();
        assert_eq!(by_zone, [("Europe/Paris", 1, 2), ("UTC", 1, 1)]);
        // The edition's language: nobody here speaks French.
        let progress = edition_progress(&db, TENANT, "e1", Some("fr"), "UTC")
            .await
            .unwrap();
        assert!(progress.iter().all(|zone| zone.audience == 0));
    }
}
