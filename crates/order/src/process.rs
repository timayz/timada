//! Anti-corruption layer from the cart context: a `CartCheckedOut` fact is
//! translated into a `PlaceOrder` command, after the cart's code — if any —
//! was redeemed with the promotion context, so the order is born with the
//! price it will be paid at. The mirror subscription gives the code back when
//! the order is cancelled. Neither is strict — each listens to one event.

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;
use timada_cart::aggregator::CartCheckedOut;
use timada_core::Money;
use timada_pricing::price_id;
use timada_promotion::{CodeKind, PromotionError};
use timada_tax::{
    BusinessBuyer, BusinessPurchase, ReverseChargeProof, TaxZone, TaxZones, VatNumber,
    VatNumberValidator, VatRegistry, qualifies_for_reverse_charge, reverse_charged,
};

use crate::{
    aggregator::OrderCancelled,
    command::{Command, OrderTax, PlaceOrder, order_id},
    error::OrderError,
    numbering::allocate_order_number,
    query::load_order_details,
    value_object::{
        DeliveryChoice, OrderDiscount, OrderLine, OrderTotals, PaymentMode, PromoKind, Seller,
        order_total,
    },
};

pub const ORDER_CHECKOUT_SUBSCRIPTION: &str = "order-checkout";
pub const ORDER_PROMO_RELEASE_SUBSCRIPTION: &str = "order-promo-release";

/// "Frais de dossier" charged on instalment plans by default: 4,49 €.
pub const INSTALLMENT_HANDLING_FEE_MINOR: i64 = 449;

/// What paying in several times costs, **per currency** — a host value, taken
/// by the checkout subscription. Euros at
/// [`INSTALLMENT_HANDLING_FEE_MINOR`] by default. A storefront offers
/// instalments only in the currencies that have a fee: nothing is converted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallmentHandlingFees(pub timada_core::PerCurrency);

impl Default for InstallmentHandlingFees {
    fn default() -> Self {
        Self(timada_core::PerCurrency::none().with(Money::eur(INSTALLMENT_HANDLING_FEE_MINOR)))
    }
}

impl InstallmentHandlingFees {
    /// The fee in `currency`; `None`: instalments are not offered in it.
    pub fn fee(&self, currency: &str) -> Option<Money> {
        self.0.get(currency).cloned()
    }
}

/// When a business's VAT number lets it buy without the shop's VAT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReverseChargePolicy {
    /// How old the registry's last "valid" may be. A registry that is down at
    /// checkout leaves the last answer standing; past this age it no longer
    /// does, and the order is taxed as a consumer's. Thirty days by default.
    pub max_check_age: std::time::Duration,
    /// A check younger than this is not made again at checkout — the
    /// storefront usually just made it. Ten minutes by default.
    pub recheck_after: std::time::Duration,
}

impl Default for ReverseChargePolicy {
    fn default() -> Self {
        Self {
            max_check_age: std::time::Duration::from_secs(30 * 86_400),
            recheck_after: std::time::Duration::from_secs(600),
        }
    }
}

/// The company `customer_id` buys as, with its VAT number when that number
/// could make a delivery in `zone` an intra-community supply.
async fn qualifying_company<E: Executor>(
    executor: &E,
    zones: &TaxZones,
    zone: &TaxZone,
    customer_id: &str,
) -> anyhow::Result<Option<(timada_customer::CompanyIdentityView, Option<VatNumber>)>> {
    let Some(company) = timada_customer::load_company_identity(executor, customer_id)
        .await?
        .filter(|company| company.is_company())
    else {
        return Ok(None);
    };
    let number = VatNumber::parse(&company.vat_number).ok().filter(|number| {
        qualifies_for_reverse_charge(zone, number, &zones.default_zone().countries)
    });
    Ok(Some((company, number)))
}

/// Asks the VAT registry again about the number of a business that is about
/// to buy without VAT, unless it was asked less than
/// [`ReverseChargePolicy::recheck_after`] ago. A registry that cannot answer
/// changes nothing: the answers before stand. To call before an order is
/// placed — the checkout ACL does, and a storefront does before it shows the
/// last total.
pub async fn refresh_vat_standing<E: Executor>(
    executor: &E,
    registry: &dyn VatNumberValidator,
    zones: &TaxZones,
    zone: &TaxZone,
    customer_id: &str,
    policy: &ReverseChargePolicy,
) -> anyhow::Result<()> {
    let Some((company, Some(_))) = qualifying_company(executor, zones, zone, customer_id).await?
    else {
        return Ok(());
    };
    let now = timada_core::time::now_unix_secs()?;
    let fresh = company
        .last_check
        .as_ref()
        .is_some_and(|check| now.saturating_sub(check.checked_at) < policy.recheck_after.as_secs());
    if fresh {
        return Ok(());
    }
    let answer = timada_customer::Command(executor)
        .check_company_vat_number(customer_id, registry)
        .await?;
    if let Some(Err(unavailable)) = answer {
        tracing::warn!(%customer_id, %unavailable, "VAT number not re-checked: the last answer stands");
    }
    Ok(())
}

/// The business `customer_id` buys as — `None` for a consumer — and, when a
/// delivery in `zone` is an intra-community supply to it, the check of its
/// VAT number that says so. Reads what is known; [`refresh_vat_standing`] is
/// what asks the registry.
pub async fn business_purchase<E: Executor>(
    executor: &E,
    zones: &TaxZones,
    zone: &TaxZone,
    customer_id: &str,
    policy: &ReverseChargePolicy,
) -> anyhow::Result<Option<BusinessPurchase>> {
    let Some((company, number)) = qualifying_company(executor, zones, zone, customer_id).await?
    else {
        return Ok(None);
    };
    let now = timada_core::time::now_unix_secs()?;
    let reverse_charge = number
        .and_then(|_| company.standing_check(now, policy.max_check_age.as_secs()))
        .map(|check| ReverseChargeProof {
            consultation_ref: check.consultation_ref.clone(),
            checked_at: check.checked_at,
        });
    Ok(Some(BusinessPurchase {
        buyer: BusinessBuyer {
            company_name: company.company_name,
            vat_number: company.vat_number,
        },
        reverse_charge,
    }))
}

/// Needs the `SqlitePool` as subscription data: order numbers are allocated
/// and promo-code redemption caps counted in SQL. Takes the host's
/// `timada_tax::TaxZones` the same way; without one it uses
/// `TaxZones::default()` (metropolitan France, overseas as exports). Optional:
/// a `timada_tax::VatRegistry`, asked again about a business's VAT number
/// before its order is placed without VAT, and a [`ReverseChargePolicy`];
/// `timada_shipping::DeliveryFees`, what each delivery method costs per
/// currency (the built-in euro fees otherwise); [`InstallmentHandlingFees`].
pub fn order_checkout_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(ORDER_CHECKOUT_SUBSCRIPTION).handler(place_order_on_cart_checked_out())
}

/// Needs the `SqlitePool` as subscription data, like the checkout one.
pub fn order_promo_release_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(ORDER_PROMO_RELEASE_SUBSCRIPTION)
        .handler(release_code_on_order_cancelled())
}

fn promotion<'a, E: Executor>(
    ctx: &'a Context<'_, E>,
) -> anyhow::Result<timada_promotion::Command<'a, E>> {
    let db = ctx
        .get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))?;
    Ok(timada_promotion::Command {
        executor: ctx.executor,
        db,
    })
}

#[evento::subscription]
async fn place_order_on_cart_checked_out<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<CartCheckedOut>,
) -> anyhow::Result<()> {
    let cart_id = event.aggregate_id.to_owned();
    let Some(cart) = timada_cart::load_cart_details(ctx.executor, &cart_id).await? else {
        anyhow::bail!("cart {cart_id} checked out but its details cannot be loaded");
    };

    // Where the order goes decides how it is taxed. The storefront only lets
    // deliverable countries through; should one slip by, the order is kept
    // and taxed as a domestic one rather than lost.
    let zones = ctx.get::<TaxZones>().unwrap_or_default();
    let country = &event.data.delivery_address.country_code;
    let zone = zones.zone_of(country).unwrap_or_else(|| {
        tracing::warn!(%cart_id, %country, "delivery country outside every tax zone");
        zones.default_zone()
    });

    // A business of another member state with a valid VAT number buys
    // without VAT: the zone is then priced like an export.
    let policy = ctx.get::<ReverseChargePolicy>().unwrap_or_default();
    if let Some(registry) = ctx.get::<VatRegistry>() {
        refresh_vat_standing(
            ctx.executor,
            registry.0.as_ref(),
            &zones,
            zone,
            &event.data.customer_id,
            &policy,
        )
        .await?;
    }
    let business =
        business_purchase(ctx.executor, &zones, zone, &event.data.customer_id, &policy).await?;
    let exempt_zone;
    let zone = match &business {
        Some(BusinessPurchase {
            reverse_charge: Some(_),
            ..
        }) => {
            exempt_zone = reverse_charged(zone);
            &exempt_zone
        }
        _ => zone,
    };

    // Delivery is charged in the cart's currency, from the host's fees.
    let currency = cart.subtotal.currency.clone();
    let method_code = &event.data.delivery.method_code;
    if !timada_shipping::DeliveryMethod::is_known(method_code) {
        return Err(OrderError::UnknownDeliveryMethod(method_code.clone()).into());
    }
    let fees = ctx
        .get::<timada_shipping::DeliveryFees>()
        .unwrap_or_default();
    let listed_fee = fees.fee(method_code, &currency).unwrap_or_else(|| {
        // The storefront only offers the methods priced in the cart's
        // currency; should one slip by, the order is kept — delivered free —
        // rather than lost.
        tracing::error!(%cart_id, %method_code, %currency, "delivery method without a fee in the cart's currency: charged nothing");
        Money::zero(&currency)
    });
    let shipping_fee = zone.charged(&listed_fee, zones.fee_vat_rate_bp);
    let handling_fee = match event.data.payment_mode {
        timada_cart::PaymentMode::Card => Money::zero(&currency),
        timada_cart::PaymentMode::Installments { .. } => ctx
            .get::<InstallmentHandlingFees>()
            .unwrap_or_default()
            .fee(&currency)
            .unwrap_or_else(|| {
                // Not offered in this currency; should one slip by, the order
                // is kept, without the fee.
                tracing::error!(%cart_id, %currency, "instalments without a handling fee in the cart's currency: charged nothing");
                Money::zero(&currency)
            }),
    };

    let charged = lines_charged_in_zone(ctx.executor, &zones, zone, cart.lines).await?;
    let line_rates = charged
        .iter()
        .map(|c| (c.line.product_id.clone(), c.rate_bp))
        .collect();
    let lines: Vec<OrderLine> = charged.into_iter().map(|c| c.line).collect();
    let tax = OrderTax {
        zone_code: zone.code.clone(),
        treatment: zone.treatment,
        line_rates,
        shipping_rate_bp: zone.applied_rate_bp(zones.fee_vat_rate_bp),
    };
    let totals = order_total(&lines, &shipping_fee, &handling_fee)?;
    let discount = match cart.promo_code.as_deref() {
        Some(code) => redeem_code(ctx, code, &order_id(&cart_id), &totals).await?,
        None => None,
    };

    // Keyed by the order id: a redelivery gets the number it already has.
    let db = ctx
        .get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))?;
    let order_number = allocate_order_number(&db, &order_id(&cart_id)).await?;

    let cmd = PlaceOrder {
        cart_id: cart_id.clone(),
        customer_id: event.data.customer_id,
        seller: Seller::Ldlc,
        lines,
        delivery_address: event.data.delivery_address,
        billing_address: event.data.billing_address,
        delivery: DeliveryChoice {
            method_code: event.data.delivery.method_code,
            pickup_store_id: event.data.delivery.pickup_store_id,
        },
        payment_mode: match event.data.payment_mode {
            timada_cart::PaymentMode::Card => PaymentMode::Card,
            timada_cart::PaymentMode::Installments { count } => PaymentMode::Installments { count },
        },
        shipping_fee,
        handling_fee,
        promo_code: cart.promo_code,
        discount,
        order_number: Some(order_number),
        tax: Some(tax),
        business,
    };

    match Command(ctx.executor).place_order(cmd).await {
        Ok(_) => Ok(()),
        // Redelivery of the same checkout: the order already exists.
        Err(OrderError::AlreadyPlaced(_)) => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// A cart line as the order will carry it: priced for a tax zone, with the
/// VAT rate inside that price.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChargedLine {
    pub line: OrderLine,
    pub rate_bp: u16,
}

/// What a cart's lines are charged in `zone`. Cart lines hold the listed,
/// tax-inclusive price of the day they were added; each product's VAT rate is
/// read from the pricing context. Public so a checkout page can show exactly
/// what the order will be placed at.
pub async fn lines_charged_in_zone<E: Executor>(
    executor: &E,
    zones: &TaxZones,
    zone: &TaxZone,
    cart_lines: Vec<timada_cart::CartLine>,
) -> anyhow::Result<Vec<ChargedLine>> {
    let mut charged = Vec::with_capacity(cart_lines.len());
    for line in cart_lines {
        let listed_rate = timada_pricing::load_product_price(executor, price_id(&line.product_id))
            .await?
            .map_or(zones.fallback_vat_rate_bp, |price| price.vat_rate_bp);
        let unit_price = zone.charged(&line.unit_price, listed_rate);
        charged.push(ChargedLine {
            rate_bp: zone.applied_rate_bp(listed_rate),
            line: OrderLine {
                unit_price,
                ..order_line(line)
            },
        });
    }
    Ok(charged)
}

/// Redeems the cart's code for the order about to be placed. Idempotent per
/// order id, so a redelivery gets the same answer. A code the promotion
/// context refuses (the storefront checks it, but a cap can fill up in
/// between) is logged and the order is placed at full price rather than lost.
async fn redeem_code<E: Executor>(
    ctx: &Context<'_, E>,
    code: &str,
    order_id: &str,
    totals: &OrderTotals,
) -> anyhow::Result<Option<OrderDiscount>> {
    let redeemed = promotion(ctx)?
        .redeem_code(code, order_id, &totals.subtotal, &totals.max_discount())
        .await;
    match redeemed {
        Ok(redeemed) => Ok(Some(OrderDiscount {
            code: redeemed.code,
            kind: match redeemed.kind {
                CodeKind::Discount => PromoKind::Discount,
                CodeKind::Voucher => PromoKind::Voucher,
            },
            amount: redeemed.amount,
        })),
        Err(
            err @ (PromotionError::UnknownCode
            | PromotionError::Inactive
            | PromotionError::Expired
            | PromotionError::LimitReached
            | PromotionError::Cancelled
            | PromotionError::InsufficientBalance { .. }
            | PromotionError::NotApplicable
            | PromotionError::Money(_)),
        ) => {
            tracing::warn!(%order_id, %code, error = %err, "promo code not honoured");
            Ok(None)
        }
        Err(err) => Err(err.into()),
    }
}

/// `OrderCancelled` → the redemption slot is freed, or the voucher gets its
/// money back. Idempotent: a code already given back is a no-op.
#[evento::subscription]
async fn release_code_on_order_cancelled<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<OrderCancelled>,
) -> anyhow::Result<()> {
    let order_id = event.aggregate_id.to_owned();
    let Some(order) = load_order_details(ctx.executor, &order_id).await? else {
        anyhow::bail!("order {order_id} cancelled but cannot be loaded");
    };
    let Some(discount) = order.discount else {
        return Ok(());
    };
    promotion(ctx)?
        .release_code(&discount.code, &order_id)
        .await?;
    tracing::info!(%order_id, code = %discount.code, "promo code released");
    Ok(())
}

fn order_line(line: timada_cart::CartLine) -> OrderLine {
    OrderLine {
        product_id: line.product_id,
        name: line.name,
        quantity: line.quantity,
        unit_price: line.unit_price,
        warranty_months: line.warranty_months,
    }
}
