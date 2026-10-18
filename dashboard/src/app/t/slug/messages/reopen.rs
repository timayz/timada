//! `POST /t/{slug}/messages/reopen` (`message_id=…`), for members with
//! `support:manage`: puts a message back among the waiting ones. Already
//! open is a no-op.

use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{MessageForm, act};

#[route(POST)]
pub async fn reopen(cx: &Cx, Form(form): Form<MessageForm>) -> Result<SeeOther> {
    act(
        cx,
        form,
        "Message reopened; the list updates in a moment.",
        async |support, actor, id| support.reopen_message(actor, id).await,
    )
    .await
}
