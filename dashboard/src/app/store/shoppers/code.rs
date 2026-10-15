//! `POST /store/shoppers/code` `email=…`: mails a sign-in code to the
//! address, from the shop. Any address gets one: the first sign-in creates
//! the account. 202 `{}` once mailed; 429 `{"error": "code_recently_sent"}`
//! within a minute of the last one.

use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{StatusCode, content::Form, route},
};

use super::{Answer, answer_error, status};
use crate::auth::mail::send_shopper_code;
use crate::auth::now;

#[derive(Deserialize)]
pub struct CodeForm {
    email: String,
}

#[route(POST)]
pub async fn code(cx: &Cx, Form(form): Form<CodeForm>) -> Result<Answer> {
    let access = match super::super::storefront(cx).await? {
        Ok(access) => access,
        Err(code) => return status(code),
    };
    let hold = super::super::hold(cx, &access).await?;
    let issued = match timada_shoppers::Command(hold.executor().clone())
        .issue_code(&form.email, now())
        .await
    {
        Ok(issued) => issued,
        Err(err) => return answer_error(err),
    };
    send_shopper_code(cx, &issued.email, &access.tenant, &issued.code).await?;
    status(StatusCode::ACCEPTED)
}
