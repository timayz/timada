//! `POST /store/shoppers/session` `email=…&code=…`: checks the mailed code
//! and opens a session. 201 `{"token": …, "expires_at": unix seconds}`; the
//! storefront keeps the token in its shopper's browser and sends it back in
//! `X-Shopper-Token`. 422 `{"error": …}` for a wrong, expired or dead code.

use serde::Deserialize;
use serde_json::json;
use topcoat::{
    Result,
    context::Cx,
    router::{StatusCode, content::Form, route},
};

use super::{Answer, answer, answer_error, status};
use crate::auth::now;

#[derive(Deserialize)]
pub struct SessionForm {
    email: String,
    code: String,
}

#[route(POST)]
pub async fn session(cx: &Cx, Form(form): Form<SessionForm>) -> Result<Answer> {
    let access = match super::super::storefront(cx).await? {
        Ok(access) => access,
        Err(code) => return status(code),
    };
    let hold = super::super::hold(cx, &access).await?;
    match timada_shoppers::Command(hold.executor().clone())
        .sign_in(
            &*super::keys(cx, &access.tenant.id),
            &form.email,
            &form.code,
            now(),
        )
        .await
    {
        Ok(signed_in) => answer(
            StatusCode::CREATED,
            json!({ "token": signed_in.token, "expires_at": signed_in.expires_at }),
        ),
        Err(err) => answer_error(err),
    }
}
