//! The persisted shapes. Every variant is frozen once merged (`events.lock`):
//! a change of shape is a new variant, never a changed field. Each event
//! carries who did it, so the stream is its own audit trail.

/// A product of the tenant's catalog. Its id is a ULID.
#[evento::aggregate(name = "timada-catalog/Product")]
pub enum Product {
    Created {
        slug: String,
        name: String,
        created_by: String,
    },
    Renamed {
        name: String,
        renamed_by: String,
    },
    Described {
        description: String,
        described_by: String,
    },
    /// The price in minor units of an ISO-4217 currency (`1250` + `EUR` is
    /// 12.50 €).
    Priced {
        amount_cents: u64,
        currency: String,
        priced_by: String,
    },
    Published {
        published_by: String,
    },
    Unpublished {
        unpublished_by: String,
    },
    /// Final: an archived product takes no more edits and its slug is free
    /// again.
    Archived {
        archived_by: String,
    },
}

/// The index `slug → product`: one stream per slug, with the deterministic id
/// [`product_slug_id`](crate::product_slug_id).
#[evento::aggregate(name = "timada-catalog/ProductSlug")]
pub enum ProductSlug {
    Claimed { product_id: String },
    Released,
}
