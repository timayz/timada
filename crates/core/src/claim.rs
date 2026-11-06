//! Claims on the index streams that make a value mean one thing in a
//! tenant: a slug, a stock code, a discount code. A command claims the
//! value, then writes its holder; a subscription gives the value back once
//! the holder lets go of it. A node may die between any two of those
//! writes, so a claim never waits on its release: a holder gone for good,
//! or moved on from the value, loses it to the next claim.

/// How long a claim counts as live while its holder has written nothing
/// past it: the holder may be committing it right now. Past this, a node
/// died between the claim and the holder's write, and the value is free.
pub const CLAIM_GRACE_SECS: u64 = 300;

/// Whether a claim made at `claimed_at` (unix seconds) may still be
/// committing at `now`.
pub fn claim_is_fresh(claimed_at: u64, now: u64) -> bool {
    now.saturating_sub(claimed_at) < CLAIM_GRACE_SECS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_claim_stays_fresh_for_the_grace_only() {
        assert!(claim_is_fresh(1_000, 1_000));
        assert!(claim_is_fresh(1_000, 1_000 + CLAIM_GRACE_SECS - 1));
        assert!(!claim_is_fresh(1_000, 1_000 + CLAIM_GRACE_SECS));
        // A clock behind the claim's stamp keeps it fresh.
        assert!(claim_is_fresh(1_000, 900));
    }
}
