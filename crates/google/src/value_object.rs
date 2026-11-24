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
