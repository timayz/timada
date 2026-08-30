//! Admin login and logout. Mounted at the site root, outside the protected
//! admin nest, so the login form itself never bounces through [`require_admin`].

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

use crate::admin_users::verify_admin;
use crate::middleware::LOGIN_PATH;
use crate::session::{
    ADMIN_SESSION_TTL_MILLIS, SessionKind, clear_session_cookie, create_session, delete_session,
    load_session, session_cookie, session_token,
};
use crate::state::AuthState;

/// Where a successful login lands.
const ADMIN_HOME: &str = "/admin";

pub fn admin_auth_router(state: AuthState) -> Router {
    Router::new()
        .route(LOGIN_PATH, get(login_page).post(submit_login))
        .route("/admin/logout", post(logout))
        .with_state(state)
}

#[derive(Template)]
#[template(path = "admin/login.html")]
struct LoginTemplate {
    error: Option<String>,
}

async fn login_page(
    State(state): State<AuthState>,
    jar: CookieJar,
) -> AppResult<impl IntoResponse> {
    // Already signed in? Straight to the dashboard.
    if let Some(token) = session_token(&jar)
        && load_session(&state.read_pool, &token, SessionKind::Admin)
            .await?
            .is_some()
    {
        return Ok(Redirect::to(ADMIN_HOME).into_response());
    }

    Ok(HtmlTemplate(LoginTemplate { error: None }).into_response())
}

#[derive(Deserialize)]
struct LoginForm {
    email: String,
    password: String,
}

async fn submit_login(
    State(state): State<AuthState>,
    jar: CookieJar,
    Form(form): Form<LoginForm>,
) -> AppResult<impl IntoResponse> {
    let Some(admin_id) = verify_admin(&state.read_pool, &form.email, &form.password).await? else {
        return Ok((
            StatusCode::UNAUTHORIZED,
            HtmlTemplate(LoginTemplate {
                error: Some("Unknown email or wrong password.".to_owned()),
            }),
        )
            .into_response());
    };

    let token = create_session(
        &state.write_pool,
        &admin_id,
        SessionKind::Admin,
        ADMIN_SESSION_TTL_MILLIS,
    )
    .await?;

    Ok((jar.add(session_cookie(token)), Redirect::to(ADMIN_HOME)).into_response())
}

async fn logout(State(state): State<AuthState>, jar: CookieJar) -> AppResult<impl IntoResponse> {
    if let Some(token) = session_token(&jar) {
        delete_session(&state.write_pool, &token).await?;
    }
    Ok((jar.add(clear_session_cookie()), Redirect::to(LOGIN_PATH)))
}
