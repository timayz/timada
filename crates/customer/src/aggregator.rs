use timada_core::{Address, Civility};

// The explicit name pins the on-disk identity: renaming the crate or the enum
// must never orphan stored events.
#[evento::aggregate(name = "timada-customer/Customer")]
pub enum Customer {
    /// A customer account was created.
    CustomerRegistered {
        email: String,
        civility: Civility,
        first_name: String,
        last_name: String,
    },

    CustomerEmailChanged {
        email: String,
    },

    /// The single billing address was set or replaced.
    BillingAddressSet {
        address: Address,
    },

    DeliveryAddressAdded {
        address_id: String,
        address: Address,
    },

    DeliveryAddressChanged {
        address_id: String,
        address: Address,
    },

    DeliveryAddressRemoved {
        address_id: String,
    },

    /// The delivery address pre-selected for the next orders.
    PreferredDeliveryAddressChosen {
        address_id: String,
    },
}
