use timada_order::OrderStatus;
use topcoat::{
    Result,
    view::{View, component, view},
};

use crate::components::badge::{BadgeVariant, badge};

pub use timada_core::format::{date, money, vat_rate};

/// Where an order stands, as a badge.
///
/// The variants read as a progression rather than as decoration: an order
/// waiting is quiet, money received is a success, a parcel on its way is in
/// progress, and a cancellation is the one thing an operator should catch from
/// across the table.
#[component]
pub async fn order_status_badge(status: OrderStatus) -> Result<impl View> {
    let (variant, label) = match status {
        OrderStatus::Placed => (BadgeVariant::Secondary, "En attente"),
        OrderStatus::Paid => (BadgeVariant::Success, "Payée"),
        OrderStatus::Shipped => (BadgeVariant::Info, "Expédiée"),
        OrderStatus::Cancelled => (BadgeVariant::Destructive, "Annulée"),
    };
    Ok(view! { badge(variant: variant, (label)) })
}
