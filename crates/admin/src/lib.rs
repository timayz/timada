//! Timada umbrella admin.
//!
//! Composes every service crate's `admin_router()` behind one router that a
//! consumer nests into their own axum app:
//!
//! ```ignore
//! let app = axum::Router::new().nest(
//!     "/admin",
//!     timada_admin::router(services).layer(axum::middleware::from_fn(my_auth)),
//! );
//! ```
//!
//! Authentication is deliberately not provided here — wrap the returned router
//! in whatever tower layer your app uses. Admin templates currently hardcode
//! the `/admin` prefix in links, so mount it at `/admin`.

use axum::Router;
use axum::routing::get;
use timada_web::HtmlTemplate;

/// The per-service states the admin router composes.
pub struct AdminServices {
    pub catalog: timada_catalog::CatalogState,
    pub region: timada_region::RegionState,
    pub promotion: timada_promotion::PromotionState,
    pub order: timada_order::OrderState,
    pub invoice: timada_invoice::InvoiceState,
    pub payment: timada_payment::PaymentState,
    pub shipping: timada_shipping::ShippingState,
    pub dropship: timada_dropship::DropshipState,
}

#[derive(askama::Template)]
#[template(path = "admin/dashboard.html")]
struct DashboardPage;

async fn dashboard() -> HtmlTemplate<DashboardPage> {
    HtmlTemplate(DashboardPage)
}

/// The full admin surface, ready to be nested at `/admin`.
pub fn router(services: AdminServices) -> Router {
    Router::new()
        .route("/", get(dashboard))
        .nest("/catalog", timada_catalog::admin_router(services.catalog))
        .nest("/regions", timada_region::admin_router(services.region))
        .nest(
            "/discounts",
            timada_promotion::admin_router(services.promotion),
        )
        .nest("/orders", timada_order::admin_router(services.order))
        .nest("/invoices", timada_invoice::admin_router(services.invoice))
        .nest("/payments", timada_payment::admin_router(services.payment))
        .nest(
            "/shipping",
            timada_shipping::admin_router(services.shipping),
        )
        .nest(
            "/suppliers",
            timada_dropship::admin_router(services.dropship),
        )
}
