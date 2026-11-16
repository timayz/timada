//! What the shop says of delivery and returns beside its offers: its
//! shipping methods (`timada-orders`' `ShippingMethods`) and its return
//! terms (`ReturnPolicy`), copied into `storefront_shipping_methods` and
//! `storefront_return_policy` by the catalog subscription.
//!
//! [`offer_terms`] reads them for [`product_ld`](crate::product_ld), which
//! writes them as `shippingDetails` and `hasMerchantReturnPolicy` on every
//! offer. The checkout does not read this copy: what a shopper pays for
//! delivery is the dashboard's to say.

use evento::{Executor, metadata::RawEvent, subscription::Context};
use timada_core::db::{Db, Json};
use timada_orders::aggregator::{ReturnPolicy, ShippingMethods};
pub use timada_orders::{ReturnTerms, ShippingMethod};
use timada_orders::{return_policy_projection, shipping_methods_projection};
use topcoat_core::context::Cx;
use topcoat_core::error::{Error, Result};

use crate::context::store;

/// What an offer says of delivery and returns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OfferTerms {
    /// The shop's shipping methods, in the merchant's order.
    pub shipping: Vec<ShippingMethod>,
    /// Its return terms, when it states some as data.
    pub returns: Option<ReturnTerms>,
}

/// The terms as the copy holds them.
pub async fn offer_terms_of(db: &Db) -> anyhow::Result<OfferTerms> {
    let methods: Option<(Json<Vec<ShippingMethod>>,)> = db
        .fetch_optional(
            "SELECT methods_json FROM storefront_shipping_methods WHERE id = 1",
            (),
        )
        .await?;
    let returns: Option<(i64, i64)> = db
        .fetch_optional(
            "SELECT days, free FROM storefront_return_policy WHERE id = 1",
            (),
        )
        .await?;
    Ok(OfferTerms {
        shipping: methods.map(|(json,)| json.0).unwrap_or_default(),
        returns: returns.map(|(days, free)| ReturnTerms {
            days: days.max(0) as u32,
            free: free != 0,
        }),
    })
}

/// What the shop's offers say of delivery and returns; see the module docs.
pub async fn offer_terms(cx: &Cx) -> Result<OfferTerms> {
    offer_terms_of(store(cx).db())
        .await
        .map_err(Error::from_anyhow)
}

/// Rewrites the one row from the stream.
#[evento::subscription_all]
pub(crate) async fn on_shipping_methods_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<ShippingMethods>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let methods = shipping_methods_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
        .map(|state| state.methods)
        .unwrap_or_default();
    db.execute(
        "INSERT INTO storefront_shipping_methods (id, methods_json) VALUES (1, ?1)
         ON CONFLICT (id) DO UPDATE SET methods_json = excluded.methods_json",
        (Json(methods).to_text()?,),
    )
    .await?;
    Ok(())
}

/// Rewrites, or drops, the one row from the stream.
#[evento::subscription_all]
pub(crate) async fn on_return_policy_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<ReturnPolicy>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let terms = return_policy_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
        .and_then(|state| state.terms);
    match terms {
        Some(terms) => {
            db.execute(
                "INSERT INTO storefront_return_policy (id, days, free) VALUES (1, ?1, ?2)
                 ON CONFLICT (id) DO UPDATE SET days = excluded.days, free = excluded.free",
                (i64::from(terms.days), i64::from(terms.free)),
            )
            .await?;
        }
        None => {
            db.execute("DELETE FROM storefront_return_policy", ())
                .await?;
        }
    }
    Ok(())
}
