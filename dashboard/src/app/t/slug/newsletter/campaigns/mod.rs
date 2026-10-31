//! `/t/{slug}/newsletter/campaigns`: the mails of news the merchant writes and
//! sends to the confirmed subscribers, for members with `newsletter:read`;
//! those with `newsletter:manage` write, test and send them. A campaign is
//! a template over the shop's variables and the subscriber's unsubscribe
//! link, checked when saved; once sent, it goes out a batch at a time and
//! can no longer be edited. A scheduled one goes out an edition per cycle
//! of its schedule, and its copy may change for the editions to come.
//!
//! The preview is the merchant's own HTML. It sits in a sandboxed frame with
//! no scripts and an origin of its own, so nothing in it can reach the
//! dashboard.

pub mod campaign_id;
pub mod new;

use serde::Deserialize;
use timada_newsletter::{CampaignDraft, CampaignStatus, campaigns_of_tenant, count_campaigns};
use timada_tenancy::{
    MAX_MAIL_SUBJECT_LEN, MailTemplateError, PERMISSION_NEWSLETTER_MANAGE,
    PERMISSION_NEWSLETTER_READ, RenderedMail, ShopMailKind, TenantState, language_name, links_to,
    render, render_text, validate, validate_text,
};
use topcoat::{
    Result,
    context::{Cx, app_context},
    icon::icon,
    router::{href, page, query_params},
    view::{View, attributes, component, view},
};

use crate::app::auth::Flash;
use crate::app::t::slug::mail::own_address;
use crate::app::t::slug::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::auth::mail::{marketing_sender, send_rendered_via, shop_vars};
use crate::auth::require_user;
use crate::components::alert::{AlertVariant, alert, alert_description};
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::components::field::{field, field_description, field_label};
use crate::components::input::input;
use crate::components::select::select;
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::components::textarea::textarea;
use crate::db::ReadPool;
use crate::ui::{
    LINK, Shell, Tone, document, empty_row, format_time, icons, notice, page_header, pager,
    section_card, status_badge,
};

const PAGE_SIZE: u32 = 50;
const KIND: ShopMailKind = ShopMailKind::Newsletter;

#[topcoat::router::query_params(error = bad_request)]
struct CampaignsQuery {
    /// 1-based.
    page: Option<u32>,
    error: Option<String>,
    message: Option<String>,
}

#[derive(serde::Serialize)]
struct PageLink {
    page: u32,
}

/// The campaigns page, with a flash error code or message.
pub fn campaigns_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, Slug(slug.to_owned()))
        .query(Flash { error, message })
        .resolve(cx)
}

/// The word for where a campaign stands.
pub fn status_label(status: CampaignStatus) -> &'static str {
    match status {
        CampaignStatus::Draft => "Draft",
        CampaignStatus::Sending => "Sending",
        CampaignStatus::Sent => "Sent",
        CampaignStatus::Scheduled => "Scheduled",
        CampaignStatus::Stopped => "Stopped",
    }
}

fn row_status(status: &str) -> CampaignStatus {
    CampaignStatus::parse(status).unwrap_or_default()
}

fn status_tone(status: CampaignStatus) -> Tone {
    match status {
        CampaignStatus::Draft => Tone::Neutral,
        CampaignStatus::Sending => Tone::Warning,
        CampaignStatus::Sent => Tone::Success,
        CampaignStatus::Scheduled => Tone::Info,
        CampaignStatus::Stopped => Tone::Neutral,
    }
}

/// Who a campaign goes to, in words.
pub fn audience_label(language: Option<&str>) -> String {
    match language {
        Some(code) => format!("Subscribers in {}", language_name(code).unwrap_or(code)),
        None => "Every subscriber".to_owned(),
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_NEWSLETTER_READ)
        .await?;
    let query = query_params::<CampaignsQuery>(cx)?;
    let page_number = query.page.unwrap_or(1).max(1);
    let nav = TenantNav::of(cx, &access).await?;
    let can_manage = access.can(cx, PERMISSION_NEWSLETTER_MANAGE).await?;
    let TenantAccess { tenant, .. } = access;
    let pool = app_context::<ReadPool>(cx);
    let total = count_campaigns(pool, &tenant.id)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let rows = campaigns_of_tenant(pool, &tenant.id, PAGE_SIZE, (page_number - 1) * PAGE_SIZE)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let pages = total.div_ceil(u64::from(PAGE_SIZE)) as u32;
    let slug = tenant.slug.clone();
    let page_link = {
        let slug = slug.clone();
        move |page: u32| {
            href!(index, Slug(slug.clone()))
                .query(PageLink { page })
                .resolve(cx)
        }
    };
    let title = format!("Campaigns · Newsletter · {}", tenant.name);

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "newsletter"),
            page_header(
                title: "Campaigns",
                subtitle: Some("The mails of news sent to the newsletter's subscribers."),
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(href!(super::index, Slug(slug.clone())))>"Subscribers"</a>
                if can_manage {
                    <a class=(button_variants(ButtonVariant::Primary, ButtonSize::Sm)) href=(href!(new::new, Slug(slug.clone())))>"New campaign"</a>
                }
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                <p class="mb-4 text-sm text-muted-foreground" data-campaign-count=(total)>(total) " campaign(s)"</p>
                table(
                    table_header(
                        table_row(
                            table_head("Subject")
                            table_head("Audience")
                            table_head("Written")
                            table_head("Mailed")
                            table_head("Status")
                        )
                    )
                    table_body(
                        if rows.is_empty() {
                            empty_row(colspan: 5, text: "No campaign yet.")
                        }
                        for row in &rows {
                            table_row(
                                attrs: attributes! { data-campaign=(&row.subject) data-campaign-status=(&row.status) },
                                table_cell(<a class=(LINK) href=(href!(campaign_id::index, Slug(slug.clone()), campaign_id::CampaignId(row.id.clone())))>(&row.subject)</a>)
                                table_cell((audience_label(row.language.as_deref())))
                                table_cell(<time datetime=(row.created_at)>(format_time(row.created_at as u64))</time>)
                                table_cell(
                                    attrs: attributes! { data-campaign-delivered=(row.delivered) data-campaign-failed=(row.failed) },
                                    (row.delivered)
                                    if row.failed > 0 { " (" (row.failed) " failed)" }
                                )
                                table_cell(
                                    status_badge(tone: status_tone(row_status(&row.status)), (status_label(row_status(&row.status))))
                                    if let Some(cadence) = &row.cadence {
                                        <span class="ml-2 text-xs text-muted-foreground" data-campaign-cadence=(cadence)>(cadence)</span>
                                    }
                                )
                            )
                        }
                    )
                )
                pager(
                    page: i64::from(page_number),
                    pages: i64::from(pages),
                    previous: (page_number > 1).then(|| page_link(page_number - 1)),
                    next: (page_number < pages).then(|| page_link(page_number + 1)),
                )
            )
        )
    })
}

/// What the editor posts.
#[derive(Deserialize)]
pub struct CampaignForm {
    subject: String,
    html: String,
    /// The plain-text part; empty derives it from the HTML.
    #[serde(default)]
    text: String,
    /// A language the shop mails in; empty is everyone.
    #[serde(default)]
    language: String,
    /// `preview`, `test` or `save`.
    action: String,
}

impl CampaignForm {
    /// The draft as typed, line breaks as the store keeps them (browsers send
    /// textarea line breaks as CRLF).
    pub fn draft(&self) -> CampaignDraft {
        let text = self.text.replace("\r\n", "\n");
        CampaignDraft {
            subject: self.subject.trim().to_owned(),
            html: self.html.replace("\r\n", "\n"),
            text: (!text.trim().is_empty()).then_some(text),
            language: Some(self.language.trim().to_owned()).filter(|code| !code.is_empty()),
        }
    }
}

/// The draft rendered with made-up data, or why it can't be. A campaign
/// must link to the unsubscribe page: the rendered mail is looked at, so a
/// mention in a comment does not count and `<a href="{{ unsubscribe_url
/// }}">` does.
pub fn preview(
    cx: &Cx,
    tenant: &TenantState,
    draft: &CampaignDraft,
) -> std::result::Result<RenderedMail, MailTemplateError> {
    validate(KIND, &draft.subject, &draft.html)?;
    if let Some(text) = &draft.text {
        validate_text(KIND, text)?;
    }
    let vars = KIND.sample(shop_vars(cx, tenant));
    let rendered = render(&draft.subject, &draft.html, draft.text.as_deref(), &vars)?;
    let unsubscribe = render_text("{{ unsubscribe_url }}", &vars)?;
    if !links_to(&rendered.html, &unsubscribe) {
        return Err(MailTemplateError(
            "Body: link to {{ unsubscribe_url }}, so the reader can leave the list.".to_owned(),
        ));
    }
    Ok(rendered)
}

/// Mails the preview to the signed-in member: their address, or the error
/// code to show. Like the campaign itself, it goes through the shop's own
/// mail server only.
pub async fn send_test(
    cx: &Cx,
    tenant: &TenantState,
    rendered: &RenderedMail,
) -> Result<std::result::Result<String, &'static str>> {
    let Some(sender) = marketing_sender(cx, tenant)
        .await
        .map_err(topcoat::Error::from_anyhow)?
    else {
        return Ok(Err("mail_server_required"));
    };
    let user = require_user(cx).await?;
    let to = own_address(user);
    let rendered = RenderedMail {
        subject: format!("[Test] {}", rendered.subject),
        ..rendered.clone()
    };
    match send_rendered_via(cx, &sender, &to, rendered, Vec::new()).await {
        Ok(()) => Ok(Ok(to)),
        Err(err) => {
            tracing::warn!(tenant = %tenant.id, error = %err, "test campaign failed");
            Ok(Err("mail_test_failed"))
        }
    }
}

/// The form a draft is written in, beside its preview.
#[component]
pub async fn campaign_editor(
    tenant: &TenantState,
    action: String,
    draft: &CampaignDraft,
    checked: &std::result::Result<RenderedMail, MailTemplateError>,
) -> Result<impl View> {
    let max_subject = MAX_MAIL_SUBJECT_LEN.to_string();
    let languages = tenant.mail_languages();
    let picked = draft.language.as_deref();
    let text = draft.text.clone().unwrap_or_default();
    Ok(view! {
        <div class="grid gap-6 lg:grid-cols-2">
            section_card(
                attrs: attributes! { class="mb-0" },
                <form method="post" action=(action) class="flex flex-col gap-4" data-campaign-form="">
                    field(
                        field_label(attrs: attributes! { for="campaign-subject" }, "Subject")
                        input(attrs: attributes! {
                            id="campaign-subject" name="subject" required="" maxlength=(&max_subject) value=(&draft.subject)
                        })
                    )
                    field(
                        field_label(attrs: attributes! { for="campaign-language" }, "Sent to")
                        select(
                            attrs: attributes! { id="campaign-language" name="language" },
                            <option value="" selected=(picked.is_none())>"Every subscriber"</option>
                            for code in &languages {
                                <option value=(*code) selected=(picked == Some(*code))>(audience_label(Some(code)))</option>
                            }
                        )
                    )
                    field(
                        field_label(attrs: attributes! { for="campaign-html" }, "Body (HTML)")
                        textarea(
                            attrs: attributes! { id="campaign-html" name="html" required="" rows="20" spellcheck="false" class="font-mono text-xs" },
                            (&draft.html)
                        )
                    )
                    field(
                        field_label(attrs: attributes! { for="campaign-text" }, "Plain-text version")
                        field_description(
                            attrs: attributes! { id="campaign-text-hint" },
                            "For mail apps that show no HTML. Leave it empty to derive it from the HTML."
                        )
                        textarea(
                            attrs: attributes! {
                                id="campaign-text" name="text" rows="8" spellcheck="false" aria-describedby="campaign-text-hint" class="font-mono text-xs"
                            },
                            (&text)
                        )
                    )
                    if let Err(err) = checked {
                        alert(
                            variant: AlertVariant::Destructive,
                            attrs: attributes! { role="alert" data-template-error="" },
                            icon(data: icons::ERROR)
                            alert_description((err.to_string()))
                        )
                    }
                    <div class="flex flex-wrap gap-2">
                        button(attrs: attributes! { type="submit" name="action" value="save" }, "Save draft")
                        button(variant: ButtonVariant::Outline, attrs: attributes! { type="submit" name="action" value="preview" }, "Preview")
                        button(variant: ButtonVariant::Outline, attrs: attributes! { type="submit" name="action" value="test" }, "Send me a test")
                    </div>
                </form>
                <p class="mt-4 text-sm text-muted-foreground">
                    "Write " <code>"{{ shop.name }}"</code> " and the shop's other variables as in the shop's mails. "
                    "Every campaign links to " <code>"{{ unsubscribe_url }}"</code> ", the page where the reader leaves the list."
                    " " <code>"{% for product in new_products %}"</code> " lists the products put on sale since the last edition, each with "
                    <code>"{{ product.name }}"</code> ", " <code>"{{ product.price }}"</code> ", " <code>"{{ product.url }}"</code> " and " <code>"{{ product.image_url }}"</code> "."
                </p>
            )
            campaign_preview(checked: checked)
        </div>
    })
}

/// A rendered campaign, or the word that it can't be.
#[component]
pub async fn campaign_preview(
    checked: &std::result::Result<RenderedMail, MailTemplateError>,
) -> Result<impl View> {
    Ok(view! {
        section_card(
            title: Some("Preview"),
            attrs: attributes! { class="mb-0" },
            if let Ok(rendered) = checked {
                <p class="mb-2 text-sm"><span class="text-muted-foreground">"Subject: "</span><span data-preview-subject="">(&rendered.subject)</span></p>
                // The mail's own HTML rarely paints a background: white under it
                // whatever the dashboard's scheme, as a mail app would.
                <iframe sandbox="" title="Preview of the campaign" srcdoc=(&rendered.html)
                    class="h-[36rem] w-full rounded-lg border border-border bg-white"></iframe>
                <h3 class="mt-4 mb-1 text-sm font-medium">"Plain-text version"</h3>
                if let Some(text) = &rendered.text {
                    <pre data-preview-text="" class="max-h-64 overflow-auto whitespace-pre-wrap rounded-lg border border-border bg-muted p-3 font-mono text-xs">(text)</pre>
                } else {
                    <p data-preview-text="" class="text-sm text-muted-foreground">"Derived from the HTML."</p>
                }
            } else {
                <p class="text-sm text-muted-foreground">"Fix the campaign to see it."</p>
            }
        )
    })
}
