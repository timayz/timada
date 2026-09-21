//! `/{mount}`: the document shell, branded error pages, and the entry redirect.

#[allow(clippy::module_inception)]
pub mod _secure;
pub mod login;
pub mod rest;

use topcoat::{
    Result,
    context::Cx,
    router::{
        Slot, StatusCode,
        error::{ForbiddenError, NotFoundError, UnauthorizedError, see_other},
        href, layout, page,
    },
    view::{View, error_boundary, view},
};

use crate::ui::shell;

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
