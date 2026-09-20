//! Product families on the read side: the SQL tables `catalog_family` and
//! `catalog_family_variant`, fed by the `catalog-family-list` subscription,
//! for the pages that list families — and [`variant_choices`], what a product
//! page offers a shopper to go from one version of an article to another.

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;

use crate::{
    aggregator::{
        FamilyCreated, FamilyDissolved, FamilyOptionsDefined, FamilyRenamed, FamilyVariantPlaced,
        FamilyVariantRemoved,
    },
    command::{Command, FamilyState},
    value_object::{FamilyOption, OptionValue},
};

/// Subscription key; the caller attaches the pool with `.data(pool)`.
pub const FAMILY_LIST_SUBSCRIPTION: &str = "catalog-family-list";

/// Between two entries, and inside one entry: characters no operator types.
const ENTRY_SEPARATOR: char = '\u{1e}';
const PART_SEPARATOR: char = '\u{1f}';

fn encode_options(options: &[FamilyOption]) -> String {
    options
        .iter()
        .map(|option| {
            std::iter::once(option.name.as_str())
                .chain(option.values.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(&PART_SEPARATOR.to_string())
        })
        .collect::<Vec<_>>()
        .join(&ENTRY_SEPARATOR.to_string())
}

fn encode_values(values: &[OptionValue]) -> String {
    values
        .iter()
        .map(|placed| format!("{}{PART_SEPARATOR}{}", placed.option, placed.value))
        .collect::<Vec<_>>()
        .join(&ENTRY_SEPARATOR.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct FamilyRow {
    pub id: String,
    pub slug: String,
    pub name: String,
    /// The options and their values, encoded; see [`Self::option_list`].
    pub options: String,
    pub variant_count: i64,
    pub dissolved: bool,
}

impl FamilyRow {
    /// What tells the variants apart, in the order shown.
    pub fn option_list(&self) -> Vec<FamilyOption> {
        self.options
            .split(ENTRY_SEPARATOR)
            .filter(|entry| !entry.is_empty())
            .map(|entry| {
                let mut parts = entry.split(PART_SEPARATOR);
                FamilyOption {
                    name: parts.next().unwrap_or_default().to_owned(),
                    values: parts.map(str::to_owned).collect(),
                }
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct FamilyVariantRow {
    pub family_id: String,
    pub product_id: String,
    /// Where the variant stands, encoded; see [`Self::values`].
    pub option_values: String,
    pub position: i64,
}

impl FamilyVariantRow {
    pub fn values(&self) -> Vec<OptionValue> {
        self.option_values
            .split(ENTRY_SEPARATOR)
            .filter_map(|entry| entry.split_once(PART_SEPARATOR))
            .map(|(option, value)| OptionValue::new(option, value))
            .collect()
    }
}

pub fn family_list_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(FAMILY_LIST_SUBSCRIPTION)
        .handler(refresh_on_family_created())
        .handler(refresh_on_family_renamed())
        .handler(refresh_on_family_options_defined())
        .handler(refresh_on_family_variant_placed())
        .handler(refresh_on_family_variant_removed())
        .handler(refresh_on_family_dissolved())
        .strict()
}

/// Every family, by name; the dissolved ones only when asked for.
pub async fn list_families(
    db: &SqlitePool,
    include_dissolved: bool,
) -> sqlx::Result<Vec<FamilyRow>> {
    let mut rows: Vec<FamilyRow> = sqlx::query_as(
        "SELECT id, slug, name, options, variant_count, dissolved FROM catalog_family
         WHERE ?1 OR dissolved = 0",
    )
    .bind(include_dissolved)
    .fetch_all(db)
    .await?;
    rows.sort_by_cached_key(|row| (timada_core::slug::sort_key(&row.name), row.id.clone()));
    Ok(rows)
}

pub async fn family_by_id(db: &SqlitePool, id: &str) -> sqlx::Result<Option<FamilyRow>> {
    sqlx::query_as(
        "SELECT id, slug, name, options, variant_count, dissolved FROM catalog_family WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(db)
    .await
}

/// The variants of a family, in the order they joined.
pub async fn family_variants(
    db: &SqlitePool,
    family_id: &str,
) -> sqlx::Result<Vec<FamilyVariantRow>> {
    sqlx::query_as(
        "SELECT family_id, product_id, option_values, position FROM catalog_family_variant
         WHERE family_id = ? ORDER BY position",
    )
    .bind(family_id)
    .fetch_all(db)
    .await
}

/// The family a product is a variant of, as the list knows it.
pub async fn family_of_product(
    db: &SqlitePool,
    product_id: &str,
) -> sqlx::Result<Option<FamilyRow>> {
    sqlx::query_as(
        "SELECT f.id, f.slug, f.name, f.options, f.variant_count, f.dissolved
         FROM catalog_family_variant v JOIN catalog_family f ON f.id = v.family_id
         WHERE v.product_id = ?",
    )
    .bind(product_id)
    .fetch_optional(db)
    .await
}

// ------------------------------------------------------- the product page

/// One option of a family as a product page offers it: `Couleur`, and what
/// choosing each colour leads to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantChoice {
    pub option: String,
    pub values: Vec<VariantChoiceValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantChoiceValue {
    pub value: String,
    /// The product this value leads to; `None` when no variant on sale has it.
    pub product_id: Option<String>,
    /// The value of the product being looked at.
    pub current: bool,
    /// Whether the product it leads to differs from the current one by this
    /// option only. `false`: that combination is not sold, the link goes to
    /// the closest one — and changes something else on the way.
    pub exact: bool,
}

/// What the page of `current` offers to reach its siblings. Only the variants
/// that pass `on_sale` are led to — the current product always is, a shopper
/// is on its page. Empty when the product has no place in the family, or the
/// family nothing to choose from.
pub fn variant_choices(
    family: &FamilyState,
    current: &str,
    on_sale: impl Fn(&str) -> bool,
) -> Vec<VariantChoice> {
    let Some(here) = family.variant(current) else {
        return Vec::new();
    };
    if family.dissolved {
        return Vec::new();
    }
    let reachable: Vec<_> = family
        .variants
        .iter()
        .filter(|variant| variant.product_id == current || on_sale(&variant.product_id))
        .collect();
    if reachable.len() < 2 {
        return Vec::new();
    }

    family
        .options
        .iter()
        .map(|option| {
            let values = option
                .values
                .iter()
                .map(|value| {
                    // The sibling standing on this value that shares the most
                    // with the current product; the first to have joined wins
                    // a tie.
                    let closest = reachable
                        .iter()
                        .filter(|variant| variant.value_of(&option.name) == Some(value))
                        .map(|variant| {
                            let shared = family
                                .options
                                .iter()
                                .filter(|other| other.name != option.name)
                                .filter(|other| {
                                    variant.value_of(&other.name) == here.value_of(&other.name)
                                })
                                .count();
                            (shared, variant)
                        })
                        .fold(None, |best: Option<(usize, _)>, candidate| match best {
                            Some(kept) if kept.0 >= candidate.0 => Some(kept),
                            _ => Some(candidate),
                        });
                    VariantChoiceValue {
                        value: value.clone(),
                        product_id: closest.map(|(_, variant)| variant.product_id.clone()),
                        current: here.value_of(&option.name) == Some(value),
                        exact: closest
                            .is_some_and(|(shared, _)| shared + 1 == family.options.len()),
                    }
                })
                .collect();
            VariantChoice {
                option: option.name.clone(),
                values,
            }
        })
        .collect()
}

// ------------------------------------------------------------ subscription

/// Rewrites the family's rows from its events: a redelivery changes nothing.
async fn refresh<E: Executor>(ctx: &Context<'_, E>, id: &str) -> anyhow::Result<()> {
    let db = ctx
        .get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))?;
    let Some(family) = Command(ctx.executor).load_family(id).await? else {
        anyhow::bail!("product family {id} has events but cannot be loaded");
    };
    let mut tx = db.begin().await?;
    sqlx::query(
        "INSERT INTO catalog_family (id, slug, name, options, variant_count, dissolved)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (id) DO UPDATE
         SET name = excluded.name, options = excluded.options,
             variant_count = excluded.variant_count, dissolved = excluded.dissolved",
    )
    .bind(&family.id)
    .bind(&family.slug)
    .bind(&family.name)
    .bind(encode_options(&family.options))
    .bind(family.variants.len() as i64)
    .bind(family.dissolved)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM catalog_family_variant WHERE family_id = ?")
        .bind(&family.id)
        .execute(&mut *tx)
        .await?;
    for (position, variant) in family.variants.iter().enumerate() {
        sqlx::query(
            "INSERT INTO catalog_family_variant (family_id, product_id, option_values, position)
             VALUES (?, ?, ?, ?)",
        )
        .bind(&family.id)
        .bind(&variant.product_id)
        .bind(encode_values(&variant.values))
        .bind(position as i64)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

#[evento::subscription]
async fn refresh_on_family_created<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<FamilyCreated>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_family_renamed<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<FamilyRenamed>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_family_options_defined<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<FamilyOptionsDefined>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_family_variant_placed<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<FamilyVariantPlaced>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_family_variant_removed<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<FamilyVariantRemoved>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_family_dissolved<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<FamilyDissolved>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}
