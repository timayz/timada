//! `/t/{slug}/products/{product_id}`: one product, read from its own stream
//! (so an edit shows at once), with the forms that change it.

pub mod archive;
pub mod attributes;
pub mod categorize;
pub mod describe;
pub mod images;
pub mod options;
pub mod price;
pub mod publish;
pub mod rename;
pub mod stock;
pub mod tax_category;
pub mod uncategorize;
pub mod unpublish;
pub mod variants;

use std::sync::Arc;
use timada_catalog::{
    CURRENCIES, CategoryState, MAX_ALT_LEN, MAX_ATTRIBUTE_NAME_LEN, MAX_ATTRIBUTE_VALUE_LEN,
    MAX_DESCRIPTION_LEN, MAX_IMAGES_PER_PRODUCT, MAX_NAME_LEN, MAX_OPTION_NAME_LEN, MAX_OPTIONS,
    MAX_SKU_LEN, MAX_VARIANTS, ProductOption, ProductState, ProductVariant, TAX_CATEGORIES,
    format_money, tax_category_label,
};
use timada_tenancy::{
    PERMISSION_PRODUCTS_MANAGE, PERMISSION_PRODUCTS_READ, PERMISSION_REVIEWS_READ,
};

use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        error::{SeeOther, not_found, see_other},
        href, module_param, page, path_param, query_params,
    },
    view::{View, attributes, class, view},
};

use super::{catalog, error_code, status_label, status_tone};
use crate::app::auth::Flash;
use crate::app::t::slug::categories::{Choice, rows_of};
use crate::app::t::slug::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::auth::user_labels;
use crate::components::badge::{BadgeVariant, badge};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::card::CARD;
use crate::components::field::{field, field_label};
use crate::components::input::input;
use crate::components::select::select;
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::components::textarea::textarea;
use crate::db::ReadPool;
use crate::media::Media;
use crate::ui::{
    LINK, Shell, document, empty_row, fact, facts, format_time, notice, page_header, section_card,
    status_badge,
};
use timada_invoicing::format_rate_fr;
use timada_invoicing::tax::rate_bps;

module_param!(pub product_id);

#[topcoat::router::query_params(error = bad_request)]
struct ProductQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn product_url(
    cx: &Cx,
    slug: &str,
    product_id: &str,
    error: Option<&str>,
    message: Option<&str>,
) -> String {
    href!(
        index,
        Slug(slug.to_owned()),
        ProductId(product_id.to_owned())
    )
    .query(Flash { error, message })
    .resolve(cx)
}

/// The product named in the URL, or 404.
async fn require_product(cx: &Cx, access: &TenantAccess) -> Result<ProductState> {
    let id = path_param::<ProductId>(cx);
    catalog(access)?
        .load_product(id)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .ok_or_else(|| not_found().into())
}

/// What every action on a product shares: the member may manage products,
/// the product exists, and the outcome goes back to the product page.
pub(crate) async fn act<F>(cx: &Cx, message: &'static str, run: F) -> Result<SeeOther>
where
    F: AsyncFnOnce(
        &timada_catalog::Command<crate::cluster::Executor>,
        &str,
        &str,
    ) -> std::result::Result<(), timada_catalog::CatalogError>,
{
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PRODUCTS_MANAGE)
        .await?;
    let product_id = path_param::<ProductId>(cx).to_owned();
    let slug = access.tenant.slug.clone();
    let gateway = catalog(&access)?;
    match run(&gateway, &access.user_id, &product_id).await {
        Ok(()) => Ok(see_other(product_url(
            cx,
            &slug,
            &product_id,
            None,
            Some(message),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(product_url(
            cx,
            &slug,
            &product_id,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}

/// A way the product varies, ready to render: its values as the textarea
/// shows them, one a line.
pub struct OptionRow {
    pub id: String,
    pub name: String,
    pub values: String,
}

impl OptionRow {
    fn of(option: &ProductOption) -> Self {
        Self {
            id: option.id.clone(),
            name: option.name.clone(),
            values: option.values.join("\n"),
        }
    }
}

/// A combination the product sells, ready to render.
pub struct VariantRow {
    pub id: String,
    /// `Colour: Beige · Size: S`, in the product's own option order.
    pub selections: String,
    pub sku: String,
    /// The amount as the form wants it typed, without the currency.
    pub amount: String,
    pub currency: String,
    pub image_id: String,
    pub stock: StockView,
}

/// The stock of one thing, ready to render.
#[derive(Clone, Default)]
pub struct StockView {
    pub tracked: bool,
    /// On the shelf, as the form wants it typed; empty when untracked.
    pub on_hand: String,
    /// What checkouts waiting for their payment hold.
    pub held: u32,
}

impl StockView {
    fn of(stock: Option<&timada_catalog::StockState>, now: u64) -> Self {
        match stock.filter(|stock| stock.tracked) {
            Some(stock) => Self {
                tracked: true,
                on_hand: stock.on_hand.to_string(),
                held: stock.held_at(now, None),
            },
            None => Self::default(),
        }
    }

    /// `12 on hand · 2 held`, or `Not counted`.
    fn label(&self) -> String {
        match (self.tracked, self.held) {
            (false, _) => "Not counted".to_owned(),
            (true, 0) => format!("{} on hand", self.on_hand),
            (true, held) => format!("{} on hand · {held} held", self.on_hand),
        }
    }

    /// What `data-stock` says: the count, or `untracked`.
    fn data(&self) -> &str {
        if self.tracked {
            &self.on_hand
        } else {
            "untracked"
        }
    }
}

/// The stock of each thing the product sells, read from their own streams
/// so a change shows at once: one entry per variant, or one for a product
/// without variants.
async fn stocks_of(access: &TenantAccess, product: &ProductState) -> Result<Vec<StockView>> {
    let gateway = catalog(access)?;
    let now = crate::auth::now();
    let ids: Vec<Option<&str>> = if product.variants.is_empty() {
        vec![None]
    } else {
        product
            .variants
            .iter()
            .map(|v| Some(v.id.as_str()))
            .collect()
    };
    let mut stocks = Vec::with_capacity(ids.len());
    for variant_id in ids {
        let stock = gateway
            .load_stock(&product.id, variant_id)
            .await
            .map_err(topcoat::Error::from_anyhow)?;
        stocks.push(StockView::of(stock.as_ref(), now));
    }
    Ok(stocks)
}

impl VariantRow {
    fn of(variant: &ProductVariant, product: &ProductState, stock: StockView) -> Self {
        let selections = variant
            .selections
            .iter()
            .map(|selection| {
                let name = product
                    .option(&selection.option_id)
                    .map(|option| option.name.as_str())
                    .unwrap_or("?");
                format!("{name}: {}", selection.value)
            })
            .collect::<Vec<_>>()
            .join(" · ");
        Self {
            id: variant.id.clone(),
            selections,
            sku: variant.sku.clone().unwrap_or_default(),
            amount: format!(
                "{}.{:02}",
                variant.price.amount_cents / 100,
                variant.price.amount_cents % 100
            ),
            currency: variant.price.currency.clone(),
            image_id: variant.image_id.clone().unwrap_or_default(),
            stock,
        }
    }
}

fn has_variants_of(product: &ProductState) -> bool {
    !product.variants.is_empty()
}

/// The categories a product is in, live ones first: read from their own
/// streams so the page shows a change the moment it is made.
async fn memberships_of(
    access: &TenantAccess,
    product: &ProductState,
) -> Result<Vec<CategoryState>> {
    let gateway = catalog(access)?;
    let mut memberships: Vec<CategoryState> = Vec::new();
    for category_id in &product.categories {
        if let Some(category) = gateway
            .load_category(category_id)
            .await
            .map_err(topcoat::Error::from_anyhow)?
        {
            memberships.push(category);
        }
    }
    memberships
        .sort_by(|a, b| (a.archived, a.position, &a.name).cmp(&(b.archived, b.position, &b.name)));
    Ok(memberships)
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PRODUCTS_READ)
        .await?;
    let can_manage = access.can(cx, PERMISSION_PRODUCTS_MANAGE).await?;
    let query = query_params::<ProductQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let product = require_product(cx, &access).await?;
    let labels = user_labels(cx, std::slice::from_ref(&product.created_by)).await?;
    let created_by = labels
        .get(&product.created_by)
        .cloned()
        .unwrap_or_else(|| product.created_by.clone());
    let pool = app_context::<ReadPool>(cx);
    // Which categories it is in comes from the product's own stream, like
    // the rest of this page, so a change shows at once; each one is then
    // read from its own stream too (a product is in a handful at most).
    let memberships = memberships_of(&access, &product).await?;
    // Each picture with the address a browser loads it from.
    let media = app_context::<Arc<Media>>(cx);
    let gallery: Vec<(timada_catalog::ProductImage, String)> = product
        .images
        .iter()
        .map(|image| (image.clone(), media.url(&access.tenant.id, &image.file())))
        .collect();
    // The rating counts published reviews only; the link is for those who
    // may read the reviews.
    let can_read_reviews = access.can(cx, PERMISSION_REVIEWS_READ).await?;
    let rating = timada_reviews::product_rating(pool, &access.tenant.id, &product.id)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let rating_label = match rating.average() {
        Some(average) => format!("★ {average:.1} · {} published", rating.review_count),
        None => "No published review".to_owned(),
    };
    let editable = can_manage && !product.is_archived();
    // The AliExpress item it was imported from, read from its own stream.
    let can_aliexpress = access
        .can(cx, timada_tenancy::PERMISSION_ALIEXPRESS_MANAGE)
        .await?;
    let link = match timada_aliexpress::link_of_product(pool, &access.tenant.id, &product.id)
        .await
        .map_err(|err| topcoat::Error::from_anyhow(err.into()))?
    {
        Some(row) => {
            timada_aliexpress::Command::new(access.executor()?.clone(), access.tenant_id())
                .load_link_by_id(&row.id)
                .await
                .map_err(topcoat::Error::from_anyhow)?
                .filter(|link| link.is_linked())
        }
        None => None,
    };
    let link_costs: Vec<(String, String, u32)> = link
        .as_ref()
        .map(|link| {
            link.skus
                .iter()
                .map(|sku| {
                    let label = sku
                        .variant_id
                        .as_deref()
                        .and_then(|id| product.variant(id))
                        .map(|variant| {
                            VariantRow::of(variant, &product, StockView::default()).selections
                        })
                        .unwrap_or_else(|| product.name.clone());
                    let cost =
                        format_money(sku.cost_cents, product.currency().unwrap_or(CURRENCIES[0]));
                    (label, cost, sku.available)
                })
                .collect()
        })
        .unwrap_or_default();
    // What a read found that the shop does not sell: (sku id, label, cost,
    // available, why it cannot be added).
    let found_row = |sku: &timada_aliexpress::aggregator::NewSku| {
        let label = sku
            .selections
            .iter()
            .map(|(option, value)| format!("{option}: {value}"))
            .collect::<Vec<_>>()
            .join(" · ");
        let cost = format_money(sku.cost_cents, product.currency().unwrap_or(CURRENCIES[0]));
        let reason = timada_aliexpress::fit(&product, sku).reason();
        (sku.sku_id.clone(), label, cost, sku.available, reason)
    };
    let link_found: Vec<(String, String, String, u32, Option<String>)> = link
        .as_ref()
        .map(|link| link.offered().map(found_row).collect())
        .unwrap_or_default();
    let link_ignored: Vec<(String, String, String, u32, Option<String>)> = link
        .as_ref()
        .map(|link| link.ignored_found().map(found_row).collect())
        .unwrap_or_default();
    let found_fitting = link_found
        .iter()
        .filter(|(.., reason)| reason.is_none())
        .count();
    let link_pricing = link.as_ref().and_then(|link| link.pricing.as_ref());
    let link_rule_fields =
        crate::app::t::slug::aliexpress::RuleFields::of(link_pricing.map(|pricing| &pricing.above));
    let link_rule_label = link_pricing
        .map(|pricing| {
            crate::app::t::slug::aliexpress::pricing_label(
                Some(pricing),
                product.currency().unwrap_or(CURRENCIES[0]),
            )
        })
        .unwrap_or_else(|| "The shop's".to_owned());
    let gallery_full = gallery.len() >= MAX_IMAGES_PER_PRODUCT;
    let last = gallery.len().saturating_sub(1);
    // The view takes what it renders, so the menus and rows are built as
    // owned values first.
    let options: Vec<OptionRow> = product.options.iter().map(OptionRow::of).collect();
    let stocks = stocks_of(&access, &product).await?;
    let variants: Vec<VariantRow> = product
        .variants
        .iter()
        .zip(stocks.iter().cloned())
        .map(|(variant, stock)| VariantRow::of(variant, &product, stock))
        .collect();
    // A product without variants is counted on its own.
    let own_stock = (!has_variants_of(&product)).then(|| stocks[0].clone());
    let own_stock_label = own_stock.as_ref().map(StockView::label).unwrap_or_default();
    let own_stock_data = own_stock
        .as_ref()
        .map(|stock| stock.data().to_owned())
        .unwrap_or_default();
    let picture_choices: Vec<(String, String)> = product
        .images
        .iter()
        .map(|image| {
            let label = if image.alt.is_empty() {
                image.file()
            } else {
                image.alt.clone()
            };
            (image.id.clone(), label)
        })
        .collect();
    let has_variants = !product.variants.is_empty();
    let options_locked = editable && has_variants;
    let can_add_option = editable && !has_variants && options.len() < MAX_OPTIONS;
    let can_add_variant = editable && !options.is_empty() && product.variants.len() < MAX_VARIANTS;
    let attributes = product.attributes.clone();
    let price_currency = product.currency().unwrap_or(CURRENCIES[0]).to_owned();
    let price_label = product
        .price_range()
        .map(|(min, max, currency)| {
            if min == max {
                format_money(min, currency)
            } else {
                timada_core::format_money_range(min, max, currency)
            }
        })
        .unwrap_or_else(|| "Not priced yet".to_owned());
    // What it may still join is the whole tree, which the read model holds.
    let joinable: Vec<Choice> = rows_of(pool, &access.tenant.id)
        .await?
        .iter()
        .filter(|row| !product.categories.contains(&row.category.id))
        .map(Choice::of)
        .collect();
    let TenantAccess { tenant, .. } = access;
    // The kind of goods only matters to a merchant who charges VAT.
    let charges_vat = crate::app::tenants::tenancy(cx)
        .load_company_of(&tenant)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .and_then(|company| company.identity)
        .is_some_and(|legal| legal.is_vat_registered());
    let tax_label = if charges_vat {
        tax_category_label(product.tax_category()).to_owned()
    } else {
        "Not charged".to_owned()
    };
    let slug = tenant.slug.clone();
    let current_currency = product
        .price
        .as_ref()
        .map(|p| p.currency.clone())
        .unwrap_or_else(|| CURRENCIES[0].to_owned());
    let current_amount = product
        .price
        .as_ref()
        .map(|p| timada_core::format_amount(p.amount_cents))
        .unwrap_or_default();
    let title = format!("{} · Products · {}", product.name, tenant.name);
    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "products"),
            page_header(
                title: &product.name,
                h1_attrs: attributes! { data-product-id=(&product.id) },
                status_badge(
                    tone: status_tone(product.status),
                    attrs: attributes! { data-product-status=(product.status.as_str()) },
                    (status_label(product.status))
                )
                if editable {
                    if product.is_published() {
                        <form method="post" action=(href!(unpublish::unpublish, Slug(slug.clone()), ProductId(product.id.clone())))>
                            button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Unpublish")
                        </form>
                    } else {
                        <form method="post" action=(href!(publish::publish, Slug(slug.clone()), ProductId(product.id.clone())))>
                            button(size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Publish")
                        </form>
                    }
                    <form method="post" action=(href!(archive::archive, Slug(slug.clone()), ProductId(product.id.clone())))>
                        button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Archive")
                    </form>
                }
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                title: Some("Overview"),
                facts(
                    fact(term: "Slug", <code>(&product.slug)</code>)
                    fact(term: "Price", attrs: attributes! { data-product-price="" data-product-price-range=(&price_label) }, (&price_label))
                    fact(term: "VAT", attrs: attributes! { data-product-vat="" }, (&tax_label))
                    if let Some(stock) = &own_stock {
                        fact(
                            term: "Stock",
                            attrs: attributes! { data-stock=(&own_stock_data) data-stock-held=(stock.held) },
                            (&own_stock_label)
                        )
                    }
                    fact(
                        term: "Reviews",
                        attrs: attributes! { data-product-rating=(rating.review_count) },
                        (&rating_label)
                        if can_read_reviews {
                            " · " <a class=(LINK) href=(crate::app::t::slug::reviews::product_reviews_url(cx, &slug, &product.id))>"See its reviews"</a>
                        }
                    )
                    fact(
                        term: "Description",
                        attrs: attributes! { data-product-description="" class="whitespace-pre-line" },
                        (if product.description.is_empty() { "—" } else { product.description.as_str() })
                    )
                    fact(term: "Created", (&created_by) ", " <time datetime=(product.created_at)>(format_time(product.created_at))</time>)
                    fact(term: "Changed", <time datetime=(product.updated_at)>(format_time(product.updated_at))</time>)
                )
            )

            if let Some(stock) = &own_stock {
                if editable {
                    section_card(
                        title: Some("Stock"),
                        description: Some("How many are on the shelf. A product whose stock is not counted sells without limit; one counted at 0 is sold out."),
                        <form method="post" action=(href!(stock::stock, Slug(slug.clone()), ProductId(product.id.clone()))) class="flex flex-wrap items-end gap-2">
                            field(
                                field_label(attrs: attributes! { for="product-stock" }, "On the shelf")
                                input(attrs: attributes! {
                                    id="product-stock" name="on_hand" type="number" min="0" max=(timada_catalog::MAX_STOCK) step="1"
                                    class="w-32" value=(&stock.on_hand)
                                })
                            )
                            button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Set stock")
                            if stock.tracked {
                                button(variant: ButtonVariant::Ghost, size: ButtonSize::Sm, attrs: attributes! { type="submit" name="untrack" value="1" }, "Stop counting")
                            }
                        </form>
                    )
                }
            }

            section_card(
                title: Some("Pictures"),
                if gallery.is_empty() {
                    <p class="text-sm text-muted-foreground">"No picture yet. The first one is what a listing shows."</p>
                }
                <ul class="grid gap-4 sm:grid-cols-2">
                    for (position, (image, url)) in gallery.iter().enumerate() {
                        <li class="flex gap-3 rounded-lg border border-border p-3" data-image=(&image.id)>
                            <img class="h-24 w-24 shrink-0 rounded-md object-cover" src=(url) alt=(&image.alt)
                                width=(image.width.unwrap_or(96)) height=(image.height.unwrap_or(96)) loading="lazy">
                            <div class="min-w-0 flex-1 text-sm">
                                if position == 0 {
                                    <p class="mb-1 text-xs text-muted-foreground" data-image-first="">"Shown in listings"</p>
                                }
                                if editable {
                                    <form method="post" action=(href!(images::image_id::alt::alt, Slug(slug.clone()), ProductId(product.id.clone()), images::image_id::ImageId(image.id.clone())))>
                                        <label class="sr-only" for=(format!("alt-{}", image.id))>"What it shows"</label>
                                        input(attrs: attributes! {
                                            id=(format!("alt-{}", image.id)) name="alt" type="text" maxlength=(MAX_ALT_LEN)
                                            class="h-8 px-2" placeholder="What it shows" value=(&image.alt)
                                        })
                                        button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="mt-2 h-7 px-2 text-xs" }, "Save")
                                    </form>
                                    <div class="mt-2 flex flex-wrap items-center gap-2">
                                        if position > 0 {
                                            <form method="post" action=(href!(images::image_id::up::up, Slug(slug.clone()), ProductId(product.id.clone()), images::image_id::ImageId(image.id.clone())))>
                                                button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="h-7 px-2 text-xs" aria-label="Move earlier" }, "↑")
                                            </form>
                                        }
                                        if position < last {
                                            <form method="post" action=(href!(images::image_id::down::down, Slug(slug.clone()), ProductId(product.id.clone()), images::image_id::ImageId(image.id.clone())))>
                                                button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="h-7 px-2 text-xs" aria-label="Move later" }, "↓")
                                            </form>
                                        }
                                        <form method="post" action=(href!(images::image_id::remove::remove, Slug(slug.clone()), ProductId(product.id.clone()), images::image_id::ImageId(image.id.clone())))>
                                            button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="h-7 px-2 text-xs" }, "Remove")
                                        </form>
                                    </div>
                                } else {
                                    <p class="truncate text-muted-foreground">(if image.alt.is_empty() { "—" } else { image.alt.as_str() })</p>
                                }
                            </div>
                        </li>
                    }
                </ul>
                if editable && !gallery_full {
                    <form method="post" enctype="multipart/form-data" class="mt-4 flex max-w-xl flex-col gap-4"
                        action=(href!(images::upload::upload, Slug(slug.clone()), ProductId(product.id.clone())))>
                        field(
                            field_label(attrs: attributes! { for="product-image" }, "Add a picture")
                            input(attrs: attributes! {
                                id="product-image" name="image" type="file" accept="image/jpeg,image/png,image/webp,image/gif" required=""
                                class="pt-1.5"
                            })
                        )
                        field(
                            field_label(attrs: attributes! { for="product-image-alt" }, "What it shows (optional)")
                            input(attrs: attributes! {
                                id="product-image-alt" name="alt" type="text" maxlength=(MAX_ALT_LEN) placeholder="A blue mug on a desk"
                            })
                        )
                        <div>
                            button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Upload")
                        </div>
                    </form>
                } else if editable {
                    <p class="mt-4 text-sm text-muted-foreground">"This product carries as many pictures as it can. Remove one to add another."</p>
                }
            )

            section_card(
                title: Some("Categories"),
                if memberships.is_empty() {
                    <p class="text-sm text-muted-foreground">"In no category yet. A shopper finds a product by browsing the categories it is in."</p>
                }
                <ul class="divide-y divide-border text-sm">
                    for membership in &memberships {
                        <li class="flex items-center justify-between gap-3 py-2" data-membership=(&membership.slug)>
                            <span class="flex items-center gap-2">
                                if membership.archived {
                                    (&membership.name)
                                    badge(variant: BadgeVariant::Secondary, "archived")
                                } else {
                                    <a class=(LINK) href=(href!(crate::app::t::slug::categories::category_id::index, Slug(slug.clone()), crate::app::t::slug::categories::category_id::CategoryId(membership.id.clone())))>(&membership.name)</a>
                                }
                            </span>
                            if editable {
                                <form method="post" action=(href!(uncategorize::uncategorize, Slug(slug.clone()), ProductId(product.id.clone())))>
                                    <input type="hidden" name="category_id" value=(&membership.id)>
                                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="h-7 px-2 text-xs" }, "Remove")
                                </form>
                            }
                        </li>
                    }
                </ul>
                if editable && !joinable.is_empty() {
                    <form method="post" action=(href!(categorize::categorize, Slug(slug.clone()), ProductId(product.id.clone()))) class="mt-3 flex flex-wrap items-center gap-2">
                        <label class="sr-only" for="product-category">"Category"</label>
                        select(
                            attrs: attributes! { id="product-category" name="category_id" required="" class="min-w-0 flex-1 sm:max-w-sm" },
                            for choice in &joinable {
                                <option value=(&choice.id)>(&choice.label)</option>
                            }
                        )
                        button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Add to category")
                    </form>
                }
            )

            section_card(
                title: Some("Options"),
                description: Some("A product that comes in several colours or sizes varies by them. Settle the options first: once it sells variants they cannot change."),
                if options.is_empty() {
                    <p class="text-sm text-muted-foreground">"This product does not vary. It sells at the price above."</p>
                }
                <ul class="flex flex-col gap-3">
                    for option in &options {
                        <li class="rounded-lg border border-border p-3" data-option=(&option.id)>
                            if editable && !options_locked {
                                <form method="post" action=(href!(options::option_id::values::values, Slug(slug.clone()), ProductId(product.id.clone()), options::option_id::OptionId(option.id.clone()))) class="flex flex-col gap-3">
                                    field(
                                        field_label(attrs: attributes! { for=(format!("option-name-{}", option.id)) }, "Name of " (&option.name))
                                        input(attrs: attributes! {
                                            id=(format!("option-name-{}", option.id)) name="name" type="text" maxlength=(MAX_OPTION_NAME_LEN) required=""
                                            value=(&option.name)
                                        })
                                    )
                                    field(
                                        field_label(attrs: attributes! { for=(format!("option-values-{}", option.id)) }, "Values of " (&option.name) ", one a line")
                                        textarea(attrs: attributes! { id=(format!("option-values-{}", option.id)) name="values" rows="3" }, (&option.values))
                                    )
                                    <div class="flex flex-wrap items-center gap-2">
                                        button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Save option")
                                    </div>
                                </form>
                                <form method="post" action=(href!(options::option_id::remove::remove, Slug(slug.clone()), ProductId(product.id.clone()), options::option_id::OptionId(option.id.clone()))) class="mt-2">
                                    button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="h-7 px-2 text-xs" }, "Remove option")
                                </form>
                            } else {
                                <p class="text-sm"><strong>(&option.name)</strong></p>
                                <p class="whitespace-pre-line text-sm text-muted-foreground">(&option.values)</p>
                            }
                        </li>
                    }
                </ul>
                if options_locked {
                    <p class="mt-3 text-sm text-muted-foreground">"Remove every variant before changing the options."</p>
                }
                if can_add_option {
                    <form method="post" action=(href!(options::define::define, Slug(slug.clone()), ProductId(product.id.clone()))) class="mt-4 flex max-w-xl flex-col gap-4">
                        field(
                            field_label(attrs: attributes! { for="new-option-name" }, "Add an option")
                            input(attrs: attributes! {
                                id="new-option-name" name="name" type="text" maxlength=(MAX_OPTION_NAME_LEN) required="" placeholder="Colour"
                            })
                        )
                        field(
                            field_label(attrs: attributes! { for="new-option-values" }, "Values, one a line")
                            textarea(attrs: attributes! { id="new-option-values" name="values" rows="3" required="" placeholder="Beige&#10;Blue&#10;Grey" })
                        )
                        <div>
                            button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Add option")
                        </div>
                    </form>
                }
            )

            if !options.is_empty() {
                section_card(
                    title: Some("Variants"),
                    description: Some("One combination a shopper buys, with its own price. A product with variants is published without a price of its own."),
                    table(
                        attrs: attributes! { data-variants="" },
                        table_header(
                            table_row(
                                table_head("Combination")
                                table_head("Price")
                                table_head("Code")
                                table_head("Stock")
                                table_head("Picture")
                                table_head(<span class="sr-only">"Actions"</span>)
                            )
                        )
                        table_body(
                            if variants.is_empty() {
                                empty_row(colspan: 6, text: "No variant yet.")
                            }
                            for variant in &variants {
                                table_row(
                                    attrs: attributes! { data-variant=(&variant.id) },
                                    table_cell(attrs: attributes! { data-variant-selections=(&variant.selections) }, (&variant.selections))
                                    table_cell(
                                        if editable {
                                            <form method="post" action=(href!(variants::variant_id::price::price, Slug(slug.clone()), ProductId(product.id.clone()), variants::variant_id::VariantId(variant.id.clone()))) class="flex items-center gap-1">
                                                <label class="sr-only" for=(format!("variant-amount-{}", variant.id))>"Price of " (&variant.selections)</label>
                                                input(attrs: attributes! {
                                                    id=(format!("variant-amount-{}", variant.id)) name="amount" type="text" inputmode="decimal" required=""
                                                    class="h-8 w-20 px-2" value=(&variant.amount)
                                                })
                                                <label class="sr-only" for=(format!("variant-currency-{}", variant.id))>"Currency of " (&variant.selections)</label>
                                                select(
                                                    attrs: attributes! { id=(format!("variant-currency-{}", variant.id)) name="currency" class="w-24" },
                                                    for currency in CURRENCIES {
                                                        <option value=(*currency) selected=(variant.currency == *currency)>(*currency)</option>
                                                    }
                                                )
                                                button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="h-7 px-2 text-xs" }, "Set")
                                            </form>
                                        } else {
                                            (&variant.amount) " " (&variant.currency)
                                        }
                                    )
                                    table_cell(
                                        if editable {
                                            <form method="post" action=(href!(variants::variant_id::sku::sku, Slug(slug.clone()), ProductId(product.id.clone()), variants::variant_id::VariantId(variant.id.clone()))) class="flex items-center gap-1">
                                                <label class="sr-only" for=(format!("variant-sku-{}", variant.id))>"Code of " (&variant.selections)</label>
                                                input(attrs: attributes! {
                                                    id=(format!("variant-sku-{}", variant.id)) name="sku" type="text" maxlength=(MAX_SKU_LEN)
                                                    class="h-8 w-28 px-2" value=(&variant.sku)
                                                })
                                                button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="h-7 px-2 text-xs" }, "Save")
                                            </form>
                                        } else if variant.sku.is_empty() { "—" } else { <code>(&variant.sku)</code> }
                                    )
                                    table_cell(
                                        attrs: attributes! { data-stock=(variant.stock.data()) data-stock-held=(variant.stock.held) },
                                        if editable {
                                            <form method="post" action=(href!(stock::stock, Slug(slug.clone()), ProductId(product.id.clone()))) class="flex items-center gap-1">
                                                <input type="hidden" name="variant_id" value=(&variant.id)>
                                                <label class="sr-only" for=(format!("variant-stock-{}", variant.id))>"Stock of " (&variant.selections)</label>
                                                input(attrs: attributes! {
                                                    id=(format!("variant-stock-{}", variant.id)) name="on_hand" type="number" min="0" max=(timada_catalog::MAX_STOCK) step="1"
                                                    class="h-8 w-20 px-2" value=(&variant.stock.on_hand) placeholder="∞"
                                                })
                                                button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="h-7 px-2 text-xs" aria-label=(format!("Set the stock of {}", variant.selections)) }, "Set")
                                                if variant.stock.tracked {
                                                    button(variant: ButtonVariant::Ghost, size: ButtonSize::Sm, attrs: attributes! { type="submit" name="untrack" value="1" class="h-7 px-2 text-xs" aria-label=(format!("Stop counting {}", variant.selections)) }, "∞")
                                                }
                                            </form>
                                            if variant.stock.held > 0 {
                                                <p class="mt-1 text-xs text-muted-foreground">(variant.stock.held) " held"</p>
                                            }
                                        } else {
                                            (variant.stock.label())
                                        }
                                    )
                                    table_cell(
                                        if editable && !picture_choices.is_empty() {
                                            <form method="post" action=(href!(variants::variant_id::image::image, Slug(slug.clone()), ProductId(product.id.clone()), variants::variant_id::VariantId(variant.id.clone()))) class="flex items-center gap-1">
                                                <label class="sr-only" for=(format!("variant-image-{}", variant.id))>"Picture of " (&variant.selections)</label>
                                                select(
                                                    attrs: attributes! { id=(format!("variant-image-{}", variant.id)) name="image_id" class="w-36" },
                                                    <option value="" selected=(variant.image_id.is_empty())>"None"</option>
                                                    for (id, label) in &picture_choices {
                                                        <option value=(id) selected=(&variant.image_id == id)>(label)</option>
                                                    }
                                                )
                                                button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="h-7 px-2 text-xs" }, "Set")
                                            </form>
                                        } else { "—" }
                                    )
                                    table_cell(
                                        attrs: attributes! { class="text-right" },
                                        if editable {
                                            <form method="post" action=(href!(variants::variant_id::remove::remove, Slug(slug.clone()), ProductId(product.id.clone()), variants::variant_id::VariantId(variant.id.clone())))>
                                                button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="h-7 px-2 text-xs" }, "Remove")
                                            </form>
                                        }
                                    )
                                )
                            }
                        )
                    )
                    if can_add_variant {
                        <form method="post" action=(href!(variants::define::define, Slug(slug.clone()), ProductId(product.id.clone()))) class="mt-4 flex max-w-xl flex-col gap-4">
                            <h3 class="text-sm font-semibold">"Add a variant"</h3>
                            for option in &options {
                                field(
                                    field_label(attrs: attributes! { for=(format!("pick-{}", option.id)) }, (&option.name))
                                    select(
                                        attrs: attributes! { id=(format!("pick-{}", option.id)) name=(format!("option:{}", option.id)) required="" },
                                        for value in option.values.lines() {
                                            <option value=(value)>(value)</option>
                                        }
                                    )
                                )
                            }
                            <div class="flex flex-wrap gap-4">
                                field(
                                    attrs: attributes! { class="min-w-0 flex-1" },
                                    field_label(attrs: attributes! { for="variant-amount" }, "Variant price")
                                    input(attrs: attributes! {
                                        id="variant-amount" name="amount" type="text" inputmode="decimal" required="" placeholder="22.90"
                                    })
                                )
                                field(
                                    field_label(attrs: attributes! { for="variant-currency" }, "Variant currency")
                                    select(
                                        attrs: attributes! { id="variant-currency" name="currency" class="w-28" },
                                        for currency in CURRENCIES {
                                            <option value=(*currency) selected=(price_currency == *currency)>(*currency)</option>
                                        }
                                    )
                                )
                            </div>
                            field(
                                field_label(attrs: attributes! { for="variant-sku" }, "Variant code (optional)")
                                input(attrs: attributes! {
                                    id="variant-sku" name="sku" type="text" maxlength=(MAX_SKU_LEN) placeholder="harness-beige-s"
                                })
                            )
                            if !picture_choices.is_empty() {
                                field(
                                    field_label(attrs: attributes! { for="variant-picture" }, "Variant picture (optional)")
                                    select(
                                        attrs: attributes! { id="variant-picture" name="image_id" },
                                        <option value="">"None"</option>
                                        for (id, label) in &picture_choices {
                                            <option value=(id)>(label)</option>
                                        }
                                    )
                                )
                            }
                            <div>
                                button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Add variant")
                            </div>
                        </form>
                    }
                )
            }

            if let Some(link) = &link {
                section_card(
                    title: Some("AliExpress"),
                    description: Some("Imported from AliExpress: its stock, and its price when a rule prices it, are read again from there."),
                    <div data-product-aliexpress=(&link.ae_product_id) data-aliexpress-offline=(link.offline)>
                        facts(
                            fact(term: "Item", <a class=(LINK) href=(format!("https://www.aliexpress.com/item/{}.html", link.ae_product_id)) rel="noopener noreferrer" target="_blank">(&link.ae_product_id)</a>)
                            fact(term: "Last read",
                                if let Some(at) = link.last_synced_at {
                                    <time datetime=(at)>(format_time(at))</time>
                                } else { "—" }
                            )
                            if link.offline {
                                fact(term: "On AliExpress", status_badge(tone: crate::ui::Tone::Warning, "No longer sold"))
                            }
                            if let Some(error) = &link.last_error {
                                fact(term: "Last problem", (error))
                            }
                            fact(term: "Price rule", attrs: attributes! { data-product-aliexpress-rule="" }, (&link_rule_label))
                        )
                        <ul class="mt-3 text-sm" data-aliexpress-costs="">
                            for (label, cost, available) in &link_costs {
                                <li>(label) " · costs " (cost) " · " (available) " on AliExpress"</li>
                            }
                        </ul>
                    </div>
                    if !link_found.is_empty() || !link_ignored.is_empty() {
                        <div class="mt-4" data-aliexpress-found="">
                            <h3 class="text-sm font-medium">"New on AliExpress"</h3>
                            <p class="text-xs text-muted-foreground">"Combinations AliExpress sells that this product does not. Added, each is a variant priced by the rule and counted from AliExpress."</p>
                            if link_found.is_empty() {
                                <p class="mt-2 text-sm text-muted-foreground">"Nothing new to add."</p>
                            }
                            <ul class="mt-2 space-y-2 text-sm">
                                for (sku_id, label, cost, available, reason) in &link_found {
                                    <li class="flex flex-wrap items-center gap-2" data-aliexpress-new=(sku_id)>
                                        <span>(label) " · costs " (cost) " · " (available) " on AliExpress"</span>
                                        if let Some(reason) = reason {
                                            status_badge(tone: crate::ui::Tone::Warning, "Can't be added")
                                            <span class="text-xs text-muted-foreground" data-aliexpress-new-reason="">(reason)</span>
                                        } else if can_aliexpress {
                                            <form method="post" action=(href!(crate::app::t::slug::aliexpress::products::link_id::combinations::combinations, Slug(slug.clone()), crate::app::t::slug::aliexpress::products::link_id::LinkId(link.id.clone())))>
                                                <input type="hidden" name="back" value=(&product.id)>
                                                <input type="hidden" name="sku_id" value=(sku_id)>
                                                button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" aria-label=(format!("Add {label} as a variant")) }, "Add as variant")
                                            </form>
                                        }
                                        if can_aliexpress {
                                            <form method="post" action=(href!(crate::app::t::slug::aliexpress::products::link_id::ignore::ignore, Slug(slug.clone()), crate::app::t::slug::aliexpress::products::link_id::LinkId(link.id.clone())))>
                                                <input type="hidden" name="back" value=(&product.id)>
                                                <input type="hidden" name="sku_id" value=(sku_id)>
                                                button(variant: ButtonVariant::Ghost, size: ButtonSize::Sm, attrs: attributes! { type="submit" aria-label=(format!("Ignore {label}")) }, "Ignore")
                                            </form>
                                        }
                                    </li>
                                }
                            </ul>
                            if can_aliexpress && found_fitting > 1 {
                                <form class="mt-2" method="post" action=(href!(crate::app::t::slug::aliexpress::products::link_id::combinations::combinations, Slug(slug.clone()), crate::app::t::slug::aliexpress::products::link_id::LinkId(link.id.clone())))>
                                    <input type="hidden" name="back" value=(&product.id)>
                                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, (format!("Add all {found_fitting}")))
                                </form>
                            }
                            if !link_ignored.is_empty() {
                                <details class="mt-3" data-aliexpress-ignored="">
                                    <summary class="cursor-pointer text-xs text-muted-foreground">(format!("Ignored ({})", link_ignored.len()))</summary>
                                    <ul class="mt-2 space-y-2 text-sm">
                                        for (sku_id, label, cost, available, _) in &link_ignored {
                                            <li class="flex flex-wrap items-center gap-2" data-aliexpress-ignored-sku=(sku_id)>
                                                <span>(label) " · costs " (cost) " · " (available) " on AliExpress"</span>
                                                if can_aliexpress {
                                                    <form method="post" action=(href!(crate::app::t::slug::aliexpress::products::link_id::restore::restore, Slug(slug.clone()), crate::app::t::slug::aliexpress::products::link_id::LinkId(link.id.clone())))>
                                                        <input type="hidden" name="back" value=(&product.id)>
                                                        <input type="hidden" name="sku_id" value=(sku_id)>
                                                        button(variant: ButtonVariant::Ghost, size: ButtonSize::Sm, attrs: attributes! { type="submit" aria-label=(format!("Offer {label} again")) }, "Offer again")
                                                    </form>
                                                }
                                            </li>
                                        }
                                    </ul>
                                </details>
                            }
                        </div>
                    }
                    if can_aliexpress {
                        <div class="mt-4 flex flex-wrap gap-2">
                            <form method="post" action=(href!(crate::app::t::slug::aliexpress::products::link_id::sync::sync, Slug(slug.clone()), crate::app::t::slug::aliexpress::products::link_id::LinkId(link.id.clone())))>
                                <input type="hidden" name="back" value=(&product.id)>
                                button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Sync now")
                            </form>
                            <form method="post" action=(href!(crate::app::t::slug::aliexpress::products::link_id::unlink::unlink, Slug(slug.clone()), crate::app::t::slug::aliexpress::products::link_id::LinkId(link.id.clone())))>
                                <input type="hidden" name="back" value=(&product.id)>
                                button(variant: ButtonVariant::Ghost, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Stop keeping in step")
                            </form>
                        </div>
                        <form method="post" action=(href!(crate::app::t::slug::aliexpress::products::link_id::pricing::pricing, Slug(slug.clone()), crate::app::t::slug::aliexpress::products::link_id::LinkId(link.id.clone()))) class="mt-4 flex flex-wrap items-end gap-2">
                            <input type="hidden" name="back" value=(&product.id)>
                            field(
                                field_label(attrs: attributes! { for="link-multiplier" }, "Its own rule: multiply the cost by")
                                input(attrs: attributes! { id="link-multiplier" name="multiplier" inputmode="decimal" placeholder="The shop's" class="w-28" value=(&link_rule_fields.multiplier) })
                            )
                            field(
                                field_label(attrs: attributes! { for="link-plus" }, "Then add")
                                input(attrs: attributes! { id="link-plus" name="plus" inputmode="decimal" class="w-24" value=(&link_rule_fields.plus) })
                            )
                            field(
                                field_label(attrs: attributes! { for="link-rounding" }, "Round")
                                select(
                                    attrs: attributes! { id="link-rounding" name="rounding" class="w-44" },
                                    for (value, label) in crate::app::t::slug::aliexpress::ROUNDINGS {
                                        <option value=(value) selected=(link_rule_fields.rounding == value)>(label)</option>
                                    }
                                )
                            )
                            button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Save rule")
                        </form>
                    }
                )
            }

            section_card(
                title: Some("Facts"),
                description: Some("What a shopper reads beside the description: what it is made of, how big it is."),
                table(
                    attrs: attributes! { data-attributes="" },
                    table_body(
                        if attributes.is_empty() {
                            empty_row(colspan: 3, text: "No fact yet.")
                        }
                        for attribute in &attributes {
                            table_row(
                                attrs: attributes! { data-attribute=(&attribute.name) },
                                <th class="p-3 pr-4 text-left align-middle font-medium" scope="row">(&attribute.name)</th>
                                table_cell((&attribute.value))
                                table_cell(
                                    attrs: attributes! { class="text-right" },
                                    if editable {
                                        <form method="post" action=(href!(attributes::remove::remove, Slug(slug.clone()), ProductId(product.id.clone())))>
                                            <input type="hidden" name="name" value=(&attribute.name)>
                                            button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="h-7 px-2 text-xs" }, "Remove")
                                        </form>
                                    }
                                )
                            )
                        }
                    )
                )
                if editable {
                    <form method="post" action=(href!(attributes::set::set, Slug(slug.clone()), ProductId(product.id.clone()))) class="mt-4 flex flex-wrap items-end gap-2">
                        field(
                            attrs: attributes! { class="min-w-0 flex-1" },
                            field_label(attrs: attributes! { for="attribute-name" }, "Fact name")
                            input(attrs: attributes! {
                                id="attribute-name" name="name" type="text" maxlength=(MAX_ATTRIBUTE_NAME_LEN) required="" placeholder="Material"
                            })
                        )
                        field(
                            attrs: attributes! { class="min-w-0 flex-1" },
                            field_label(attrs: attributes! { for="attribute-value" }, "Fact value")
                            input(attrs: attributes! {
                                id="attribute-value" name="value" type="text" maxlength=(MAX_ATTRIBUTE_VALUE_LEN) required="" placeholder="Nylon"
                            })
                        )
                        button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="h-9" }, "Set fact")
                    </form>
                }
            )

            if product.is_archived() {
                <p class="mb-6 text-sm text-muted-foreground">"An archived product cannot change any more."</p>
            }

            if editable {
                <section class="mb-6 grid gap-4 md:grid-cols-2">
                    <form method="post" action=(href!(rename::rename, Slug(slug.clone()), ProductId(product.id.clone()))) class=(class!(CARD, "gap-4 px-6"))>
                        field(
                            field_label(attrs: attributes! { for="product-name" }, "Name")
                            input(attrs: attributes! {
                                id="product-name" name="name" type="text" maxlength=(MAX_NAME_LEN) required="" value=(&product.name)
                            })
                        )
                        <div>
                            button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Rename")
                        </div>
                    </form>
                    <form method="post" action=(href!(price::price, Slug(slug.clone()), ProductId(product.id.clone()))) class=(class!(CARD, "gap-4 px-6"))>
                        field(
                            field_label(attrs: attributes! { for="product-amount" }, "Price")
                            <div class="flex gap-2">
                                input(attrs: attributes! {
                                    id="product-amount" name="amount" type="text" inputmode="decimal" required="" placeholder="12.50" value=(&current_amount)
                                })
                                <label class="sr-only" for="product-currency">"Currency"</label>
                                select(
                                    attrs: attributes! { id="product-currency" name="currency" class="w-28 shrink-0" },
                                    for currency in CURRENCIES {
                                        <option value=(*currency) selected=(current_currency == *currency)>(*currency)</option>
                                    }
                                )
                            </div>
                        )
                        <div>
                            button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Set price")
                        </div>
                    </form>
                    if charges_vat {
                        <form method="post" action=(href!(tax_category::tax_category, Slug(slug.clone()), ProductId(product.id.clone()))) class=(class!(CARD, "gap-4 px-6"))>
                            field(
                                field_label(attrs: attributes! { for="product-tax" }, "Kind of goods, for VAT")
                                select(
                                    attrs: attributes! { id="product-tax" name="category" },
                                    for (key, label) in TAX_CATEGORIES.iter().copied() {
                                        <option value=(key) selected=(product.tax_category() == key)>
                                            (label) " (" (format_rate_fr(rate_bps("FR", key))) " in France)"
                                        </option>
                                    }
                                )
                                <p class="text-xs text-muted-foreground">"Each country taxes a kind at its own rate; most goods are standard."</p>
                            )
                            <div>
                                button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Set kind")
                            </div>
                        </form>
                    }
                    <form method="post" action=(href!(describe::describe, Slug(slug.clone()), ProductId(product.id.clone()))) class=(class!(CARD, "gap-4 px-6 md:col-span-2"))>
                        field(
                            field_label(attrs: attributes! { for="product-description" }, "Description")
                            textarea(attrs: attributes! { id="product-description" name="description" rows="5" maxlength=(MAX_DESCRIPTION_LEN) }, (&product.description))
                        )
                        <div>
                            button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Save description")
                        </div>
                    </form>
                </section>
            }
        )
    })
}

#[derive(serde::Deserialize)]
pub struct RenameForm {
    pub name: String,
}

#[derive(serde::Deserialize)]
pub struct DescribeForm {
    #[serde(default)]
    pub description: String,
}

#[derive(serde::Deserialize)]
pub struct PriceForm {
    pub amount: String,
    pub currency: String,
}

#[derive(serde::Deserialize)]
pub struct TaxCategoryForm {
    /// A key of `TAX_CATEGORIES`.
    pub category: String,
}
