//! `POST /t/{slug}/messages/handle` (`message_id=…`), for members with
//! `support:manage`: marks a message answered. Already handled is a no-op.

use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{MessageForm, act};

#[route(POST)]
pub async fn handle(cx: &Cx, Form(form): Form<MessageForm>) -> Result<SeeOther> {
    act(
        cx,
        form,
        "Message marked handled; the list updates in a moment.",
        async |support, access, id| support.handle_message(&access.user_id, id).await,
    )
    .await
}
