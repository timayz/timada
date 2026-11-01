mod common;

use common::*;
use timada_tenancy::{LegalIdentity, TenancyError, companies_of_user, tenants_of_company};

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
async fn a_company_holds_the_identity_and_its_siren_is_unique() {
    let cmd = command().await;
    let alice = new_id();
    let company = cmd.create_company(&alice, identity()).await.unwrap();
    assert_eq!(company.siren, "120027016");
    assert!(company.is_owner(&alice));
    let legal = company.identity.clone().unwrap();
    assert_eq!(legal.legal_name, "Chatounette SAS");
    assert_eq!(legal.vat_number.as_deref(), Some("FR85120027016"));
    assert_eq!(legal.country, "FR");

    // Someone else with the same SIREN is turned away…
    let mallory = new_id();
    let err = cmd.create_company(&mallory, identity()).await.unwrap_err();
    assert!(matches!(err, TenancyError::SirenTaken), "{err}");
    // …until an administrator releases it.
    cmd.release_siren("admin", "120027016").await.unwrap();
    let theirs = cmd.create_company(&mallory, identity()).await.unwrap();
    assert_ne!(theirs.id, company.id);

    // Only owners edit; the SIRET keeps the SIREN.
    let err = cmd
        .set_company_identity(&mallory, &company.id, identity())
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::NotCompanyOwner), "{err}");
    let err = cmd
        .set_company_identity(
            &alice,
            &company.id,
            LegalIdentity {
                siret: "73282932000074".to_owned(),
                vat_number: None,
                oss_registered: false,
                ..identity()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::SirenMismatch), "{err}");
    cmd.set_company_identity(
        &alice,
        &company.id,
        LegalIdentity {
            legal_name: "Chatounette".to_owned(),
            ..identity()
        },
    )
    .await
    .unwrap();
    let renamed = cmd.load_company(&company.id).await.unwrap().unwrap();
    assert_eq!(renamed.name(), "Chatounette");

    // A wrong identity is refused before anything is written.
    let err = cmd
        .create_company(
            &new_id(),
            LegalIdentity {
                vat_number: None,
                ..identity()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::OssWithoutVatNumber), "{err}");
}

#[tokio::test]
async fn owners_come_and_go_but_one_stays() {
    let cmd = command().await;
    let alice = new_id();
    let bob = new_id();
    let company = cmd.create_company(&alice, identity()).await.unwrap();
    cmd.add_company_owner(&alice, &company.id, &bob)
        .await
        .unwrap();
    cmd.remove_company_owner(&bob, &company.id, &alice)
        .await
        .unwrap();
    let err = cmd
        .remove_company_owner(&bob, &company.id, &bob)
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::LastCompanyOwner), "{err}");
    let err = cmd
        .add_company_owner(&alice, &company.id, &new_id())
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::NotCompanyOwner), "{err}");
}

#[tokio::test]
async fn tenants_link_with_a_series_code_of_their_own() {
    let (cmd, pool) = command_with_pool().await;
    let alice = new_id();
    let company = cmd.create_company(&alice, identity()).await.unwrap();
    let shop = create_tenant(&cmd, "Shop", &alice).await;
    let pro = create_tenant(&cmd, "Pro", &alice).await;

    cmd.link_tenant(&alice, &company.id, &shop.id, None)
        .await
        .unwrap();
    let linked = cmd.load_tenant(&shop.id).await.unwrap().unwrap();
    let link = linked.company.clone().unwrap();
    assert_eq!(link.company_id, company.id);
    assert_eq!(link.series_code, "");
    assert!(cmd.load_company_of(&linked).await.unwrap().is_some());

    // The plain series is the first shop's: the second needs a code.
    let err = cmd
        .link_tenant(&alice, &company.id, &pro.id, Some(""))
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::SeriesCodeTaken), "{err}");
    let err = cmd
        .link_tenant(&alice, &company.id, &pro.id, Some("pro-1"))
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::InvalidSeriesCode), "{err}");
    cmd.link_tenant(&alice, &company.id, &pro.id, Some(" pro "))
        .await
        .unwrap();
    let pro_state = cmd.load_tenant(&pro.id).await.unwrap().unwrap();
    assert_eq!(pro_state.company.unwrap().series_code, "PRO");

    // A code is never freed: unlinked, PRO stays Pro's.
    cmd.unlink_tenant(&alice, &pro.id).await.unwrap();
    assert!(
        cmd.load_tenant(&pro.id)
            .await
            .unwrap()
            .unwrap()
            .company
            .is_none()
    );
    let err = cmd
        .link_tenant(&alice, &company.id, &shop.id, Some("PRO"))
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::SeriesCodeTaken), "{err}");
    cmd.link_tenant(&alice, &company.id, &pro.id, Some("PRO"))
        .await
        .unwrap();

    // Linking needs both the company and the tenant.
    let bob = new_id();
    let bobs = create_tenant(&cmd, "Bob's", &bob).await;
    let err = cmd
        .link_tenant(&bob, &company.id, &bobs.id, Some("BOB"))
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::NotCompanyOwner), "{err}");
    cmd.add_company_owner(&alice, &company.id, &bob)
        .await
        .unwrap();
    let err = cmd
        .link_tenant(&alice, &company.id, &bobs.id, Some("BOB"))
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Forbidden), "{err}");
    cmd.link_tenant(&bob, &company.id, &bobs.id, Some("BOB"))
        .await
        .unwrap();

    timada_tenancy::read_model_subscription::<evento::Turso>(pool.clone())
        .run_once(&cmd.0)
        .await
        .unwrap();
    let mine = companies_of_user(&pool, &alice).await.unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].legal_name, "Chatounette SAS");
    let codes: Vec<(String, String)> = tenants_of_company(&pool, &company.id)
        .await
        .unwrap()
        .into_iter()
        .map(|row| (row.name, row.series_code))
        .collect();
    assert_eq!(
        codes,
        [
            ("Bob's".to_owned(), "BOB".to_owned()),
            ("Pro".to_owned(), "PRO".to_owned()),
            ("Shop".to_owned(), String::new()),
        ]
    );
}
