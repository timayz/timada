//! What is read from Merchant Center under a shop's access.
//!
//! Three things, through Google's Merchant API: the accounts the grant
//! reaches, the products of one with where each stands (a page of a
//! thousand at a time), and what Google holds against the account itself.
//!
//! Before any of it works, the platform's Cloud project has to be
//! registered once with a Merchant Center account of the operator's
//! (`developerRegistration:registerGcp`, see `docs/google.md`): until then
//! Google answers that the project is not registered, which is told as a
//! refusal in its words rather than as a grant to renew.

use serde::Deserialize;
use timada_google::aggregator::{MerchantAccountIssue, MerchantIssue, MerchantProductRow};
use timada_google::{
    MERCHANT_ABSENT, MERCHANT_ACCEPTED, MERCHANT_PENDING, MERCHANT_REFUSED, MerchantAccount,
    MerchantReport,
};

use super::{Google, GoogleFault, reason, unavailable};

/// How many products a page of the list carries: the most Google gives.
const PRODUCTS_A_PAGE: usize = 1_000;

/// How many pages of products a read follows: past it the feed is told as
/// too large rather than read in part.
const MAX_PRODUCT_PAGES: usize = 100;

#[derive(Deserialize)]
struct Accounts {
    #[serde(default)]
    accounts: Vec<Account>,
}

#[derive(Deserialize)]
struct Account {
    #[serde(default, rename = "accountId")]
    account_id: serde_json::Value,
    #[serde(default, rename = "accountName")]
    account_name: String,
}

#[derive(Deserialize)]
struct Products {
    #[serde(default)]
    products: Vec<Product>,
    #[serde(default, rename = "nextPageToken")]
    next_page_token: String,
}

#[derive(Deserialize)]
struct Product {
    #[serde(default, rename = "offerId")]
    offer_id: String,
    #[serde(default, rename = "productStatus")]
    status: ProductStatus,
}

#[derive(Deserialize, Default)]
struct ProductStatus {
    #[serde(default, rename = "destinationStatuses")]
    destinations: Vec<Destination>,
    #[serde(default, rename = "itemLevelIssues")]
    issues: Vec<ItemIssue>,
}

#[derive(Deserialize)]
struct Destination {
    #[serde(default, rename = "reportingContext")]
    context: String,
    #[serde(default, rename = "approvedCountries")]
    approved: Vec<String>,
    #[serde(default, rename = "pendingCountries")]
    pending: Vec<String>,
    #[serde(default, rename = "disapprovedCountries")]
    disapproved: Vec<String>,
}

#[derive(Deserialize)]
struct ItemIssue {
    #[serde(default)]
    code: String,
    #[serde(default)]
    severity: String,
    #[serde(default)]
    attribute: String,
    #[serde(default, rename = "reportingContext")]
    context: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    detail: String,
    #[serde(default)]
    documentation: String,
}

#[derive(Deserialize)]
struct AccountIssues {
    #[serde(default, rename = "accountIssues")]
    issues: Vec<AccountIssue>,
}

#[derive(Deserialize)]
struct AccountIssue {
    #[serde(default)]
    title: String,
    #[serde(default)]
    severity: String,
    #[serde(default)]
    detail: String,
    #[serde(default, rename = "documentationUri")]
    documentation: String,
}

/// What Google's APIs answer a refused call with.
#[derive(Deserialize, Default)]
struct Refusal {
    #[serde(default)]
    error: RefusalError,
}

#[derive(Deserialize, Default)]
struct RefusalError {
    #[serde(default)]
    message: String,
}

/// The place Google names, as it is kept: `ads`, `listings`, or `None` for
/// one a shop does not read (display ads, local listings…).
fn context(named: &str) -> Option<&'static str> {
    match named {
        "SHOPPING_ADS" => Some("ads"),
        "FREE_LISTINGS" => Some("listings"),
        _ => None,
    }
}

impl Product {
    /// Where the product stands in `place`: refused as soon as one country
    /// refuses it, still looked at while one has not answered, accepted
    /// otherwise; not there at all when Google does not name the place.
    fn standing(&self, place: &str) -> &'static str {
        let Some(found) = self
            .status
            .destinations
            .iter()
            .find(|destination| context(&destination.context) == Some(place))
        else {
            return MERCHANT_ABSENT;
        };
        if !found.disapproved.is_empty() {
            MERCHANT_REFUSED
        } else if !found.pending.is_empty() {
            MERCHANT_PENDING
        } else if !found.approved.is_empty() {
            MERCHANT_ACCEPTED
        } else {
            MERCHANT_ABSENT
        }
    }

    fn row(self) -> MerchantProductRow {
        MerchantProductRow {
            ads: self.standing("ads").to_owned(),
            listings: self.standing("listings").to_owned(),
            issues: self
                .status
                .issues
                .into_iter()
                // An issue of a place the shop does not read says nothing
                // of the two it does; one that names no place is of both.
                .filter(|issue| issue.context.is_empty() || context(&issue.context).is_some())
                .map(|issue| MerchantIssue {
                    severity: match issue.severity.as_str() {
                        "DISAPPROVED" => "refused",
                        "DEMOTED" => "limited",
                        _ => "none",
                    }
                    .to_owned(),
                    context: context(&issue.context).unwrap_or_default().to_owned(),
                    code: issue.code,
                    attribute: issue.attribute,
                    description: issue.description,
                    detail: issue.detail,
                    documentation: issue.documentation,
                })
                .collect(),
            item_id: self.offer_id,
        }
    }
}

/// Opens what the Merchant API answered. A project not registered with a
/// Merchant Center account is answered like a bad access (401): told in
/// Google's words, since no renewed grant would change it.
fn open_merchant<T: serde::de::DeserializeOwned>(
    status: u16,
    body: &[u8],
) -> Result<T, GoogleFault> {
    if status == 200 {
        return serde_json::from_slice(body)
            .map_err(|err| GoogleFault::Unavailable(format!("unreadable answer: {err}")));
    }
    let refusal: Refusal = serde_json::from_slice(body).unwrap_or_default();
    let said = if refusal.error.message.is_empty() {
        format!("HTTP {status}")
    } else {
        refusal.error.message
    };
    match status {
        401 if said.contains("not registered") => Err(GoogleFault::Refused(reason(&said))),
        // The access is turned down: [`Google::read`] asks for another.
        401 => Err(GoogleFault::ConnectAgain),
        429 | 500.. => Err(GoogleFault::Unavailable(reason(&said))),
        _ => Err(GoogleFault::Refused(reason(&said))),
    }
}

impl Google {
    async fn merchant_get<T: serde::de::DeserializeOwned>(
        &self,
        access_token: &str,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T, GoogleFault> {
        let inner = self.inner()?;
        let url = reqwest::Url::parse_with_params(&format!("{}/{path}", inner.merchant_url), query)
            .map_err(|err| GoogleFault::Refused(err.to_string()))?;
        let response = inner
            .client
            .get(url)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(unavailable)?;
        let status = response.status().as_u16();
        let body = response.bytes().await.map_err(unavailable)?;
        open_merchant(status, &body)
    }

    /// The Merchant Center accounts the grant reaches, by name.
    pub async fn merchant_accounts(
        &self,
        access_token: &str,
    ) -> Result<Vec<MerchantAccount>, GoogleFault> {
        let list: Accounts = self
            .merchant_get(access_token, "accounts/v1/accounts", &[("pageSize", "500")])
            .await?;
        let mut accounts: Vec<MerchantAccount> = list
            .accounts
            .into_iter()
            .map(|account| MerchantAccount {
                // Google writes the number as text; a stand-in may not.
                account_id: match account.account_id {
                    serde_json::Value::String(id) => id,
                    serde_json::Value::Number(id) => id.to_string(),
                    _ => String::new(),
                },
                name: account.account_name,
            })
            .filter(|account| !account.account_id.is_empty())
            .collect();
        accounts.sort_by(|a, b| (&a.name, &a.account_id).cmp(&(&b.name, &b.account_id)));
        Ok(accounts)
    }

    /// One page of the account's products, each with where it stands, and
    /// what asks for the next page (`None` after the last).
    pub async fn merchant_products(
        &self,
        access_token: &str,
        account_id: &str,
        page_token: Option<&str>,
    ) -> Result<(Vec<MerchantProductRow>, Option<String>), GoogleFault> {
        let size = PRODUCTS_A_PAGE.to_string();
        let mut query = vec![("pageSize", size.as_str())];
        if let Some(token) = page_token {
            query.push(("pageToken", token));
        }
        let page: Products = self
            .merchant_get(
                access_token,
                &format!("products/v1/accounts/{account_id}/products"),
                &query,
            )
            .await?;
        Ok((
            page.products.into_iter().map(Product::row).collect(),
            Some(page.next_page_token).filter(|token| !token.is_empty()),
        ))
    }

    /// What Google holds against the account itself.
    pub async fn merchant_account_issues(
        &self,
        access_token: &str,
        account_id: &str,
    ) -> Result<Vec<MerchantAccountIssue>, GoogleFault> {
        let list: AccountIssues = self
            .merchant_get(
                access_token,
                &format!("accounts/v1/accounts/{account_id}/issues"),
                &[("pageSize", "100")],
            )
            .await?;
        Ok(list
            .issues
            .into_iter()
            .map(|issue| MerchantAccountIssue {
                title: issue.title,
                severity: issue.severity.to_lowercase(),
                detail: issue.detail,
                documentation: issue.documentation,
            })
            .collect())
    }

    /// Everything Merchant Center says of the account: every product, a
    /// page at a time, then what is held against the account, each
    /// question under the shop's access ([`Google::read`]).
    pub async fn merchant_report<E: evento::Executor + Clone>(
        &self,
        store: &E,
        connection: &timada_google::ConnectionState,
        account_id: &str,
    ) -> Result<MerchantReport, GoogleFault> {
        let mut report = MerchantReport::default();
        let mut next: Option<String> = None;
        for page in 0.. {
            if page == MAX_PRODUCT_PAGES {
                return Err(GoogleFault::Refused(format!(
                    "the account holds more than {} products",
                    MAX_PRODUCT_PAGES * PRODUCTS_A_PAGE
                )));
            }
            let token = next.clone();
            let (products, following) = self
                .read(store, connection, |access| {
                    let token = token.clone();
                    async move {
                        self.merchant_products(&access, account_id, token.as_deref())
                            .await
                    }
                })
                .await?;
            report.products.extend(products);
            match following {
                Some(token) => next = Some(token),
                None => break,
            }
        }
        report.account_issues = self
            .read(store, connection, |access| async move {
                self.merchant_account_issues(&access, account_id).await
            })
            .await?;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::google::tests::{SHOP, StandIn, connected, token_answer};

    #[tokio::test]
    async fn the_accounts_a_grant_reaches_are_listed_by_name() {
        let stand_in = StandIn::start(|request| {
            assert_eq!(
                request.line,
                "GET /merchant/accounts/v1/accounts?pageSize=500 HTTP/1.1"
            );
            (
                200,
                r#"{"accounts":[{"name":"accounts/5550002","accountId":"5550002","accountName":"Zèbre"},{"name":"accounts/5550001","accountId":5550001,"accountName":"Acme"}]}"#.into(),
            )
        })
        .await;
        let accounts = stand_in
            .google()
            .merchant_accounts("access-1")
            .await
            .unwrap();
        assert_eq!(
            accounts,
            vec![
                MerchantAccount {
                    account_id: "5550001".into(),
                    name: "Acme".into()
                },
                MerchantAccount {
                    account_id: "5550002".into(),
                    name: "Zèbre".into()
                },
            ]
        );
    }

    #[tokio::test]
    async fn a_read_follows_every_page_and_tells_where_each_product_stands() {
        let stand_in = StandIn::start(|request| {
            if request.line.starts_with("POST /token") {
                return token_answer("access-1");
            }
            assert_eq!(request.authorization, "Bearer access-1");
            if request.line.contains("/issues") {
                return (
                    200,
                    r#"{"accountIssues":[{"name":"accounts/5550001/issues/store-ownership","title":"Store ownership not verified","severity":"CRITICAL","detail":"Verify your store","documentationUri":"https://support.google.com/merchants/answer/176793"}]}"#.into(),
                );
            }
            if request.line.contains("pageToken=next-1") {
                // The last page: no token for another.
                return (
                    200,
                    r#"{"products":[{"offerId":"01BOWL","productStatus":{"destinationStatuses":[{"reportingContext":"SHOPPING_ADS","pendingCountries":["FR"]},{"reportingContext":"DISPLAY_ADS","disapprovedCountries":["FR"]}]}},{"offerId":"01BED"}]}"#.into(),
                );
            }
            assert_eq!(
                request.line,
                "GET /merchant/products/v1/accounts/5550001/products?pageSize=1000 HTTP/1.1"
            );
            (
                200,
                r#"{"products":[{"name":"accounts/5550001/products/fr~FR~01TREE","offerId":"01TREE","productStatus":{"destinationStatuses":[{"reportingContext":"SHOPPING_ADS","approvedCountries":["BE"],"disapprovedCountries":["FR"]},{"reportingContext":"FREE_LISTINGS","approvedCountries":["FR","BE"]}],"itemLevelIssues":[{"code":"image_too_small","severity":"DISAPPROVED","resolution":"merchant_action","attribute":"image_link","reportingContext":"SHOPPING_ADS","description":"Image too small","detail":"Use a larger image","documentation":"https://support.google.com/merchants/answer/6324350","applicableCountries":["FR"]},{"code":"local_only","severity":"DISAPPROVED","reportingContext":"LOCAL_INVENTORY_ADS","description":"No store"},{"code":"missing_gtin","severity":"DEMOTED","attribute":"gtin","description":"Limited performance due to missing identifiers"}]}}],"nextPageToken":"next-1"}"#.into(),
            )
        })
        .await;
        let google = stand_in.google();
        let store = evento::Turso::in_memory_concurrent().await.unwrap();
        let connection = connected(&store, &google, SHOP, "refresh-1").await;
        let report = google
            .merchant_report(&store, &connection, "5550001")
            .await
            .unwrap();
        // Two pages of products, then the account: one access for the lot.
        assert_eq!(stand_in.lines().len(), 4);
        assert_eq!(stand_in.renewals(), 1);

        assert_eq!(report.products.len(), 3);
        let tree = &report.products[0];
        // Refused in one country is refused; the free listings take it.
        assert_eq!(
            (
                tree.item_id.as_str(),
                tree.ads.as_str(),
                tree.listings.as_str()
            ),
            ("01TREE", MERCHANT_REFUSED, MERCHANT_ACCEPTED)
        );
        // The issue of a place the shop does not read is left out.
        assert_eq!(tree.issues.len(), 2);
        assert_eq!(
            tree.issues[0],
            MerchantIssue {
                code: "image_too_small".into(),
                severity: "refused".into(),
                attribute: "image_link".into(),
                context: "ads".into(),
                description: "Image too small".into(),
                detail: "Use a larger image".into(),
                documentation: "https://support.google.com/merchants/answer/6324350".into(),
            }
        );
        assert_eq!(
            (
                tree.issues[1].severity.as_str(),
                tree.issues[1].context.as_str()
            ),
            ("limited", "")
        );
        let bowl = &report.products[1];
        assert_eq!(
            (bowl.ads.as_str(), bowl.listings.as_str()),
            (MERCHANT_PENDING, MERCHANT_ABSENT)
        );
        // Not looked at yet: Google says nothing of it.
        let bed = &report.products[2];
        assert_eq!(
            (bed.ads.as_str(), bed.listings.as_str()),
            (MERCHANT_ABSENT, MERCHANT_ABSENT)
        );
        assert_eq!(
            report.account_issues,
            vec![MerchantAccountIssue {
                title: "Store ownership not verified".into(),
                severity: "critical".into(),
                detail: "Verify your store".into(),
                documentation: "https://support.google.com/merchants/answer/176793".into(),
            }]
        );
    }

    #[tokio::test]
    async fn a_project_not_registered_is_told_in_googles_words() {
        let stand_in = StandIn::start(|request| {
            if request.line.contains("/accounts/v1/accounts?") {
                (
                    401,
                    r#"{"error":{"code":401,"message":"GCP project with id timada and number 12345 is not registered with the merchant account.","status":"UNAUTHENTICATED"}}"#.into(),
                )
            } else if request.line.contains("/issues") {
                (401, r#"{"error":{"code":401,"message":"Invalid Credentials"}}"#.into())
            } else {
                (429, r#"{"error":{"code":429,"message":"Quota exceeded"}}"#.into())
            }
        })
        .await;
        let google = stand_in.google();
        assert_eq!(
            google.merchant_accounts("access-1").await,
            Err(GoogleFault::Refused(
                "GCP project with id timada and number 12345 is not registered with the merchant account.".into()
            ))
        );
        assert_eq!(
            google.merchant_account_issues("access-1", "5550001").await,
            Err(GoogleFault::ConnectAgain)
        );
        assert_eq!(
            google
                .merchant_products("access-1", "5550001", None)
                .await
                .map(|_| ()),
            Err(GoogleFault::Unavailable("Quota exceeded".into()))
        );
    }
}
