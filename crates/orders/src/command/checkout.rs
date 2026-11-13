use std::future::Future;

use evento::{Executor, ProjectionAggregate, WriteError};

use timada_catalog::{DeliveryDays, Sellable};
use timada_discounts::{Offer, OfferItem, OffersState, offers_id, offers_projection};
use ulid::Ulid;

use super::PendingReturn;
use crate::aggregator::{
    BillingAddressGiven, BillingAddressSet, BuyerIdentified, CheckedOut, CheckoutClaimed,
    CheckoutRequested, Claimed, DiscountGranted, Discounted, LanguageGiven, LanguageSet,
    NewsletterOptInSet, NewsletterOptedIn, PaymentCancelled, PaymentRefused, PaymentRequested,
    PaymentReturnRefused, PaymentReturnRequested, PaymentReturned, Placed, Released,
    ShippingCharged, ShippingGranted, ShippingMethodChosen, TimeZoneGiven, TimeZoneSet,
};
use crate::error::OrderError;
use crate::value_object::{
    MAX_REASON_LEN, OrderLine, Payment, ShippingAddress, new_reference, normalize_billing,
    normalize_buyer_vat_number, normalize_email, normalize_note, normalize_shipping, order_id,
    order_reference_id,
};

use super::{
    CartItem, CartLine, CartState, CheckoutRequest, OrderDiscount, OrderShipping, OrderState,
};

/// How many fresh references are tried before giving up: with 40 bits each,
/// a second clash in a row means something else is wrong.
const REFERENCE_ATTEMPTS: usize = 5;
/// How many times a write to a cart is tried again when the shopper changed
/// it at the same moment.
const CART_WRITE_ATTEMPTS: usize = 3;

/// What a shopper gives at checkout.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Checkout {
    pub email: String,
    pub shipping: ShippingAddress,
    pub note: Option<String>,
    /// The total the shopper was shown; when given, a payment is only asked
    /// for at that total, so a price that moved in between is never charged
    /// unseen.
    pub expected_total_cents: Option<u64>,
    /// A business buyer's EU VAT number, for an invoice without VAT under
    /// reverse charge (checked against VIES when invoiced).
    pub vat_number: Option<String>,
    /// Where to bill, when not at `shipping`: what the invoice is addressed
    /// to.
    pub billing: Option<ShippingAddress>,
    /// The id of the shipping method picked, in a shop that has some: one
    /// that ships to `shipping`'s country, in the cart's currency.
    pub shipping_method: Option<String>,
    /// The language the shopper browses in, as the browser states it (a
    /// tag or a whole `Accept-Language` value); kept as its primary subtag,
    /// dropped when it is not one. The shop mails the shopper in it.
    pub language: Option<String>,
    /// The shopper ticked the box asking for the shop's news.
    pub newsletter: bool,
    /// The IANA time zone the shopper's browser gives; dropped when the
    /// zone database does not name it.
    pub time_zone: Option<String>,
}

/// The order a payment made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedOrder {
    pub order_id: String,
    pub reference: String,
}

impl From<&OrderState> for PlacedOrder {
    fn from(order: &OrderState) -> Self {
        Self {
            order_id: order.id.clone(),
            reference: order.reference.clone(),
        }
    }
}

/// A cart priced from the catalog as it stands: the lines an order would
/// hold, and the ones that cannot be sold any more.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Quote {
    /// Each with the kind of goods it is for VAT ([`OrderLine::tax_category`]).
    pub lines: Vec<OrderLine>,
    /// The offer each of `lines` is priced at, in the same order: `Some` on
    /// the line of the one unit an offer prices, which comes right before
    /// the line of the cart's other units at full price (if any).
    pub line_offers: Vec<Option<LineOffer>>,
    /// The offers the shopper may add: the ones whose conditions the cart
    /// meets and whose item it does not hold yet.
    pub offers: Vec<QuotedOffer>,
    /// The lines whose product is off sale, gone, or whose variant is.
    pub unavailable: Vec<CartLine>,
    /// The lines of [`Self::unavailable`] that are on sale, with less left
    /// than the cart wants.
    pub out_of_stock: Vec<CartLine>,
    /// `None` while no line is priced.
    pub currency: Option<String>,
    /// What the lines add up to.
    pub subtotal_cents: u64,
    /// The cart's discount code, when it takes something off.
    pub discount: Option<OrderDiscount>,
    /// Why the cart's discount code takes nothing off (one of
    /// [`timada_discounts::SHOPPER_CODES`]); the total is then full price.
    #[serde(deserialize_with = "crate::wire::shopper_code")]
    pub discount_refused: Option<crate::wire::ShopperCode>,
    /// What the goods cost: the subtotal, less the discount. Shipping, when
    /// the shop charges for it, comes on top (see [`Quote::shipping_for`]).
    pub total_cents: u64,
}

impl Quote {
    /// When the goods arrive, counted from the order ([`DeliveryDays::window`]).
    pub fn delivery_window(&self) -> Option<timada_catalog::DeliveryDays> {
        timada_catalog::DeliveryDays::window(self.lines.iter().map(|line| line.delivery_days))
    }

    pub fn item_count(&self) -> u32 {
        self.lines.iter().map(|line| line.quantity).sum()
    }

    /// What shipping these goods with `method` costs, or `None` when the
    /// method is priced in another currency than the goods.
    pub fn shipping_for(&self, method: &super::ShippingMethod) -> Option<u64> {
        (self.currency.as_deref() == Some(method.currency.as_str()))
            .then(|| method.price_for(self.total_cents))
    }
}

/// The offer one unit of a line is priced at.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LineOffer {
    pub offer_id: String,
    /// What one costs without the offer.
    pub full_unit_amount_cents: u64,
}

/// An offer a cart may take, priced from the catalog as it stands.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct QuotedOffer {
    pub offer_id: String,
    pub product_id: String,
    pub variant_id: Option<String>,
    /// The product's name.
    pub name: String,
    /// The variant's values as the shop shows them (`Sable / Large`).
    pub variant_label: Option<String>,
    /// What the merchant titled it, if anything.
    pub title: Option<String>,
    /// What one costs without the offer.
    pub unit_amount_cents: u64,
    /// What the one unit costs with it.
    pub offer_amount_cents: u64,
}

/// Where a quote reads the catalog from: a product as a cart prices it, what
/// is on the shelf of a thing, and the shop's delivery time.
///
/// Two sources exist. [`StoreCatalog`] reads the products' own streams, so
/// what it says is what a payment asked for now would charge: the payment
/// saga prices with it. A read model that holds the same (the catalog's
/// `catalog_products`, `catalog_variants` and `catalog_stock` tables, a
/// moment behind the store) prices the cart a shopper is shown, at no cost
/// to the store; a price that moved in between is caught by the payment,
/// which refuses what the shopper did not see (`price_changed`), and stock
/// that ran out by its exact check against the stock's stream. What the read
/// model cannot sell is asked of the store before the shopper is told
/// ([`Command::quote_from`](super::Command::quote_from)).
pub trait QuoteSource: Sync {
    /// The product, or `None` for one the shop has not.
    fn sellable(
        &self,
        product_id: &str,
    ) -> impl Future<Output = anyhow::Result<Option<Sellable>>> + Send;

    /// What the checkout of `cart_id` may take of a thing: `None` for one
    /// that is not counted.
    fn available(
        &self,
        cart_id: &str,
        product_id: &str,
        variant_id: Option<&str>,
    ) -> impl Future<Output = anyhow::Result<Option<u32>>> + Send;

    /// The shop's delivery time for a product that gives none.
    fn default_delivery_days(
        &self,
    ) -> impl Future<Output = anyhow::Result<Option<DeliveryDays>>> + Send;
}

/// The catalog as the store has it now, read through the orders gateway.
pub struct StoreCatalog<'a, E: Executor>(pub &'a super::Command<E>);

impl<E: Executor> QuoteSource for StoreCatalog<'_, E> {
    async fn sellable(&self, product_id: &str) -> anyhow::Result<Option<Sellable>> {
        Ok(self
            .0
            .load_product(product_id)
            .await?
            .map(|product| Sellable::of(&product)))
    }

    async fn available(
        &self,
        cart_id: &str,
        product_id: &str,
        variant_id: Option<&str>,
    ) -> anyhow::Result<Option<u32>> {
        let Some(stock) =
            timada_catalog::load_stock(&self.0.executor, &self.0.tenant, product_id, variant_id)
                .await?
        else {
            return Ok(None);
        };
        Ok(stock.available_for(Some(cart_id), timada_discounts::unix_now()))
    }

    async fn default_delivery_days(&self) -> anyhow::Result<Option<DeliveryDays>> {
        self.0.load_default_delivery_days().await
    }
}

/// Whether a shopper can buy `item` of `product` as asked: published,
/// priced, and a variant on sale named exactly when it sells variants. The
/// one set of rules for a cart's writes, the quote a shopper is shown and
/// the price a payment is asked at.
pub fn require_sellable(product: Option<&Sellable>, item: &CartItem) -> Result<(), OrderError> {
    let unavailable = || OrderError::Unavailable {
        product_id: item.product_id.clone(),
    };
    let product = product.ok_or_else(unavailable)?;
    if !product.published || !product.is_priced() {
        return Err(unavailable());
    }
    match (&item.variant_id, product.has_variants()) {
        (None, false) => Ok(()),
        (None, true) => Err(OrderError::VariantRequired),
        (Some(_), false) => Err(OrderError::VariantNotFound),
        (Some(variant_id), true) => product
            .variant(variant_id)
            .map(|_| ())
            .ok_or(OrderError::VariantNotFound),
    }
}

/// `OutOfStock` when the cart wants more of a thing than `available` says
/// is left of it; `None` is a thing that is not counted.
pub fn require_in_stock(
    product_id: &str,
    available: Option<u32>,
    quantity: u32,
) -> Result<(), OrderError> {
    match available {
        Some(available) if available < quantity => Err(OrderError::OutOfStock {
            product_id: product_id.to_owned(),
            available,
        }),
        _ => Ok(()),
    }
}

/// What `catalog` says of a line of the cart `cart_id`: the product to price
/// it from, or why the shopper cannot buy it as it stands (`Unavailable`,
/// `VariantRequired`, `VariantNotFound` or `OutOfStock`). The outer error is
/// a failure to read.
async fn line_terms(
    catalog: &impl QuoteSource,
    cart_id: &str,
    item: &CartItem,
) -> Result<Result<Sellable, OrderError>, OrderError> {
    let product = catalog.sellable(&item.product_id).await?;
    if let Err(refusal) = require_sellable(product.as_ref(), item) {
        return Ok(Err(refusal));
    }
    let available = catalog
        .available(cart_id, &item.product_id, item.variant_id.as_deref())
        .await?;
    if let Err(refusal) = require_in_stock(&item.product_id, available, item.quantity) {
        return Ok(Err(refusal));
    }
    Ok(Ok(product.expect("a sellable product is some")))
}

impl<E: Executor> super::Command<E> {
    /// Prices a cart from the catalog as it stands now, reading every product
    /// from the store (never from a replica), so what it says is what a
    /// payment asked for now would charge.
    pub async fn quote_cart(&self, cart_id: &str) -> Result<Quote, OrderError> {
        let cart = self.require_cart(cart_id).await?;
        self.quote(&cart).await
    }

    /// The exact quote: the catalog as the store has it now.
    pub(crate) async fn quote(&self, cart: &CartState) -> Result<Quote, OrderError> {
        self.price(cart, &StoreCatalog(self), None::<&StoreCatalog<'_, E>>)
            .await
    }

    /// The quote a shopper is shown: priced from `catalog`, the catalog's
    /// read model, a moment behind the store and at no cost to it. A line it
    /// cannot sell is looked at again in `store`, the product's and the
    /// stock's own streams, before the quote says so: the cart took the line
    /// after reading those, and the read model may not have the product put
    /// on sale, or the stock put back, a moment before. Only such a line is
    /// read from the store; one the read model sells is priced as it has it,
    /// a price that moved in between being the payment's to refuse.
    pub async fn quote_from(
        &self,
        cart: &CartState,
        catalog: &impl QuoteSource,
        store: &impl QuoteSource,
    ) -> Result<Quote, OrderError> {
        self.price(cart, catalog, Some(store)).await
    }

    /// Prices the cart's lines at full price, then lets each live offer
    /// price one unit of its item, when the rest of the cart meets its
    /// conditions: that unit becomes a line of its own, so the order, its
    /// invoice and its mail say what it cost. The discount code comes off
    /// what that adds up to. The products come from `catalog`, a line it
    /// cannot sell from `store` when one is given; an offer's product from
    /// `catalog` alone, since one missing there hides the offer for a moment
    /// and stops no checkout. The offers and the code's discount come from
    /// their own streams whatever the source, one small aggregate per shop
    /// and per code, which the discount read model (a listing) holds no
    /// window or cap of.
    async fn price(
        &self,
        cart: &CartState,
        catalog: &impl QuoteSource,
        store: Option<&impl QuoteSource>,
    ) -> Result<Quote, OrderError> {
        let mut quote = Quote::default();
        let mut priced: Vec<OrderLine> = Vec::new();
        // Read once, the first time a product gives no time of its own.
        let mut default_days = None;
        for line in &cart.lines {
            let item = CartItem {
                product_id: line.product_id.clone(),
                variant_id: line.variant_id.clone(),
                quantity: line.quantity,
            };
            let mut terms = line_terms(catalog, &cart.id, &item).await?;
            if terms.is_err()
                && let Some(store) = store
            {
                terms = line_terms(store, &cart.id, &item).await?;
            }
            let product = match terms {
                Ok(product) => product,
                Err(
                    OrderError::Unavailable { .. }
                    | OrderError::VariantRequired
                    | OrderError::VariantNotFound,
                ) => {
                    quote.unavailable.push(line.clone());
                    continue;
                }
                Err(OrderError::OutOfStock { .. }) => {
                    quote.unavailable.push(line.clone());
                    quote.out_of_stock.push(line.clone());
                    continue;
                }
                Err(err) => return Err(err),
            };
            let variant = line
                .variant_id
                .as_deref()
                .and_then(|id| product.variant(id));
            let price = match variant {
                Some(variant) => &variant.price,
                // `require_sellable` let a line without a variant through
                // only for a product priced on its own.
                None => product.price.as_ref().ok_or(OrderError::Unavailable {
                    product_id: product.id.clone(),
                })?,
            };
            match &quote.currency {
                Some(currency) if *currency != price.currency => {
                    return Err(OrderError::MixedCurrencies);
                }
                Some(_) => {}
                None => quote.currency = Some(price.currency.clone()),
            }
            priced.push(OrderLine {
                product_id: product.id.clone(),
                variant_id: line.variant_id.clone(),
                name: product.name.clone(),
                variant_label: variant.and_then(|variant| variant.label.clone()),
                sku: variant.and_then(|variant| variant.sku.clone()),
                unit_amount_cents: price.amount_cents,
                quantity: line.quantity,
                tax_category: product.tax_category.clone(),
                delivery_days: match product.delivery_days {
                    Some(days) => Some(days),
                    None => match default_days {
                        Some(days) => days,
                        None => *default_days.insert(catalog.default_delivery_days().await?),
                    },
                },
            });
        }
        self.price_offers(&mut quote, priced, catalog).await?;
        quote.total_cents = quote.subtotal_cents;
        if let (Some(code), Some(currency)) = (&cart.discount_code, &quote.currency) {
            match self
                .price_discount(code, quote.subtotal_cents, currency)
                .await
            {
                Ok(discount) => {
                    quote.total_cents -= discount.amount_cents;
                    quote.discount = Some(discount);
                }
                Err(OrderError::DiscountRefused { code }) => quote.discount_refused = Some(code),
                Err(err) => return Err(err),
            }
        }
        Ok(quote)
    }

    /// What the discount a code names takes off `subtotal_cents` now.
    async fn price_discount(
        &self,
        code: &str,
        subtotal_cents: u64,
        currency: &str,
    ) -> Result<OrderDiscount, OrderError> {
        let discount =
            self.load_discount_by_code(code)
                .await?
                .ok_or(OrderError::DiscountRefused {
                    code: timada_discounts::DISCOUNT_UNKNOWN,
                })?;
        let amount_cents = discount
            .amount_off(subtotal_cents, currency, timada_discounts::unix_now())
            .map_err(OrderError::from_discount)?;
        Ok(OrderDiscount {
            discount_id: discount.id,
            code: discount.code,
            subtotal_cents,
            amount_cents,
        })
    }

    /// Puts the lines priced at full price on the quote, each one an offer
    /// prices split in two (its one unit at the offer's price, the others at
    /// full price), adds them up, and lists the offers the cart may take.
    async fn price_offers(
        &self,
        quote: &mut Quote,
        priced: Vec<OrderLine>,
        catalog: &impl QuoteSource,
    ) -> Result<(), OrderError> {
        let offers = match &quote.currency {
            Some(_) => offers_projection()
                .load(offers_id())
                .tenant(&self.tenant)
                .execute(&self.executor)
                .await?
                .unwrap_or_default(),
            None => OffersState::default(),
        };
        let currency = quote.currency.clone().unwrap_or_default();
        let now = timada_discounts::unix_now();
        let items: Vec<OfferItem<'_>> = priced
            .iter()
            .map(|line| OfferItem {
                product_id: &line.product_id,
                variant_id: line.variant_id.as_deref(),
                quantity: line.quantity,
                unit_amount_cents: line.unit_amount_cents,
            })
            .collect();
        let applied: Vec<Option<LineOffer>> = offers
            .applied(&items, &currency, now)
            .into_iter()
            .zip(&priced)
            .map(|(offer, line)| {
                offer.map(|offer| LineOffer {
                    offer_id: offer.id.clone(),
                    full_unit_amount_cents: line.unit_amount_cents,
                })
            })
            .collect();
        let applicable: Vec<Offer> = offers
            .applicable(&items, &currency, now)
            .into_iter()
            .cloned()
            .collect();
        drop(items);

        for (line, line_offer) in priced.into_iter().zip(applied) {
            let Some(line_offer) = line_offer else {
                quote.lines.push(line);
                quote.line_offers.push(None);
                continue;
            };
            let offer_price = offers
                .offer(&line_offer.offer_id)
                .map_or(line.unit_amount_cents, |offer| offer.price_cents);
            quote.lines.push(OrderLine {
                unit_amount_cents: offer_price,
                quantity: 1,
                ..line.clone()
            });
            quote.line_offers.push(Some(line_offer));
            if line.quantity > 1 {
                quote.lines.push(OrderLine {
                    quantity: line.quantity - 1,
                    ..line
                });
                quote.line_offers.push(None);
            }
        }
        quote.subtotal_cents = quote.lines.iter().map(OrderLine::amount_cents).sum();

        for offer in applicable {
            let item = CartItem {
                product_id: offer.product_id.clone(),
                variant_id: offer.variant_id.clone(),
                quantity: 1,
            };
            let product = catalog.sellable(&offer.product_id).await?;
            let product = match require_sellable(product.as_ref(), &item) {
                Ok(()) => product.expect("a sellable product is some"),
                Err(
                    OrderError::Unavailable { .. }
                    | OrderError::VariantRequired
                    | OrderError::VariantNotFound,
                ) => continue,
                Err(err) => return Err(err),
            };
            let variant = offer
                .variant_id
                .as_deref()
                .and_then(|id| product.variant(id));
            let Some(price) = variant
                .map(|variant| &variant.price)
                .or(product.price.as_ref())
            else {
                continue;
            };
            if price.currency != currency || offer.price_cents >= price.amount_cents {
                continue;
            }
            quote.offers.push(QuotedOffer {
                offer_id: offer.id,
                product_id: product.id.clone(),
                variant_id: offer.variant_id,
                name: product.name.clone(),
                variant_label: variant.and_then(|variant| variant.label.clone()),
                title: offer.title,
                unit_amount_cents: price.amount_cents,
                offer_amount_cents: offer.price_cents,
            });
        }
        Ok(())
    }
}

/// A payment opened with the provider, as the dashboard records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedPayment {
    pub provider: String,
    pub payment_id: String,
    pub client_secret: String,
    pub publishable_key: String,
}

/// A request to pay waiting for its answer, with the cart priced for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingPayment {
    pub request: CheckoutRequest,
    pub lines: Vec<OrderLine>,
    pub currency: String,
    /// What to charge: the lines, less the discount, plus shipping.
    pub total_cents: u64,
    /// The discount to charge it with, whose use the dashboard reserves
    /// before it opens the payment.
    pub discount: Option<OrderDiscount>,
    /// The shipping to charge it with; `None` in a shop without methods.
    pub shipping: Option<OrderShipping>,
    /// Who supplies some of the goods once the order is placed: the money
    /// is then taken once they are bought from it. The orders know no
    /// supplier: the dashboard says, before it records the payment.
    pub supplier: Option<String>,
}

/// What the checkout of a request holds, or may hold: the goods (one hold
/// per thing, for the cart) and a use of a discount (for the cart and the
/// request's address). What goes back when the request is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutHolds {
    pub stock_lines: Vec<timada_catalog::StockLine>,
    pub discount_id: Option<String>,
    pub email: String,
}

impl From<&PendingPayment> for CheckoutHolds {
    /// What a request priced as `pending` holds.
    fn from(pending: &PendingPayment) -> Self {
        Self {
            stock_lines: crate::stock_lines(&pending.lines),
            discount_id: pending
                .discount
                .as_ref()
                .map(|discount| discount.discount_id.clone()),
            email: pending.request.email.clone(),
        }
    }
}

impl<E: Executor> super::Command<E> {
    /// What the dashboard does with a request to pay: `None` when there is
    /// nothing to do (answered already, the cart ordered, the request
    /// unknown), the cart priced for it otherwise, or the refusal to record
    /// (the cart empty or off sale, the total not the one the shopper saw).
    pub async fn pending_payment(
        &self,
        cart_id: &str,
        request_id: &str,
    ) -> Result<Option<PendingPayment>, OrderError> {
        let cart = self.require_cart(cart_id).await?;
        let Some(request) = cart.request(request_id) else {
            return Ok(None);
        };
        if request.answer.is_some() || cart.is_checked_out() {
            return Ok(None);
        }
        if cart.lines.is_empty() {
            return Err(OrderError::EmptyCart);
        }
        let quote = self.quote(&cart).await?;
        if let Some(line) = quote.out_of_stock.first() {
            return Err(OrderError::OutOfStock {
                product_id: line.product_id.clone(),
                available: 0,
            });
        }
        if let Some(line) = quote.unavailable.first() {
            return Err(OrderError::Unavailable {
                product_id: line.product_id.clone(),
            });
        }
        // A code the shopper counted on is never dropped silently.
        if let Some(code) = quote.discount_refused {
            return Err(OrderError::DiscountRefused { code });
        }
        let shipping = self.price_shipping(&quote, request).await?;
        let total_cents = quote.total_cents
            + shipping
                .as_ref()
                .map_or(0, |shipping| shipping.amount_cents);
        if let Some(expected) = request.expected_total_cents
            && expected != total_cents
        {
            return Err(OrderError::PriceChanged { total_cents });
        }
        let currency = quote.currency.ok_or(OrderError::EmptyCart)?;
        Ok(Some(PendingPayment {
            request: request.clone(),
            lines: quote.lines,
            currency,
            total_cents,
            discount: quote.discount,
            shipping,
            supplier: None,
        }))
    }

    /// The shipping a request is charged: `None` in a shop without methods;
    /// `ShippingUnavailable` when the method picked is gone, priced in
    /// another currency or does not ship to the request's country (or none
    /// was picked).
    async fn price_shipping(
        &self,
        quote: &Quote,
        request: &CheckoutRequest,
    ) -> Result<Option<OrderShipping>, OrderError> {
        let methods = self.shipping_methods().await?;
        if methods.is_empty() {
            return Ok(None);
        }
        let method = request
            .shipping_method
            .as_deref()
            .and_then(|id| methods.iter().find(|method| method.id == id))
            .filter(|method| method.covers(&request.shipping.country))
            .ok_or(OrderError::ShippingUnavailable)?;
        let amount_cents = quote
            .shipping_for(method)
            .ok_or(OrderError::ShippingUnavailable)?;
        Ok(Some(OrderShipping {
            method_id: method.id.clone(),
            name: method.name.clone(),
            amount_cents,
        }))
    }

    /// What the checkout of a request may hold, as its cart stands: a hold
    /// on each thing its lines name, and a use of the discount its code
    /// names, for the request's address. For a request refused before it
    /// was priced (its claimant stopped): what its checkout took, unless the
    /// shopper changed the cart since; a hold left over lapses on its own.
    pub async fn checkout_holds(
        &self,
        cart_id: &str,
        request_id: &str,
    ) -> Result<Option<CheckoutHolds>, OrderError> {
        let Some(cart) = self.load_cart(cart_id).await? else {
            return Ok(None);
        };
        let Some(request) = cart.request(request_id) else {
            return Ok(None);
        };
        let discount_id = match &cart.discount_code {
            Some(code) => self
                .load_discount_by_code(code)
                .await?
                .map(|discount| discount.id),
            None => None,
        };
        Ok(Some(CheckoutHolds {
            stock_lines: cart
                .lines
                .iter()
                .map(|line| timada_catalog::StockLine {
                    product_id: line.product_id.clone(),
                    variant_id: line.variant_id.clone(),
                    quantity: line.quantity,
                })
                .collect(),
            discount_id,
            email: request.email.clone(),
        }))
    }

    /// The order a reference names, if any. A reference claimed for an order
    /// placed under another one names nothing.
    pub async fn load_order_by_reference(
        &self,
        reference: &str,
    ) -> anyhow::Result<Option<OrderState>> {
        let Some(index) = super::order_reference_projection()
            .load(order_reference_id(reference))
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await?
            .filter(|index| !index.released)
        else {
            return Ok(None);
        };
        Ok(self
            .load_order(&index.order_id)
            .await?
            .filter(|order| order.reference == reference))
    }
}

/// Who answers a request to pay, as [`Command::claim_checkout`](super::Command::claim_checkout) says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckoutClaim {
    /// The claimant answers it: it took it now, or holds it already (told
    /// the request again, it goes on).
    Taken,
    /// Another node took it on and its claim holds: left to it.
    Held,
    /// Nothing to answer: answered already, the cart ordered, or no such
    /// request.
    Answered,
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// The shopper asks to pay for the cart as it stands; returns the id the
    /// answer will carry. Nothing is priced or charged here: the dashboard
    /// answers with a payment to confirm, or a refusal.
    pub async fn request_checkout(
        &self,
        actor: &str,
        cart_id: &str,
        checkout: Checkout,
        routing_key: Option<String>,
    ) -> Result<String, OrderError> {
        let email = normalize_email(&checkout.email)?;
        let shipping = normalize_shipping(checkout.shipping)?;
        // The courier, and a supplier that ships for the shop, need to reach
        // the shopper.
        if shipping.phone.is_none() {
            return Err(OrderError::MissingPhone);
        }
        let billing = checkout.billing.map(normalize_billing).transpose()?;
        let note = normalize_note(checkout.note.as_deref())?;
        let vat_number = normalize_buyer_vat_number(checkout.vat_number.as_deref())?;
        let shipping_method = checkout
            .shipping_method
            .map(|id| id.trim().to_owned())
            .filter(|id| !id.is_empty());
        let language = checkout
            .language
            .as_deref()
            .and_then(timada_core::primary_language);
        if vat_number.is_some() && !self.accepts_business_buyers().await? {
            return Err(OrderError::BusinessBuyersRefused);
        }
        let cart = self.require_open_cart(cart_id).await?;
        if cart.lines.is_empty() {
            return Err(OrderError::EmptyCart);
        }
        let request_id = Ulid::generate().to_string();
        let mut write = cart.write()?;
        write
            .routing_key_opt(routing_key)
            .event(&CheckoutRequested {
                request_id: request_id.clone(),
                email,
                shipping,
                note,
                expected_total_cents: checkout.expected_total_cents,
                requested_by: actor.to_owned(),
            });
        if let Some(vat_number) = vat_number {
            write.event(&BuyerIdentified {
                request_id: request_id.clone(),
                vat_number,
                identified_by: actor.to_owned(),
            });
        }
        if let Some(billing) = billing {
            write.event(&BillingAddressGiven {
                request_id: request_id.clone(),
                billing,
                given_by: actor.to_owned(),
            });
        }
        if let Some(method_id) = shipping_method {
            write.event(&ShippingMethodChosen {
                request_id: request_id.clone(),
                method_id,
                chosen_by: actor.to_owned(),
            });
        }
        if let Some(language) = language {
            write.event(&LanguageGiven {
                request_id: request_id.clone(),
                language,
                given_by: actor.to_owned(),
            });
        }
        if checkout.newsletter {
            write.event(&NewsletterOptedIn {
                request_id: request_id.clone(),
                opted_in_by: actor.to_owned(),
            });
        }
        if let Some(time_zone) = checkout
            .time_zone
            .as_deref()
            .and_then(timada_core::time_zone)
        {
            write.event(&TimeZoneGiven {
                request_id: request_id.clone(),
                time_zone,
                given_by: actor.to_owned(),
            });
        }
        write.tenant(&self.tenant).commit(&self.executor).await?;
        Ok(request_id)
    }

    /// Takes the request to pay on for `claimant` (a node), so one node
    /// holds the goods and the code, opens the payment and answers: written
    /// at the version read, so of two nodes claiming at once one is
    /// refused, reads again and finds the other's claim. While a claim
    /// holds ([`CHECKOUT_CLAIM_SECS`](crate::CHECKOUT_CLAIM_SECS)) its
    /// claimant goes on with the request (told it again after an error or a
    /// restart) and every other node leaves it alone; a lapsed claim is
    /// taken again the same way, its claimant's own included.
    pub async fn claim_checkout(
        &self,
        actor: &str,
        cart_id: &str,
        request_id: &str,
        claimant: &str,
        routing_key: Option<String>,
    ) -> Result<CheckoutClaim, OrderError> {
        for attempt in 1..=CART_WRITE_ATTEMPTS {
            let Some(cart) = self.load_cart(cart_id).await? else {
                return Ok(CheckoutClaim::Answered);
            };
            let Some(request) = cart.request(request_id) else {
                return Ok(CheckoutClaim::Answered);
            };
            if request.answer.is_some() || cart.is_checked_out() {
                return Ok(CheckoutClaim::Answered);
            }
            if let Some(claim) = &request.claim
                && claim.holds_at(timada_discounts::unix_now())
            {
                return Ok(if claim.claimant == claimant {
                    CheckoutClaim::Taken
                } else {
                    CheckoutClaim::Held
                });
            }
            let written = cart
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&CheckoutClaimed {
                    request_id: request_id.to_owned(),
                    claimant: claimant.to_owned(),
                    claimed_by: actor.to_owned(),
                })
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await;
            match written {
                Ok(_) => return Ok(CheckoutClaim::Taken),
                Err(WriteError::InvalidOriginalVersion) if attempt < CART_WRITE_ATTEMPTS => {}
                Err(err) => return Err(err.into()),
            }
        }
        Ok(CheckoutClaim::Held)
    }

    /// Records the payment opened for a request, priced as `pending` says.
    /// A request answered already (by another node, say) is left alone.
    pub async fn record_payment_request(
        &self,
        actor: &str,
        cart_id: &str,
        pending: PendingPayment,
        opened: OpenedPayment,
        routing_key: Option<String>,
    ) -> Result<(), OrderError> {
        let request_id = pending.request.request_id.clone();
        for attempt in 1..=CART_WRITE_ATTEMPTS {
            let cart = self.require_cart(cart_id).await?;
            if cart.request(&request_id).is_none_or(|r| r.answer.is_some()) {
                return Ok(());
            }
            let mut write = cart.write()?;
            write
                .routing_key_opt(routing_key.clone())
                .event(&PaymentRequested {
                    request_id: request_id.clone(),
                    provider: opened.provider.clone(),
                    payment_id: opened.payment_id.clone(),
                    client_secret: opened.client_secret.clone(),
                    publishable_key: opened.publishable_key.clone(),
                    lines: pending.lines.clone(),
                    currency: pending.currency.clone(),
                    total_cents: pending.total_cents,
                    supplier: pending.supplier.clone(),
                    requested_by: actor.to_owned(),
                });
            if let Some(discount) = &pending.discount {
                write.event(&DiscountGranted {
                    request_id: request_id.clone(),
                    discount_id: discount.discount_id.clone(),
                    code: discount.code.clone(),
                    subtotal_cents: discount.subtotal_cents,
                    amount_cents: discount.amount_cents,
                    granted_by: actor.to_owned(),
                });
            }
            if let Some(shipping) = &pending.shipping {
                write.event(&ShippingGranted {
                    request_id: request_id.clone(),
                    method_id: shipping.method_id.clone(),
                    name: shipping.name.clone(),
                    amount_cents: shipping.amount_cents,
                    granted_by: actor.to_owned(),
                });
            }
            let written = write.tenant(&self.tenant).commit(&self.executor).await;
            match written {
                Ok(_) => return Ok(()),
                Err(WriteError::InvalidOriginalVersion) if attempt < CART_WRITE_ATTEMPTS => {}
                Err(err) => return Err(err.into()),
            }
        }
        Ok(())
    }

    /// Records why a request to pay cannot go ahead; `code` is what the
    /// storefront words. A request answered already is left alone: whether
    /// this refusal is its answer.
    pub async fn refuse_payment_request(
        &self,
        actor: &str,
        cart_id: &str,
        request_id: &str,
        code: &str,
        routing_key: Option<String>,
    ) -> Result<bool, OrderError> {
        for attempt in 1..=CART_WRITE_ATTEMPTS {
            let cart = self.require_cart(cart_id).await?;
            if cart.request(request_id).is_none_or(|r| r.answer.is_some()) {
                return Ok(false);
            }
            let written = cart
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&PaymentRefused {
                    request_id: request_id.to_owned(),
                    code: code.to_owned(),
                    refused_by: actor.to_owned(),
                })
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await;
            match written {
                Ok(_) => return Ok(true),
                Err(WriteError::InvalidOriginalVersion) if attempt < CART_WRITE_ATTEMPTS => {}
                Err(err) => return Err(err.into()),
            }
        }
        Ok(false)
    }

    /// Asks for the payment `payment_id` of the cart to go back to the
    /// shopper, before the provider is asked: `code` says why
    /// (`Cart::PaymentReturnRequested`). Asked again while it stands, the
    /// return asked first comes back, with its own code; `None` once the
    /// return is settled (the payment went back, or the provider would not
    /// give it back). A payment the cart never opened, or the one its order
    /// was placed with, is `PaymentNotFound`.
    pub async fn request_payment_return(
        &self,
        actor: &str,
        cart_id: &str,
        payment_id: &str,
        code: &str,
        routing_key: Option<String>,
    ) -> Result<Option<PendingReturn>, OrderError> {
        for attempt in 1..=CART_WRITE_ATTEMPTS {
            let cart = self.require_cart(cart_id).await?;
            if cart.returned_payment(payment_id).is_some()
                || cart.return_refusal(payment_id).is_some()
            {
                return Ok(None);
            }
            if let Some(pending) = cart.pending_return(payment_id) {
                return Ok(Some(pending.clone()));
            }
            let request = cart
                .request_paid_by(payment_id)
                .ok_or(OrderError::PaymentNotFound)?;
            // The payment of the order goes back with the order, never so.
            let ordered = self
                .load_order(&order_id(cart_id))
                .await?
                .is_some_and(|order| order.payment.payment_id == payment_id);
            if ordered {
                return Err(OrderError::PaymentNotFound);
            }
            let written = cart
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&PaymentReturnRequested {
                    request_id: request.request_id.clone(),
                    payment_id: payment_id.to_owned(),
                    code: code.to_owned(),
                    requested_by: actor.to_owned(),
                })
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await;
            match written {
                Ok(_) => {
                    let cart = self.require_cart(cart_id).await?;
                    return Ok(cart.pending_return(payment_id).cloned());
                }
                Err(WriteError::InvalidOriginalVersion) if attempt < CART_WRITE_ATTEMPTS => {}
                Err(err) => return Err(err.into()),
            }
        }
        Ok(None)
    }

    /// Records that the payment `payment_id` of the cart went back to the
    /// shopper as [`request_payment_return`](Self::request_payment_return)
    /// asked, under the code it was asked with: `refund_id` when the money
    /// had been taken and was refunded. Told twice, or once the return is
    /// settled otherwise, nothing more happens; a payment nobody asked to
    /// give back is `PaymentNotFound`.
    pub async fn return_payment(
        &self,
        actor: &str,
        cart_id: &str,
        payment_id: &str,
        refund_id: Option<String>,
        routing_key: Option<String>,
    ) -> Result<(), OrderError> {
        for attempt in 1..=CART_WRITE_ATTEMPTS {
            let cart = self.require_cart(cart_id).await?;
            if cart.returned_payment(payment_id).is_some()
                || cart.return_refusal(payment_id).is_some()
            {
                return Ok(());
            }
            let (request_id, code) = cart
                .requests
                .iter()
                .find_map(|request| {
                    request
                        .returning
                        .as_ref()
                        .filter(|pending| pending.payment_id == payment_id)
                        .map(|pending| (request.request_id.clone(), pending.code.clone()))
                })
                .ok_or(OrderError::PaymentNotFound)?;
            let written = cart
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&PaymentReturned {
                    request_id,
                    payment_id: payment_id.to_owned(),
                    code,
                    refund_id: refund_id.clone(),
                    returned_by: actor.to_owned(),
                })
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await;
            match written {
                Ok(_) => return Ok(()),
                Err(WriteError::InvalidOriginalVersion) if attempt < CART_WRITE_ATTEMPTS => {}
                Err(err) => return Err(err.into()),
            }
        }
        Ok(())
    }

    /// Records that the provider will not give back the payment `payment_id`
    /// of the cart, for good, as `reason` says: the return
    /// [`request_payment_return`](Self::request_payment_return) asked for is
    /// settled, and nothing more is tried. Told twice, or once the payment
    /// went back, nothing more happens; a payment nobody asked to give back
    /// is `PaymentNotFound`.
    pub async fn refuse_payment_return(
        &self,
        actor: &str,
        cart_id: &str,
        payment_id: &str,
        reason: &str,
        routing_key: Option<String>,
    ) -> Result<(), OrderError> {
        let reason: String = reason.trim().chars().take(MAX_REASON_LEN).collect();
        for attempt in 1..=CART_WRITE_ATTEMPTS {
            let cart = self.require_cart(cart_id).await?;
            if cart.returned_payment(payment_id).is_some()
                || cart.return_refusal(payment_id).is_some()
            {
                return Ok(());
            }
            let request_id = cart
                .requests
                .iter()
                .find(|request| {
                    request
                        .returning
                        .as_ref()
                        .is_some_and(|pending| pending.payment_id == payment_id)
                })
                .map(|request| request.request_id.clone())
                .ok_or(OrderError::PaymentNotFound)?;
            let written = cart
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&PaymentReturnRefused {
                    request_id,
                    payment_id: payment_id.to_owned(),
                    reason: reason.clone(),
                    refused_by: actor.to_owned(),
                })
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await;
            match written {
                Ok(_) => return Ok(()),
                Err(WriteError::InvalidOriginalVersion) if attempt < CART_WRITE_ATTEMPTS => {}
                Err(err) => return Err(err.into()),
            }
        }
        Ok(())
    }

    /// Records that the payment of a request was cancelled with the provider
    /// before the shopper confirmed it, what its checkout held having lapsed.
    /// Told twice, the second time is a no-op; a payment the cart never
    /// opened, or that already made an order or was given back, is
    /// `PaymentNotFound`.
    pub async fn cancel_payment(
        &self,
        actor: &str,
        cart_id: &str,
        payment_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), OrderError> {
        for attempt in 1..=CART_WRITE_ATTEMPTS {
            let cart = self.require_cart(cart_id).await?;
            if cart.is_payment_cancelled(payment_id) {
                return Ok(());
            }
            let request = cart
                .request_paid_by(payment_id)
                .filter(|_| !cart.is_checked_out())
                .ok_or(OrderError::PaymentNotFound)?;
            let written = cart
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&PaymentCancelled {
                    request_id: request.request_id.clone(),
                    payment_id: payment_id.to_owned(),
                    cancelled_by: actor.to_owned(),
                })
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await;
            match written {
                Ok(_) => return Ok(()),
                Err(WriteError::InvalidOriginalVersion) if attempt < CART_WRITE_ATTEMPTS => {}
                Err(err) => return Err(err.into()),
            }
        }
        Ok(())
    }

    /// The provider holds `payment` on the shopper's card: the cart becomes
    /// the order, with the lines, address and note of the request it pays
    /// for, and the money is taken later. Told twice, the same order comes
    /// back. A second payment of a cart already ordered is
    /// `DuplicatePayment` (the caller releases it); one that does not hold
    /// what the cart was priced at is `AmountMismatch`; one on its way back
    /// is `PaymentNotFound`.
    pub async fn place_order(
        &self,
        actor: &str,
        cart_id: &str,
        payment: Payment,
        routing_key: Option<String>,
    ) -> Result<PlacedOrder, OrderError> {
        let cart = self.require_cart(cart_id).await?;
        let order_id = order_id(cart_id);
        if let Some(order) = self.load_order(&order_id).await? {
            return self
                .settle_existing(cart, &order, &payment, actor, routing_key)
                .await;
        }
        let request = cart
            .request_paid_by(&payment.payment_id)
            .filter(|request| !request.gives_back())
            .ok_or(OrderError::PaymentNotFound)?
            .clone();
        let Some(snapshot) = request.payment() else {
            return Err(OrderError::PaymentNotFound);
        };
        if snapshot.total_cents != payment.amount_cents
            || !snapshot.currency.eq_ignore_ascii_case(&payment.currency)
        {
            return Err(OrderError::AmountMismatch);
        }

        let reference = self.claim_reference(&order_id, routing_key.clone()).await?;
        let mut write = evento::append(&order_id);
        write
            .original_version(0)
            .routing_key_opt(routing_key.clone())
            .event(&Placed {
                reference: reference.clone(),
                cart_id: cart_id.to_owned(),
                email: request.email.clone(),
                shipping: request.shipping.clone(),
                note: request.note.clone(),
                lines: snapshot.lines.clone(),
                currency: snapshot.currency.clone(),
                total_cents: snapshot.total_cents,
                payment: Payment {
                    currency: snapshot.currency.clone(),
                    ..payment.clone()
                },
                supplier: snapshot.supplier.clone(),
                placed_by: actor.to_owned(),
            });
        if let Some(discount) = &snapshot.discount {
            write.event(&Discounted {
                discount_id: discount.discount_id.clone(),
                code: discount.code.clone(),
                subtotal_cents: discount.subtotal_cents,
                amount_cents: discount.amount_cents,
                discounted_by: actor.to_owned(),
            });
        }
        if let Some(billing) = &request.billing {
            write.event(&BillingAddressSet {
                billing: billing.clone(),
                set_by: actor.to_owned(),
            });
        }
        if let Some(shipping) = &snapshot.shipping {
            write.event(&ShippingCharged {
                method_id: shipping.method_id.clone(),
                name: shipping.name.clone(),
                amount_cents: shipping.amount_cents,
                charged_by: actor.to_owned(),
            });
        }
        if let Some(language) = &request.language {
            write.event(&LanguageSet {
                language: language.clone(),
                set_by: actor.to_owned(),
            });
        }
        if request.newsletter {
            write.event(&NewsletterOptInSet {
                set_by: actor.to_owned(),
            });
        }
        if let Some(time_zone) = &request.time_zone {
            write.event(&TimeZoneSet {
                time_zone: time_zone.clone(),
                set_by: actor.to_owned(),
            });
        }
        let placed = write.tenant(&self.tenant).commit(&self.executor).await;
        match placed {
            Ok(_) => {}
            // Told at the same moment on another node: that one stands, under
            // the reference it claimed, and says whether this payment is the
            // same. The reference claimed here names nothing: given back.
            Err(WriteError::InvalidOriginalVersion) => {
                self.release_reference(&reference, actor, routing_key.clone())
                    .await;
                let order = self.require_order(&order_id).await?;
                return self
                    .settle_existing(cart, &order, &payment, actor, routing_key)
                    .await;
            }
            Err(err) => return Err(err.into()),
        }
        let order = self.require_order(&order_id).await?;
        self.check_out(cart, &order, actor, routing_key).await?;
        Ok((&order).into())
    }
}

impl<E: Executor> super::Command<E> {
    /// The cart has its order already: the same payment told again finishes
    /// marking the cart; another one is a duplicate.
    async fn settle_existing(
        &self,
        cart: CartState,
        order: &OrderState,
        payment: &Payment,
        actor: &str,
        routing_key: Option<String>,
    ) -> Result<PlacedOrder, OrderError> {
        if order.payment.payment_id != payment.payment_id {
            return Err(OrderError::DuplicatePayment {
                order_id: order.id.clone(),
            });
        }
        self.check_out(cart, order, actor, routing_key).await?;
        Ok(order.into())
    }

    /// Claims a fresh reference for the order, trying another one on a clash.
    async fn claim_reference(
        &self,
        order_id: &str,
        routing_key: Option<String>,
    ) -> Result<String, OrderError> {
        for _ in 0..REFERENCE_ATTEMPTS {
            let reference = new_reference();
            let claimed = evento::append(order_reference_id(&reference))
                .original_version(0)
                .routing_key_opt(routing_key.clone())
                .event(&Claimed {
                    order_id: order_id.to_owned(),
                })
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await;
            match claimed {
                Ok(_) => return Ok(reference),
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Err(anyhow::anyhow!("no free order reference after {REFERENCE_ATTEMPTS} attempts").into())
    }

    /// Gives back a reference [`claim_reference`](Self::claim_reference)
    /// just claimed, for an order placed under another one. A reference
    /// left claimed names nothing either
    /// ([`load_order_by_reference`](Self::load_order_by_reference) checks the
    /// order carries it), so a failure is only logged.
    async fn release_reference(&self, reference: &str, actor: &str, routing_key: Option<String>) {
        let released = evento::append(order_reference_id(reference))
            .original_version(1)
            .routing_key_opt(routing_key)
            .event(&Released {
                released_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await;
        if let Err(err) = released {
            tracing::warn!(tenant = %self.tenant, %reference, %err, "cannot give back an order reference");
        }
    }

    /// Marks the cart as the order, unless it already is. A shopper changing
    /// the cart at the same moment only moves the version: the mark is tried
    /// again on the fresh cart.
    async fn check_out(
        &self,
        mut cart: CartState,
        order: &OrderState,
        actor: &str,
        routing_key: Option<String>,
    ) -> Result<(), OrderError> {
        for attempt in 1..=CART_WRITE_ATTEMPTS {
            if cart.is_checked_out() {
                return Ok(());
            }
            let marked = cart
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&CheckedOut {
                    order_id: order.id.clone(),
                    checked_out_by: actor.to_owned(),
                })
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await;
            match marked {
                Ok(_) => return Ok(()),
                Err(WriteError::InvalidOriginalVersion) if attempt < CART_WRITE_ATTEMPTS => {
                    cart = self.require_cart(&cart.id).await?;
                }
                Err(err) => return Err(err.into()),
            }
        }
        Ok(())
    }
}
