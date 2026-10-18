//! `/t/{slug}/payments`: how shoppers pay this tenant, for members with
//! `payments:manage`. The merchant connects its own Stripe account: the
//! publishable key (public), the secret key (checked against Stripe, then
//! kept sealed) and the signing secret of the webhook it registers at the
//! address shown here.

pub mod configure;
pub mod remove;

use std::sync::Arc;

use timada_tenancy::PERMISSION_PAYMENTS_MANAGE;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params},
    view::{View, view},
};

use super::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::app::auth::Flash;
use crate::config::Config;
use crate::payments::Payments;
use crate::ui::{Shell, document, format_time, notice};

#[topcoat::router::query_params(error = bad_request)]
struct PaymentsQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn payments_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, Slug(slug.to_owned()))
        .query(Flash { error, message })
        .resolve(cx)
}

/// Tells the tenant's store what its payment settings now are, so a
/// storefront shows the payment form (or stops) at once. A failure only
/// delays it: the payment-setup subscription writes it too.
pub async fn publish_setup(cx: &Cx, access: &TenantAccess) {
    let payments = app_context::<Arc<Payments>>(cx);
    let published = async {
        let Some(tenant) = payments.active_tenant(&access.tenant.id).await? else {
            return Ok(());
        };
        let executor = access.executor().map_err(|err| anyhow::anyhow!("{err}"))?;
        crate::payments::publish_setup(payments, &tenant, executor.clone()).await
    };
    if let Err(err) = published.await {
        tracing::warn!(tenant = %access.tenant.slug, %err, "cannot tell the store how its shoppers pay");
    }
}

/// Where Stripe sends this tenant's payment notices.
pub fn webhook_url(cx: &Cx, tenant_id: &str) -> String {
    format!(
        "{}/stripe/webhooks/{tenant_id}",
        app_context::<Config>(cx).server.url.trim_end_matches('/')
    )
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PAYMENTS_MANAGE)
        .await?;
    let query = query_params::<PaymentsQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let TenantAccess { tenant, .. } = access;
    let enabled = app_context::<Arc<Payments>>(cx).enabled();
    let webhook = webhook_url(cx, &tenant.id);
    let slug = tenant.slug.clone();
    let settings = tenant.payments.clone();
    let title = format!("Payments · {}", tenant.name);

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "payments"),
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">"Payments"</h1>
            </header>
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            if !enabled {
                <p role="alert" class="mb-4 rounded border border-amber-300 bg-amber-50 p-3 text-sm text-amber-900">
                    "Payments are not enabled on this platform: the operator has to set them up first."
                </p>
            }

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <h2 class="mb-2 text-lg font-medium">"Stripe"</h2>
                if let Some(settings) = &settings {
                    <dl class="mb-4 grid grid-cols-[12rem_1fr] gap-y-1 text-sm" data-payments="stripe">
                        <dt class="text-neutral-500">"Publishable key"</dt>
                        <dd><code data-publishable-key="">(&settings.publishable_key)</code></dd>
                        <dt class="text-neutral-500">"Secret key"</dt><dd>"Saved, never shown again"</dd>
                        <dt class="text-neutral-500">"Webhook signing secret"</dt><dd>"Saved, never shown again"</dd>
                        <dt class="text-neutral-500">"Since"</dt>
                        <dd><time datetime=(settings.configured_at)>(format_time(settings.configured_at))</time></dd>
                    </dl>
                    <form method="post" action=(href!(remove::remove, Slug(slug.clone())))>
                        <button class="rounded border border-red-300 px-3 py-1 text-sm text-red-800 hover:bg-red-50">"Stop taking payments"</button>
                    </form>
                } else {
                    <p class="mb-2 text-sm" data-payments="none">"Not set up: shoppers cannot pay yet."</p>
                }
            </section>

            <section class="rounded border border-neutral-200 bg-white p-4">
                <h2 class="mb-2 text-lg font-medium">
                    if settings.is_some() { "Replace the keys" } else { "Connect a Stripe account" }
                </h2>
                <ol class="mb-4 list-decimal space-y-1 pl-5 text-sm text-neutral-600">
                    <li>"In the Stripe dashboard, under Developers › API keys, copy the publishable and the secret key."</li>
                    <li>
                        "Under Developers › Webhooks, add an endpoint at "
                        <code data-webhook-url="">(&webhook)</code>
                        " for the event "<code>"payment_intent.succeeded"</code>", and copy its signing secret."
                    </li>
                </ol>
                <form method="post" action=(href!(configure::configure, Slug(slug.clone()))) class="space-y-3">
                    <label class="block text-sm" for="payments-publishable">"Publishable key"</label>
                    <input id="payments-publishable" name="publishable_key" required="" placeholder="pk_live_…"
                        class="w-full rounded border border-neutral-300 px-3 py-2 font-mono">
                    <label class="block text-sm" for="payments-secret">"Secret key"</label>
                    <input id="payments-secret" name="secret_key" type="password" required="" placeholder="sk_live_…" autocomplete="off"
                        class="w-full rounded border border-neutral-300 px-3 py-2 font-mono">
                    <label class="block text-sm" for="payments-webhook">"Webhook signing secret"</label>
                    <input id="payments-webhook" name="webhook_secret" type="password" required="" placeholder="whsec_…" autocomplete="off"
                        class="w-full rounded border border-neutral-300 px-3 py-2 font-mono">
                    <button class="rounded bg-neutral-900 px-4 py-2 font-medium text-white hover:bg-neutral-700">"Save"</button>
                </form>
            </section>
        )
    })
}
