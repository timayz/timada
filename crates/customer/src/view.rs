//! The write-side view of a customer, replayed from its events.

use evento::ProjectionAggregate;
use evento::metadata::Event;
use evento::projection::Projection;
use timada_core::Executor;

use crate::aggregate::{Customer, CustomerRegistered};

#[evento::projection(bitcode::Encode, bitcode::Decode)]
#[derive(Debug, PartialEq, Eq)]
pub struct CustomerView {
    pub id: String,
    pub email: String,
    pub full_name: String,
}

impl ProjectionAggregate for CustomerView {
    fn aggregate_id(&self) -> String {
        self.id.to_owned()
    }
}

#[evento::handler]
async fn apply_registered(
    event: Event<CustomerRegistered>,
    view: &mut CustomerView,
) -> anyhow::Result<()> {
    view.id = event.aggregate_id.clone();
    view.email = event.data.email.clone();
    view.full_name = event.data.full_name.clone();
    Ok(())
}

/// Replay one customer. `None` means no such aggregate.
pub async fn load_customer(
    executor: &Executor,
    customer_id: &str,
) -> anyhow::Result<Option<CustomerView>> {
    Projection::<_, CustomerView>::new::<Customer>()
        .handler(apply_registered())
        .strict()
        .load(customer_id)
        .execute(executor)
        .await
}
