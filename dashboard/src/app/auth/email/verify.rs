use serde::Deserialize;
use timada_identity::{
    ChallengePurpose, IdentityError, RegisterWithPassword, ResetPassword, SignInWithPassword,
    hash_password,
};
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, redirect, see_other},
        href, page, query_params, route,
    },
    session,
    view::{View, view},
};

use crate::app::auth::{
    error_code, forgot_password_url, home_url_with_message, login_url, register_url, signed_in_url,
    verify_url,
};
use crate::auth::{
    cookies::{EmailFlow, email_flow, is_fresh},
    current_user, identity, login_context, now, sign_in_as,
};
use crate::ui::{document, notice};

#[topcoat::router::query_params(error = bad_request)]
struct VerifyQuery {
    error: Option<String>,
    message: Option<String>,
}

/// The flow in progress, or `None` when there is none or it is stale.
fn current_flow(cx: &Cx) -> Option<(EmailFlow, ChallengePurpose)> {
    let flow = match email_flow(cx).parse() {
        Ok(Some(store)) => store.get(),
        Ok(None) | Err(_) => return None,
    };
    let purpose = ChallengePurpose::parse(&flow.purpose)?;
    is_fresh(flow.issued_at, now()).then_some((flow, purpose))
}

#[page]
pub async fn verify(cx: &Cx) -> Result<impl View> {
    let Some((flow, purpose)) = current_flow(cx) else {
        return Err(redirect(login_url(cx, Some("expired"))).into());
    };
    let query = query_params::<VerifyQuery>(cx)?;
    let (title, action) = match purpose {
        ChallengePurpose::Register => ("Confirm your e-mail address", "Create account"),
        ChallengePurpose::SignIn => ("Check your e-mail", "Sign in"),
        ChallengePurpose::ResetPassword => ("Choose a new password", "Change password"),
    };

    Ok(view! {
        document(
            title: title,
            <h1 class="mb-6 text-2xl font-semibold">(title)</h1>
            notice(error: query.error.as_deref(), message: query.message.as_deref())
            <p class="mb-4 text-sm text-neutral-600">
                "We sent a 6-digit code to " <strong>(&flow.email)</strong> ". It is valid for 10 minutes."
            </p>

            <form method="post" action=(href!(submit)) class="mb-4 space-y-3 rounded border border-neutral-200 bg-white p-4">
                <label class="block text-sm" for="code">"Code"</label>
                <input id="code" name="code" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2 tracking-widest"
                    type="text" inputmode="numeric" pattern="[0-9]{6}" maxlength="6" autocomplete="one-time-code" required="" autofocus="">
                if purpose == ChallengePurpose::ResetPassword {
                    <label class="block text-sm" for="password">"New password"</label>
                    <input id="password" name="password" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2"
                        type="password" autocomplete="new-password" minlength="8" maxlength="128" required="">
                    <label class="block text-sm" for="password-confirm">"Confirm the new password"</label>
                    <input id="password-confirm" name="password_confirm" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2"
                        type="password" autocomplete="new-password" minlength="8" maxlength="128" required="">
                }
                <button class="w-full rounded bg-neutral-900 px-4 py-2 font-medium text-white hover:bg-neutral-700">(action)</button>
            </form>

            <div class="flex items-center justify-between text-sm">
                <form method="post" action=(href!(super::resend::resend))>
                    <button class="underline">"Send a new code"</button>
                </form>
                if purpose == ChallengePurpose::Register {
                    <a class="underline" href=(href!(crate::app::auth::register::register))>"Use another address"</a>
                } else {
                    <a class="underline" href=(href!(crate::app::auth::login::login))>"Back to sign in"</a>
                }
            </div>
        )
    })
}

#[derive(Deserialize)]
struct VerifyForm {
    code: String,
    /// Password reset only.
    password: Option<String>,
    password_confirm: Option<String>,
}

/// What a completed flow signs the browser in as.
struct Completed {
    user_id: String,
    label: String,
    switched_from: Option<String>,
    reset: bool,
}

#[route(POST)]
pub async fn submit(cx: &Cx, Form(form): Form<VerifyForm>) -> Result<SeeOther> {
    let Some((flow, purpose)) = current_flow(cx) else {
        return Ok(see_other(login_url(cx, Some("expired"))));
    };
    let context = login_context(cx);
    let email = flow.email.clone();

    let outcome: std::result::Result<Completed, IdentityError> = match purpose {
        ChallengePurpose::Register => {
            let Some(password_hash) = flow.password_hash.clone() else {
                return Ok(see_other(register_url(cx, Some("expired"), Some(&email))));
            };
            identity(cx)
                .register_with_password(
                    RegisterWithPassword {
                        email: email.clone(),
                        code: form.code,
                        password_hash,
                        context,
                    },
                    now(),
                )
                .await
                .map(|done| Completed {
                    user_id: done.user_id,
                    label: done.label,
                    switched_from: None,
                    reset: false,
                })
        }
        ChallengePurpose::SignIn => {
            let Some(user_id) = flow.user_id.clone() else {
                return Ok(see_other(login_url(cx, Some("expired"))));
            };
            identity(cx)
                .sign_in_with_password(
                    SignInWithPassword {
                        email: email.clone(),
                        user_id,
                        code: form.code,
                        session_user_id: current_user(cx).await.map(|u| u.id.clone()),
                        context,
                    },
                    now(),
                )
                .await
                .map(|done| Completed {
                    user_id: done.user_id,
                    label: done.label,
                    switched_from: done.switched_from,
                    reset: false,
                })
        }
        ChallengePurpose::ResetPassword => {
            let password = form.password.unwrap_or_default();
            if form.password_confirm.unwrap_or_default() != password {
                return Ok(see_other(verify_url(cx, Some("password_mismatch"), None)));
            }
            let password_hash = match hash_password(&password).await {
                Ok(hash) => hash,
                Err(err) if err.is_user_facing() => {
                    return Ok(see_other(verify_url(cx, Some(error_code(&err)), None)));
                }
                Err(err) => return Err(err.into()),
            };
            identity(cx)
                .reset_password(
                    ResetPassword {
                        email: email.clone(),
                        code: form.code,
                        password_hash,
                        context,
                    },
                    now(),
                )
                .await
                .map(|done| Completed {
                    user_id: done.user_id,
                    label: done.label,
                    switched_from: None,
                    reset: true,
                })
        }
    };

    match outcome {
        Ok(done) => {
            email_flow(cx).remove();
            sign_in_as(cx, &done.user_id).await?;
            if done.reset {
                // After the new session exists, so the cutoff spares it.
                let current = session::token_hash(cx).await?;
                identity(cx)
                    .revoke_sessions(&done.user_id, &done.user_id, current.as_deref())
                    .await?;
                return Ok(see_other(home_url_with_message(
                    cx,
                    "Password changed. Other browsers were signed out.",
                )));
            }
            Ok(see_other(signed_in_url(
                cx,
                &done.label,
                done.switched_from.is_some(),
            )))
        }
        // The flow survives a typo; the code counts the attempt.
        Err(IdentityError::InvalidCode) => {
            Ok(see_other(verify_url(cx, Some("invalid_code"), None)))
        }
        Err(err) if err.is_user_facing() => {
            email_flow(cx).remove();
            let code = error_code(&err);
            Ok(see_other(match purpose {
                ChallengePurpose::Register => register_url(cx, Some(code), Some(&email)),
                ChallengePurpose::SignIn => login_url(cx, Some(code)),
                ChallengePurpose::ResetPassword => forgot_password_url(cx, Some(code)),
            }))
        }
        Err(err) => Err(err.into()),
    }
}
