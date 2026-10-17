//! Signing in: the shopper gives their e-mail address and the shop mails
//! them a code, which the next page takes. There is no sign-up: the first
//! code entered creates the account.

use topcoat_core::context::Cx;
use topcoat_core::error::Result;
use topcoat_router::error::redirect;

use crate::actions::query;
use crate::context::menu;
use crate::query::CategoryNode;
use crate::session::{SignInFlow, sign_in_flow};
use crate::storefront::paths;
use crate::urls::safe_next;

use super::{Flash, flash};

#[derive(serde::Deserialize)]
struct SignInQuery {
    next: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInModel {
    pub nav: Vec<CategoryNode>,
    /// Where to go once signed in: a path on this site, else the account.
    pub next: String,
    /// The address a code was last asked for, to start from.
    pub email: String,
    pub error: Option<Flash>,
}

pub async fn sign_in_page(cx: &Cx) -> Result<SignInModel> {
    let query: SignInQuery = query(cx)?;
    Ok(SignInModel {
        nav: menu(cx).await?,
        next: safe_next(query.next.as_deref(), paths(cx).account_url(None)),
        email: sign_in_flow(cx).map(|flow| flow.email).unwrap_or_default(),
        error: flash(cx, query.error.as_deref()),
    })
}

#[derive(serde::Deserialize)]
struct CodeQuery {
    error: Option<String>,
    resent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInCodeModel {
    pub nav: Vec<CategoryNode>,
    /// The sign-in on its way: the address the code went to.
    pub flow: SignInFlow,
    pub error: Option<Flash>,
    /// Whether a new code was just sent.
    pub resent: bool,
    /// The sign-in page again, to use another address.
    pub other_address_url: String,
}

/// Redirects to the sign-in page when no code is on its way.
pub async fn sign_in_code_page(cx: &Cx) -> Result<SignInCodeModel> {
    let paths = paths(cx);
    let Some(flow) = sign_in_flow(cx) else {
        return Err(redirect(paths.sign_in_url(None, None)).into());
    };
    let query: CodeQuery = query(cx)?;
    Ok(SignInCodeModel {
        nav: menu(cx).await?,
        other_address_url: paths.sign_in_url(Some(&flow.next), None),
        flow,
        error: flash(cx, query.error.as_deref()),
        resent: query.resent.is_some(),
    })
}
