use timada_tenancy::PERMISSION_ORDERS_READ;
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::not_found,
        header::{CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_TYPE},
        path_param,
        response::IntoResponse,
        route,
    },
};

use super::DocumentId;
use crate::app::t::slug::invoices::invoicing;
use crate::app::t::slug::require_tenant;

/// The document as the PDF it was issued as, shown in the browser.
#[route(GET)]
pub async fn pdf(cx: &Cx) -> Result<impl IntoResponse> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ORDERS_READ)
        .await?;
    let document_id = path_param::<DocumentId>(cx);
    let document = invoicing(&access)?
        .load_document(document_id)
        .await
        .map_err(|err| topcoat::Error::from_anyhow(err.into()))?
        .ok_or_else(not_found)?;
    let name = timada_invoicing::pdf::file_name(&document);
    let bytes = tokio::task::spawn_blocking(move || timada_invoicing::pdf::render(&document))
        .await
        .map_err(|err| topcoat::Error::from_anyhow(err.into()))?
        .map_err(topcoat::Error::from_anyhow)?;
    Ok((
        [
            (CONTENT_TYPE, "application/pdf".to_owned()),
            (CONTENT_DISPOSITION, format!("inline; filename=\"{name}\"")),
            (CACHE_CONTROL, "private, no-store".to_owned()),
        ],
        bytes,
    ))
}
