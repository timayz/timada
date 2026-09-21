//! `/{mount}/login` and `/{mount}/logout`.

use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::see_other, page, query_params, query_params as query},
    view::{View, view},
};

use crate::{
    auth::sign_in,
    components::{
        button::button,
        card::{card, card_content, card_header, card_title},
        input::input,
        label::label,
    },
};

#[query_params(error = bad_request)]
struct LoginQuery {
    next: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LoginForm {
    email: String,
    password: String,
    next: Option<String>,
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let next = query::<LoginQuery>(cx)?.next.clone();
    Ok(view! { login_form(next: next, error: None) })
}

#[page(POST)]
pub async fn submit(cx: &Cx, Form(form): Form<LoginForm>) -> Result<impl View> {
    if let Some(admin) = sign_in(cx, &form.email, &form.password).await? {
        // Back where they were going, or to where their role works.
        let target = form
            .next
            .filter(|n| n.starts_with('/') && !n.starts_with("//"))
            .unwrap_or_else(|| super::_secure::section_link(cx, admin.role.home()));
        return Err(see_other(target).into());
    }
    Ok(view! { login_form(next: form.next, error: Some("Email ou mot de passe incorrect.")) })
}

#[topcoat::view::component]
async fn login_form(next: Option<String>, error: Option<&str>) -> Result<impl View> {
    Ok(view! {
        <div class="mx-auto mt-16 max-w-sm">
            card(
                card_header(card_title("Connexion"))
                card_content(
                    <form method="post" class="flex flex-col gap-4">
                        if let Some(next) = &next { <input type="hidden" name="next" value=(next)> }
                        <div class="flex flex-col gap-1.5">
                            label(attrs: topcoat::view::attributes! { for="email" }, "Email")
                            input(attrs: topcoat::view::attributes! { id="email" name="email" type="email" required=(true) autocomplete="username" })
                        </div>
                        <div class="flex flex-col gap-1.5">
                            label(attrs: topcoat::view::attributes! { for="password" }, "Mot de passe")
                            input(attrs: topcoat::view::attributes! { id="password" name="password" type="password" required=(true) autocomplete="current-password" })
                        </div>
                        if let Some(error) = error {
                            <p role="alert" class="text-sm text-destructive">(error)</p>
                        }
                        button(attrs: topcoat::view::attributes! { type="submit" }, "Se connecter")
                    </form>
                )
            )
        </div>
    })
}
