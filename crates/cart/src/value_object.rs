use bitcode::{Decode, Encode};
use timada_core::Money;

/// A cart line as shown in "votre panier".
#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub struct CartLine {
    pub product_id: String,
    pub name: String,
    pub quantity: u32,
    pub unit_price: Money,
    pub warranty_months: u16,
}

/// The delivery option picked at checkout; `pickup_store_id` is set for
/// "retrait en boutique".
#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub struct DeliveryChoice {
    pub method_code: String,
    pub pickup_store_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub enum PaymentMode {
    #[default]
    Card,
    /// "Paiement en 3 fois" and friends.
    Installments { count: u8 },
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub enum CartStatus {
    #[default]
    Open,
    Saved,
    CheckedOut,
}
