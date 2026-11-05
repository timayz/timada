//! `/orders/{order_id}/documents/{document_id}`: one of them, as PDF, for
//! the browser that placed the order.

use timada_storefront::document_pdf;
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::not_found,
        header::{CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_TYPE},
        module_param, path_param,
        response::IntoResponse,
        route,
    },
};

use crate::app::orders::order_id::OrderId;
use crate::shopper::holds_order;

module_param!(pub document_id);

#[route(GET)]
pub async fn pdf(cx: &Cx) -> Result<impl IntoResponse> {
    let order_id = path_param::<OrderId>(cx);
    if !holds_order(cx, order_id).await? {
        return Err(not_found().into());
    }
    let document_id = path_param::<DocumentId>(cx);
    let Some(pdf) = document_pdf(cx, order_id, document_id).await? else {
        return Err(not_found().into());
    };
    Ok((
        [
            (CONTENT_TYPE, "application/pdf".to_owned()),
            (
                CONTENT_DISPOSITION,
                format!("inline; filename=\"{}\"", pdf.file_name),
            ),
            (CACHE_CONTROL, "private, no-store".to_owned()),
        ],
        pdf.bytes,
    ))
}
