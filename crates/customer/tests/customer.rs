use timada_core::{Address, Civility};
use timada_customer::{
    Command, CustomerError, ListCustomers, RegisterCustomer, count_customers,
    customer_list_subscription, list_customers, load_address_book, migrations,
};

fn jonathan() -> RegisterCustomer {
    RegisterCustomer {
        email: "Jonathan@Example.com".into(),
        civility: Civility::Mr,
        first_name: "Jonathan".into(),
        last_name: "Lapiquonne".into(),
    }
}

fn gwada() -> Address {
    Address {
        civility: Civility::Mr,
        first_name: "Jonathan".into(),
        last_name: "Lapiquonne".into(),
        line1: "La roso".into(),
        line2: None,
        postal_code: "97330".into(),
        city: "Saint ane".into(),
        country_code: "GP".into(),
        phone: Some("0596843446".into()),
        mobile: Some("0562128368".into()),
    }
}

fn ramonville() -> Address {
    Address {
        civility: Civility::Mr,
        first_name: "Jonathan".into(),
        last_name: "Lapiquonne".into(),
        line1: "121, Avenue Tolosane".into(),
        line2: Some("Apt A21".into()),
        postal_code: "31520".into(),
        city: "Ramonville-Saint-Agne".into(),
        country_code: "FR".into(),
        phone: Some("0721358738".into()),
        mobile: None,
    }
}

#[tokio::test]
async fn address_book_follows_preferred_rules() -> anyhow::Result<()> {
    let (executor, _db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command(&executor);

    let id = cmd.register_customer(jonathan()).await?;
    let first = cmd.add_delivery_address(&id, gwada()).await?;
    let second = cmd.add_delivery_address(&id, ramonville()).await?;
    assert_ne!(first, second);

    let view = load_address_book(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("address book missing"))?;
    assert_eq!(view.email, "jonathan@example.com");
    assert_eq!(view.deliveries.len(), 2);
    assert_eq!(
        view.preferred_delivery().map(|d| d.id.as_str()),
        Some(first.as_str())
    );

    let blocked = cmd.remove_delivery_address(&id, first.clone()).await;
    assert!(matches!(blocked, Err(CustomerError::CannotRemovePreferred)));

    cmd.choose_preferred_delivery_address(&id, second.clone())
        .await?;
    cmd.remove_delivery_address(&id, first.clone()).await?;

    let view = load_address_book(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("address book missing"))?;
    assert_eq!(view.deliveries.len(), 1);
    assert_eq!(view.deliveries[0].id, second);
    assert!(view.deliveries[0].preferred);
    assert_eq!(view.deliveries[0].address.city, "Ramonville-Saint-Agne");

    // Removing the last address is allowed and clears the preference.
    cmd.remove_delivery_address(&id, second.clone()).await?;
    let next = cmd.add_delivery_address(&id, gwada()).await?;
    let view = load_address_book(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("address book missing"))?;
    assert_eq!(
        view.preferred_delivery().map(|d| d.id.as_str()),
        Some(next.as_str())
    );

    let unknown = cmd
        .change_delivery_address(&id, "nope".into(), ramonville())
        .await;
    assert!(matches!(unknown, Err(CustomerError::AddressNotFound)));

    Ok(())
}

#[tokio::test]
async fn billing_address_and_validation() -> anyhow::Result<()> {
    let (executor, _db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command(&executor);

    let bad_email = cmd
        .register_customer(RegisterCustomer {
            email: "not-an-email".into(),
            ..jonathan()
        })
        .await;
    assert!(matches!(bad_email, Err(CustomerError::InvalidEmail(_))));

    let id = cmd.register_customer(jonathan()).await?;
    cmd.set_billing_address(&id, ramonville()).await?;

    let invalid = cmd
        .add_delivery_address(
            &id,
            Address {
                city: String::new(),
                ..gwada()
            },
        )
        .await;
    assert!(matches!(invalid, Err(CustomerError::Address(_))));

    let view = load_address_book(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("address book missing"))?;
    assert_eq!(view.billing.map(|a| a.postal_code), Some("31520".into()));
    assert!(view.deliveries.is_empty());

    Ok(())
}

#[tokio::test]
async fn customer_list_follows_registrations_and_email_changes() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command(&executor);

    let first = cmd.register_customer(jonathan()).await?;
    // Ids are ULIDs: within one millisecond their order is random.
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    let second = cmd
        .register_customer(RegisterCustomer {
            email: "marie@example.com".into(),
            civility: Civility::Mrs,
            first_name: "Marie".into(),
            last_name: "Curie".into(),
        })
        .await?;
    cmd.change_email(&first, "jonathan.l@example.com".into())
        .await?;

    customer_list_subscription()
        .data(db.clone())
        .run_once(&executor)
        .await?;

    let all = list_customers(&db, &ListCustomers::default()).await?;
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].customer_id, second, "newest registration first");
    assert_eq!(count_customers(&db, None).await?, 2);

    let filtered = list_customers(
        &db,
        &ListCustomers {
            q: Some("jonathan".into()),
            ..ListCustomers::default()
        },
    )
    .await?;
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].customer_id, first);
    assert_eq!(filtered[0].email, "jonathan.l@example.com");
    assert_eq!(count_customers(&db, Some("jonathan")).await?, 1);
    assert_eq!(count_customers(&db, Some("nobody")).await?, 0);

    Ok(())
}

#[tokio::test]
async fn a_guest_orders_without_an_account_and_may_open_one_later() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command(&executor);

    // The same address as an account's: a guest claims nothing.
    let member = cmd.register_customer(jonathan()).await?;
    let guest = cmd.register_guest(jonathan()).await?;
    assert_ne!(member, guest);
    assert!(matches!(
        cmd.register_guest(RegisterCustomer {
            email: "nowhere".into(),
            ..jonathan()
        })
        .await,
        Err(CustomerError::InvalidEmail(_))
    ));

    let book = |id: String| {
        let executor = &executor;
        async move {
            load_address_book(executor, id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("no address book"))
        }
    };
    assert!(book(guest.clone()).await?.guest);
    assert!(!book(member.clone()).await?.guest);
    // A guest is a customer: delivered and invoiced like any other.
    cmd.add_delivery_address(&guest, gwada()).await?;
    assert_eq!(book(guest.clone()).await?.deliveries.len(), 1);

    customer_list_subscription()
        .data(db.clone())
        .run_once(&executor)
        .await?;
    let guests = |rows: Vec<timada_customer::CustomerListRow>| {
        rows.into_iter()
            .filter(|row| row.guest)
            .map(|row| row.customer_id)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        guests(list_customers(&db, &ListCustomers::default()).await?),
        std::slice::from_ref(&guest)
    );

    // An account is opened once; an account holder has nothing to open.
    assert!(cmd.open_account(&guest).await?);
    assert!(!cmd.open_account(&guest).await?);
    assert!(!cmd.open_account(&member).await?);
    assert!(matches!(
        cmd.open_account("nobody").await,
        Err(CustomerError::CustomerNotFound)
    ));
    assert!(!book(guest.clone()).await?.guest);
    customer_list_subscription()
        .data(db.clone())
        .run_once(&executor)
        .await?;
    assert!(guests(list_customers(&db, &ListCustomers::default()).await?).is_empty());
    Ok(())
}

#[tokio::test]
async fn a_business_is_identified_and_its_vat_number_checked() -> anyhow::Result<()> {
    use timada_customer::{CustomerError, load_company_identity};
    use timada_tax::{FakeValidator, VatNumber};

    let (executor, _db) = timada_core::testing::memory_executor(vec![]).await?;
    let cmd = Command(&executor);
    let id = cmd
        .register_customer(RegisterCustomer {
            email: "achats@example.de".into(),
            civility: Civility::Mrs,
            first_name: "Grete".into(),
            last_name: "Hermann".into(),
        })
        .await?;
    let registry = FakeValidator::default();
    let number = VatNumber::parse("DE 123 456 789")?;
    let view = || async {
        load_company_identity(&executor, &id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("customer missing"))
    };

    // A consumer: nothing to check.
    assert!(!view().await?.is_company());
    assert!(
        cmd.check_company_vat_number(&id, &registry)
            .await?
            .is_none()
    );
    assert!(matches!(
        cmd.record_vat_check(
            &id,
            &number,
            &timada_tax::VatCheck {
                valid: true,
                consultation_ref: None,
                registered_name: None
            }
        )
        .await,
        Err(CustomerError::NoCompanyIdentity)
    ));

    assert!(matches!(
        cmd.identify_company(&id, "  ", &number).await,
        Err(CustomerError::Required("company_name"))
    ));
    cmd.identify_company(&id, " Hermann GmbH ", &number).await?;
    // Said again: nothing recorded.
    cmd.identify_company(&id, "Hermann GmbH", &number).await?;
    let company = view().await?;
    assert_eq!(
        (company.company_name.as_str(), company.vat_number.as_str()),
        ("Hermann GmbH", "DE123456789")
    );
    let now = timada_core::time::now_unix_secs()?;
    assert!(
        company.standing_check(now, 3_600).is_none(),
        "not checked yet"
    );

    // Valid: the proof is kept, and stands while it is recent.
    let answer = cmd.check_company_vat_number(&id, &registry).await?;
    assert!(answer.is_some_and(|a| a.is_ok_and(|check| check.valid)));
    let company = view().await?;
    let standing = company
        .standing_check(now + 60, 3_600)
        .ok_or_else(|| anyhow::anyhow!("no standing check"))?;
    assert!(
        standing
            .consultation_ref
            .as_deref()
            .is_some_and(|reference| reference.starts_with("FAKE-DE123456789"))
    );
    assert!(
        company.standing_check(now + 7_200, 3_600).is_none(),
        "too old"
    );

    // The registry is down: nothing is recorded, the last answer stands.
    registry.set_down(true);
    let answer = cmd.check_company_vat_number(&id, &registry).await?;
    assert!(answer.is_some_and(|a| a.is_err()));
    assert_eq!(view().await?, company);
    registry.set_down(false);

    // Struck off the registry: no exemption any more, however recent the
    // valid answer before.
    registry.reject(&number);
    cmd.check_company_vat_number(&id, &registry).await?;
    let company = view().await?;
    assert!(
        company
            .last_check
            .as_ref()
            .is_some_and(|check| !check.valid)
    );
    assert!(company.standing_check(now + 60, 3_600).is_none());

    // Another number starts unchecked; an answer about the old one is refused.
    let other = VatNumber::parse("ATU12345678")?;
    cmd.identify_company(&id, "Hermann GmbH", &other).await?;
    let company = view().await?;
    assert_eq!(company.vat_number, "ATU12345678");
    assert_eq!(company.last_check, None);
    let stale = timada_tax::VatCheck {
        valid: true,
        consultation_ref: None,
        registered_name: None,
    };
    assert!(matches!(
        cmd.record_vat_check(&id, &number, &stale).await,
        Err(CustomerError::VatNumberMismatch)
    ));

    // A consumer again; twice is harmless, and the address book never noticed.
    cmd.remove_company_identity(&id).await?;
    cmd.remove_company_identity(&id).await?;
    assert!(!view().await?.is_company());
    assert!(
        timada_customer::load_address_book(&executor, &id)
            .await?
            .is_some()
    );
    Ok(())
}
