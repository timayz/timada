//! `/{mount}/team/{operator_id}`: one operator — their role, a new temporary
//! password, the end (or the return) of their access. Somebody else's, never
//! one's own: an owner does not demote or lock out themselves.

use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::Form,
        error::{RouterErrorExt, see_other},
        href, page, path_param, path_param as param,
    },
    view::{View, component, view},
};

use super::role_select;
use crate::{
    auth::{
        Role, signed_in_admin,
        team::{
            MIN_PASSWORD_LEN, TeamError, change_role, deactivate, list_operators, reactivate,
            reset_password,
        },
    },
    components::{
        button::{ButtonVariant, button},
        card::{card, card_content, card_header, card_title},
        input::input,
        label::label,
    },
    config::AdminServices,
    ui::page_header,
};

path_param!(pub operator_id: String, error = not_found);

#[page]
pub async fn show() -> Result<impl View> {
    Ok(view! { operator_view(error: None) })
}

/// Who is asking: the signed-in operator's id.
fn acting(cx: &Cx) -> Result<String> {
    signed_in_admin(cx)
        .map(|admin| admin.id.clone())
        .ok_or_else(|| anyhow::anyhow!("the team pages are behind the sign-in").into())
}

/// Back to the operator's page when done; the refusal, in words, otherwise.
fn outcome(cx: &Cx, id: String, done: std::result::Result<(), TeamError>) -> Result<String> {
    match done {
        Ok(()) => Err(see_other(href!(show, OperatorId(id)).resolve(cx)).into()),
        Err(TeamError::NotFound) => Err(topcoat::router::error::not_found().into()),
        Err(TeamError::Server(err)) => Err(err.into()),
        Err(refused) => Ok(refused.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct RoleForm {
    role: String,
}

#[page(POST "./role")]
pub async fn set_role(cx: &Cx, Form(form): Form<RoleForm>) -> Result<impl View> {
    let id = param::<OperatorId>(cx)?.clone();
    let db = &app_context::<AdminServices>(cx).db;
    let error = match Role::parse(&form.role) {
        None => "Choisissez un rôle.".to_owned(),
        Some(role) => {
            let done = change_role(db, &acting(cx)?, &id, role).await;
            outcome(cx, id, done)?
        }
    };
    Ok(view! { operator_view(error: Some(error)) })
}

#[derive(Debug, Deserialize)]
pub struct ResetForm {
    temporary_password: String,
}

#[page(POST "./reset-password")]
pub async fn reset(cx: &Cx, Form(form): Form<ResetForm>) -> Result<impl View> {
    let id = param::<OperatorId>(cx)?.clone();
    let db = &app_context::<AdminServices>(cx).db;
    let done = reset_password(db, &acting(cx)?, &id, &form.temporary_password).await;
    let error = outcome(cx, id, done)?;
    Ok(view! { operator_view(error: Some(error)) })
}

#[page(POST "./deactivate")]
pub async fn end_access(cx: &Cx) -> Result<impl View> {
    let id = param::<OperatorId>(cx)?.clone();
    let db = &app_context::<AdminServices>(cx).db;
    let done = deactivate(db, &acting(cx)?, &id).await;
    let error = outcome(cx, id, done)?;
    Ok(view! { operator_view(error: Some(error)) })
}

#[page(POST "./reactivate")]
pub async fn restore_access(cx: &Cx) -> Result<impl View> {
    let id = param::<OperatorId>(cx)?.clone();
    let db = &app_context::<AdminServices>(cx).db;
    let done = reactivate(db, &acting(cx)?, &id).await;
    let error = outcome(cx, id, done)?;
    Ok(view! { operator_view(error: Some(error)) })
}

#[component]
async fn operator_view(cx: &Cx, error: Option<String>) -> Result<impl View> {
    let id = param::<OperatorId>(cx)?.clone();
    let db = &app_context::<AdminServices>(cx).db;
    let operators = match list_operators(db).await {
        Ok(operators) => operators,
        Err(err) => return Err(anyhow::Error::from(err).into()),
    };
    let operator = operators
        .into_iter()
        .find(|operator| operator.id == id)
        .ok_or_not_found()?;
    let yourself = acting(cx)? == operator.id;
    let standing = match (operator.active, operator.must_change_password) {
        (false, _) => "Accès retiré : cet opérateur ne peut plus se connecter.",
        (true, true) => "Mot de passe temporaire : il choisira le sien à sa prochaine connexion.",
        (true, false) => "Actif.",
    };
    let hint = format!("{MIN_PASSWORD_LEN} caractères au moins, à lui transmettre.");
    let back = href!(super::index).resolve(cx);

    Ok(view! {
        page_header(title: &operator.email)
        <p class="-mt-4 mb-6 text-sm text-muted-foreground">(operator.role.label()) " · " (standing)</p>
        if let Some(error) = &error {
            <p role="alert" class="mb-6 text-sm text-destructive">(error.clone())</p>
        }
        if yourself {
            <p class="text-sm text-muted-foreground">"C'est vous. Votre rôle et votre accès se changent par un autre propriétaire ; votre mot de passe, depuis « Mon mot de passe »."</p>
        } else {
            <div class="grid gap-6 lg:grid-cols-3">
                card(
                    card_header(card_title("Rôle"))
                    card_content(
                        <form method="post" action=(href!(set_role, OperatorId(id.clone())).resolve(cx)) class="flex flex-col gap-4">
                            <div class="flex flex-col gap-1.5">
                                label(attrs: topcoat::view::attributes! { for="role" }, "Rôle")
                                role_select(selected: operator.role)
                            </div>
                            <div>button(attrs: topcoat::view::attributes! { type="submit" }, "Changer le rôle")</div>
                        </form>
                    )
                )
                card(
                    card_header(card_title("Mot de passe oublié"))
                    card_content(
                        <form method="post" action=(href!(reset, OperatorId(id.clone())).resolve(cx)) class="flex flex-col gap-4">
                            <div class="flex flex-col gap-1.5">
                                label(attrs: topcoat::view::attributes! { for="temporary_password" }, "Nouveau mot de passe temporaire")
                                input(attrs: topcoat::view::attributes! { id="temporary_password" name="temporary_password" type="text" required=(true) autocomplete="off" aria-describedby="temporary-hint" })
                                <p id="temporary-hint" class="text-xs text-muted-foreground">(hint)</p>
                            </div>
                            <div>button(variant: ButtonVariant::Outline, attrs: topcoat::view::attributes! { type="submit" }, "Réinitialiser")</div>
                        </form>
                    )
                )
                card(
                    card_header(card_title("Accès"))
                    card_content(
                        if operator.active {
                            <form method="post" action=(href!(end_access, OperatorId(id.clone())).resolve(cx)) class="flex flex-col gap-4">
                                <p class="text-sm text-muted-foreground">"L'opérateur est déconnecté partout, aussitôt. Rien n'est effacé."</p>
                                <div>button(variant: ButtonVariant::Outline, attrs: topcoat::view::attributes! { type="submit" }, "Retirer l'accès")</div>
                            </form>
                        } else {
                            <form method="post" action=(href!(restore_access, OperatorId(id.clone())).resolve(cx)) class="flex flex-col gap-4">
                                <p class="text-sm text-muted-foreground">"L'opérateur retrouve son rôle et son mot de passe."</p>
                                <div>button(attrs: topcoat::view::attributes! { type="submit" }, "Rendre l'accès")</div>
                            </form>
                        }
                    )
                )
            </div>
        }
        <p class="mt-6 text-sm"><a href=(back) class="underline-offset-4 hover:underline">"Retour à l'équipe"</a></p>
    })
}
