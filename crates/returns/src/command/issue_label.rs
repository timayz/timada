use evento::{Executor, ProjectionAggregate};

use crate::{
    aggregator::{ReturnApproved, ReturnLabelIssued},
    error::ReturnError,
    label::{
        LABEL_CONTENT_TYPES, LabelFile, LabelRequest, MAX_LABEL_FILE_BYTES, ReturnLabelProvider,
    },
    value_object::ReturnStatus,
};

/// The prepaid label an operator (or a provider) gives a return.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueLabel {
    pub carrier: String,
    pub tracking_number: String,
    /// Where the customer downloads it, when it lives at the carrier's.
    pub url: Option<String>,
    /// The label itself, when the operator has the file.
    pub file: Option<LabelFile>,
    /// The customer is not charged for it, whatever the policy says.
    pub waive_fee: bool,
}

impl<E: Executor> super::Command<'_, E> {
    /// Attaches the prepaid label to an approved return. One label per
    /// return; its fee — the policy's, unless the shop is at fault or the
    /// operator waives it — is settled here and comes off the refund when
    /// the parcel is received.
    pub async fn issue_return_label(
        &self,
        id: impl Into<String>,
        label: IssueLabel,
    ) -> Result<(), ReturnError> {
        let request = self.load_existing(id).await?;
        request.expect_status(ReturnStatus::Approved)?;
        let issued = self.label_event(&request, label, false).await?;
        request
            .write()?
            .event(&issued)
            .commit(self.executor)
            .await?;
        tracing::info!(return_id = %request.id, "return label issued");
        Ok(())
    }

    /// Accepts the request and hands the label over in one go, so that what
    /// tells the customer their return is accepted can carry the label.
    pub async fn approve_return_with_label(
        &self,
        id: impl Into<String>,
        label: IssueLabel,
    ) -> Result<(), ReturnError> {
        let request = self.load_existing(id).await?;
        request.expect_status(ReturnStatus::Requested)?;
        let issued = self.label_event(&request, label, true).await?;
        request
            .write()?
            .event(&ReturnApproved)
            .event(&issued)
            .commit(self.executor)
            .await?;
        tracing::info!(return_id = %request.id, "return approved with its label");
        Ok(())
    }

    /// Asks `provider` for the label of an approved return and attaches it.
    pub async fn provide_return_label(
        &self,
        id: impl Into<String>,
        provider: &dyn ReturnLabelProvider,
        waive_fee: bool,
    ) -> Result<(), ReturnError> {
        let request = self.load_existing(id).await?;
        request.expect_status(ReturnStatus::Approved)?;
        if request.label_fee.is_some() {
            return Err(ReturnError::LabelAlreadyIssued);
        }
        let order = timada_order::load_order_details(self.executor, &request.order_id)
            .await?
            .ok_or(ReturnError::OrderNotFound)?;
        let provided = provider
            .label(&LabelRequest {
                return_id: request.id.clone(),
                rma_number: request.rma_number.clone(),
                sender: order.delivery_address,
                units: request.lines.iter().map(|l| l.quantity).sum(),
            })
            .await?;
        let issued = self
            .label_event(
                &request,
                IssueLabel {
                    carrier: provided.carrier,
                    tracking_number: provided.tracking_number,
                    url: provided.url,
                    file: provided.file,
                    waive_fee,
                },
                false,
            )
            .await?;
        request
            .write()?
            .event(&issued)
            .commit(self.executor)
            .await?;
        tracing::info!(return_id = %request.id, "return label provided");
        Ok(())
    }

    /// Checks the label, keeps its file, and words the event.
    async fn label_event(
        &self,
        request: &super::ReturnState,
        label: IssueLabel,
        with_approval: bool,
    ) -> Result<ReturnLabelIssued, ReturnError> {
        if request.label_fee.is_some() {
            return Err(ReturnError::LabelAlreadyIssued);
        }
        let carrier = label.carrier.trim().to_owned();
        let tracking_number = label.tracking_number.trim().to_owned();
        if carrier.is_empty() {
            return Err(ReturnError::Required("carrier"));
        }
        if tracking_number.is_empty() {
            return Err(ReturnError::Required("tracking_number"));
        }
        let url = label
            .url
            .map(|url| url.trim().to_owned())
            .filter(|url| !url.is_empty());
        if let Some(url) = &url
            && !(url.starts_with("https://") || url.starts_with("http://"))
        {
            return Err(ReturnError::InvalidLabelUrl);
        }
        if url.is_none() && label.file.is_none() {
            return Err(ReturnError::LabelMissing);
        }
        let mut file_name = None;
        if let Some(file) = &label.file {
            let well_formed = !file.bytes.is_empty()
                && file.bytes.len() <= MAX_LABEL_FILE_BYTES
                && LABEL_CONTENT_TYPES.contains(&file.content_type.as_str());
            if !well_formed {
                return Err(ReturnError::InvalidLabelFile);
            }
            // The name is only ever shown and offered: keep what a header
            // and a file system both accept.
            let name: String = file
                .file_name
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            let name = if name.trim_matches(['_', '.']).is_empty() {
                format!("etiquette-{}", request.rma_number)
            } else {
                name
            };
            // Before the event: a label announced has its file.
            sqlx::query(
                "INSERT INTO return_label_file (return_id, file_name, content_type, content)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (return_id) DO UPDATE SET
                    file_name = excluded.file_name, content_type = excluded.content_type,
                    content = excluded.content",
            )
            .bind(&request.id)
            .bind(&name)
            .bind(&file.content_type)
            .bind(&file.bytes)
            .execute(&self.db)
            .await?;
            file_name = Some(name);
        }

        let currency = request
            .lines
            .first()
            .map_or(timada_core::Money::EUR, |l| l.unit_price.currency.as_str());
        Ok(ReturnLabelIssued {
            carrier,
            tracking_number,
            url,
            file_name,
            fee: self
                .policy
                .label_fee(request.ground, label.waive_fee, currency),
            with_approval,
        })
    }
}
