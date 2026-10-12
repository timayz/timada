//! Events. Every variant and every field is frozen once it ships (bitcode is
//! positional); evolve by adding variants.

/// A merchant on the platform. Roles are free-form names a tenant defines
/// for itself (`RoleDefined`); `owner` is built in and unrestricted (see
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
    /// A custom role with the permissions it grants, as `resource:action`
    /// strings from [`PERMISSIONS`](crate::PERMISSIONS).
    RoleDefined {
        role: String,
        label: String,
        permissions: Vec<String>,
    },
    /// The label and the whole permission set of a role, replaced.
    RoleUpdated {
        role: String,
        label: String,
        permissions: Vec<String>,
    },
    /// Only a role no member holds is removed.
    RoleRemoved {
        role: String,
    },
}

/// Index: `slug → tenant`. One stream per slug, id = [`tenant_slug_id`](crate::tenant_slug_id).
#[evento::aggregate(name = "timada-tenancy/TenantSlug")]
pub enum TenantSlug {
    Claimed { tenant_id: String },
    Released,
}

/// An invitation to join a tenant, mailed as a link carrying a random token.
/// One stream per token, id = [`invitation_id`](crate::invitation_id): the
/// token never leaves the e-mail, the stream only knows its hash. Whoever
/// opens the link while signed in joins (the token is the credential).
#[evento::aggregate(name = "timada-tenancy/Invitation")]
pub enum Invitation {
    Sent {
        tenant_id: String,
        /// The address the link was mailed to (normalized), for the pages.
        email: String,
        role: String,
        invited_by: String,
        /// Unix seconds.
        sent_at: u64,
        /// Unix seconds; the link is refused from then on.
        expires_at: u64,
    },
    Accepted {
        user_id: String,
        accepted_at: u64,
    },
    Revoked {
        revoked_by: String,
    },
}

/// The store API key events, in their own module: the macro names an event
/// struct after its variant, and `Created` / `Revoked` already belong to the
/// tenant and the invitation above.
pub mod api_key {
    /// A store's credential to the tenant's event store, presented when a
    /// storefront connects. One stream per token, id =
    /// [`api_key_id`](crate::api_key_id): the token is shown once at creation,
    /// the stream knows its hash and a short hint to tell keys apart. A key
    /// reads and writes everything in its tenant's store; revoking it is the
    /// only way to narrow that.
    #[evento::aggregate(name = "timada-tenancy/ApiKey")]
    pub enum ApiKey {
        Created {
            tenant_id: String,
            label: String,
            /// The token's first characters (`tk_` and eight more), for the pages.
            hint: String,
            created_by: String,
            /// Unix seconds.
            created_at: u64,
        },
        Revoked {
            revoked_by: String,
            revoked_at: u64,
        },
    }
}
