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
    context::{Cx, app_context, try_app_context},
    router::{
        Body, Method, Next, StatusCode,
        content::Html,
        error::see_other,
        href, layer,
        request::{method, uri},
        response::{IntoResponse, Response},
    },
};

use crate::{
    auth::{CurrentAdmin, OWN_PASSWORD, Section, current_admin},
    config::{AdminConfig, Stylesheet},
};

/// What a refused operator is answered. A layer's error never reaches the
/// layout's error boundary — that is what makes a refused write safe: no
/// page ran — so the refusal is a page of its own, with the way back to
/// where the operator's role works.
fn refusal(cx: &Cx, config: &AdminConfig, home: &str) -> Result<Response> {
    // Outside a view an asset is resolved by hand; without a bundle the page
    // goes unstyled rather than not at all.
    let stylesheet = match &config.stylesheet {
        Stylesheet::Bundled => try_app_context::<topcoat::asset::AssetConfig>(cx)
            .map(|assets| assets.resolve(topcoat::tailwind::stylesheet!()))
            .unwrap_or_default(),
        Stylesheet::Url(url) => url.clone(),
    };
    let page = format!(
        "<!DOCTYPE html>\
         <html lang=\"fr\" class=\"h-full bg-background text-foreground\">\
         <head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>Accès refusé — Timada admin</title>\
         <link rel=\"stylesheet\" href=\"{stylesheet}\"></head>\
         <body class=\"min-h-full\"><main class=\"mx-auto max-w-6xl px-4 py-16 text-center\">\
         <h1 class=\"text-2xl font-semibold\">Accès refusé</h1>\
         <p class=\"mt-2 text-muted-foreground\">Votre rôle ne donne pas accès à cette page \
         ni à cette action. Le propriétaire de la boutique peut le changer.</p>\
         <p class=\"mt-6\"><a class=\"underline underline-offset-4\" href=\"{home}\">Retour à mon espace</a></p>\
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
        Err(err) => Err(anyhow::anyhow!("{err:#}").into()),
        Ok(Some(admin)) => {
            let config = app_context::<AdminConfig>(cx);
            let path = uri(cx).path().to_owned();
            let writes = !matches!(*method(cx), Method::GET | Method::HEAD);
            // A temporary password opens one page: the one that replaces it.
            if admin.must_change_password
                && path_under_mount(&path, &config.mount).trim_matches('/') != OWN_PASSWORD
            {
                return Err(see_other(href!(password::index).resolve(cx)).into());
            }
            if !admin
                .role
                .permits(path_under_mount(&path, &config.mount), writes)
            {
                tracing::warn!(
                    admin_id = %admin.id,
                    role = admin.role.as_str(),
                    %path,
                    "refused: not this role's"
                );
                return refusal(cx, config, &section_link(cx, admin.role.home()));
            }
            let cx = cx.with(CurrentAdmin(admin.clone()));
            next.run(&cx, body).await
        }
        Ok(None) => {
            let login = href!(super::login::index)
                .query([("next", uri(cx).path())])
                .resolve(cx);
            Err(see_other(login).into())
        }
    }
}
