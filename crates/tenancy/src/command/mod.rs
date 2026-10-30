//! Write side: the command gateway and the strict write models it loads.
//!
//! Every write model folds *all* events of its aggregate (`strict()`), so the
//! version it observes, and `write()` sends as `original_version`, never goes
//! stale. None of them is snapshotted: a tenant stream is a handful of events.

mod aliexpress;
mod api_key;
mod company;
mod create;
mod invite;
mod legal;
mod mail;
mod member;
mod payments;
mod rename;
mod role;
mod slug_index;
mod suspend;

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Deref;

pub use aliexpress::*;
pub use api_key::*;
pub use company::*;
pub use create::*;
pub use invite::*;
pub use legal::*;
pub use mail::*;
pub use payments::*;
pub use role::*;

use evento::{Executor, Projection, metadata::Event};

use crate::aggregator::api_key::{
    Created as ApiKeyCreated, Revoked as ApiKeyRevoked, Scoped as ApiKeyScoped,
};
use crate::aggregator::{
    Accepted, Claimed, CompanyLinked, CompanyUnlinked, Created, MailBrandingCleared,
    MailBrandingSet, MailLanguagesSet, MailServerAdded, MailServerChanged, MailServerDeleted,
    MailServersPicked, MailTemplateReset, MailTemplateSaved, MailTemplateTextSaved, MemberGranted,
    MemberRevoked, MemberRoleChanged, PaymentsConfigured, PaymentsRemoved, Released, Renamed,
    Resumed, Revoked, RoleDefined, RoleRemoved, RoleUpdated, Sent, Suspended,
};
use crate::value_object::{
    ROLE_OWNER, api_key_id, api_key_scope_implies, implies, invitation_id, tenant_slug_id,
};

/// The command gateway. Cheap to clone when `E` is (the dashboard's executor
/// is an `Arc` wrapper).
pub struct Command<E: Executor>(pub E);

impl<E: Executor> Deref for Command<E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

// =============================================================================
// Tenant
// =============================================================================

/// The zone of a shop that set none.
pub const DEFAULT_TIME_ZONE: &str = "UTC";

#[evento::projection(id = id, name = "timada-tenancy/TenantState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct TenantState {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub created_by: String,
    /// Unix seconds of `Created`.
    pub created_at: u64,
    /// The reason given when the tenant was suspended; `None` while active.
    pub suspended: Option<String>,
    /// `user id → role`, sorted so pages are stable.
    pub members: BTreeMap<String, String>,
    /// The custom roles, by name. `owner` is not among them: it is implicit
    /// and grants everything.
    pub roles: BTreeMap<String, RoleDef>,
    /// How shoppers pay, once set up.
    pub payments: Option<PaymentSettings>,
    /// The company it sells as, once linked.
    pub company: Option<TenantCompany>,
    /// The merchant's own mail servers, in the order they were added.
    pub mail_servers: Vec<MailServerSettings>,
    /// The server the shop's mails to its shoppers go out through; `None`
    /// is the platform's. Read it through [`TenantState::shop_mails_server`].
    pub shop_mails_server_id: Option<String>,
    /// The server the shop's marketing goes out through; `None`: no
    /// marketing goes out. Read it through [`TenantState::marketing_server`].
    pub marketing_server_id: Option<String>,
    /// The languages the shop mails in, once picked; English until then.
    pub mail_languages: Option<MailLanguages>,
    /// The merchant's wording of the shop's mails, by
    /// [`ShopMailKind`](crate::ShopMailKind) key and language code. A mail
    /// not here is sent in the built-in wording.
    pub mail_templates: BTreeMap<(String, String), MailTemplate>,
    /// Who changed the mails' wording and when, newest last; the last
    /// [`MAIL_TEMPLATE_HISTORY`] changes of each mail in each language.
    pub mail_template_history: Vec<MailTemplateChange>,
    /// The look all the shop's mails share, once set.
    pub mail_branding: Option<MailBranding>,
    /// What the shop buys on AliExpress, once set up.
    pub aliexpress: Option<AliexpressSettings>,
    /// Where the shop lives, once set. Read it through
    /// [`TenantState::shop_time_zone`].
    pub time_zone: Option<String>,
}

/// What the shop buys on AliExpress, through the platform's app and the
/// account of the company it sells as ([`CompanyState::aliexpress`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AliexpressSettings {
    pub ship_to_country: String,
    pub currency: String,
    pub language: String,
    /// How imports are priced, by cost band (`None`: by hand).
    pub pricing: Option<crate::value_object::Pricing>,
    pub configured_by: String,
    /// Unix seconds.
    pub configured_at: u64,
    /// Paid orders with AliExpress items are placed there as they come.
    pub auto_forward: bool,
    /// AliExpress is asked to charge the account when an order is placed.
    pub try_to_pay: bool,
}

/// The tokens a company's AliExpress account gave the platform's app.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AliexpressAuthorization {
    pub access_token_sealed: Vec<u8>,
    pub refresh_token_sealed: Vec<u8>,
    /// Unix seconds.
    pub expires_at: u64,
    pub refresh_expires_at: u64,
    pub account: String,
    pub authorized_by: String,
    /// Unix seconds.
    pub authorized_at: u64,
}

/// The look all of a shop's mails share.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MailBranding {
    /// A picture in the tenant's media, `<id>.<ext>`.
    pub logo_file: Option<String>,
    pub logo_size: u64,
    /// `#rrggbb`.
    pub accent: Option<String>,
    pub footer: Option<String>,
    pub set_by: String,
    /// Unix seconds.
    pub set_at: u64,
}

/// How many changes of one mail's wording, in one language, the tenant
/// remembers.
pub const MAIL_TEMPLATE_HISTORY: usize = 20;

/// The languages a shop mails in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MailLanguages {
    /// What a shopper gets when their language is not among `others`.
    pub default: String,
    /// The other languages the shop words its mails in, without `default`.
    pub others: Vec<String>,
    pub set_by: String,
    /// Unix seconds.
    pub set_at: u64,
}

/// The merchant's wording of one mail.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MailTemplate {
    pub subject: String,
    pub html: String,
    /// The merchant's plain-text part; `None` sends the text derived from
    /// the HTML.
    pub text: Option<String>,
    pub saved_by: String,
    /// Unix seconds.
    pub saved_at: u64,
}

/// One change to a mail's wording in one language.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MailTemplateChange {
    pub mail: String,
    pub language: String,
    pub by: String,
    /// Unix seconds.
    pub at: u64,
    /// Back to the built-in wording, rather than saved.
    pub reset: bool,
}

/// One of the mail servers the shop's mails can go out through. The
/// password is sealed bytes only the host can open.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MailServerSettings {
    /// A ULID.
    pub id: String,
    /// The merchant's name for it.
    pub name: Option<String>,
    pub host: String,
    pub port: u16,
    /// One of [`MAIL_SECURITIES`](crate::MAIL_SECURITIES).
    pub security: String,
    pub username: Option<String>,
    pub password_sealed: Option<Vec<u8>>,
    pub from_address: String,
    pub from_name: Option<String>,
    pub configured_by: String,
    /// Unix seconds.
    pub configured_at: u64,
}

impl MailServerSettings {
    /// What the merchant knows it by: its name, else its address.
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.from_address)
    }
}

/// The company a tenant sells as and the series its invoices are numbered
/// in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TenantCompany {
    pub company_id: String,
    /// `""` for the plain series.
    pub series_code: String,
}

/// The merchant's account with the payment provider. The secrets are sealed
/// bytes only the host can open.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaymentSettings {
    pub provider: String,
    pub publishable_key: String,
    pub secret_key_sealed: Vec<u8>,
    pub webhook_secret_sealed: Vec<u8>,
    pub configured_by: String,
    /// Unix seconds.
    pub configured_at: u64,
}

/// A custom role as the tenant defined it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoleDef {
    pub label: String,
    /// Entries of [`PERMISSIONS`](crate::PERMISSIONS).
    pub permissions: BTreeSet<String>,
}

impl TenantState {
    /// Where the shop lives: the zone it set, else UTC.
    pub fn shop_time_zone(&self) -> &str {
        self.time_zone.as_deref().unwrap_or(DEFAULT_TIME_ZONE)
    }

    pub fn is_suspended(&self) -> bool {
        self.suspended.is_some()
    }

    pub fn role_of(&self, user_id: &str) -> Option<&str> {
        self.members.get(user_id).map(String::as_str)
    }

    pub fn is_owner(&self, user_id: &str) -> bool {
        self.role_of(user_id) == Some(ROLE_OWNER)
    }

    /// Whether `role` can be granted: `owner` or a defined custom role.
    pub fn has_role(&self, role: &str) -> bool {
        role == ROLE_OWNER || self.roles.contains_key(role)
    }

    /// Whether `user_id` holds `permission` here: owners hold every one,
    /// other members what their role grants (`manage` implying `read`),
    /// non-members nothing.
    pub fn can(&self, user_id: &str, permission: &str) -> bool {
        match self.role_of(user_id) {
            Some(ROLE_OWNER) => true,
            Some(role) => self
                .roles
                .get(role)
                .is_some_and(|def| def.permissions.iter().any(|held| implies(held, permission))),
            None => false,
        }
    }

    pub fn owners(&self) -> usize {
        self.members_with_role(ROLE_OWNER)
    }

    pub fn members_with_role(&self, role: &str) -> usize {
        self.members.values().filter(|r| r.as_str() == role).count()
    }
}

pub fn tenant_projection<E: Executor>() -> Projection<E, TenantState> {
    Projection::new::<crate::aggregator::Tenant>()
        .handler(on_created())
        .handler(on_renamed())
        .handler(on_time_zone_set())
        .handler(on_suspended())
        .handler(on_resumed())
        .handler(on_member_granted())
        .handler(on_member_role_changed())
        .handler(on_member_revoked())
        .handler(on_role_defined())
        .handler(on_role_updated())
        .handler(on_role_removed())
        .handler(on_payments_configured())
        .handler(on_payments_removed())
        .handler(on_company_linked())
        .handler(on_company_unlinked())
        .handler(on_mail_server_added())
        .handler(on_mail_server_changed())
        .handler(on_mail_server_deleted())
        .handler(on_mail_servers_picked())
        .handler(on_aliexpress_configured())
        .handler(on_aliexpress_removed())
        .handler(on_aliexpress_forwarding_set())
        .handler(on_mail_languages_set())
        .handler(on_mail_template_saved())
        .handler(on_mail_template_text_saved())
        .handler(on_mail_template_reset())
        .handler(on_mail_branding_set())
        .handler(on_mail_branding_cleared())
        .strict()
}

#[evento::handler]
async fn on_created(event: Event<Created>, tenant: &mut TenantState) -> anyhow::Result<()> {
    tenant.id = event.aggregate_id.to_owned();
    tenant.slug = event.data.slug.to_owned();
    tenant.name = event.data.name.to_owned();
    tenant.created_by = event.data.created_by.to_owned();
    tenant.created_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_renamed(event: Event<Renamed>, tenant: &mut TenantState) -> anyhow::Result<()> {
    tenant.name = event.data.name.to_owned();
    Ok(())
}

#[evento::handler]
async fn on_time_zone_set(
    event: Event<crate::aggregator::TimeZoneSet>,
    tenant: &mut TenantState,
) -> anyhow::Result<()> {
    tenant.time_zone = Some(event.data.time_zone.to_owned());
    Ok(())
}

#[evento::handler]
async fn on_suspended(event: Event<Suspended>, tenant: &mut TenantState) -> anyhow::Result<()> {
    tenant.suspended = Some(event.data.reason.to_owned());
    Ok(())
}

#[evento::handler]
async fn on_resumed(_event: Event<Resumed>, tenant: &mut TenantState) -> anyhow::Result<()> {
    tenant.suspended = None;
    Ok(())
}

#[evento::handler]
async fn on_member_granted(
    event: Event<MemberGranted>,
    tenant: &mut TenantState,
) -> anyhow::Result<()> {
    tenant
        .members
        .insert(event.data.user_id.to_owned(), event.data.role.to_owned());
    Ok(())
}

#[evento::handler]
async fn on_member_role_changed(
    event: Event<MemberRoleChanged>,
    tenant: &mut TenantState,
) -> anyhow::Result<()> {
    if let Some(role) = tenant.members.get_mut(&event.data.user_id) {
        *role = event.data.role.to_owned();
    }
    Ok(())
}

#[evento::handler]
async fn on_member_revoked(
    event: Event<MemberRevoked>,
    tenant: &mut TenantState,
) -> anyhow::Result<()> {
    tenant.members.remove(&event.data.user_id);
    Ok(())
}

#[evento::handler]
async fn on_role_defined(
    event: Event<RoleDefined>,
    tenant: &mut TenantState,
) -> anyhow::Result<()> {
    tenant.roles.insert(
        event.data.role.to_owned(),
        RoleDef {
            label: event.data.label.to_owned(),
            permissions: event.data.permissions.iter().cloned().collect(),
        },
    );
    Ok(())
}

#[evento::handler]
async fn on_role_updated(
    event: Event<RoleUpdated>,
    tenant: &mut TenantState,
) -> anyhow::Result<()> {
    if let Some(def) = tenant.roles.get_mut(&event.data.role) {
        def.label = event.data.label.to_owned();
        def.permissions = event.data.permissions.iter().cloned().collect();
    }
    Ok(())
}

#[evento::handler]
async fn on_role_removed(
    event: Event<RoleRemoved>,
    tenant: &mut TenantState,
) -> anyhow::Result<()> {
    tenant.roles.remove(&event.data.role);
    Ok(())
}

#[evento::handler]
async fn on_payments_configured(
    event: Event<PaymentsConfigured>,
    tenant: &mut TenantState,
) -> anyhow::Result<()> {
    tenant.payments = Some(PaymentSettings {
        provider: event.data.provider.to_owned(),
        publishable_key: event.data.publishable_key.to_owned(),
        secret_key_sealed: event.data.secret_key_sealed.to_owned(),
        webhook_secret_sealed: event.data.webhook_secret_sealed.to_owned(),
        configured_by: event.data.configured_by.to_owned(),
        configured_at: event.timestamp,
    });
    Ok(())
}

#[evento::handler]
async fn on_payments_removed(
    _event: Event<PaymentsRemoved>,
    tenant: &mut TenantState,
) -> anyhow::Result<()> {
    tenant.payments = None;
    Ok(())
}

#[evento::handler]
async fn on_mail_server_added(
    event: Event<MailServerAdded>,
    tenant: &mut TenantState,
) -> anyhow::Result<()> {
    let data = &event.data;
    tenant.put_mail_server(MailServerSettings {
        id: data.server_id.to_owned(),
        name: data.name.to_owned(),
        host: data.host.to_owned(),
        port: data.port,
        security: data.security.to_owned(),
        username: data.username.to_owned(),
        password_sealed: data.password_sealed.to_owned(),
        from_address: data.from_address.to_owned(),
        from_name: data.from_name.to_owned(),
        configured_by: data.added_by.to_owned(),
        configured_at: event.timestamp,
    });
    Ok(())
}

#[evento::handler]
async fn on_mail_server_changed(
    event: Event<MailServerChanged>,
    tenant: &mut TenantState,
) -> anyhow::Result<()> {
    let data = &event.data;
    tenant.put_mail_server(MailServerSettings {
        id: data.server_id.to_owned(),
        name: data.name.to_owned(),
        host: data.host.to_owned(),
        port: data.port,
        security: data.security.to_owned(),
        username: data.username.to_owned(),
        password_sealed: data.password_sealed.to_owned(),
        from_address: data.from_address.to_owned(),
        from_name: data.from_name.to_owned(),
        configured_by: data.changed_by.to_owned(),
        configured_at: event.timestamp,
    });
    Ok(())
}

#[evento::handler]
async fn on_mail_server_deleted(
    event: Event<MailServerDeleted>,
    tenant: &mut TenantState,
) -> anyhow::Result<()> {
    tenant.drop_mail_server(&event.data.server_id);
    Ok(())
}

#[evento::handler]
async fn on_mail_servers_picked(
    event: Event<MailServersPicked>,
    tenant: &mut TenantState,
) -> anyhow::Result<()> {
    tenant.shop_mails_server_id = event.data.shop_mails.to_owned();
    tenant.marketing_server_id = event.data.marketing.to_owned();
    Ok(())
}

#[evento::handler]
async fn on_aliexpress_configured(
    event: Event<crate::aggregator::AliexpressConfigured>,
    tenant: &mut TenantState,
) -> anyhow::Result<()> {
    let data = &event.data;
    // Setting the market again keeps how orders go.
    let (auto_forward, try_to_pay) = tenant
        .aliexpress
        .take()
        .map(|was| (was.auto_forward, was.try_to_pay))
        .unwrap_or_default();
    tenant.aliexpress = Some(AliexpressSettings {
        ship_to_country: data.ship_to_country.to_owned(),
        currency: data.currency.to_owned(),
        language: data.language.to_owned(),
        pricing: data.pricing.clone(),
        configured_by: data.configured_by.to_owned(),
        configured_at: event.timestamp,
        auto_forward,
        try_to_pay,
    });
    Ok(())
}

#[evento::handler]
async fn on_aliexpress_removed(
    _event: Event<crate::aggregator::AliexpressRemoved>,
    tenant: &mut TenantState,
) -> anyhow::Result<()> {
    tenant.aliexpress = None;
    Ok(())
}

#[evento::handler]
async fn on_aliexpress_forwarding_set(
    event: Event<crate::aggregator::AliexpressForwardingSet>,
    tenant: &mut TenantState,
) -> anyhow::Result<()> {
    if let Some(settings) = tenant.aliexpress.as_mut() {
        settings.auto_forward = event.data.auto_forward;
        settings.try_to_pay = event.data.try_to_pay;
    }
    Ok(())
}

impl TenantState {
    /// One of the shop's mail servers.
    pub fn mail_server(&self, id: &str) -> Option<&MailServerSettings> {
        self.mail_servers.iter().find(|server| server.id == id)
    }

    /// The server the shop's mails to its shoppers go out through; `None`
    /// for the platform's.
    pub fn shop_mails_server(&self) -> Option<&MailServerSettings> {
        self.shop_mails_server_id
            .as_deref()
            .and_then(|id| self.mail_server(id))
    }

    /// The server the shop's marketing goes out through; `None` when none
    /// is picked, and then no marketing goes out.
    pub fn marketing_server(&self) -> Option<&MailServerSettings> {
        self.marketing_server_id
            .as_deref()
            .and_then(|id| self.mail_server(id))
    }

    /// Adds `server`, or replaces the one with its id where it stands.
    fn put_mail_server(&mut self, server: MailServerSettings) {
        match self
            .mail_servers
            .iter_mut()
            .find(|saved| saved.id == server.id)
        {
            Some(saved) => *saved = server,
            None => self.mail_servers.push(server),
        }
    }

    /// Removes a server; what it was picked for has no server picked any
    /// more.
    fn drop_mail_server(&mut self, id: &str) {
        self.mail_servers.retain(|server| server.id != id);
        if self.shop_mails_server_id.as_deref() == Some(id) {
            self.shop_mails_server_id = None;
        }
        if self.marketing_server_id.as_deref() == Some(id) {
            self.marketing_server_id = None;
        }
    }

    /// The language the shop mails in when a shopper's is not one of its
    /// others, or unknown: the one it picked, else English.
    pub fn mail_default_language(&self) -> &str {
        self.mail_languages
            .as_ref()
            .map_or(crate::DEFAULT_LANGUAGE, |languages| &languages.default)
    }

    /// Every language the shop mails in, the default first.
    pub fn mail_languages(&self) -> Vec<&str> {
        let mut languages = vec![self.mail_default_language()];
        if let Some(picked) = &self.mail_languages {
            languages.extend(picked.others.iter().map(String::as_str));
        }
        languages
    }

    /// Whether the shop mails in `language`.
    pub fn mails_in(&self, language: &str) -> bool {
        self.mail_languages().contains(&language)
    }

    /// The merchant's wording of `kind` in `language`, when not the
    /// built-in one. A wording in a language the shop no longer mails in
    /// is kept, but not found here.
    pub fn mail_template_in(
        &self,
        kind: crate::ShopMailKind,
        language: &str,
    ) -> Option<&MailTemplate> {
        if !self.mails_in(language) {
            return None;
        }
        self.mail_templates
            .get(&(kind.key().to_owned(), language.to_owned()))
    }

    /// The wording `kind` goes out in to a shopper speaking `language`:
    /// theirs when the shop mails in it and worded it, else the default
    /// language's, else `None` for the built-in wording. A shopper whose
    /// language is unknown gets the default language's.
    pub fn mail_template_for(
        &self,
        kind: crate::ShopMailKind,
        language: Option<&str>,
    ) -> Option<&MailTemplate> {
        language
            .and_then(|language| self.mail_template_in(kind, language))
            .or_else(|| self.mail_template_in(kind, self.mail_default_language()))
    }

    /// Who changed `kind`'s wording in `language` and when, newest first.
    pub fn mail_template_changes<'a>(
        &'a self,
        kind: crate::ShopMailKind,
        language: &'a str,
    ) -> impl Iterator<Item = &'a MailTemplateChange> {
        self.mail_template_history
            .iter()
            .rev()
            .filter(move |change| change.mail == kind.key() && change.language == language)
    }

    fn save_mail_template(&mut self, mail: &str, language: &str, template: MailTemplate, at: u64) {
        let by = template.saved_by.clone();
        self.mail_templates
            .insert((mail.to_owned(), language.to_owned()), template);
        self.record_mail_template_change(MailTemplateChange {
            mail: mail.to_owned(),
            language: language.to_owned(),
            by,
            at,
            reset: false,
        });
    }

    fn reset_mail_template(&mut self, mail: &str, language: &str, by: &str, at: u64) {
        self.mail_templates
            .remove(&(mail.to_owned(), language.to_owned()));
        self.record_mail_template_change(MailTemplateChange {
            mail: mail.to_owned(),
            language: language.to_owned(),
            by: by.to_owned(),
            at,
            reset: true,
        });
    }

    fn record_mail_template_change(&mut self, change: MailTemplateChange) {
        let same = |other: &MailTemplateChange| {
            other.mail == change.mail && other.language == change.language
        };
        let oldest = self.mail_template_history.iter().position(same);
        let kept = self
            .mail_template_history
            .iter()
            .filter(|c| same(c))
            .count();
        self.mail_template_history.push(change);
        if kept >= MAIL_TEMPLATE_HISTORY
            && let Some(oldest) = oldest
        {
            self.mail_template_history.remove(oldest);
        }
    }
}

#[evento::handler]
async fn on_mail_languages_set(
    event: Event<MailLanguagesSet>,
    tenant: &mut TenantState,
) -> anyhow::Result<()> {
    tenant.mail_languages = Some(MailLanguages {
        default: event.data.default.to_owned(),
        others: event.data.others.to_owned(),
        set_by: event.data.set_by.to_owned(),
        set_at: event.timestamp,
    });
    Ok(())
}

#[evento::handler]
async fn on_mail_template_saved(
    event: Event<MailTemplateSaved>,
    tenant: &mut TenantState,
) -> anyhow::Result<()> {
    let data = &event.data;
    tenant.save_mail_template(
        &data.mail,
        &data.language,
        MailTemplate {
            subject: data.subject.to_owned(),
            html: data.html.to_owned(),
            text: None,
            saved_by: data.saved_by.to_owned(),
            saved_at: event.timestamp,
        },
        event.timestamp,
    );
    Ok(())
}

#[evento::handler]
async fn on_mail_template_text_saved(
    event: Event<MailTemplateTextSaved>,
    tenant: &mut TenantState,
) -> anyhow::Result<()> {
    let key = (event.data.mail.to_owned(), event.data.language.to_owned());
    if let Some(template) = tenant.mail_templates.get_mut(&key) {
        template.text = Some(event.data.text.to_owned());
    }
    Ok(())
}

#[evento::handler]
async fn on_mail_template_reset(
    event: Event<MailTemplateReset>,
    tenant: &mut TenantState,
) -> anyhow::Result<()> {
    let data = &event.data;
    tenant.reset_mail_template(&data.mail, &data.language, &data.reset_by, event.timestamp);
    Ok(())
}

#[evento::handler]
async fn on_mail_branding_set(
    event: Event<MailBrandingSet>,
    tenant: &mut TenantState,
) -> anyhow::Result<()> {
    let data = &event.data;
    tenant.mail_branding = Some(MailBranding {
        logo_file: data.logo_file.to_owned(),
        logo_size: data.logo_size,
        accent: data.accent.to_owned(),
        footer: data.footer.to_owned(),
        set_by: data.set_by.to_owned(),
        set_at: event.timestamp,
    });
    Ok(())
}

#[evento::handler]
async fn on_mail_branding_cleared(
    _event: Event<MailBrandingCleared>,
    tenant: &mut TenantState,
) -> anyhow::Result<()> {
    tenant.mail_branding = None;
    Ok(())
}

#[evento::handler]
async fn on_company_linked(
    event: Event<CompanyLinked>,
    tenant: &mut TenantState,
) -> anyhow::Result<()> {
    tenant.company = Some(TenantCompany {
        company_id: event.data.company_id.to_owned(),
        series_code: event.data.series_code.to_owned(),
    });
    Ok(())
}

#[evento::handler]
async fn on_company_unlinked(
    _event: Event<CompanyUnlinked>,
    tenant: &mut TenantState,
) -> anyhow::Result<()> {
    tenant.company = None;
    Ok(())
}

// =============================================================================
// TenantSlug (index)
// =============================================================================

#[evento::projection(id = id, name = "timada-tenancy/TenantSlugState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct TenantSlugState {
    pub id: String,
    pub tenant_id: String,
    pub released: bool,
}

pub fn tenant_slug_projection<E: Executor>() -> Projection<E, TenantSlugState> {
    Projection::new::<crate::aggregator::TenantSlug>()
        .handler(on_slug_claimed())
        .handler(on_slug_released())
        .strict()
}

#[evento::handler]
async fn on_slug_claimed(event: Event<Claimed>, index: &mut TenantSlugState) -> anyhow::Result<()> {
    index.id = event.aggregate_id.to_owned();
    index.tenant_id = event.data.tenant_id.to_owned();
    index.released = false;
    Ok(())
}

#[evento::handler]
async fn on_slug_released(
    _event: Event<Released>,
    index: &mut TenantSlugState,
) -> anyhow::Result<()> {
    index.released = true;
    Ok(())
}

// =============================================================================
// Invitation
// =============================================================================

#[evento::projection(id = id, name = "timada-tenancy/InvitationState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct InvitationState {
    pub id: String,
    pub tenant_id: String,
    /// Normalized address the link went to.
    pub email: String,
    pub role: String,
    pub invited_by: String,
    pub sent_at: u64,
    pub expires_at: u64,
    pub accepted_by: Option<String>,
    pub accepted_at: Option<u64>,
    pub revoked: bool,
}

impl InvitationState {
    /// Whether the link still works at `now`: not accepted, not revoked, not
    /// expired.
    pub fn is_open_at(&self, now: u64) -> bool {
        self.accepted_by.is_none() && !self.revoked && now < self.expires_at
    }
}

pub fn invitation_projection<E: Executor>() -> Projection<E, InvitationState> {
    Projection::new::<crate::aggregator::Invitation>()
        .handler(on_invitation_sent())
        .handler(on_invitation_accepted())
        .handler(on_invitation_revoked())
        .strict()
}

#[evento::handler]
async fn on_invitation_sent(
    event: Event<Sent>,
    invitation: &mut InvitationState,
) -> anyhow::Result<()> {
    invitation.id = event.aggregate_id.to_owned();
    invitation.tenant_id = event.data.tenant_id.to_owned();
    invitation.email = event.data.email.to_owned();
    invitation.role = event.data.role.to_owned();
    invitation.invited_by = event.data.invited_by.to_owned();
    invitation.sent_at = event.data.sent_at;
    invitation.expires_at = event.data.expires_at;
    Ok(())
}

#[evento::handler]
async fn on_invitation_accepted(
    event: Event<Accepted>,
    invitation: &mut InvitationState,
) -> anyhow::Result<()> {
    invitation.accepted_by = Some(event.data.user_id.to_owned());
    invitation.accepted_at = Some(event.data.accepted_at);
    Ok(())
}

#[evento::handler]
async fn on_invitation_revoked(
    _event: Event<Revoked>,
    invitation: &mut InvitationState,
) -> anyhow::Result<()> {
    invitation.revoked = true;
    Ok(())
}

// =============================================================================
// ApiKey
// =============================================================================

#[evento::projection(id = id, name = "timada-tenancy/ApiKeyState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct ApiKeyState {
    pub id: String,
    pub tenant_id: String,
    pub label: String,
    pub hint: String,
    pub created_by: String,
    pub created_at: u64,
    pub revoked_by: Option<String>,
    pub revoked_at: Option<u64>,
    /// What the key may do, sorted.
    pub scopes: BTreeSet<String>,
}

impl ApiKeyState {
    pub fn is_active(&self) -> bool {
        self.revoked_at.is_none()
    }

    /// Whether one of the key's scopes grants `scope`.
    pub fn allows(&self, scope: &str) -> bool {
        self.scopes
            .iter()
            .any(|held| api_key_scope_implies(held, scope))
    }
}

pub fn api_key_projection<E: Executor>() -> Projection<E, ApiKeyState> {
    Projection::new::<crate::aggregator::api_key::ApiKey>()
        .handler(on_api_key_created())
        .handler(on_api_key_revoked())
        .handler(on_api_key_scoped())
        .strict()
}

#[evento::handler]
async fn on_api_key_created(
    event: Event<ApiKeyCreated>,
    key: &mut ApiKeyState,
) -> anyhow::Result<()> {
    key.id = event.aggregate_id.to_owned();
    key.tenant_id = event.data.tenant_id.to_owned();
    key.label = event.data.label.to_owned();
    key.hint = event.data.hint.to_owned();
    key.created_by = event.data.created_by.to_owned();
    key.created_at = event.data.created_at;
    key.scopes = event.data.scopes.iter().cloned().collect();
    Ok(())
}

#[evento::handler]
async fn on_api_key_revoked(
    event: Event<ApiKeyRevoked>,
    key: &mut ApiKeyState,
) -> anyhow::Result<()> {
    key.revoked_by = Some(event.data.revoked_by.to_owned());
    key.revoked_at = Some(event.data.revoked_at);
    Ok(())
}

#[evento::handler]
async fn on_api_key_scoped(
    event: Event<ApiKeyScoped>,
    key: &mut ApiKeyState,
) -> anyhow::Result<()> {
    key.scopes = event.data.scopes.iter().cloned().collect();
    Ok(())
}

// =============================================================================
// Loads
// =============================================================================

impl<E: Executor> Command<E> {
    pub async fn load_tenant(&self, tenant_id: &str) -> anyhow::Result<Option<TenantState>> {
        tenant_projection().load(tenant_id).execute(&self.0).await
    }

    /// The tenant a (normalized) slug points at, if any.
    pub async fn load_tenant_by_slug(&self, slug: &str) -> anyhow::Result<Option<TenantState>> {
        let Some(index) = self.load_slug_index(slug).await? else {
            return Ok(None);
        };
        self.load_tenant(&index.tenant_id).await
    }

    /// The live index entry of a slug (released ones are `None`).
    pub async fn load_slug_index(&self, slug: &str) -> anyhow::Result<Option<TenantSlugState>> {
        Ok(self
            .load_slug_index_any(slug)
            .await?
            .filter(|index| !index.released))
    }

    /// The index entry including released ones (needed to re-claim).
    pub(crate) async fn load_slug_index_any(
        &self,
        slug: &str,
    ) -> anyhow::Result<Option<TenantSlugState>> {
        tenant_slug_projection()
            .load(tenant_slug_id(slug))
            .execute(&self.0)
            .await
    }

    /// An invitation by its stream id (what the pages list).
    pub async fn load_invitation(
        &self,
        invitation_id: &str,
    ) -> anyhow::Result<Option<InvitationState>> {
        invitation_projection()
            .load(invitation_id)
            .execute(&self.0)
            .await
    }

    /// The invitation a link token opens, if any.
    pub async fn load_invitation_by_token(
        &self,
        token: &str,
    ) -> anyhow::Result<Option<InvitationState>> {
        self.load_invitation(&invitation_id(token)).await
    }

    /// A store API key by its stream id (what the pages list).
    pub async fn load_api_key(&self, key_id: &str) -> anyhow::Result<Option<ApiKeyState>> {
        api_key_projection().load(key_id).execute(&self.0).await
    }

    /// The store API key a token names, if any.
    pub async fn load_api_key_by_token(&self, token: &str) -> anyhow::Result<Option<ApiKeyState>> {
        self.load_api_key(&api_key_id(token)).await
    }
}
