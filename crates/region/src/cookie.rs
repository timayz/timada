//! The region cookie: which currency zone this browser shops in.

use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use sqlx::SqlitePool;

use crate::projections::{RegionRow, list_regions};

/// Name of the cookie holding the chosen region's id.
pub const REGION_COOKIE: &str = "timada_region";

/// Build the cookie for `region_id`. Long-lived — a returning shopper should
/// land in the currency they chose last time. `make_permanent` (the cookie
/// crate's 20-year idiom) stands in for an explicit `Max-Age`, which would
/// need a direct `time` dependency for no behavioural difference.
pub fn region_cookie(region_id: String) -> Cookie<'static> {
    let mut cookie = Cookie::build((REGION_COOKIE, region_id))
        .path("/")
        .same_site(SameSite::Lax)
        .http_only(true)
        .build();
    cookie.make_permanent();
    cookie
}

/// The region this browser shops in.
///
/// The cookie's choice wins when it still names a real region; otherwise the
/// first region alphabetically. `None` only when no region exists at all —
/// a store that has not been seeded.
pub async fn current_region(
    read_pool: &SqlitePool,
    jar: &CookieJar,
) -> anyhow::Result<Option<RegionRow>> {
    let regions = list_regions(read_pool).await?;
    let chosen = jar.get(REGION_COOKIE).map(|cookie| cookie.value());

    Ok(match chosen {
        Some(id) => regions
            .iter()
            .find(|region| region.id == id)
            .cloned()
            .or_else(|| regions.first().cloned()),
        None => regions.first().cloned(),
    })
}
