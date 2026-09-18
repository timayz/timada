use timada_core::Money;

use crate::value_object::{DiscountKind, VoucherKind};

#[evento::aggregate(name = "timada-promotion/Discount")]
pub enum Discount {
    /// A promo code was created.
    DiscountCreated {
        code: String,
        kind: DiscountKind,
        max_redemptions: Option<u32>,
        valid_until: Option<u64>,
    },

    /// The code was used on an order (the cap is enforced in SQL first).
    DiscountRedeemed { order_id: String },

    /// The code can no longer be used.
    DiscountDeactivated,
}

#[evento::aggregate(name = "timada-promotion/Voucher")]
pub enum Voucher {
    /// A gift voucher or credit note was issued.
    VoucherIssued {
        code: String,
        customer_id: Option<String>,
        value: Money,
        kind: VoucherKind,
        expires_at: Option<u64>,
    },

    /// Part of the balance was spent on an order.
    VoucherRedeemed { order_id: String, amount: Money },

    /// The remaining balance was cancelled.
    VoucherCancelled { reason: String },
}
