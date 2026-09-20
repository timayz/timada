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
    ("colissimo-europe", "Colissimo", false, 1_290),
    ("store-pickup", "LDLC", true, 0),
];

impl DeliveryMethod {
    /// Whether `code` names a delivery method at all.
    pub fn is_known(code: &str) -> bool {
        METHODS.iter().any(|(known, ..)| *known == code)
    }

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

/// Every delivery method the shop offers, in display order, at its euro fee.
pub fn delivery_offers() -> Vec<DeliveryOffer> {
    DeliveryFees::default().offers(Money::EUR)
}

/// Shipping fee charged in euros for a delivery method, if the code is known.
pub fn shipping_fee(code: &str) -> Option<Money> {
    DeliveryFees::default().fee(code, Money::EUR)
}

/// What each delivery method costs, **per currency** — a host value. The
/// built-in euro fees are the `Default`; a shop selling in other currencies
/// says what each method costs there. Nothing is converted: a method without
/// a fee in a currency is not offered to a cart in that currency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryFees {
    /// `(method code, fee)`, at most one per method and currency.
    fees: Vec<(String, Money)>,
}

impl Default for DeliveryFees {
    fn default() -> Self {
        Self {
            fees: METHODS
                .iter()
                .map(|(code, .., fee)| ((*code).to_owned(), Money::eur(*fee)))
                .collect(),
        }
    }
}

impl DeliveryFees {
    /// No fee at all: not even the built-in euro ones.
    pub fn none() -> Self {
        Self { fees: Vec::new() }
    }

    /// Sets what `code` costs in `fee`'s currency, replacing what it cost
    /// there. An unknown method or a negative fee is ignored with a warning:
    /// a host's configuration never brings a shop down.
    pub fn with_fee(mut self, code: &str, fee: Money) -> Self {
        if fee.is_negative() || !DeliveryMethod::is_known(code) {
            tracing::warn!(%code, ?fee, "delivery fee ignored: unknown method or negative amount");
            return self;
        }
        self.fees
            .retain(|(known, money)| !(known == code && money.currency == fee.currency));
        self.fees.push((code.to_owned(), fee));
        self
    }

    /// What `code` costs in `currency`; `None` when the method is unknown or
    /// not offered in that currency.
    pub fn fee(&self, code: &str, currency: &str) -> Option<Money> {
        self.fees
            .iter()
            .find(|(known, fee)| known == code && fee.currency == currency)
            .map(|(_, fee)| fee.clone())
    }

    /// The methods offered to a cart in `currency`, in display order.
    pub fn offers(&self, currency: &str) -> Vec<DeliveryOffer> {
        METHODS
            .iter()
            .filter_map(|(code, carrier, pickup, _)| {
                Some(DeliveryOffer {
                    code,
                    carrier,
                    requires_pickup_store: *pickup,
                    fee: self.fee(code, currency)?,
                })
            })
            .collect()
    }
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
    Cancelled,
}
