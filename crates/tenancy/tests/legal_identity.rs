mod common;

use common::*;
use timada_tenancy::{LegalIdentity, TenancyError};

fn identity() -> LegalIdentity {
    LegalIdentity {
        legal_name: " Chatounette SAS ".to_owned(),
        legal_form: "SAS".to_owned(),
        share_capital_cents: Some(1_000_000),
        siret: "120 027 016 00092".to_owned(),
        rcs_city: Some("Paris".to_owned()),
        line1: "1 rue de la Paix".to_owned(),
        line2: Some(" ".to_owned()),
        postal_code: "75002".to_owned(),
        city: "Paris".to_owned(),
        country: "fr".to_owned(),
        contact_email: "Hello@Example.com".to_owned(),
        vat_number: Some("fr85 120027016".to_owned()),
        oss_registered: true,
    }
}

#[tokio::test]
async fn a_manager_sets_the_legal_identity_normalized() {
    let cmd = command().await;
    let alice = new_id();
    let tenant = create_tenant(&cmd, "Acme", &alice).await;

    cmd.set_legal_identity(&alice, &tenant.id, identity())
        .await
        .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    let legal = state.legal.clone().unwrap();
    assert_eq!(legal.legal_name, "Chatounette SAS");
    assert_eq!(legal.siret, "12002701600092");
    assert_eq!(legal.siren(), "120027016");
    assert_eq!(legal.line2, None);
    assert_eq!(legal.country, "FR");
    assert_eq!(legal.contact_email, "hello@example.com");
    assert_eq!(legal.vat_number.as_deref(), Some("FR85120027016"));
    assert!(legal.is_vat_registered());

    // The same identity is a no-op.
    cmd.set_legal_identity(&alice, &tenant.id, identity())
        .await
        .unwrap();
    assert_eq!(cmd.load_tenant(&tenant.id).await.unwrap().unwrap(), state);

    // The exemption: no number, so no One-Stop Shop either.
    let franchise = LegalIdentity {
        vat_number: None,
        oss_registered: false,
        ..identity()
    };
    cmd.set_legal_identity(&alice, &tenant.id, franchise)
        .await
        .unwrap();
    let legal = cmd
        .load_tenant(&tenant.id)
        .await
        .unwrap()
        .unwrap()
        .legal
        .unwrap();
    assert_eq!(legal.vat_number, None);
    assert!(!legal.oss_registered);
}

#[tokio::test]
async fn a_wrong_legal_identity_is_refused() {
    let cmd = command().await;
    let alice = new_id();
    let tenant = create_tenant(&cmd, "Acme", &alice).await;

    let cases = [
        (
            LegalIdentity {
                siret: "12002701600093".to_owned(),
                ..identity()
            },
            "siret",
        ),
        (
            LegalIdentity {
                vat_number: Some("FR86120027016".to_owned()),
                ..identity()
            },
            "vat",
        ),
        (
            LegalIdentity {
                vat_number: None,
                ..identity()
            },
            "oss",
        ),
        (
            LegalIdentity {
                legal_name: " ".to_owned(),
                ..identity()
            },
            "identity",
        ),
    ];
    for (identity, kind) in cases {
        let err = cmd
            .set_legal_identity(&alice, &tenant.id, identity)
            .await
            .unwrap_err();
        let expected = match kind {
            "siret" => matches!(err, TenancyError::InvalidSiret),
            "vat" => matches!(err, TenancyError::InvalidVatNumber),
            "oss" => matches!(err, TenancyError::OssWithoutVatNumber),
            _ => matches!(err, TenancyError::InvalidLegalIdentity),
        };
        assert!(expected, "{kind}: {err}");
    }

    let err = cmd
        .set_legal_identity(&new_id(), &tenant.id, identity())
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Forbidden), "{err}");
}
