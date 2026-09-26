//! Everything under this group requires a signed-in admin whose role permits
//! the request. The group adds no URL segment; the layer wraps every page
//! below it, so a page added later is closed to everybody but the owner
//! until [`Role::permits`](crate::auth::Role::permits) names it.

pub mod categories;
pub mod customers;
pub mod disputes;
pub mod emails;
pub mod families;
pub mod inventory;
pub mod invoices;
pub mod journal;
pub mod orders;
pub mod password;
pub mod products;
pub mod promotions;
pub mod questions;
pub mod refunds;
pub mod returns;
pub mod reviews;
pub mod team;
pub mod vat;

use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        Body, Method, Next, StatusCode,
        content::Html,
        error::{BadRequestError, NotFoundError, SeeOther, see_other},
        href, layer,
        request::{method, uri},
        response::{IntoResponse, Response},
    },
};

use crate::{
    auth::{
        CurrentAdmin, OWN_PASSWORD, Section, current_admin,
        journal::{self as records, Outcome},
    },
    config::{AdminConfig, AdminServices},
    ui::{stylesheet_url, theme},
};

/// How a request that reached its page went. A redirect — how a page says
/// "done" — and the refusals a page words itself travel as errors too: only
/// what is none of them is a failure.
fn how_it_went(answered: &Result<Response>) -> (Outcome, u16) {
    match answered {
        Ok(response) if response.status().is_server_error() => {
            (Outcome::Error, response.status().as_u16())
        }
        Ok(response) => (Outcome::Passed, response.status().as_u16()),
        Err(err) if err.downcast_ref::<SeeOther>().is_some() => (Outcome::Passed, 303),
        Err(err) if err.downcast_ref::<NotFoundError>().is_some() => (Outcome::Passed, 404),
        Err(err) if err.downcast_ref::<BadRequestError>().is_some() => (Outcome::Passed, 400),
        Err(_) => (Outcome::Error, 500),
    }
}

/// What a refused operator is answered. A layer's error never reaches the
/// layout's error boundary — that is what makes a refused write safe: no
/// page ran — so the refusal is a page of its own, with the way back to
/// where the operator's role works.
///
/// Written out rather than built from [`crate::ui::document`]: `view!` expands
/// against bindings that `#[component]` and `#[page]` introduce, so it cannot
/// be used in a plain function, and a component cannot be invoked by hand
/// either — its child is an inert scope only the macro can build. What the
/// shell and this page must agree on is therefore shared as values instead: the
/// stylesheet comes from the same lookup, and the scheme the operator chose is
/// carried on `<html>` and declared in the head. Otherwise a refusal would be
/// the one page in the back office that flashes white at somebody working in
/// the dark.
fn refusal(cx: &Cx, home: &str) -> Result<Response> {
    let stylesheet = stylesheet_url(cx);
    let scheme = theme::chosen(cx);
    let chosen = scheme
        .html_class()
        .map(|class| format!(" {class}"))
        .unwrap_or_default();
    let declared = scheme.color_scheme();
    let page = format!(
        "<!DOCTYPE html>\
         <html lang=\"fr\" class=\"h-full bg-background text-foreground{chosen}\">\
         <head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <meta name=\"color-scheme\" content=\"{declared}\">\
         <title>Accès refusé — Timada admin</title>\
         <link rel=\"stylesheet\" href=\"{stylesheet}\"></head>\
         <body class=\"min-h-full\"><main class=\"mx-auto max-w-2xl px-4 py-16 text-center\">\
         <h1 class=\"text-2xl font-semibold\">Accès refusé</h1>\
         <p class=\"mt-2 text-muted-foreground\">Votre rôle ne donne pas accès à cette page \
         ni à cette action. Le propriétaire de la boutique peut le changer.</p>\
         <p class=\"mt-6\"><a class=\"text-primary underline underline-offset-4\" href=\"{home}\">Retour à mon espace</a></p>\
         </main></body></html>"
    );
    (StatusCode::FORBIDDEN, Html(page)).into_response(cx)
}

/// The page a section opens on.
pub fn section_link(cx: &Cx, section: Section) -> String {
    match section {
        Section::Orders => href!(orders::index).resolve(cx),
        Section::Products => href!(products::index).resolve(cx),
        Section::Categories => href!(categories::index).resolve(cx),
        Section::Families => href!(families::index).resolve(cx),
        Section::Inventory => href!(inventory::index).resolve(cx),
        Section::Customers => href!(customers::index).resolve(cx),
        Section::Promotions => href!(promotions::index).resolve(cx),
        Section::Invoices => href!(invoices::index).resolve(cx),
        Section::Returns => href!(returns::index).resolve(cx),
        Section::Refunds => href!(refunds::index).resolve(cx),
        Section::Disputes => href!(disputes::index).resolve(cx),
        Section::Vat => href!(vat::index).resolve(cx),
        Section::Reviews => href!(reviews::index).resolve(cx),
        Section::Questions => href!(questions::index).resolve(cx),
        Section::Emails => href!(emails::index).resolve(cx),
    }
}

/// What follows the mount segment: `/admin/orders/o-1/refund` → `orders/o-1/refund`.
fn path_under_mount<'a>(path: &'a str, mount: &str) -> &'a str {
    let path = path.trim_start_matches('/');
    path.strip_prefix(mount.trim_matches('/'))
        .unwrap_or(path)
        .trim_start_matches('/')
}

#[layer]
async fn require_admin(cx: &Cx, body: Body, next: Next<'_>) -> Result<Response> {
    match current_admin(cx).await {
        Err(err) => Err(topcoat::Error::msg(format!("{err:#}"))),
        Ok(Some(admin)) => {
            let config = app_context::<AdminConfig>(cx);
            let db = &app_context::<AdminServices>(cx).db;
            let path = uri(cx).path().to_owned();
            let under = path_under_mount(&path, &config.mount).to_owned();
            let verb = method(cx).as_str().to_owned();
            let writes = !matches!(*method(cx), Method::GET | Method::HEAD);
            // A temporary password opens one page: the one that replaces it.
            if admin.must_change_password && under.trim_matches('/') != OWN_PASSWORD {
                if writes {
                    let refused = Outcome::Refused;
                    records::record(db, Some(admin), &admin.email, &verb, &under, refused, 303)
                        .await;
                }
                return Err(see_other(href!(password::index).resolve(cx)).into());
            }
            if !admin.role.permits(&under, writes) {
                // Every refusal is written down, a read too: somebody tried.
                let refused = Outcome::Refused;
                records::record(db, Some(admin), &admin.email, &verb, &under, refused, 403).await;
                tracing::warn!(
                    admin_id = %admin.id,
                    role = admin.role.as_str(),
                    %path,
                    "refused: not this role's"
                );
                return refusal(cx, &section_link(cx, admin.role.home()));
            }
            let cx = cx.with(CurrentAdmin(admin.clone()));
            let answered = next.run(&cx, body).await;
            // What writes is written down, with how it went.
            if writes {
                let (outcome, status) = how_it_went(&answered);
                records::record(
                    db,
                    Some(admin),
                    &admin.email,
                    &verb,
                    &under,
                    outcome,
                    status,
                )
                .await;
            }
            answered
        }
        Ok(None) => {
            let login = href!(super::login::index)
                .query([("next", uri(cx).path())])
                .resolve(cx);
            Err(see_other(login).into())
        }
    }
}
