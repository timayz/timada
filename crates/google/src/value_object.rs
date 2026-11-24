use crate::error::GoogleError;

/// Reading the queries and pages of a site in Search Console.
pub const ACCESS_SEARCH_CONSOLE: &str = "search_console";
/// Reading and running the campaigns of a Google Ads account.
pub const ACCESS_ADS: &str = "ads";
/// Reading what Merchant Center says of the shop's products.
pub const ACCESS_MERCHANT_CENTER: &str = "merchant_center";

/// Every kind of access a connection may cover.
pub const ACCESSES: [&str; 3] = [ACCESS_SEARCH_CONSOLE, ACCESS_ADS, ACCESS_MERCHANT_CENTER];

/// Who writes what the host notices on its own (a grant that lapsed).
pub const GOOGLE_ACTOR: &str = "timada-google";

/// The longest account address kept.
pub const MAX_ACCOUNT_LEN: usize = 254;

/// The longest Search Console site name kept.
pub const MAX_SITE_LEN: usize = 300;

/// The stream of a shop's connection. Stream ids are one space for every
/// kind of aggregate, and the shop's own id names its tenant's stream.
pub fn connection_id(tenant_id: &str) -> String {
    evento::hash_ids(vec!["google-connection", tenant_id])
}

/// The stream of one finished day of a shop's site.
pub fn search_day_id(tenant_id: &str, site: &str, day: &str) -> String {
    evento::hash_ids(vec!["google-search-day", tenant_id, site, day])
}

/// The stream that says where a shop's daily look for new days stands.
pub fn search_watch_id(tenant_id: &str) -> String {
    evento::hash_ids(vec!["google-search-watch", tenant_id])
}

/// How many queries, and how many pages, are kept of a day: its most
/// clicked. A day's event carries both lists.
pub const MAX_SEARCH_ROWS: usize = 500;

/// How many pairs of a page and a query it showed for are kept of a day:
/// its most shown. They ride in the day's event with the two lists.
pub const MAX_PAGE_QUERY_ROWS: usize = 1_000;

/// The longest query or address kept: Google's own filters take 4,096
/// characters, and nothing a person types or a shop serves comes near.
pub const MAX_SEARCH_KEY_LEN: usize = 1_000;

/// The longest reason kept for a pass that failed.
pub const MAX_REASON_LEN: usize = 300;

/// How many days back a shop just connected is read for.
pub const BACKFILL_DAYS: u32 = 90;

/// For how many days a site's numbers are kept: sixteen months, which is
/// as far back as Search Console itself goes.
pub const KEPT_DAYS: u32 = 486;

/// For how many days the queries and the pages of a day are kept: the
/// longest period a page adds up, and a little more.
pub const KEPT_ROW_DAYS: u32 = 92;

/// How long after a pass that found everything the next is due.
pub const CHECK_EVERY_SECS: u64 = 24 * 60 * 60;

/// How long after a pass that left older days to fetch the next is due.
pub const CATCH_UP_SECS: u64 = 10 * 60;

/// How long after a pass that failed the next is due: an hour, doubled for
/// each failure in a row before it, a day at most.
pub fn retry_delay_secs(attempts: u32) -> u64 {
    let doublings = attempts.saturating_sub(1).min(5);
    (3_600u64 << doublings).min(CHECK_EVERY_SECS)
}

/// The day `days` before `day` (`2026-11-20`), as text that sorts the way
/// days do; `day` itself when it is no day.
pub fn day_before(day: &str, days: u32) -> String {
    match timada_core::civil::parse_date(day) {
        Some(at) => timada_core::civil::format_date(at.saturating_sub(u64::from(days) * 86_400)),
        None => day.to_owned(),
    }
}

/// What Google calls a kind of access when asked for it; `None` for a kind
/// that is not one of [`ACCESSES`].
pub fn scope_of(access: &str) -> Option<&'static str> {
    match access {
        ACCESS_SEARCH_CONSOLE => Some("https://www.googleapis.com/auth/webmasters.readonly"),
        ACCESS_ADS => Some("https://www.googleapis.com/auth/adwords"),
        ACCESS_MERCHANT_CENTER => Some("https://www.googleapis.com/auth/content"),
        _ => None,
    }
}

/// The kinds of access among `scopes`, as Google lists what an account
/// granted, in the order of [`ACCESSES`]. Full access to Search Console
/// reads it too.
pub fn accesses_of<'a>(scopes: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let scopes: Vec<&str> = scopes.into_iter().collect();
    ACCESSES
        .into_iter()
        .filter(|access| {
            scopes.iter().any(|scope| {
                Some(*scope) == scope_of(access)
                    || (*access == ACCESS_SEARCH_CONSOLE
                        && *scope == "https://www.googleapis.com/auth/webmasters")
            })
        })
        .map(str::to_owned)
        .collect()
}

/// A Search Console site as Search Console names it: an address with its
/// scheme and its closing slash (`https://shop.example/`), or a whole
/// domain (`sc-domain:shop.example`).
pub fn normalize_site(site: &str) -> Result<String, GoogleError> {
    let site = site.trim();
    let named = match site.strip_prefix("sc-domain:") {
        Some(domain) => !domain.is_empty() && !domain.contains('/'),
        None => {
            let rest = site
                .strip_prefix("https://")
                .or_else(|| site.strip_prefix("http://"));
            rest.is_some_and(|rest| rest.len() > 1 && rest.ends_with('/'))
        }
    };
    if !named
        || site.len() > MAX_SITE_LEN
        || site.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(GoogleError::InvalidSite);
    }
    Ok(site.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scopes_granted_name_the_kinds_of_access() {
        assert_eq!(
            accesses_of([
                "openid",
                "https://www.googleapis.com/auth/userinfo.email",
                "https://www.googleapis.com/auth/content",
                "https://www.googleapis.com/auth/webmasters.readonly",
            ]),
            vec![ACCESS_SEARCH_CONSOLE, ACCESS_MERCHANT_CENTER]
        );
        assert_eq!(
            accesses_of(["https://www.googleapis.com/auth/webmasters"]),
            vec![ACCESS_SEARCH_CONSOLE]
        );
        assert!(accesses_of(["openid"]).is_empty());
        for access in ACCESSES {
            assert_eq!(accesses_of(scope_of(access)), vec![access]);
        }
    }

    #[test]
    fn a_failed_pass_waits_longer_each_time_up_to_a_day() {
        assert_eq!(retry_delay_secs(0), 3_600);
        assert_eq!(retry_delay_secs(1), 3_600);
        assert_eq!(retry_delay_secs(2), 7_200);
        assert_eq!(retry_delay_secs(5), 57_600);
        assert_eq!(retry_delay_secs(6), 86_400);
        assert_eq!(retry_delay_secs(u32::MAX), 86_400);
    }

    #[test]
    fn days_are_counted_back_across_months_and_years() {
        assert_eq!(day_before("2026-11-20", 0), "2026-11-20");
        assert_eq!(day_before("2026-11-20", 20), "2026-10-31");
        assert_eq!(day_before("2026-01-01", 1), "2025-12-31");
        assert_eq!(day_before("2024-03-01", 1), "2024-02-29");
        assert_eq!(day_before("soon", 3), "soon");
        // Every stream has an id of its own.
        assert_ne!(search_watch_id("shop"), connection_id("shop"));
        assert_ne!(
            search_day_id("shop", "sc-domain:a.example", "2026-11-20"),
            search_day_id("shop", "sc-domain:b.example", "2026-11-20")
        );
    }

    #[test]
    fn a_site_is_named_as_search_console_names_it() {
        assert_eq!(
            normalize_site(" https://shop.example/ ").unwrap(),
            "https://shop.example/"
        );
        assert_eq!(
            normalize_site("sc-domain:shop.example").unwrap(),
            "sc-domain:shop.example"
        );
        assert_eq!(
            normalize_site("http://shop.example/fr/").unwrap(),
            "http://shop.example/fr/"
        );
        for site in [
            "",
            "shop.example",
            "https://shop.example",
            "https://",
            "sc-domain:",
            "sc-domain:shop.example/fr",
            "https://shop .example/",
        ] {
            assert!(
                matches!(normalize_site(site), Err(GoogleError::InvalidSite)),
                "{site}"
            );
        }
    }
}
