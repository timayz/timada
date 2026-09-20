mod add_delivery_address;
mod change_delivery_address;
mod change_email;
mod choose_preferred_delivery_address;
mod company;
mod register_customer;
mod remove_delivery_address;
mod set_billing_address;

use std::ops::Deref;

pub use register_customer::RegisterCustomer;

use evento::{Executor, Projection, metadata::Event};

use crate::{
    aggregator::{
        BillingAddressSet, CompanyIdentified, CompanyIdentityRemoved, Customer,
        CustomerEmailChanged, CustomerRegistered, DeliveryAddressAdded, DeliveryAddressChanged,
        DeliveryAddressRemoved, PreferredDeliveryAddressChosen, VatNumberChecked,
    },
    error::CustomerError,
};

/// Deterministic address id: the n-th address added by a customer.
pub fn address_id(customer_id: &str, seq: u32) -> String {
    timada_core::id::derived(&[customer_id, &seq.to_string()], "address")
}

pub struct Command<'a, E: Executor>(pub &'a E);

impl<E: Executor> Deref for Command<'_, E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl<E: Executor> Command<'_, E> {
    pub async fn load(&self, id: impl Into<String>) -> anyhow::Result<Option<CustomerState>> {
        create_projection().load(id).execute(self.0).await
    }

    /// Loads a customer that must exist.
    async fn load_existing(&self, id: impl Into<String>) -> Result<CustomerState, CustomerError> {
        self.load(id).await?.ok_or(CustomerError::CustomerNotFound)
    }
}

/// Write-side state: just enough to guard the commands.
#[evento::projection(id = id)]
#[evento::snapshot(none)]
pub struct CustomerState {
    pub id: String,
    pub next_address_seq: u32,
    pub delivery_ids: Vec<String>,
    pub preferred: Option<String>,
    /// `(company name, VAT number)` while the customer buys as a business.
    pub company: Option<(String, String)>,
}

impl CustomerState {
    pub fn has_delivery_address(&self, address_id: &str) -> bool {
        self.delivery_ids.iter().any(|id| id == address_id)
    }
}

// Strict + explicit skips: a non-strict projection only *reads* the events it
// handles, so the version it observes (and `write()` relies on) would go stale.
fn create_projection<E: Executor>() -> Projection<E, CustomerState> {
    Projection::new::<Customer>()
        .handler(on_customer_registered())
        .handler(on_delivery_address_added())
        .handler(on_delivery_address_removed())
        .handler(on_preferred_delivery_address_chosen())
        .handler(on_company_identified())
        .handler(on_company_identity_removed())
        .skip::<VatNumberChecked>()
        .skip::<CustomerEmailChanged>()
        .skip::<BillingAddressSet>()
        .skip::<DeliveryAddressChanged>()
        .strict()
}

#[evento::handler]
async fn on_customer_registered(
    event: Event<CustomerRegistered>,
    row: &mut CustomerState,
) -> anyhow::Result<()> {
    row.id = event.aggregate_id.to_owned();
    row.next_address_seq = 1;
    Ok(())
}

#[evento::handler]
async fn on_delivery_address_added(
    event: Event<DeliveryAddressAdded>,
    row: &mut CustomerState,
) -> anyhow::Result<()> {
    row.delivery_ids.push(event.data.address_id);
    row.next_address_seq += 1;
    Ok(())
}

#[evento::handler]
async fn on_delivery_address_removed(
    event: Event<DeliveryAddressRemoved>,
    row: &mut CustomerState,
) -> anyhow::Result<()> {
    row.delivery_ids.retain(|id| *id != event.data.address_id);
    if row.preferred.as_deref() == Some(event.data.address_id.as_str()) {
        row.preferred = None;
    }
    Ok(())
}

#[evento::handler]
async fn on_preferred_delivery_address_chosen(
    event: Event<PreferredDeliveryAddressChosen>,
    row: &mut CustomerState,
) -> anyhow::Result<()> {
    row.preferred = Some(event.data.address_id);
    Ok(())
}

#[evento::handler]
async fn on_company_identified(
    event: Event<CompanyIdentified>,
    row: &mut CustomerState,
) -> anyhow::Result<()> {
    row.company = Some((event.data.company_name, event.data.vat_number));
    Ok(())
}

#[evento::handler]
async fn on_company_identity_removed(
    _event: Event<CompanyIdentityRemoved>,
    row: &mut CustomerState,
) -> anyhow::Result<()> {
    row.company = None;
    Ok(())
}
