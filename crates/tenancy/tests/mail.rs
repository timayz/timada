mod common;

use common::*;
use timada_tenancy::{
    ConfigureMailServer, DefineRole, SetMailBranding, ShopMailKind, TenancyError,
};

fn role(name: &str, permission: &str) -> DefineRole {
    DefineRole {
        role: name.to_owned(),
        label: name.to_owned(),
        permissions: vec![permission.to_owned()],
    }
}

fn server(host: &str, security: &str) -> ConfigureMailServer {
    ConfigureMailServer {
        host: host.into(),
        port: 587,
        security: security.into(),
        username: Some(" shop@acme.test ".into()),
        password_sealed: Some(vec![1, 2, 3]),
        from_address: " orders@acme.test ".into(),
        from_name: Some("  ".into()),
    }
}

#[tokio::test]
async fn an_owner_sets_up_and_removes_a_mail_server() {
    let cmd = command().await;
    let alice = new_id();
    let tenant = create_tenant(&cmd, "Acme", &alice).await;

    cmd.configure_mail_server(&alice, &tenant.id, server(" smtp.acme.test ", "starttls"))
        .await
        .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    let mail = state.mail_server.unwrap();
    assert_eq!(mail.host, "smtp.acme.test");
    assert_eq!(mail.port, 587);
    assert_eq!(mail.security, "starttls");
    assert_eq!(mail.username.as_deref(), Some("shop@acme.test"));
    assert_eq!(mail.password_sealed, Some(vec![1, 2, 3]));
    assert_eq!(mail.from_address, "orders@acme.test");
    assert_eq!(mail.from_name, None, "a blank name is the tenant's");
    assert_eq!(mail.configured_by, alice);

    for bad in [
        server("", "tls"),
        server("smtp.acme.test", "ssl"),
        server("smtp acme", "tls"),
        ConfigureMailServer {
            port: 0,
            ..server("smtp.acme.test", "tls")
        },
        ConfigureMailServer {
            from_address: "orders".into(),
            ..server("smtp.acme.test", "tls")
        },
    ] {
        let err = cmd
            .configure_mail_server(&alice, &tenant.id, bad)
            .await
            .unwrap_err();
        assert!(matches!(err, TenancyError::InvalidMailServer), "{err}");
    }
    let err = cmd
        .configure_mail_server(&new_id(), &tenant.id, server("smtp.acme.test", "tls"))
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Forbidden), "{err}");
    let err = cmd
        .remove_mail_server(&new_id(), &tenant.id)
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Forbidden), "{err}");

    cmd.remove_mail_server(&alice, &tenant.id).await.unwrap();
    cmd.remove_mail_server(&alice, &tenant.id).await.unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert!(state.mail_server.is_none());
}

#[tokio::test]
async fn a_member_needs_mail_manage() {
    let cmd = command().await;
    let alice = new_id();
    let bob = new_id();
    let tenant = create_tenant(&cmd, "Acme", &alice).await;
    cmd.define_role(&alice, &tenant.id, role("mailer", "mail:manage"))
        .await
        .unwrap();
    cmd.define_role(&alice, &tenant.id, role("clerk", "orders:manage"))
        .await
        .unwrap();
    cmd.grant_member(&alice, &tenant.id, &bob, "clerk")
        .await
        .unwrap();
    let err = cmd
        .configure_mail_server(&bob, &tenant.id, server("smtp.acme.test", "tls"))
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Forbidden), "{err}");

    cmd.change_member_role(&alice, &tenant.id, &bob, "mailer")
        .await
        .unwrap();
    cmd.configure_mail_server(&bob, &tenant.id, server("smtp.acme.test", "tls"))
        .await
        .unwrap();
}

#[tokio::test]
async fn an_owner_words_a_mail_and_resets_it() {
    let cmd = command().await;
    let alice = new_id();
    let tenant = create_tenant(&cmd, "Acme", &alice).await;
    let kind = ShopMailKind::ShopperCode;

    cmd.save_mail_template(
        &alice,
        &tenant.id,
        kind.key(),
        "  Code for {{ shop.name }} ",
        "<p>{{ code }}</p>",
    )
    .await
    .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    let saved = state.mail_template(kind).unwrap();
    assert_eq!(saved.subject, "Code for {{ shop.name }}");
    assert_eq!(saved.html, "<p>{{ code }}</p>");
    assert_eq!(saved.saved_by, alice);

    // Saving the same wording again records nothing.
    cmd.save_mail_template(
        &alice,
        &tenant.id,
        kind.key(),
        "Code for {{ shop.name }}",
        "<p>{{ code }}</p>",
    )
    .await
    .unwrap();

    let err = cmd
        .save_mail_template(&alice, &tenant.id, kind.key(), "Code", "<p>{{ nope }}</p>")
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::InvalidMailTemplate(_)), "{err}");
    assert_eq!(
        err.to_string(),
        "Body: this mail has no variable {{ nope }}."
    );
    let err = cmd
        .save_mail_template(&alice, &tenant.id, "welcome", "Hi", "<p>Hi</p>")
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::UnknownMail), "{err}");
    let err = cmd
        .reset_mail_template(&new_id(), &tenant.id, kind.key())
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Forbidden), "{err}");

    cmd.reset_mail_template(&alice, &tenant.id, kind.key())
        .await
        .unwrap();
    cmd.reset_mail_template(&alice, &tenant.id, kind.key())
        .await
        .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert!(state.mail_template(kind).is_none());
    let changes: Vec<_> = state
        .mail_template_changes(kind)
        .map(|change| (change.by.as_str(), change.reset))
        .collect();
    assert_eq!(changes, [(alice.as_str(), true), (alice.as_str(), false)]);
}

#[tokio::test]
async fn wording_a_mail_needs_mail_manage() {
    let cmd = command().await;
    let alice = new_id();
    let bob = new_id();
    let tenant = create_tenant(&cmd, "Acme", &alice).await;
    cmd.define_role(&alice, &tenant.id, role("clerk", "orders:manage"))
        .await
        .unwrap();
    cmd.grant_member(&alice, &tenant.id, &bob, "clerk")
        .await
        .unwrap();
    let kind = ShopMailKind::ReviewRequest;
    let err = cmd
        .save_mail_template(
            &bob,
            &tenant.id,
            kind.key(),
            kind.default_subject(),
            kind.default_html(),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Forbidden), "{err}");
}

#[tokio::test]
async fn an_owner_brands_the_mails_and_clears_it() {
    let cmd = command().await;
    let alice = new_id();
    let tenant = create_tenant(&cmd, "Acme", &alice).await;

    cmd.set_mail_branding(
        &alice,
        &tenant.id,
        SetMailBranding {
            logo_file: Some("01LOGO.png".into()),
            logo_size: 1234,
            accent: Some(" #C0FFEE ".into()),
            footer: Some("Acme SARL\r\nParis ".into()),
        },
    )
    .await
    .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    let branding = state.mail_branding.unwrap();
    assert_eq!(branding.logo_file.as_deref(), Some("01LOGO.png"));
    assert_eq!(branding.logo_size, 1234);
    assert_eq!(branding.accent.as_deref(), Some("#c0ffee"));
    assert_eq!(branding.footer.as_deref(), Some("Acme SARL\nParis"));
    assert_eq!(branding.set_by, alice);

    for bad in [
        SetMailBranding {
            accent: Some("blue".into()),
            ..Default::default()
        },
        SetMailBranding {
            footer: Some("x".repeat(501)),
            ..Default::default()
        },
        SetMailBranding {
            logo_file: Some("../secret.png".into()),
            ..Default::default()
        },
    ] {
        let err = cmd
            .set_mail_branding(&alice, &tenant.id, bad)
            .await
            .unwrap_err();
        assert!(matches!(err, TenancyError::InvalidMailBranding), "{err}");
    }
    let err = cmd
        .set_mail_branding(&new_id(), &tenant.id, SetMailBranding::default())
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Forbidden), "{err}");

    cmd.clear_mail_branding(&alice, &tenant.id).await.unwrap();
    cmd.clear_mail_branding(&alice, &tenant.id).await.unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert!(state.mail_branding.is_none());
}
