//! `/t/{slug}/products/{product_id}`: one product, read from its own stream
//! (so an edit shows at once), with the forms that change it.

pub mod ai_writing;
pub mod archive;
pub mod attributes;
pub mod categorize;
pub mod classify;
pub mod compare_at;
pub mod cost;
pub mod delete;
pub mod delivery_days;
pub mod describe;
pub mod feed_labels;
pub mod feed_listing;
pub mod identify;
pub mod images;
pub mod main_category;
pub mod margin;
pub mod options;
pub mod own_costs;
pub mod price;
pub mod publish;
pub mod rename;
pub mod seo;
pub mod stock;
pub mod tax_category;
pub mod translations;
pub mod uncategorize;
pub mod unpublish;
pub mod variants;

use std::sync::Arc;

use timada_catalog::{
    AGE_GROUPS, CURRENCIES, CategoryState, GENDERS, MAX_ALT_LEN, MAX_ATTRIBUTE_NAME_LEN,
    MAX_ATTRIBUTE_VALUE_LEN, MAX_BRAND_LEN, MAX_DELIVERY_DAYS, MAX_DESCRIPTION_LEN,
    MAX_FEED_LABEL_LEN, MAX_GOOGLE_CATEGORY_LEN, MAX_IMAGES_PER_PRODUCT, MAX_META_DESCRIPTION_LEN,
    MAX_META_TITLE_LEN, MAX_MPN_LEN, MAX_NAME_LEN, MAX_OPTION_LABEL_LEN, MAX_OPTION_NAME_LEN,
    MAX_OPTIONS, MAX_SKU_LEN, MAX_VARIANTS, OPTION_KINDS, ProductOption, ProductState,
    ProductVariant, TAX_CATEGORIES, TranslationKind, format_money, tax_category_label,
};
use timada_core::Rendition;
use timada_tenancy::{
    PERMISSION_PRODUCTS_MANAGE, PERMISSION_PRODUCTS_READ, PERMISSION_REVIEWS_READ,
};

use topcoat::{
    Result,
    context::{Cx, context},
    router::{
        error::{SeeOther, not_found, see_other},
        href, module_param, page, query_params,
    },
    view::{View, attributes, class, view},
};

use super::{catalog, error_code, status_label, status_tone};
use crate::app::auth::Flash;
use crate::app::t::slug::categories::{Choice, rows_of};
use crate::app::t::slug::{TenantAccess, TenantNav, require_tenant, slug};
use crate::auth::user_labels;
use crate::components::badge::{BadgeVariant, badge};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::card::CARD;
use crate::components::checkbox::checkbox;
use crate::components::field::{field, field_description, field_label};
use crate::components::input::input;
use crate::components::label::label;
use crate::components::select::select;
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::components::textarea::textarea;
use crate::db::Db;
use crate::media::Media;
use crate::ui::translations::{
    MainWording, WordCard, translation_cards, translation_cards_of, word_cards,
};
use crate::ui::{
    LINK, PageTab, Shell, document, empty_row, fact, facts, format_time, notice, page_header,
    page_tabs, section_card, status_badge,
};
use margin::{MarginView, margin_tab};
use timada_invoicing::format_rate_fr;
use timada_invoicing::tax::rate_bps;

module_param!(pub product_id);

#[topcoat::router::query_params(error = bad_request)]
struct ProductQuery {
    tab: Option<String>,
    /// The price the margin tab is asked about, as typed.
    what_if: Option<String>,
    error: Option<String>,
    message: Option<String>,
}

/// The part of the page shown, named in the URL so that an action comes back
/// to the tab it was sent from.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Overview,
    Pictures,
    Categories,
    Variants,
    Facts,
    Shopping,
    Margin,
    Translations,
    AliExpress,
}

impl Tab {
    /// What `?tab=` says; nothing for the tab the page opens on.
    pub(crate) fn key(self) -> Option<&'static str> {
        match self {
            Self::Overview => None,
            Self::Pictures => Some("pictures"),
            Self::Categories => Some("categories"),
            Self::Variants => Some("variants"),
            Self::Facts => Some("facts"),
            Self::Shopping => Some("shopping"),
            Self::Margin => Some("margin"),
            Self::Translations => Some("translations"),
            Self::AliExpress => Some("aliexpress"),
        }
    }

    fn of(key: Option<&str>) -> Self {
        [
            Self::Pictures,
            Self::Categories,
            Self::Variants,
            Self::Facts,
            Self::Shopping,
            Self::Margin,
            Self::Translations,
            Self::AliExpress,
        ]
        .into_iter()
        .find(|tab| tab.key() == key)
        .unwrap_or(Self::Overview)
    }
}

#[derive(serde::Serialize)]
struct TabQuery<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    tab: Option<&'a str>,
    #[serde(flatten)]
    flash: Flash<'a>,
}

pub fn product_url(
    cx: &Cx,
    slug: &str,
    product_id: &str,
    error: Option<&str>,
    message: Option<&str>,
) -> String {
    product_tab_url(cx, slug, product_id, Tab::Overview, error, message)
}

pub fn product_tab_url(
    cx: &Cx,
    slug: &str,
    product_id: &str,
    tab: Tab,
    error: Option<&str>,
    message: Option<&str>,
) -> String {
    href!(
        index,
        slug: slug.to_owned(),
        product_id: product_id.to_owned()
    )
    .query(TabQuery {
        tab: tab.key(),
        flash: Flash { error, message },
    })
    .resolve(cx)
}

/// The product named in the URL, or 404.
async fn require_product(cx: &Cx, access: &TenantAccess) -> Result<ProductState> {
    let id = product_id(cx);
    catalog(access)?
        .load_product(id)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .ok_or_else(|| not_found().into())
}

/// What every action on a product shares: the member may manage products,
/// the product exists, and the outcome goes back to `tab` of the product page.
pub(crate) async fn act<F>(cx: &Cx, tab: Tab, message: &'static str, run: F) -> Result<SeeOther>
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
    let product_id = product_id(cx).to_owned();
    let slug = access.tenant.slug.clone();
    let gateway = catalog(&access)?;
    match run(&gateway, &access.user_id, &product_id).await {
        Ok(()) => Ok(see_other(product_tab_url(
            cx,
            &slug,
            &product_id,
            tab,
            None,
            Some(message),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(product_tab_url(
            cx,
            &slug,
            &product_id,
            tab,
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
    /// What the shop shows instead of the name; empty for the name.
    pub label: String,
    /// Each value with what the shop shows instead of it; empty for the
    /// value.
    pub value_labels: Vec<(String, String)>,
    /// The value it is hidden from shoppers at; empty when they choose.
    pub fixed_value: String,
    /// The values it can be hidden at: something stays on sale at each.
    pub hide_at: Vec<String>,
    /// What it is to a shopping feed (`OPTION_KINDS`); empty for none.
    pub kind: String,
}

impl OptionRow {
    fn of(option: &ProductOption, product: &ProductState) -> Self {
        Self {
            id: option.id.clone(),
            name: option.name.clone(),
            values: option.values.join("\n"),
            label: option.label.clone().unwrap_or_default(),
            value_labels: option
                .values
                .iter()
                .map(|value| {
                    (
                        value.clone(),
                        option.value_labels.get(value).cloned().unwrap_or_default(),
                    )
                })
                .collect(),
            fixed_value: option.fixed_value.clone().unwrap_or_default(),
            hide_at: option
                .values
                .iter()
                .filter(|value| {
                    option.fixed_value.as_ref() == Some(*value)
                        || product.can_hide_at(&option.id, value)
                })
                .cloned()
                .collect(),
            kind: option.kind.clone().unwrap_or_default(),
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
    pub gtin: String,
    pub mpn: String,
    /// The price its reduction is announced against, as the form wants it
    /// typed; empty without one.
    pub compare_at: String,
    /// The most that may be: its lowest price of the thirty days before
    /// the current one, or empty when it had no other.
    pub compare_at_ceiling: String,
    pub stock: StockView,
    /// Whether shoppers can buy it: a hidden option may leave it off sale.
    pub on_sale: bool,
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

/// How many stock streams [`stocks_of`] reads at once.
const STOCK_READS: usize = 8;

/// The stock of each thing the product sells, read from their own streams
/// so a change shows at once: one entry per variant, or one for a product
/// without variants. Each is a pointer and a generation of up to a thousand
/// events, so they are read a few at a time rather than one after another.
async fn stocks_of(access: &TenantAccess, product: &ProductState) -> Result<Vec<StockView>> {
    let executor = access.executor()?.clone();
    let tenant = access.tenant_id().to_owned();
    let now = crate::auth::now();
    let ids: Vec<Option<String>> = if product.variants.is_empty() {
        vec![None]
    } else {
        product
            .variants
            .iter()
            .map(|v| Some(v.id.clone()))
            .collect()
    };
    let mut stocks = Vec::with_capacity(ids.len());
    for chunk in ids.chunks(STOCK_READS) {
        let mut reads = tokio::task::JoinSet::new();
        for (n, variant_id) in chunk.iter().cloned().enumerate() {
            let gateway = timada_catalog::Command::new(executor.clone(), tenant.clone());
            let product_id = product.id.clone();
            reads.spawn(async move {
                let stock = gateway.load_stock(&product_id, variant_id.as_deref()).await;
                (n, stock)
            });
        }
        let mut read = reads.join_all().await;
        read.sort_by_key(|(n, _)| *n);
        for (_, stock) in read {
            let stock = stock.map_err(topcoat::Error::from_anyhow)?;
            stocks.push(StockView::of(stock.as_ref(), now));
        }
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
            gtin: variant.gtin.clone().unwrap_or_default(),
            mpn: variant.mpn.clone().unwrap_or_default(),
            compare_at: product
                .compare_at(Some(&variant.id))
                .map(timada_core::format_amount)
                .unwrap_or_default(),
            compare_at_ceiling: product
                .compare_at_ceiling(Some(&variant.id))
                .map(timada_core::format_amount)
                .unwrap_or_default(),
            stock,
            on_sale: product.sells(variant),
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
    // A product's stream holds 65,535 changes: past that it can change no
    // more, so how far it is shows here (#472).
    let changes = evento::ProjectionAggregate::aggregate_version(&product)
        .map_err(topcoat::Error::from_anyhow)?;
    let changes_label = format!("{changes} of the {} a product holds", u16::MAX);
    let labels = user_labels(cx, std::slice::from_ref(&product.created_by)).await?;
    let created_by = labels
        .get(&product.created_by)
        .cloned()
        .unwrap_or_else(|| product.created_by.clone());
    let pool = context::<Db>(cx);
    // Which categories it is in comes from the product's own stream, like
    // the rest of this page, so a change shows at once; each one is then
    // read from its own stream too (a product is in a handful at most).
    let memberships = memberships_of(&access, &product).await?;
    // The live ones, among which the merchant picks the main category:
    // the one picked, else the first the product was filed under.
    let live_memberships: Vec<(String, String)> = memberships
        .iter()
        .filter(|category| !category.archived)
        .map(|category| (category.id.clone(), category.name.clone()))
        .collect();
    let main_category = product
        .filed_under()
        .into_iter()
        .find(|id| live_memberships.iter().any(|(live, _)| live == id))
        .unwrap_or_default()
        .to_owned();
    // Each picture with the address a browser loads it from.
    let media = context::<Arc<Media>>(cx);
    let gallery: Vec<(timada_catalog::ProductImage, String)> = product
        .images
        .iter()
        .map(|image| {
            (
                image.clone(),
                media.url(&access.tenant.id, &image.id, Rendition::Listing),
            )
        })
        .collect();
    // The rating counts published reviews only; the link is for those who
    // may read the reviews.
    let can_read_reviews = access.can(cx, PERMISSION_REVIEWS_READ).await?;
    let rating = timada_reviews::product_rating(context::<Db>(cx), &access.tenant.id, &product.id)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let rating_label = match rating.average() {
        Some(average) => format!("★ {average:.1} · {} published", rating.review_count),
        None => "No published review".to_owned(),
    };
    let editable = can_manage && !product.is_archived();
    // A product somebody ordered stays on record: it can be archived, not
    // deleted.
    let deletable = can_manage
        && !timada_orders::product_ever_ordered(pool, &access.tenant.id, &product.id)
            .await
            .map_err(topcoat::Error::from_anyhow)?;
    // The AliExpress item it was imported from, read from its own stream.
    let can_aliexpress = access
        .can(cx, timada_tenancy::PERMISSION_ALIEXPRESS_MANAGE)
        .await?;
    let link = match timada_aliexpress::link_of_product(pool, &access.tenant.id, &product.id)
        .await
        .map_err(topcoat::Error::from_anyhow)?
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
    let link_costs: Vec<(String, String)> = link
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
                    (label, cost)
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
    let options: Vec<OptionRow> = product
        .options
        .iter()
        .map(|option| OptionRow::of(option, &product))
        .collect();
    // The languages the shop is translated into: a shop read in one has
    // no translations tab.
    let other_languages = crate::app::t::slug::announcements::pages(&access)?
        .languages()
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .map(|languages| languages.others)
        .unwrap_or_default();
    // A product no longer linked has no AliExpress tab to come back to.
    let tab = match Tab::of(query.tab.as_deref()) {
        Tab::AliExpress if link.is_none() => Tab::Overview,
        Tab::Translations if other_languages.is_empty() => Tab::Overview,
        tab => tab,
    };
    // Only the tabs that show the stock read it: the overview for a product
    // counted on its own, the variants tab for each variant.
    let shows_stock = match tab {
        Tab::Overview => !has_variants_of(&product),
        Tab::Variants => true,
        _ => false,
    };
    let stocks = if shows_stock {
        stocks_of(&access, &product).await?
    } else {
        vec![StockView::default(); product.variants.len().max(1)]
    };
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
                image.id.clone()
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
    let joinable: Vec<Choice> = rows_of(context::<Db>(cx), &access.tenant.id)
        .await?
        .iter()
        .filter(|row| !product.categories.contains(&row.category.id))
        .map(Choice::of)
        .collect();
    // Only the translations tab reads what the product says elsewhere.
    let translation_cards = if tab == Tab::Translations {
        let translations = catalog(&access)?
            .load_translations(TranslationKind::Product, &product.id, &other_languages)
            .await
            .map_err(topcoat::Error::from_anyhow)?;
        translation_cards_of(&other_languages, &translations)
    } else {
        Vec::new()
    };
    // And what each language reads for the words the product shares with
    // the rest of the shop.
    let mut word_cards = Vec::new();
    if tab == Tab::Translations {
        let words = product.words();
        for language in &other_languages {
            let worded = catalog(&access)?
                .load_words(language, &words)
                .await
                .map_err(topcoat::Error::from_anyhow)?;
            let words = words
                .iter()
                .map(|word| {
                    let wording = worded.get(&timada_catalog::word_key(word));
                    (word.clone(), wording.cloned().unwrap_or_default())
                })
                .collect();
            word_cards.push(WordCard::new(language, words));
        }
    }
    let main_wording = MainWording {
        name: product.name.clone(),
        description: product.description.clone(),
        meta_title: product.meta_title.clone(),
        meta_description: product.meta_description.clone(),
        name_max: MAX_NAME_LEN,
        description_max: MAX_DESCRIPTION_LEN,
        category: None,
        images: product
            .images
            .iter()
            .map(|image| (image.id.clone(), image.alt.clone()))
            .collect(),
    };
    // Only the margin tab reads what a sale costs the shop and the VAT in
    // a price.
    let margin_view = if tab == Tab::Margin {
        let gateway = catalog(&access)?;
        let costs = gateway
            .load_sale_costs()
            .await
            .map_err(topcoat::Error::from_anyhow)?;
        let own = gateway
            .load_product_costs(&product.id)
            .await
            .map_err(topcoat::Error::from_anyhow)?;
        let profile =
            crate::margins::tax_profile(context::<crate::cluster::Executor>(cx), &access.tenant)
                .await
                .map_err(topcoat::Error::from_anyhow)?;
        // What is not an amount is not asked about.
        let what_if = query.what_if.as_deref().and_then(|typed| {
            let cents = timada_catalog::parse_amount(typed).ok()?;
            Some((cents, timada_core::format_amount(cents)))
        });
        Some(MarginView::of(
            &product,
            &own,
            costs.costs.as_ref(),
            costs.tiers(),
            timada_invoicing::tax::home_rate_bps(profile, product.tax_category()),
            what_if,
        ))
    } else {
        None
    };
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
    let delivery_label = product
        .delivery_days
        .map_or_else(|| "None given".to_owned(), crate::ui::delivery_days_label);
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
    let page_tab = |of: Tab, label: &'static str, count: Option<usize>| PageTab {
        label,
        href: product_tab_url(cx, &slug, &product.id, of, None, None),
        count,
        current: tab == of,
    };
    let mut tabs = vec![
        page_tab(Tab::Overview, "Overview", None),
        page_tab(Tab::Pictures, "Pictures", Some(gallery.len())),
        page_tab(Tab::Categories, "Categories", Some(memberships.len())),
        page_tab(Tab::Variants, "Options & variants", Some(variants.len())),
        page_tab(Tab::Facts, "Facts", Some(attributes.len())),
        page_tab(Tab::Shopping, "Shopping feed", None),
        page_tab(Tab::Margin, "Cost & margin", None),
    ];
    if !other_languages.is_empty() {
        tabs.push(page_tab(Tab::Translations, "Translations", None));
    }
    if link.is_some() {
        tabs.push(page_tab(Tab::AliExpress, "AliExpress", None));
    }
    let save_translation =
        href!(translations::save::save, slug: slug.clone(), product_id: product.id.clone())
            .resolve(cx);
    let remove_translation =
        href!(translations::remove::remove, slug: slug.clone(), product_id: product.id.clone())
            .resolve(cx);
    let save_words =
        href!(translations::words::words, slug: slug.clone(), product_id: product.id.clone())
            .resolve(cx);
    // The reduction of a product sold as is, and the most it may be
    // measured from.
    let compare_at = product
        .compare_at(None)
        .map(timada_core::format_amount)
        .unwrap_or_default();
    let compare_at_hint = match product.compare_at_ceiling(None) {
        Some(ceiling) => format!(
            "Struck through beside what it costs. At most {}: the lowest it was on sale at in the 30 days before its current one. Empty announces no reduction.",
            format_money(ceiling, &current_currency)
        ),
        None => "It was on sale at nothing else in the 30 days before its current one, so there is no reduction to announce yet: lower it first.".to_owned(),
    };
    // What the shopping tab fills its fields with.
    let brand = product.brand.clone().unwrap_or_default();
    let gtin = product.gtin.clone().unwrap_or_default();
    let mpn = product.mpn.clone().unwrap_or_default();
    let google_category = product.google_category.clone().unwrap_or_default();
    let age_group = product.age_group.clone().unwrap_or_default();
    let gender = product.gender.clone().unwrap_or_default();
    let feed_labels = product.feed_labels.clone();
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
                        <form method="post" action=(href!(unpublish::unpublish, slug: slug.clone(), product_id: product.id.clone()))>
                            button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Unpublish")
                        </form>
                    } else {
                        <form method="post" action=(href!(publish::publish, slug: slug.clone(), product_id: product.id.clone()))>
                            button(size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Publish")
                        </form>
                    }
                    <form method="post" action=(href!(archive::archive, slug: slug.clone(), product_id: product.id.clone()))>
                        button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Archive")
                    </form>
                }
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())
            if product.is_archived() {
                <p class="mb-6 text-sm text-muted-foreground">"An archived product cannot change any more."</p>
            }
            page_tabs(label: "Product sections", tabs: &tabs)

            if tab == Tab::Overview {
                section_card(
                    title: Some("Overview"),
                    facts(
                        // Whoever can edit reads these in the forms below.
                        if !editable {
                            fact(term: "Slug", <code>(&product.slug)</code>)
                            fact(term: "Price", attrs: attributes! { data-product-price="" data-product-price-range=(&price_label) }, (&price_label))
                            fact(term: "VAT", attrs: attributes! { data-product-vat="" }, (&tax_label))
                        }
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
                        if !editable {
                            fact(
                                term: "Description",
                                attrs: attributes! { data-product-description="" class="whitespace-pre-line" },
                                (if product.description.is_empty() { "—" } else { product.description.as_str() })
                            )
                        }
                        fact(term: "Created", (&created_by) ", " <time datetime=(product.created_at)>(format_time(product.created_at))</time>)
                        fact(term: "Changed", <time datetime=(product.updated_at)>(format_time(product.updated_at))</time>)
                        fact(term: "Changes", attrs: attributes! { data-product-changes=(changes) }, (&changes_label))
                    )
                )

                if editable {
                    <section class="mb-6 grid gap-4 md:grid-cols-2">
                        <form method="post" action=(href!(rename::rename, slug: slug.clone(), product_id: product.id.clone())) class=(class!(CARD, "gap-4 px-6"))>
                            field(
                                field_label(attrs: attributes! { for="product-name" }, "Name")
                                input(attrs: attributes! {
                                    id="product-name" name="name" type="text" maxlength=(MAX_NAME_LEN) required="" value=(&product.name)
                                })
                                <p class="text-xs text-muted-foreground">"Slug: " <code>(&product.slug)</code></p>
                            )
                            <div>
                                button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Rename")
                            </div>
                        </form>
                        <form method="post" action=(href!(price::price, slug: slug.clone(), product_id: product.id.clone())) class=(class!(CARD, "gap-4 px-6"))>
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
                        if !has_variants && product.price.is_some() {
                            <form method="post" action=(href!(compare_at::compare_at, slug: slug.clone(), product_id: product.id.clone())) class=(class!(CARD, "gap-4 px-6")) data-compare-at-form="">
                                field(
                                    field_label(attrs: attributes! { for="product-compare-at" }, "Reduced from")
                                    field_description(attrs: attributes! { id="product-compare-at-hint" }, (&compare_at_hint))
                                    input(attrs: attributes! {
                                        id="product-compare-at" name="amount" type="text" inputmode="decimal" placeholder="19.90"
                                        aria-describedby="product-compare-at-hint" value=(&compare_at)
                                    })
                                )
                                <div>
                                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Apply reduction")
                                </div>
                            </form>
                        }
                        if charges_vat {
                            <form method="post" action=(href!(tax_category::tax_category, slug: slug.clone(), product_id: product.id.clone())) class=(class!(CARD, "gap-4 px-6"))>
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
                        if link.is_some() {
                            <div class=(class!(CARD, "gap-2 px-6")) data-delivery-days=(delivery_label.clone())>
                                <p class="text-sm font-medium">"Delivery time"</p>
                                <p class="text-sm">(delivery_label.clone())</p>
                                <p class="text-xs text-muted-foreground">"Synced from AliExpress: the delivery service an order ships with, to the shop's country."</p>
                            </div>
                        } else {
                            <form method="post" action=(href!(delivery_days::delivery_days, slug: slug.clone(), product_id: product.id.clone())) class=(class!(CARD, "gap-4 px-6"))>
                                <fieldset class="grid gap-2">
                                    <legend class="text-sm font-medium">"Delivery time, in days"</legend>
                                    <div class="flex items-center gap-2">
                                        <label class="sr-only" for="product-delivery-min">"Shortest delivery, in days"</label>
                                        input(attrs: attributes! { id="product-delivery-min" name="min" type="number" min="1" max=(MAX_DELIVERY_DAYS) inputmode="numeric" class="w-24" value=(product.delivery_days.map(|days| days.min.to_string()).unwrap_or_default()) })
                                        <span aria-hidden="true">"to"</span>
                                        <label class="sr-only" for="product-delivery-max">"Longest delivery, in days"</label>
                                        input(attrs: attributes! { id="product-delivery-max" name="max" type="number" min="1" max=(MAX_DELIVERY_DAYS) inputmode="numeric" class="w-24" value=(product.delivery_days.map(|days| days.max.to_string()).unwrap_or_default()) })
                                    </div>
                                    <p class="text-xs text-muted-foreground">"Empty uses the shop's default, set on the Shipping page."</p>
                                </fieldset>
                                <div>
                                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Set delivery time")
                                </div>
                            </form>
                        }
                        <form method="post" action=(href!(describe::describe, slug: slug.clone(), product_id: product.id.clone())) class=(class!(CARD, "gap-4 px-6 md:col-span-2"))>
                            field(
                                field_label(attrs: attributes! { for="product-description" }, "Description")
                                textarea(attrs: attributes! { id="product-description" name="description" rows="5" maxlength=(MAX_DESCRIPTION_LEN) }, (&product.description))
                            )
                            <div>
                                button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Save description")
                            </div>
                        </form>
                        <form method="post" action=(href!(seo::seo, slug: slug.clone(), product_id: product.id.clone())) class=(class!(CARD, "gap-4 px-6 md:col-span-2"))>
                            field(
                                field_label(attrs: attributes! { for="product-meta-title" }, "Search engines' title")
                                field_description(attrs: attributes! { id="product-meta-title-hint" }, "Empty uses the product's name.")
                                input(attrs: attributes! {
                                    id="product-meta-title" name="meta_title" type="text" maxlength=(MAX_META_TITLE_LEN)
                                    aria-describedby="product-meta-title-hint" value=(&product.meta_title)
                                })
                            )
                            field(
                                field_label(attrs: attributes! { for="product-meta-description" }, "Search engines' description")
                                field_description(attrs: attributes! { id="product-meta-description-hint" }, "Empty uses the start of the description.")
                                textarea(
                                    attrs: attributes! {
                                        id="product-meta-description" name="meta_description" rows="3" maxlength=(MAX_META_DESCRIPTION_LEN)
                                        aria-describedby="product-meta-description-hint"
                                    },
                                    (&product.meta_description)
                                )
                            )
                            <div>
                                button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Save for search engines")
                            </div>
                        </form>
                    </section>
                }

                if let Some(stock) = &own_stock {
                    if editable {
                        section_card(
                            title: Some("Stock"),
                            description: Some("How many are on the shelf. A product whose stock is not counted sells without limit; one counted at 0 is sold out."),
                            <form method="post" action=(href!(stock::stock, slug: slug.clone(), product_id: product.id.clone())) class="flex flex-wrap items-end gap-2">
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

                if deletable {
                    <form method="post" action=(href!(delete::delete, slug: slug.clone(), product_id: product.id.clone())) class="mb-6 flex flex-wrap items-center gap-3">
                        button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Delete product")
                        <p class="text-sm text-muted-foreground">"For good: nobody has ordered it, so it can go as if it had never been. Its slug is free again and its pictures are deleted."</p>
                    </form>
                }
            }

            if tab == Tab::Pictures {
                section_card(
                    title: Some("Pictures"),
                    if gallery.is_empty() {
                        <p class="text-sm text-muted-foreground">"No picture yet. The first one is what a listing shows."</p>
                    }
                    <ul class="grid gap-4 sm:grid-cols-2">
                        for (position, (image, url)) in gallery.iter().enumerate() {
                            <li class="flex gap-3 rounded-lg border border-border p-3" data-image=(&image.id)>
                                <img class="h-24 w-24 shrink-0 rounded-md object-cover" src=(url) alt=(&image.alt)
                                    width=(image.picture.listing.width) height=(image.picture.listing.height) loading="lazy">
                                <div class="min-w-0 flex-1 text-sm">
                                    if position == 0 {
                                        <p class="mb-1 text-xs text-muted-foreground" data-image-first="">"Shown in listings"</p>
                                    }
                                    if editable {
                                        <form method="post" action=(href!(images::image_id::alt::alt, slug: slug.clone(), product_id: product.id.clone(), images::image_id::image_id: image.id.clone()))>
                                            <label class="sr-only" for=(format!("alt-{}", image.id))>"What it shows"</label>
                                            input(attrs: attributes! {
                                                id=(format!("alt-{}", image.id)) name="alt" type="text" maxlength=(MAX_ALT_LEN)
                                                class="h-8 px-2" placeholder="What it shows" value=(&image.alt)
                                            })
                                            button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="mt-2 h-7 px-2 text-xs" }, "Save")
                                        </form>
                                        <div class="mt-2 flex flex-wrap items-center gap-2">
                                            if position > 0 {
                                                <form method="post" action=(href!(images::image_id::up::up, slug: slug.clone(), product_id: product.id.clone(), images::image_id::image_id: image.id.clone()))>
                                                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="h-7 px-2 text-xs" aria-label="Move earlier" }, "↑")
                                                </form>
                                            }
                                            if position < last {
                                                <form method="post" action=(href!(images::image_id::down::down, slug: slug.clone(), product_id: product.id.clone(), images::image_id::image_id: image.id.clone()))>
                                                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="h-7 px-2 text-xs" aria-label="Move later" }, "↓")
                                                </form>
                                            }
                                            <form method="post" action=(href!(images::image_id::remove::remove, slug: slug.clone(), product_id: product.id.clone(), images::image_id::image_id: image.id.clone()))>
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
                            action=(href!(images::upload::upload, slug: slug.clone(), product_id: product.id.clone()))>
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
            }

            if tab == Tab::Categories {
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
                                        <a class=(LINK) href=(href!(crate::app::t::slug::categories::category_id::index, slug: slug.clone(), crate::app::t::slug::categories::category_id::category_id: membership.id.clone()))>(&membership.name)</a>
                                    }
                                </span>
                                if editable {
                                    <form method="post" action=(href!(uncategorize::uncategorize, slug: slug.clone(), product_id: product.id.clone()))>
                                        <input type="hidden" name="category_id" value=(&membership.id)>
                                        button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="h-7 px-2 text-xs" }, "Remove")
                                    </form>
                                }
                            </li>
                        }
                    </ul>
                    if live_memberships.len() > 1 {
                        <form method="post" action=(href!(main_category::main_category, slug: slug.clone(), product_id: product.id.clone())) class="mt-3 flex flex-wrap items-center gap-2" data-main-category=(&main_category)>
                            <label class="text-sm font-medium" for="product-main-category">"Its address is under"</label>
                            select(
                                attrs: attributes! { id="product-main-category" name="category_id" disabled=(!editable) aria-describedby="product-main-category-hint" class="min-w-0 flex-1 sm:max-w-sm" },
                                for (id, name) in &live_memberships {
                                    <option value=(id) selected=(*id == main_category)>(name)</option>
                                }
                            )
                            if editable {
                                button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Set main category")
                            }
                            <p id="product-main-category-hint" class="basis-full text-sm text-muted-foreground">"The product's address is under this category wherever a shopper did not come from another one, and it is the address search engines are told."</p>
                        </form>
                    }
                    if editable && !joinable.is_empty() {
                        <form method="post" action=(href!(categorize::categorize, slug: slug.clone(), product_id: product.id.clone())) class="mt-3 flex flex-wrap items-center gap-2">
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
            }

            if tab == Tab::Variants {
                section_card(
                    title: Some("Options"),
                    description: Some("A product that comes in several colours or sizes varies by them. Settle the options first: once it sells variants they cannot change. A label is what the shop shows instead of a name or a value, and can change at any time; an import that named them still finds them. An option shoppers should not choose, like where an imported product ships from or to, can be hidden at one value: only the variants taking it stay on sale."),
                    if options.is_empty() {
                        <p class="text-sm text-muted-foreground">"This product does not vary. It sells at the price in its overview."</p>
                    }
                    <ul class="flex flex-col gap-3">
                        for option in &options {
                            <li class="rounded-lg border border-border p-3" data-option=(&option.id)>
                                if editable && !options_locked {
                                    <form method="post" action=(href!(options::option_id::values::values, slug: slug.clone(), product_id: product.id.clone(), options::option_id::option_id: option.id.clone())) class="flex flex-col gap-3">
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
                                    <form method="post" action=(href!(options::option_id::remove::remove, slug: slug.clone(), product_id: product.id.clone(), options::option_id::option_id: option.id.clone())) class="mt-2">
                                        button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="h-7 px-2 text-xs" }, "Remove option")
                                    </form>
                                } else {
                                    <p class="text-sm"><strong>(&option.name)</strong></p>
                                    <p class="whitespace-pre-line text-sm text-muted-foreground">(&option.values)</p>
                                }
                                if !option.fixed_value.is_empty() {
                                    <p class="mt-2 text-sm text-muted-foreground" data-option-hidden=(&option.fixed_value)>"Hidden from shoppers: only the variants with " (&option.fixed_value) " are on sale."</p>
                                }
                                if editable && (has_variants || !option.fixed_value.is_empty()) {
                                    <form method="post" action=(href!(options::option_id::visibility::visibility, slug: slug.clone(), product_id: product.id.clone(), options::option_id::option_id: option.id.clone())) class="mt-3 flex flex-wrap items-center gap-2 border-t border-border pt-3" data-option-visibility="">
                                        <label class="text-sm" for=(format!("option-visibility-{}", option.id))>"Shoppers see " (&option.name)</label>
                                        select(
                                            attrs: attributes! { id=(format!("option-visibility-{}", option.id)) name="value" class="w-56" },
                                            <option value="" selected=(option.fixed_value.is_empty())>"Yes, they choose"</option>
                                            for value in &option.hide_at {
                                                <option value=(value) selected=(&option.fixed_value == value)>"No, always " (value)</option>
                                            }
                                        )
                                        button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Save visibility")
                                    </form>
                                }
                                if editable {
                                    <form method="post" action=(href!(options::option_id::labels::labels, slug: slug.clone(), product_id: product.id.clone(), options::option_id::option_id: option.id.clone())) class="mt-3 flex flex-col gap-2 border-t border-border pt-3" data-option-labels="">
                                        <div class="flex flex-wrap items-center gap-2">
                                            <label class="text-sm" for=(format!("option-label-{}", option.id))>"Label of " (&option.name)</label>
                                            input(attrs: attributes! {
                                                id=(format!("option-label-{}", option.id)) name="label" type="text" maxlength=(MAX_OPTION_LABEL_LEN)
                                                class="h-8 w-40 px-2" value=(&option.label) placeholder=(&option.name)
                                            })
                                        </div>
                                        for (value, label) in &option.value_labels {
                                            <div class="flex flex-wrap items-center gap-2" data-value-label=(value)>
                                                <label class="text-sm text-muted-foreground" for=(format!("value-label-{}-{}", option.id, value))>"Label of " (value) " (" (&option.name) ")"</label>
                                                input(attrs: attributes! {
                                                    id=(format!("value-label-{}-{}", option.id, value)) name=(format!("{}{}", options::option_id::labels::VALUE_FIELD, value)) type="text" maxlength=(MAX_OPTION_LABEL_LEN)
                                                    class="h-8 w-40 px-2" value=(label) placeholder=(value)
                                                })
                                            </div>
                                        }
                                        <div>
                                            button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Save labels")
                                        </div>
                                    </form>
                                }
                            </li>
                        }
                    </ul>
                    if options_locked {
                        <p class="mt-3 text-sm text-muted-foreground">"Remove every variant before changing the options."</p>
                    }
                    if can_add_option {
                        <form method="post" action=(href!(options::define::define, slug: slug.clone(), product_id: product.id.clone())) class="mt-4 flex max-w-xl flex-col gap-4">
                            field(
                                field_label(attrs: attributes! { for="new-option-name" }, "Add an option")
                                input(attrs: attributes! {
                                    id="new-option-name" name="name" type="text" maxlength=(MAX_OPTION_NAME_LEN) required="" placeholder="Colour"
                                })
                            )
                            field(
                                field_label(attrs: attributes! { for="new-option-values" }, "Values, one a line")
                                textarea(attrs: attributes! { id="new-option-values" name="values" rows="3" required="" placeholder=("Beige\nBlue\nGrey") })
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
                        description: Some("One combination a shopper buys, with its own price. A product with variants is published without a price of its own. A variant on sale for less than before may say what it is reduced from: at most the lowest it was on sale at in the 30 days before its current price."),
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
                                        attrs: attributes! { data-variant=(&variant.id) data-variant-on-sale=(variant.on_sale) },
                                        table_cell(
                                            attrs: attributes! { data-variant-selections=(&variant.selections) },
                                            (&variant.selections)
                                            if !variant.on_sale {
                                                <p class="text-xs text-muted-foreground">"Not on sale"</p>
                                            }
                                        )
                                        table_cell(
                                            if editable {
                                                <form method="post" action=(href!(variants::variant_id::price::price, slug: slug.clone(), product_id: product.id.clone(), variants::variant_id::variant_id: variant.id.clone())) class="flex items-center gap-1">
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
                                                <form method="post" action=(href!(variants::variant_id::compare_at::compare_at, slug: slug.clone(), product_id: product.id.clone(), variants::variant_id::variant_id: variant.id.clone())) class="mt-1 flex items-center gap-1" data-variant-compare-at="">
                                                    <label class="text-xs text-muted-foreground" for=(format!("variant-compare-at-{}", variant.id))>"Reduced from" <span class="sr-only">", for " (&variant.selections)</span></label>
                                                    input(attrs: attributes! {
                                                        id=(format!("variant-compare-at-{}", variant.id)) name="amount" type="text" inputmode="decimal"
                                                        class="h-8 w-20 px-2" value=(&variant.compare_at)
                                                        placeholder=(if variant.compare_at_ceiling.is_empty() { "—".to_owned() } else { format!("≤ {}", variant.compare_at_ceiling) })
                                                    })
                                                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="h-7 px-2 text-xs" }, "Apply reduction")
                                                </form>
                                            } else {
                                                (&variant.amount) " " (&variant.currency)
                                                if !variant.compare_at.is_empty() {
                                                    " " <s class="text-muted-foreground">(&variant.compare_at)</s>
                                                }
                                            }
                                        )
                                        table_cell(
                                            if editable {
                                                <form method="post" action=(href!(variants::variant_id::sku::sku, slug: slug.clone(), product_id: product.id.clone(), variants::variant_id::variant_id: variant.id.clone())) class="flex items-center gap-1">
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
                                                <form method="post" action=(href!(stock::stock, slug: slug.clone(), product_id: product.id.clone())) class="flex items-center gap-1">
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
                                                <form method="post" action=(href!(variants::variant_id::image::image, slug: slug.clone(), product_id: product.id.clone(), variants::variant_id::variant_id: variant.id.clone())) class="flex items-center gap-1">
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
                                                <form method="post" action=(href!(variants::variant_id::remove::remove, slug: slug.clone(), product_id: product.id.clone(), variants::variant_id::variant_id: variant.id.clone()))>
                                                    button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="h-7 px-2 text-xs" }, "Remove")
                                                </form>
                                            }
                                        )
                                    )
                                }
                            )
                        )
                        if can_add_variant {
                            <form method="post" action=(href!(variants::define::define, slug: slug.clone(), product_id: product.id.clone())) class="mt-4 flex max-w-xl flex-col gap-4">
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
            }

            if tab == Tab::Facts {
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
                                            <form method="post" action=(href!(attributes::remove::remove, slug: slug.clone(), product_id: product.id.clone()))>
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
                        <form method="post" action=(href!(attributes::set::set, slug: slug.clone(), product_id: product.id.clone())) class="mt-4 flex flex-wrap items-end gap-2">
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
            }

            if let Some(margin) = &margin_view {
                margin_tab(editable: editable, margin: margin)
            }

            if tab == Tab::Translations {
                translation_cards(editable: editable, save: &save_translation, remove: &remove_translation, main: &main_wording, cards: &translation_cards)
                word_cards(editable: editable, save: &save_words, cards: &word_cards)
            }

            if tab == Tab::Shopping {
                section_card(
                    title: Some("In the feed"),
                    description: Some("The shop's shopping feed carries every published product. One taken out of it stays in the shop and out of the ads."),
                    <form method="post" action=(href!(feed_listing::feed_listing, slug: slug.clone(), product_id: product.id.clone())) data-feed-listing="">
                        <fieldset disabled=(!editable) class="flex max-w-xl flex-col gap-4">
                            label(
                                checkbox(attrs: attributes! { name="listed" value="on" checked=(!product.off_feed) })
                                "Send this product to the shopping feed"
                            )
                            if editable {
                                <div>button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Save feed listing")</div>
                            }
                        </fieldset>
                    </form>
                )

                section_card(
                    title: Some("Identifiers"),
                    description: Some("Who makes the product and the numbers it is known under in every shop. A shopping feed asks for the brand, and for the GTIN (the digits under the barcode) wherever the product has one. A product with variants gives each its own numbers, below."),
                    <form method="post" action=(href!(identify::identify, slug: slug.clone(), product_id: product.id.clone())) data-identifiers="">
                        <fieldset disabled=(!editable) class="flex max-w-xl flex-col gap-4">
                            field(
                                field_label(attrs: attributes! { for="product-brand" }, "Brand")
                                input(attrs: attributes! {
                                    id="product-brand" name="brand" type="text" maxlength=(MAX_BRAND_LEN) value=(&brand)
                                })
                            )
                            if !has_variants {
                                field(
                                    field_label(attrs: attributes! { for="product-gtin" }, "GTIN")
                                    field_description(attrs: attributes! { id="product-gtin-hint" }, "8, 12, 13 or 14 digits: an EAN, a UPC or an ISBN.")
                                    input(attrs: attributes! {
                                        id="product-gtin" name="gtin" type="text" inputmode="numeric" maxlength="20"
                                        aria-describedby="product-gtin-hint" value=(&gtin)
                                    })
                                )
                                field(
                                    field_label(attrs: attributes! { for="product-mpn" }, "Part number (MPN)")
                                    field_description(attrs: attributes! { id="product-mpn-hint" }, "The maker's own number for it.")
                                    input(attrs: attributes! {
                                        id="product-mpn" name="mpn" type="text" maxlength=(MAX_MPN_LEN)
                                        aria-describedby="product-mpn-hint" value=(&mpn)
                                    })
                                )
                            }
                            label(
                                checkbox(attrs: attributes! { name="no_identifier" value="on" checked=(product.no_identifier) })
                                "This product has no GTIN and no part number"
                            )
                            <p class="text-sm text-muted-foreground">"Tick it only for what truly has none, like something made by hand or to order: a feed then says so instead of leaving the numbers out."</p>
                            if editable {
                                <div>button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Save identifiers")</div>
                            }
                        </fieldset>
                    </form>
                    if has_variants {
                        table(
                            attrs: attributes! { class="mt-6" data-variant-identifiers="" },
                            table_header(
                                table_row(
                                    table_head("Combination")
                                    table_head("GTIN")
                                    table_head("Part number (MPN)")
                                    table_head(<span class="sr-only">"Actions"</span>)
                                )
                            )
                            table_body(
                                for variant in &variants {
                                    table_row(
                                        attrs: attributes! { data-variant=(&variant.id) },
                                        table_cell((&variant.selections))
                                        table_cell(
                                            <label class="sr-only" for=(format!("variant-gtin-{}", variant.id))>"GTIN of " (&variant.selections)</label>
                                            input(attrs: attributes! {
                                                id=(format!("variant-gtin-{}", variant.id)) form=(format!("variant-identifiers-{}", variant.id))
                                                name="gtin" type="text" inputmode="numeric" maxlength="20" disabled=(!editable)
                                                class="h-8 w-40 px-2" value=(&variant.gtin)
                                            })
                                        )
                                        table_cell(
                                            <label class="sr-only" for=(format!("variant-mpn-{}", variant.id))>"Part number of " (&variant.selections)</label>
                                            input(attrs: attributes! {
                                                id=(format!("variant-mpn-{}", variant.id)) form=(format!("variant-identifiers-{}", variant.id))
                                                name="mpn" type="text" maxlength=(MAX_MPN_LEN) disabled=(!editable)
                                                class="h-8 w-40 px-2" value=(&variant.mpn)
                                            })
                                        )
                                        table_cell(
                                            attrs: attributes! { class="text-right" },
                                            if editable {
                                                <form method="post" id=(format!("variant-identifiers-{}", variant.id)) action=(href!(variants::variant_id::identify::identify, slug: slug.clone(), product_id: product.id.clone(), variants::variant_id::variant_id: variant.id.clone()))>
                                                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="h-7 px-2 text-xs" }, "Save")
                                                </form>
                                            }
                                        )
                                    )
                                }
                            )
                        )
                    }
                )

                section_card(
                    title: Some("Where a feed files it"),
                    description: Some("Its category in Google's product taxonomy, as its number or its full path, and whom it is made for. Clothing and shoes need the age group and the gender; most other goods need neither."),
                    <form method="post" action=(href!(classify::classify, slug: slug.clone(), product_id: product.id.clone())) data-classification="">
                        <fieldset disabled=(!editable) class="flex max-w-xl flex-col gap-4">
                            field(
                                field_label(attrs: attributes! { for="product-google-category" }, "Google product category")
                                input(attrs: attributes! {
                                    id="product-google-category" name="google_category" type="text" maxlength=(MAX_GOOGLE_CATEGORY_LEN)
                                    value=(&google_category) placeholder="Animals & Pet Supplies > Pet Supplies > Cat Supplies"
                                })
                            )
                            field(
                                field_label(attrs: attributes! { for="product-age-group" }, "Age group")
                                select(
                                    attrs: attributes! { id="product-age-group" name="age_group" class="w-64" },
                                    <option value="" selected=(age_group.is_empty())>"Not said"</option>
                                    for (key, label) in AGE_GROUPS {
                                        <option value=(*key) selected=(age_group == *key)>(*label)</option>
                                    }
                                )
                            )
                            field(
                                field_label(attrs: attributes! { for="product-gender" }, "Gender")
                                select(
                                    attrs: attributes! { id="product-gender" name="gender" class="w-64" },
                                    <option value="" selected=(gender.is_empty())>"Not said"</option>
                                    for (key, label) in GENDERS {
                                        <option value=(*key) selected=(gender == *key)>(*label)</option>
                                    }
                                )
                            )
                            if editable {
                                <div>button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Save classification")</div>
                            }
                        </fieldset>
                    </form>
                    if !options.is_empty() {
                        <ul class="mt-6 flex flex-col gap-2 border-t border-border pt-4" data-option-kinds="">
                            for option in &options {
                                <li data-option=(&option.id)>
                                    <form method="post" action=(href!(options::option_id::kind::kind, slug: slug.clone(), product_id: product.id.clone(), options::option_id::option_id: option.id.clone())) class="flex flex-wrap items-center gap-2">
                                        <label class="text-sm" for=(format!("option-kind-{}", option.id))>"To a feed, " (&option.name) " is"</label>
                                        select(
                                            attrs: attributes! { id=(format!("option-kind-{}", option.id)) name="kind" class="w-48" disabled=(!editable) },
                                            <option value="" selected=(option.kind.is_empty())>"None of these"</option>
                                            for (key, label) in OPTION_KINDS {
                                                <option value=(*key) selected=(option.kind == *key)>(*label)</option>
                                            }
                                        )
                                        if editable {
                                            button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Save kind")
                                        }
                                    </form>
                                </li>
                            }
                        </ul>
                    }
                )

                section_card(
                    title: Some("Campaign labels"),
                    description: Some("Five free labels to group products by in ad campaigns: a margin tier, where it ships from, a season. Shoppers never see them. The products list sets one on many products at once."),
                    <form method="post" action=(href!(feed_labels::feed_labels, slug: slug.clone(), product_id: product.id.clone())) data-feed-labels="">
                        <fieldset disabled=(!editable) class="flex max-w-xl flex-col gap-4">
                            for (n, feed_label) in feed_labels.iter().enumerate() {
                                field(
                                    field_label(attrs: attributes! { for=(format!("product-feed-label-{n}")) }, "Label " (n))
                                    input(attrs: attributes! {
                                        id=(format!("product-feed-label-{n}")) name=(format!("label_{n}")) type="text"
                                        maxlength=(MAX_FEED_LABEL_LEN) value=(feed_label)
                                    })
                                )
                            }
                            if editable {
                                <div>button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Save labels")</div>
                            }
                        </fieldset>
                    </form>
                )

                section_card(
                    title: Some("Written by a machine"),
                    description: Some("A shopping feed has to say which texts generative AI wrote. Tick what applies to this product as it reads today."),
                    <form method="post" action=(href!(ai_writing::ai_writing, slug: slug.clone(), product_id: product.id.clone())) data-ai-writing="">
                        <fieldset disabled=(!editable) class="flex max-w-xl flex-col gap-4">
                            label(
                                checkbox(attrs: attributes! { name="title" value="on" checked=(product.ai_title) })
                                "AI wrote the name"
                            )
                            label(
                                checkbox(attrs: attributes! { name="description" value="on" checked=(product.ai_description) })
                                "AI wrote the description"
                            )
                            if editable {
                                <div>button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Save")</div>
                            }
                        </fieldset>
                    </form>
                )
            }

            if tab == Tab::AliExpress {
                if let Some(link) = &link {
                    section_card(
                        title: Some("AliExpress"),
                        description: Some("Imported from AliExpress: its stock, and its price when a rule prices it, are read again from there. Its costs include what AliExpress charges to deliver one to the shop's country."),
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
                                for (label, cost) in &link_costs {
                                    <li>(label) " · costs " (cost)</li>
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
                                                <form method="post" action=(href!(crate::app::t::slug::aliexpress::products::link_id::combinations::combinations, slug: slug.clone(), crate::app::t::slug::aliexpress::products::link_id::link_id: link.id.clone()))>
                                                    <input type="hidden" name="back" value=(&product.id)>
                                                    <input type="hidden" name="sku_id" value=(sku_id)>
                                                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" aria-label=(format!("Add {label} as a variant")) }, "Add as variant")
                                                </form>
                                            }
                                            if can_aliexpress {
                                                <form method="post" action=(href!(crate::app::t::slug::aliexpress::products::link_id::ignore::ignore, slug: slug.clone(), crate::app::t::slug::aliexpress::products::link_id::link_id: link.id.clone()))>
                                                    <input type="hidden" name="back" value=(&product.id)>
                                                    <input type="hidden" name="sku_id" value=(sku_id)>
                                                    button(variant: ButtonVariant::Ghost, size: ButtonSize::Sm, attrs: attributes! { type="submit" aria-label=(format!("Ignore {label}")) }, "Ignore")
                                                </form>
                                            }
                                        </li>
                                    }
                                </ul>
                                if can_aliexpress && found_fitting > 1 {
                                    <form class="mt-2" method="post" action=(href!(crate::app::t::slug::aliexpress::products::link_id::combinations::combinations, slug: slug.clone(), crate::app::t::slug::aliexpress::products::link_id::link_id: link.id.clone()))>
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
                                                        <form method="post" action=(href!(crate::app::t::slug::aliexpress::products::link_id::restore::restore, slug: slug.clone(), crate::app::t::slug::aliexpress::products::link_id::link_id: link.id.clone()))>
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
                                <form method="post" action=(href!(crate::app::t::slug::aliexpress::products::link_id::sync::sync, slug: slug.clone(), crate::app::t::slug::aliexpress::products::link_id::link_id: link.id.clone()))>
                                    <input type="hidden" name="back" value=(&product.id)>
                                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Sync now")
                                </form>
                                <form method="post" action=(href!(crate::app::t::slug::aliexpress::products::link_id::unlink::unlink, slug: slug.clone(), crate::app::t::slug::aliexpress::products::link_id::link_id: link.id.clone()))>
                                    <input type="hidden" name="back" value=(&product.id)>
                                    button(variant: ButtonVariant::Ghost, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Stop keeping in step")
                                </form>
                            </div>
                            <form method="post" action=(href!(crate::app::t::slug::aliexpress::products::link_id::pricing::pricing, slug: slug.clone(), crate::app::t::slug::aliexpress::products::link_id::link_id: link.id.clone())) class="mt-4 flex flex-wrap items-end gap-2">
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
pub struct SeoForm {
    #[serde(default)]
    pub meta_title: String,
    #[serde(default)]
    pub meta_description: String,
}

#[derive(serde::Deserialize)]
pub struct PriceForm {
    pub amount: String,
    pub currency: String,
}

#[derive(serde::Deserialize)]
pub struct CompareAtForm {
    /// Empty announces no reduction.
    #[serde(default)]
    pub amount: String,
}

#[derive(serde::Deserialize)]
pub struct IdentifyForm {
    #[serde(default)]
    pub brand: String,
    /// Not posted for a product with variants: each has its own.
    #[serde(default)]
    pub gtin: String,
    #[serde(default)]
    pub mpn: String,
    /// An unticked checkbox is not sent at all.
    #[serde(default)]
    pub no_identifier: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct ClassifyForm {
    #[serde(default)]
    pub google_category: String,
    /// A key of `AGE_GROUPS`, or empty.
    #[serde(default)]
    pub age_group: String,
    /// A key of `GENDERS`, or empty.
    #[serde(default)]
    pub gender: String,
}

#[derive(serde::Deserialize)]
pub struct FeedLabelsForm {
    #[serde(default)]
    pub label_0: String,
    #[serde(default)]
    pub label_1: String,
    #[serde(default)]
    pub label_2: String,
    #[serde(default)]
    pub label_3: String,
    #[serde(default)]
    pub label_4: String,
}

/// What the product costs, as its margin tab posts it.
#[derive(serde::Deserialize)]
pub struct CostForm {
    /// Of a product sold as is; empty forgets it.
    #[serde(default)]
    pub cost: Option<String>,
    /// One cost for every combination; empty sets each from its own field.
    #[serde(default)]
    pub every: String,
    /// The combinations' own fields, among anything else posted.
    #[serde(flatten)]
    pub fields: std::collections::HashMap<String, String>,
}

#[derive(serde::Deserialize)]
pub struct OwnCostsForm {
    /// Both empty for the shop's.
    #[serde(default)]
    pub shipping: String,
    #[serde(default)]
    pub import: String,
}

#[derive(serde::Deserialize)]
pub struct FeedListingForm {
    /// An unticked checkbox is not sent at all.
    #[serde(default)]
    pub listed: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct AiWritingForm {
    /// An unticked checkbox is not sent at all.
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct DeliveryDaysForm {
    /// Days, both empty for the shop's default.
    #[serde(default)]
    pub min: String,
    #[serde(default)]
    pub max: String,
}

#[derive(serde::Deserialize)]
pub struct TaxCategoryForm {
    /// A key of `TAX_CATEGORIES`.
    pub category: String,
}
