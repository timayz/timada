use askama::Template;
use axum::{
    http::{header, StatusCode, Uri},
    response::IntoResponse,
};
use rust_embed::RustEmbed;

use crate::html_template::HtmlTemplate;

#[derive(RustEmbed)]
#[folder = "public/"]
#[prefix = "/static/"]
struct Assets;

pub async fn static_handler(uri: Uri) -> impl IntoResponse {
    let mut path = uri.to_string();

    if !path.starts_with("/static/") {
        let template = NotFoundTemplate;
        return (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/html")],
            HtmlTemplate(template),
        )
            .into_response();
    }

    if let Some(query) = uri.query() {
        path = path.replace(&format!("?{query}"), "");
    }

    match Assets::get(path.as_str()) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.as_ref())], content.data).into_response()
        }
        None => (StatusCode::NOT_FOUND, "404 Not Found").into_response(),
    }
}

#[derive(Template)]
#[template(path = "404.html")]
struct NotFoundTemplate;
