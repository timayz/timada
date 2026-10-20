//! `/t/{slug}/newsletter/campaigns/new`, for members with
//! `newsletter:manage`: a campaign written from the built-in start. Saving
//! it keeps it as a draft and opens its own page.

use timada_newsletter::CampaignDraft;
use timada_tenancy::PERMISSION_NEWSLETTER_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::see_other, href, page},
    view::{View, view},
};

use super::campaign_id::campaign_url;
use super::{CampaignForm, KIND, campaign_editor, campaigns_url, preview, send_test};
use crate::app::t::slug::{Slug, TenantNav, require_tenant};
use crate::components::button::{ButtonSize, ButtonVariant, button_variants};
use crate::ui::{Shell, document, notice, page_header};

#[page]
pub async fn new(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_NEWSLETTER_MANAGE)
        .await?;
    let nav = TenantNav::of(cx, &access).await?;
    let draft = CampaignDraft {
        subject: KIND.default_subject().to_owned(),
        html: KIND.default_html().to_owned(),
        text: None,
        language: None,
    };
    let checked = preview(cx, &access.tenant, &draft);
    let tenant = access.tenant;
    let title = format!("New campaign · Newsletter · {}", tenant.name);
    let action = href!(create, Slug(tenant.slug.clone())).resolve(cx);
    let back = campaigns_url(cx, &tenant.slug, None, None);
    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "newsletter"),
            page_header(
                title: "New campaign",
                subtitle: Some("Write it, preview it, send yourself a test; it goes to nobody until you send it."),
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(back)>"Back to Campaigns"</a>
            )
            campaign_editor(tenant: &tenant, action: action, draft: &draft, checked: &checked)
        )
    })
}

/// Previews, tests or saves the draft. A saved draft opens its own page;
/// otherwise the page shows the draft as it was sent, so nothing typed is
/// lost.
#[page(POST)]
pub async fn create(cx: &Cx, Form(form): Form<CampaignForm>) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_NEWSLETTER_MANAGE)
        .await?;
    let draft = form.draft();
    let checked = preview(cx, &access.tenant, &draft);
    let mut error = None;
    let mut message = None;
    match form.action.as_str() {
        "save" if checked.is_ok() => {
            match super::super::newsletter(&access)?
                .draft_campaign(&access.user_id, draft.clone())
                .await
            {
                Ok(id) => {
                    return Err(see_other(campaign_url(
                        cx,
                        &access.tenant.slug,
                        &id,
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
    let title = format!("New campaign · Newsletter · {}", tenant.name);
    let action = href!(create, Slug(tenant.slug.clone())).resolve(cx);
    let back = campaigns_url(cx, &tenant.slug, None, None);
    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "newsletter"),
            page_header(
                title: "New campaign",
                subtitle: Some("Write it, preview it, send yourself a test; it goes to nobody until you send it."),
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(back)>"Back to Campaigns"</a>
            )
            notice(error: error, message: message.as_deref())
            campaign_editor(tenant: &tenant, action: action, draft: &draft, checked: &checked)
        )
    })
}
