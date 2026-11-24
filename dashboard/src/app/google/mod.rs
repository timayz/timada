//! `/google`: what Google calls on the platform. Where it sends a member
//! back once the shop's account said yes to the platform's application: one
//! address for every shop, registered once on the application.

pub mod callback;

use topcoat::context::{Cx, context};

use crate::config::Config;

/// The address to register as the application's redirect URI.
pub fn callback_url(cx: &Cx) -> String {
    format!(
        "{}{}",
        context::<Config>(cx).server.url.trim_end_matches('/'),
        topcoat::router::href!(callback::callback).resolve(cx)
    )
}
