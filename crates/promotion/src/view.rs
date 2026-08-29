//! The write-side view of a discount, replayed from its events.

use evento::ProjectionAggregate;
use evento::metadata::Event;
use evento::projection::Projection;
use timada_core::Executor;

use crate::aggregate::{Discount, DiscountCreated, DiscountDisabled, DiscountKind};

#[evento::projection(bitcode::Encode, bitcode::Decode)]
#[derive(Debug, PartialEq, Eq)]
pub struct DiscountView {
    pub id: String,
    pub code: String,
    pub kind: DiscountKind,
    pub starts_at: i64,
    pub ends_at: Option<i64>,
    pub usage_limit: Option<u32>,
    pub disabled: bool,
}

impl ProjectionAggregate for DiscountView {
    fn aggregate_id(&self) -> String {
        self.id.to_owned()
    }
}

#[evento::handler]
async fn apply_created(
    event: Event<DiscountCreated>,
    view: &mut DiscountView,
) -> anyhow::Result<()> {
    view.id = event.aggregate_id.clone();
    view.code = event.data.code.clone();
    view.kind = event.data.kind;
    view.starts_at = event.data.starts_at;
    view.ends_at = event.data.ends_at;
    view.usage_limit = event.data.usage_limit;
    Ok(())
}

#[evento::handler]
async fn apply_disabled(
    _event: Event<DiscountDisabled>,
    view: &mut DiscountView,
) -> anyhow::Result<()> {
    view.disabled = true;
    Ok(())
}

/// Replay one discount. `None` means no such aggregate — look it up by
/// [`discount_id`](crate::discount_id) of the code.
pub async fn load_discount(
    executor: &Executor,
    discount_id: &str,
) -> anyhow::Result<Option<DiscountView>> {
    Projection::<_, DiscountView>::new::<Discount>()
        .handler(apply_created())
        .handler(apply_disabled())
        .strict()
        .load(discount_id)
        .execute(executor)
        .await
}
