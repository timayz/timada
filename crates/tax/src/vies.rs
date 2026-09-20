//! VIES, the EU's VAT registry, as a [`VatNumberValidator`] (feature `vies`):
//! one call to its REST service. A requester that names itself — the shop's
//! own VAT number — gets a consultation number back: the proof to keep.

use std::time::Duration;

use serde::Deserialize;

use crate::vat_number::{
    VatCheck, VatCheckFuture, VatCheckUnavailable, VatNumber, VatNumberValidator,
};

const VIES: &str = "https://ec.europa.eu/taxation_customs/vies/rest-api";

#[derive(Debug, Clone)]
pub struct ViesValidator {
    http: reqwest::Client,
    /// The shop's own number; without it VIES answers, but proves nothing.
    requester: Option<VatNumber>,
    api_base: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Answer {
    valid: Option<bool>,
    request_identifier: Option<String>,
    name: Option<String>,
    /// `false` with `errorWrappers` when VIES could not process the request.
    action_succeed: Option<bool>,
    #[serde(default)]
    error_wrappers: Vec<ErrorWrapper>,
}

#[derive(Debug, Deserialize)]
struct ErrorWrapper {
    error: Option<String>,
}

impl ViesValidator {
    pub fn new(requester: Option<VatNumber>) -> Result<Self, VatCheckUnavailable> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|err| VatCheckUnavailable(err.to_string()))?;
        Ok(Self {
            http,
            requester,
            api_base: VIES.to_owned(),
        })
    }

    /// Points the validator at another address — a stand-in, in tests.
    pub fn with_api_base(mut self, api_base: impl Into<String>) -> Self {
        self.api_base = api_base.into();
        self
    }
}

impl VatNumberValidator for ViesValidator {
    fn check<'a>(&'a self, number: &'a VatNumber) -> VatCheckFuture<'a> {
        Box::pin(async move {
            let mut body = serde_json::json!({
                "countryCode": number.prefix(),
                "vatNumber": number.number(),
            });
            if let Some(requester) = &self.requester {
                body["requesterMemberStateCode"] = requester.prefix().into();
                body["requesterNumber"] = requester.number().into();
            }
            let unavailable = |err: String| VatCheckUnavailable(err);
            let response = self
                .http
                .post(format!(
                    "{}/check-vat-number",
                    self.api_base.trim_end_matches('/')
                ))
                .json(&body)
                .send()
                .await
                .map_err(|err| unavailable(err.to_string()))?;
            let status = response.status();
            let answer: Answer = response
                .json()
                .await
                .map_err(|err| unavailable(format!("HTTP {status}: {err}")))?;

            if answer.action_succeed == Some(false) || !answer.error_wrappers.is_empty() {
                let error = answer
                    .error_wrappers
                    .into_iter()
                    .find_map(|wrapper| wrapper.error)
                    .unwrap_or_else(|| format!("HTTP {status}"));
                // The one error that is a verdict: VIES cannot read the number.
                if error == "INVALID_INPUT" {
                    return Ok(VatCheck {
                        valid: false,
                        consultation_ref: None,
                        registered_name: None,
                    });
                }
                // MS_UNAVAILABLE, MS_MAX_CONCURRENT_REQ, TIMEOUT, SERVICE_UNAVAILABLE…
                return Err(unavailable(error));
            }
            let Some(valid) = answer.valid else {
                return Err(unavailable(format!("HTTP {status}: no verdict")));
            };
            Ok(VatCheck {
                valid,
                consultation_ref: answer
                    .request_identifier
                    .filter(|reference| !reference.trim().is_empty()),
                // Member states that do not disclose names answer "---".
                registered_name: answer
                    .name
                    .map(|name| name.trim().to_owned())
                    .filter(|name| !name.is_empty() && name != "---"),
            })
        })
    }
}
