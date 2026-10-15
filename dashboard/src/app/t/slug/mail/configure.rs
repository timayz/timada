use serde::Deserialize;
use timada_tenancy::{ConfigureMailServer, MAIL_SECURITIES, PERMISSION_MAIL_MANAGE};
use topcoat::{
    Result,
    context::{Cx, app_context},
    mail::{Mailbox, Transport, mail},
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::mail_url;
use crate::app::t::slug::require_tenant;
use crate::app::tenants::{error_code, tenancy};
use crate::shop_mail::{MailServer, ShopMail};

/// Every field a string: a urlencoded form hands them so, and a bad port is
/// the page's message rather than a 400.
#[derive(Deserialize)]
pub struct ConfigureForm {
    host: String,
    port: String,
    security: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
    from_address: String,
    #[serde(default)]
    from_name: String,
    test_to: String,
}

/// Saves the mail server once a test mail went out through it.
#[route(POST)]
pub async fn configure(cx: &Cx, Form(form): Form<ConfigureForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_MAIL_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let fail = |code: &str| Ok(see_other(mail_url(cx, slug, Some(code), None)));
    let shop_mail = app_context::<ShopMail>(cx);
    if !shop_mail.enabled() {
        return fail("mail_servers_disabled");
    }
    let host = form.host.trim().to_owned();
    let username = Some(form.username.trim().to_owned()).filter(|name| !name.is_empty());
    let from_name = Some(form.from_name.trim().to_owned()).filter(|name| !name.is_empty());
    let Ok(port) = form.port.trim().parse::<u16>() else {
        return fail("invalid_mail_server");
    };
    if host.is_empty() || port == 0 || !MAIL_SECURITIES.contains(&form.security.as_str()) {
        return fail("invalid_mail_server");
    }
    let (Ok(from), Ok(test_to)) = (
        Mailbox::named(
            from_name.as_deref().unwrap_or(&access.tenant.name),
            form.from_address.trim(),
        ),
        Mailbox::new(form.test_to.trim()),
    ) else {
        return fail("invalid_email");
    };
    // A blank password keeps the saved one, as long as the user name stays.
    let saved = access.tenant.mail_server.as_ref();
    let password = if !form.password.is_empty() {
        Some(form.password.clone())
    } else if username.is_some() && saved.is_some_and(|saved| saved.username == username) {
        shop_mail
            .server_of(&access.tenant)
            .map_err(topcoat::Error::from_anyhow)?
            .and_then(|server| server.password)
    } else {
        None
    };
    if let Err(err) = shop_mail.check_host(&host, port).await {
        tracing::info!(tenant = slug, %err, "a shop mail server was refused");
        return fail("mail_server_not_public");
    }
    let server = MailServer {
        host: host.clone(),
        port,
        security: form.security.clone(),
        username: username.clone(),
        password: password.clone(),
    };
    let transport = server.transport().map_err(topcoat::Error::from_anyhow)?;
    // The body owns what it shows.
    let shown_host = host.clone();
    let shop_name = access.tenant.name.clone();
    let test = mail! {
        from: from,
        to: test_to,
        subject: "Your shop's mail server works",
        html: {
            cx =>
            <p>"This test mail went out through " <strong>(&shown_host)</strong> "."</p>
            <p>"From now on, the mails " <strong>(&shop_name)</strong>
                " sends its shoppers (order confirmations, review requests, sign-in codes) go out through it."</p>
        },
    }?;
    if let Err(err) = transport.send(cx, test).await {
        tracing::info!(tenant = slug, %err, "a test mail did not go out");
        return fail("mail_server_failed");
    }
    let cmd = ConfigureMailServer {
        host,
        port,
        security: form.security,
        username,
        password_sealed: password
            .as_deref()
            .map(|password| shop_mail.seal(password))
            .transpose()
            .map_err(topcoat::Error::from_anyhow)?,
        from_address: form.from_address,
        from_name,
    };
    match tenancy(cx)
        .configure_mail_server(&access.user_id, &access.tenant.id, cmd)
        .await
    {
        Ok(()) => Ok(see_other(mail_url(
            cx,
            slug,
            None,
            Some("Test mail sent: your mails now go out through your server."),
        ))),
        Err(err) if err.is_user_facing() => fail(error_code(&err)),
        Err(err) => Err(err.into()),
    }
}
