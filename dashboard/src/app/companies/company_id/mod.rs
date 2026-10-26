//! `/companies/{company_id}`: one company, for its owners: its identity,
//! its owners, the tenants selling as it with their series codes, its EU
//! distance sales of the year (every tenant together), and the AliExpress
//! account its tenants buy through.

pub mod aliexpress;
pub mod oss;
pub mod owners;
pub mod remove_owner;
pub mod save;

use std::sync::Arc;

use timada_invoicing::tax::EU_DISTANCE_THRESHOLD_CENTS;
use timada_invoicing::{Ledger, civil_date, format_amount_fr};
use timada_tenancy::{CompanyState, tenants_of_company};
use topcoat::{
    Result,
    context::{Cx, app_context},
    icon::icon,
    router::{error::not_found, href, module_param, page, path_param, query_params},
    view::{View, attributes, view},
};

use super::identity_fields;
use crate::aliexpress::Aliexpress;
use crate::app::auth::Flash;
use crate::app::t::slug::Slug;
use crate::app::tenants::tenancy;
use crate::auth::{require_user, user_labels};
use crate::components::alert::{AlertVariant, alert, alert_description};
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::components::input::input;
use crate::db::ReadPool;
use crate::ui::{
    LINK, Shell, Tone, document, fact, facts, format_time, icons, notice, page_header,
    section_card, status_badge,
};

module_param!(pub company_id);

#[topcoat::router::query_params(error = bad_request)]
struct CompanyQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn company_url(
    cx: &Cx,
    company_id: &str,
    error: Option<&str>,
    message: Option<&str>,
) -> String {
    href!(index, CompanyId(company_id.to_owned()))
        .query(Flash { error, message })
        .resolve(cx)
}

/// The company in the URL and the signed-in user, who must own it (else
/// 404: a company is nobody else's business).
pub(crate) async fn require_owned(cx: &Cx) -> Result<(CompanyState, String)> {
    let user = require_user(cx).await?;
    let id = path_param::<CompanyId>(cx);
    let company = tenancy(cx)
        .load_company(id)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .filter(|company| company.is_owner(&user.id))
        .ok_or_else(not_found)?;
    Ok((company, user.id.clone()))
}

/// Unix seconds now.
pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let (company, _) = require_owned(cx).await?;
    let query = query_params::<CompanyQuery>(cx)?;
    let tenants = tenants_of_company(app_context::<ReadPool>(cx), &company.id).await?;
    let owners: Vec<String> = company.owners.iter().cloned().collect();
    let labels = user_labels(cx, &owners).await?;
    let year = civil_date(now()).0;
    let distance = Ledger(tenancy(cx).0.clone())
        .total(&company.id, year)
        .await
        .map_err(|err| topcoat::Error::from_anyhow(err.into()))?;
    let sales = format_amount_fr(distance.sales_cents, "EUR");
    let threshold = format_amount_fr(EU_DISTANCE_THRESHOLD_CENTS, "EUR");
    let legal = company.identity.clone().unwrap_or_default();
    let exempt_past_threshold = distance.passed && !legal.is_vat_registered();
    let aliexpress_enabled = app_context::<Arc<Aliexpress>>(cx).enabled();
    let aliexpress_state = if company.aliexpress.is_some() {
        "connected"
    } else {
        "none"
    };
    let id = company.id.clone();
    let title = format!("{} · Companies", company.name());
    let siren = format!("SIREN {}", company.siren);
    let distance_title = format!("Sales to EU consumers abroad, {year}");

    Ok(view! {
        document(
            title: &title,
            shell: Shell::page("companies"),
            page_header(
                title: &company.name(),
                subtitle: Some(&siren),
                h1_attrs: attributes! { data-company-siren=(&company.siren) },
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(href!(oss::oss, CompanyId(id.clone())))>
                    icon(data: icons::INVOICING)
                    "One-Stop Shop return"
                </a>
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                title: Some(&distance_title),
                attrs: attributes! { data-eu-distance=(distance.sales_cents) },
                <p class="text-sm">
                    <strong class="text-lg tabular-nums">(&sales)</strong> " excluding VAT, every tenant together, of the " (&threshold) " a year above which their countries' VAT applies."
                    if distance.passed { " " <strong data-eu-threshold-passed="">"Threshold passed: their VAT applies for the rest of the year and all of next."</strong> }
                </p>
                if exempt_past_threshold {
                    alert(
                        variant: AlertVariant::Warning,
                        attrs: attributes! { role="alert" class="mt-4" },
                        icon(data: icons::WARNING)
                        alert_description(
                            "Past the threshold, the exemption no longer covers these sales: register for VAT and the One-Stop Shop, then enter the VAT number below."
                        )
                    )
                }
            )

            section_card(
                title: Some("Tenants selling as this company"),
                description: Some("Each one's invoices are numbered in its own series."),
                if tenants.is_empty() {
                    <p class="text-sm text-muted-foreground">"None yet: link one from its Invoicing page."</p>
                } else {
                    <ul class="divide-y divide-border text-sm">
                        for tenant in &tenants {
                            <li class="flex flex-wrap items-center justify-between gap-2 py-2.5" data-company-tenant=(&tenant.slug)>
                                <a class=(LINK) href=(href!(crate::app::t::slug::index, Slug(tenant.slug.clone())))>(&tenant.name)</a>
                                <code class="text-xs" data-series-code=(&tenant.series_code)>
                                    if tenant.series_code.is_empty() { "F/A…" } else { (&tenant.series_code) "-F/A…" }
                                </code>
                            </li>
                        }
                    </ul>
                }
            )

            section_card(
                title: Some("AliExpress"),
                description: Some("The AliExpress buyer account the company's tenants import products through and place their orders with."),
                <div class="mb-4" data-company-aliexpress=(aliexpress_state)>
                    if let Some(authorization) = &company.aliexpress {
                        facts(
                            fact(term: "Account", status_badge(tone: Tone::Success, "Connected") " " <span data-aliexpress-account="">(&authorization.account)</span>)
                            fact(term: "Access until", <time datetime=(authorization.expires_at)>(format_time(authorization.expires_at))</time>)
                        )
                    } else {
                        <p class="text-sm text-muted-foreground">"Not connected. Each tenant then chooses what it buys in, on its AliExpress page."</p>
                    }
                </div>
                if aliexpress_enabled {
                    <div class="flex flex-wrap gap-2">
                        <form method="post" action=(href!(aliexpress::connect::connect, CompanyId(id.clone())))>
                            button(size: ButtonSize::Sm, attrs: attributes! { type="submit" },
                                (if company.aliexpress.is_some() { "Connect again" } else { "Connect the AliExpress account" }))
                        </form>
                        if company.aliexpress.is_some() {
                            <form method="post" action=(href!(aliexpress::disconnect::disconnect, CompanyId(id.clone())))>
                                button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Disconnect")
                            </form>
                        }
                    </div>
                } else {
                    alert(
                        variant: AlertVariant::Warning,
                        attrs: attributes! { role="alert" },
                        icon(data: icons::WARNING)
                        alert_description("AliExpress is not set up on this platform: the operator has to give it an app and set secrets.key first.")
                    )
                }
            )

            section_card(
                title: Some("Owners"),
                description: Some("Owners edit the company and link tenants to it. A registered user is added by e-mail."),
                <ul class="divide-y divide-border text-sm">
                    for owner in &owners {
                        <li class="flex flex-wrap items-center justify-between gap-2 py-2.5" data-company-owner=(labels.get(owner).map_or(owner.as_str(), String::as_str))>
                            <span class="font-medium">(labels.get(owner).map_or(owner.as_str(), String::as_str))</span>
                            if owners.len() > 1 {
                                <form method="post" action=(href!(remove_owner::remove_owner, CompanyId(id.clone())))>
                                    <input type="hidden" name="user_id" value=(owner)>
                                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="text-destructive-text" }, "Remove")
                                </form>
                            }
                        </li>
                    }
                </ul>
                <form method="post" action=(href!(owners::add_owner, CompanyId(id.clone()))) class="mt-4 flex flex-wrap gap-2">
                    <label class="sr-only" for="owner-email">"E-mail of the new owner"</label>
                    input(attrs: attributes! {
                        id="owner-email" name="email" type="email" required=""
                        placeholder="E-mail of a registered user" class="sm:max-w-sm"
                    })
                    button(variant: ButtonVariant::Outline, attrs: attributes! { type="submit" }, icon(data: icons::PLUS) "Add owner")
                </form>
            )

            <form method="post" action=(href!(save::save, CompanyId(id.clone())))>
                identity_fields(legal: &legal)
                button(attrs: attributes! { type="submit" }, "Save")
            </form>
        )
    })
}
