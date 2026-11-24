use serde::Deserialize;
use timada_orders::{ExportClock, ExportDestination, ExportEvery, ExportKind, OrderError};
use topcoat::{
    Result,
    context::{Cx, context},
    router::{
        content::Form,
        error::{SeeOther, forbidden, see_other},
        href, route,
    },
};

use super::shipping::{error_code, orders};
use super::{require_tenant, slug};
use crate::app::WebhookSealing;
use crate::app::auth::Flash;
use crate::scheduled_exports::{open_address, seal_address};

/// A schedule as an owner fills it in on the page of what it exports.
#[derive(Deserialize)]
pub struct ScheduleForm {
    /// `orders`, `customers` or `newsletter`.
    what: String,
    /// `remove` takes the schedule off; anything else saves it.
    #[serde(default)]
    action: String,
    #[serde(default)]
    every: String,
    /// `mail` or `url`.
    #[serde(default)]
    to: String,
    #[serde(default)]
    mail: String,
    #[serde(default)]
    url: String,
}

/// The page a kind of export is offered from, with a flash.
fn page_url(
    cx: &Cx,
    slug: &str,
    kind: ExportKind,
    error: Option<&str>,
    message: Option<&str>,
) -> String {
    let flash = Flash { error, message };
    match kind {
        ExportKind::Orders => href!(super::orders::index, slug: slug.to_owned())
            .query(flash)
            .resolve(cx),
        ExportKind::Customers => href!(super::customers::index, slug: slug.to_owned())
            .query(flash)
            .resolve(cx),
        ExportKind::Newsletter => href!(super::newsletter::index, slug: slug.to_owned())
            .query(flash)
            .resolve(cx),
        ExportKind::Sales => href!(super::sales::index, slug: slug.to_owned())
            .query(flash)
            .resolve(cx),
    }
}

/// Schedules an export, changes it or takes it off, and lands back on its
/// page. For the shop's owners alone, whatever their other members' roles
/// allow: a file of customers that leaves on its own leaves the shop's
/// hands (#625).
#[route(POST)]
pub async fn export_schedule(cx: &Cx, Form(form): Form<ScheduleForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    if !access.is_owner() {
        return Err(forbidden().into());
    }
    let Some(kind) = ExportKind::parse(&form.what) else {
        return Err(forbidden().into());
    };
    let slug = access.tenant.slug.as_str();
    let back = |error: Option<&str>, message: Option<&str>| {
        Ok(see_other(page_url(cx, slug, kind, error, message)))
    };
    let orders = orders(&access)?;
    if form.action == "remove" {
        return match orders.unschedule_export(&access.user_id, kind).await {
            Ok(()) => back(None, Some("Scheduled export taken off.")),
            Err(err) if err.is_user_facing() => back(Some(error_code(&err)), None),
            Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
        };
    }
    let Some(every) = ExportEvery::parse(&form.every) else {
        return back(Some("invalid_export_schedule"), None);
    };
    let destination = if form.to == "url" {
        let WebhookSealing(keys, allow_private_hosts) = context::<WebhookSealing>(cx);
        let Some(keys) = keys else {
            return back(Some("export_address_unavailable"), None);
        };
        let url = form.url.trim();
        let web = reqwest::Url::parse(url)
            .is_ok_and(|parsed| *allow_private_hosts || parsed.scheme() == "https");
        if !web || url.len() > 2000 {
            return back(Some("invalid_export_address"), None);
        }
        if !allow_private_hosts && crate::net::check_public_url(url).await.is_err() {
            return back(Some("export_address_not_public"), None);
        }
        // Sealed anew, the same address reads as another: one saved again
        // as it stands keeps its seal, and the schedule its place.
        let kept = orders
            .load_export_schedule(kind)
            .await
            .map_err(topcoat::Error::from_anyhow)?
            .and_then(|state| state.plan)
            .and_then(|plan| match plan.destination {
                ExportDestination::Url { sealed } => Some(sealed),
                ExportDestination::Mail(_) => None,
            })
            .filter(|sealed| {
                open_address(keys, access.tenant_id(), kind, sealed).as_deref() == Some(url)
            });
        ExportDestination::Url {
            sealed: match kept {
                Some(sealed) => sealed,
                None => seal_address(keys, access.tenant_id(), kind, url)
                    .map_err(topcoat::Error::from_anyhow)?,
            },
        }
    } else {
        ExportDestination::Mail(form.mail)
    };
    let zone =
        jiff::tz::TimeZone::get(access.tenant.shop_time_zone()).unwrap_or(jiff::tz::TimeZone::UTC);
    let set: std::result::Result<(), OrderError> = orders
        .schedule_export(
            &access.user_id,
            kind,
            every,
            destination,
            &ExportClock::new(crate::auth::now(), zone),
        )
        .await;
    match set {
        Ok(()) => back(None, Some("Scheduled export saved.")),
        Err(err) if err.is_user_facing() => back(Some(error_code(&err)), None),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
