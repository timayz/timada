use timada_tenancy::PERMISSION_ORDERS_READ;
use topcoat::{
    Result,
    context::{Cx, context},
    router::{query_params, response::IntoResponse, route},
};

use crate::app::t::slug::require_tenant;
use crate::db::Db;
use crate::export::{ExportQuery, Span, download_headers, file_name, orders_csv};

/// The orders placed over the days asked for, one line per item, as a CSV file, for members with
/// `orders:read`.
#[route(GET)]
pub async fn export(cx: &Cx) -> Result<impl IntoResponse> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ORDERS_READ)
        .await?;
    let query = query_params::<ExportQuery>(cx)?;
    let zone =
        jiff::tz::TimeZone::get(access.tenant.shop_time_zone()).unwrap_or(jiff::tz::TimeZone::UTC);
    let span = Span::of(query, &zone);
    let name = file_name("orders", &access.tenant.slug, &span);
    Ok((
        download_headers(&name),
        orders_csv(
            context::<Db>(cx).clone(),
            access.tenant.id.clone(),
            span.period,
        ),
    ))
}
