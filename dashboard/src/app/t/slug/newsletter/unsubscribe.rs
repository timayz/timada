//! `POST /t/{slug}/newsletter/unsubscribe` (`subscriber_id=…`), for members
//! with `newsletter:manage`: takes a subscriber off the list (or withdraws an
//! address not confirmed yet). Off it already is a no-op.

use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{SubscriberForm, act};

#[route(POST)]
pub async fn unsubscribe(cx: &Cx, Form(form): Form<SubscriberForm>) -> Result<SeeOther> {
    act(
        cx,
        form,
        "Subscriber taken off the list; the list updates in a moment.",
        async |newsletter, access, id| newsletter.unsubscribe(&access.user_id, id).await,
    )
    .await
}
