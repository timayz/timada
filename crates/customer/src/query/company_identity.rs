//! The business a customer buys as: its name, its VAT number, and what the
//! VAT registry said about that number — folded from the `Customer` stream.

use evento::{Executor, metadata::Event, projection::Projection};

use crate::aggregator::{
    BillingAddressSet, CompanyIdentified, CompanyIdentityRemoved, Customer, CustomerEmailChanged,
    CustomerRegistered, DeliveryAddressAdded, DeliveryAddressChanged, DeliveryAddressRemoved,
    PreferredDeliveryAddressChosen, VatNumberChecked,
};

/// One answer of the VAT registry.
#[derive(Debug, Clone, PartialEq, Eq, bitcode::Encode, bitcode::Decode)]
pub struct VatCheckRecord {
    pub valid: bool,
    /// Unix seconds.
    pub checked_at: u64,
    pub consultation_ref: Option<String>,
    pub registered_name: Option<String>,
}

#[evento::projection(bitcode::Encode, bitcode::Decode)]
#[derive(Debug, PartialEq)]
pub struct CompanyIdentityView {
    pub customer_id: String,
    /// Empty while the customer buys as a consumer.
    pub company_name: String,
    /// Compact, with its prefix; empty while the customer buys as a consumer.
    pub vat_number: String,
    /// The latest answer about *this* number.
    pub last_check: Option<VatCheckRecord>,
    /// The latest answer that said "valid" — still the latest one, or the one
    /// before a registry outage; forgotten once the registry says "invalid".
    pub last_valid_check: Option<VatCheckRecord>,
}

impl CompanyIdentityView {
    pub fn is_company(&self) -> bool {
        !self.vat_number.is_empty()
    }

    /// The check that lets this business buy without the seller's VAT at
    /// `now`: the latest answer, valid, and no older than `max_age_secs` —
    /// a sale is exempt on the strength of a recent check, not of a number
    /// that was valid once.
    pub fn standing_check(&self, now: u64, max_age_secs: u64) -> Option<&VatCheckRecord> {
        self.last_valid_check
            .as_ref()
            .filter(|check| now.saturating_sub(check.checked_at) <= max_age_secs)
    }
}

pub fn create_projection<E: Executor>() -> Projection<E, CompanyIdentityView> {
    Projection::new::<Customer>()
        .handler(on_customer_registered())
        .handler(on_company_identified())
        .handler(on_company_identity_removed())
        .handler(on_vat_number_checked())
        .skip::<CustomerEmailChanged>()
        .skip::<BillingAddressSet>()
        .skip::<DeliveryAddressAdded>()
        .skip::<DeliveryAddressChanged>()
        .skip::<DeliveryAddressRemoved>()
        .skip::<PreferredDeliveryAddressChosen>()
        .strict()
}

pub async fn load<E: Executor>(
    executor: &E,
    id: impl Into<String>,
) -> anyhow::Result<Option<CompanyIdentityView>> {
    create_projection().load(id).execute(executor).await
}

#[evento::handler]
async fn on_customer_registered(
    event: Event<CustomerRegistered>,
    row: &mut CompanyIdentityView,
) -> anyhow::Result<()> {
    row.customer_id = event.aggregate_id.to_owned();
    Ok(())
}

#[evento::handler]
async fn on_company_identified(
    event: Event<CompanyIdentified>,
    row: &mut CompanyIdentityView,
) -> anyhow::Result<()> {
    // Another number: what was said of the one before says nothing of it.
    if row.vat_number != event.data.vat_number {
        row.last_check = None;
        row.last_valid_check = None;
    }
    row.company_name = event.data.company_name;
    row.vat_number = event.data.vat_number;
    Ok(())
}

#[evento::handler]
async fn on_company_identity_removed(
    _event: Event<CompanyIdentityRemoved>,
    row: &mut CompanyIdentityView,
) -> anyhow::Result<()> {
    row.company_name.clear();
    row.vat_number.clear();
    row.last_check = None;
    row.last_valid_check = None;
    Ok(())
}

#[evento::handler]
async fn on_vat_number_checked(
    event: Event<VatNumberChecked>,
    row: &mut CompanyIdentityView,
) -> anyhow::Result<()> {
    if row.vat_number != event.data.vat_number {
        return Ok(());
    }
    let record = VatCheckRecord {
        valid: event.data.valid,
        checked_at: event.timestamp,
        consultation_ref: event.data.consultation_ref,
        registered_name: event.data.registered_name,
    };
    row.last_valid_check = record.valid.then(|| record.clone());
    row.last_check = Some(record);
    Ok(())
}
