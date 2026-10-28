//! What a condition looks at: the facts of a run's subject, read when the
//! run gets to the condition, so a condition after a wait sees the order
//! as it is then (refunded meanwhile, say).

use sqlx::SqlitePool;
use timada_automations::catalog::{Fact, Facts, Subject};
use timada_core::Keys;
use timada_orders::{OrderStatus, orders_of_email};
use timada_reviews::ReviewStatus;
use timada_support::MessageStatus;

/// The facts of a tenant's subject; none for one gone (erased, say), so
/// every rule on it is false.
pub async fn facts_of<E: evento::Executor + Clone>(
    store: &E,
    read_pool: &SqlitePool,
    keys: &dyn Keys,
    tenant_id: &str,
    subject: Subject,
    subject_id: &str,
) -> anyhow::Result<Facts> {
    let mut facts = Facts::new();
    match subject {
        Subject::Order => {
            let orders = timada_orders::Command::new(store.clone(), tenant_id);
            let Some(order) = orders.load_order(subject_id).await? else {
                return Ok(facts);
            };
            let earlier = orders_of_email(read_pool, tenant_id, &order.email, 50)
                .await?
                .iter()
                .any(|row| row.id != order.id && (row.placed_at as u64) < order.placed_at);
            facts.insert("order.total", Fact::Cents(order.total_cents));
            facts.insert(
                "order.items",
                Fact::Number(
                    order
                        .lines
                        .iter()
                        .map(|line| u64::from(line.quantity))
                        .sum(),
                ),
            );
            facts.insert(
                "order.country",
                Fact::Country(order.shipping.country.clone()),
            );
            facts.insert("order.discounted", Fact::Bool(order.discount.is_some()));
            facts.insert("order.first", Fact::Bool(!earlier));
            facts.insert("order.fulfilled", Fact::Bool(order.fulfilled_at.is_some()));
            facts.insert("order.refunded", Fact::Bool(order.refunded_cents > 0));
            facts.insert(
                "order.cancelled",
                Fact::Bool(order.status == OrderStatus::Cancelled),
            );
        }
        Subject::Shopper => {
            let shoppers = timada_shoppers::Command::new(store.clone(), tenant_id);
            let Some(state) = shoppers.load_shopper(subject_id).await? else {
                return Ok(facts);
            };
            if let Some(shopper) = state.open_with(keys).await? {
                facts.insert(
                    "shopper.has_ordered",
                    Fact::Bool(has_ordered(read_pool, tenant_id, &shopper.email).await?),
                );
            }
        }
        Subject::Subscriber => {
            let newsletter = timada_newsletter::Command::new(store.clone(), tenant_id);
            let Some(state) = newsletter.load_subscriber(subject_id).await? else {
                return Ok(facts);
            };
            if let Some(language) = &state.language {
                facts.insert("subscriber.language", Fact::Text(language.clone()));
            }
            facts.insert("subscriber.source", Fact::Text(state.source.clone()));
            if let Some(subscriber) = state.open_with(keys).await? {
                facts.insert(
                    "subscriber.has_ordered",
                    Fact::Bool(has_ordered(read_pool, tenant_id, &subscriber.email).await?),
                );
            }
        }
        Subject::Review => {
            let reviews = timada_reviews::Command::new(store.clone(), tenant_id);
            let Some(review) = reviews.load_review(subject_id).await? else {
                return Ok(facts);
            };
            facts.insert("review.rating", Fact::Number(u64::from(review.rating)));
            facts.insert("review.photos", Fact::Bool(!review.photos.is_empty()));
            facts.insert(
                "review.published",
                Fact::Bool(review.status == ReviewStatus::Published),
            );
        }
        Subject::Message => {
            let support = timada_support::Command::new(store.clone(), tenant_id);
            let Some(message) = support.load_message(subject_id).await? else {
                return Ok(facts);
            };
            if let Some(language) = &message.language {
                facts.insert("message.language", Fact::Text(language.clone()));
            }
            facts.insert(
                "message.handled",
                Fact::Bool(message.status == MessageStatus::Handled),
            );
        }
    }
    Ok(facts)
}

async fn has_ordered(read_pool: &SqlitePool, tenant_id: &str, email: &str) -> anyhow::Result<bool> {
    Ok(!orders_of_email(read_pool, tenant_id, email, 1)
        .await?
        .is_empty())
}
