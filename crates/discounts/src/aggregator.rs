//! The persisted shapes. Every variant is frozen once merged (`events.lock`):
//! a change of shape is a new variant, never a changed field. Each event
//! carries who did it, so the stream is its own audit trail. No field is an
//! enum, so the lock never freezes one.

/// A discount code of the tenant. Its id is a ULID.
#[evento::aggregate(name = "timada-discounts/Discount")]
pub enum Discount {
    /// Exactly one of `percent_bps` and `amount_cents` is set: a percentage
    /// in basis points (`1000` is 10 %), or minor units of `currency` off.
    Created {
        code: String,
        percent_bps: Option<u32>,
        amount_cents: Option<u64>,
        currency: Option<String>,
        /// Unix seconds; `None` is from the start.
        starts_at: Option<u64>,
        /// Unix seconds, excluded; `None` is no end.
        ends_at: Option<u64>,
        /// How many orders it serves at most; `None` is no limit.
        max_uses: Option<u32>,
        /// One order per shopper e-mail.
        once_per_email: bool,
        created_by: String,
    },
    /// Shoppers cannot use it until it is reactivated.
    Deactivated {
        deactivated_by: String,
    },
    Reactivated {
        reactivated_by: String,
    },
    /// Final: no more edits and its code is free again.
    Archived {
        archived_by: String,
    },
}

/// The index `code → discount`: one stream per code, with the deterministic
/// id [`discount_code_id`](crate::discount_code_id).
#[evento::aggregate(name = "timada-discounts/DiscountCode")]
pub enum DiscountCode {
    Claimed { discount_id: String },
    Released,
}

/// The uses of a code with a cap: one stream per discount, with the
/// deterministic id [`discount_usage_id`](crate::discount_usage_id). A cart
/// holds at most one use.
pub mod usage {
    #[evento::aggregate(name = "timada-discounts/DiscountUsage")]
    pub enum DiscountUsage {
        /// Held for a checkout until `expires_at` (unix seconds, excluded),
        /// unless the order consumes it first. Reserving again moves the
        /// expiry.
        Reserved {
            cart_id: String,
            email: String,
            expires_at: u64,
            reserved_by: String,
        },
        Released {
            cart_id: String,
            released_by: String,
        },
        /// The order the checkout made: the use is spent for good.
        Consumed {
            cart_id: String,
            order_id: String,
            consumed_by: String,
        },
    }
}

/// The use of a code good once per e-mail by one shopper: one stream per
/// discount and e-mail, with the deterministic id
/// [`discount_buyer_id`](crate::discount_buyer_id).
pub mod buyer {
    #[evento::aggregate(name = "timada-discounts/DiscountBuyer")]
    pub enum DiscountBuyer {
        Reserved {
            cart_id: String,
            expires_at: u64,
            reserved_by: String,
        },
        Released {
            cart_id: String,
            released_by: String,
        },
        Consumed {
            cart_id: String,
            order_id: String,
            consumed_by: String,
        },
    }
}
