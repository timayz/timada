use evento::{Executor, ProjectionAggregate, WriteError};

use timada_discounts::{Offer, OfferItem, OffersState, offers_id, offers_projection};
use ulid::Ulid;

use crate::aggregator::{
    BillingAddressGiven, BillingAddressSet, BuyerIdentified, CheckedOut, CheckoutRequested,
    Claimed, DiscountGranted, Discounted, LanguageGiven, LanguageSet, NewsletterOptInSet,
    NewsletterOptedIn, PaymentRefused, PaymentRequested, Placed, ShippingCharged, ShippingGranted,
    ShippingMethodChosen,
};
use crate::error::OrdersError;
use crate::value_object::{
    OrderLine, Payment, ShippingAddress, new_reference, normalize_billing,
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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Quote {
    pub lines: Vec<OrderLine>,
    /// What kind of goods each of `lines` is for VAT, in the same order.
    pub tax_categories: Vec<String>,
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
    pub discount_refused: Option<&'static str>,
    /// What the goods cost: the subtotal, less the discount. Shipping, when
    /// the shop charges for it, comes on top (see [`Quote::shipping_for`]).
    pub total_cents: u64,
}

impl Quote {
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineOffer {
    pub offer_id: String,
    /// What one costs without the offer.
    pub full_unit_amount_cents: u64,
}

/// An offer a cart may take, priced from the catalog as it stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotedOffer {
    pub offer_id: String,
    pub product_id: String,
    pub variant_id: Option<String>,
    /// The product's name.
    pub name: String,
    /// The variant's values (`Beige / Large`).
    pub variant_label: Option<String>,
    /// What the merchant titled it, if anything.
    pub title: Option<String>,
    /// What one costs without the offer.
    pub unit_amount_cents: u64,
    /// What the one unit costs with it.
    pub offer_amount_cents: u64,
}

/// `Beige / Large`: the values a variant takes.
fn variant_label(variant: &timada_catalog::ProductVariant) -> String {
    variant
        .selections
        .iter()
        .map(|selection| selection.value.as_str())
        .collect::<Vec<_>>()
        .join(" / ")
}

impl<E: Executor> super::Command<E> {
    /// Prices a cart from the catalog as it stands now, reading every product
    /// from the store (never from a replica), so what it says is what a
    /// payment asked for now would charge.
    pub async fn quote_cart(&self, cart_id: &str) -> Result<Quote, OrdersError> {
        let cart = self.require_cart(cart_id).await?;
        self.quote(&cart).await
    }

    /// Prices the cart's lines at full price, then lets each live offer
    /// price one unit of its item, when the rest of the cart meets its
    /// conditions: that unit becomes a line of its own, so the order, its
    /// invoice and its mail say what it cost. The discount code comes off
    /// what that adds up to.
    pub(crate) async fn quote(&self, cart: &CartState) -> Result<Quote, OrdersError> {
        let mut quote = Quote::default();
        let mut priced: Vec<(OrderLine, String)> = Vec::new();
        for line in &cart.lines {
            let item = CartItem {
                product_id: line.product_id.clone(),
                variant_id: line.variant_id.clone(),
                quantity: line.quantity,
            };
            let product = match self.require_sellable(&item).await {
                Ok(product) => product,
                Err(
                    OrdersError::Unavailable { .. }
                    | OrdersError::VariantRequired
                    | OrdersError::VariantNotFound,
                ) => {
                    quote.unavailable.push(line.clone());
                    continue;
                }
                Err(err) => return Err(err),
            };
            match self
                .require_in_stock(
                    &cart.id,
                    &line.product_id,
                    line.variant_id.as_deref(),
                    line.quantity,
                )
                .await
            {
                Ok(()) => {}
                Err(OrdersError::OutOfStock { .. }) => {
                    quote.unavailable.push(line.clone());
                    quote.out_of_stock.push(line.clone());
                    continue;
                }
                Err(err) => return Err(err),
            }
            let variant = line
                .variant_id
                .as_deref()
                .and_then(|id| product.variant(id));
            let price = match variant {
                Some(variant) => &variant.price,
                // `require_sellable` let a line without a variant through
                // only for a product priced on its own.
                None => product.price.as_ref().ok_or(OrdersError::Unavailable {
                    product_id: product.id.clone(),
                })?,
            };
            match &quote.currency {
                Some(currency) if *currency != price.currency => {
                    return Err(OrdersError::MixedCurrencies);
                }
                Some(_) => {}
                None => quote.currency = Some(price.currency.clone()),
            }
            let order_line = OrderLine {
                product_id: product.id.clone(),
                variant_id: line.variant_id.clone(),
                name: product.name.clone(),
                variant_label: variant.map(variant_label),
                sku: variant.and_then(|variant| variant.sku.clone()),
                unit_amount_cents: price.amount_cents,
                quantity: line.quantity,
            };
            priced.push((order_line, product.tax_category().to_owned()));
        }
        self.price_offers(&mut quote, priced).await?;
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
                Err(OrdersError::DiscountRefused { code }) => quote.discount_refused = Some(code),
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
    ) -> Result<OrderDiscount, OrdersError> {
        let discount =
            self.load_discount_by_code(code)
                .await?
                .ok_or(OrdersError::DiscountRefused {
                    code: timada_discounts::DISCOUNT_UNKNOWN,
                })?;
        let amount_cents = discount
            .amount_off(subtotal_cents, currency, timada_discounts::unix_now())
            .map_err(OrdersError::from_discount)?;
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
        priced: Vec<(OrderLine, String)>,
    ) -> Result<(), OrdersError> {
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
            .map(|(line, _)| OfferItem {
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
            .map(|(offer, (line, _))| {
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

        for ((line, tax_category), line_offer) in priced.into_iter().zip(applied) {
            let Some(line_offer) = line_offer else {
                quote.lines.push(line);
                quote.tax_categories.push(tax_category);
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
            quote.tax_categories.push(tax_category.clone());
            quote.line_offers.push(Some(line_offer));
            if line.quantity > 1 {
                quote.lines.push(OrderLine {
                    quantity: line.quantity - 1,
                    ..line
                });
                quote.tax_categories.push(tax_category);
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
            let product = match self.require_sellable(&item).await {
                Ok(product) => product,
                Err(
                    OrdersError::Unavailable { .. }
                    | OrdersError::VariantRequired
                    | OrdersError::VariantNotFound,
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
                variant_label: variant.map(variant_label),
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
    ) -> Result<Option<PendingPayment>, OrdersError> {
        let cart = self.require_cart(cart_id).await?;
        let Some(request) = cart.request(request_id) else {
            return Ok(None);
        };
        if request.answer.is_some() || cart.is_checked_out() {
            return Ok(None);
        }
        if cart.lines.is_empty() {
            return Err(OrdersError::EmptyCart);
        }
        let quote = self.quote(&cart).await?;
        if let Some(line) = quote.out_of_stock.first() {
            return Err(OrdersError::OutOfStock {
                product_id: line.product_id.clone(),
                available: 0,
            });
        }
        if let Some(line) = quote.unavailable.first() {
            return Err(OrdersError::Unavailable {
                product_id: line.product_id.clone(),
            });
        }
        // A code the shopper counted on is never dropped silently.
        if let Some(code) = quote.discount_refused {
            return Err(OrdersError::DiscountRefused { code });
        }
        let shipping = self.price_shipping(&quote, request).await?;
        let total_cents = quote.total_cents
            + shipping
                .as_ref()
                .map_or(0, |shipping| shipping.amount_cents);
        if let Some(expected) = request.expected_total_cents
            && expected != total_cents
        {
            return Err(OrdersError::PriceChanged { total_cents });
        }
        let currency = quote.currency.ok_or(OrdersError::EmptyCart)?;
        Ok(Some(PendingPayment {
            request: request.clone(),
            lines: quote.lines,
            currency,
            total_cents,
            discount: quote.discount,
            shipping,
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
    ) -> Result<Option<OrderShipping>, OrdersError> {
        let methods = self.shipping_methods().await?;
        if methods.is_empty() {
            return Ok(None);
        }
        let method = request
            .shipping_method
            .as_deref()
            .and_then(|id| methods.iter().find(|method| method.id == id))
            .filter(|method| method.covers(&request.shipping.country))
            .ok_or(OrdersError::ShippingUnavailable)?;
        let amount_cents = quote
            .shipping_for(method)
            .ok_or(OrdersError::ShippingUnavailable)?;
        Ok(Some(OrderShipping {
            method_id: method.id.clone(),
            name: method.name.clone(),
            amount_cents,
        }))
    }

    /// The order a reference names, if any.
    pub async fn load_order_by_reference(
        &self,
        reference: &str,
    ) -> anyhow::Result<Option<OrderState>> {
        let Some(index) = super::order_reference_projection()
            .load(order_reference_id(reference))
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await?
        else {
            return Ok(None);
        };
        self.load_order(&index.order_id).await
    }
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
    ) -> Result<String, OrdersError> {
        let email = normalize_email(&checkout.email)?;
        let shipping = normalize_shipping(checkout.shipping)?;
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
            return Err(OrdersError::BusinessBuyersRefused);
        }
        let cart = self.require_open_cart(cart_id).await?;
        if cart.lines.is_empty() {
            return Err(OrdersError::EmptyCart);
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
        write.tenant(&self.tenant).commit(&self.executor).await?;
        Ok(request_id)
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
    ) -> Result<(), OrdersError> {
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
    /// storefront words. A request answered already is left alone.
    pub async fn refuse_payment_request(
        &self,
        actor: &str,
        cart_id: &str,
        request_id: &str,
        code: &str,
        routing_key: Option<String>,
    ) -> Result<(), OrdersError> {
        for attempt in 1..=CART_WRITE_ATTEMPTS {
            let cart = self.require_cart(cart_id).await?;
            if cart.request(request_id).is_none_or(|r| r.answer.is_some()) {
                return Ok(());
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
                Ok(_) => return Ok(()),
                Err(WriteError::InvalidOriginalVersion) if attempt < CART_WRITE_ATTEMPTS => {}
                Err(err) => return Err(err.into()),
            }
        }
        Ok(())
    }

    /// The provider says `payment` went through: the cart becomes the order,
    /// with the lines, address and note of the request it paid for. Told
    /// twice, the same order comes back. A second payment of a cart already
    /// ordered is `DuplicatePayment` (the caller gives it back); one that
    /// does not match what the cart was priced at is `AmountMismatch`.
    pub async fn place_paid_order(
        &self,
        actor: &str,
        cart_id: &str,
        payment: Payment,
        routing_key: Option<String>,
    ) -> Result<PlacedOrder, OrdersError> {
        let cart = self.require_cart(cart_id).await?;
        let order_id = order_id(cart_id);
        if let Some(order) = self.load_order(&order_id).await? {
            return self
                .settle_existing(cart, &order, &payment, actor, routing_key)
                .await;
        }
        let request = cart
            .request_paid_by(&payment.payment_id)
            .ok_or(OrdersError::PaymentNotFound)?
            .clone();
        let Some(snapshot) = request.payment() else {
            return Err(OrdersError::PaymentNotFound);
        };
        if snapshot.total_cents != payment.amount_cents
            || !snapshot.currency.eq_ignore_ascii_case(&payment.currency)
        {
            return Err(OrdersError::AmountMismatch);
        }

        let reference = self.claim_reference(&order_id, routing_key.clone()).await?;
        let mut write = evento::append(&order_id);
        write
            .original_version(0)
            .routing_key_opt(routing_key.clone())
            .event(&Placed {
                reference,
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
        let placed = write.tenant(&self.tenant).commit(&self.executor).await;
        match placed {
            Ok(_) => {}
            // Told at the same moment on another node: that one stands, and
            // says whether this payment is the same.
            Err(WriteError::InvalidOriginalVersion) => {
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
    ) -> Result<PlacedOrder, OrdersError> {
        if order.payment.payment_id != payment.payment_id {
            return Err(OrdersError::DuplicatePayment {
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
    ) -> Result<String, OrdersError> {
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

    /// Marks the cart as the order, unless it already is. A shopper changing
    /// the cart at the same moment only moves the version: the mark is tried
    /// again on the fresh cart.
    async fn check_out(
        &self,
        mut cart: CartState,
        order: &OrderState,
        actor: &str,
        routing_key: Option<String>,
    ) -> Result<(), OrdersError> {
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
