//! `/t/{slug}/newsletter/campaigns/{campaign_id}/send`, for members with
//! `newsletter:manage`: asks to confirm, with how many subscribers it goes
//! to, then sends the draft. From then on every node with the shop open
//! mails it a batch at a time, and it can no longer be edited.
//!
//! A campaign with a schedule is not sent but started: its editions then go
//! out on the schedule's days.
//!
//! A campaign goes out through the mail server the shop picked for its
//! marketing only, never the platform's: a shop that picked none is sent to
//! `/t/{slug}/mail` instead.

use timada_newsletter::{CampaignStatus, count_audience};
use timada_tenancy::PERMISSION_NEWSLETTER_MANAGE;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        error::{SeeOther, see_other},
        href, page, route,
    },
    view::{View, attributes, view},
};

use super::schedule::{describe, pacing, shop_today, upcoming};
use super::{CampaignId, campaign_url, require_campaign};
use crate::app::t::slug::newsletter::campaigns::audience_label;
use crate::app::t::slug::{Slug, TenantNav, require_tenant};
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::db::Db;
use crate::shop_mail::ShopMail;
use crate::ui::{Shell, document, notice, page_header, section_card};

#[page]
pub async fn send(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_NEWSLETTER_MANAGE)
        .await?;
    let campaign = require_campaign(&access, cx).await?;
    let slug = access.tenant.slug.clone();
    let back = campaign_url(cx, &slug, &campaign.id, None, None);
    if campaign.status != CampaignStatus::Draft {
        return Err(see_other(back).into());
    }
    let audience = count_audience(
        app_context::<Db>(cx),
        &access.tenant.id,
        campaign.language.as_deref(),
    )
    .await
    .map_err(topcoat::Error::from_anyhow)?;
    let nav = TenantNav::of(cx, &access).await?;
    let tenant = access.tenant;
    let shop_mail = app_context::<ShopMail>(cx);
    // The server it goes through, as the merchant knows it.
    let through = tenant
        .marketing_server()
        .filter(|_| shop_mail.has_marketing_server(&tenant))
        .map(|server| match &server.name {
            Some(name) => format!("{name} <{}>", server.from_address),
            None => server.from_address.clone(),
        });
    let own_server = through.is_some();
    let set_up = if tenant.mail_servers.is_empty() {
        "Set up the mail server"
    } else {
        "Pick a server for marketing"
    };
    let mail_settings = href!(crate::app::t::slug::mail::index, Slug(slug.clone())).resolve(cx);
    let title = format!("Send · {} · Newsletter · {}", campaign.subject, tenant.name);
    let to = audience_label(campaign.language.as_deref());
    let shop_zone = tenant.shop_time_zone().to_owned();
    let plan = campaign.schedule.as_ref().map(|plan| {
        (
            describe(plan, &shop_zone),
            pacing(plan, audience),
            upcoming(plan, shop_today(&shop_zone)),
        )
    });
    let (heading, label, target) = match &plan {
        Some(_) => (
            "Start the schedule",
            "Start the schedule".to_owned(),
            href!(confirm, Slug(slug.clone()), CampaignId(campaign.id.clone())).resolve(cx),
        ),
        None => (
            "Send the campaign",
            format!("Send to {audience} subscriber(s)"),
            href!(confirm, Slug(slug.clone()), CampaignId(campaign.id.clone())).resolve(cx),
        ),
    };
    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "newsletter"),
            page_header(title: heading, subtitle: Some(campaign.subject.as_str()))
            if !own_server {
                notice(error: Some("mail_server_required"), message: None)
            }
            section_card(
                <p class="mb-4 text-sm" data-campaign-audience=(audience)>
                    (to) ": " <strong>(audience)</strong> " confirmed address(es) today. "
                    if let Some((summary, pacing, next)) = &plan {
                        <span data-schedule-summary="">(summary)</span> " " (pacing)
                        if !next.is_empty() { " Next: " (next.join(", ")) "." }
                        " Each edition goes to the subscribers confirmed by then"
                        if let Some(through) = &through {
                            ", through " <strong data-campaign-server="">(through)</strong>
                        }
                        ". Its copy can still change for the editions to come."
                    } else if let Some(through) = &through {
                        "It goes out through " <strong data-campaign-server="">(through)</strong>
                        " over the next minutes, and can no longer be edited."
                    } else {
                        "It goes out through the server picked for marketing over the next minutes, and can no longer be edited."
                    }
                </p>
                <div class="flex flex-wrap gap-2">
                    if own_server {
                        <form method="post" action=(&target)>
                            button(attrs: attributes! { type="submit" }, (label))
                        </form>
                    } else {
                        <a class=(button_variants(ButtonVariant::Primary, ButtonSize::Md)) href=(mail_settings) data-mail-settings="">(set_up)</a>
                    }
                    <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Md)) href=(back)>"Not yet"</a>
                </div>
            )
        )
    })
}

#[route(POST)]
pub async fn confirm(cx: &Cx) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_NEWSLETTER_MANAGE)
        .await?;
    let campaign = require_campaign(&access, cx).await?;
    let slug = access.tenant.slug.clone();
    if !app_context::<ShopMail>(cx).has_marketing_server(&access.tenant) {
        return Ok(see_other(campaign_url(
            cx,
            &slug,
            &campaign.id,
            Some("mail_server_required"),
            None,
        )));
    }
    let newsletter = crate::app::t::slug::newsletter::newsletter(&access)?;
    let (done, message) = if campaign.schedule.is_some() {
        let today = shop_today(access.tenant.shop_time_zone());
        (
            newsletter
                .activate_schedule(&access.user_id, &campaign.id, today)
                .await,
            "Scheduled: each edition goes out on the schedule's days.",
        )
    } else {
        (
            newsletter
                .send_campaign(&access.user_id, &campaign.id)
                .await,
            "Sending: the subscribers get it over the next minutes.",
        )
    };
    match done {
        Ok(()) => Ok(see_other(campaign_url(
            cx,
            &slug,
            &campaign.id,
            None,
            Some(message),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(campaign_url(
            cx,
            &slug,
            &campaign.id,
            err.code(),
            None,
        ))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
