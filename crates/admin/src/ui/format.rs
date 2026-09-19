use timada_order::OrderStatus;
use topcoat::{
    Result,
    view::{View, component, view},
};

use crate::components::badge::{BadgeVariant, badge};

pub use timada_core::format::{date, money, vat_rate};

#[component]
pub async fn order_status_badge(status: OrderStatus) -> Result<impl View> {
    let (variant, label) = match status {
        OrderStatus::Placed => (BadgeVariant::Secondary, "En attente"),
        OrderStatus::Paid => (BadgeVariant::Primary, "Payée"),
        OrderStatus::Shipped => (BadgeVariant::Outline, "Expédiée"),
        OrderStatus::Cancelled => (BadgeVariant::Destructive, "Annulée"),
    };
    Ok(view! { badge(variant: variant, (label)) })
}
