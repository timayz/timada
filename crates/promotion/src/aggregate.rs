use timada_core::Money;

/// What a discount takes off the tax-inclusive cart total.
#[derive(Debug, Clone, Copy, PartialEq, Eq, bitcode::Encode, bitcode::Decode)]
pub enum DiscountKind {
    /// A share of the gross total, in basis points (1000 = 10 %).
    Percentage { bps: u32 },
    /// A fixed amount, clamped to the total so it can never go negative. Only
    /// applies to carts in its own currency.
    Fixed { amount: Money },
}

impl Default for DiscountKind {
    fn default() -> Self {
        Self::Percentage { bps: 0 }
    }
}

#[evento::aggregate]
pub enum Discount {
    /// A code came into being. `starts_at`/`ends_at` are epoch milliseconds;
    /// no `ends_at` means open-ended, no `usage_limit` means uncapped.
    DiscountCreated {
        code: String,
        kind: DiscountKind,
        starts_at: i64,
        ends_at: Option<i64>,
        usage_limit: Option<u32>,
    },
    /// Switched off by an admin. Terminal — a fresh campaign is a fresh code.
    DiscountDisabled,
}
