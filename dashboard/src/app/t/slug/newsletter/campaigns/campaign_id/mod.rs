//! `/t/{slug}/newsletter/campaigns/{campaign_id}`: one campaign. A draft is
//! written here by members with `newsletter:manage`, and sent from
//! [`send`]; one sent shows how far it went out, and its copy.

pub mod send;

use timada_newsletter::{CampaignDraft, CampaignState, CampaignStatus, campaign_row};
use timada_tenancy::{PERMISSION_NEWSLETTER_MANAGE, PERMISSION_NEWSLETTER_READ};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::Form,
        error::{not_found, see_other},
        href, module_param, page, path_param, query_params,
    },
    view::{View, view},
};

use super::{
    CampaignForm, audience_label, campaign_editor, campaign_preview, campaigns_url, preview,
    send_test, status_label,
};
use crate::app::auth::Flash;
use crate::app::t::slug::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::components::button::{ButtonSize, ButtonVariant, button_variants};
use crate::db::ReadPool;
use crate::ui::{Shell, document, fact, facts, format_time, notice, page_header, section_card};

module_param!(pub campaign_id);

#[topcoat::router::query_params(error = bad_request)]
struct CampaignQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn campaign_url(
    cx: &Cx,
    slug: &str,
    campaign_id: &str,
    error: Option<&str>,
    message: Option<&str>,
) -> String {
    href!(
        index,
        Slug(slug.to_owned()),
        CampaignId(campaign_id.to_owned())
    )
    .query(Flash { error, message })
    .resolve(cx)
}

/// The campaign the URL names, or 404.
pub async fn require_campaign(access: &TenantAccess, cx: &Cx) -> Result<CampaignState> {
    let id = path_param::<CampaignId>(cx);
    super::super::newsletter(access)?
        .load_campaign(id)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .ok_or_else(|| not_found().into())
}

fn draft_of(campaign: &CampaignState) -> CampaignDraft {
    CampaignDraft {
        subject: campaign.subject.clone(),
        html: campaign.html.clone(),
        text: campaign.text.clone(),
        language: campaign.language.clone(),
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_NEWSLETTER_READ)
        .await?;
    let query = query_params::<CampaignQuery>(cx)?;
    let campaign = require_campaign(&access, cx).await?;
    let can_manage = access.can(cx, PERMISSION_NEWSLETTER_MANAGE).await?;
    let nav = TenantNav::of(cx, &access).await?;
    let draft = draft_of(&campaign);
    let checked = preview(cx, &access.tenant, &draft);
    let row = campaign_row(app_context::<ReadPool>(cx), &access.tenant.id, &campaign.id)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let (delivered, failed) = row.map_or((0, 0), |row| (row.delivered, row.failed));
    let tenant = access.tenant;
    let slug = tenant.slug.clone();
    let title = format!("{} · Newsletter · {}", campaign.subject, tenant.name);
    let editable = can_manage && campaign.status == CampaignStatus::Draft;
    let action = href!(submit, Slug(slug.clone()), CampaignId(campaign.id.clone())).resolve(cx);
    let back = campaigns_url(cx, &slug, None, None);
    let send = href!(
        send::send,
        Slug(slug.clone()),
        CampaignId(campaign.id.clone())
    )
    .resolve(cx);
    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "newsletter"),
            page_header(
                title: &campaign.subject,
                subtitle: Some(status_label(campaign.status)),
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(back)>"Back to Campaigns"</a>
                if editable {
                    <a class=(button_variants(ButtonVariant::Primary, ButtonSize::Sm)) href=(send)>"Send…"</a>
                }
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())
            if editable {
                campaign_editor(tenant: &tenant, action: action, draft: &draft, checked: &checked)
            } else {
                section_card(
                    facts(
                        fact(term: "Sent to", (audience_label(campaign.language.as_deref())))
                        fact(term: "Status", <span data-campaign-status=(campaign.status.as_str())>(status_label(campaign.status))</span>)
                        if let Some(at) = campaign.sent_at {
                            fact(term: "Sent", <time datetime=(at)>(format_time(at))</time>)
                        }
                        fact(term: "Mailed", <span data-campaign-delivered=(delivered)>(delivered)</span>)
                        if failed > 0 {
                            fact(term: "Not delivered", <span data-campaign-failed=(failed)>(failed)</span>)
                        }
                    )
                )
                campaign_preview(checked: &checked)
            }
        )
    })
}

/// Previews, tests or saves the draft; see the new campaign's page.
#[page(POST)]
pub async fn submit(cx: &Cx, Form(form): Form<CampaignForm>) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_NEWSLETTER_MANAGE)
        .await?;
    let campaign = require_campaign(&access, cx).await?;
    let draft = form.draft();
    let checked = preview(cx, &access.tenant, &draft);
    let mut error = None;
    let mut message = None;
    match form.action.as_str() {
        "save" if checked.is_ok() => {
            match super::super::newsletter(&access)?
                .edit_campaign(&access.user_id, &campaign.id, draft.clone())
                .await
            {
                Ok(()) => {
                    return Err(see_other(campaign_url(
                        cx,
                        &access.tenant.slug,
                        &campaign.id,
                        None,
                        Some("Draft saved."),
                    ))
                    .into());
                }
                Err(err) if err.is_user_facing() => error = err.code(),
                Err(err) => return Err(topcoat::Error::from_anyhow(err.into())),
            }
        }
        "save" => error = Some("invalid_mail_template"),
        "test" => {
            if let Ok(rendered) = &checked {
                match send_test(cx, &access.tenant, rendered).await? {
                    Ok(to) => message = Some(format!("Test mail sent to {to}.")),
                    Err(code) => error = Some(code),
                }
            }
        }
        _ => {}
    }
    let nav = TenantNav::of(cx, &access).await?;
    let tenant = access.tenant;
    let slug = tenant.slug.clone();
    let title = format!("{} · Newsletter · {}", campaign.subject, tenant.name);
    let action = href!(submit, Slug(slug.clone()), CampaignId(campaign.id.clone())).resolve(cx);
    let back = campaigns_url(cx, &slug, None, None);
    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "newsletter"),
            page_header(
                title: &campaign.subject,
                subtitle: Some(status_label(campaign.status)),
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(back)>"Back to Campaigns"</a>
            )
            notice(error: error, message: message.as_deref())
            campaign_editor(tenant: &tenant, action: action, draft: &draft, checked: &checked)
        )
    })
}
