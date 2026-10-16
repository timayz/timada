//! Checkout offers: one unit of a product at a price below its own, while
//! the cart meets the offer's conditions. The tenant keeps them all in one
//! stream ([`offers_id`]), which a checkout reads whole each time it prices
//! a cart.

use evento::{Executor, Projection, ProjectionAggregate, metadata::Event};
use timada_catalog::{MAX_AMOUNT_CENTS, product_projection};
use ulid::Ulid;

use crate::aggregator::{OfferArchived, OfferCreated, OfferDeactivated, OfferReactivated};
use crate::error::DiscountError;
use crate::value_object::{MAX_OFFER_TITLE_LEN, MAX_OFFERS, normalize_currency, offers_id};

/// What a merchant gives to create an offer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CreateOffer {
    pub product_id: String,
    /// Required when the product sells variants, absent otherwise.
    pub variant_id: Option<String>,
    /// What the one unit costs: more than 0, below the product's price.
    pub price_cents: u64,
    /// The product's currency.
    pub currency: String,
    /// What the checkout says above it; blank is the product's name.
    pub title: Option<String>,
    /// Only while the cart holds this product too.
    pub requires_product_id: Option<String>,
    /// Only while the rest of the cart is worth this much at least.
    pub min_subtotal_cents: Option<u64>,
    /// Unix seconds; `None` is from now on.
    pub starts_at: Option<u64>,
    /// Unix seconds, excluded; `None` is no end.
    pub ends_at: Option<u64>,
}

/// One offer of the tenant.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Offer {
    pub id: String,
    pub product_id: String,
    pub variant_id: Option<String>,
    pub price_cents: u64,
    pub currency: String,
    pub title: Option<String>,
    pub requires_product_id: Option<String>,
    pub min_subtotal_cents: Option<u64>,
    /// Unix seconds; `None` is from the start.
    pub starts_at: Option<u64>,
    /// Unix seconds, excluded; `None` is no end.
    pub ends_at: Option<u64>,
    /// Deactivated offers are kept, but shown to no shopper.
    pub active: bool,
    pub created_by: String,
    /// Unix seconds of `OfferCreated`.
    pub created_at: u64,
}

/// One line of a cart, as an offer looks at it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OfferItem<'a> {
    pub product_id: &'a str,
    pub variant_id: Option<&'a str>,
    pub quantity: u32,
    /// The full price of one, in the cart's currency.
    pub unit_amount_cents: u64,
}

impl Offer {
    /// Whether shoppers are offered it at `now` (unix seconds): active, and
    /// inside its window.
    pub fn is_live_at(&self, now: u64) -> bool {
        self.active
            && self.starts_at.is_none_or(|start| start <= now)
            && self.ends_at.is_none_or(|end| now < end)
    }

    /// Whether it is for that product and variant.
    pub fn is_for(&self, product_id: &str, variant_id: Option<&str>) -> bool {
        self.product_id == product_id && self.variant_id.as_deref() == variant_id
    }

    /// Whether a cart of `items` (without the unit the offer prices) meets
    /// its conditions at `now`: something else is in it, the product it
    /// requires is, and it is worth the minimum.
    pub fn holds(&self, items: &[OfferItem<'_>], now: u64) -> bool {
        let held = || items.iter().filter(|item| item.quantity > 0);
        self.is_live_at(now)
            && held().next().is_some()
            && self
                .requires_product_id
                .as_deref()
                .is_none_or(|required| held().any(|item| item.product_id == required))
            && self.min_subtotal_cents.is_none_or(|min| {
                held()
                    .map(|item| u64::from(item.quantity) * item.unit_amount_cents)
                    .sum::<u64>()
                    >= min
            })
    }
}

#[evento::projection(id = id, name = "timada-discounts/OffersState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct OffersState {
    pub id: String,
    /// The offers not archived, in the order they were created.
    pub offers: Vec<Offer>,
}

impl OffersState {
    pub fn offer(&self, offer_id: &str) -> Option<&Offer> {
        self.offers.iter().find(|offer| offer.id == offer_id)
    }

    /// For each of `items`, the offer one of its units is priced at at
    /// `now`, if any: one for that item, in `currency`, cheaper than its
    /// full price, whose conditions the cart meets without that unit. The
    /// cheapest wins when several do.
    pub fn applied(
        &self,
        items: &[OfferItem<'_>],
        currency: &str,
        now: u64,
    ) -> Vec<Option<&Offer>> {
        items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let mut rest = items.to_vec();
                rest[index].quantity = rest[index].quantity.saturating_sub(1);
                self.offers
                    .iter()
                    .filter(|offer| {
                        item.quantity > 0
                            && offer.is_for(item.product_id, item.variant_id)
                            && offer.currency.eq_ignore_ascii_case(currency)
                            && offer.price_cents < item.unit_amount_cents
                            && offer.holds(&rest, now)
                    })
                    .min_by_key(|offer| offer.price_cents)
            })
            .collect()
    }

    /// The offers a cart of `items` in `currency` could take at `now`: those
    /// whose item is not in it yet and whose conditions it meets, the
    /// cheapest one for each item.
    pub fn applicable(&self, items: &[OfferItem<'_>], currency: &str, now: u64) -> Vec<&Offer> {
        let mut found: Vec<&Offer> = Vec::new();
        for offer in &self.offers {
            let in_cart = items
                .iter()
                .any(|item| item.quantity > 0 && offer.is_for(item.product_id, item.variant_id));
            if in_cart || !offer.currency.eq_ignore_ascii_case(currency) || !offer.holds(items, now)
            {
                continue;
            }
            match found
                .iter_mut()
                .find(|other| other.is_for(&offer.product_id, offer.variant_id.as_deref()))
            {
                Some(other) if other.price_cents > offer.price_cents => *other = offer,
                Some(_) => {}
                None => found.push(offer),
            }
        }
        found
    }
}

pub fn offers_projection<E: Executor>() -> Projection<E, OffersState> {
    Projection::new::<crate::aggregator::Offers>()
        .handler(on_offer_created())
        .handler(on_offer_deactivated())
        .handler(on_offer_reactivated())
        .handler(on_offer_archived())
        .strict()
}

#[evento::handler]
async fn on_offer_created(
    event: Event<OfferCreated>,
    state: &mut OffersState,
) -> anyhow::Result<()> {
    let data = &event.data;
    state.id = event.aggregate_id.to_owned();
    state.offers.push(Offer {
        id: data.offer_id.clone(),
        product_id: data.product_id.clone(),
        variant_id: data.variant_id.clone(),
        price_cents: data.price_cents,
        currency: data.currency.clone(),
        title: data.title.clone(),
        requires_product_id: data.requires_product_id.clone(),
        min_subtotal_cents: data.min_subtotal_cents,
        starts_at: data.starts_at,
        ends_at: data.ends_at,
        active: true,
        created_by: data.created_by.clone(),
        created_at: event.timestamp,
    });
    Ok(())
}

#[evento::handler]
async fn on_offer_deactivated(
    event: Event<OfferDeactivated>,
    state: &mut OffersState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    if let Some(offer) = state
        .offers
        .iter_mut()
        .find(|offer| offer.id == event.data.offer_id)
    {
        offer.active = false;
    }
    Ok(())
}

#[evento::handler]
async fn on_offer_reactivated(
    event: Event<OfferReactivated>,
    state: &mut OffersState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    if let Some(offer) = state
        .offers
        .iter_mut()
        .find(|offer| offer.id == event.data.offer_id)
    {
        offer.active = true;
    }
    Ok(())
}

#[evento::handler]
async fn on_offer_archived(
    event: Event<OfferArchived>,
    state: &mut OffersState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.offers.retain(|offer| offer.id != event.data.offer_id);
    Ok(())
}

impl<E: Executor> super::Command<E> {
    /// The offers stream (`None` before the first offer).
    pub async fn load_offers(&self) -> anyhow::Result<Option<OffersState>> {
        offers_projection().load(offers_id()).execute(&self.0).await
    }

    /// The tenant's offers not archived, in the order they were created.
    pub async fn offers(&self) -> anyhow::Result<Vec<Offer>> {
        Ok(self
            .load_offers()
            .await?
            .map(|state| state.offers)
            .unwrap_or_default())
    }

    /// The offers stream holding `offer_id`, or `OfferNotFound`.
    async fn require_offer(&self, offer_id: &str) -> Result<(OffersState, Offer), DiscountError> {
        let state = self
            .load_offers()
            .await?
            .ok_or(DiscountError::OfferNotFound)?;
        let offer = state
            .offer(offer_id)
            .cloned()
            .ok_or(DiscountError::OfferNotFound)?;
        Ok((state, offer))
    }
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Creates an active offer and returns its id. The product must exist
    /// (published or not), the variant be one it sells exactly when it sells
    /// variants, and the price be below what that costs now, in its
    /// currency.
    pub async fn create_offer(
        &self,
        actor: &str,
        cmd: CreateOffer,
        routing_key: Option<String>,
    ) -> Result<String, DiscountError> {
        let product = product_projection()
            .load(&cmd.product_id)
            .execute(&self.0)
            .await?
            .filter(|product| !product.is_archived())
            .ok_or(DiscountError::OfferProductNotFound)?;
        let variant_id = cmd
            .variant_id
            .map(|id| id.trim().to_owned())
            .filter(|id| !id.is_empty());
        let price = match (&variant_id, product.variants.is_empty()) {
            (None, true) => product.price.as_ref(),
            (Some(id), false) => product.variant(id).map(|variant| &variant.price),
            _ => None,
        }
        .ok_or(DiscountError::OfferVariant)?;
        let currency = normalize_currency(&cmd.currency)?;
        if currency != price.currency {
            return Err(DiscountError::OfferCurrency);
        }
        if cmd.price_cents == 0 || cmd.price_cents >= price.amount_cents {
            return Err(DiscountError::InvalidOfferPrice);
        }
        let title = cmd
            .title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .map(str::to_owned);
        if title.as_deref().is_some_and(|title| {
            title.chars().count() > MAX_OFFER_TITLE_LEN || title.chars().any(char::is_control)
        }) {
            return Err(DiscountError::InvalidOfferTitle);
        }
        let requires_product_id = cmd
            .requires_product_id
            .map(|id| id.trim().to_owned())
            .filter(|id| !id.is_empty());
        if let Some(required) = &requires_product_id
            && product_projection()
                .load(required)
                .execute(&self.0)
                .await?
                .filter(|product| !product.is_archived())
                .is_none()
        {
            return Err(DiscountError::OfferProductNotFound);
        }
        if cmd
            .min_subtotal_cents
            .is_some_and(|min| min == 0 || min > MAX_AMOUNT_CENTS)
        {
            return Err(DiscountError::InvalidMinSubtotal);
        }
        if let (Some(start), Some(end)) = (cmd.starts_at, cmd.ends_at)
            && end <= start
        {
            return Err(DiscountError::InvalidWindow);
        }

        let state = self.load_offers().await?;
        if state
            .as_ref()
            .is_some_and(|state| state.offers.len() >= MAX_OFFERS)
        {
            return Err(DiscountError::TooManyOffers);
        }
        let mut write = match &state {
            Some(state) => state.write()?,
            None => {
                let mut write = evento::append(offers_id());
                write.original_version(0);
                write
            }
        };
        let offer_id = Ulid::generate().to_string();
        write.routing_key_opt(routing_key).event(&OfferCreated {
            offer_id: offer_id.clone(),
            product_id: product.id.clone(),
            variant_id,
            price_cents: cmd.price_cents,
            currency,
            title,
            requires_product_id,
            min_subtotal_cents: cmd.min_subtotal_cents,
            starts_at: cmd.starts_at,
            ends_at: cmd.ends_at,
            created_by: actor.to_owned(),
        });
        write.commit(&self.0).await?;
        Ok(offer_id)
    }

    /// Stops offering it; already inactive is a no-op. A checkout already
    /// being paid keeps the price it was shown.
    pub async fn deactivate_offer(
        &self,
        actor: &str,
        offer_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), DiscountError> {
        let (state, offer) = self.require_offer(offer_id).await?;
        if !offer.active {
            return Ok(());
        }
        state
            .write()?
            .routing_key_opt(routing_key)
            .event(&OfferDeactivated {
                offer_id: offer_id.to_owned(),
                deactivated_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// Offers it again; already active is a no-op.
    pub async fn reactivate_offer(
        &self,
        actor: &str,
        offer_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), DiscountError> {
        let (state, offer) = self.require_offer(offer_id).await?;
        if offer.active {
            return Ok(());
        }
        state
            .write()?
            .routing_key_opt(routing_key)
            .event(&OfferReactivated {
                offer_id: offer_id.to_owned(),
                reactivated_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// Retires the offer for good.
    pub async fn archive_offer(
        &self,
        actor: &str,
        offer_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), DiscountError> {
        let (state, _) = self.require_offer(offer_id).await?;
        state
            .write()?
            .routing_key_opt(routing_key)
            .event(&OfferArchived {
                offer_id: offer_id.to_owned(),
                archived_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(id: &str, product_id: &str, price_cents: u64) -> Offer {
        Offer {
            id: id.to_owned(),
            product_id: product_id.to_owned(),
            price_cents,
            currency: "EUR".to_owned(),
            active: true,
            ..Default::default()
        }
    }

    fn item(product_id: &str, quantity: u32, unit_amount_cents: u64) -> OfferItem<'_> {
        OfferItem {
            product_id,
            variant_id: None,
            quantity,
            unit_amount_cents,
        }
    }

    fn state(offers: Vec<Offer>) -> OffersState {
        OffersState {
            id: offers_id(),
            offers,
            ..Default::default()
        }
    }

    #[test]
    fn one_unit_takes_the_offer_while_the_rest_of_the_cart_meets_it() {
        let tracker = Offer {
            requires_product_id: Some("bowl".into()),
            ..offer("o1", "tracker", 1200)
        };
        let offers = state(vec![tracker]);
        let cart = [item("bowl", 1, 2000), item("tracker", 2, 3000)];
        let applied = offers.applied(&cart, "EUR", 0);
        assert_eq!(applied[0], None);
        assert_eq!(applied[1].map(|offer| offer.id.as_str()), Some("o1"));
        // Without the bowl, no offer.
        let alone = [item("tracker", 2, 3000)];
        assert_eq!(offers.applied(&alone, "EUR", 0), vec![None]);
        // Another currency, no offer.
        assert_eq!(offers.applied(&cart, "USD", 0), vec![None, None]);
    }

    #[test]
    fn the_minimum_counts_the_cart_without_the_offered_unit() {
        let offers = state(vec![Offer {
            min_subtotal_cents: Some(5000),
            ..offer("o1", "tracker", 1200)
        }]);
        // 40.00 of bowls and the tracker: 40.00 without it.
        let short = [item("bowl", 2, 2000), item("tracker", 1, 3000)];
        assert_eq!(offers.applied(&short, "EUR", 0), vec![None, None]);
        let enough = [item("bowl", 3, 2000), item("tracker", 1, 3000)];
        assert!(offers.applied(&enough, "EUR", 0)[1].is_some());
        // A second tracker counts towards the minimum of the first.
        let two = [item("bowl", 1, 2000), item("tracker", 2, 3000)];
        assert!(offers.applied(&two, "EUR", 0)[1].is_some());
    }

    #[test]
    fn an_offer_needs_something_else_in_the_cart() {
        let offers = state(vec![offer("o1", "tracker", 1200)]);
        assert_eq!(
            offers.applied(&[item("tracker", 1, 3000)], "EUR", 0),
            vec![None]
        );
        assert!(offers.applicable(&[], "EUR", 0).is_empty());
    }

    #[test]
    fn the_window_and_the_status_are_respected() {
        let offers = state(vec![Offer {
            starts_at: Some(100),
            ends_at: Some(200),
            ..offer("o1", "tracker", 1200)
        }]);
        let cart = [item("bowl", 1, 2000), item("tracker", 1, 3000)];
        assert!(offers.applied(&cart, "EUR", 99)[1].is_none());
        assert!(offers.applied(&cart, "EUR", 100)[1].is_some());
        assert!(offers.applied(&cart, "EUR", 200)[1].is_none());
        let mut off = offers.offers[0].clone();
        off.active = false;
        assert!(state(vec![off]).applied(&cart, "EUR", 150)[1].is_none());
    }

    #[test]
    fn an_offer_never_costs_more_than_the_price() {
        let offers = state(vec![offer("o1", "tracker", 3000)]);
        let cart = [item("bowl", 1, 2000), item("tracker", 1, 3000)];
        assert!(offers.applied(&cart, "EUR", 0)[1].is_none());
    }

    #[test]
    fn the_cheapest_offer_is_suggested_for_an_item_not_in_the_cart() {
        let offers = state(vec![
            offer("o1", "tracker", 1500),
            offer("o2", "tracker", 1200),
            Offer {
                requires_product_id: Some("lead".into()),
                ..offer("o3", "collar", 500)
            },
        ]);
        let cart = [item("bowl", 1, 2000)];
        let ids: Vec<&str> = offers
            .applicable(&cart, "EUR", 0)
            .iter()
            .map(|offer| offer.id.as_str())
            .collect();
        assert_eq!(ids, vec!["o2"]);
        let with_tracker = [item("bowl", 1, 2000), item("tracker", 1, 3000)];
        assert!(offers.applicable(&with_tracker, "EUR", 0).is_empty());
        assert_eq!(
            offers.applied(&with_tracker, "EUR", 0)[1].map(|offer| offer.id.as_str()),
            Some("o2")
        );
    }
}
