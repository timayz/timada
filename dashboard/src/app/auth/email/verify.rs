use serde::Deserialize;
use timada_core::Sending;
use timada_identity::{
    AddEmail, IdentityError, RegisterWithPassword, ResetPassword, SignInWithPassword, hash_password,
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
    view::{View, attributes, class, view},
};

use crate::app::auth::{
    account_url, error_code, forgot_password_url, home_url_with_message, login_url, register_url,
    signed_in_url, verify_url,
};
use crate::auth::{
    cookies::{EmailFlow, EmailFlowKind, email_flow, is_fresh},
    current_user, identity, login_context, now, sign_in_as,
};
use crate::components::button::button;
use crate::components::card::CARD;
use crate::components::field::{field, field_label};
use crate::components::input::input;
use crate::ui::{LINK, auth_document, notice};

#[topcoat::router::query_params(error = bad_request)]
struct VerifyQuery {
    error: Option<String>,
    message: Option<String>,
}

/// The flow in progress, or `None` when there is none or it is stale.
fn current_flow(cx: &Cx) -> Option<EmailFlow> {
    let flow = match email_flow(cx).parse() {
        Ok(Some(store)) => store.get(),
        Ok(None) | Err(_) => return None,
    };
    is_fresh(flow.issued_at, now()).then_some(flow)
}

#[page]
pub async fn verify(cx: &Cx) -> Result<impl View> {
    let Some(flow) = current_flow(cx) else {
        return Err(redirect(login_url(cx, Some("expired"))).into());
    };
    let query = query_params::<VerifyQuery>(cx)?;
    let kind = flow.kind;
    // Where the latest code's mail stands: the subscription that sends it
    // records it on the challenge, so the page can say when the address
    // refused it. Not for a password reset: the first step said "if an
    // account exists", and an address nobody registered has no challenge,
    // so saying "sending" or "couldn't deliver" would tell a stranger which
    // addresses have an account. That flow reads "sent" whatever happened.
    let mail = if kind == EmailFlowKind::ResetPassword {
        None
    } else {
        identity(cx)
            .load_email_challenge(&flow.email)
            .await
            .map_err(topcoat::Error::from_anyhow)?
            .map(|challenge| challenge.mail)
    };
    let (state, lead, tail) = match mail {
        Some(Sending::Undeliverable { .. }) => (
            "undeliverable",
            "We couldn't deliver a code to ",
            ". Check the address and ask for a new code.",
        ),
        Some(Sending::Pending | Sending::Claimed { .. } | Sending::Deferred { .. }) => (
            "sending",
            "We are sending a 6-digit code to ",
            ". It is valid for 10 minutes.",
        ),
        Some(Sending::Failed { .. }) => (
            "failed",
            "We couldn't send a code to ",
            ". Our mail server did not answer; ask for a new code in a moment.",
        ),
        Some(Sending::Sent) | None => (
            "sent",
            "We sent a 6-digit code to ",
            ". It is valid for 10 minutes.",
        ),
    };
    let (title, action) = match kind {
        EmailFlowKind::Register => ("Confirm your e-mail address", "Create account"),
        EmailFlowKind::SignIn => ("Check your e-mail", "Sign in"),
        EmailFlowKind::ResetPassword => ("Choose a new password", "Change password"),
        EmailFlowKind::AddEmail => ("Confirm your e-mail address", "Add e-mail address"),
    };

    Ok(view! {
        auth_document(
            title: title,
            <div class=(class!(CARD, "gap-5 px-6"))>
                <div class="text-center">
                    <h1 class="text-2xl font-bold tracking-tight">(title)</h1>
                    <p class="mt-1 text-sm text-muted-foreground" data-mail-state=(state)>
                        (lead) <strong class="text-foreground">(&flow.email)</strong> (tail)
                    </p>
                </div>
                notice(error: query.error.as_deref(), message: query.message.as_deref())

                <form method="post" action=(href!(submit)) class="flex flex-col gap-4">
                    field(
                        field_label(attrs: attributes! { for="code" }, "Code")
                        input(attrs: attributes! {
                            id="code" name="code" type="text" inputmode="numeric" pattern="[0-9]{6}" maxlength="6"
                            autocomplete="one-time-code" required="" autofocus="" class="text-center text-lg tracking-[0.5em]"
                        })
                    )
                    if kind == EmailFlowKind::ResetPassword {
                        field(
                            field_label(attrs: attributes! { for="password" }, "New password")
                            input(attrs: attributes! {
                                id="password" name="password" type="password" autocomplete="new-password" minlength="8" maxlength="128" required=""
                            })
                        )
                        field(
                            field_label(attrs: attributes! { for="password-confirm" }, "Confirm the new password")
                            input(attrs: attributes! {
                                id="password-confirm" name="password_confirm" type="password" autocomplete="new-password" minlength="8" maxlength="128" required=""
                            })
                        )
                    }
                    button(attrs: attributes! { class="w-full" }, (action))
                </form>

                <div class="flex items-center justify-between text-sm">
                    <form method="post" action=(href!(super::resend::resend))>
                        <button class=(LINK)>"Send a new code"</button>
                    </form>
                    if kind == EmailFlowKind::Register {
                        <a class=(LINK) href=(href!(crate::app::auth::register::register))>"Use another address"</a>
                    } else if kind == EmailFlowKind::AddEmail {
                        <a class=(LINK) href=(href!(crate::app::auth::account::account))>"Back to your account"</a>
                    } else {
                        <a class=(LINK) href=(href!(crate::app::auth::login::login))>"Back to sign in"</a>
                    }
                </div>
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
    let Some(flow) = current_flow(cx) else {
        return Ok(see_other(login_url(cx, Some("expired"))));
    };
    let kind = flow.kind;
    let context = login_context(cx);
    let email = flow.email.clone();

    if kind == EmailFlowKind::AddEmail {
        return add_email(cx, flow, form.code).await;
    }

    let outcome: std::result::Result<Completed, IdentityError> = match kind {
        EmailFlowKind::Register => {
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
        EmailFlowKind::SignIn => {
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
        EmailFlowKind::ResetPassword => {
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
        EmailFlowKind::AddEmail => unreachable!("handled above"),
    };

    match outcome {
        Ok(done) => {
            email_flow(cx).remove();
            sign_in_as(cx, &done.user_id).await?;
            if done.reset {
                // The reset itself ended every session of the user. The new
                // one is spared by name, in case it started within the
                // reset's millisecond; should that fail, the browser signs
                // in again.
                let current = session::token_hash(cx).await?;
                if let Err(err) = identity(cx)
                    .revoke_sessions(&done.user_id, &done.user_id, current.as_deref())
                    .await
                {
                    tracing::warn!(%err, "cannot spare the session a password reset started");
                }
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
            Ok(see_other(match kind {
                EmailFlowKind::Register => register_url(cx, Some(code), Some(&email)),
                EmailFlowKind::SignIn => login_url(cx, Some(code)),
                EmailFlowKind::ResetPassword => forgot_password_url(cx, Some(code)),
                EmailFlowKind::AddEmail => unreachable!("handled above"),
            }))
        }
        Err(err) => Err(err.into()),
    }
}

/// The code is back for an address added from the account page. The browser
/// stays signed in as it was; the flow only adds a way to sign in.
async fn add_email(cx: &Cx, flow: EmailFlow, code: String) -> Result<SeeOther> {
    let (Some(bound_user_id), Some(password_hash)) = (flow.user_id, flow.password_hash) else {
        email_flow(cx).remove();
        return Ok(see_other(account_url(cx, Some("expired"), None)));
    };
    let outcome = identity(cx)
        .add_email(
            AddEmail {
                session_user_id: current_user(cx).await.map(|u| u.id.clone()),
                bound_user_id,
                email: flow.email,
                code,
                password_hash,
            },
            now(),
        )
        .await;
    match outcome {
        Ok(()) => {
            email_flow(cx).remove();
            Ok(see_other(account_url(
                cx,
                None,
                Some("E-mail address added. You can sign in with it and your password."),
            )))
        }
        Err(IdentityError::InvalidCode) => {
            Ok(see_other(verify_url(cx, Some("invalid_code"), None)))
        }
        Err(err) if err.is_user_facing() => {
            email_flow(cx).remove();
            Ok(see_other(account_url(cx, Some(error_code(&err)), None)))
        }
        Err(err) => Err(err.into()),
    }
}
