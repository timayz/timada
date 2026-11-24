//! What is read from Search Console under a shop's access.

use serde::Deserialize;

use super::{Google, GoogleFault};

/// A site the account sees in Search Console.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    /// As Search Console names it: `https://shop.example/` for an address,
    /// `sc-domain:shop.example` for a whole domain.
    pub site: String,
    /// Whether the account owns it, rather than being let in by its owner.
    pub owner: bool,
}

#[derive(Deserialize)]
struct SiteList {
    #[serde(default, rename = "siteEntry")]
    site_entry: Vec<SiteEntry>,
}

#[derive(Deserialize)]
struct SiteEntry {
    #[serde(default, rename = "siteUrl")]
    site_url: String,
    #[serde(default, rename = "permissionLevel")]
    permission_level: String,
}

impl Google {
    /// The Search Console sites the account reads, by name. One it added
    /// without proving it is theirs (`siteUnverifiedUser`) has nothing to
    /// read and is left out.
    pub async fn sites(&self, access_token: &str) -> Result<Vec<Site>, GoogleFault> {
        let url = format!("{}/sites", self.inner()?.search_console_url);
        let url = reqwest::Url::parse(&url).map_err(|err| GoogleFault::Refused(err.to_string()))?;
        let list: SiteList = self.get(url, access_token).await?;
        let mut sites: Vec<Site> = list
            .site_entry
            .into_iter()
            .filter(|entry| {
                !entry.site_url.is_empty() && entry.permission_level != "siteUnverifiedUser"
            })
            .map(|entry| Site {
                owner: entry.permission_level == "siteOwner",
                site: entry.site_url,
            })
            .collect();
        sites.sort_by(|a, b| a.site.cmp(&b.site));
        Ok(sites)
    }
}
