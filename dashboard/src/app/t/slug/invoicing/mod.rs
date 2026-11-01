//! `/t/{slug}/invoicing`: the company the tenant sells as, for members with
//! `tenant:manage`. Its invoices name that company and count with its other
//! tenants' towards its VAT obligations, numbered in a series of the
//! tenant's own (its code). Shoppers cannot pay until it is linked. The
//! page also says whether the shop sells to businesses (reverse charge).

pub mod business_buyers;
pub mod link;
pub mod unlink;

use timada_tenancy::{PERMISSION_TENANT_MANAGE, companies_of_user};
use topcoat::{
    Result,
    context::{Cx, app_context},
    icon::icon,
    router::{href, page, query_params},
    view::{View, attributes, class, view},
};

use super::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::app::auth::Flash;
use crate::app::companies::{company_id, new::new_company_url};
use crate::app::tenants::tenancy;
use crate::components::alert::{AlertVariant, alert, alert_description};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::field::{field, field_label};
use crate::components::input::input;
use crate::components::select::select;
use crate::db::Db;
use crate::ui::{
    LINK, Shell, Tone, document, fact, facts, icons, notice, page_header, section_card,
    status_badge,
};

#[topcoat::router::query_params(error = bad_request)]
struct InvoicingQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn invoicing_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, Slug(slug.to_owned()))
        .query(Flash { error, message })
        .resolve(cx)
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_TENANT_MANAGE)
        .await?;
    let query = query_params::<InvoicingQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let user_id = access.user_id.clone();
    let business = timada_orders::Command::new(access.executor()?.clone(), access.tenant_id())
        .accepts_business_buyers()
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let TenantAccess { tenant, .. } = access;
    let slug = tenant.slug.clone();
    let company = match &tenant.company {
        Some(link) => tenancy(cx)
            .load_company(&link.company_id)
            .await
            .map_err(topcoat::Error::from_anyhow)?,
        None => None,
    };
    let code = tenant
        .company
        .as_ref()
        .map(|link| link.series_code.clone())
        .unwrap_or_default();
    let owns = company.as_ref().is_some_and(|c| c.is_owner(&user_id));
    let mine = companies_of_user(app_context::<Db>(cx), &user_id)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let create = new_company_url(cx, None, Some(&slug));
    let year = timada_invoicing::civil_date(crate::app::companies::company_id::now()).0;
    let first_number = if code.is_empty() {
        format!("F{year}-000001…")
    } else {
        format!("{code}-F{year}-000001…")
    };
    let title = format!("Invoicing · {}", tenant.name);

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "invoicing"),
            page_header(title: "Invoicing", subtitle: Some("The company the tenant sells as, and whom it sells to."))
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            if let Some(company) = &company {
                section_card(
                    title: Some("Sells as"),
                    attrs: attributes! { data-invoicing="linked" },
                    facts(
                        fact(
                            term: "Company",
                            attrs: attributes! { data-linked-company=(&company.siren) },
                            if owns {
                                <a class=(LINK) href=(href!(company_id::index, company_id::CompanyId(company.id.clone())))>(company.name())</a>
                            } else {
                                (company.name())
                            }
                        )
                        fact(term: "SIREN", attrs: attributes! { class="font-mono" }, (&company.siren))
                        fact(term: "Invoice numbers", attrs: attributes! { class="font-mono" data-series=(&code) }, (&first_number))
                    )
                    <form method="post" action=(href!(unlink::unlink, Slug(slug.clone()))) class="mt-4">
                        button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Stop selling as this company")
                    </form>
                )
            } else {
                alert(
                    variant: AlertVariant::Warning,
                    attrs: attributes! { role="alert" class="mb-6" data-invoicing="none" },
                    icon(data: icons::WARNING)
                    alert_description("Not set up: shoppers cannot pay until the tenant sells as a company its invoices can name.")
                )
            }

            section_card(
                title: Some(if company.is_some() { "Sell as another company, or under another code" } else { "Sell as a company" }),
                if mine.is_empty() {
                    <p class="mb-4 text-sm text-muted-foreground">"You own no company yet."</p>
                } else {
                    <form method="post" action=(href!(link::link, Slug(slug.clone()))) class="mb-4 flex max-w-xl flex-col gap-4">
                        field(
                            field_label(attrs: attributes! { for="link-company" }, "Company")
                            select(
                                attrs: attributes! { id="link-company" name="company_id" },
                                for row in &mine {
                                    <option value=(&row.id)>(&row.legal_name) " (" (&row.siren) ")"</option>
                                }
                            )
                        )
                        field(
                            field_label(attrs: attributes! { for="link-code" }, "Series code (blank for the company's first tenant)")
                            input(attrs: attributes! {
                                id="link-code" name="series_code" maxlength="10" placeholder="PRO" value=(&code)
                                class="w-40 font-mono uppercase"
                            })
                            <p class="text-xs text-muted-foreground">"Each tenant of a company numbers its invoices in its own series: PRO gives PRO-F2026-000001. A code stays the tenant's for good."</p>
                        )
                        <div>
                            button(attrs: attributes! { type="submit" }, "Link")
                        </div>
                    </form>
                }
                <a class=(class!(LINK, "text-sm")) href=(&create)>"Register a new company"</a>
            )

            section_card(
                title: Some("Business buyers"),
                attrs: attributes! { data-business-buyers=(if business { "on" } else { "off" }) },
                <div class="mb-4 flex flex-wrap items-center gap-3 text-sm">
                    if business {
                        status_badge(tone: Tone::Success, "Accepted")
                        <p class="text-muted-foreground">"Shoppers may give an EU company VAT number at checkout: a sale to a business in another EU country is invoiced without VAT (reverse charge), once VIES confirms the number."</p>
                    } else {
                        status_badge(tone: Tone::Neutral, "Consumers only")
                        <p class="text-muted-foreground">"The shop sells to consumers only: checkout asks for no VAT number and every invoice carries VAT."</p>
                    }
                </div>
                <form method="post" action=(href!(business_buyers::business_buyers, Slug(slug.clone())))>
                    if business {
                        <input type="hidden" name="accept" value="off">
                        button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Stop accepting business buyers")
                    } else {
                        <input type="hidden" name="accept" value="on">
                        button(attrs: attributes! { type="submit" }, "Accept business buyers")
                    }
                </form>
            )
        )
    })
}
