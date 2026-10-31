//! A shop's own mail servers, for members with `mail:manage`: `add` (from
//! the form on `/t/{slug}/mail`), `pick` (which server sends what) and
//! `{server_id}` (one server, to change or delete). Saving a server sends a
//! test mail through it first: what cannot deliver is not kept.

pub mod add;
pub mod pick;
pub mod server_id;

use serde::Deserialize;
use timada_tenancy::{
    MAIL_SECURITIES, MAIL_SECURITY_NONE, MAIL_SECURITY_STARTTLS, MAIL_SECURITY_TLS,
    MAX_MAIL_SERVER_NAME_LEN, MailServerSettings, SaveMailServer, TenantState,
};
use topcoat::{
    Result,
    context::{Cx, app_context},
    mail::{Mailbox, Transport, mail},
    view::{View, attributes, component, view},
};

use crate::components::button::button;
use crate::components::field::{field, field_label};
use crate::components::input::input;
use crate::components::select::select;
use crate::shop_mail::{MailServer, NoAnswer, ShopMail};

/// A server as the merchant fills it in, to add or to change. Every field a
/// string: a urlencoded form hands them so, and a bad port is the page's
/// message rather than a 400.
#[derive(Deserialize)]
pub struct ServerForm {
    #[serde(default)]
    name: String,
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

/// The password a server is saved with.
#[derive(Debug, PartialEq, Eq)]
pub enum Password {
    /// The one typed in the form.
    Typed(String),
    /// The saved one, still sealed.
    Kept(Vec<u8>),
    None,
}

/// What the password field means. Left blank, it keeps the saved password,
/// but only for the same user name on the same host: a password is never
/// sent to a server other than the one it was typed for.
pub fn kept_password(
    typed: &str,
    host: &str,
    username: Option<&str>,
    saved: Option<&MailServerSettings>,
) -> Password {
    if !typed.is_empty() {
        return Password::Typed(typed.to_owned());
    }
    match (saved, username) {
        (Some(saved), Some(username))
            if saved.username.as_deref() == Some(username)
                && saved.host.eq_ignore_ascii_case(host) =>
        {
            saved
                .password_sealed
                .clone()
                .map_or(Password::None, Password::Kept)
        }
        _ => Password::None,
    }
}

/// Checks `form` and sends a test mail through the server it describes:
/// that server, ready to record, or the code of the message to show.
/// `saved` is the server being changed; `None` for a new one.
pub async fn tested(
    cx: &Cx,
    tenant: &TenantState,
    form: ServerForm,
    saved: Option<&MailServerSettings>,
) -> Result<std::result::Result<SaveMailServer, &'static str>> {
    let shop_mail = app_context::<ShopMail>(cx);
    if !shop_mail.enabled() {
        return Ok(Err("mail_servers_disabled"));
    }
    let filled = |value: &str| Some(value.trim().to_owned()).filter(|value| !value.is_empty());
    let name = filled(&form.name);
    let host = form.host.trim().to_owned();
    let username = filled(&form.username);
    let from_name = filled(&form.from_name);
    let Ok(port) = form.port.trim().parse::<u16>() else {
        return Ok(Err("invalid_mail_server"));
    };
    if host.is_empty()
        || port == 0
        || !MAIL_SECURITIES.contains(&form.security.as_str())
        || name
            .as_ref()
            .is_some_and(|name| name.chars().count() > MAX_MAIL_SERVER_NAME_LEN)
    {
        return Ok(Err("invalid_mail_server"));
    }
    let (Ok(from), Ok(test_to)) = (
        Mailbox::named(
            from_name.as_deref().unwrap_or(&tenant.name),
            form.from_address.trim(),
        ),
        Mailbox::new(form.test_to.trim()),
    ) else {
        return Ok(Err("invalid_email"));
    };
    let (password, kept) = match kept_password(&form.password, &host, username.as_deref(), saved) {
        Password::Typed(password) => (Some(password), None),
        Password::Kept(sealed) => match shop_mail.unseal(&tenant.id, &sealed) {
            Ok(password) => (Some(password), Some(sealed)),
            Err(err) => {
                // Sealed under a key the platform no longer has: the
                // merchant types it again.
                tracing::info!(tenant = %tenant.slug, %err, "a saved mail server password did not open");
                return Ok(Err("mail_server_failed"));
            }
        },
        Password::None => (None, None),
    };
    if let Err(err) = shop_mail.check_host(&host, port).await {
        tracing::info!(tenant = %tenant.slug, %err, "a shop mail server was refused");
        return Ok(Err("mail_server_not_public"));
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
    let test = mail! {
        from: from,
        to: test_to,
        subject: "Your shop's mail server works",
        html: {
            cx =>
            <p>"This test mail went out through " <strong>(&shown_host)</strong> "."</p>
            <p>"Pick what it sends on your shop's Mail page."</p>
        },
    }?;
    if let Err(err) = transport.send(cx, test).await {
        tracing::info!(tenant = %tenant.slug, %err, "a test mail did not go out");
        return Ok(Err(if err.is::<NoAnswer>() {
            "mail_server_silent"
        } else {
            "mail_server_failed"
        }));
    }
    let password_sealed = match (kept, password) {
        (Some(sealed), _) => Some(sealed),
        (None, Some(password)) => Some(
            shop_mail
                .seal(&tenant.id, &password)
                .map_err(topcoat::Error::from_anyhow)?,
        ),
        (None, None) => None,
    };
    Ok(Ok(SaveMailServer {
        name,
        host,
        port,
        security: form.security,
        username,
        password_sealed,
        from_address: form.from_address,
        from_name,
    }))
}

/// How a connection is secured, as the form names it.
pub fn security_label(security: &str) -> &'static str {
    match security {
        MAIL_SECURITY_TLS => "TLS",
        MAIL_SECURITY_STARTTLS => "STARTTLS",
        MAIL_SECURITY_NONE => "None",
        _ => "Unknown",
    }
}

/// What a server is picked for: the shop's mails, its marketing.
pub fn picked_for(tenant: &TenantState, server_id: &str) -> (bool, bool) {
    (
        tenant
            .shop_mails_server()
            .is_some_and(|server| server.id == server_id),
        tenant
            .marketing_server()
            .is_some_and(|server| server.id == server_id),
    )
}

/// The settings of a server, blank (`current` has no id) to add one or
/// filled from `current` to change it; saving sends a test mail to
/// `test_to` first.
#[component]
pub async fn server_form(
    action: String,
    current: &MailServerSettings,
    shop_name: &str,
    test_to: &str,
) -> Result<impl View> {
    let port = if current.port == 0 {
        "587".to_owned()
    } else {
        current.port.to_string()
    };
    let security = if current.security.is_empty() {
        MAIL_SECURITY_STARTTLS
    } else {
        current.security.as_str()
    };
    let has_password = current.password_sealed.is_some();
    let max_name = MAX_MAIL_SERVER_NAME_LEN.to_string();
    Ok(view! {
        <form method="post" action=(action) class="grid max-w-3xl gap-4 md:grid-cols-2">
            field(
                attrs: attributes! { class="md:col-span-2" },
                field_label(attrs: attributes! { for="mail-name" }, "Name")
                input(attrs: attributes! {
                    id="mail-name" name="name" maxlength=(&max_name) placeholder="Newsletter"
                    value=(current.name.as_deref().unwrap_or_default()) class="max-w-sm"
                })
                <p class="text-sm text-muted-foreground">"Optional: how you tell your servers apart. Without one, it goes by its address."</p>
            )
            field(
                field_label(attrs: attributes! { for="mail-host" }, "Server")
                input(attrs: attributes! {
                    id="mail-host" name="host" required="" maxlength="253" placeholder="smtp.example.com" value=(&current.host) class="font-mono"
                })
            )
            field(
                field_label(attrs: attributes! { for="mail-port" }, "Port")
                input(attrs: attributes! {
                    id="mail-port" name="port" type="number" min="1" max="65535" required="" value=(&port) class="font-mono"
                })
            )
            field(
                field_label(attrs: attributes! { for="mail-security" }, "Security")
                select(
                    attrs: attributes! { id="mail-security" name="security" },
                    for (value, label) in [
                        (MAIL_SECURITY_STARTTLS, "STARTTLS (usually port 587)"),
                        (MAIL_SECURITY_TLS, "TLS (usually port 465)"),
                        (MAIL_SECURITY_NONE, "None (not encrypted)"),
                    ] {
                        <option value=(value) selected=(security == value)>(label)</option>
                    }
                )
            )
            field(
                field_label(attrs: attributes! { for="mail-username" }, "User name")
                input(attrs: attributes! {
                    id="mail-username" name="username" maxlength="254" autocomplete="off"
                    value=(current.username.as_deref().unwrap_or_default()) class="font-mono"
                })
            )
            field(
                field_label(attrs: attributes! { for="mail-password" }, "Password")
                input(attrs: attributes! {
                    id="mail-password" name="password" type="password" autocomplete="off"
                    placeholder=(if has_password { "Leave blank to keep the saved one" } else { "" }) class="font-mono"
                })
                if has_password {
                    <p class="text-sm text-muted-foreground">"The saved one is kept only for the same server and user name."</p>
                }
            )
            field(
                field_label(attrs: attributes! { for="mail-from-address" }, "Send from address")
                input(attrs: attributes! {
                    id="mail-from-address" name="from_address" type="email" required="" maxlength="254"
                    placeholder="orders@example.com" value=(&current.from_address)
                })
            )
            field(
                field_label(attrs: attributes! { for="mail-from-name" }, "Send from name")
                input(attrs: attributes! {
                    id="mail-from-name" name="from_name" maxlength="80" placeholder=(shop_name)
                    value=(current.from_name.as_deref().unwrap_or_default())
                })
            )
            field(
                field_label(attrs: attributes! { for="mail-test-to" }, "Send the test mail to")
                input(attrs: attributes! {
                    id="mail-test-to" name="test_to" type="email" required="" maxlength="254" value=(test_to)
                })
            )
            <div class="md:col-span-2">
                button(attrs: attributes! { type="submit" }, "Test and save")
            </div>
        </form>
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved() -> MailServerSettings {
        MailServerSettings {
            id: "a".into(),
            host: "smtp.acme.test".into(),
            username: Some("shop".into()),
            password_sealed: Some(vec![1, 2, 3]),
            ..Default::default()
        }
    }

    #[test]
    fn a_blank_password_is_kept_for_the_same_user_on_the_same_host_only() {
        let saved = saved();
        assert_eq!(
            kept_password("", "SMTP.acme.test", Some("shop"), Some(&saved)),
            Password::Kept(vec![1, 2, 3])
        );
        assert_eq!(
            kept_password("new", "smtp.acme.test", Some("shop"), Some(&saved)),
            Password::Typed("new".into())
        );
        // Another host would get the saved password: it is not kept.
        assert_eq!(
            kept_password("", "smtp.elsewhere.test", Some("shop"), Some(&saved)),
            Password::None
        );
        assert_eq!(
            kept_password("", "smtp.acme.test", Some("other"), Some(&saved)),
            Password::None
        );
        assert_eq!(
            kept_password("", "smtp.acme.test", None, Some(&saved)),
            Password::None
        );
        assert_eq!(
            kept_password("", "smtp.acme.test", Some("shop"), None),
            Password::None
        );
    }
}
