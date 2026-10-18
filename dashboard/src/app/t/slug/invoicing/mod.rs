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
    router::{href, page, query_params},
    view::{View, view},
};

use super::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::app::auth::Flash;
use crate::app::companies::{company_id, new::new_company_url};
use crate::app::tenants::tenancy;
use crate::db::ReadPool;
use crate::ui::{Shell, document, notice};

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
    let business = timada_orders::Command(access.executor()?.clone())
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
    let mine = companies_of_user(app_context::<ReadPool>(cx), &user_id).await?;
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
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">"Invoicing"</h1>
            </header>
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            if let Some(company) = &company {
                <section class="mb-6 rounded border border-neutral-200 bg-white p-4 text-sm" data-invoicing="linked">
                    <h2 class="mb-2 text-lg font-medium">"Sells as"</h2>
                    <dl class="grid grid-cols-[10rem_1fr] gap-y-1">
                        <dt class="text-neutral-500">"Company"</dt>
                        <dd data-linked-company=(&company.siren)>
                            if owns {
                                <a class="underline" href=(href!(company_id::index, company_id::CompanyId(company.id.clone())))>(company.name())</a>
                            } else {
                                (company.name())
                            }
                        </dd>
                        <dt class="text-neutral-500">"SIREN"</dt><dd class="font-mono">(&company.siren)</dd>
                        <dt class="text-neutral-500">"Invoice numbers"</dt>
                        <dd class="font-mono" data-series=(&code)>
                            (&first_number)
                        </dd>
                    </dl>
                    <form method="post" action=(href!(unlink::unlink, Slug(slug.clone()))) class="mt-3">
                        <button class="rounded border border-red-300 px-3 py-1 text-red-800 hover:bg-red-50">"Stop selling as this company"</button>
                    </form>
                </section>
            } else {
                <p role="alert" data-invoicing="none" class="mb-4 rounded border border-amber-300 bg-amber-50 p-3 text-sm text-amber-900">
                    "Not set up: shoppers cannot pay until the tenant sells as a company its invoices can name."
                </p>
            }

            <section class="rounded border border-neutral-200 bg-white p-4 text-sm">
                <h2 class="mb-2 text-lg font-medium">
                    if company.is_some() { "Sell as another company, or under another code" } else { "Sell as a company" }
                </h2>
                if mine.is_empty() {
                    <p class="mb-2 text-neutral-600">"You own no company yet."</p>
                } else {
                    <form method="post" action=(href!(link::link, Slug(slug.clone()))) class="mb-4 space-y-3">
                        <label class="block" for="link-company">"Company"</label>
                        <select id="link-company" name="company_id" class="rounded border border-neutral-300 px-3 py-2">
                            for row in &mine {
                                <option value=(&row.id)>(&row.legal_name) " (" (&row.siren) ")"</option>
                            }
                        </select>
                        <label class="block" for="link-code">"Series code (blank for the company's first tenant)"</label>
                        <input id="link-code" name="series_code" maxlength="10" placeholder="PRO" value=(&code)
                            class="w-40 rounded border border-neutral-300 px-3 py-2 font-mono uppercase">
                        <p class="text-xs text-neutral-500">"Each tenant of a company numbers its invoices in its own series: PRO gives PRO-F2026-000001. A code stays the tenant's for good."</p>
                        <button class="rounded bg-neutral-900 px-4 py-2 font-medium text-white hover:bg-neutral-700">"Link"</button>
                    </form>
                }
                <a class="underline" href=(&create)>"Register a new company"</a>
            </section>

            <section class="mt-6 rounded border border-neutral-200 bg-white p-4 text-sm" data-business-buyers=(if business { "on" } else { "off" })>
                <h2 class="mb-2 text-lg font-medium">"Business buyers"</h2>
                if business {
                    <p class="mb-3 text-neutral-600">"Shoppers may give an EU company VAT number at checkout: a sale to a business in another EU country is invoiced without VAT (reverse charge), once VIES confirms the number."</p>
                } else {
                    <p class="mb-3 text-neutral-600">"The shop sells to consumers only: checkout asks for no VAT number and every invoice carries VAT."</p>
                }
                <form method="post" action=(href!(business_buyers::business_buyers, Slug(slug.clone())))>
                    if business {
                        <input type="hidden" name="accept" value="off">
                        <button class="rounded border border-neutral-300 px-3 py-1 hover:bg-neutral-50">"Stop accepting business buyers"</button>
                    } else {
                        <input type="hidden" name="accept" value="on">
                        <button class="rounded bg-neutral-900 px-4 py-2 font-medium text-white hover:bg-neutral-700">"Accept business buyers"</button>
                    }
                </form>
            </section>
        )
    })
}
