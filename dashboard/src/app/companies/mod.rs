//! `/companies`: the companies a user owns. A company is who sells in law,
//! behind one or more tenants: its identity (what invoices say about it),
//! its VAT registration, its owners. Its SIREN is unique on the platform.

pub mod company_id;
pub mod new;

use serde::Deserialize;
use timada_tenancy::{LegalIdentity, companies_of_user};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params},
    view::{View, component, view},
};

use crate::app::auth::Flash;
use crate::auth::require_user;
use crate::db::ReadPool;
use crate::ui::{document, notice};

#[topcoat::router::query_params(error = bad_request)]
struct CompaniesQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn companies_url(cx: &Cx, error: Option<&str>, message: Option<&str>) -> String {
    href!(index).query(Flash { error, message }).resolve(cx)
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let user = require_user(cx).await?;
    let query = query_params::<CompaniesQuery>(cx)?;
    let mine = companies_of_user(app_context::<ReadPool>(cx), &user.id).await?;
    Ok(view! {
        document(
            title: "Companies",
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">"Companies"</h1>
                <nav class="flex items-center gap-4 text-sm">
                    <a class="underline" href=(href!(crate::app::home))>"Dashboard"</a>
                </nav>
            </header>
            notice(error: query.error.as_deref(), message: query.message.as_deref())
            <section class="rounded border border-neutral-200 bg-white p-4">
                <div class="mb-2 flex items-center justify-between">
                    <h2 class="text-lg font-medium">"Your companies"</h2>
                    <a class="rounded bg-neutral-900 px-3 py-1 text-sm font-medium text-white hover:bg-neutral-700" href=(href!(new::new))>"Register a company"</a>
                </div>
                <p class="mb-2 text-sm text-neutral-600">"A company is who sells in law: its tenants' invoices name it, and they count together towards its VAT obligations."</p>
                if mine.is_empty() {
                    <p class="text-sm text-neutral-500">"You own no company yet."</p>
                }
                <ul class="divide-y divide-neutral-100 text-sm">
                    for row in &mine {
                        <li class="flex items-center justify-between py-2" data-company=(&row.siren)>
                            <a class="underline" href=(href!(company_id::index, company_id::CompanyId(row.id.clone())))>(&row.legal_name)</a>
                            <span class="font-mono text-neutral-500">"SIREN " (&row.siren)</span>
                        </li>
                    }
                </ul>
            </section>
        )
    })
}

/// The identity form, as posted.
#[derive(Deserialize)]
pub struct IdentityForm {
    legal_name: String,
    legal_form: String,
    share_capital: String,
    siret: String,
    rcs_city: String,
    contact_email: String,
    line1: String,
    line2: String,
    postal_code: String,
    city: String,
    country: String,
    vat_number: String,
    /// A checkbox: absent when unticked.
    #[serde(default)]
    oss_registered: Option<String>,
}

impl IdentityForm {
    /// The identity typed; an unreadable share capital is its error code.
    pub fn identity(&self) -> std::result::Result<LegalIdentity, &'static str> {
        let share_capital_cents = match self.share_capital.trim() {
            "" => None,
            capital => {
                Some(timada_catalog::parse_amount(capital).map_err(|_| "invalid_share_capital")?)
            }
        };
        Ok(LegalIdentity {
            legal_name: self.legal_name.clone(),
            legal_form: self.legal_form.clone(),
            share_capital_cents,
            siret: self.siret.clone(),
            rcs_city: Some(self.rcs_city.clone()),
            line1: self.line1.clone(),
            line2: Some(self.line2.clone()),
            postal_code: self.postal_code.clone(),
            city: self.city.clone(),
            country: self.country.clone(),
            contact_email: self.contact_email.clone(),
            vat_number: Some(self.vat_number.clone()),
            oss_registered: self.oss_registered.is_some(),
        })
    }
}

/// `1000050` cents as `10000.50`, what the form shows back.
fn format_capital(cents: u64) -> String {
    if cents.is_multiple_of(100) {
        (cents / 100).to_string()
    } else {
        format!("{}.{:02}", cents / 100, cents % 100)
    }
}

/// The fields of the identity form, filled with `legal`.
#[component]
pub async fn identity_fields(legal: &LegalIdentity) -> Result<impl View> {
    let capital = legal
        .share_capital_cents
        .map(format_capital)
        .unwrap_or_default();
    Ok(view! {
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
    })
}
