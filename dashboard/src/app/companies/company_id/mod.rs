//! `/companies/{company_id}`: one company, for its owners: its identity,
//! its owners, the tenants selling as it with their series codes, and its
//! EU distance sales of the year (every tenant together).

pub mod oss;
pub mod owners;
pub mod remove_owner;
pub mod save;

use timada_invoicing::tax::EU_DISTANCE_THRESHOLD_CENTS;
use timada_invoicing::{Ledger, civil_date, format_amount_fr};
use timada_tenancy::{CompanyState, tenants_of_company};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{error::not_found, href, module_param, page, path_param, query_params},
    view::{View, view},
};

use super::identity_fields;
use crate::app::auth::Flash;
use crate::app::t::slug::Slug;
use crate::app::tenants::tenancy;
use crate::auth::{require_user, user_labels};
use crate::db::ReadPool;
use crate::ui::{document, notice};

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
    let id = company.id.clone();
    let title = format!("{} · Companies", company.name());

    Ok(view! {
        document(
            title: &title,
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold" data-company-siren=(&company.siren)>(company.name())</h1>
                <nav class="flex items-center gap-4 text-sm">
                    <a class="underline" href=(href!(super::index))>"Companies"</a>
                    <a class="underline" href=(href!(oss::oss, CompanyId(id.clone())))>"One-Stop Shop return"</a>
                </nav>
            </header>
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            <section class="mb-6 rounded border border-neutral-200 bg-white p-4 text-sm" data-eu-distance=(distance.sales_cents)>
                <h2 class="mb-1 text-lg font-medium">"Sales to EU consumers abroad, " (year)</h2>
                <p>
                    <strong>(&sales)</strong> " excluding VAT, every tenant together, of the " (&threshold) " a year above which their countries' VAT applies."
                    if distance.passed { " " <strong data-eu-threshold-passed="">"Threshold passed: their VAT applies for the rest of the year and all of next."</strong> }
                </p>
                if exempt_past_threshold {
                    <p role="alert" class="mt-2 rounded border border-amber-300 bg-amber-50 p-3 text-amber-900">
                        "Past the threshold, the exemption no longer covers these sales: register for VAT and the One-Stop Shop, then enter the VAT number below."
                    </p>
                }
            </section>

            <section class="mb-6 rounded border border-neutral-200 bg-white p-4 text-sm">
                <h2 class="mb-2 text-lg font-medium">"Tenants selling as this company"</h2>
                if tenants.is_empty() {
                    <p class="text-neutral-500">"None yet: link one from its Invoicing page."</p>
                }
                <ul class="divide-y divide-neutral-100">
                    for tenant in &tenants {
                        <li class="flex items-center justify-between py-2" data-company-tenant=(&tenant.slug)>
                            <a class="underline" href=(href!(crate::app::t::slug::index, Slug(tenant.slug.clone())))>(&tenant.name)</a>
                            <span class="font-mono" data-series-code=(&tenant.series_code)>
                                if tenant.series_code.is_empty() { "F/A…" } else { (&tenant.series_code) "-F/A…" }
                            </span>
                        </li>
                    }
                </ul>
            </section>

            <section class="mb-6 rounded border border-neutral-200 bg-white p-4 text-sm">
                <h2 class="mb-2 text-lg font-medium">"Owners"</h2>
                <ul class="mb-3 divide-y divide-neutral-100">
                    for owner in &owners {
                        <li class="flex items-center justify-between py-2" data-company-owner=(labels.get(owner).map_or(owner.as_str(), String::as_str))>
                            (labels.get(owner).map_or(owner.as_str(), String::as_str))
                            if owners.len() > 1 {
                                <form method="post" action=(href!(remove_owner::remove_owner, CompanyId(id.clone())))>
                                    <input type="hidden" name="user_id" value=(owner)>
                                    <button class="text-red-800 underline">"Remove"</button>
                                </form>
                            }
                        </li>
                    }
                </ul>
                <form method="post" action=(href!(owners::add_owner, CompanyId(id.clone()))) class="flex gap-2">
                    <label class="sr-only" for="owner-email">"E-mail of the new owner"</label>
                    <input id="owner-email" name="email" type="email" required="" placeholder="E-mail of a registered user"
                        class="w-full rounded border border-neutral-300 px-3 py-2">
                    <button class="rounded border border-neutral-300 px-3 py-2">"Add owner"</button>
                </form>
            </section>

            <form method="post" action=(href!(save::save, CompanyId(id.clone()))) class="space-y-6">
                identity_fields(legal: &legal)
                <button class="rounded bg-neutral-900 px-4 py-2 font-medium text-white hover:bg-neutral-700">"Save"</button>
            </form>
        )
    })
}
