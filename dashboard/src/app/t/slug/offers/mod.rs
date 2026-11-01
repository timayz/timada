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
    view::{View, attributes, view},
};

use super::discounts::{describe_window, discounts};
use super::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::app::auth::Flash;
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::field::{field, field_description, field_label};
use crate::components::input::input;
use crate::components::select::select;
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::db::Db;
use crate::ui::{
    Shell, Tone, document, empty_row, notice, page_header, section_card, status_badge,
};

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
    let pool = app_context::<Db>(cx);
    let rows = products_of_tenant(
        pool,
        &access.tenant.id,
        &ProductFilter::default(),
        0,
        PICKER_PRODUCTS,
    )
    .await
    .map_err(topcoat::Error::from_anyhow)?;
    let catalog = timada_catalog::Command::new(access.executor()?.clone(), access.tenant_id());
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
            shell: Shell::tenant(&tenant, &nav, "offers"),
            page_header(title: "Offers", subtitle: Some("One unit of a product below its price while the cart meets the conditions."))
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                description: Some("The checkout shows an offer as an \"Exclusive offer\" card while the cart meets its conditions: one unit of the item at the offer's price, the others at full price. The order and its invoice bill that unit at the offer's price; a discount code then comes off the whole."),
                table(
                    table_header(
                        table_row(
                            table_head("Item")
                            table_head("Offer price")
                            table_head("Conditions")
                            table_head("When")
                            table_head("Status")
                            if can_manage {
                                table_head(<span class="sr-only">"Actions"</span>)
                            }
                        )
                    )
                    table_body(
                        if rows.is_empty() {
                            empty_row(colspan: 6, text: "No offer yet.")
                        }
                        for (offer, item, conditions, window, status) in &rows {
                            table_row(
                                attrs: attributes! { data-offer=(item) },
                                table_cell(
                                    attrs: attributes! { class="whitespace-normal" },
                                    <span class="font-medium">(item)</span>
                                    if let Some(heading) = &offer.title {
                                        <div class="text-xs text-muted-foreground">"“" (heading) "”"</div>
                                    }
                                )
                                table_cell(attrs: attributes! { class="tabular-nums" data-offer-price="" }, (format_money(offer.price_cents, &offer.currency)))
                                table_cell(attrs: attributes! { class="whitespace-normal" data-offer-conditions="" }, (conditions))
                                table_cell((window))
                                table_cell(
                                    attrs: attributes! { data-offer-status=(if offer.active { "active" } else { "inactive" }) },
                                    status_badge(tone: offer_tone(offer.active, status), (*status))
                                )
                                if can_manage {
                                    table_cell(
                                        <div class="flex justify-end gap-2">
                                            if offer.active {
                                                <form method="post" action=(href!(offer_id::deactivate::deactivate, Slug(slug.clone()), offer_id::OfferId(offer.id.clone())))>
                                                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" aria-label=(format!("Deactivate the offer on {item}")) }, "Deactivate")
                                                </form>
                                            } else {
                                                <form method="post" action=(href!(offer_id::reactivate::reactivate, Slug(slug.clone()), offer_id::OfferId(offer.id.clone())))>
                                                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" aria-label=(format!("Reactivate the offer on {item}")) }, "Reactivate")
                                                </form>
                                            }
                                            <form method="post" action=(href!(offer_id::archive::archive, Slug(slug.clone()), offer_id::OfferId(offer.id.clone())))>
                                                button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" aria-label=(format!("Archive the offer on {item}")) }, "Archive")
                                            </form>
                                        </div>
                                    )
                                }
                            )
                        }
                    )
                )
            )

            if can_manage {
                section_card(
                    title: Some("Create an offer"),
                    if items.is_empty() {
                        <p class="text-sm text-muted-foreground">"Add a product first: an offer is for one of them."</p>
                    } else {
                        <form method="post" action=(href!(create::create, Slug(slug.clone()))) class="flex max-w-xl flex-col gap-4">
                            field(
                                field_label(attrs: attributes! { for="new-offer-item" }, "Item")
                                select(
                                    attrs: attributes! { id="new-offer-item" name="item" required="" },
                                    for item in &items {
                                        <option value=(&item.value)>(&item.label)</option>
                                    }
                                )
                            )
                            field(
                                field_label(attrs: attributes! { for="new-offer-price" }, "Offer price")
                                input(attrs: attributes! {
                                    id="new-offer-price" name="price"
                                    type="text" inputmode="decimal" required="" placeholder="12.00"
                                })
                                field_description("In the product's currency, below its price.")
                            )
                            field(
                                field_label(attrs: attributes! { for="new-offer-title" }, "Title (optional)")
                                input(attrs: attributes! {
                                    id="new-offer-title" name="title"
                                    type="text" maxlength=(MAX_OFFER_TITLE_LEN) placeholder="Add a smart GPS tracker at 60% off"
                                })
                            )
                            field(
                                field_label(attrs: attributes! { for="new-offer-requires" }, "Only when the cart holds (optional)")
                                select(
                                    attrs: attributes! { id="new-offer-requires" name="requires_product_id" },
                                    <option value="">"Any product"</option>
                                    for (id, name) in &products {
                                        <option value=(id)>(name)</option>
                                    }
                                )
                            )
                            field(
                                field_label(attrs: attributes! { for="new-offer-min" }, "Only when the rest of the cart is worth at least (optional)")
                                input(attrs: attributes! {
                                    id="new-offer-min" name="min_subtotal"
                                    type="text" inputmode="decimal" placeholder="50.00"
                                })
                            )
                            <div class="grid gap-4 sm:grid-cols-2">
                                field(
                                    field_label(attrs: attributes! { for="new-offer-starts" }, "First day (optional)")
                                    input(attrs: attributes! { id="new-offer-starts" name="starts_on" type="date" })
                                )
                                field(
                                    field_label(attrs: attributes! { for="new-offer-ends" }, "Last day (optional)")
                                    input(attrs: attributes! { id="new-offer-ends" name="ends_on" type="date" })
                                )
                            </div>
                            <p class="text-sm text-muted-foreground">"Days are counted in UTC. The rest of the cart is what it holds besides the one unit the offer prices."</p>
                            <div>
                                button(attrs: attributes! { type="submit" }, "Create offer")
                            </div>
                        </form>
                    }
                )
            }
        )
    })
}

/// The colour of where an offer stands.
fn offer_tone(active: bool, status: &str) -> Tone {
    match (active, status) {
        (false, _) => Tone::Warning,
        (true, "Active") => Tone::Success,
        (true, _) => Tone::Info,
    }
}
