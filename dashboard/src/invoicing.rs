//! Issues the invoice of every order once its money is taken (`Paid`), or
//! the merchant says it came in outside timada (`SettledOutside`), and the
//! credit note of every refund, in the store. An order whose hold went
//! back, nothing taken, has neither. What a document says comes from places
//! only the dashboard sees together: the order and the cart it was paid
//! from (the tenant's store), the kind of goods of each product (the
//! catalog, same store), the merchant's legal identity (the platform store),
//! for a business buyer, VIES's word on its VAT number, and for a sale in
//! another currency than the euro, the European Central Bank's euro rate of
//! the day. The invoicing crate decides the VAT from them.

use evento::{
    Evento, Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use std::sync::Arc;

use timada_invoicing::rates::EURO;
use timada_invoicing::tax::TaxProfile;
use timada_invoicing::{
    Address, InvoiceDiscount, InvoiceShipping, NewCreditNote, NewInvoice, NewInvoiceLine,
    PAYMENT_METHOD_CARD, PAYMENT_METHOD_OUTSIDE, Seller,
};
use timada_orders::aggregator::{Paid, Refunded, SettledOutside};
use timada_tenancy::LegalIdentity;

use crate::cluster::Executor as Store;
use crate::ecb::{EuroRates, RateAnswer};
use crate::vies::{Vies, ViesAnswer, check_patiently};

/// The cursor identity of the subscription, in the store.
pub const SUBSCRIPTION: &str = "timada-dashboard/invoicing";
/// Who the documents say issued them.
pub const ISSUER: &str = "timada-dashboard";

/// The platform store, where the merchant's legal identity lives.
#[derive(Clone)]
struct Platform(Store);

/// The store, to write the documents with.
#[derive(Clone)]
struct TenantStore(Evento);

/// Follows every tenant's orders. Every node runs it:
/// issuing is safe to repeat and to race (see `timada_invoicing`).
pub fn subscription<E: Executor>(
    platform: Store,
    vies: Arc<dyn Vies>,
    euro_rates: Arc<dyn EuroRates>,
    store: Evento,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(Platform(platform))
        .data(vies)
        .data(euro_rates)
        .data(TenantStore(store))
        .handler(on_paid())
        .handler(on_settled_outside())
        .handler(on_refunded())
        .any_routing_key()
        .retry(5)
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The merchant as its invoices name it.
fn seller(legal: &LegalIdentity) -> Seller {
    Seller {
        address: Address {
            name: legal.legal_name.clone(),
            line1: legal.line1.clone(),
            line2: legal.line2.clone(),
            postal_code: legal.postal_code.clone(),
            city: legal.city.clone(),
            country: legal.country.clone(),
        },
        legal_form: legal.legal_form.clone(),
        share_capital_cents: legal.share_capital_cents,
        siret: legal.siret.clone(),
        rcs_city: legal.rcs_city.clone(),
        contact_email: legal.contact_email.clone(),
        vat_number: legal.vat_number.clone(),
    }
}

#[evento::subscription]
async fn on_paid<E: Executor>(context: &Context<'_, E>, event: Event<Paid>) -> anyhow::Result<()> {
    issue_invoice(
        context,
        &event.tenant,
        &event.aggregate_id,
        event.timestamp,
        PAYMENT_METHOD_CARD,
    )
    .await
}

/// The merchant collected the money outside timada: invoiced as a payment
/// is, the invoice saying so.
#[evento::subscription]
async fn on_settled_outside<E: Executor>(
    context: &Context<'_, E>,
    event: Event<SettledOutside>,
) -> anyhow::Result<()> {
    issue_invoice(
        context,
        &event.tenant,
        &event.aggregate_id,
        event.timestamp,
        PAYMENT_METHOD_OUTSIDE,
    )
    .await
}

/// Issues the invoice of `order_id`, paid at `paid_at` (unix seconds) by
/// `payment_method`, unless it has one.
async fn issue_invoice<E: Executor>(
    context: &Context<'_, E>,
    tenant_id: &str,
    order_id: &str,
    paid_at: u64,
    payment_method: &str,
) -> anyhow::Result<()> {
    let platform: Platform = context.extract();
    let store: TenantStore = context.extract();
    let invoicing = timada_invoicing::Command::new(store.0.clone(), tenant_id);
    if invoicing.load_invoice_of_order(order_id).await?.is_some() {
        return Ok(());
    }
    // What was sold, as `Placed` wrote it, with the discount, the shipping
    // and the billing address written beside it.
    let Some(order) = timada_orders::Command::new(store.0.clone(), tenant_id)
        .load_order(order_id)
        .await?
    else {
        return Ok(());
    };
    let tenancy = timada_tenancy::Command(platform.0.clone());
    let link = tenancy
        .load_tenant(tenant_id)
        .await?
        .and_then(|state| state.company);
    let company = match &link {
        Some(link) => tenancy.load_company(&link.company_id).await?,
        None => None,
    };
    let (Some(link), Some(company)) = (link, company) else {
        // Only an order paid before invoicing existed, or a tenant unlinked
        // between payment and now: checkout refuses to take a payment
        // without a company.
        tracing::warn!(tenant = %tenant_id, order = %order_id, "no company: order not invoiced");
        return Ok(());
    };
    let Some(legal) = company.identity.clone() else {
        tracing::warn!(tenant = %tenant_id, order = %order_id, "the company has no identity: order not invoiced");
        return Ok(());
    };
    let vies: Arc<dyn Vies> = context.extract();
    // The kind of goods as it was sold, carried on the line: a product
    // re-filed since (or gone) changes nothing about this invoice.
    let lines: Vec<NewInvoiceLine> = order
        .lines
        .iter()
        .map(|line| NewInvoiceLine {
            name: line.name.clone(),
            variant_label: line.variant_label.clone(),
            sku: line.sku.clone(),
            quantity: line.quantity,
            unit_amount_cents: line.unit_amount_cents,
            tax_category: line.tax_category().to_owned(),
        })
        .collect();
    // A business buyer's number, from the request this payment answered,
    // counts once VIES says it is valid; otherwise the sale is to a consumer
    // (VAT charged: the safe side).
    let vat_number = timada_orders::Command::new(store.0.clone(), tenant_id)
        .load_cart(&order.cart_id)
        .await?
        .and_then(|cart| {
            cart.request_paid_by(&order.payment.payment_id)
                .and_then(|request| request.vat_number.clone())
        });
    let (buyer_vat_number, vies_reference) = match vat_number {
        Some(number) if legal.is_vat_registered() => {
            match check_patiently(vies.as_ref(), &number, legal.vat_number.as_deref()).await {
                ViesAnswer::Valid { reference } => (Some(number), reference),
                ViesAnswer::Invalid => {
                    tracing::info!(tenant = %tenant_id, order = %order_id, %number, "VIES: not a valid VAT number");
                    (None, None)
                }
                ViesAnswer::Unavailable(reason) => {
                    tracing::warn!(tenant = %tenant_id, order = %order_id, %number, %reason, "VIES did not answer: invoiced with VAT");
                    (None, None)
                }
            }
        }
        _ => (None, None),
    };
    let discount = order.discount.clone().map(|discount| InvoiceDiscount {
        code: discount.code,
        amount_cents: discount.amount_cents,
    });
    let shipping = order
        .shipping_charge
        .clone()
        .map(|shipping| InvoiceShipping {
            name: shipping.name,
            amount_cents: shipping.amount_cents,
        });
    let address = |address: &timada_orders::ShippingAddress| Address {
        name: address.name.clone(),
        line1: address.line1.clone(),
        line2: address.line2.clone(),
        postal_code: address.postal_code.clone(),
        city: address.city.clone(),
        country: address.country.clone(),
    };
    let delivery = address(&order.shipping);
    let buyer = order
        .billing
        .as_ref()
        .map_or_else(|| delivery.clone(), address);
    // A sale in another currency: the euro rate of the day of issue, for
    // the EU distance-sales threshold and the VAT in euros the invoice
    // states. Without one, the crate issues what it can; a distance sale
    // whose tax hangs on it is an error here, which retries once the rates
    // are back.
    let issued_at = now();
    let currency = order.currency.clone();
    let euro_rate = if currency.eq_ignore_ascii_case(EURO) {
        None
    } else {
        let rates: Arc<dyn EuroRates> = context.extract();
        let day = timada_core::civil::format_date(issued_at);
        match rates.rate_on(&currency, &day).await {
            RateAnswer::Rate(rate) => Some(rate),
            RateAnswer::NoSuchCurrency => {
                tracing::warn!(tenant = %tenant_id, order = %order_id, %currency, "no euro reference rate for this currency: invoiced without one");
                None
            }
            RateAnswer::Unavailable(reason) => {
                tracing::warn!(tenant = %tenant_id, order = %order_id, %currency, %reason, "the euro reference rates could not be read: invoiced without one");
                None
            }
        }
    };
    let invoice = invoicing
        .issue_invoice(
            NewInvoice {
                company_id: company.id.clone(),
                tenant_id: tenant_id.to_owned(),
                series_code: link.series_code.clone(),
                order_id: order_id.to_owned(),
                order_reference: order.reference.clone(),
                paid_at,
                payment_method: payment_method.to_owned(),
                seller: seller(&legal),
                buyer,
                buyer_email: order.email.clone(),
                delivery,
                currency: order.currency.clone(),
                lines,
                discount,
                shipping,
                total_cents: order.total_cents,
                issued_by: ISSUER.to_owned(),
                tax: TaxProfile {
                    vat_registered: legal.is_vat_registered(),
                    oss_registered: legal.oss_registered,
                },
                buyer_vat_number,
                vies_reference,
                euro_rate,
            },
            &timada_invoicing::Ledger(platform.0.clone()),
            issued_at,
        )
        .await?;
    tracing::info!(tenant = %tenant_id, order = %order_id, invoice = %invoice.label, "invoice issued");
    Ok(())
}

#[evento::subscription]
async fn on_refunded<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Refunded>,
) -> anyhow::Result<()> {
    let store: TenantStore = context.extract();
    let invoicing = timada_invoicing::Command::new(store.0, &event.tenant);
    let credit = NewCreditNote {
        order_id: event.aggregate_id.clone(),
        refund_id: event.data.refund_id.clone(),
        amount_cents: event.data.amount_cents,
        issued_by: ISSUER.to_owned(),
    };
    match invoicing.issue_credit_note(credit, now()).await {
        Ok(note) => {
            tracing::info!(tenant = %event.tenant, order = %event.aggregate_id, credit_note = %note.label, "credit note issued");
            Ok(())
        }
        // An order paid before invoicing existed has nothing to correct.
        Err(timada_invoicing::InvoicingError::InvoiceNotFound) => {
            tracing::warn!(tenant = %event.tenant, order = %event.aggregate_id, "refund of an order without invoice");
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}
