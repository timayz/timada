mod common;

use common::*;
use timada_tenancy::aggregator::{MailServerConfigured, MailServerRemoved};
use timada_tenancy::{
    DefineRole, LEGACY_MAIL_SERVER_ID, MAX_MAIL_SERVERS, PickMailServers, SaveMailServer,
    SaveMailTemplate, SetMailBranding, SetMailLanguages, ShopMailKind, TenancyError,
};

fn role(name: &str, permission: &str) -> DefineRole {
    DefineRole {
        role: name.to_owned(),
        label: name.to_owned(),
        permissions: vec![permission.to_owned()],
    }
}

fn server(host: &str, security: &str) -> SaveMailServer {
    SaveMailServer {
        name: Some("  ".into()),
        host: host.into(),
        port: 587,
        security: security.into(),
        username: Some(" shop@acme.test ".into()),
        password_sealed: Some(vec![1, 2, 3]),
        from_address: " orders@acme.test ".into(),
        from_name: Some("  ".into()),
    }
}

fn picks(shop_mails: Option<&str>, marketing: Option<&str>) -> PickMailServers {
    PickMailServers {
        shop_mails: shop_mails.map(str::to_owned),
        marketing: marketing.map(str::to_owned),
    }
}

#[tokio::test]
async fn an_owner_adds_changes_and_deletes_mail_servers() {
    let cmd = command().await;
    let alice = new_id();
    let tenant = create_tenant(&cmd, "Acme", &alice).await;

    // The first server sends everything.
    let orders = cmd
        .add_mail_server(&alice, &tenant.id, server(" smtp.acme.test ", "starttls"))
        .await
        .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    let mail = state.mail_server(&orders).unwrap();
    assert_eq!(mail.id, orders);
    assert_eq!(mail.name, None, "a blank name shows the address");
    assert_eq!(mail.label(), "orders@acme.test");
    assert_eq!(mail.host, "smtp.acme.test");
    assert_eq!(mail.port, 587);
    assert_eq!(mail.security, "starttls");
    assert_eq!(mail.username.as_deref(), Some("shop@acme.test"));
    assert_eq!(mail.password_sealed, Some(vec![1, 2, 3]));
    assert_eq!(mail.from_address, "orders@acme.test");
    assert_eq!(mail.from_name, None, "a blank name is the tenant's");
    assert_eq!(mail.configured_by, alice);
    assert_eq!(state.shop_mails_server().unwrap().id, orders);
    assert_eq!(state.marketing_server().unwrap().id, orders);

    // A later one sends nothing until picked.
    let news = cmd
        .add_mail_server(
            &alice,
            &tenant.id,
            SaveMailServer {
                name: Some(" News ".into()),
                from_address: "news@acme.test".into(),
                ..server("smtp.news.test", "tls")
            },
        )
        .await
        .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    let ids: Vec<&str> = state.mail_servers.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, [orders.as_str(), news.as_str()]);
    assert_eq!(state.mail_server(&news).unwrap().label(), "News");
    assert_eq!(state.shop_mails_server().unwrap().id, orders);
    assert_eq!(state.marketing_server().unwrap().id, orders);

    cmd.pick_mail_servers(&alice, &tenant.id, picks(Some(&orders), Some(&news)))
        .await
        .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert_eq!(state.shop_mails_server().unwrap().id, orders);
    assert_eq!(state.marketing_server().unwrap().id, news);
    let version = state.aggregate_version;
    cmd.pick_mail_servers(&alice, &tenant.id, picks(Some(&orders), Some(&news)))
        .await
        .unwrap();
    let err = cmd
        .pick_mail_servers(&alice, &tenant.id, picks(None, Some(&new_id())))
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::MailServerNotFound), "{err}");

    // A change keeps the server's place and what it is picked for.
    let changed = SaveMailServer {
        name: Some("Orders".into()),
        ..server("smtp2.acme.test", "tls")
    };
    cmd.change_mail_server(&alice, &tenant.id, &orders, changed.clone())
        .await
        .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert_eq!(state.mail_servers[0].id, orders);
    assert_eq!(state.mail_servers[0].host, "smtp2.acme.test");
    assert_eq!(state.mail_servers[0].label(), "Orders");
    assert_eq!(state.shop_mails_server().unwrap().id, orders);
    assert_eq!(state.marketing_server().unwrap().id, news);
    assert_eq!(
        state.aggregate_version,
        version + 1,
        "an unchanged pick wrote nothing"
    );
    let version = state.aggregate_version;
    cmd.change_mail_server(&alice, &tenant.id, &orders, changed)
        .await
        .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert_eq!(
        state.aggregate_version, version,
        "an unchanged server wrote nothing"
    );
    let err = cmd
        .change_mail_server(
            &alice,
            &tenant.id,
            &new_id(),
            server("smtp.acme.test", "tls"),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::MailServerNotFound), "{err}");

    for bad in [
        server("", "tls"),
        server("smtp.acme.test", "ssl"),
        server("smtp acme", "tls"),
        SaveMailServer {
            port: 0,
            ..server("smtp.acme.test", "tls")
        },
        SaveMailServer {
            from_address: "orders".into(),
            ..server("smtp.acme.test", "tls")
        },
        SaveMailServer {
            name: Some("n".repeat(81)),
            ..server("smtp.acme.test", "tls")
        },
    ] {
        let err = cmd
            .add_mail_server(&alice, &tenant.id, bad.clone())
            .await
            .unwrap_err();
        assert!(matches!(err, TenancyError::InvalidMailServer), "{err}");
        let err = cmd
            .change_mail_server(&alice, &tenant.id, &orders, bad)
            .await
            .unwrap_err();
        assert!(matches!(err, TenancyError::InvalidMailServer), "{err}");
    }

    let stranger = new_id();
    let err = cmd
        .add_mail_server(&stranger, &tenant.id, server("smtp.acme.test", "tls"))
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Forbidden), "{err}");
    let err = cmd
        .change_mail_server(
            &stranger,
            &tenant.id,
            &orders,
            server("smtp.acme.test", "tls"),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Forbidden), "{err}");
    let err = cmd
        .pick_mail_servers(&stranger, &tenant.id, picks(None, None))
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Forbidden), "{err}");
    let err = cmd
        .delete_mail_server(&stranger, &tenant.id, &orders)
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Forbidden), "{err}");

    // Deleting the marketing server leaves marketing with no server: it
    // waits, it never falls back to the shop mails' server.
    cmd.delete_mail_server(&alice, &tenant.id, &news)
        .await
        .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert_eq!(state.mail_servers.len(), 1);
    assert_eq!(state.shop_mails_server().unwrap().id, orders);
    assert!(state.marketing_server().is_none());
    assert_eq!(state.marketing_server_id, None);
    let err = cmd
        .delete_mail_server(&alice, &tenant.id, &news)
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::MailServerNotFound), "{err}");

    // Shop mails through the platform's server, by choice.
    cmd.pick_mail_servers(&alice, &tenant.id, picks(None, Some(&orders)))
        .await
        .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert!(state.shop_mails_server().is_none());
    assert_eq!(state.marketing_server().unwrap().id, orders);

    cmd.delete_mail_server(&alice, &tenant.id, &orders)
        .await
        .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert!(state.mail_servers.is_empty());
    assert!(state.shop_mails_server().is_none());
    assert!(state.marketing_server().is_none());

    // With none left, the next one is the first again.
    let mut ids = Vec::new();
    for n in 0..MAX_MAIL_SERVERS {
        let host = format!("smtp{n}.acme.test");
        ids.push(
            cmd.add_mail_server(&alice, &tenant.id, server(&host, "tls"))
                .await
                .unwrap(),
        );
    }
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert_eq!(state.shop_mails_server().unwrap().id, ids[0]);
    assert_eq!(state.marketing_server().unwrap().id, ids[0]);
    let err = cmd
        .add_mail_server(&alice, &tenant.id, server("smtp.acme.test", "tls"))
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::TooManyMailServers), "{err}");
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
        .add_mail_server(&bob, &tenant.id, server("smtp.acme.test", "tls"))
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Forbidden), "{err}");

    cmd.change_member_role(&alice, &tenant.id, &bob, "mailer")
        .await
        .unwrap();
    let id = cmd
        .add_mail_server(&bob, &tenant.id, server("smtp.acme.test", "tls"))
        .await
        .unwrap();
    cmd.pick_mail_servers(&bob, &tenant.id, picks(None, Some(&id)))
        .await
        .unwrap();
}

/// The one server a shop set up before it could have several.
fn legacy_server(host: &str) -> MailServerConfigured {
    MailServerConfigured {
        host: host.into(),
        port: 587,
        security: "starttls".into(),
        username: Some("shop@acme.test".into()),
        password_sealed: Some(vec![1, 2, 3]),
        from_address: "orders@acme.test".into(),
        from_name: None,
        configured_by: "alice".into(),
    }
}

#[tokio::test]
async fn a_server_set_up_before_shops_could_have_several_does_both() {
    let (executor, _pool) = executor_with_pool().await;
    let cmd = timada_tenancy::Command(executor.clone());
    let alice = new_id();
    let tenant = create_tenant(&cmd, "Acme", &alice).await;
    // What the log holds from before: set up, then set up again.
    evento::append(&tenant.id)
        .original_version(tenant.aggregate_version)
        .event(&legacy_server("smtp.acme.test"))
        .event(&legacy_server("smtp2.acme.test"))
        .commit(&executor)
        .await
        .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert_eq!(state.mail_servers.len(), 1);
    let main = &state.mail_servers[0];
    assert_eq!(main.id, LEGACY_MAIL_SERVER_ID);
    assert_eq!(main.host, "smtp2.acme.test");
    assert_eq!(main.label(), "orders@acme.test");
    assert_eq!(state.shop_mails_server().unwrap().id, LEGACY_MAIL_SERVER_ID);
    assert_eq!(state.marketing_server().unwrap().id, LEGACY_MAIL_SERVER_ID);

    // The commands handle it like any other.
    let news = cmd
        .add_mail_server(&alice, &tenant.id, server("smtp.news.test", "tls"))
        .await
        .unwrap();
    cmd.pick_mail_servers(
        &alice,
        &tenant.id,
        picks(Some(LEGACY_MAIL_SERVER_ID), Some(&news)),
    )
    .await
    .unwrap();
    cmd.change_mail_server(
        &alice,
        &tenant.id,
        LEGACY_MAIL_SERVER_ID,
        SaveMailServer {
            name: Some("Orders".into()),
            ..server("smtp3.acme.test", "tls")
        },
    )
    .await
    .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert_eq!(state.mail_servers[0].label(), "Orders");
    assert_eq!(state.shop_mails_server().unwrap().id, LEGACY_MAIL_SERVER_ID);
    assert_eq!(state.marketing_server().unwrap().id, news);

    // A late one, from a node not upgraded yet, takes no pick away.
    evento::append(&tenant.id)
        .original_version(state.aggregate_version)
        .event(&legacy_server("smtp4.acme.test"))
        .commit(&executor)
        .await
        .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert_eq!(state.mail_servers.len(), 2);
    assert_eq!(state.mail_servers[0].host, "smtp4.acme.test");
    assert_eq!(state.marketing_server().unwrap().id, news);

    // Removed the old way, it takes its pick with it.
    evento::append(&tenant.id)
        .original_version(state.aggregate_version)
        .event(&MailServerRemoved {
            removed_by: alice.clone(),
        })
        .commit(&executor)
        .await
        .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert_eq!(state.mail_servers.len(), 1);
    assert!(state.shop_mails_server().is_none());
    assert_eq!(state.marketing_server().unwrap().id, news);
}

/// A mail's wording, as the editor sends it.
fn wording(subject: &str, html: &str, text: Option<&str>) -> SaveMailTemplate {
    SaveMailTemplate {
        subject: subject.into(),
        html: html.into(),
        text: text.map(str::to_owned),
    }
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
        "en",
        wording("  Code for {{ shop.name }} ", "<p>{{ code }}</p>", None),
    )
    .await
    .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    let saved = state.mail_template_in(kind, "en").unwrap();
    assert_eq!(saved.subject, "Code for {{ shop.name }}");
    assert_eq!(saved.html, "<p>{{ code }}</p>");
    assert_eq!(saved.text, None);
    assert_eq!(saved.saved_by, alice);

    // Saving the same wording again records nothing.
    cmd.save_mail_template(
        &alice,
        &tenant.id,
        kind.key(),
        "en",
        wording("Code for {{ shop.name }}", "<p>{{ code }}</p>", Some("  ")),
    )
    .await
    .unwrap();

    let err = cmd
        .save_mail_template(
            &alice,
            &tenant.id,
            kind.key(),
            "en",
            wording("Code", "<p>{{ nope }}</p>", None),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::InvalidMailTemplate(_)), "{err}");
    assert_eq!(
        err.to_string(),
        "Body: this mail has no variable {{ nope }}."
    );
    let err = cmd
        .save_mail_template(
            &alice,
            &tenant.id,
            "welcome",
            "en",
            wording("Hi", "<p>Hi</p>", None),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::UnknownMail), "{err}");
    let err = cmd
        .reset_mail_template(&new_id(), &tenant.id, kind.key(), "en")
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Forbidden), "{err}");

    cmd.reset_mail_template(&alice, &tenant.id, kind.key(), "en")
        .await
        .unwrap();
    cmd.reset_mail_template(&alice, &tenant.id, kind.key(), "en")
        .await
        .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert!(state.mail_template_in(kind, "en").is_none());
    let changes: Vec<_> = state
        .mail_template_changes(kind, "en")
        .map(|change| (change.by.as_str(), change.reset))
        .collect();
    assert_eq!(changes, [(alice.as_str(), true), (alice.as_str(), false)]);
}

#[tokio::test]
async fn an_owner_writes_the_plain_text_of_a_mail() {
    let cmd = command().await;
    let alice = new_id();
    let tenant = create_tenant(&cmd, "Acme", &alice).await;
    let kind = ShopMailKind::ShopperCode;
    let saved_text = |state: &timada_tenancy::TenantState| {
        state
            .mail_template_in(kind, "en")
            .and_then(|template| template.text.clone())
    };

    cmd.save_mail_template(
        &alice,
        &tenant.id,
        kind.key(),
        "en",
        wording("Code", "<p>{{ code }}</p>", Some("Your code: {{ code }}")),
    )
    .await
    .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert_eq!(saved_text(&state).as_deref(), Some("Your code: {{ code }}"));

    // A plain text naming a variable the mail lacks is refused, and nothing
    // is recorded.
    let err = cmd
        .save_mail_template(
            &alice,
            &tenant.id,
            kind.key(),
            "en",
            wording("Code", "<p>{{ code }}</p>", Some("{{ order.reference }}")),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::InvalidMailTemplate(_)), "{err}");
    assert_eq!(
        err.to_string(),
        "Plain text: this mail has no variable {{ order.reference }}."
    );

    // Saving without it goes back to the text derived from the HTML.
    cmd.save_mail_template(
        &alice,
        &tenant.id,
        kind.key(),
        "en",
        wording("Code", "<p>{{ code }}</p>", None),
    )
    .await
    .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert!(state.mail_template_in(kind, "en").is_some());
    assert_eq!(saved_text(&state), None);

    cmd.save_mail_template(
        &alice,
        &tenant.id,
        kind.key(),
        "en",
        wording("Code", "<p>{{ code }}</p>", Some("Code: {{ code }}")),
    )
    .await
    .unwrap();
    cmd.reset_mail_template(&alice, &tenant.id, kind.key(), "en")
        .await
        .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert!(state.mail_template_in(kind, "en").is_none());
    let saves = state
        .mail_template_changes(kind, "en")
        .filter(|change| !change.reset)
        .count();
    assert_eq!(
        saves, 3,
        "the plain text is part of each save, not a change of its own"
    );
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
            "en",
            wording(kind.default_subject(), kind.default_html(), None),
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

/// The languages a shop picks, as the settings form sends them.
fn languages(default: &str, others: &[&str]) -> SetMailLanguages {
    SetMailLanguages {
        default: default.into(),
        others: others.iter().map(|code| (*code).to_owned()).collect(),
    }
}

#[tokio::test]
async fn a_shop_picks_the_languages_it_mails_in() {
    let cmd = command().await;
    let alice = new_id();
    let bob = new_id();
    let tenant = create_tenant(&cmd, "Acme", &alice).await;
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert_eq!(state.mail_default_language(), "en", "English until picked");
    assert_eq!(state.mail_languages(), ["en"]);
    assert!(state.mails_in("en") && !state.mails_in("fr"));

    // Repeats and the default among the others are dropped; the rest come
    // in the list's order, whatever the case typed.
    cmd.set_mail_languages(
        &alice,
        &tenant.id,
        languages(" FR ", &["de", "fr", "es", "de"]),
    )
    .await
    .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert_eq!(state.mail_default_language(), "fr");
    assert_eq!(state.mail_languages(), ["fr", "de", "es"]);
    assert_eq!(state.mail_languages.as_ref().unwrap().set_by, alice);
    assert!(state.mails_in("es") && !state.mails_in("en"));

    for bad in [
        languages("zz", &[]),
        languages("fr", &["xx"]),
        languages("", &[]),
    ] {
        let err = cmd
            .set_mail_languages(&alice, &tenant.id, bad)
            .await
            .unwrap_err();
        assert!(matches!(err, TenancyError::InvalidMailLanguages), "{err}");
    }
    cmd.define_role(&alice, &tenant.id, role("clerk", "orders:manage"))
        .await
        .unwrap();
    cmd.grant_member(&alice, &tenant.id, &bob, "clerk")
        .await
        .unwrap();
    let err = cmd
        .set_mail_languages(&bob, &tenant.id, languages("en", &[]))
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Forbidden), "{err}");

    // Picking the same again records nothing.
    cmd.set_mail_languages(&alice, &tenant.id, languages("fr", &["es", "de"]))
        .await
        .unwrap();
    let again = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert_eq!(again.mail_languages, state.mail_languages);
}

#[tokio::test]
async fn a_mail_is_worded_per_language() {
    let cmd = command().await;
    let alice = new_id();
    let tenant = create_tenant(&cmd, "Acme", &alice).await;
    let kind = ShopMailKind::ShopperCode;

    // Not a language the shop mails in yet.
    let french = wording(
        "Code pour {{ shop.name }}",
        "<p>Bonjour {{ code }}</p>",
        None,
    );
    let err = cmd
        .save_mail_template(&alice, &tenant.id, kind.key(), "fr", french.clone())
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::UnknownLanguage), "{err}");
    let err = cmd
        .reset_mail_template(&alice, &tenant.id, kind.key(), "fr")
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::UnknownLanguage), "{err}");

    cmd.set_mail_languages(&alice, &tenant.id, languages("en", &["fr"]))
        .await
        .unwrap();
    cmd.save_mail_template(&alice, &tenant.id, kind.key(), " FR ", french)
        .await
        .unwrap();
    cmd.save_mail_template(
        &alice,
        &tenant.id,
        kind.key(),
        "en",
        wording(
            "Code for {{ shop.name }}",
            "<p>Hello {{ code }}</p>",
            Some("Hi {{ code }}"),
        ),
    )
    .await
    .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert_eq!(
        state.mail_template_in(kind, "fr").unwrap().html,
        "<p>Bonjour {{ code }}</p>"
    );
    assert_eq!(
        state.mail_template_in(kind, "en").unwrap().html,
        "<p>Hello {{ code }}</p>"
    );
    assert_eq!(
        state.mail_template_in(kind, "en").unwrap().text.as_deref(),
        Some("Hi {{ code }}")
    );
    assert_eq!(state.mail_template_in(kind, "fr").unwrap().text, None);

    // A shopper gets theirs, or the default language's: a German speaker,
    // and one whose language is unknown, are mailed in English.
    let subject = |language: Option<&str>| {
        state
            .mail_template_for(kind, language)
            .map(|template| template.subject.as_str())
    };
    assert_eq!(subject(Some("fr")), Some("Code pour {{ shop.name }}"));
    assert_eq!(subject(Some("de")), Some("Code for {{ shop.name }}"));
    assert_eq!(subject(None), Some("Code for {{ shop.name }}"));
    // Another mail, worded in French only: an English speaker gets the
    // built-in wording, not the French one.
    cmd.save_mail_template(
        &alice,
        &tenant.id,
        ShopMailKind::ReviewRequest.key(),
        "fr",
        wording("Votre avis", "<p>{{ link }}</p>", None),
    )
    .await
    .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert!(
        state
            .mail_template_for(ShopMailKind::ReviewRequest, Some("fr"))
            .is_some()
    );
    assert!(
        state
            .mail_template_for(ShopMailKind::ReviewRequest, Some("en"))
            .is_none()
    );
    assert!(
        state
            .mail_template_for(ShopMailKind::ReviewRequest, None)
            .is_none()
    );

    // Each language has its own history, and its own reset.
    cmd.reset_mail_template(&alice, &tenant.id, kind.key(), "fr")
        .await
        .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert!(state.mail_template_in(kind, "fr").is_none());
    assert!(state.mail_template_in(kind, "en").is_some());
    let changes = |language: &str| -> Vec<bool> {
        state
            .mail_template_changes(kind, language)
            .map(|change| change.reset)
            .collect()
    };
    assert_eq!(changes("fr"), [true, false]);
    assert_eq!(changes("en"), [false]);

    // A language dropped keeps its wording, out of use, until picked again.
    cmd.save_mail_template(
        &alice,
        &tenant.id,
        kind.key(),
        "fr",
        wording("Code", "<p>{{ code }}</p>", None),
    )
    .await
    .unwrap();
    cmd.set_mail_languages(&alice, &tenant.id, languages("en", &[]))
        .await
        .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert!(state.mail_template_in(kind, "fr").is_none());
    assert_eq!(
        state
            .mail_template_for(kind, Some("fr"))
            .map(|template| template.subject.as_str()),
        Some("Code for {{ shop.name }}")
    );
    cmd.set_mail_languages(&alice, &tenant.id, languages("en", &["fr"]))
        .await
        .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert_eq!(state.mail_template_in(kind, "fr").unwrap().subject, "Code");
}
