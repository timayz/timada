//! Issues the invoice of every paid order and the credit note of every
//! refund, in each tenant's store. What a document says comes from places
//! only the dashboard sees together: the order and the cart it was paid
//! from (the tenant's store), the kind of goods of each product (the
//! catalog, same store), the merchant's legal identity (the platform store)
//! and, for a business buyer, VIES's word on its VAT number. The invoicing
//! crate decides the VAT from them.

use evento::{
    Evento, Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use std::sync::Arc;

use timada_invoicing::tax::TaxProfile;
use timada_invoicing::{
    Address, NewCreditNote, NewInvoice, NewInvoiceLine, PAYMENT_METHOD_CARD, Seller,
};
use timada_orders::aggregator::{Placed, Refunded};
use timada_tenancy::LegalIdentity;

use crate::cluster::Executor as Store;
use crate::vies::{Vies, ViesAnswer, check_patiently};

/// The cursor identity of the subscription, in each tenant's store.
pub const SUBSCRIPTION: &str = "timada-dashboard/invoicing";
/// Who the documents say issued them.
pub const ISSUER: &str = "timada-dashboard";

#[derive(Clone)]
struct Tenant(String);

/// The platform store, where the merchant's legal identity lives.
#[derive(Clone)]
struct Platform(Store);

/// The tenant's own executor, to write the documents with.
#[derive(Clone)]
struct TenantStore(Evento);

/// Follows one tenant's orders. Every node with the tenant open runs it:
/// issuing is safe to repeat and to race (see `timada_invoicing`).
pub fn subscription<E: Executor>(
    platform: Store,
    vies: Arc<dyn Vies>,
    tenant_id: &str,
    store: Evento,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(Platform(platform))
        .data(vies)
        .data(TenantStore(store))
        .data(Tenant(tenant_id.to_owned()))
        .handler(on_placed())
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
async fn on_placed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Placed>,
) -> anyhow::Result<()> {
    let platform: Platform = context.extract();
    let store: TenantStore = context.extract();
    let tenant: Tenant = context.extract();
    let invoicing = timada_invoicing::Command(store.0.clone());
    if invoicing
        .load_invoice_of_order(&event.aggregate_id)
        .await?
        .is_some()
    {
        return Ok(());
    }
    let tenancy = timada_tenancy::Command(platform.0.clone());
    let link = tenancy
        .load_tenant(&tenant.0)
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
        tracing::warn!(tenant = %tenant.0, order = %event.aggregate_id, "no company: order not invoiced");
        return Ok(());
    };
    let Some(legal) = company.identity.clone() else {
        tracing::warn!(tenant = %tenant.0, order = %event.aggregate_id, "the company has no identity: order not invoiced");
        return Ok(());
    };
    let vies: Arc<dyn Vies> = context.extract();
    let catalog = timada_catalog::Command(store.0.clone());
    let mut lines = Vec::with_capacity(event.data.lines.len());
    for line in &event.data.lines {
        let tax_category = catalog
            .load_product(&line.product_id)
            .await?
            .map(|product| product.tax_category().to_owned())
            .unwrap_or_else(|| timada_catalog::TAX_CATEGORY_STANDARD.to_owned());
        lines.push(NewInvoiceLine {
            name: line.name.clone(),
            variant_label: line.variant_label.clone(),
            sku: line.sku.clone(),
            quantity: line.quantity,
            unit_amount_cents: line.unit_amount_cents,
            tax_category,
        });
    }
    // A business buyer's number, from the request this payment answered,
    // counts once VIES says it is valid; otherwise the sale is to a consumer
    // (VAT charged: the safe side).
    let vat_number = timada_orders::Command(store.0.clone())
        .load_cart(&event.data.cart_id)
        .await?
        .and_then(|cart| {
            cart.request_paid_by(&event.data.payment.payment_id)
                .and_then(|request| request.vat_number.clone())
        });
    let (buyer_vat_number, vies_reference) = match vat_number {
        Some(number) if legal.is_vat_registered() => {
            match check_patiently(vies.as_ref(), &number, legal.vat_number.as_deref()).await {
                ViesAnswer::Valid { reference } => (Some(number), reference),
                ViesAnswer::Invalid => {
                    tracing::info!(tenant = %tenant.0, order = %event.aggregate_id, %number, "VIES: not a valid VAT number");
                    (None, None)
                }
                ViesAnswer::Unavailable(reason) => {
                    tracing::warn!(tenant = %tenant.0, order = %event.aggregate_id, %number, %reason, "VIES did not answer: invoiced with VAT");
                    (None, None)
                }
            }
        }
        _ => (None, None),
    };
    let shipping = &event.data.shipping;
    let address = Address {
        name: shipping.name.clone(),
        line1: shipping.line1.clone(),
        line2: shipping.line2.clone(),
        postal_code: shipping.postal_code.clone(),
        city: shipping.city.clone(),
        country: shipping.country.clone(),
    };
    let invoice = invoicing
        .issue_invoice(
            NewInvoice {
                company_id: company.id.clone(),
                tenant_id: tenant.0.clone(),
                series_code: link.series_code.clone(),
                order_id: event.aggregate_id.clone(),
                order_reference: event.data.reference.clone(),
                paid_at: event.timestamp,
                payment_method: PAYMENT_METHOD_CARD.to_owned(),
                seller: seller(&legal),
                buyer: address.clone(),
                buyer_email: event.data.email.clone(),
                delivery: address,
                currency: event.data.currency.clone(),
                lines,
                total_cents: event.data.total_cents,
                issued_by: ISSUER.to_owned(),
                tax: TaxProfile {
                    vat_registered: legal.is_vat_registered(),
                    oss_registered: legal.oss_registered,
                },
                buyer_vat_number,
                vies_reference,
            },
            &timada_invoicing::Ledger(platform.0.clone()),
            now(),
        )
        .await?;
    tracing::info!(tenant = %tenant.0, order = %event.aggregate_id, invoice = %invoice.label, "invoice issued");
    Ok(())
}

#[evento::subscription]
async fn on_refunded<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Refunded>,
) -> anyhow::Result<()> {
    let store: TenantStore = context.extract();
    let tenant: Tenant = context.extract();
    let invoicing = timada_invoicing::Command(store.0);
    let credit = NewCreditNote {
        order_id: event.aggregate_id.clone(),
        refund_id: event.data.refund_id.clone(),
        amount_cents: event.data.amount_cents,
        issued_by: ISSUER.to_owned(),
    };
    match invoicing.issue_credit_note(credit, now()).await {
        Ok(note) => {
            tracing::info!(tenant = %tenant.0, order = %event.aggregate_id, credit_note = %note.label, "credit note issued");
            Ok(())
        }
        // An order paid before invoicing existed has nothing to correct.
        Err(timada_invoicing::InvoicingError::InvoiceNotFound) => {
            tracing::warn!(tenant = %tenant.0, order = %event.aggregate_id, "refund of an order without invoice");
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}
