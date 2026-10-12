use timada_identity::ChallengePurpose;

use super::Started;
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        route,
    },
};

use crate::app::auth::{login_url, verify_url};
use crate::auth::{
    cookies::{email_flow, is_fresh},
    now,
};

/// Mails another code for the flow in progress and keeps the flow alive.
#[route(POST)]
pub async fn resend(cx: &Cx) -> Result<SeeOther> {
    let store = email_flow(cx);
    let flow = match store.parse() {
        Ok(Some(store)) => store.get(),
        Ok(None) | Err(_) => return Ok(see_other(login_url(cx, Some("expired")))),
    };
    let Some(purpose) = ChallengePurpose::parse(&flow.purpose) else {
        return Ok(see_other(login_url(cx, Some("expired"))));
    };
    if !is_fresh(flow.issued_at, now()) {
        return Ok(see_other(login_url(cx, Some("expired"))));
    }

    let started = super::start_challenge(cx, &flow.email, purpose).await?;
    match started {
        // Asking again is the one place the throttle is an error to show.
        Ok(Started::AlreadySent) => Ok(see_other(verify_url(cx, Some("code_recently_sent"), None))),
        // A reset for an address nobody registered: the first step said "if
        // an account exists"; keep saying so.
        Ok(Started::Sent) | Err("not_registered") => {
            email_flow(cx)
                .set(crate::auth::cookies::EmailFlow {
                    issued_at: now(),
                    ..flow
                })
                .commit()?;
            Ok(see_other(verify_url(
                cx,
                None,
                Some("A new code was sent."),
            )))
        }
        Err(code) => Ok(see_other(verify_url(cx, Some(code), None))),
    }
}
