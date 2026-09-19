//! Which facts of the other contexts are worth an e-mail. One subscription,
//! not strict: it listens to a handful of events across five aggregates.
//! Every handler only enqueues (see [`crate::enqueue`]); the message id is
//! derived from the event id, so a redelivery never writes a second e-mail.

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;
use timada_inventory::aggregator::BackInStockAlertTriggered;
use timada_order::{
    OrderDetailsView,
    aggregator::{OrderCancelled, OrderConfirmationResent, OrderPlaced, OrderShipped},
};
use timada_payment::aggregator::PaymentRefunded;
use timada_review::aggregator::QuestionAnswered;

use crate::{
    config::MailerConfig,
    email::Email,
    outbox::enqueue,
    template::{self, Content},
};

pub const MAILER_SUBSCRIPTION: &str = "mailer";

/// Needs the `SqlitePool` and a [`MailerConfig`] as subscription data.
pub fn mailer_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(MAILER_SUBSCRIPTION)
        .handler(confirm_on_order_placed())
        .handler(confirm_again_on_confirmation_resent())
        .handler(notify_on_order_shipped())
        .handler(notify_on_order_cancelled())
        .handler(notify_on_payment_refunded())
        .handler(notify_on_alert_triggered())
        .handler(notify_on_question_answered())
}

/// The subscription data and whether the event is recent enough to be worth
/// an e-mail; `None` means "skip".
fn setup<E: Executor>(
    ctx: &Context<'_, E>,
    event_timestamp: u64,
) -> anyhow::Result<Option<(SqlitePool, MailerConfig)>> {
    let db = ctx
        .get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))?;
    let config = ctx
        .get::<MailerConfig>()
        .ok_or_else(|| anyhow::anyhow!("MailerConfig missing from subscription context"))?;
    let age = timada_core::time::now_unix_secs()?.saturating_sub(event_timestamp);
    if age > config.max_event_age_secs {
        return Ok(None);
    }
    Ok(Some((db, config)))
}

async fn queue(
    db: &SqlitePool,
    config: &MailerConfig,
    event_id: &str,
    kind: &str,
    to: &str,
    (subject, body): Content,
) -> anyhow::Result<()> {
    let message_id = timada_core::id::derived(&[event_id], kind);
    let email = Email {
        from: config.from.clone(),
        to: to.to_owned(),
        subject,
        body,
    };
    if enqueue(db, &message_id, kind, &email).await? {
        tracing::info!(%message_id, %kind, "e-mail queued");
    }
    Ok(())
}

/// The order and who placed it: `(order, e-mail, first name)`.
async fn order_and_customer<E: Executor>(
    executor: &E,
    order_id: &str,
) -> anyhow::Result<(OrderDetailsView, String, String)> {
    let Some(order) = timada_order::load_order_details(executor, order_id).await? else {
        anyhow::bail!("order {order_id} cannot be loaded");
    };
    let Some(customer) = timada_customer::load_address_book(executor, &order.customer_id).await?
    else {
        anyhow::bail!(
            "customer {} of order {order_id} cannot be loaded",
            order.customer_id
        );
    };
    Ok((order, customer.email, customer.first_name))
}

#[evento::subscription]
async fn confirm_on_order_placed<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<OrderPlaced>,
) -> anyhow::Result<()> {
    let Some((db, config)) = setup(ctx, event.timestamp)? else {
        return Ok(());
    };
    let (order, to, first_name) = order_and_customer(ctx.executor, &event.aggregate_id).await?;
    let content = template::order_confirmation(&config, &first_name, &order);
    queue(
        &db,
        &config,
        &event.id.to_string(),
        "order-confirmation",
        &to,
        content,
    )
    .await
}

/// "Renvoyer la confirmation": each request is its own event, so its own e-mail.
#[evento::subscription]
async fn confirm_again_on_confirmation_resent<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<OrderConfirmationResent>,
) -> anyhow::Result<()> {
    let Some((db, config)) = setup(ctx, event.timestamp)? else {
        return Ok(());
    };
    let (order, to, first_name) = order_and_customer(ctx.executor, &event.aggregate_id).await?;
    let content = template::order_confirmation(&config, &first_name, &order);
    queue(
        &db,
        &config,
        &event.id.to_string(),
        "order-confirmation",
        &to,
        content,
    )
    .await
}

#[evento::subscription]
async fn notify_on_order_shipped<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<OrderShipped>,
) -> anyhow::Result<()> {
    let Some((db, config)) = setup(ctx, event.timestamp)? else {
        return Ok(());
    };
    let (order, to, first_name) = order_and_customer(ctx.executor, &event.aggregate_id).await?;
    let content = template::order_shipped(&config, &first_name, &order);
    queue(
        &db,
        &config,
        &event.id.to_string(),
        "order-shipped",
        &to,
        content,
    )
    .await
}

#[evento::subscription]
async fn notify_on_order_cancelled<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<OrderCancelled>,
) -> anyhow::Result<()> {
    let Some((db, config)) = setup(ctx, event.timestamp)? else {
        return Ok(());
    };
    let (order, to, first_name) = order_and_customer(ctx.executor, &event.aggregate_id).await?;
    let content = template::order_cancelled(&config, &first_name, &order, &event.data.reason);
    queue(
        &db,
        &config,
        &event.id.to_string(),
        "order-cancelled",
        &to,
        content,
    )
    .await
}

#[evento::subscription]
async fn notify_on_payment_refunded<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<PaymentRefunded>,
) -> anyhow::Result<()> {
    let Some((db, config)) = setup(ctx, event.timestamp)? else {
        return Ok(());
    };
    let Some(payment) = timada_payment::load_payment(ctx.executor, &event.aggregate_id).await?
    else {
        anyhow::bail!(
            "payment {} refunded but cannot be loaded",
            event.aggregate_id
        );
    };
    let (order, to, first_name) = order_and_customer(ctx.executor, &payment.order_id).await?;
    let content = template::refund(&config, &first_name, &order, &event.data.amount);
    queue(&db, &config, &event.id.to_string(), "refund", &to, content).await
}

/// Goes to the address given when the alert was asked for.
#[evento::subscription]
async fn notify_on_alert_triggered<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<BackInStockAlertTriggered>,
) -> anyhow::Result<()> {
    let Some((db, config)) = setup(ctx, event.timestamp)? else {
        return Ok(());
    };
    let Some(alert) = timada_inventory::Command(ctx.executor)
        .load_alert(&event.aggregate_id)
        .await?
    else {
        anyhow::bail!(
            "alert {} triggered but cannot be loaded",
            event.aggregate_id
        );
    };
    let product_name = timada_catalog::load_product_page(ctx.executor, &alert.product_id)
        .await?
        .map_or_else(|| "Votre produit".to_owned(), |p| p.name);
    let content = template::back_in_stock(&config, &alert.product_id, &product_name);
    queue(
        &db,
        &config,
        &event.id.to_string(),
        "back-in-stock",
        &alert.email,
        content,
    )
    .await
}

/// Tells whoever asked. A customer answering their own question is not told.
#[evento::subscription]
async fn notify_on_question_answered<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<QuestionAnswered>,
) -> anyhow::Result<()> {
    let Some((db, config)) = setup(ctx, event.timestamp)? else {
        return Ok(());
    };
    let Some(question) = timada_review::Command(ctx.executor)
        .load_question(&event.aggregate_id)
        .await?
    else {
        anyhow::bail!(
            "question {} answered but cannot be loaded",
            event.aggregate_id
        );
    };
    if matches!(&event.data.author, timada_review::AnswerAuthor::Customer { customer_id } if *customer_id == question.customer_id)
    {
        return Ok(());
    }
    let Some(customer) =
        timada_customer::load_address_book(ctx.executor, &question.customer_id).await?
    else {
        // Asked by someone who is not a registered customer: nobody to write to.
        return Ok(());
    };
    let product_name = timada_catalog::load_product_page(ctx.executor, &question.product_id)
        .await?
        .map_or_else(|| "un produit".to_owned(), |p| p.name);
    let content = template::question_answered(
        &config,
        &customer.first_name,
        &question.product_id,
        &product_name,
        &question.body,
        &event.data.body,
    );
    queue(
        &db,
        &config,
        &event.id.to_string(),
        "question-answered",
        &customer.email,
        content,
    )
    .await
}
