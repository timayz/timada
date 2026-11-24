//! What is read from Search Console under a shop's access.

use serde::Deserialize;
use serde_json::json;
use timada_google::SearchNumbers;
use timada_google::aggregator::{PageQueryRow, SearchRow};

use super::{Google, GoogleFault};

/// What a day's rows are grouped by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dimension {
    Query,
    Page,
}

impl Dimension {
    fn key(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Page => "page",
        }
    }
}

/// What a site did on one day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DayNumbers {
    /// `2026-11-20`, a day of Google's (Pacific time).
    pub day: String,
    pub numbers: SearchNumbers,
}

#[derive(Deserialize)]
struct Analytics {
    #[serde(default)]
    rows: Vec<AnalyticsRow>,
}

/// Google writes the four figures as decimals, the counts included.
#[derive(Deserialize)]
struct AnalyticsRow {
    #[serde(default)]
    keys: Vec<String>,
    #[serde(default)]
    clicks: f64,
    #[serde(default)]
    impressions: f64,
    #[serde(default)]
    position: f64,
}

impl AnalyticsRow {
    fn numbers(&self) -> SearchNumbers {
        // `as` saturates, and takes what is no number for zero.
        SearchNumbers {
            clicks: self.clicks.round() as u64,
            impressions: self.impressions.round() as u64,
            position_milli: (self.position * 1000.0).round() as u32,
        }
    }
}

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

    /// Where a site's search numbers are asked for. The site is one part
    /// of the path, its slashes written out.
    fn analytics_url(&self, site: &str) -> Result<reqwest::Url, GoogleFault> {
        let refused = |reason: String| GoogleFault::Refused(reason);
        let mut url = reqwest::Url::parse(&self.inner()?.search_console_url)
            .map_err(|err| refused(err.to_string()))?;
        url.path_segments_mut()
            .map_err(|()| refused("Search Console's address takes no path".into()))?
            .extend(["sites", site, "searchAnalytics", "query"]);
        Ok(url)
    }

    /// What `site` did in web search on each day from `first` to `last`
    /// (`2026-11-20`, both included), the oldest first. Google answers with
    /// the days it has finished counting only (two to three days behind),
    /// and leaves out a day nothing was shown on. One question, grouped by
    /// day alone: the cheap kind.
    pub async fn search_days(
        &self,
        access_token: &str,
        site: &str,
        first: &str,
        last: &str,
    ) -> Result<Vec<DayNumbers>, GoogleFault> {
        let body = json!({
            "startDate": first,
            "endDate": last,
            "dimensions": ["date"],
            "type": "web",
            "dataState": "final",
            "rowLimit": 1000,
        });
        let answer: Analytics = self
            .post(self.analytics_url(site)?, access_token, &body)
            .await?;
        let mut days: Vec<DayNumbers> = answer
            .rows
            .iter()
            .filter_map(|row| {
                let day = row.keys.first()?;
                (day.len() == 10 && timada_core::civil::parse_date(day).is_some()).then(|| {
                    DayNumbers {
                        day: day.clone(),
                        numbers: row.numbers(),
                    }
                })
            })
            .collect();
        days.sort_by(|a, b| a.day.cmp(&b.day));
        Ok(days)
    }

    /// The most clicked queries, or pages, of `site` in web search on
    /// `day`, `limit` at most, as Google sorts them: by clicks. One
    /// question over one day and one grouping: Google counts grouping by
    /// query or by page as costly, both at once as the most, and a longer
    /// range as more.
    pub async fn search_rows(
        &self,
        access_token: &str,
        site: &str,
        day: &str,
        dimension: Dimension,
        limit: usize,
    ) -> Result<Vec<SearchRow>, GoogleFault> {
        let body = json!({
            "startDate": day,
            "endDate": day,
            "dimensions": [dimension.key()],
            "type": "web",
            "dataState": "final",
            "rowLimit": limit.clamp(1, 25_000),
        });
        let answer: Analytics = self
            .post(self.analytics_url(site)?, access_token, &body)
            .await?;
        Ok(answer
            .rows
            .iter()
            .filter_map(|row| {
                let numbers = row.numbers();
                Some(SearchRow {
                    key: row.keys.first()?.clone(),
                    clicks: numbers.clicks,
                    impressions: numbers.impressions,
                    position_milli: numbers.position_milli,
                })
            })
            .collect())
    }

    /// The queries each page of `site` showed for in web search on `day`,
    /// `limit` at most, in Google's order (by clicks, which says little of
    /// a page nobody reaches yet: the caller keeps the most shown). The
    /// kind of question Google counts as the most costly, so it is asked
    /// over one day, once a day.
    pub async fn search_page_queries(
        &self,
        access_token: &str,
        site: &str,
        day: &str,
        limit: usize,
    ) -> Result<Vec<PageQueryRow>, GoogleFault> {
        let body = json!({
            "startDate": day,
            "endDate": day,
            "dimensions": ["page", "query"],
            "type": "web",
            "dataState": "final",
            "rowLimit": limit.clamp(1, 25_000),
        });
        let answer: Analytics = self
            .post(self.analytics_url(site)?, access_token, &body)
            .await?;
        Ok(answer
            .rows
            .iter()
            .filter_map(|row| {
                let numbers = row.numbers();
                Some(PageQueryRow {
                    page: row.keys.first()?.clone(),
                    query: row.keys.get(1)?.clone(),
                    clicks: numbers.clicks,
                    impressions: numbers.impressions,
                    position_milli: numbers.position_milli,
                })
            })
            .collect())
    }
}
