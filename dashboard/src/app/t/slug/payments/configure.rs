use std::sync::Arc;

use serde::Deserialize;
use timada_orders::PROVIDER_STRIPE;
use timada_tenancy::{ConfigurePayments, PERMISSION_PAYMENTS_MANAGE};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{payments_url, publish_setup};
use crate::app::t::slug::require_tenant;
use crate::app::tenants::{error_code, tenancy};
use crate::db::Db;
use crate::payments::Payments;

#[derive(Deserialize)]
pub struct ConfigureForm {
    publishable_key: String,
    secret_key: String,
    webhook_secret: String,
}

/// Saves the Stripe account once Stripe accepts its secret key; after a
/// stop, it takes payments again. Another account is refused while anything
/// is held or owed under the current one: another publishable key, or a
/// secret key that cannot read a payment still held.
#[route(POST)]
pub async fn configure(cx: &Cx, Form(form): Form<ConfigureForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PAYMENTS_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let payments = app_context::<Arc<Payments>>(cx);
    if !payments.enabled() {
        return Ok(see_other(payments_url(
            cx,
            slug,
            Some("payments_disabled"),
            None,
        )));
    }
    let secret_key = form.secret_key.trim();
    let webhook_secret = form.webhook_secret.trim();
    if !(secret_key.starts_with("sk_") || secret_key.starts_with("rk_")) {
        return Ok(see_other(payments_url(
            cx,
            slug,
            Some("invalid_secret_key"),
            None,
        )));
    }
    if !webhook_secret.starts_with("whsec_") {
        return Ok(see_other(payments_url(
            cx,
            slug,
            Some("invalid_webhook_secret"),
            None,
        )));
    }
    if let Err(err) = payments.gateway.check_key(secret_key).await {
        tracing::info!(tenant = slug, %err, "Stripe refused a secret key");
        return Ok(see_other(payments_url(
            cx,
            slug,
            Some("stripe_refused_key"),
            None,
        )));
    }
    // What is still held or owed is settled under the account it was paid
    // through: until it is, another account's keys would strand it. The
    // same account's keys (a rolled secret key) are fine: the new key must
    // read one of the payments still held, and keep the publishable key.
    if let Some(current) = &access.tenant.payments {
        let db = app_context::<Db>(cx);
        let owed = timada_orders::money_owed(db, &access.tenant.id)
            .await
            .map_err(topcoat::Error::from_anyhow)?;
        let other_account = owed > 0
            && (current.publishable_key != form.publishable_key.trim()
                || match timada_orders::owed_payment_id(db, &access.tenant.id)
                    .await
                    .map_err(topcoat::Error::from_anyhow)?
                {
                    Some(payment_id) => !payments
                        .reads_payment(secret_key, &payment_id)
                        .await
                        .map_err(topcoat::Error::from_anyhow)?,
                    None => false,
                });
        if other_account {
            return Ok(see_other(payments_url(
                cx,
                slug,
                Some("account_in_use"),
                None,
            )));
        }
    }
    let cmd = ConfigurePayments {
        provider: PROVIDER_STRIPE.to_owned(),
        publishable_key: form.publishable_key,
        secret_key_sealed: payments
            .seal(&access.tenant.id, secret_key)
            .map_err(topcoat::Error::from_anyhow)?,
        webhook_secret_sealed: payments
            .seal(&access.tenant.id, webhook_secret)
            .map_err(topcoat::Error::from_anyhow)?,
    };
    match tenancy(cx)
        .configure_payments(&access.user_id, &access.tenant.id, cmd)
        .await
    {
        Ok(()) => {
            publish_setup(cx, &access).await;
            Ok(see_other(payments_url(
                cx,
                slug,
                None,
                Some("Stripe connected."),
            )))
        }
        Err(err) if err.is_user_facing() => Ok(see_other(payments_url(
            cx,
            slug,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(err.into()),
    }
}
