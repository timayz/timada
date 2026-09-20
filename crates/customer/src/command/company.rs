use evento::{Executor, ProjectionAggregate};
use timada_tax::{VatCheck, VatCheckUnavailable, VatNumber, VatNumberValidator};

use crate::{
    aggregator::{CompanyIdentified, CompanyIdentityRemoved, VatNumberChecked},
    error::CustomerError,
};

impl<E: Executor> super::Command<'_, E> {
    /// The customer buys as a business, under this name and VAT number. Said
    /// again it changes either; the same identity records nothing. A number
    /// that changed has not been checked: the checks before were about
    /// another one.
    pub async fn identify_company(
        &self,
        id: impl Into<String>,
        company_name: &str,
        vat_number: &VatNumber,
    ) -> Result<(), CustomerError> {
        let customer = self.load_existing(id).await?;
        let company_name = company_name.trim();
        if company_name.is_empty() {
            return Err(CustomerError::Required("company_name"));
        }
        let vat_number = vat_number.to_string();
        if customer
            .company
            .as_ref()
            .is_some_and(|(name, number)| name == company_name && *number == vat_number)
        {
            return Ok(());
        }
        customer
            .write()?
            .event(&CompanyIdentified {
                company_name: company_name.to_owned(),
                vat_number,
            })
            .commit(self.0)
            .await?;
        Ok(())
    }

    /// The customer buys as a consumer again; harmless when it already does.
    pub async fn remove_company_identity(
        &self,
        id: impl Into<String>,
    ) -> Result<(), CustomerError> {
        let customer = self.load_existing(id).await?;
        if customer.company.is_none() {
            return Ok(());
        }
        customer
            .write()?
            .event(&CompanyIdentityRemoved)
            .commit(self.0)
            .await?;
        Ok(())
    }

    /// Records what the VAT registry answered about the company's number.
    pub async fn record_vat_check(
        &self,
        id: impl Into<String>,
        vat_number: &VatNumber,
        check: &VatCheck,
    ) -> Result<(), CustomerError> {
        let customer = self.load_existing(id).await?;
        let Some((_, current)) = &customer.company else {
            return Err(CustomerError::NoCompanyIdentity);
        };
        let vat_number = vat_number.to_string();
        if *current != vat_number {
            return Err(CustomerError::VatNumberMismatch);
        }
        customer
            .write()?
            .event(&VatNumberChecked {
                vat_number,
                valid: check.valid,
                consultation_ref: check.consultation_ref.clone(),
                registered_name: check.registered_name.clone(),
            })
            .commit(self.0)
            .await?;
        Ok(())
    }

    /// Asks the registry about the company's number and records the answer.
    /// `Ok(None)`: the customer has no company identity. `Ok(Some(Err(_)))`:
    /// the registry could not answer — nothing is recorded, the checks made
    /// before stand.
    pub async fn check_company_vat_number(
        &self,
        id: impl Into<String>,
        validator: &dyn VatNumberValidator,
    ) -> Result<Option<Result<VatCheck, VatCheckUnavailable>>, CustomerError> {
        let id = id.into();
        let customer = self.load_existing(&id).await?;
        let Some((_, number)) = &customer.company else {
            return Ok(None);
        };
        // Stored compact and well formed; a number that no longer parses
        // (the shapes changed) is not valid.
        let Ok(number) = VatNumber::parse(number) else {
            return Ok(None);
        };
        match validator.check(&number).await {
            Ok(check) => {
                self.record_vat_check(&id, &number, &check).await?;
                Ok(Some(Ok(check)))
            }
            Err(unavailable) => Ok(Some(Err(unavailable))),
        }
    }
}
