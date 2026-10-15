//! Checks a business buyer's EU VAT number with VIES, the European
//! Commission's service: a sale to a business in another EU country goes
//! without VAT (reverse charge) only on a number VIES says is valid, and the
//! reference VIES gives for the check is kept on the invoice as proof.

use std::time::Duration;

use openidconnect::reqwest;
use serde::Deserialize;

/// The Commission's REST service.
pub const VIES_URL: &str = "https://ec.europa.eu/taxation_customs/vies/rest-api";

/// What VIES says about a number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViesAnswer {
    /// Valid, with VIES's reference for the check when it gave one.
    Valid {
        reference: Option<String>,
    },
    Invalid,
    /// VIES (or the member state behind it) could not answer.
    Unavailable(String),
}

#[async_trait::async_trait]
pub trait Vies: Send + Sync {
    /// `vat_number` with its country prefix (`DE123456789`); `requester`, the
    /// seller's own number, makes VIES return a reference for the check.
    async fn check(&self, vat_number: &str, requester: Option<&str>) -> ViesAnswer;
}

pub struct ViesClient {
    url: String,
    client: reqwest::Client,
}

impl ViesClient {
    /// `url`: the Commission's service when `None`.
    pub fn new(url: Option<String>) -> Self {
        Self {
            url: url
                .unwrap_or_else(|| VIES_URL.to_owned())
                .trim_end_matches('/')
                .to_owned(),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap_or_default(),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CheckResponse {
    #[serde(default)]
    valid: bool,
    #[serde(default)]
    request_identifier: Option<String>,
    #[serde(default)]
    user_error: Option<String>,
}

/// The answers that mean VIES could not tell, not that the number is bad.
const UNAVAILABLE: &[&str] = &[
    "MS_UNAVAILABLE",
    "MS_MAX_CONCURRENT_REQ",
    "GLOBAL_MAX_CONCURRENT_REQ",
    "SERVICE_UNAVAILABLE",
    "TIMEOUT",
];

#[async_trait::async_trait]
impl Vies for ViesClient {
    async fn check(&self, vat_number: &str, requester: Option<&str>) -> ViesAnswer {
        if vat_number.len() < 3 {
            return ViesAnswer::Invalid;
        }
        let (country, number) = vat_number.split_at(2);
        let mut body = serde_json::json!({ "countryCode": country, "vatNumber": number });
        if let Some(requester) = requester.filter(|r| r.len() > 2) {
            let (requester_country, requester_number) = requester.split_at(2);
            body["requesterMemberStateCode"] = requester_country.into();
            body["requesterNumber"] = requester_number.into();
        }
        let response = self
            .client
            .post(format!("{}/check-vat-number", self.url))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_string())
            .send()
            .await;
        let response = match response {
            Ok(response) if response.status().is_success() => response,
            Ok(response) => return ViesAnswer::Unavailable(format!("HTTP {}", response.status())),
            Err(err) => return ViesAnswer::Unavailable(err.to_string()),
        };
        let answer = match response
            .bytes()
            .await
            .map(|body| serde_json::from_slice::<CheckResponse>(&body))
        {
            Ok(Ok(answer)) => answer,
            Ok(Err(err)) => return ViesAnswer::Unavailable(err.to_string()),
            Err(err) => return ViesAnswer::Unavailable(err.to_string()),
        };
        match answer.user_error.as_deref() {
            Some(error) if UNAVAILABLE.contains(&error) => {
                ViesAnswer::Unavailable(error.to_owned())
            }
            _ if answer.valid => ViesAnswer::Valid {
                reference: answer.request_identifier.filter(|r| !r.is_empty()),
            },
            _ => ViesAnswer::Invalid,
        }
    }
}

/// Asks VIES a few times before giving up: it is often briefly unavailable
/// for one member state.
pub async fn check_patiently(
    vies: &dyn Vies,
    vat_number: &str,
    requester: Option<&str>,
) -> ViesAnswer {
    let mut answer = ViesAnswer::Unavailable("not asked".to_owned());
    for attempt in 0..3u32 {
        answer = vies.check(vat_number, requester).await;
        if !matches!(answer, ViesAnswer::Unavailable(_)) {
            break;
        }
        tokio::time::sleep(Duration::from_secs(u64::from(attempt + 1))).await;
    }
    answer
}
