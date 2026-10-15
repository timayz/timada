use evento::migrator::Migrate;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use timada_orders::ShippingAddress;
use timada_shoppers::{
    CODE_MAX_ATTEMPTS, CODE_RESEND_AFTER_SECS, CODE_TTL_SECS, Command, MAX_ADDRESSES,
    SESSION_TTL_SECS, ShopperError,
};

const NOW: u64 = 1_700_000_000;
const EMAIL: &str = "Ada@Example.com";

type Shoppers = Command<evento::Sqlite>;

async fn shoppers() -> Shoppers {
    let options = SqliteConnectOptions::new()
        .filename(":memory:")
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let migrator = evento::sql_migrator::new::<sqlx::Sqlite>().unwrap();
    migrator
        .run(&mut *conn, &evento::migrator::Plan::apply_all())
        .await
        .unwrap();
    drop(conn);
    Command(pool.into())
}

async fn signed_in(shoppers: &Shoppers, email: &str, now: u64) -> timada_shoppers::SignedIn {
    let issued = shoppers.issue_code(email, now).await.unwrap();
    shoppers.sign_in(email, &issued.code, now).await.unwrap()
}

fn address(city: &str) -> ShippingAddress {
    ShippingAddress {
        name: " Ada Lovelace ".to_owned(),
        line1: "1 rue de la Paix".to_owned(),
        line2: Some(" ".to_owned()),
        postal_code: "75002".to_owned(),
        city: city.to_owned(),
        country: "fr".to_owned(),
        phone: None,
    }
}

#[tokio::test]
async fn the_first_sign_in_creates_the_shopper_and_the_next_reuses_it() {
    let shoppers = shoppers().await;
    let first = signed_in(&shoppers, EMAIL, NOW).await;
    let second = signed_in(&shoppers, "ada@example.com", NOW + CODE_RESEND_AFTER_SECS).await;
    assert_eq!(first.shopper_id, second.shopper_id);
    assert_ne!(first.token, second.token);
    assert_eq!(first.expires_at, NOW + SESSION_TTL_SECS);

    let resolved = shoppers
        .resolve_session(&first.token, NOW + 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resolved.shopper.email, "ada@example.com");
    assert_eq!(resolved.shopper.id, first.shopper_id);
}

#[tokio::test]
async fn a_code_is_not_resent_too_soon_and_works_once() {
    let shoppers = shoppers().await;
    let issued = shoppers.issue_code(EMAIL, NOW).await.unwrap();
    assert_eq!(issued.email, "ada@example.com");
    assert!(matches!(
        shoppers.issue_code(EMAIL, NOW + 10).await,
        Err(ShopperError::CodeRecentlySent)
    ));
    shoppers
        .sign_in(EMAIL, &issued.code, NOW + 10)
        .await
        .unwrap();
    assert!(matches!(
        shoppers.sign_in(EMAIL, &issued.code, NOW + 11).await,
        Err(ShopperError::WrongCode)
    ));
    // A used code does not hold back the next one.
    shoppers.issue_code(EMAIL, NOW + 12).await.unwrap();
}

#[tokio::test]
async fn wrong_guesses_kill_the_code() {
    let shoppers = shoppers().await;
    let issued = shoppers.issue_code(EMAIL, NOW).await.unwrap();
    let wrong = if issued.code == "000000" {
        "000001"
    } else {
        "000000"
    };
    for attempt in 1..=CODE_MAX_ATTEMPTS {
        let err = shoppers.sign_in(EMAIL, wrong, NOW).await.unwrap_err();
        if attempt < CODE_MAX_ATTEMPTS {
            assert!(matches!(err, ShopperError::WrongCode), "{err:?}");
        } else {
            assert!(matches!(err, ShopperError::TooManyAttempts), "{err:?}");
        }
    }
    assert!(matches!(
        shoppers.sign_in(EMAIL, &issued.code, NOW).await,
        Err(ShopperError::TooManyAttempts)
    ));
}

#[tokio::test]
async fn codes_and_sessions_expire() {
    let shoppers = shoppers().await;
    let issued = shoppers.issue_code(EMAIL, NOW).await.unwrap();
    assert!(matches!(
        shoppers
            .sign_in(EMAIL, &issued.code, NOW + CODE_TTL_SECS)
            .await,
        Err(ShopperError::CodeExpired)
    ));
    assert!(matches!(
        shoppers.issue_code("not an address", NOW).await,
        Err(ShopperError::InvalidEmail)
    ));

    let session = signed_in(&shoppers, "bob@example.com", NOW).await;
    let at = |offset| shoppers.resolve_session(&session.token, NOW + offset);
    assert!(at(SESSION_TTL_SECS - 1).await.unwrap().is_some());
    assert!(at(SESSION_TTL_SECS).await.unwrap().is_none());
    assert!(
        shoppers
            .resolve_session("unknown", NOW)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn signing_out_ends_only_that_session() {
    let shoppers = shoppers().await;
    let one = signed_in(&shoppers, EMAIL, NOW).await;
    let two = signed_in(&shoppers, EMAIL, NOW + CODE_RESEND_AFTER_SECS).await;
    shoppers.sign_out(&one.token).await.unwrap();
    shoppers.sign_out(&one.token).await.unwrap();
    shoppers.sign_out("unknown").await.unwrap();
    assert!(
        shoppers
            .resolve_session(&one.token, NOW + 100)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        shoppers
            .resolve_session(&two.token, NOW + 100)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn addresses_are_saved_once_newest_first_and_capped() {
    let shoppers = shoppers().await;
    let shopper = signed_in(&shoppers, EMAIL, NOW).await.shopper_id;
    let paris = shoppers
        .save_address(&shopper, address("Paris"))
        .await
        .unwrap();
    let lyon = shoppers
        .save_address(&shopper, address("Lyon"))
        .await
        .unwrap();
    assert_eq!(
        shoppers
            .save_address(&shopper, address("Paris"))
            .await
            .unwrap(),
        paris
    );
    let state = shoppers.load_shopper(&shopper).await.unwrap().unwrap();
    let ids: Vec<_> = state.addresses.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, [paris.as_str(), lyon.as_str()]);
    let saved = &state.addresses[0].address;
    assert_eq!(saved.name, "Ada Lovelace");
    assert_eq!(saved.line2, None);
    assert_eq!(saved.country, "FR");

    let mut bad = address("Paris");
    bad.country = "France".to_owned();
    let err = shoppers.save_address(&shopper, bad).await.unwrap_err();
    assert_eq!(err.code(), Some("invalid_country"));

    shoppers.remove_address(&shopper, &lyon).await.unwrap();
    shoppers.remove_address(&shopper, &lyon).await.unwrap();
    for n in 1..MAX_ADDRESSES {
        shoppers
            .save_address(&shopper, address(&format!("City {n}")))
            .await
            .unwrap();
    }
    assert!(matches!(
        shoppers
            .save_address(&shopper, address("One too many"))
            .await,
        Err(ShopperError::TooManyAddresses)
    ));
    // One already kept still moves first.
    shoppers
        .save_address(&shopper, address("Paris"))
        .await
        .unwrap();
    let state = shoppers.load_shopper(&shopper).await.unwrap().unwrap();
    assert_eq!(state.addresses.len(), MAX_ADDRESSES);
    assert_eq!(state.addresses[0].id, paris);
}
