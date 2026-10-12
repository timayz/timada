//! Events. Every variant and every field is frozen once it ships (bitcode is
//! positional); evolve by adding variants.

/// A merchant on the platform. Roles are free-form names so a tenant can
/// define its own later; `owner` is built in (see
/// [`ROLE_OWNER`](crate::ROLE_OWNER)).
#[evento::aggregate(name = "timada-tenancy/Tenant")]
pub enum Tenant {
    Created {
        slug: String,
        name: String,
        created_by: String,
    },
    Renamed {
        name: String,
    },
    /// A platform administrator took the tenant offline; its pages refuse
    /// and its database is not opened.
    Suspended {
        reason: String,
    },
    Resumed,
    MemberGranted {
        user_id: String,
        role: String,
    },
    MemberRoleChanged {
        user_id: String,
        role: String,
    },
    MemberRevoked {
        user_id: String,
    },
}

/// Index: `slug → tenant`. One stream per slug, id = [`tenant_slug_id`](crate::tenant_slug_id).
#[evento::aggregate(name = "timada-tenancy/TenantSlug")]
pub enum TenantSlug {
    Claimed { tenant_id: String },
    Released,
}
