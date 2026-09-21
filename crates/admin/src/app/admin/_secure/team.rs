//! `/{mount}/team`: who operates the shop, as what. The owners' page — no
//! section of [`Role`] claims it — where an operator is added with a
//! temporary password; an operator's own page changes their role, resets
//! their password, or ends their access.

pub mod operator_id;

use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Form, error::see_other, href, page},
    view::{View, component, view},
};

use crate::{
    auth::{
        Role,
        team::{MIN_PASSWORD_LEN, TeamError, add_operator, list_operators},
    },
    components::{
        button::button,
        card::{card, card_content, card_header, card_title},
        input::input,
        label::label,
        table::{table, table_body, table_cell, table_head, table_header, table_row},
    },
    config::AdminServices,
    ui::{date, page_header},
};

/// A `<select name="role">` over the roles, `selected` chosen.
#[component]
pub async fn role_select(selected: Role) -> Result<impl View> {
    Ok(view! {
        <select id="role" name="role" class="h-9 w-full rounded-md border border-input bg-background px-3 text-sm shadow-xs">
            for role in Role::ALL {
                <option value=(role.as_str()) selected=(role == selected)>(role.label())</option>
            }
        </select>
    })
}

struct Line {
    link: String,
    email: String,
    role: &'static str,
    standing: &'static str,
    since: String,
}

#[page]
pub async fn index() -> Result<impl View> {
    Ok(view! { team_view(error: None) })
}

#[derive(Debug, Deserialize)]
pub struct NewOperatorForm {
    email: String,
    role: String,
    temporary_password: String,
}

#[page(POST "./new")]
pub async fn create(cx: &Cx, Form(form): Form<NewOperatorForm>) -> Result<impl View> {
    let services = app_context::<AdminServices>(cx);
    let error = match Role::parse(&form.role) {
        None => "Choisissez un rôle.".to_owned(),
        Some(role) => {
            match add_operator(&services.db, &form.email, &form.temporary_password, role).await {
                Ok(id) => {
                    let theirs = href!(operator_id::show, operator_id::OperatorId(id)).resolve(cx);
                    return Err(see_other(theirs).into());
                }
                Err(TeamError::Server(err)) => return Err(err.into()),
                Err(refused) => refused.to_string(),
            }
        }
    };
    Ok(view! { team_view(error: Some(error)) })
}

#[component]
async fn team_view(cx: &Cx, error: Option<String>) -> Result<impl View> {
    let db = &app_context::<AdminServices>(cx).db;
    let operators = match list_operators(db).await {
        Ok(operators) => operators,
        Err(err) => return Err(anyhow::Error::from(err).into()),
    };
    let lines: Vec<Line> = operators
        .into_iter()
        .map(|operator| Line {
            link: href!(
                operator_id::show,
                operator_id::OperatorId(operator.id.clone())
            )
            .resolve(cx),
            email: operator.email,
            role: operator.role.label(),
            standing: match (operator.active, operator.must_change_password) {
                (false, _) => "Accès retiré",
                (true, true) => "Mot de passe temporaire",
                (true, false) => "Actif",
            },
            since: date(operator.created_at.max(0) as u64),
        })
        .collect();
    let hint = format!(
        "{MIN_PASSWORD_LEN} caractères au moins. Transmettez-le à l'opérateur : il choisira le sien à sa première connexion."
    );

    Ok(view! {
        page_header(title: "Équipe")
        <div class="grid gap-6 lg:grid-cols-3">
            <div class="lg:col-span-2">
                table(
                    table_header(table_row(
                        table_head("Opérateur") table_head("Rôle") table_head("Accès") table_head("Depuis le")
                    ))
                    table_body(
                        for line in &lines {
                            table_row(
                                table_cell(<a href=(line.link.clone()) class="underline-offset-4 hover:underline">(line.email.clone())</a>)
                                table_cell((line.role))
                                table_cell(<span class="text-muted-foreground">(line.standing)</span>)
                                table_cell((line.since.clone()))
                            )
                        }
                    )
                )
            </div>
            card(
                card_header(card_title("Ajouter un opérateur"))
                card_content(
                    <form method="post" action=(href!(create).resolve(cx)) class="flex flex-col gap-4">
                        <div class="flex flex-col gap-1.5">
                            label(attrs: topcoat::view::attributes! { for="email" }, "Adresse e-mail")
                            input(attrs: topcoat::view::attributes! { id="email" name="email" type="email" required=(true) autocomplete="off" })
                        </div>
                        <div class="flex flex-col gap-1.5">
                            label(attrs: topcoat::view::attributes! { for="role" }, "Rôle")
                            role_select(selected: Role::Support)
                        </div>
                        <div class="flex flex-col gap-1.5">
                            label(attrs: topcoat::view::attributes! { for="temporary_password" }, "Mot de passe temporaire")
                            input(attrs: topcoat::view::attributes! { id="temporary_password" name="temporary_password" type="text" required=(true) autocomplete="off" aria-describedby="temporary-hint" })
                            <p id="temporary-hint" class="text-xs text-muted-foreground">(hint)</p>
                        </div>
                        if let Some(error) = &error {
                            <p role="alert" class="text-sm text-destructive">(error.clone())</p>
                        }
                        <div>button(attrs: topcoat::view::attributes! { type="submit" }, "Ajouter")</div>
                    </form>
                )
            )
        </div>
    })
}
