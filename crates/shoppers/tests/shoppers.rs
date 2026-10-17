mod common;

use std::ops::Deref;

use common::MemoryKeys;
use evento::migrator::Migrate;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use timada_orders::ShippingAddress;
use timada_shoppers::{
    CODE_MAX_ATTEMPTS, CODE_RESEND_AFTER_SECS, CODE_TTL_SECS, Command, MAX_ADDRESSES,
    SESSION_TTL_SECS, Shopper, ShopperError, ShopperKeys, ShopperState,
};

const NOW: u64 = 1_700_000_000;
const EMAIL: &str = "Ada@Example.com";

/// The commands over one in-memory store, with the keys they keep.
struct Shoppers {
    command: Command<evento::Sqlite>,
    keys: MemoryKeys,
    pool: SqlitePool,
}

impl Deref for Shoppers {
    type Target = Command<evento::Sqlite>;

    fn deref(&self) -> &Self::Target {
        &self.command
    }
}

impl Shoppers {
    async fn open(&self, state: &ShopperState) -> Option<Shopper> {
        state.open_with(&self.keys).await.unwrap()
    }

    /// Every event's payload, as stored.
    async fn raw_events(&self) -> Vec<Vec<u8>> {
        sqlx::query_scalar("SELECT data FROM event")
            .fetch_all(&self.pool)
            .await
            .unwrap()
    }
}

async fn shoppers() -> Shoppers {
    shoppers_with(MemoryKeys::default()).await
}

async fn shoppers_with(keys: MemoryKeys) -> Shoppers {
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
    Shoppers {
        command: Command(pool.clone().into()),
        keys,
        pool,
    }
}

async fn signed_in(shoppers: &Shoppers, email: &str, now: u64) -> timada_shoppers::SignedIn {
    let issued = shoppers.issue_code(email, now).await.unwrap();
    shoppers
        .sign_in(&shoppers.keys, email, &issued.code, now)
        .await
        .unwrap()
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
    let shopper = shoppers.open(&resolved.shopper).await.unwrap();
    assert_eq!(shopper.email, "ada@example.com");
    assert_eq!(shopper.id, first.shopper_id);
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
        .sign_in(&shoppers.keys, EMAIL, &issued.code, NOW + 10)
        .await
        .unwrap();
    assert!(matches!(
        shoppers
            .sign_in(&shoppers.keys, EMAIL, &issued.code, NOW + 11)
            .await,
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
        let err = shoppers
            .sign_in(&shoppers.keys, EMAIL, wrong, NOW)
            .await
            .unwrap_err();
        if attempt < CODE_MAX_ATTEMPTS {
            assert!(matches!(err, ShopperError::WrongCode), "{err:?}");
        } else {
            assert!(matches!(err, ShopperError::TooManyAttempts), "{err:?}");
        }
    }
    assert!(matches!(
        shoppers
            .sign_in(&shoppers.keys, EMAIL, &issued.code, NOW)
            .await,
        Err(ShopperError::TooManyAttempts)
    ));
}

#[tokio::test]
async fn codes_and_sessions_expire() {
    let shoppers = shoppers().await;
    let issued = shoppers.issue_code(EMAIL, NOW).await.unwrap();
    assert!(matches!(
        shoppers
            .sign_in(&shoppers.keys, EMAIL, &issued.code, NOW + CODE_TTL_SECS)
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
        .save_address(&shoppers.keys, &shopper, address("Paris"))
        .await
        .unwrap();
    let lyon = shoppers
        .save_address(&shoppers.keys, &shopper, address("Lyon"))
        .await
        .unwrap();
    assert_eq!(
        shoppers
            .save_address(&shoppers.keys, &shopper, address("Paris"))
            .await
            .unwrap(),
        paris
    );
    let state = shoppers.load_shopper(&shopper).await.unwrap().unwrap();
    let state = shoppers.open(&state).await.unwrap();
    let ids: Vec<_> = state.addresses.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, [paris.as_str(), lyon.as_str()]);
    let saved = &state.addresses[0].address;
    assert_eq!(saved.name, "Ada Lovelace");
    assert_eq!(saved.line2, None);
    assert_eq!(saved.country, "FR");

    let mut bad = address("Paris");
    bad.country = "France".to_owned();
    let err = shoppers
        .save_address(&shoppers.keys, &shopper, bad)
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("invalid_country"));

    shoppers.remove_address(&shopper, &lyon).await.unwrap();
    shoppers.remove_address(&shopper, &lyon).await.unwrap();
    for n in 1..MAX_ADDRESSES {
        shoppers
            .save_address(&shoppers.keys, &shopper, address(&format!("City {n}")))
            .await
            .unwrap();
    }
    assert!(matches!(
        shoppers
            .save_address(&shoppers.keys, &shopper, address("One too many"))
            .await,
        Err(ShopperError::TooManyAddresses)
    ));
    // One already kept still moves first.
    shoppers
        .save_address(&shoppers.keys, &shopper, address("Paris"))
        .await
        .unwrap();
    let state = shoppers.load_shopper(&shopper).await.unwrap().unwrap();
    assert_eq!(state.addresses.len(), MAX_ADDRESSES);
    assert_eq!(state.addresses[0].id, paris);
}

/// Revocations compare event times in milliseconds: a sign-in in the same
/// millisecond as one would fall under it.
async fn next_millisecond() {
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
}

#[tokio::test]
async fn signing_out_everywhere_else_keeps_only_the_asking_session() {
    let shoppers = shoppers().await;
    let here = signed_in(&shoppers, EMAIL, NOW).await;
    let elsewhere = signed_in(&shoppers, EMAIL, NOW + CODE_RESEND_AFTER_SECS).await;
    let other_shopper = signed_in(&shoppers, "bob@example.com", NOW).await;
    shoppers
        .revoke_other_sessions(&here.shopper_id, &here.token)
        .await
        .unwrap();

    let resolves = async |token: &str| {
        shoppers
            .resolve_session(token, NOW + 200)
            .await
            .unwrap()
            .is_some()
    };
    assert!(resolves(&here.token).await);
    assert!(!resolves(&elsewhere.token).await);
    assert!(resolves(&other_shopper.token).await, "only that shopper's");

    next_millisecond().await;
    let later = signed_in(&shoppers, EMAIL, NOW + 2 * CODE_RESEND_AFTER_SECS).await;
    assert!(resolves(&later.token).await);
}

#[tokio::test]
async fn forgetting_a_shopper_ends_every_session_and_the_next_sign_in_starts_afresh() {
    let shoppers = shoppers().await;
    let one = signed_in(&shoppers, EMAIL, NOW).await;
    let two = signed_in(&shoppers, EMAIL, NOW + CODE_RESEND_AFTER_SECS).await;
    shoppers
        .save_address(&shoppers.keys, &one.shopper_id, address("Paris"))
        .await
        .unwrap();
    let first = shoppers
        .load_shopper(&one.shopper_id)
        .await
        .unwrap()
        .unwrap();
    let first_key = first.key_id.clone();
    let key = shoppers.keys.get(&first_key).await.unwrap().unwrap();

    shoppers
        .forget(&shoppers.keys, &one.shopper_id)
        .await
        .unwrap();
    shoppers
        .forget(&shoppers.keys, &one.shopper_id)
        .await
        .unwrap();
    shoppers.forget(&shoppers.keys, "unknown").await.unwrap();
    for token in [&one.token, &two.token] {
        assert!(
            shoppers
                .resolve_session(token, NOW + 200)
                .await
                .unwrap()
                .is_none()
        );
    }
    assert!(
        shoppers
            .load_shopper_by_email("ada@example.com")
            .await
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        shoppers
            .save_address(&shoppers.keys, &one.shopper_id, address("Lyon"))
            .await,
        Err(ShopperError::NotFound)
    ));

    assert!(!shoppers.keys.has(&first_key), "the key is destroyed");
    assert_eq!(shoppers.keys.len(), 0);
    // What the account recorded opened under its key only: with the key
    // gone, nothing does.
    assert!(first.open(&key).is_ok());
    assert!(shoppers.open(&first).await.is_none());

    next_millisecond().await;
    let again = signed_in(&shoppers, EMAIL, NOW + 2 * CODE_RESEND_AFTER_SECS).await;
    assert_eq!(again.shopper_id, one.shopper_id, "the address's stream");
    let resolved = shoppers
        .resolve_session(&again.token, NOW + 200)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(resolved.shopper.key_id, first_key, "a new key");
    let shopper = shoppers.open(&resolved.shopper).await.unwrap();
    assert_eq!(shopper.email, "ada@example.com");
    assert!(shopper.addresses.is_empty());
}

/// Bytes that hold `needle` anywhere.
fn holds(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

#[tokio::test]
async fn nothing_personal_is_recorded_in_clear() {
    let shoppers = shoppers().await;
    let shopper = signed_in(&shoppers, EMAIL, NOW).await.shopper_id;
    shoppers
        .save_address(&shoppers.keys, &shopper, address("Montpellier"))
        .await
        .unwrap();
    let events = shoppers.raw_events().await;
    assert!(!events.is_empty());
    for data in events {
        for personal in [
            "ada@example.com",
            "Ada@Example.com",
            "Montpellier",
            "rue de la Paix",
            "Lovelace",
        ] {
            assert!(!holds(&data, personal), "{personal} is recorded in clear");
        }
    }
}

#[tokio::test]
async fn address_ids_are_keyed_by_the_registration() {
    let shoppers = shoppers().await;
    let ada = signed_in(&shoppers, EMAIL, NOW).await.shopper_id;
    let bob = signed_in(&shoppers, "bob@example.com", NOW)
        .await
        .shopper_id;
    let save = async |shopper: &str| {
        shoppers
            .save_address(&shoppers.keys, shopper, address("Paris"))
            .await
            .unwrap()
    };
    let first = save(&ada).await;
    assert_eq!(save(&ada).await, first, "the same address, the same entry");
    assert_ne!(save(&bob).await, first, "another shopper, another id");

    shoppers.forget(&shoppers.keys, &ada).await.unwrap();
    next_millisecond().await;
    let ada = signed_in(&shoppers, EMAIL, NOW + CODE_RESEND_AFTER_SECS)
        .await
        .shopper_id;
    assert_ne!(save(&ada).await, first, "another registration, another id");
}

#[tokio::test]
async fn a_key_that_cannot_be_kept_registers_nothing() {
    let shoppers = shoppers_with(MemoryKeys::refusing()).await;
    let issued = shoppers.issue_code(EMAIL, NOW).await.unwrap();
    let err = shoppers
        .sign_in(&shoppers.keys, EMAIL, &issued.code, NOW)
        .await
        .unwrap_err();
    assert!(!err.is_user_facing(), "{err:?}");
    assert!(
        shoppers
            .load_shopper_by_email("ada@example.com")
            .await
            .unwrap()
            .is_none()
    );
}
