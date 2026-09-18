use bitcode::{Decode, Encode};
use timada_core::Money;

#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub enum DeliveryKind {
    #[default]
    HomeDelivery,
    StorePickup {
        store_id: String,
    },
}

/// A delivery option offered at checkout ("Chronopost DOM", "Retrait en
/// boutique", ...). Resolved from the static catalogue below.
#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub struct DeliveryMethod {
    pub code: String,
    pub carrier: String,
    pub kind: DeliveryKind,
}

/// `(code, carrier, requires a pickup store, fee in EUR minor units)`.
const METHODS: &[(&str, &str, bool, i64)] = &[
    ("chronopost-dom", "Chronopost", false, 2_395),
    ("colissimo", "Colissimo", false, 590),
    ("store-pickup", "LDLC", true, 0),
];

impl DeliveryMethod {
    /// Looks a method up by code. Store pickup needs the store to collect
    /// from; without one there is no valid method.
    pub fn resolve(code: &str, pickup_store_id: Option<String>) -> Option<DeliveryMethod> {
        let (code, carrier, pickup, _) = METHODS.iter().find(|(c, ..)| *c == code)?;
        let kind = if *pickup {
            DeliveryKind::StorePickup {
                store_id: pickup_store_id?,
            }
        } else {
            DeliveryKind::HomeDelivery
        };
        Some(DeliveryMethod {
            code: (*code).to_owned(),
            carrier: (*carrier).to_owned(),
            kind,
        })
    }
}

/// A delivery method as offered on the checkout page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryOffer {
    pub code: &'static str,
    pub carrier: &'static str,
    /// "Retrait en boutique": the customer must pick a store.
    pub requires_pickup_store: bool,
    pub fee: Money,
}

/// Every delivery method the shop offers, in display order.
pub fn delivery_offers() -> Vec<DeliveryOffer> {
    METHODS
        .iter()
        .map(|(code, carrier, pickup, fee)| DeliveryOffer {
            code,
            carrier,
            requires_pickup_store: *pickup,
            fee: Money::eur(*fee),
        })
        .collect()
}

/// Shipping fee charged for a delivery method, if the code is known.
pub fn shipping_fee(code: &str) -> Option<Money> {
    METHODS
        .iter()
        .find(|(c, ..)| *c == code)
        .map(|(.., fee)| Money::eur(*fee))
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub struct ShipmentLine {
    pub product_id: String,
    pub quantity: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Encode, Decode)]
pub enum ShipmentStatus {
    #[default]
    Created,
    Dispatched,
    Delivered,
}
