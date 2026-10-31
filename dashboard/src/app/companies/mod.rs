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
    icon::icon,
    router::{href, page, query_params},
    view::{Child, View, attributes, class, component, view},
};

use crate::app::auth::Flash;
use crate::auth::require_user;
use crate::components::button::{ButtonSize, ButtonVariant, button_variants};
use crate::components::card::CARD;
use crate::components::checkbox::checkbox;
use crate::components::field::{field, field_label};
use crate::components::input::input;
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::db::ReadPool;
use crate::ui::{LINK, Shell, document, empty_state, icons, notice, page_header, section_card};

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
            shell: Shell::page("companies"),
            page_header(
                title: "Companies",
                subtitle: Some("A company is who sells in law: its tenants' invoices name it, and they count together towards its VAT obligations."),
                <a class=(button_variants(ButtonVariant::Primary, ButtonSize::Md)) href=(href!(new::new))>
                    icon(data: icons::PLUS)
                    "Register a company"
                </a>
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())
            section_card(
                title: Some("Your companies"),
                if mine.is_empty() {
                    empty_state(
                        title: "No company yet",
                        hint: Some("Register the company your tenants sell as, then link them from their Invoicing page."),
                        symbol: Some(icons::COMPANIES),
                    )
                } else {
                    table(
                        table_header(
                            table_row(
                                table_head("Company")
                                table_head("SIREN")
                            )
                        )
                        table_body(
                            for row in &mine {
                                table_row(
                                    attrs: attributes! { data-company=(&row.siren) },
                                    table_cell(
                                        <a class=(LINK) href=(href!(company_id::index, company_id::CompanyId(row.id.clone())))>(&row.legal_name)</a>
                                    )
                                    table_cell(<code class="text-xs">(&row.siren)</code>)
                                )
                            }
                        )
                    )
                }
            )
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
        timada_core::format_amount(cents)
    }
}

/// A `<fieldset>` drawn as a card. The legend floats so the browser lays it
/// out as the card's heading rather than on its border.
#[component]
async fn identity_group(
    legend: &str,
    #[default] description: Option<&str>,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    Ok(view! {
        <fieldset class=(class!(CARD, "mb-6 gap-4 px-6"))>
            <legend class="float-left w-full text-base font-semibold">(legend)</legend>
            if let Some(description) = description {
                <p class="-mt-2 text-sm text-muted-foreground">(description)</p>
            }
            <div class="grid gap-4 sm:grid-cols-2">(child)</div>
        </fieldset>
    })
}

/// The fields of the identity form, filled with `legal`.
#[component]
pub async fn identity_fields(legal: &LegalIdentity) -> Result<impl View> {
    let capital = legal
        .share_capital_cents
        .map(format_capital)
        .unwrap_or_default();
    Ok(view! {
        identity_group(
            legend: "Company",
            field(
                attrs: attributes! { class="sm:col-span-2" },
                field_label(attrs: attributes! { for="legal-name" }, "Legal name")
                input(attrs: attributes! { id="legal-name" name="legal_name" required="" maxlength="120" value=(&legal.legal_name) })
            )
            field(
                field_label(attrs: attributes! { for="legal-form" }, "Legal form (SAS, SARL, EI…)")
                input(attrs: attributes! { id="legal-form" name="legal_form" required="" maxlength="40" value=(&legal.legal_form) })
            )
            field(
                field_label(attrs: attributes! { for="legal-capital" }, "Share capital in euros (blank for none)")
                input(attrs: attributes! { id="legal-capital" name="share_capital" inputmode="decimal" value=(&capital) })
            )
            field(
                field_label(attrs: attributes! { for="legal-siret" }, "SIRET")
                input(attrs: attributes! { id="legal-siret" name="siret" required="" inputmode="numeric" value=(&legal.siret) class="font-mono" })
            )
            field(
                field_label(attrs: attributes! { for="legal-rcs" }, "RCS city (blank if not registered)")
                input(attrs: attributes! { id="legal-rcs" name="rcs_city" maxlength="100" value=(legal.rcs_city.as_deref().unwrap_or("")) })
            )
            field(
                attrs: attributes! { class="sm:col-span-2" },
                field_label(attrs: attributes! { for="legal-email" }, "Contact e-mail")
                input(attrs: attributes! { id="legal-email" name="contact_email" type="email" required="" value=(&legal.contact_email) })
            )
        )

        identity_group(
            legend: "Registered address",
            field(
                attrs: attributes! { class="sm:col-span-2" },
                field_label(attrs: attributes! { for="legal-line1" }, "Address")
                input(attrs: attributes! { id="legal-line1" name="line1" required="" maxlength="200" value=(&legal.line1) })
            )
            field(
                attrs: attributes! { class="sm:col-span-2" },
                field_label(attrs: attributes! { for="legal-line2" }, "Address, line 2")
                input(attrs: attributes! { id="legal-line2" name="line2" maxlength="200" value=(legal.line2.as_deref().unwrap_or("")) })
            )
            field(
                field_label(attrs: attributes! { for="legal-postal" }, "Postal code")
                input(attrs: attributes! { id="legal-postal" name="postal_code" required="" maxlength="20" value=(&legal.postal_code) })
            )
            field(
                field_label(attrs: attributes! { for="legal-city" }, "City")
                input(attrs: attributes! { id="legal-city" name="city" required="" maxlength="100" value=(&legal.city) })
            )
            field(
                field_label(attrs: attributes! { for="legal-country" }, "Country (two letters)")
                input(attrs: attributes! { id="legal-country" name="country" required="" maxlength="2" value=(&legal.country) class="w-24 uppercase" })
            )
        )

        identity_group(
            legend: "VAT",
            description: Some("Prices in the catalog include VAT. Without a VAT number, invoices say “TVA non applicable, art. 293 B du CGI”."),
            field(
                attrs: attributes! { class="sm:col-span-2" },
                field_label(attrs: attributes! { for="legal-vat" }, "Intra-community VAT number (blank under the exemption)")
                input(attrs: attributes! { id="legal-vat" name="vat_number" maxlength="40" placeholder="FR00123456789" value=(legal.vat_number.as_deref().unwrap_or("")) class="font-mono" })
            )
            <label class="flex items-start gap-2 text-sm sm:col-span-2">
                checkbox(attrs: attributes! { name="oss_registered" value="on" checked=(legal.oss_registered) class="mt-0.5" })
                <span>"Registered for the EU One-Stop Shop (OSS): charge other EU countries' VAT to their consumers from the first euro"</span>
            </label>
            <p class="text-sm text-muted-foreground sm:col-span-2">
                "Without it, other EU countries' VAT applies once your sales to their consumers pass €10,000 in the year, "
                "and your shops sell to those consumers in euros only: only euro sales count towards the threshold, and a sale "
                "in another currency would owe their VAT, which only the One-Stop Shop lets you remit."
            </p>
        )
    })
}
