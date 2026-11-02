//! `/aliexpress`: what AliExpress calls on the platform. Where it sends an
//! owner back once the company's account said yes to the platform's app
//! (one callback for every company), and where it pushes the status of the
//! orders the shops placed; both registered once on the app.

pub mod callback;
pub mod webhook;

use topcoat::context::{Cx, app_context};

use crate::config::Config;

/// The address to register as the app's callback.
pub fn callback_url(cx: &Cx) -> String {
    format!(
        "{}{}",
        app_context::<Config>(cx).server.url.trim_end_matches('/'),
        topcoat::router::href!(callback::callback).resolve(cx)
    )
}
