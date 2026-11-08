//! What a cart is priced with, as the read model holds it: a product's
//! name, price, kind of goods and delivery time, the combinations a shopper
//! can buy, what is on the shelf of each, and the shop's delivery time. The
//! orders context builds the same [`Sellable`] from a product's own stream
//! when a payment is priced for real, so one set of rules prices both.

use anyhow::Result;
use timada_core::db::{Db, FromRow, fetch_all, fetch_optional};

use crate::command::{Price, ProductState};
use crate::value_object::DeliveryDays;

/// A product as a cart prices it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sellable {
    pub id: String,
    pub name: String,
    pub published: bool,
    /// Its own price, for a product sold as one thing.
    pub price: Option<Price>,
    /// What kind of goods it is for VAT (`standard` unless told otherwise).
    pub tax_category: String,
    /// How long delivery takes; `None` is the shop's default.
    pub delivery_days: Option<DeliveryDays>,
    /// How many combinations it has, on sale or not: one with any sells
    /// through them alone.
    pub variant_count: usize,
    /// The combinations a shopper can buy.
    pub variants: Vec<SellableVariant>,
}

/// One combination a shopper can buy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SellableVariant {
    pub id: String,
    pub sku: Option<String>,
    /// Its values as the shop shows them (`Sable / Large`); `None` when
    /// every option is hidden.
    pub label: Option<String>,
    pub price: Price,
}

impl Sellable {
    /// The product as its own stream has it.
    pub fn of(product: &ProductState) -> Self {
        Self {
            id: product.id.clone(),
            name: product.name.clone(),
            published: product.is_published(),
            // A product sold through its combinations carries no price of
            // its own, as the read model has it too ([`sellable`]).
            price: product
                .price
                .clone()
                .filter(|_| product.variants.is_empty()),
            tax_category: product.tax_category().to_owned(),
            delivery_days: product.delivery_days,
            variant_count: product.variants.len(),
            variants: product
                .sold_variants()
                .map(|variant| SellableVariant {
                    id: variant.id.clone(),
                    sku: variant.sku.clone(),
                    label: product.variant_label(variant),
                    price: variant.price.clone(),
                })
                .collect(),
        }
    }

    /// Whether it sells through combinations rather than as one thing.
    pub fn has_variants(&self) -> bool {
        self.variant_count > 0
    }

    /// Whether a shopper could be charged for it: a plain price, or at least
    /// one combination on sale to pick.
    pub fn is_priced(&self) -> bool {
        if self.has_variants() {
            !self.variants.is_empty()
        } else {
            self.price.is_some()
        }
    }

    /// One of the combinations a shopper can buy, by id.
    pub fn variant(&self, variant_id: &str) -> Option<&SellableVariant> {
        self.variants
            .iter()
            .find(|variant| variant.id == variant_id)
    }

    /// The currency everything on it is priced in.
    pub fn currency(&self) -> Option<&str> {
        self.price
            .as_ref()
            .map(|price| price.currency.as_str())
            .or_else(|| self.variants.first().map(|v| v.price.currency.as_str()))
    }
}

#[derive(FromRow)]
struct SellableRow {
    id: String,
    name: String,
    status: String,
    amount_cents: Option<i64>,
    currency: Option<String>,
    tax_category: String,
    delivery_min_days: Option<i64>,
    delivery_max_days: Option<i64>,
    variant_count: i64,
}

#[derive(FromRow)]
struct VariantRow {
    id: String,
    sku: Option<String>,
    label: Option<String>,
    amount_cents: i64,
    currency: String,
}

/// A product as the read model has it, or `None` for one the tenant has
/// not (or no longer: a deleted product loses its rows). The row and its
/// combinations are read in one transaction, so they are of one moment: the
/// subscription writes both in one.
pub async fn sellable(db: &Db, tenant_id: &str, product_id: &str) -> Result<Option<Sellable>> {
    let env = (tenant_id, product_id);
    let rows = db
        .read(&env, |conn, (tenant_id, product_id)| {
            Box::pin(async move {
                let Some(row) = fetch_optional::<SellableRow>(
                    conn,
                    "SELECT id, name, status, amount_cents, currency, tax_category,
                        delivery_min_days, delivery_max_days, variant_count
                     FROM catalog_products WHERE tenant_id = ?1 AND id = ?2",
                    (*tenant_id, *product_id),
                )
                .await?
                else {
                    return Ok(None);
                };
                let variants: Vec<VariantRow> = fetch_all(
                    conn,
                    "SELECT id, sku, label, amount_cents, currency FROM catalog_variants
                     WHERE tenant_id = ?1 AND product_id = ?2 AND on_sale = 1
                     ORDER BY position",
                    (*tenant_id, *product_id),
                )
                .await?;
                Ok(Some((row, variants)))
            })
        })
        .await?;
    let Some((row, variants)) = rows else {
        return Ok(None);
    };
    let price = match (row.amount_cents, &row.currency) {
        // A product sold through its combinations carries their currency
        // and no amount of its own.
        (Some(amount_cents), Some(currency)) if row.variant_count == 0 => Some(Price {
            amount_cents: amount_cents.max(0) as u64,
            currency: currency.clone(),
        }),
        _ => None,
    };
    let delivery_days = match (row.delivery_min_days, row.delivery_max_days) {
        (Some(min), Some(max)) => Some(DeliveryDays {
            min: min.clamp(0, i64::from(u16::MAX)) as u16,
            max: max.clamp(0, i64::from(u16::MAX)) as u16,
        }),
        _ => None,
    };
    Ok(Some(Sellable {
        id: row.id,
        name: row.name,
        published: row.status == crate::command::ProductStatus::Published.as_str(),
        price,
        tax_category: row.tax_category,
        delivery_days,
        variant_count: row.variant_count.max(0) as usize,
        variants: variants
            .into_iter()
            .map(|variant| SellableVariant {
                id: variant.id,
                sku: variant.sku,
                label: variant.label,
                price: Price {
                    amount_cents: variant.amount_cents.max(0) as u64,
                    currency: variant.currency,
                },
            })
            .collect(),
    }))
}

/// What is on the shelf of a thing, as the read model has it: `None` for
/// one that is not counted (it sells without limit). What other checkouts
/// hold of it is not taken off: that is the payment's exact check, against
/// the stock's own stream.
pub async fn on_hand(
    db: &Db,
    tenant_id: &str,
    product_id: &str,
    variant_id: Option<&str>,
) -> Result<Option<u32>> {
    let row: Option<(bool, i64)> = db
        .fetch_optional(
            "SELECT tracked, on_hand FROM catalog_stock
             WHERE tenant_id = ?1 AND product_id = ?2 AND variant_id = ?3",
            (tenant_id, product_id, variant_id.unwrap_or("")),
        )
        .await?;
    Ok(row
        .filter(|(tracked, _)| *tracked)
        .map(|(_, on_hand)| on_hand.clamp(0, i64::from(u32::MAX)) as u32))
}

/// The shop's delivery time for a product that gives none, as the read
/// model has it.
pub async fn default_delivery_days(db: &Db, tenant_id: &str) -> Result<Option<DeliveryDays>> {
    let row: Option<(Option<i64>, Option<i64>)> = db
        .fetch_optional(
            "SELECT min_days, max_days FROM catalog_delivery_default WHERE tenant_id = ?1",
            (tenant_id,),
        )
        .await?;
    Ok(match row {
        Some((Some(min), Some(max))) => Some(DeliveryDays {
            min: min.clamp(0, i64::from(u16::MAX)) as u16,
            max: max.clamp(0, i64::from(u16::MAX)) as u16,
        }),
        _ => None,
    })
}
