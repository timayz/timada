//! `GET {prefix}/orders/{order_id}/documents/{document_id}`: one of an
//! order's documents as PDF, for a browser that holds the order.

use topcoat_core::context::Cx;
use topcoat_core::error::Result;
use topcoat_router::error::not_found;
use topcoat_router::header::{CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_TYPE};
use topcoat_router::response::Response;
use topcoat_router::{Body, HeaderValue, path_param_segment};

use crate::invoice::document_pdf;
use crate::session::holds_order;

use super::action;

pub(crate) async fn pdf(cx: &Cx) -> Result<Response> {
    let order_id = path_param_segment(cx, "order_id").to_owned();
    if !holds_order(cx, &order_id).await? {
        return Err(not_found().into());
    }
    let document_id = path_param_segment(cx, "document_id");
    let Some(pdf) = document_pdf(cx, &order_id, document_id).await? else {
        return Err(not_found().into());
    };
    let mut response = Response::new(Body::from(pdf.bytes));
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/pdf"));
    if let Ok(disposition) =
        HeaderValue::from_str(&format!("inline; filename=\"{}\"", pdf.file_name))
    {
        headers.insert(CONTENT_DISPOSITION, disposition);
    }
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    Ok(response)
}

action!(pdf_route = pdf);
