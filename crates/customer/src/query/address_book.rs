//! The "Mes adresses" page: identity plus billing and delivery addresses,
//! folded from one `Customer` stream. Executor-backed snapshots via bitcode.

use evento::{Executor, metadata::Event, projection::Projection};
use timada_core::{Address, Civility};

use crate::{
    aggregator::{
        BillingAddressSet, Customer, CustomerEmailChanged, CustomerRegistered,
        DeliveryAddressAdded, DeliveryAddressChanged, DeliveryAddressRemoved,
        PreferredDeliveryAddressChosen,
    },
    value_object::DeliveryAddress,
};

#[evento::projection(bitcode::Encode, bitcode::Decode)]
#[derive(Debug, PartialEq)]
pub struct AddressBookView {
    pub customer_id: String,
    pub email: String,
    pub civility: Civility,
    pub first_name: String,
    pub last_name: String,
    pub billing: Option<Address>,
    pub deliveries: Vec<DeliveryAddress>,
}

impl AddressBookView {
    pub fn preferred_delivery(&self) -> Option<&DeliveryAddress> {
        self.deliveries.iter().find(|d| d.preferred)
    }
}

pub fn create_projection<E: Executor>() -> Projection<E, AddressBookView> {
    Projection::new::<Customer>()
        .handler(on_customer_registered())
        .handler(on_customer_email_changed())
        .handler(on_billing_address_set())
        .handler(on_delivery_address_added())
        .handler(on_delivery_address_changed())
        .handler(on_delivery_address_removed())
        .handler(on_preferred_delivery_address_chosen())
        .strict()
}

pub async fn load<E: Executor>(
    executor: &E,
    id: impl Into<String>,
) -> anyhow::Result<Option<AddressBookView>> {
    create_projection().load(id).execute(executor).await
}

#[evento::handler]
async fn on_customer_registered(
    event: Event<CustomerRegistered>,
    row: &mut AddressBookView,
) -> anyhow::Result<()> {
    row.customer_id = event.aggregate_id.to_owned();
    row.email = event.data.email;
    row.civility = event.data.civility;
    row.first_name = event.data.first_name;
    row.last_name = event.data.last_name;
    Ok(())
}

#[evento::handler]
async fn on_customer_email_changed(
    event: Event<CustomerEmailChanged>,
    row: &mut AddressBookView,
) -> anyhow::Result<()> {
    row.email = event.data.email;
    Ok(())
}

#[evento::handler]
async fn on_billing_address_set(
    event: Event<BillingAddressSet>,
    row: &mut AddressBookView,
) -> anyhow::Result<()> {
    row.billing = Some(event.data.address);
    Ok(())
}

#[evento::handler]
async fn on_delivery_address_added(
    event: Event<DeliveryAddressAdded>,
    row: &mut AddressBookView,
) -> anyhow::Result<()> {
    row.deliveries.push(DeliveryAddress {
        id: event.data.address_id,
        address: event.data.address,
        preferred: false,
    });
    Ok(())
}

#[evento::handler]
async fn on_delivery_address_changed(
    event: Event<DeliveryAddressChanged>,
    row: &mut AddressBookView,
) -> anyhow::Result<()> {
    if let Some(delivery) = row
        .deliveries
        .iter_mut()
        .find(|d| d.id == event.data.address_id)
    {
        delivery.address = event.data.address;
    }
    Ok(())
}

#[evento::handler]
async fn on_delivery_address_removed(
    event: Event<DeliveryAddressRemoved>,
    row: &mut AddressBookView,
) -> anyhow::Result<()> {
    row.deliveries.retain(|d| d.id != event.data.address_id);
    Ok(())
}

#[evento::handler]
async fn on_preferred_delivery_address_chosen(
    event: Event<PreferredDeliveryAddressChosen>,
    row: &mut AddressBookView,
) -> anyhow::Result<()> {
    for delivery in &mut row.deliveries {
        delivery.preferred = delivery.id == event.data.address_id;
    }
    Ok(())
}
