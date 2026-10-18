//! `/t/{slug}/shipping`: the ways a tenant ships, for members with
//! `shipping:manage`. Read from the tenant's own stream, so a change shows at
//! once. While the list is empty, shipping is included in the prices; once
//! it is not, a shopper picks one of the methods that ship to their country.

pub mod add;
pub mod method_id;

use std::collections::HashMap;

use serde::Deserialize;
use timada_catalog::{CURRENCIES, format_money, parse_amount};
use timada_orders::{
    Command, MAX_SHIPPING_NAME_LEN, NewShippingMethod, OrdersError, ShippingMethod,
};
use timada_tenancy::PERMISSION_SHIPPING_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{href, page, query_params},
    view::{Unescaped, View, component, view},
};

use super::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::app::auth::Flash;
use crate::cluster::Executor;
use crate::ui::{Shell, document, notice};

#[topcoat::router::query_params(error = bad_request)]
struct ShippingQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn shipping_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, Slug(slug.to_owned()))
        .query(Flash { error, message })
        .resolve(cx)
}

/// The orders gateway over the tenant's own store (403 while suspended).
pub fn orders(access: &TenantAccess) -> Result<Command<Executor>> {
    Ok(Command(access.executor()?.clone()))
}

/// The query-string code of a user-facing error.
pub fn error_code(err: &OrdersError) -> &'static str {
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
    pub fn method(self) -> std::result::Result<NewShippingMethod, OrdersError> {
        let amount = |text: &str| parse_amount(text).map_err(|_| OrdersError::InvalidShippingPrice);
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
pub async fn countries_field(id: &str, selected: &[String]) -> Result<impl View> {
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
        <fieldset id=(id)>
            <legend class="block text-sm">"Countries (optional)"</legend>
            <input type="search" aria-label="Filter countries" placeholder="Filter countries" hidden=""
                class="mt-1 w-full rounded border border-neutral-300 px-3 py-2" data-country-filter=(&list)>
            <div id=(&list) class="mt-2 max-h-64 overflow-y-auto rounded border border-neutral-300 p-2">
                for (code, name, checked) in &rows {
                    <label class="flex items-center gap-2 py-0.5" data-country=(code)>
                        <input type="checkbox" name=(format!("{COUNTRY_FIELD_PREFIX}{code}")) value="on" checked=(*checked)>
                        <span>(name) " " <code class="text-neutral-500">(code)</code></span>
                    </label>
                }
            </div>
            <p class="mt-1 text-sm text-neutral-500">"Tick the countries it ships to. Leave them all unticked to ship everywhere."</p>
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
    format!("{}.{:02}", cents / 100, cents % 100)
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
    let TenantAccess { tenant, .. } = access;
    let slug = tenant.slug.clone();
    let title = format!("Shipping · {}", tenant.name);

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "shipping"),
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">"Shipping"</h1>
            </header>
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <p class="mb-4 text-sm text-neutral-500">
                    "A shopper picks one of the methods that ship to their country, and pays its price on top of the items. Without any method, shipping is included in the prices. The invoice charges shipping at the VAT rates of the goods it carries."
                </p>
                <table class="w-full text-left text-sm">
                    <thead>
                        <tr class="border-b border-neutral-200 text-neutral-500">
                            <th class="py-2 font-medium">"Method"</th>
                            <th class="py-2 font-medium">"Price"</th>
                            <th class="py-2 font-medium">"Free"</th>
                            <th class="py-2 font-medium">"Countries"</th>
                        </tr>
                    </thead>
                    <tbody class="divide-y divide-neutral-100">
                        if methods.is_empty() {
                            <tr><td class="py-2 text-neutral-500" colspan="4">"No shipping method: shipping is included."</td></tr>
                        }
                        for method in &methods {
                            <tr data-shipping-method=(&method.name)>
                                <td class="py-2">
                                    <a class="underline" href=(href!(method_id::index, Slug(slug.clone()), method_id::MethodId(method.id.clone())))>(&method.name)</a>
                                </td>
                                <td class="py-2" data-shipping-price="">(format_money(method.price_cents, &method.currency))</td>
                                <td class="py-2" data-shipping-free="">(describe_free_above(method))</td>
                                <td class="py-2" data-shipping-countries="">(describe_countries(method))</td>
                            </tr>
                        }
                    </tbody>
                </table>
            </section>

            <section class="rounded border border-neutral-200 bg-white p-4">
                <h2 class="mb-2 text-lg font-medium">"Add a shipping method"</h2>
                <form method="post" action=(href!(add::add, Slug(slug.clone()))) class="space-y-3">
                    <label class="block text-sm" for="new-shipping-name">"Name"</label>
                    <input id="new-shipping-name" name="name" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2"
                        type="text" maxlength=(MAX_SHIPPING_NAME_LEN) required="" placeholder="Tracked delivery">
                    <label class="block text-sm" for="new-shipping-price">"Price"</label>
                    <div class="flex gap-2">
                        <input id="new-shipping-price" name="price" class="w-full rounded border border-neutral-300 px-3 py-2"
                            type="text" inputmode="decimal" required="" placeholder="4.90">
                        <select name="currency" aria-label="Currency" class="rounded border border-neutral-300 px-3 py-2">
                            for currency in CURRENCIES {
                                <option value=(*currency)>(*currency)</option>
                            }
                        </select>
                    </div>
                    <label class="block text-sm" for="new-shipping-free">"Free from (optional)"</label>
                    <input id="new-shipping-free" name="free_above" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2"
                        type="text" inputmode="decimal" placeholder="50.00">
                    <p class="text-sm text-neutral-500">"Shipping costs nothing once the items, less any discount, reach this amount."</p>
                    countries_field(id: "new-shipping-countries", selected: &[])
                    <button class="rounded bg-neutral-900 px-4 py-2 font-medium text-white hover:bg-neutral-700">"Add shipping method"</button>
                </form>
            </section>
        )
    })
}
