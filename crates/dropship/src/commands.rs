//! Write-side commands for [`SupplierOrder`](crate::aggregate::SupplierOrder).

use evento::AggregateExt as _;
use timada_core::Executor;

use crate::aggregate::{SupplierOrderConfirmed, SupplierOrderPlaced, SupplierOrderRejected};
use crate::registry::SupplierRegistry;
use crate::supplier::{SupplierLine, SupplierOrderRequest};

/// The id of the supplier order for `(order_id, supplier_id)`.
///
/// Deriving it instead of generating one means the fulfillment saga can replay
/// `forward_order` without keeping a mapping table, and a redelivered event
/// lands on the same aggregate.
pub fn supplier_order_id(order_id: &str, supplier_id: &str) -> String {
    evento::hash_ids(vec![order_id, supplier_id])
}

/// Forward one order's lines to one supplier and record what came back.
///
/// Returns the supplier-order aggregate id. A supplier refusal is *not* an
/// error here: it is recorded as `SupplierOrderRejected` so the saga can
/// compensate through the same event-driven path it uses for success. Failing
/// to resolve `supplier_id` in the registry is treated the same way — a
/// misconfigured registry must not make the saga's subscription retry forever.
///
/// Idempotent on `SupplierOrderPlaced`: calling it twice for the same
/// `(order_id, supplier_id)` returns the existing id without re-contacting the
/// supplier. (A crash between the placed event and the supplier's answer leaves
/// the aggregate unresolved; recovering that needs a sweeper, which this pass
/// does not have.)
#[tracing::instrument(skip(executor, registry, lines), fields(lines = lines.len()))]
pub async fn forward_order(
    executor: &Executor,
    registry: &SupplierRegistry,
    order_id: &str,
    supplier_id: &str,
    lines: Vec<SupplierLine>,
) -> anyhow::Result<String> {
    let id = supplier_order_id(order_id, supplier_id);

    if executor.has_event::<SupplierOrderPlaced>(&id).await? {
        tracing::debug!(supplier_order_id = %id, "order already forwarded, skipping");
        return Ok(id);
    }

    let request = SupplierOrderRequest {
        order_id: order_id.to_owned(),
        lines: lines.clone(),
    };

    evento::append(&id)
        .event(&SupplierOrderPlaced {
            order_id: order_id.to_owned(),
            supplier_id: supplier_id.to_owned(),
            lines,
        })
        .commit(executor)
        .await?;

    let outcome = match registry.get(supplier_id) {
        Ok(supplier) => supplier.place_order(&request).await,
        Err(source) => Err(source),
    };

    // `SupplierOrderPlaced` is version 1, so the resolution event is version 2.
    match outcome {
        Ok(confirmation) => {
            tracing::info!(
                supplier_order_id = %id,
                external_ref = %confirmation.external_ref,
                "supplier confirmed order"
            );
            evento::append(&id)
                .original_version(1)
                .event(&SupplierOrderConfirmed {
                    external_ref: confirmation.external_ref,
                })
                .commit(executor)
                .await?;
        }
        Err(source) => {
            tracing::warn!(supplier_order_id = %id, error = %source, "supplier rejected order");
            evento::append(&id)
                .original_version(1)
                .event(&SupplierOrderRejected {
                    reason: source.to_string(),
                })
                .commit(executor)
                .await?;
        }
    }

    Ok(id)
}
