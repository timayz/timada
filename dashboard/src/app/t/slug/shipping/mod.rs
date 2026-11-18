//! `/t/{slug}/shipping`: the ways a tenant ships, for members with
//! `shipping:manage`. Read from the tenant's own stream, so a change shows at
//! once. While the list is empty, shipping is included in the prices; once
//! it is not, a shopper picks one of the methods that ship to their country.

pub mod add;
pub mod delivery_days;
pub mod method_id;
pub mod names;
pub mod returns;

use std::collections::HashMap;

use serde::Deserialize;
use timada_catalog::{CURRENCIES, MAX_DELIVERY_DAYS, format_money, parse_amount};
use timada_orders::{
    Command, MAX_RETURN_DAYS, MAX_SHIPPING_NAME_LEN, NewShippingMethod, OrderError, ShippingMethod,
};
use timada_tenancy::PERMISSION_SHIPPING_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{href, page, query_params},
    view::{Unescaped, View, attributes, component, view},
};

use super::{TenantAccess, TenantNav, require_tenant, slug};
use crate::app::auth::Flash;
use crate::cluster::Executor;
use crate::components::button::button;
use crate::components::checkbox::checkbox;
use crate::components::field::{field, field_label};
use crate::components::input::input;
use crate::components::select::select;
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::ui::{LINK, Shell, document, empty_row, notice, page_header, section_card};

#[topcoat::router::query_params(error = bad_request)]
struct ShippingQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn shipping_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, slug: slug.to_owned())
        .query(Flash { error, message })
        .resolve(cx)
}

/// The orders gateway in the tenant's streams (403 while suspended).
pub fn orders(access: &TenantAccess) -> Result<Command<Executor>> {
    Ok(Command::new(access.executor()?.clone(), access.tenant_id()))
}

/// The query-string code of a user-facing error.
pub fn error_code(err: &OrderError) -> &'static str {
    err.code().unwrap_or("unexpected")
}

/// A method as the merchant fills it in, to add or to change.
#[derive(Deserialize)]
pub struct MethodForm {
    name: String,
    price: String,
    currency: String,
    #[serde(default)]
    free_above: String,
    /// The ticked country checkboxes, among anything else posted.
    #[serde(flatten)]
    fields: HashMap<String, String>,
}

impl MethodForm {
    /// The method the form describes, or `InvalidShippingPrice` for an
    /// amount that is not one.
    pub fn method(self) -> std::result::Result<NewShippingMethod, OrderError> {
        let amount = |text: &str| parse_amount(text).map_err(|_| OrderError::InvalidShippingPrice);
        let free_above_cents = match self.free_above.trim() {
            "" => None,
            text => Some(amount(text)?),
        };
        Ok(NewShippingMethod {
            name: self.name,
            price_cents: amount(&self.price)?,
            currency: self.currency,
            countries: checked_countries(&self.fields),
            free_above_cents,
        })
    }
}

/// The form field of a country checkbox: `country:<code>`.
const COUNTRY_FIELD_PREFIX: &str = "country:";

/// The countries ticked in a posted method form.
fn checked_countries(fields: &HashMap<String, String>) -> Vec<String> {
    fields
        .keys()
        .filter_map(|key| key.strip_prefix(COUNTRY_FIELD_PREFIX))
        .map(str::to_owned)
        .collect()
}

/// Narrows the country list to what the merchant types, by name or code.
const COUNTRY_FILTER_SCRIPT: &str = r#"
for (const filter of document.querySelectorAll("[data-country-filter]")) {
  const list = document.getElementById(filter.dataset.countryFilter);
  filter.hidden = false;
  filter.addEventListener("input", () => {
    const text = filter.value.trim().toLowerCase();
    for (const row of list.querySelectorAll("[data-country]")) {
      const shown = text === "" || row.textContent.toLowerCase().includes(text);
      row.style.display = shown ? "" : "none";
    }
  });
}
"#;

/// The countries a method ships to, as checkboxes: those `selected` first,
/// then every ISO 3166 country by name. None ticked ships everywhere.
#[component]
pub async fn countries_field(cx: &Cx, id: &str, selected: &[String]) -> Result<impl View> {
    let mut rows: Vec<(String, String, bool)> = selected
        .iter()
        .map(|code| {
            let name =
                rust_iso3166::from_alpha2(code).map_or(code.as_str(), |country| country.name);
            (code.clone(), name.to_owned(), true)
        })
        .collect();
    rows.extend(
        rust_iso3166::ALL
            .iter()
            .filter(|country| !selected.iter().any(|code| code == country.alpha2))
            .map(|country| (country.alpha2.to_owned(), country.name.to_owned(), false)),
    );
    let list = format!("{id}-list");

    Ok(view! {
        <fieldset id=(id) class="flex flex-col gap-2">
            <legend class="text-sm font-medium">"Countries (optional)"</legend>
            input(attrs: attributes! {
                type="search" aria-label="Filter countries" placeholder="Filter countries" hidden=""
                data-country-filter=(&list)
            })
            <div id=(&list) class="max-h-64 overflow-y-auto rounded-lg border border-border p-2 text-sm">
                for (code, name, checked) in &rows {
                    <label class="flex items-center gap-2 py-0.5" data-country=(code)>
                        checkbox(attrs: attributes! { name=(format!("{COUNTRY_FIELD_PREFIX}{code}")) value="on" checked=(*checked) })
                        <span>(name) " " <code class="text-muted-foreground">(code)</code></span>
                    </label>
                }
            </div>
            <p class="text-sm text-muted-foreground">"Tick the countries it ships to. Leave them all unticked to ship everywhere."</p>
            <script>(Unescaped::new_unchecked(COUNTRY_FILTER_SCRIPT))</script>
        </fieldset>
    })
}

/// Where a method ships, in words.
pub fn describe_countries(method: &ShippingMethod) -> String {
    if method.countries.is_empty() {
        "Everywhere".to_owned()
    } else {
        method.countries.join(", ")
    }
}

/// When a method stops costing anything, in words.
pub fn describe_free_above(method: &ShippingMethod) -> String {
    match method.free_above_cents {
        Some(cents) => format!("From {}", format_money(cents, &method.currency)),
        None => "Never".to_owned(),
    }
}

/// Minor units as the price field shows them: `490` gives `4.90`.
pub fn amount_field(cents: u64) -> String {
    timada_core::format_amount(cents)
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_SHIPPING_MANAGE)
        .await?;
    let query = query_params::<ShippingQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let methods = orders(&access)?
        .shipping_methods()
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let default_days = crate::app::t::slug::products::catalog(&access)?
        .load_default_delivery_days()
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let return_terms = orders(&access)?
        .return_policy()
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    // What each of the shop's other languages calls the methods: a method's
    // id, its name, and the name there.
    let other_languages = crate::app::t::slug::announcements::pages(&access)?
        .languages()
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .map(|languages| languages.others)
        .unwrap_or_default();
    let mut named_in: Vec<(String, String, Vec<(String, String, String)>)> = Vec::new();
    if !methods.is_empty() {
        for language in other_languages {
            let names = orders(&access)?
                .shipping_names(&language)
                .await
                .map_err(topcoat::Error::from_anyhow)?;
            let rows = methods
                .iter()
                .map(|method| {
                    let name = names.get(&method.id).cloned().unwrap_or_default();
                    (method.id.clone(), method.name.clone(), name)
                })
                .collect();
            let heading = format!(
                "Shipping methods in {}",
                timada_core::languages::language_name(&language).unwrap_or(&language)
            );
            named_in.push((language, heading, rows));
        }
    }
    let TenantAccess { tenant, .. } = access;
    let slug = tenant.slug.clone();
    let title = format!("Shipping · {}", tenant.name);

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "shipping"),
            page_header(title: "Shipping", subtitle: Some("The ways the shop ships, and what each costs."))
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                title: Some("Shipping methods"),
                description: Some("A shopper picks one of the methods that ship to their country, and pays its price on top of the items. Without any method, shipping is included in the prices. The invoice charges shipping at the VAT rates of the goods it carries."),
                table(
                    table_header(
                        table_row(
                            table_head("Method")
                            table_head("Price")
                            table_head("Free")
                            table_head("Countries")
                        )
                    )
                    table_body(
                        if methods.is_empty() {
                            empty_row(colspan: 4, text: "No shipping method: shipping is included.")
                        }
                        for method in &methods {
                            table_row(
                                attrs: attributes! { data-shipping-method=(&method.name) },
                                table_cell(
                                    <a class=(LINK) href=(href!(method_id::index, slug: slug.clone(), method_id::method_id: method.id.clone()))>(&method.name)</a>
                                )
                                table_cell(attrs: attributes! { class="tabular-nums" data-shipping-price="" }, (format_money(method.price_cents, &method.currency)))
                                table_cell(attrs: attributes! { data-shipping-free="" }, (describe_free_above(method)))
                                table_cell(attrs: attributes! { class="whitespace-normal" data-shipping-countries="" }, (describe_countries(method)))
                            )
                        }
                    )
                )
            )

            for (language, heading, rows) in &named_in {
                section_card(
                    title: Some(heading),
                    description: Some("What a shopper reading this language is shown for each method, in the cart, at checkout and with their order. One left empty is read in the main language."),
                    attrs: attributes! { data-shipping-names=(language) },
                    <form method="post" action=(href!(names::names, slug: slug.clone())) class="flex max-w-xl flex-col gap-4">
                        <input type="hidden" name="language" value=(language) />
                        for (id, name, theirs) in rows {
                            field(
                                field_label(attrs: attributes! { for=(format!("shipping-name-{language}-{id}")) }, (name))
                                input(attrs: attributes! {
                                    id=(format!("shipping-name-{language}-{id}")) name=(format!("{}{id}", names::NAME_FIELD_PREFIX)) type="text"
                                    maxlength=(MAX_SHIPPING_NAME_LEN) value=(theirs) placeholder=(name) lang=(language) autocomplete="off"
                                })
                            )
                        }
                        <div>button(attrs: attributes! { type="submit" }, "Save names")</div>
                    </form>
                )
            }

            section_card(
                title: Some("Default delivery time"),
                description: Some("How long delivery takes for a product that gives no time of its own. Shoppers see it on the product page, in the cart, at checkout and with their order. Products imported from AliExpress take theirs from there."),
                <form method="post" action=(href!(delivery_days::delivery_days, slug: slug.clone())) class="flex max-w-xl flex-col gap-4" data-default-delivery=(default_days.map(crate::ui::delivery_days_label).unwrap_or_default())>
                    <fieldset class="grid gap-2">
                        <legend class="text-sm font-medium">"Delivery time, in days"</legend>
                        <div class="flex items-center gap-2">
                            <label class="sr-only" for="default-delivery-min">"Shortest delivery, in days"</label>
                            input(attrs: attributes! { id="default-delivery-min" name="min" type="number" min="1" max=(MAX_DELIVERY_DAYS) inputmode="numeric" class="w-24" value=(default_days.map(|days| days.min.to_string()).unwrap_or_default()) })
                            <span aria-hidden="true">"to"</span>
                            <label class="sr-only" for="default-delivery-max">"Longest delivery, in days"</label>
                            input(attrs: attributes! { id="default-delivery-max" name="max" type="number" min="1" max=(MAX_DELIVERY_DAYS) inputmode="numeric" class="w-24" value=(default_days.map(|days| days.max.to_string()).unwrap_or_default()) })
                        </div>
                        <p class="text-sm text-muted-foreground">"Leave both empty to announce no delivery time."</p>
                    </fieldset>
                    <div>
                        button(attrs: attributes! { type="submit" }, "Set default delivery time")
                    </div>
                </form>
            )

            section_card(
                title: Some("Returns"),
                description: Some("What search engines show of your returns beside your prices: for how long a buyer may send goods back, and who pays for it. Your returns page says the rest. They are stated for the countries your shipping methods name, with what each method costs."),
                <form method="post" action=(href!(returns::returns, slug: slug.clone())) class="flex max-w-xl flex-col gap-4" data-return-terms=(return_terms.map(|terms| terms.days.to_string()).unwrap_or_default())>
                    field(
                        field_label(attrs: attributes! { for="return-days" }, "Return window, in days")
                        input(attrs: attributes! {
                            id="return-days" name="days" type="number" min="1" max=(MAX_RETURN_DAYS) inputmode="numeric" class="w-24"
                            value=(return_terms.map(|terms| terms.days.to_string()).unwrap_or_default())
                            aria-describedby="return-days-hint"
                        })
                        <p id="return-days-hint" class="text-sm text-muted-foreground">"Counted from the delivery. Leave empty to state nothing."</p>
                    )
                    <label class="flex items-center gap-2 text-sm font-medium">
                        checkbox(attrs: attributes! { name="free" value="on" checked=(return_terms.is_some_and(|terms| terms.free)) })
                        "The shop pays for sending goods back"
                    </label>
                    <div>
                        button(attrs: attributes! { type="submit" }, "Set return terms")
                    </div>
                </form>
            )

            section_card(
                title: Some("Add a shipping method"),
                <form method="post" action=(href!(add::add, slug: slug.clone())) class="flex max-w-xl flex-col gap-4">
                    field(
                        field_label(attrs: attributes! { for="new-shipping-name" }, "Name")
                        input(attrs: attributes! {
                            id="new-shipping-name" name="name" type="text"
                            maxlength=(MAX_SHIPPING_NAME_LEN) required="" placeholder="Tracked delivery"
                        })
                    )
                    field(
                        field_label(attrs: attributes! { for="new-shipping-price" }, "Price")
                        <div class="flex gap-2">
                            input(attrs: attributes! {
                                id="new-shipping-price" name="price" type="text" inputmode="decimal" required="" placeholder="4.90"
                            })
                            select(
                                attrs: attributes! { name="currency" aria-label="Currency" class="w-28 shrink-0" },
                                for currency in CURRENCIES {
                                    <option value=(*currency)>(*currency)</option>
                                }
                            )
                        </div>
                    )
                    field(
                        field_label(attrs: attributes! { for="new-shipping-free" }, "Free from (optional)")
                        input(attrs: attributes! {
                            id="new-shipping-free" name="free_above" type="text" inputmode="decimal" placeholder="50.00"
                        })
                        <p class="text-sm text-muted-foreground">"Shipping costs nothing once the items, less any discount, reach this amount."</p>
                    )
                    countries_field(id: "new-shipping-countries", selected: &[])
                    <div>
                        button(attrs: attributes! { type="submit" }, "Add shipping method")
                    </div>
                </form>
            )
        )
    })
}
