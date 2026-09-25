//! `/{mount}`: the document shell, branded error pages, and the entry redirect.

#[allow(clippy::module_inception)]
pub mod _secure;
pub mod login;
pub mod rest;

use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{
        Slot, StatusCode,
        content::Form,
        error::{ForbiddenError, NotFoundError, UnauthorizedError, see_other},
        href, layout, page,
    },
    view::{View, error_boundary, view},
};

use crate::ui::{rail, shell, theme};

#[layout]
pub async fn admin_layout(slot: Slot<'_>) -> Result<impl View> {
    Ok(view! {
        shell(
            error_boundary(
                fallback: |error| {
                    let (status, title, detail) = if error.downcast_ref::<NotFoundError>().is_some() {
                        (StatusCode::NOT_FOUND, "Page introuvable", "Cette page n'existe pas.")
                    } else if error.downcast_ref::<ForbiddenError>().is_some()
                        || error.downcast_ref::<UnauthorizedError>().is_some()
                    {
                        (StatusCode::FORBIDDEN, "Accès refusé", "Vous n'avez pas accès à cette page.")
                    } else {
                        return Err(error);
                    };
                    Ok(view! {
                        (status)
                        <section class="py-16 text-center">
                            <h1 class="text-2xl font-semibold">(title)</h1>
                            <p class="mt-2 text-muted-foreground">(detail)</p>
                        </section>
                    })
                },
                (slot)
            )
        )
    })
}

/// `/{mount}/logout`: closes the session and returns to the login page.
#[page(POST "./logout")]
pub async fn logout(cx: &Cx) -> Result<impl View> {
    crate::auth::sign_out(cx).await?;
    Err::<(), _>(see_other(href!(login::index).resolve(cx)).into())
}

#[derive(Debug, Deserialize)]
pub struct SchemeForm {
    scheme: String,
    next: Option<String>,
}

/// `/{mount}/theme`: records the colour scheme and returns the operator to the
/// page the switch was pressed on.
///
/// Outside the auth layer, so an operator can pick their scheme on the login
/// page — which is where someone working at night meets the shop first.
#[page(POST "./theme")]
pub async fn choose_scheme(cx: &Cx, Form(form): Form<SchemeForm>) -> Result<impl View> {
    theme::remember(
        cx,
        theme::Scheme::of_value(&form.scheme).unwrap_or_default(),
    );
    // The same rule the login form follows: back inside this site, or home.
    let back = form
        .next
        .filter(|next| next.starts_with('/') && !next.starts_with("//"))
        .unwrap_or_else(|| href!(index).resolve(cx));
    Err::<(), _>(see_other(back).into())
}

#[derive(Debug, Deserialize)]
pub struct RailForm {
    rail: String,
    next: Option<String>,
}

/// `/{mount}/nav`: folds the navigation rail down to its icons, or unfolds it.
///
/// A round trip for a width, which is the price of having no JavaScript: the
/// labels have to stop being rendered, and that is a decision about markup, not
/// a class a selector can toggle.
#[page(POST "./nav")]
pub async fn fold_rail(cx: &Cx, Form(form): Form<RailForm>) -> Result<impl View> {
    rail::remember(cx, rail::Rail::of_value(&form.rail).unwrap_or_default());
    let back = form
        .next
        .filter(|next| next.starts_with('/') && !next.starts_with("//"))
        .unwrap_or_else(|| href!(index).resolve(cx));
    Err::<(), _>(see_other(back).into())
}

/// `/{mount}` → where the operator's role works; the orders list — hence the
/// login — for a visitor.
#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let section = match crate::auth::current_admin(cx).await {
        Ok(Some(admin)) => admin.role.home(),
        Ok(None) => crate::auth::Section::Orders,
        Err(err) => return Err(anyhow::anyhow!("{err:#}").into()),
    };
    Err::<(), _>(see_other(_secure::section_link(cx, section)).into())
}
