//! `/t/{slug}/invoicing`: who the merchant is in law, for members with
//! `tenant:manage`. Every invoice copies it when issued, so it must be set
//! before shoppers can pay: the legal name and form, the SIRET, the address,
//! and either a VAT number with the default rate or the exemption for small
//! businesses, and whether it is registered for the EU One-Stop Shop.

pub mod save;

use timada_invoicing::tax::EU_DISTANCE_THRESHOLD_CENTS;
use timada_invoicing::{civil_date, eu_distance_sales, format_amount_fr};
use timada_tenancy::{LegalIdentity, PERMISSION_TENANT_MANAGE};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params},
    view::{View, view},
};

use super::{Slug, TenantAccess, TenantNav, require_tenant, tenant_nav};
use crate::app::auth::Flash;
use crate::db::ReadPool;
use crate::ui::{document, notice};

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

/// `1000050` cents as `10000.50`, what the form shows back.
fn format_capital(cents: u64) -> String {
    if cents.is_multiple_of(100) {
        (cents / 100).to_string()
    } else {
        format!("{}.{:02}", cents / 100, cents % 100)
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_TENANT_MANAGE)
        .await?;
    let query = query_params::<InvoicingQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let TenantAccess { tenant, .. } = access;
    let slug = tenant.slug.clone();
    let set = tenant.legal.is_some();
    let legal = tenant.legal.clone().unwrap_or_else(|| LegalIdentity {
        country: "FR".to_owned(),
        ..LegalIdentity::default()
    });
    let capital = legal
        .share_capital_cents
        .map(format_capital)
        .unwrap_or_default();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let year = civil_date(now).0;
    let (distance_cents, distance_passed) =
        eu_distance_sales(app_context::<ReadPool>(cx), &tenant.id, year)
            .await?
            .unwrap_or_default();
    let distance = format_amount_fr(distance_cents.max(0) as u64, "EUR");
    let threshold = format_amount_fr(EU_DISTANCE_THRESHOLD_CENTS, "EUR");
    let exempt_past_threshold = distance_passed && set && !legal.is_vat_registered();
    let title = format!("Invoicing · {}", tenant.name);

    Ok(view! {
        document(
            title: &title,
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">"Invoicing"</h1>
                tenant_nav(tenant: &tenant, nav: &nav, current: "invoicing")
            </header>
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            if !set {
                <p role="alert" data-invoicing="none" class="mb-4 rounded border border-amber-300 bg-amber-50 p-3 text-sm text-amber-900">
                    "Not set up: shoppers cannot pay until the invoices can name you."
                </p>
            }

            <section class="mb-6 rounded border border-neutral-200 bg-white p-4 text-sm" data-eu-distance=(distance_cents)>
                <h2 class="mb-1 text-lg font-medium">"Sales to EU consumers abroad, " (year)</h2>
                <p>
                    <strong>(&distance)</strong> " excluding VAT, of the " (&threshold) " a year above which their countries' VAT applies."
                    if distance_passed { " " <strong data-eu-threshold-passed="">"Threshold passed: their VAT applies for the rest of the year and all of next."</strong> }
                </p>
                if exempt_past_threshold {
                    <p role="alert" class="mt-2 rounded border border-amber-300 bg-amber-50 p-3 text-amber-900">
                        "Past the threshold, the exemption no longer covers these sales: register for VAT and the One-Stop Shop, then enter the VAT number below."
                    </p>
                }
            </section>

            <form method="post" action=(href!(save::save, Slug(slug.clone()))) class="space-y-6">
                <fieldset class="space-y-3 rounded border border-neutral-200 bg-white p-4">
                    <legend class="px-1 text-lg font-medium">"Company"</legend>
                    <label class="block text-sm" for="legal-name">"Legal name"</label>
                    <input id="legal-name" name="legal_name" required="" maxlength="120" value=(&legal.legal_name)
                        class="w-full rounded border border-neutral-300 px-3 py-2">
                    <label class="block text-sm" for="legal-form">"Legal form (SAS, SARL, EI…)"</label>
                    <input id="legal-form" name="legal_form" required="" maxlength="40" value=(&legal.legal_form)
                        class="w-full rounded border border-neutral-300 px-3 py-2">
                    <label class="block text-sm" for="legal-capital">"Share capital in euros (blank for none)"</label>
                    <input id="legal-capital" name="share_capital" inputmode="decimal" value=(&capital)
                        class="w-full rounded border border-neutral-300 px-3 py-2">
                    <label class="block text-sm" for="legal-siret">"SIRET"</label>
                    <input id="legal-siret" name="siret" required="" inputmode="numeric" value=(&legal.siret)
                        class="w-full rounded border border-neutral-300 px-3 py-2 font-mono">
                    <label class="block text-sm" for="legal-rcs">"RCS city (blank if not registered)"</label>
                    <input id="legal-rcs" name="rcs_city" maxlength="100" value=(legal.rcs_city.as_deref().unwrap_or(""))
                        class="w-full rounded border border-neutral-300 px-3 py-2">
                    <label class="block text-sm" for="legal-email">"Contact e-mail"</label>
                    <input id="legal-email" name="contact_email" type="email" required="" value=(&legal.contact_email)
                        class="w-full rounded border border-neutral-300 px-3 py-2">
                </fieldset>

                <fieldset class="space-y-3 rounded border border-neutral-200 bg-white p-4">
                    <legend class="px-1 text-lg font-medium">"Registered address"</legend>
                    <label class="block text-sm" for="legal-line1">"Address"</label>
                    <input id="legal-line1" name="line1" required="" maxlength="200" value=(&legal.line1)
                        class="w-full rounded border border-neutral-300 px-3 py-2">
                    <label class="block text-sm" for="legal-line2">"Address, line 2"</label>
                    <input id="legal-line2" name="line2" maxlength="200" value=(legal.line2.as_deref().unwrap_or(""))
                        class="w-full rounded border border-neutral-300 px-3 py-2">
                    <label class="block text-sm" for="legal-postal">"Postal code"</label>
                    <input id="legal-postal" name="postal_code" required="" maxlength="20" value=(&legal.postal_code)
                        class="w-full rounded border border-neutral-300 px-3 py-2">
                    <label class="block text-sm" for="legal-city">"City"</label>
                    <input id="legal-city" name="city" required="" maxlength="100" value=(&legal.city)
                        class="w-full rounded border border-neutral-300 px-3 py-2">
                    <label class="block text-sm" for="legal-country">"Country (two letters)"</label>
                    <input id="legal-country" name="country" required="" maxlength="2" value=(&legal.country)
                        class="w-24 rounded border border-neutral-300 px-3 py-2 uppercase">
                </fieldset>

                <fieldset class="space-y-3 rounded border border-neutral-200 bg-white p-4">
                    <legend class="px-1 text-lg font-medium">"VAT"</legend>
                    <p class="text-sm text-neutral-600">
                        "Prices in the catalog include VAT. Without a VAT number, invoices say "
                        <q>"TVA non applicable, art. 293 B du CGI"</q>"."
                    </p>
                    <label class="block text-sm" for="legal-vat">"Intra-community VAT number (blank under the exemption)"</label>
                    <input id="legal-vat" name="vat_number" maxlength="40" placeholder="FR00123456789" value=(legal.vat_number.as_deref().unwrap_or(""))
                        class="w-full rounded border border-neutral-300 px-3 py-2 font-mono">
                    <label class="flex items-center gap-2 text-sm">
                        <input type="checkbox" name="oss_registered" value="on" checked=(legal.oss_registered)>
                        "Registered for the EU One-Stop Shop (OSS): charge other EU countries' VAT to their consumers from the first euro"
                    </label>
                    <p class="text-sm text-neutral-600">
                        "Without it, other EU countries' VAT applies once your sales to their consumers pass €10,000 in the year."
                    </p>
                </fieldset>

                <button class="rounded bg-neutral-900 px-4 py-2 font-medium text-white hover:bg-neutral-700">"Save"</button>
            </form>
        )
    })
}
