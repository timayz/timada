//! `POST /t/{slug}/messages/forget` (`message_id=…`), for members with
//! `support:manage`: erases a message, as its sender asked. Its key is
//! destroyed, which leaves what they wrote unreadable for good, and the list
//! drops it. Already erased is a no-op; unknown is refused.

use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{MessageForm, act};

#[route(POST)]
pub async fn forget(cx: &Cx, Form(form): Form<MessageForm>) -> Result<SeeOther> {
    act(
        cx,
        form,
        "Message erased; the list updates in a moment.",
        async |support, access, id| {
            let keys = crate::app::store::shoppers::keys(cx, &access.tenant.id);
            support.forget_message(&*keys, &access.user_id, id).await
        },
    )
    .await
}
