//! Events. Every variant and every field is frozen once it ships (bitcode is
//! positional); evolve by adding variants.

use crate::value_object::PricingRule;

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
    /// How shoppers pay this tenant: the merchant's own account with the
    /// provider. The secrets are sealed by the host before they get here
    /// (the crate never sees them in clear), so the event log holds none.
    PaymentsConfigured {
        /// `stripe`.
        provider: String,
        /// Public: what a shopper's browser loads the card form with.
        publishable_key: String,
        secret_key_sealed: Vec<u8>,
        /// What the provider signs the payment notices with.
        webhook_secret_sealed: Vec<u8>,
        configured_by: String,
    },
    PaymentsRemoved {
        removed_by: String,
    },
    /// The tenant sells as this company ([`company`]): its invoices name
    /// the company and are numbered in the company's series `series_code`
    /// (`""` is the plain series, `F2026-…`; `PRO` gives `PRO-F2026-…`).
    CompanyLinked {
        company_id: String,
        series_code: String,
        linked_by: String,
    },
    /// The tenant stops selling until linked again.
    CompanyUnlinked {
        unlinked_by: String,
    },
    /// The merchant's own mail server, which the shop's mails to its
    /// shoppers go out through, from `from_address`. The password is sealed
    /// by the host, as the payment secrets are.
    MailServerConfigured {
        host: String,
        port: u16,
        /// `tls` (implicit TLS), `starttls` or `none`.
        security: String,
        username: Option<String>,
        password_sealed: Option<Vec<u8>>,
        from_address: String,
        /// The display name; `None` is the tenant's name.
        from_name: Option<String>,
        configured_by: String,
    },
    /// The shop's mails go out through the platform's server again.
    MailServerRemoved {
        removed_by: String,
    },
    /// One more of the merchant's own mail servers, `server_id` among the
    /// shop's (a ULID). The two events above are the one server a shop had
    /// before it could have several. The password is sealed by the host.
    MailServerAdded {
        server_id: String,
        /// The merchant's name for it; `None` shows it by its address.
        name: Option<String>,
        host: String,
        port: u16,
        /// `tls` (implicit TLS), `starttls` or `none`.
        security: String,
        username: Option<String>,
        password_sealed: Option<Vec<u8>>,
        from_address: String,
        /// The display name; `None` is the tenant's name.
        from_name: Option<String>,
        added_by: String,
    },
    /// Replaces everything one of the shop's servers was.
    MailServerChanged {
        server_id: String,
        name: Option<String>,
        host: String,
        port: u16,
        security: String,
        username: Option<String>,
        password_sealed: Option<Vec<u8>>,
        from_address: String,
        from_name: Option<String>,
        changed_by: String,
    },
    /// One of the shop's servers is gone; what it was picked for goes back
    /// to no server picked.
    MailServerDeleted {
        server_id: String,
        deleted_by: String,
    },
    /// Which of the shop's servers sends what. `shop_mails`: its mails to
    /// its shoppers (`None`: the platform's server, under the shop's name).
    /// `marketing`: its newsletter campaigns and automated mails (`None`:
    /// none go out, they never go through the platform's server).
    MailServersPicked {
        shop_mails: Option<String>,
        marketing: Option<String>,
        picked_by: String,
    },
    /// The merchant's own wording of one of the shop's mails to its
    /// shoppers. `mail` is a [`ShopMailKind`](crate::ShopMailKind) key;
    /// `subject` and `html` are templates over that mail's variables.
    MailTemplateSaved {
        mail: String,
        subject: String,
        html: String,
        saved_by: String,
    },
    /// The merchant's own plain-text part of a mail, committed right after
    /// the [`MailTemplateSaved`] it goes with. A save without it sends the
    /// text derived from the HTML. `text` is a template over the same
    /// variables, printed unescaped.
    MailTemplateTextSaved {
        mail: String,
        text: String,
    },
    /// The mail goes back to the built-in wording.
    MailTemplateReset {
        mail: String,
        reset_by: String,
    },
    /// The languages the shop mails in, as ISO 639-1 codes: a shopper is
    /// mailed in theirs when it is one of `others` and worded, else in
    /// `default`. A shop that never set them mails in English, and the
    /// wordings saved before this (the three events above) are English.
    MailLanguagesSet {
        default: String,
        others: Vec<String>,
        set_by: String,
    },
    /// The merchant's wording of one mail in one of the shop's languages;
    /// see [`MailTemplateSaved`].
    MailTemplateSavedIn {
        mail: String,
        language: String,
        subject: String,
        html: String,
        saved_by: String,
    },
    /// The plain-text part of a wording in one language, committed right
    /// after its [`MailTemplateSavedIn`]; see [`MailTemplateTextSaved`].
    MailTemplateTextSavedIn {
        mail: String,
        language: String,
        text: String,
    },
    /// The mail, in that language, goes back to the built-in wording.
    MailTemplateResetIn {
        mail: String,
        language: String,
        reset_by: String,
    },
    /// The look all the shop's mails share. `logo_file` is a picture in the
    /// tenant's media (`<id>.<ext>`, `logo_size` bytes); `accent` is a
    /// `#rrggbb` colour; `footer` is plain text. Each is `None` when unset.
    MailBrandingSet {
        logo_file: Option<String>,
        logo_size: u64,
        accent: Option<String>,
        footer: Option<String>,
        set_by: String,
    },
    /// The mails go back to the plain look.
    MailBrandingCleared {
        cleared_by: String,
    },
    /// The shop imports products from AliExpress through the platform's
    /// app, acting for the AliExpress account of the company it sells as.
    /// Products are fetched as shipped to `ship_to_country`, priced in
    /// `currency` and worded in `language`; `pricing` turns what AliExpress
    /// charges into the shop's price (`None`: the merchant prices by hand).
    AliexpressConfigured {
        ship_to_country: String,
        currency: String,
        language: String,
        pricing: Option<PricingRule>,
        configured_by: String,
    },
    /// The shop no longer imports from AliExpress.
    AliexpressRemoved {
        removed_by: String,
    },
    /// What happens to the shop's paid orders with AliExpress items:
    /// `auto_forward` places them on AliExpress as they come (else the
    /// merchant does, one by one), and `try_to_pay` asks AliExpress to
    /// charge the account's saved payment method (else the merchant pays
    /// in AliExpress).
    AliexpressForwardingSet {
        auto_forward: bool,
        try_to_pay: bool,
        set_by: String,
    },
    /// Where the shop lives: an IANA name (`Europe/Paris`). Its hours are
    /// the ones a campaign keeps for subscribers whose own zone is unknown.
    TimeZoneSet {
        time_zone: String,
        set_by: String,
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
    /// the stream knows its hash and a short hint to tell keys apart. What a
    /// key may do is its scopes (`Scoped`); a key created before scopes
    /// existed has none recorded and keeps the full access it was issued
    /// with (see [`LEGACY_API_KEY_SCOPES`](crate::LEGACY_API_KEY_SCOPES)).
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
        /// The whole scope set of a key, replaced: recorded with `Created`
        /// and again on every change. Scopes are `resource:action` strings
        /// from [`API_KEY_SCOPES`](crate::API_KEY_SCOPES), never an enum, so
        /// the vocabulary can grow without a new event.
        Scoped {
            scopes: Vec<String>,
            scoped_by: String,
            /// Unix seconds.
            scoped_at: u64,
        },
    }
}

/// A company: who sells in law, behind one or more tenants. Its own owners
/// manage it. Its id is a ULID; its SIREN is unique across the platform
/// ([`company_siren`]).
pub mod company {
    #[evento::aggregate(name = "timada-tenancy/Company")]
    pub enum Company {
        Created {
            /// The nine digits every SIRET of the company starts with.
            siren: String,
            created_by: String,
        },
        /// What its invoices must say about it, replaced as a whole. An
        /// invoice copies it when issued, so a change never rewrites one.
        IdentitySet {
            /// The registered name (raison sociale), or the trader's own name.
            legal_name: String,
            /// `SAS`, `SARL`, `EI`…
            legal_form: String,
            /// In euro cents; `None` for a form without one.
            share_capital_cents: Option<u64>,
            /// 14 digits, starting with the SIREN.
            siret: String,
            /// The city of the trade register (RCS) it is entered in, if any.
            rcs_city: Option<String>,
            line1: String,
            line2: Option<String>,
            postal_code: String,
            city: String,
            /// ISO 3166-1 alpha-2, uppercase.
            country: String,
            contact_email: String,
            /// `None` under the VAT exemption for small businesses.
            vat_number: Option<String>,
            /// Registered for the EU One-Stop Shop. Needs a `vat_number`.
            oss_registered: bool,
            set_by: String,
        },
        OwnerAdded {
            user_id: String,
            added_by: String,
        },
        OwnerRemoved {
            user_id: String,
            removed_by: String,
        },
        /// The company's AliExpress account said yes to the platform's app:
        /// its tokens, sealed by the host, and when they lapse (unix
        /// seconds). Written again each time they are refreshed. Every
        /// tenant selling as the company imports and orders through it.
        AliexpressAuthorized {
            access_token_sealed: Vec<u8>,
            refresh_token_sealed: Vec<u8>,
            expires_at: u64,
            refresh_expires_at: u64,
            account: String,
            authorized_by: String,
        },
        /// The account is forgotten: its tenants import nothing until
        /// another says yes.
        AliexpressDisconnected {
            disconnected_by: String,
        },
    }
}

/// Index: `SIREN → company`, one stream per SIREN, id =
/// [`company_siren_id`](crate::company_siren_id). A platform administrator
/// releases a SIREN someone claimed who is not that company.
pub mod company_siren {
    #[evento::aggregate(name = "timada-tenancy/CompanySiren")]
    pub enum CompanySiren {
        Claimed { company_id: String },
        Released { released_by: String },
    }
}

/// Index: `(company, series code) → tenant`, id =
/// [`company_series_id`](crate::company_series_id). Never released: a code
/// that numbered invoices never numbers another tenant's.
pub mod company_series {
    #[evento::aggregate(name = "timada-tenancy/CompanySeries")]
    pub enum CompanySeries {
        Claimed { tenant_id: String },
    }
}
