//! Storefront account pages. Merged at the site root, so these paths are
//! public URLs.

use askama::Template;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect};
use axum::routing::{get, post};
use axum::{Form, Router};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use timada_core::AppResult;
use timada_web::HtmlTemplate;

use crate::commands::{LoginError, RegisterError, login_customer, register_customer};
use crate::current::current_customer;
use crate::state::CustomerState;
use crate::view::CustomerView;

/// Where a fresh login or registration lands.
const ACCOUNT_PATH: &str = "/account";

pub fn store_router(state: CustomerState) -> Router {
    Router::new()
        .route("/register", get(register_page).post(submit_register))
        .route("/login", get(login_page).post(submit_login))
        .route("/logout", post(logout))
        .route(ACCOUNT_PATH, get(account_page))
        .with_state(state)
}

#[derive(Template)]
#[template(path = "store/register.html")]
struct RegisterTemplate {
    error: Option<String>,
    email: String,
    full_name: String,
}

#[derive(Template)]
#[template(path = "store/login.html")]
struct LoginTemplate {
    error: Option<String>,
    email: String,
}

#[derive(Template)]
#[template(path = "store/account.html")]
struct AccountTemplate {
    customer: CustomerView,
}

async fn register_page(
    State(state): State<CustomerState>,
    jar: CookieJar,
) -> AppResult<impl IntoResponse> {
    if current_customer(&state, &jar).await?.is_some() {
        return Ok(Redirect::to(ACCOUNT_PATH).into_response());
    }
    Ok(HtmlTemplate(RegisterTemplate {
        error: None,
        email: String::new(),
        full_name: String::new(),
    })
    .into_response())
}

#[derive(Deserialize)]
struct RegisterForm {
    email: String,
    full_name: String,
    password: String,
}

async fn submit_register(
    State(state): State<CustomerState>,
    jar: CookieJar,
    Form(form): Form<RegisterForm>,
) -> AppResult<impl IntoResponse> {
    let refused = match register_customer(
        &state.ctx.executor,
        &state.auth.write_pool,
        &form.email,
        &form.full_name,
        &form.password,
    )
    .await
    {
        Ok(_) => None,
        Err(RegisterError::Storage(source)) => return Err(timada_core::AppError::Internal(source)),
        Err(refused) => Some(refused.to_string()),
    };

    if let Some(error) = refused {
        // Re-render with what they typed (never the password) so a typo does
        // not cost the whole form.
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            HtmlTemplate(RegisterTemplate {
                error: Some(error),
                email: form.email,
                full_name: form.full_name,
            }),
        )
            .into_response());
    }

    // Freshly registered: sign them straight in with the same credentials.
    let (_, token) = match login_customer(&state.auth, &form.email, &form.password).await {
        Ok(session) => session,
        Err(LoginError::Storage(source)) => return Err(timada_core::AppError::Internal(source)),
        // The password we just stored no longer verifies — something is badly
        // wrong; fall back to the login form rather than a half-session.
        Err(LoginError::BadCredentials) => return Ok(Redirect::to("/login").into_response()),
    };

    Ok((
        jar.add(timada_auth::session_cookie(token)),
        Redirect::to(ACCOUNT_PATH),
    )
        .into_response())
}

async fn login_page(
    State(state): State<CustomerState>,
    jar: CookieJar,
) -> AppResult<impl IntoResponse> {
    if current_customer(&state, &jar).await?.is_some() {
        return Ok(Redirect::to(ACCOUNT_PATH).into_response());
    }
    Ok(HtmlTemplate(LoginTemplate {
        error: None,
        email: String::new(),
    })
    .into_response())
}

#[derive(Deserialize)]
struct LoginForm {
    email: String,
    password: String,
}

async fn submit_login(
    State(state): State<CustomerState>,
    jar: CookieJar,
    Form(form): Form<LoginForm>,
) -> AppResult<impl IntoResponse> {
    match login_customer(&state.auth, &form.email, &form.password).await {
        Ok((_, token)) => Ok((
            jar.add(timada_auth::session_cookie(token)),
            Redirect::to(ACCOUNT_PATH),
        )
            .into_response()),
        Err(LoginError::BadCredentials) => Ok((
            StatusCode::UNAUTHORIZED,
            HtmlTemplate(LoginTemplate {
                error: Some(LoginError::BadCredentials.to_string()),
                email: form.email,
            }),
        )
            .into_response()),
        Err(LoginError::Storage(source)) => Err(timada_core::AppError::Internal(source)),
    }
}

async fn logout(
    State(state): State<CustomerState>,
    jar: CookieJar,
) -> AppResult<impl IntoResponse> {
    if let Some(token) = timada_auth::session_token(&jar) {
        timada_auth::delete_session(&state.auth.write_pool, &token).await?;
    }
    Ok((
        jar.add(timada_auth::clear_session_cookie()),
        Redirect::to("/"),
    ))
}

async fn account_page(
    State(state): State<CustomerState>,
    jar: CookieJar,
) -> AppResult<impl IntoResponse> {
    let Some(customer) = current_customer(&state, &jar).await? else {
        return Ok(Redirect::to("/login").into_response());
    };
    Ok(HtmlTemplate(AccountTemplate { customer }).into_response())
}
