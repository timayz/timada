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
    icon::icon,
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use super::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::app::auth::Flash;
use crate::components::alert::{AlertVariant, alert, alert_description};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::field::{field, field_label};
use crate::components::input::input;
use crate::config::Config;
use crate::db::Db;
use crate::payments::Payments;
use crate::ui::{
    Shell, Tone, document, fact, facts, format_time, icons, notice, page_header, section_card,
    status_badge,
};

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
    // Stopped, the keys stay until what is held or owed is settled.
    let stopped_note = match settings.as_ref().filter(|settings| settings.is_stopped()) {
        Some(_) => Some(stopped_note(
            timada_orders::money_owed(app_context::<Db>(cx), &tenant.id)
                .await
                .map_err(topcoat::Error::from_anyhow)?,
        )),
        None => None,
    };
    let title = format!("Payments · {}", tenant.name);

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "payments"),
            page_header(title: "Payments", subtitle: Some("How shoppers pay this tenant: its own Stripe account."))
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            if !enabled {
                alert(
                    variant: AlertVariant::Warning,
                    attrs: attributes! { role="alert" class="mb-6" },
                    icon(data: icons::WARNING)
                    alert_description("Payments are not enabled on this platform: the operator has to set them up first.")
                )
            }

            section_card(
                title: Some("Stripe"),
                if let Some(settings) = &settings {
                    <div class="mb-4" data-payments="stripe">
                        facts(
                            if stopped_note.is_some() {
                                fact(term: "Status", status_badge(tone: Tone::Warning, attrs: attributes! { data-payments-stopped="" }, "Stopped"))
                            } else {
                                fact(term: "Status", status_badge(tone: Tone::Success, "Connected"))
                            }
                            fact(term: "Publishable key", <code data-publishable-key="">(&settings.publishable_key)</code>)
                            fact(term: "Secret key", "Saved, never shown again")
                            fact(term: "Webhook signing secret", "Saved, never shown again")
                            fact(term: "Since", <time datetime=(settings.configured_at)>(format_time(settings.configured_at))</time>)
                        )
                    </div>
                    if let Some(note) = &stopped_note {
                        <p class="text-sm text-muted-foreground" data-payments-forgetting="">(note)</p>
                    } else {
                        <form method="post" action=(href!(remove::remove, Slug(slug.clone())))>
                            button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Stop taking payments")
                        </form>
                    }
                } else {
                    <p class="text-sm text-muted-foreground" data-payments="none">"Not set up: shoppers cannot pay yet."</p>
                }
            )

            section_card(
                title: Some(match &settings {
                    Some(_) if stopped_note.is_some() => "Take payments again",
                    Some(_) => "Replace the keys",
                    None => "Connect a Stripe account",
                }),
                <ol class="mb-4 list-decimal space-y-1 pl-5 text-sm text-muted-foreground">
                    <li>"In the Stripe dashboard, under Developers › API keys, copy the publishable and the secret key."</li>
                    <li>
                        "Under Developers › Webhooks, add an endpoint at "
                        <code class="text-foreground" data-webhook-url="">(&webhook)</code>
                        " for the events "
                        <code class="text-foreground">"payment_intent.amount_capturable_updated"</code>", "
                        <code class="text-foreground">"payment_intent.succeeded"</code>", "
                        <code class="text-foreground">"payment_intent.canceled"</code>" and "
                        <code class="text-foreground">"charge.refunded"</code>
                        ", and copy its signing secret."
                    </li>
                </ol>
                <form method="post" action=(href!(configure::configure, Slug(slug.clone()))) class="flex max-w-xl flex-col gap-4">
                    field(
                        field_label(attrs: attributes! { for="payments-publishable" }, "Publishable key")
                        input(attrs: attributes! {
                            id="payments-publishable" name="publishable_key" required="" placeholder="pk_live_…" class="font-mono"
                        })
                    )
                    field(
                        field_label(attrs: attributes! { for="payments-secret" }, "Secret key")
                        input(attrs: attributes! {
                            id="payments-secret" name="secret_key" type="password" required="" placeholder="sk_live_…" autocomplete="off" class="font-mono"
                        })
                    )
                    field(
                        field_label(attrs: attributes! { for="payments-webhook" }, "Webhook signing secret")
                        input(attrs: attributes! {
                            id="payments-webhook" name="webhook_secret" type="password" required="" placeholder="whsec_…" autocomplete="off" class="font-mono"
                        })
                    )
                    <div>
                        button(attrs: attributes! { type="submit" }, "Save")
                    </div>
                </form>
            )
        )
    })
}

/// What the page says of a tenant that stopped taking payments, `owed`
/// payments still held or owed.
fn stopped_note(owed: i64) -> String {
    match owed {
        0 => "Payments stopped: nothing is held any more, and the keys are forgotten at the next check.".to_owned(),
        1 => "Payments stopped: the keys are forgotten once the payment still held is settled.".to_owned(),
        n => format!("Payments stopped: the keys are forgotten once the {n} payments still held are settled."),
    }
}
