use bitcode::{Decode, Encode};
use timada_core::Address;

/// A delivery address as listed in the address book.
#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub struct DeliveryAddress {
    pub id: String,
    pub address: Address,
    pub preferred: bool,
}
