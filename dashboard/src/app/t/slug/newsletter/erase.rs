//! `POST /t/{slug}/newsletter/erase` (`subscriber_id=…`), for members with
//! `newsletter:manage`: erases a subscriber, as they asked. Their key is
//! destroyed, which leaves their address unreadable for good and the links
//! mailed to them dead, and the list drops them. Already erased is a no-op;
//! unknown is refused.

use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{SubscriberForm, act};

#[route(POST)]
pub async fn erase(cx: &Cx, Form(form): Form<SubscriberForm>) -> Result<SeeOther> {
    act(
        cx,
        form,
        "Subscriber erased; the list updates in a moment.",
        async |newsletter, access, id| {
            let keys = crate::app::store::shoppers::keys(cx, &access.tenant.id);
            newsletter
                .forget_subscriber(&*keys, &access.user_id, id)
                .await
        },
    )
    .await
}
