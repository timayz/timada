//! Which facts of the other contexts are worth an e-mail. One subscription,
//! not strict: it listens to a handful of events across eight aggregates.
//! Every handler only enqueues (see [`crate::enqueue`]); the message id is
//! derived from the event id, so a redelivery never writes a second e-mail.

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;
use timada_customer::aggregator::CustomerRegistered;
use timada_inventory::aggregator::BackInStockAlertTriggered;
use timada_order::{
    OrderDetailsView,
    aggregator::{OrderCancelled, OrderConfirmationResent, OrderPlaced, OrderShipped},
};
use timada_payment::aggregator::PaymentRefunded;
use timada_returns::{
    ReturnView,
    aggregator::{ReturnApproved, ReturnCompleted, ReturnRefused},
};
use timada_review::aggregator::{
    AnswerPublished, QuestionAnswered, QuestionRejected, ReviewPublished, ReviewRejected,
};

use crate::{
    config::MailerConfig,
    email::Email,
    outbox::enqueue,
    templates::{Content, MailerTemplates},
};

pub const MAILER_SUBSCRIPTION: &str = "mailer";

/// Needs the `SqlitePool` and a [`MailerConfig`] as subscription data, and
/// takes a [`MailerTemplates`] the same way when the host words its own
/// e-mails; the built-in French ones are used otherwise. With the
/// `invoice-pdf` feature, a `timada_invoice::InvoiceIssuer` handed as data
/// turns on the e-mail that carries each issued invoice as a PDF.
pub fn mailer_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    let builder = SubscriptionBuilder::new(MAILER_SUBSCRIPTION)
        .handler(welcome_on_customer_registered())
        .handler(confirm_on_order_placed())
        .handler(confirm_again_on_confirmation_resent())
        .handler(notify_on_order_shipped())
        .handler(notify_on_order_cancelled())
        .handler(notify_on_payment_refunded())
        .handler(notify_on_alert_triggered())
        .handler(notify_on_question_answered())
        .handler(notify_on_answer_published())
        .handler(notify_on_question_rejected())
        .handler(notify_on_review_published())
        .handler(notify_on_review_rejected())
        .handler(notify_on_return_approved())
        .handler(notify_on_return_refused())
        .handler(notify_on_return_completed());
    #[cfg(feature = "invoice-pdf")]
    let builder = builder.handler(invoice::send_on_invoice_issued());
    builder
}

/// The subscription data and whether the event is recent enough to be worth
/// an e-mail; `None` means "skip".
fn setup<E: Executor>(
    ctx: &Context<'_, E>,
    event_timestamp: u64,
) -> anyhow::Result<Option<(SqlitePool, MailerConfig, MailerTemplates)>> {
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
    let templates = ctx.get::<MailerTemplates>().unwrap_or_default();
    Ok(Some((db, config, templates)))
}

async fn queue(
    db: &SqlitePool,
    config: &MailerConfig,
    event_id: &str,
    kind: &str,
    to: &str,
    content: Content,
) -> anyhow::Result<()> {
    queue_with(db, config, event_id, kind, to, content, Vec::new()).await
}

async fn queue_with(
    db: &SqlitePool,
    config: &MailerConfig,
    event_id: &str,
    kind: &str,
    to: &str,
    content: Content,
    attachments: Vec<crate::email::Attachment>,
) -> anyhow::Result<()> {
    let message_id = timada_core::id::derived(&[event_id], kind);
    let email = Email {
        from: config.from.clone(),
        to: to.to_owned(),
        subject: content.subject,
        body: content.body,
        html_body: content.html_body,
        attachments,
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
    let Some((db, config, templates)) = setup(ctx, event.timestamp)? else {
        return Ok(());
    };
    let (order, to, first_name) = order_and_customer(ctx.executor, &event.aggregate_id).await?;
    let content = templates.0.order_confirmation(&config, &first_name, &order);
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
    let Some((db, config, templates)) = setup(ctx, event.timestamp)? else {
        return Ok(());
    };
    let (order, to, first_name) = order_and_customer(ctx.executor, &event.aggregate_id).await?;
    let content = templates.0.order_confirmation(&config, &first_name, &order);
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
    let Some((db, config, templates)) = setup(ctx, event.timestamp)? else {
        return Ok(());
    };
    let (order, to, first_name) = order_and_customer(ctx.executor, &event.aggregate_id).await?;
    let content = templates.0.order_shipped(&config, &first_name, &order);
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
    let Some((db, config, templates)) = setup(ctx, event.timestamp)? else {
        return Ok(());
    };
    let (order, to, first_name) = order_and_customer(ctx.executor, &event.aggregate_id).await?;
    let content = templates
        .0
        .order_cancelled(&config, &first_name, &order, &event.data.reason);
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
    let Some((db, config, templates)) = setup(ctx, event.timestamp)? else {
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
    let content = templates
        .0
        .refund(&config, &first_name, &order, &event.data.amount);
    queue(&db, &config, &event.id.to_string(), "refund", &to, content).await
}

/// Goes to the address given when the alert was asked for.
#[evento::subscription]
async fn notify_on_alert_triggered<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<BackInStockAlertTriggered>,
) -> anyhow::Result<()> {
    let Some((db, config, templates)) = setup(ctx, event.timestamp)? else {
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
    let content = templates
        .0
        .back_in_stock(&config, &alert.product_id, &product_name);
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
    let Some((db, config, templates)) = setup(ctx, event.timestamp)? else {
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
    let content = templates.0.question_answered(
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

/// The return and who asked for it: `(return, e-mail, first name)`.
async fn return_and_customer<E: Executor>(
    executor: &E,
    return_id: &str,
) -> anyhow::Result<(ReturnView, String, String)> {
    let Some(request) = timada_returns::load_return(executor, return_id).await? else {
        anyhow::bail!("return {return_id} cannot be loaded");
    };
    let Some(customer) = timada_customer::load_address_book(executor, &request.customer_id).await?
    else {
        anyhow::bail!(
            "customer {} of return {return_id} cannot be loaded",
            request.customer_id
        );
    };
    Ok((request, customer.email, customer.first_name))
}

#[evento::subscription]
async fn notify_on_return_approved<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ReturnApproved>,
) -> anyhow::Result<()> {
    let Some((db, config, templates)) = setup(ctx, event.timestamp)? else {
        return Ok(());
    };
    let (request, to, first_name) = return_and_customer(ctx.executor, &event.aggregate_id).await?;
    let content = templates.0.return_approved(&config, &first_name, &request);
    queue(
        &db,
        &config,
        &event.id.to_string(),
        "return-approved",
        &to,
        content,
    )
    .await
}

#[evento::subscription]
async fn notify_on_return_refused<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ReturnRefused>,
) -> anyhow::Result<()> {
    let Some((db, config, templates)) = setup(ctx, event.timestamp)? else {
        return Ok(());
    };
    let (request, to, first_name) = return_and_customer(ctx.executor, &event.aggregate_id).await?;
    let content = templates.0.return_refused(&config, &first_name, &request);
    queue(
        &db,
        &config,
        &event.id.to_string(),
        "return-refused",
        &to,
        content,
    )
    .await
}

#[evento::subscription]
async fn notify_on_return_completed<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ReturnCompleted>,
) -> anyhow::Result<()> {
    let Some((db, config, templates)) = setup(ctx, event.timestamp)? else {
        return Ok(());
    };
    let (request, to, first_name) = return_and_customer(ctx.executor, &event.aggregate_id).await?;
    let content = templates.0.return_completed(&config, &first_name, &request);
    queue(
        &db,
        &config,
        &event.id.to_string(),
        "return-completed",
        &to,
        content,
    )
    .await
}

/// A customer's answer went through moderation: tell whoever asked — unless
/// they answered their own question.
#[evento::subscription]
async fn notify_on_answer_published<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<AnswerPublished>,
) -> anyhow::Result<()> {
    let Some((db, config, templates)) = setup(ctx, event.timestamp)? else {
        return Ok(());
    };
    let Some(question) = timada_review::Command(ctx.executor)
        .load_question(&event.aggregate_id)
        .await?
    else {
        anyhow::bail!("question {} cannot be loaded", event.aggregate_id);
    };
    let Some(answer) = question
        .customer_answers
        .iter()
        .find(|a| a.answer_id == event.data.answer_id)
    else {
        anyhow::bail!(
            "answer {} published but not on question {}",
            event.data.answer_id,
            question.id
        );
    };
    if answer.customer_id == question.customer_id {
        return Ok(());
    }
    let Some(customer) =
        timada_customer::load_address_book(ctx.executor, &question.customer_id).await?
    else {
        return Ok(());
    };
    let product_name = timada_catalog::load_product_page(ctx.executor, &question.product_id)
        .await?
        .map_or_else(|| "un produit".to_owned(), |p| p.name);
    let content = templates.0.question_answered(
        &config,
        &customer.first_name,
        &question.product_id,
        &product_name,
        &question.body,
        &answer.body,
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

#[evento::subscription]
async fn welcome_on_customer_registered<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<CustomerRegistered>,
) -> anyhow::Result<()> {
    let Some((db, config, templates)) = setup(ctx, event.timestamp)? else {
        return Ok(());
    };
    let content = templates.0.welcome(&config, &event.data.first_name);
    queue(
        &db,
        &config,
        &event.id.to_string(),
        "welcome",
        &event.data.email,
        content,
    )
    .await
}

#[evento::subscription]
async fn notify_on_question_rejected<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<QuestionRejected>,
) -> anyhow::Result<()> {
    let Some((db, config, templates)) = setup(ctx, event.timestamp)? else {
        return Ok(());
    };
    let Some(question) = timada_review::Command(ctx.executor)
        .load_question(&event.aggregate_id)
        .await?
    else {
        anyhow::bail!("question {} cannot be loaded", event.aggregate_id);
    };
    let Some(customer) =
        timada_customer::load_address_book(ctx.executor, &question.customer_id).await?
    else {
        return Ok(());
    };
    let product_name = timada_catalog::load_product_page(ctx.executor, &question.product_id)
        .await?
        .map_or_else(|| "un produit".to_owned(), |p| p.name);
    let content = templates.0.question_refused(
        &config,
        &customer.first_name,
        &product_name,
        &question.body,
        &event.data.reason,
    );
    queue(
        &db,
        &config,
        &event.id.to_string(),
        "question-refused",
        &customer.email,
        content,
    )
    .await
}

/// The review, its author and the product's name: what both review e-mails need.
async fn review_and_customer<E: Executor>(
    executor: &E,
    review_id: &str,
) -> anyhow::Result<Option<(timada_review::ReviewView, String, String, String)>> {
    let Some(review) = timada_review::load_review_details(executor, review_id).await? else {
        anyhow::bail!("review {review_id} cannot be loaded");
    };
    let Some(customer) = timada_customer::load_address_book(executor, &review.customer_id).await?
    else {
        // Written by someone who is not a registered customer: nobody to tell.
        return Ok(None);
    };
    let product_name = timada_catalog::load_product_page(executor, &review.product_id)
        .await?
        .map_or_else(|| "un produit".to_owned(), |p| p.name);
    Ok(Some((
        review,
        customer.email,
        customer.first_name,
        product_name,
    )))
}

#[evento::subscription]
async fn notify_on_review_published<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ReviewPublished>,
) -> anyhow::Result<()> {
    let Some((db, config, templates)) = setup(ctx, event.timestamp)? else {
        return Ok(());
    };
    let Some((review, to, first_name, product_name)) =
        review_and_customer(ctx.executor, &event.aggregate_id).await?
    else {
        return Ok(());
    };
    let content =
        templates
            .0
            .review_published(&config, &first_name, &review.product_id, &product_name);
    queue(
        &db,
        &config,
        &event.id.to_string(),
        "review-published",
        &to,
        content,
    )
    .await
}

#[evento::subscription]
async fn notify_on_review_rejected<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ReviewRejected>,
) -> anyhow::Result<()> {
    let Some((db, config, templates)) = setup(ctx, event.timestamp)? else {
        return Ok(());
    };
    let Some((_, to, first_name, product_name)) =
        review_and_customer(ctx.executor, &event.aggregate_id).await?
    else {
        return Ok(());
    };
    let content =
        templates
            .0
            .review_rejected(&config, &first_name, &product_name, &event.data.reason);
    queue(
        &db,
        &config,
        &event.id.to_string(),
        "review-rejected",
        &to,
        content,
    )
    .await
}

#[cfg(feature = "invoice-pdf")]
mod invoice {
    use evento::{Executor, metadata::Event, subscription::Context};
    use timada_invoice::{
        ArchivePolicy, InvoiceArchive, InvoiceIssuer, aggregator::InvoiceIssued, archive_invoice,
        invoice_document, invoice_pdf_file_name, load_invoice, render_invoice_pdf,
    };

    use super::{order_and_customer, queue_with, setup};
    use crate::email::Attachment;

    /// `InvoiceIssued` → the invoice, as issued, in the customer's mailbox.
    /// Opt-in: without an [`InvoiceIssuer`] in the subscription data there is
    /// nobody to put at the top of the document, and nothing is sent. With an
    /// [`InvoiceArchive`] in the data too, what is sent is the archived file —
    /// filed here if the archive has not caught up — so the customer's
    /// mailbox and their account hold the same bytes.
    #[evento::subscription]
    pub(super) async fn send_on_invoice_issued<E: Executor>(
        ctx: &Context<'_, E>,
        event: Event<InvoiceIssued>,
    ) -> anyhow::Result<()> {
        let Some((db, config, templates)) = setup(ctx, event.timestamp)? else {
            return Ok(());
        };
        let Some(issuer) = ctx.get::<InvoiceIssuer>() else {
            tracing::warn!(
                invoice_id = %event.aggregate_id,
                "no InvoiceIssuer in the mailer's data: invoice not e-mailed"
            );
            return Ok(());
        };
        let Some(invoice) = load_invoice(ctx.executor, &event.aggregate_id).await? else {
            anyhow::bail!("invoice {} cannot be loaded", event.aggregate_id);
        };
        // The order itself gives its number: no read model to wait for.
        let (order, to, first_name) = order_and_customer(ctx.executor, &invoice.order_id).await?;
        let number = order.order_number.clone();
        // As issued: credit notes come later and have their own e-mail.
        let Some(document) = invoice_document(&issuer, invoice, number, Vec::new())? else {
            // Voided since: there is no invoice to send any more.
            return Ok(());
        };

        let content = templates.0.invoice_issued(&config, &first_name, &document);
        let file_name = invoice_pdf_file_name(&document);
        let archived = match ctx.get::<InvoiceArchive>() {
            Some(archive) => {
                let policy = ctx.get::<ArchivePolicy>().unwrap_or_default();
                archive_invoice(
                    ctx.executor,
                    &db,
                    archive.0.as_ref(),
                    &issuer,
                    &event.aggregate_id,
                    &policy,
                )
                .await?
                .map(|(_, bytes)| bytes)
            }
            None => None,
        };
        let pdf = match archived {
            Some(bytes) => bytes,
            None => tokio::task::spawn_blocking(move || render_invoice_pdf(&document)).await??,
        };
        queue_with(
            &db,
            &config,
            &event.id.to_string(),
            "invoice-issued",
            &to,
            content,
            vec![Attachment::pdf(file_name, pdf)],
        )
        .await
    }
}
