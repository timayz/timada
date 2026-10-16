//! `/t/{slug}/offers`: the checkout offers of a tenant, for members with
//! `discounts:read`; those with `discounts:manage` create, deactivate and
//! archive them. An offer prices one unit of a product (or of one variant)
//! below its price while the cart meets its conditions. Read from the
//! tenant's own stream, so a change shows at once.

pub mod create;
pub mod offer_id;

use std::collections::HashMap;

use timada_catalog::{ProductFilter, ProductStatus, format_money, products_of_tenant};
use timada_discounts::{MAX_OFFER_TITLE_LEN, Offer, unix_now};
use timada_tenancy::{PERMISSION_DISCOUNTS_MANAGE, PERMISSION_DISCOUNTS_READ};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params},
    view::{View, view},
};

use super::discounts::{describe_window, discounts};
use super::{Slug, TenantAccess, TenantNav, require_tenant, tenant_nav};
use crate::app::auth::Flash;
use crate::db::ReadPool;
use crate::ui::{document, notice};

/// How many products the pickers list, most recently changed first.
const PICKER_PRODUCTS: i64 = 200;

#[topcoat::router::query_params(error = bad_request)]
struct OffersQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn offers_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, Slug(slug.to_owned()))
        .query(Flash { error, message })
        .resolve(cx)
}

/// The value of an item in the picker: `product` or `product:variant`.
pub fn item_value(product_id: &str, variant_id: Option<&str>) -> String {
    match variant_id {
        Some(variant_id) => format!("{product_id}:{variant_id}"),
        None => product_id.to_owned(),
    }
}

/// One thing an offer can be for, as the picker lists it.
struct Item {
    value: String,
    label: String,
}

/// The items the tenant sells (each variant of a product that has some),
/// and the products by id, for the pickers and the list.
async fn catalog_items(
    cx: &Cx,
    access: &TenantAccess,
) -> Result<(Vec<Item>, Vec<(String, String)>, HashMap<String, String>)> {
    let pool = app_context::<ReadPool>(cx);
    let rows = products_of_tenant(
        pool,
        &access.tenant.id,
        &ProductFilter::default(),
        0,
        PICKER_PRODUCTS,
    )
    .await
    .map_err(|err| topcoat::Error::from_anyhow(err.into()))?;
    let catalog = timada_catalog::Command(access.executor()?.clone());
    let mut items = Vec::new();
    let mut products = Vec::new();
    let mut labels = HashMap::new();
    for row in rows
        .iter()
        .filter(|row| row.status() != ProductStatus::Archived)
    {
        products.push((row.id.clone(), row.name.clone()));
        if row.variant_count == 0 {
            let price = row.price_label().unwrap_or_else(|| "no price".to_owned());
            let label = format!("{} ({price})", row.name);
            labels.insert(item_value(&row.id, None), row.name.clone());
            items.push(Item {
                value: item_value(&row.id, None),
                label,
            });
            continue;
        }
        let Some(product) = catalog
            .load_product(&row.id)
            .await
            .map_err(topcoat::Error::from_anyhow)?
        else {
            continue;
        };
        for variant in &product.variants {
            let values: Vec<&str> = variant
                .selections
                .iter()
                .map(|selection| selection.value.as_str())
                .collect();
            let name = format!("{} — {}", product.name, values.join(" / "));
            let value = item_value(&product.id, Some(&variant.id));
            labels.insert(value.clone(), name.clone());
            items.push(Item {
                label: format!("{name} ({})", variant.price),
                value,
            });
        }
    }
    Ok((items, products, labels))
}

/// What has to hold for an offer to apply, in words.
fn describe_conditions(offer: &Offer, products: &[(String, String)]) -> String {
    let mut parts = Vec::new();
    if let Some(required) = &offer.requires_product_id {
        let name = products
            .iter()
            .find(|(id, _)| id == required)
            .map_or("a product no longer sold", |(_, name)| name.as_str());
        parts.push(format!("Cart holds {name}"));
    }
    if let Some(min) = offer.min_subtotal_cents {
        parts.push(format!(
            "Rest of the cart from {}",
            format_money(min, &offer.currency)
        ));
    }
    if parts.is_empty() {
        "Anything else in the cart".to_owned()
    } else {
        parts.join(", ")
    }
}

/// Where an offer stands, in words.
fn describe_status(offer: &Offer, now: u64) -> &'static str {
    if !offer.active {
        "Deactivated"
    } else if offer.starts_at.is_some_and(|start| now < start) {
        "Not started yet"
    } else if offer.ends_at.is_some_and(|end| end <= now) {
        "Over"
    } else {
        "Active"
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_DISCOUNTS_READ)
        .await?;
    let can_manage = access.can(cx, PERMISSION_DISCOUNTS_MANAGE).await?;
    let query = query_params::<OffersQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let offers = discounts(&access)?
        .offers()
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let (items, products, labels) = catalog_items(cx, &access).await?;
    let now = unix_now();
    let rows: Vec<(Offer, String, String, String, &'static str)> = offers
        .into_iter()
        .map(|offer| {
            let item = labels
                .get(&item_value(&offer.product_id, offer.variant_id.as_deref()))
                .cloned()
                .unwrap_or_else(|| "A product no longer sold".to_owned());
            let conditions = describe_conditions(&offer, &products);
            let window = describe_window(offer.starts_at, offer.ends_at);
            let status = describe_status(&offer, now);
            (offer, item, conditions, window, status)
        })
        .collect();
    let TenantAccess { tenant, .. } = access;
    let slug = tenant.slug.clone();
    let title = format!("Offers · {}", tenant.name);

    Ok(view! {
        document(
            title: &title,
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">"Offers"</h1>
                tenant_nav(tenant: &tenant, nav: &nav, current: "offers")
            </header>
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <p class="mb-4 text-sm text-neutral-500">
                    "The checkout shows an offer as an \"Exclusive offer\" card while the cart meets its conditions: one unit of the item at the offer's price, the others at full price. The order and its invoice bill that unit at the offer's price; a discount code then comes off the whole."
                </p>
                <table class="w-full text-left text-sm">
                    <thead>
                        <tr class="border-b border-neutral-200 text-neutral-500">
                            <th class="py-2 font-medium">"Item"</th>
                            <th class="py-2 font-medium">"Offer price"</th>
                            <th class="py-2 font-medium">"Conditions"</th>
                            <th class="py-2 font-medium">"When"</th>
                            <th class="py-2 font-medium">"Status"</th>
                            if can_manage {
                                <th class="py-2 font-medium"><span class="sr-only">"Actions"</span></th>
                            }
                        </tr>
                    </thead>
                    <tbody class="divide-y divide-neutral-100">
                        if rows.is_empty() {
                            <tr><td class="py-2 text-neutral-500" colspan="6">"No offer yet."</td></tr>
                        }
                        for (offer, item, conditions, window, status) in &rows {
                            <tr data-offer=(item)>
                                <td class="py-2">
                                    (item)
                                    if let Some(heading) = &offer.title {
                                        <div class="text-neutral-500">"“" (heading) "”"</div>
                                    }
                                </td>
                                <td class="py-2" data-offer-price="">(format_money(offer.price_cents, &offer.currency))</td>
                                <td class="py-2" data-offer-conditions="">(conditions)</td>
                                <td class="py-2">(window)</td>
                                <td class="py-2" data-offer-status=(if offer.active { "active" } else { "inactive" })>(*status)</td>
                                if can_manage {
                                    <td class="py-2">
                                        <div class="flex justify-end gap-2">
                                            if offer.active {
                                                <form method="post" action=(href!(offer_id::deactivate::deactivate, Slug(slug.clone()), offer_id::OfferId(offer.id.clone())))>
                                                    <button class="rounded border border-neutral-300 px-3 py-1" aria-label=(format!("Deactivate the offer on {item}"))>"Deactivate"</button>
                                                </form>
                                            } else {
                                                <form method="post" action=(href!(offer_id::reactivate::reactivate, Slug(slug.clone()), offer_id::OfferId(offer.id.clone())))>
                                                    <button class="rounded border border-neutral-300 px-3 py-1" aria-label=(format!("Reactivate the offer on {item}"))>"Reactivate"</button>
                                                </form>
                                            }
                                            <form method="post" action=(href!(offer_id::archive::archive, Slug(slug.clone()), offer_id::OfferId(offer.id.clone())))>
                                                <button class="rounded border border-red-300 px-3 py-1 text-red-700" aria-label=(format!("Archive the offer on {item}"))>"Archive"</button>
                                            </form>
                                        </div>
                                    </td>
                                }
                            </tr>
                        }
                    </tbody>
                </table>
            </section>

            if can_manage {
                <section class="rounded border border-neutral-200 bg-white p-4">
                    <h2 class="mb-2 text-lg font-medium">"Create an offer"</h2>
                    if items.is_empty() {
                        <p class="text-sm text-neutral-500">"Add a product first: an offer is for one of them."</p>
                    } else {
                        <form method="post" action=(href!(create::create, Slug(slug.clone()))) class="space-y-3">
                            <label class="block text-sm" for="new-offer-item">"Item"</label>
                            <select id="new-offer-item" name="item" required="" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2">
                                for item in &items {
                                    <option value=(&item.value)>(&item.label)</option>
                                }
                            </select>
                            <label class="block text-sm" for="new-offer-price">"Offer price"</label>
                            <input id="new-offer-price" name="price" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2"
                                type="text" inputmode="decimal" required="" placeholder="12.00">
                            <p class="text-sm text-neutral-500">"In the product's currency, below its price."</p>
                            <label class="block text-sm" for="new-offer-title">"Title (optional)"</label>
                            <input id="new-offer-title" name="title" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2"
                                type="text" maxlength=(MAX_OFFER_TITLE_LEN) placeholder="Add a smart GPS tracker at 60% off">
                            <label class="block text-sm" for="new-offer-requires">"Only when the cart holds (optional)"</label>
                            <select id="new-offer-requires" name="requires_product_id" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2">
                                <option value="">"Any product"</option>
                                for (id, name) in &products {
                                    <option value=(id)>(name)</option>
                                }
                            </select>
                            <label class="block text-sm" for="new-offer-min">"Only when the rest of the cart is worth at least (optional)"</label>
                            <input id="new-offer-min" name="min_subtotal" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2"
                                type="text" inputmode="decimal" placeholder="50.00">
                            <div class="flex gap-4">
                                <div class="flex-1">
                                    <label class="block text-sm" for="new-offer-starts">"First day (optional)"</label>
                                    <input id="new-offer-starts" name="starts_on" type="date" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2">
                                </div>
                                <div class="flex-1">
                                    <label class="block text-sm" for="new-offer-ends">"Last day (optional)"</label>
                                    <input id="new-offer-ends" name="ends_on" type="date" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2">
                                </div>
                            </div>
                            <p class="text-sm text-neutral-500">"Days are counted in UTC. The rest of the cart is what it holds besides the one unit the offer prices."</p>
                            <button class="rounded bg-neutral-900 px-4 py-2 font-medium text-white hover:bg-neutral-700">"Create offer"</button>
                        </form>
                    }
                </section>
            }
        )
    })
}
