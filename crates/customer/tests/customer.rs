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
