use jiff::{Timestamp, tz::TimeZone};
use timada_tenancy::PERMISSION_ORDERS_READ;
use topcoat::{
    Result,
    context::{Cx, context},
    router::{query_params, response::IntoResponse, route},
};

use super::{Days, SalesQuery};
use crate::app::t::slug::require_tenant;
use crate::db::Db;
use crate::export::{download_headers, sales_csv};

/// The sales report of the days asked for (the last thirty without both)
/// as a CSV file, with what the ads cost, for members with `orders:read`.
#[route(GET)]
pub async fn export(cx: &Cx) -> Result<impl IntoResponse> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ORDERS_READ)
        .await?;
    let query = query_params::<SalesQuery>(cx)?;
    let zone = TimeZone::get(access.tenant.shop_time_zone()).unwrap_or(TimeZone::UTC);
    let today = Timestamp::now().to_zoned(zone.clone()).date();
    let days = Days::of(query, today);
    let name = format!(
        "sales-{}-{}-{}.csv",
        access.tenant.slug, days.first, days.last
    );
    Ok((
        download_headers(&name),
        sales_csv(
            context::<Db>(cx).clone(),
            access.tenant.id.clone(),
            days.period(&zone),
            (days.first, days.last),
        ),
    ))
}
