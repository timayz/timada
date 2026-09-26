//! `/{mount}/password`: an operator's own password — every role's page. An
//! operator given a temporary password is sent here before anything else.

use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Form, error::see_other, page},
    view::{View, component, view},
};

use crate::{
    auth::{
        signed_in_admin,
        team::{MIN_PASSWORD_LEN, TeamError, change_own_password},
    },
    components::{
        button::button,
        card::{card, card_content, card_header, card_title},
        input::input,
        label::label,
    },
    config::AdminServices,
    ui::{form_error, page_header},
};

#[page]
pub async fn index() -> Result<impl View> {
    Ok(view! { password_view(error: None) })
}

#[derive(Debug, Deserialize)]
pub struct PasswordForm {
    current: String,
    new: String,
    confirm: String,
}

#[page(POST)]
pub async fn change(cx: &Cx, Form(form): Form<PasswordForm>) -> Result<impl View> {
    let services = app_context::<AdminServices>(cx);
    let Some(admin) = signed_in_admin(cx) else {
        return Err(topcoat::Error::msg(
            "the password page is behind the sign-in",
        ));
    };
    let error = if form.new != form.confirm {
        "Les deux mots de passe ne sont pas identiques.".to_owned()
    } else {
        // This session goes on; the operator's others are closed.
        let here = topcoat::session::token_hash(cx).await?;
        match change_own_password(
            &services.db,
            &admin.id,
            &form.current,
            &form.new,
            here.as_ref(),
        )
        .await
        {
            Ok(()) => {
                let home = super::section_link(cx, admin.role.home());
                return Err(see_other(home).into());
            }
            Err(TeamError::Server(err)) => return Err(topcoat::Error::from_anyhow(err)),
            Err(refused) => refused.to_string(),
        }
    };
    Ok(view! { password_view(error: Some(error)) })
}

#[component]
async fn password_view(cx: &Cx, error: Option<String>) -> Result<impl View> {
    let temporary = signed_in_admin(cx).is_some_and(|admin| admin.must_change_password);
    let hint = format!("{MIN_PASSWORD_LEN} caractères au moins.");
    Ok(view! {
        page_header(title: "Mon mot de passe")
        <div class="max-w-md">
            card(
                card_header(card_title("Changer de mot de passe"))
                card_content(
                    if temporary {
                        <p class="mb-4 text-sm text-muted-foreground">"Votre mot de passe est temporaire : choisissez le vôtre pour continuer."</p>
                    }
                    <form method="post" class="flex flex-col gap-4">
                        <div class="flex flex-col gap-1.5">
                            label(attrs: topcoat::view::attributes! { for="current" }, "Mot de passe actuel")
                            input(attrs: topcoat::view::attributes! { id="current" name="current" type="password" required=(true) autocomplete="current-password" })
                        </div>
                        <div class="flex flex-col gap-1.5">
                            label(attrs: topcoat::view::attributes! { for="new" }, "Nouveau mot de passe")
                            input(attrs: topcoat::view::attributes! { id="new" name="new" type="password" required=(true) autocomplete="new-password" aria-describedby="new-hint" })
                            <p id="new-hint" class="text-xs text-muted-foreground">(hint)</p>
                        </div>
                        <div class="flex flex-col gap-1.5">
                            label(attrs: topcoat::view::attributes! { for="confirm" }, "Confirmez le nouveau mot de passe")
                            input(attrs: topcoat::view::attributes! { id="confirm" name="confirm" type="password" required=(true) autocomplete="new-password" })
                        </div>
                        if let Some(error) = &error {
                            form_error((error.clone()))
                        }
                        <div>button(attrs: topcoat::view::attributes! { type="submit" }, "Enregistrer")</div>
                    </form>
                )
            )
        </div>
    })
}
