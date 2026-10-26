//! Companies: who sells in law, behind one or more tenants. A company's
//! SIREN is unique across the platform (an index aggregate, like a slug),
//! its owners manage it, and each tenant linked to it numbers its invoices
//! in a series code of its own, unique within the company and never freed.

use std::collections::BTreeSet;

use evento::{Executor, Projection, ProjectionAggregate, WriteError, metadata::Event};
use ulid::Ulid;

use crate::aggregator::company::{
    AliexpressAuthorized, AliexpressDisconnected, Created, IdentitySet, OwnerAdded, OwnerRemoved,
};
use crate::aggregator::company_series::Claimed as SeriesClaimed;
use crate::aggregator::company_siren::{Claimed as SirenClaimed, Released as SirenReleased};
use crate::aggregator::{CompanyLinked, CompanyUnlinked};
use crate::error::TenancyError;
use crate::value_object::{
    PERMISSION_TENANT_MANAGE, company_series_id, company_siren_id, normalize_series_code,
};

use super::{AliexpressAuthorization, Command, LegalIdentity, TenantState, normalize_identity};

// =============================================================================
// Company
// =============================================================================

#[evento::projection(id = id, name = "timada-tenancy/CompanyState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct CompanyState {
    pub id: String,
    pub siren: String,
    pub created_by: String,
    /// Unix seconds of `Created`.
    pub created_at: u64,
    /// What its invoices say about it.
    pub identity: Option<LegalIdentity>,
    pub owners: BTreeSet<String>,
    /// Its AliExpress account, once it said yes to the platform's app.
    pub aliexpress: Option<AliexpressAuthorization>,
}

impl CompanyState {
    pub fn is_owner(&self, user_id: &str) -> bool {
        self.owners.contains(user_id)
    }

    /// The legal name, or the SIREN before an identity is set.
    pub fn name(&self) -> &str {
        self.identity
            .as_ref()
            .map_or(self.siren.as_str(), |identity| identity.legal_name.as_str())
    }
}

pub fn company_projection<E: Executor>() -> Projection<E, CompanyState> {
    Projection::new::<crate::aggregator::company::Company>()
        .handler(on_company_created())
        .handler(on_identity_set())
        .handler(on_owner_added())
        .handler(on_owner_removed())
        .handler(on_aliexpress_authorized())
        .handler(on_aliexpress_disconnected())
        .strict()
}

#[evento::handler]
async fn on_company_created(
    event: Event<Created>,
    company: &mut CompanyState,
) -> anyhow::Result<()> {
    company.id = event.aggregate_id.to_owned();
    company.siren = event.data.siren.to_owned();
    company.created_by = event.data.created_by.to_owned();
    company.created_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_identity_set(
    event: Event<IdentitySet>,
    company: &mut CompanyState,
) -> anyhow::Result<()> {
    let data = &event.data;
    company.identity = Some(LegalIdentity {
        legal_name: data.legal_name.to_owned(),
        legal_form: data.legal_form.to_owned(),
        share_capital_cents: data.share_capital_cents,
        siret: data.siret.to_owned(),
        rcs_city: data.rcs_city.to_owned(),
        line1: data.line1.to_owned(),
        line2: data.line2.to_owned(),
        postal_code: data.postal_code.to_owned(),
        city: data.city.to_owned(),
        country: data.country.to_owned(),
        contact_email: data.contact_email.to_owned(),
        vat_number: data.vat_number.to_owned(),
        oss_registered: data.oss_registered,
    });
    Ok(())
}

#[evento::handler]
async fn on_owner_added(
    event: Event<OwnerAdded>,
    company: &mut CompanyState,
) -> anyhow::Result<()> {
    company.owners.insert(event.data.user_id.to_owned());
    Ok(())
}

#[evento::handler]
async fn on_owner_removed(
    event: Event<OwnerRemoved>,
    company: &mut CompanyState,
) -> anyhow::Result<()> {
    company.owners.remove(&event.data.user_id);
    Ok(())
}

#[evento::handler]
async fn on_aliexpress_authorized(
    event: Event<AliexpressAuthorized>,
    company: &mut CompanyState,
) -> anyhow::Result<()> {
    let data = &event.data;
    company.aliexpress = Some(AliexpressAuthorization {
        access_token_sealed: data.access_token_sealed.to_owned(),
        refresh_token_sealed: data.refresh_token_sealed.to_owned(),
        expires_at: data.expires_at,
        refresh_expires_at: data.refresh_expires_at,
        account: data.account.to_owned(),
        authorized_by: data.authorized_by.to_owned(),
        authorized_at: event.timestamp,
    });
    Ok(())
}

#[evento::handler]
async fn on_aliexpress_disconnected(
    _event: Event<AliexpressDisconnected>,
    company: &mut CompanyState,
) -> anyhow::Result<()> {
    company.aliexpress = None;
    Ok(())
}

// =============================================================================
// CompanySiren (index)
// =============================================================================

#[evento::projection(id = id, name = "timada-tenancy/CompanySirenState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct CompanySirenState {
    pub id: String,
    pub company_id: String,
    pub released: bool,
}

pub fn company_siren_projection<E: Executor>() -> Projection<E, CompanySirenState> {
    Projection::new::<crate::aggregator::company_siren::CompanySiren>()
        .handler(on_siren_claimed())
        .handler(on_siren_released())
        .strict()
}

#[evento::handler]
async fn on_siren_claimed(
    event: Event<SirenClaimed>,
    index: &mut CompanySirenState,
) -> anyhow::Result<()> {
    index.id = event.aggregate_id.to_owned();
    index.company_id = event.data.company_id.to_owned();
    index.released = false;
    Ok(())
}

#[evento::handler]
async fn on_siren_released(
    _event: Event<SirenReleased>,
    index: &mut CompanySirenState,
) -> anyhow::Result<()> {
    index.released = true;
    Ok(())
}

// =============================================================================
// CompanySeries (index)
// =============================================================================

#[evento::projection(id = id, name = "timada-tenancy/CompanySeriesState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct CompanySeriesState {
    pub id: String,
    pub tenant_id: String,
}

pub fn company_series_projection<E: Executor>() -> Projection<E, CompanySeriesState> {
    Projection::new::<crate::aggregator::company_series::CompanySeries>()
        .handler(on_series_claimed())
        .strict()
}

#[evento::handler]
async fn on_series_claimed(
    event: Event<SeriesClaimed>,
    index: &mut CompanySeriesState,
) -> anyhow::Result<()> {
    index.id = event.aggregate_id.to_owned();
    index.tenant_id = event.data.tenant_id.to_owned();
    Ok(())
}

// =============================================================================
// Commands
// =============================================================================

fn identity_set(identity: LegalIdentity, set_by: &str) -> IdentitySet {
    IdentitySet {
        legal_name: identity.legal_name,
        legal_form: identity.legal_form,
        share_capital_cents: identity.share_capital_cents,
        siret: identity.siret,
        rcs_city: identity.rcs_city,
        line1: identity.line1,
        line2: identity.line2,
        postal_code: identity.postal_code,
        city: identity.city,
        country: identity.country,
        contact_email: identity.contact_email,
        vat_number: identity.vat_number,
        oss_registered: identity.oss_registered,
        set_by: set_by.to_owned(),
    }
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Registers a company from its legal identity, the actor its first
    /// owner. Its SIREN (the SIRET's first nine digits) must be free: one
    /// already registered is `SirenTaken`.
    pub async fn create_company(
        &self,
        actor_user_id: &str,
        identity: LegalIdentity,
        routing_key: Option<String>,
    ) -> Result<CompanyState, TenancyError> {
        let identity = normalize_identity(identity)?;
        let siren = identity.siren().to_owned();
        let company_id = Ulid::generate().to_string();
        self.claim_siren(&siren, &company_id).await?;
        evento::append(&company_id)
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&Created {
                siren,
                created_by: actor_user_id.to_owned(),
            })
            .event(&identity_set(identity, actor_user_id))
            .event(&OwnerAdded {
                user_id: actor_user_id.to_owned(),
                added_by: actor_user_id.to_owned(),
            })
            .commit(&self.0)
            .await?;
        self.require_company(&company_id).await
    }

    /// Replaces the legal identity. Owners only; the SIRET keeps the
    /// company's SIREN; the same identity is a no-op.
    pub async fn set_company_identity(
        &self,
        actor_user_id: &str,
        company_id: &str,
        identity: LegalIdentity,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let identity = normalize_identity(identity)?;
        let company = self
            .require_company_owner(company_id, actor_user_id)
            .await?;
        if identity.siren() != company.siren {
            return Err(TenancyError::SirenMismatch);
        }
        if company.identity.as_ref() == Some(&identity) {
            return Ok(());
        }
        company
            .write()?
            .routing_key_opt(routing_key)
            .event(&identity_set(identity, actor_user_id))
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// Makes another user an owner too; already one is a no-op.
    pub async fn add_company_owner(
        &self,
        actor_user_id: &str,
        company_id: &str,
        user_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let company = self
            .require_company_owner(company_id, actor_user_id)
            .await?;
        if company.is_owner(user_id) {
            return Ok(());
        }
        company
            .write()?
            .routing_key_opt(routing_key)
            .event(&OwnerAdded {
                user_id: user_id.to_owned(),
                added_by: actor_user_id.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// Removes an owner (oneself included); never the last one.
    pub async fn remove_company_owner(
        &self,
        actor_user_id: &str,
        company_id: &str,
        user_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let company = self
            .require_company_owner(company_id, actor_user_id)
            .await?;
        if !company.is_owner(user_id) {
            return Ok(());
        }
        if company.owners.len() == 1 {
            return Err(TenancyError::LastCompanyOwner);
        }
        company
            .write()?
            .routing_key_opt(routing_key)
            .event(&OwnerRemoved {
                user_id: user_id.to_owned(),
                removed_by: actor_user_id.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// Makes a tenant sell as the company, its invoices numbered under
    /// `series_code` (blank: the plain series). Needs owning the company
    /// and `tenant:manage` on the tenant. A code is the tenant's for good:
    /// one another tenant of the company ever held is `SeriesCodeTaken`.
    pub async fn link_tenant(
        &self,
        actor_user_id: &str,
        company_id: &str,
        tenant_id: &str,
        series_code: Option<&str>,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let code = normalize_series_code(series_code)?;
        self.require_company_owner(company_id, actor_user_id)
            .await?;
        let tenant = self.require_active(tenant_id).await?;
        if !tenant.can(actor_user_id, PERMISSION_TENANT_MANAGE) {
            return Err(TenancyError::Forbidden);
        }
        if tenant
            .company
            .as_ref()
            .is_some_and(|link| link.company_id == company_id && link.series_code == code)
        {
            return Ok(());
        }
        self.claim_series(company_id, &code, tenant_id).await?;
        tenant
            .write()?
            .routing_key_opt(routing_key)
            .event(&CompanyLinked {
                company_id: company_id.to_owned(),
                series_code: code,
                linked_by: actor_user_id.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// Stops the tenant selling as its company; not linked is a no-op.
    /// Needs `tenant:manage`.
    pub async fn unlink_tenant(
        &self,
        actor_user_id: &str,
        tenant_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let tenant = self.require_active(tenant_id).await?;
        if !tenant.can(actor_user_id, PERMISSION_TENANT_MANAGE) {
            return Err(TenancyError::Forbidden);
        }
        if tenant.company.is_none() {
            return Ok(());
        }
        tenant
            .write()?
            .routing_key_opt(routing_key)
            .event(&CompanyUnlinked {
                unlinked_by: actor_user_id.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// Frees a SIREN so another company can claim it: what a platform
    /// administrator does when someone registered a SIREN that is not
    /// theirs (the host checks the actor is one). The company that held it
    /// stays, without the claim.
    pub async fn release_siren(
        &self,
        admin_user_id: &str,
        siren: &str,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let Some(index) = self.load_company_siren(siren).await? else {
            return Ok(());
        };
        if index.released {
            return Ok(());
        }
        index
            .write()?
            .routing_key_opt(routing_key)
            .event(&SirenReleased {
                released_by: admin_user_id.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }
}

impl<E: Executor> Command<E> {
    pub async fn load_company(&self, company_id: &str) -> anyhow::Result<Option<CompanyState>> {
        company_projection().load(company_id).execute(&self.0).await
    }

    /// The index entry of a SIREN, released or not.
    pub async fn load_company_siren(
        &self,
        siren: &str,
    ) -> anyhow::Result<Option<CompanySirenState>> {
        company_siren_projection()
            .load(company_siren_id(siren))
            .execute(&self.0)
            .await
    }

    /// The company a tenant sells as, with the identity its invoices need;
    /// `None` until it is linked to one that has one.
    pub async fn load_company_of(
        &self,
        tenant: &TenantState,
    ) -> anyhow::Result<Option<CompanyState>> {
        let Some(link) = &tenant.company else {
            return Ok(None);
        };
        Ok(self
            .load_company(&link.company_id)
            .await?
            .filter(|company| company.identity.is_some()))
    }

    pub(crate) async fn require_company(
        &self,
        company_id: &str,
    ) -> Result<CompanyState, TenancyError> {
        self.load_company(company_id)
            .await?
            .ok_or(TenancyError::CompanyNotFound)
    }

    pub(crate) async fn require_company_owner(
        &self,
        company_id: &str,
        user_id: &str,
    ) -> Result<CompanyState, TenancyError> {
        let company = self.require_company(company_id).await?;
        if !company.is_owner(user_id) {
            return Err(TenancyError::NotCompanyOwner);
        }
        Ok(company)
    }

    /// Points a SIREN at `company_id`: a free, released or dangling (its
    /// company never got written) entry is taken; a live one is not.
    async fn claim_siren(&self, siren: &str, company_id: &str) -> Result<(), TenancyError> {
        let claimed = SirenClaimed {
            company_id: company_id.to_owned(),
        };
        let created = evento::append(company_siren_id(siren))
            .original_version(0)
            .event(&claimed)
            .commit(&self.0)
            .await;
        match created {
            Ok(_) => Ok(()),
            Err(WriteError::InvalidOriginalVersion) => {
                let Some(existing) = self.load_company_siren(siren).await? else {
                    return Err(TenancyError::SirenTaken);
                };
                let dangling =
                    !existing.released && self.load_company(&existing.company_id).await?.is_none();
                if existing.released || dangling {
                    existing.write()?.event(&claimed).commit(&self.0).await?;
                    Ok(())
                } else {
                    Err(TenancyError::SirenTaken)
                }
            }
            Err(err) => Err(err.into()),
        }
    }

    /// Gives a series code of the company to `tenant_id` for good; already
    /// its own is fine.
    async fn claim_series(
        &self,
        company_id: &str,
        code: &str,
        tenant_id: &str,
    ) -> Result<(), TenancyError> {
        let id = company_series_id(company_id, code);
        let created = evento::append(&id)
            .original_version(0)
            .event(&SeriesClaimed {
                tenant_id: tenant_id.to_owned(),
            })
            .commit(&self.0)
            .await;
        match created {
            Ok(_) => Ok(()),
            Err(WriteError::InvalidOriginalVersion) => {
                let holder = company_series_projection()
                    .load(&id)
                    .execute(&self.0)
                    .await?
                    .map(|index| index.tenant_id);
                if holder.as_deref() == Some(tenant_id) {
                    Ok(())
                } else {
                    Err(TenancyError::SeriesCodeTaken)
                }
            }
            Err(err) => Err(err.into()),
        }
    }
}
