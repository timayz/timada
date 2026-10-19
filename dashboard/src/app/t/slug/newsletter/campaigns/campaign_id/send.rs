//! `/t/{slug}/newsletter/campaigns/{campaign_id}/send`, for members with
//! `newsletter:manage`: asks to confirm, with how many subscribers it goes
//! to, then sends the draft. From then on every node with the shop open
//! mails it a batch at a time, and it can no longer be edited.

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

use super::{CampaignId, campaign_url, require_campaign};
use crate::app::t::slug::newsletter::campaigns::audience_label;
use crate::app::t::slug::{Slug, TenantNav, require_tenant};
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::db::ReadPool;
use crate::ui::{Shell, document, page_header, section_card};

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
        app_context::<ReadPool>(cx),
        &access.tenant.id,
        campaign.language.as_deref(),
    )
    .await
    .map_err(topcoat::Error::from_anyhow)?;
    let nav = TenantNav::of(cx, &access).await?;
    let tenant = access.tenant;
    let title = format!("Send · {} · Newsletter · {}", campaign.subject, tenant.name);
    let to = audience_label(campaign.language.as_deref());
    let label = format!("Send to {audience} subscriber(s)");
    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "newsletter"),
            page_header(title: "Send the campaign", subtitle: Some(campaign.subject.as_str()))
            section_card(
                <p class="mb-4 text-sm" data-campaign-audience=(audience)>
                    (to) ": " <strong>(audience)</strong> " confirmed address(es) today. "
                    "It goes out from the shop over the next minutes, and can no longer be edited."
                </p>
                <div class="flex flex-wrap gap-2">
                    <form method="post" action=(href!(confirm, Slug(slug.clone()), CampaignId(campaign.id.clone())))>
                        button(attrs: attributes! { type="submit" }, (label))
                    </form>
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
    match crate::app::t::slug::newsletter::newsletter(&access)?
        .send_campaign(&access.user_id, &campaign.id)
        .await
    {
        Ok(()) => Ok(see_other(campaign_url(
            cx,
            &slug,
            &campaign.id,
            None,
            Some("Sending: the subscribers get it over the next minutes."),
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
