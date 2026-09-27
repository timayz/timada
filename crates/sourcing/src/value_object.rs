use bitcode::{Decode, Encode};
use timada_core::Money;

use crate::connector::SupplierItemRef;

/// One line of what the shop buys from a supplier: its own product, what the
/// supplier calls it, how many, and what one was quoted at.
#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub struct SupplierOrderLine {
    pub product_id: String,
    pub external_item_id: String,
    pub external_sku: Option<String>,
    pub quantity: u32,
    /// What the supplier quoted for one unit, in its own currency.
    pub unit_cost: Money,
}

impl SupplierOrderLine {
    /// What the line comes to at the quoted cost.
    pub fn total(&self) -> Result<Money, timada_core::MoneyError> {
        self.unit_cost.checked_mul(self.quantity)
    }

    /// How the connector is told which item this is.
    pub fn item(&self) -> SupplierItemRef {
        SupplierItemRef::new(self.external_item_id.clone(), self.external_sku.clone())
    }
}

/// Where a purchase stands. Folded from the events, and carried by the
/// snapshotted view — so it is a view shape, free to grow with a
/// `.revision(n)` bump, unlike an enum nested in an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Encode, Decode)]
pub enum SupplierOrderStatus {
    /// Waiting for somebody to buy it.
    #[default]
    Drafted,
    Placed,
    Shipped,
    Refused,
    Cancelled,
}

impl SupplierOrderStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Drafted => "drafted",
            Self::Placed => "placed",
            Self::Shipped => "shipped",
            Self::Refused => "refused",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "drafted" => Some(Self::Drafted),
            "placed" => Some(Self::Placed),
            "shipped" => Some(Self::Shipped),
            "refused" => Some(Self::Refused),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Drafted => "À commander",
            Self::Placed => "Commandé",
            Self::Shipped => "Expédié",
            Self::Refused => "Refusé",
            Self::Cancelled => "Annulé",
        }
    }

    /// Whether there is still something for the shop to do about it.
    pub fn is_open(self) -> bool {
        matches!(self, Self::Drafted | Self::Placed)
    }
}
