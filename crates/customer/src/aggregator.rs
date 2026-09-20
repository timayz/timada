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

    /// The customer buys as a business: its name and VAT number, as its
    /// invoices must show them. Said again to change either.
    CompanyIdentified {
        company_name: String,
        /// Compact, with its country prefix: `DE123456789`.
        vat_number: String,
    },

    /// The customer buys as a consumer again.
    CompanyIdentityRemoved,

    /// The VAT registry answered about the company's number. The consultation
    /// number, when the registry gave one, is the proof of the check. A
    /// registry that could not answer leaves no event.
    VatNumberChecked {
        vat_number: String,
        valid: bool,
        consultation_ref: Option<String>,
        registered_name: Option<String>,
    },
}
