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
use crate::payments::Payments;

#[derive(Deserialize)]
pub struct ConfigureForm {
    publishable_key: String,
    secret_key: String,
    webhook_secret: String,
}

/// Saves the Stripe account once Stripe accepts its secret key.
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
