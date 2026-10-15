use evento::{Executor, ProjectionAggregate, WriteError};

use ulid::Ulid;

use crate::aggregator::{
    BuyerIdentified, CheckedOut, CheckoutRequested, Claimed, DiscountGranted, Discounted,
    PaymentRefused, PaymentRequested, Placed,
};
use crate::error::OrdersError;
use crate::value_object::{
    OrderLine, Payment, ShippingAddress, new_reference, normalize_buyer_vat_number,
    normalize_email, normalize_note, normalize_shipping, order_id, order_reference_id,
};

use super::{CartItem, CartLine, CartState, CheckoutRequest, OrderDiscount, OrderState};

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
    /// The lines whose product is off sale, gone, or whose variant is.
    pub unavailable: Vec<CartLine>,
    /// `None` while no line is priced.
    pub currency: Option<String>,
    /// What the lines add up to.
    pub subtotal_cents: u64,
    /// The cart's discount code, when it takes something off.
    pub discount: Option<OrderDiscount>,
    /// Why the cart's discount code takes nothing off (one of
    /// [`timada_discounts::SHOPPER_CODES`]); the total is then full price.
    pub discount_refused: Option<&'static str>,
    /// What a payment asked for now would charge: the subtotal, less the
    /// discount.
    pub total_cents: u64,
}

impl Quote {
    pub fn item_count(&self) -> u32 {
        self.lines.iter().map(|line| line.quantity).sum()
    }
}

impl<E: Executor> super::Command<E> {
    /// Prices a cart from the catalog as it stands now, reading every product
    /// from the store (never from a replica), so what it says is what a
    /// payment asked for now would charge.
    pub async fn quote_cart(&self, cart_id: &str) -> Result<Quote, OrdersError> {
        let cart = self.require_cart(cart_id).await?;
        self.quote(&cart).await
    }

    pub(crate) async fn quote(&self, cart: &CartState) -> Result<Quote, OrdersError> {
        let mut quote = Quote::default();
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
            let label = variant.map(|variant| {
                variant
                    .selections
                    .iter()
                    .map(|selection| selection.value.as_str())
                    .collect::<Vec<_>>()
                    .join(" / ")
            });
            let order_line = OrderLine {
                product_id: product.id.clone(),
                variant_id: line.variant_id.clone(),
                name: product.name.clone(),
                variant_label: label,
                sku: variant.and_then(|variant| variant.sku.clone()),
                unit_amount_cents: price.amount_cents,
                quantity: line.quantity,
            };
            quote.subtotal_cents += order_line.amount_cents();
            quote.lines.push(order_line);
            quote.tax_categories.push(product.tax_category().to_owned());
        }
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
    /// What to charge: the lines, less the discount.
    pub total_cents: u64,
    /// The discount to charge it with, whose use the dashboard reserves
    /// before it opens the payment.
    pub discount: Option<OrderDiscount>,
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
        if let Some(line) = quote.unavailable.first() {
            return Err(OrdersError::Unavailable {
                product_id: line.product_id.clone(),
            });
        }
        // A code the shopper counted on is never dropped silently.
        if let Some(code) = quote.discount_refused {
            return Err(OrdersError::DiscountRefused { code });
        }
        if let Some(expected) = request.expected_total_cents
            && expected != quote.total_cents
        {
            return Err(OrdersError::PriceChanged {
                total_cents: quote.total_cents,
            });
        }
        let currency = quote.currency.ok_or(OrdersError::EmptyCart)?;
        Ok(Some(PendingPayment {
            request: request.clone(),
            lines: quote.lines,
            currency,
            total_cents: quote.total_cents,
            discount: quote.discount,
        }))
    }

    /// The order a reference names, if any.
    pub async fn load_order_by_reference(
        &self,
        reference: &str,
    ) -> anyhow::Result<Option<OrderState>> {
        let Some(index) = super::order_reference_projection()
            .load(order_reference_id(reference))
            .execute(&self.0)
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
        let note = normalize_note(checkout.note.as_deref())?;
        let vat_number = normalize_buyer_vat_number(checkout.vat_number.as_deref())?;
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
        write.commit(&self.0).await?;
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
            let written = write.commit(&self.0).await;
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
                .commit(&self.0)
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
        let placed = write.commit(&self.0).await;
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
                .commit(&self.0)
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
                .commit(&self.0)
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
