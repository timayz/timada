//! SQL list read model: one row per customer, for the admin's customer
//! listing and search. Fed by the `customer-list` subscription; the account
//! page itself is served by the [`crate::AddressBookView`] projection.

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;

use crate::aggregator::{
    BillingAddressSet, CompanyIdentified, CompanyIdentityRemoved, CustomerAccountOpened,
    CustomerEmailChanged, CustomerRegistered, CustomerRegisteredAsGuest, DeliveryAddressAdded,
    DeliveryAddressChanged, DeliveryAddressRemoved, PreferredDeliveryAddressChosen,
    VatNumberChecked,
};

/// Subscription key; the caller attaches the pool with `.data(pool)`.
pub const CUSTOMER_LIST_SUBSCRIPTION: &str = "customer-list";

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct CustomerListRow {
    pub customer_id: String,
    pub email: String,
    pub first_name: String,
    pub last_name: String,
    /// Unix seconds of `CustomerRegistered`.
    pub registered_at: i64,
    /// Ordered without an account, and has none so far.
    pub guest: bool,
}

/// Filters for [`list_customers`]; `q` matches email, first or last name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListCustomers {
    pub q: Option<String>,
    pub limit: u32,
    pub offset: u32,
}

impl Default for ListCustomers {
    fn default() -> Self {
        Self {
            q: None,
            limit: 50,
            offset: 0,
        }
    }
}

pub fn customer_list_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(CUSTOMER_LIST_SUBSCRIPTION)
        .handler(insert_on_customer_registered())
        .handler(update_on_customer_email_changed())
        .handler(flag_on_customer_registered_as_guest())
        .handler(unflag_on_customer_account_opened())
        .skip::<BillingAddressSet>()
        .skip::<DeliveryAddressAdded>()
        .skip::<DeliveryAddressChanged>()
        .skip::<DeliveryAddressRemoved>()
        .skip::<PreferredDeliveryAddressChosen>()
        .skip::<CompanyIdentified>()
        .skip::<CompanyIdentityRemoved>()
        .skip::<VatNumberChecked>()
        .strict()
}

/// Customers matching the filter, newest registration first (ids are
/// time-ordered ULIDs, which breaks same-second ties).
pub async fn list_customers(
    db: &SqlitePool,
    filter: &ListCustomers,
) -> sqlx::Result<Vec<CustomerListRow>> {
    sqlx::query_as(
        "SELECT customer_id, email, first_name, last_name, registered_at, guest
         FROM customer_list
         WHERE (?1 IS NULL OR email LIKE ?1 OR first_name LIKE ?1 OR last_name LIKE ?1)
         ORDER BY registered_at DESC, customer_id DESC
         LIMIT ?2 OFFSET ?3",
    )
    .bind(like_pattern(filter.q.as_deref()))
    .bind(filter.limit)
    .bind(filter.offset)
    .fetch_all(db)
    .await
}

/// The customers with the given ids, in no particular order; unknown ids are
/// simply absent.
pub async fn customers_by_ids(
    db: &SqlitePool,
    ids: &[String],
) -> sqlx::Result<Vec<CustomerListRow>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
        "SELECT customer_id, email, first_name, last_name, registered_at, guest
         FROM customer_list
         WHERE customer_id IN (",
    );
    let mut bound = query.separated(", ");
    for id in ids {
        bound.push_bind(id);
    }
    query.push(")");
    query.build_query_as().fetch_all(db).await
}

/// Number of customers matching `q` (all customers when `None`).
pub async fn count_customers(db: &SqlitePool, q: Option<&str>) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM customer_list
         WHERE (?1 IS NULL OR email LIKE ?1 OR first_name LIKE ?1 OR last_name LIKE ?1)",
    )
    .bind(like_pattern(q))
    .fetch_one(db)
    .await
}

fn like_pattern(q: Option<&str>) -> Option<String> {
    q.map(str::trim)
        .filter(|q| !q.is_empty())
        .map(|q| format!("%{q}%"))
}

fn pool<E: Executor>(ctx: &Context<'_, E>) -> anyhow::Result<SqlitePool> {
    ctx.get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))
}

#[evento::subscription]
async fn insert_on_customer_registered<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<CustomerRegistered>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT OR IGNORE INTO customer_list
            (customer_id, email, first_name, last_name, registered_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&event.aggregate_id)
    .bind(&event.data.email)
    .bind(&event.data.first_name)
    .bind(&event.data.last_name)
    .bind(event.timestamp as i64)
    .execute(&pool(ctx)?)
    .await?;
    Ok(())
}

#[evento::subscription]
async fn update_on_customer_email_changed<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<CustomerEmailChanged>,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE customer_list SET email = ? WHERE customer_id = ?")
        .bind(&event.data.email)
        .bind(&event.aggregate_id)
        .execute(&pool(ctx)?)
        .await?;
    Ok(())
}

/// Absolute, so a redelivery changes nothing; the row exists already, the
/// registration comes first in the stream.
async fn set_guest<E: Executor>(
    ctx: &Context<'_, E>,
    customer_id: &str,
    guest: bool,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE customer_list SET guest = ? WHERE customer_id = ?")
        .bind(guest)
        .bind(customer_id)
        .execute(&pool(ctx)?)
        .await?;
    Ok(())
}

#[evento::subscription]
async fn flag_on_customer_registered_as_guest<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<CustomerRegisteredAsGuest>,
) -> anyhow::Result<()> {
    set_guest(ctx, &event.aggregate_id, true).await
}

#[evento::subscription]
async fn unflag_on_customer_account_opened<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<CustomerAccountOpened>,
) -> anyhow::Result<()> {
    set_guest(ctx, &event.aggregate_id, false).await
}
