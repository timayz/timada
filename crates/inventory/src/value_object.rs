use bitcode::{Decode, Encode};

/// Where units are physically held. Store stock is what "Retrait en boutique"
/// looks at; the warehouse serves home delivery.
#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub enum StockLocation {
    #[default]
    Warehouse,
    Store {
        store_id: String,
    },
}

impl StockLocation {
    /// Stable key used in derived stock item ids.
    pub fn key(&self) -> String {
        match self {
            Self::Warehouse => "warehouse".to_owned(),
            Self::Store { store_id } => format!("store:{store_id}"),
        }
    }

    pub fn is_warehouse(&self) -> bool {
        matches!(self, Self::Warehouse)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Encode, Decode)]
pub enum Availability {
    InStock,
    #[default]
    OutOfStock,
}

/// Result of a reservation attempt; both outcomes are recorded as events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReservationOutcome {
    Reserved,
    Rejected { available: u32 },
}
