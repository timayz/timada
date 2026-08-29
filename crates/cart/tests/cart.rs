//! Cover for the cart's write side and its replayed view against a real
//! SQLite event store — mocking it here would prove nothing, least of all the
//! optimistic-concurrency chain these tests exist to guard.

use timada_cart::{AddItemError, add_item, load_cart, mark_checked_out, remove_item};
use timada_catalog::ProductView;
use timada_core::{Currency, Money, ServiceContext, new_id};

/// A temp-file database, torn down on drop.
struct TestDb {
    dir: std::path::PathBuf,
    ctx: ServiceContext,
}

impl TestDb {
    async fn new() -> anyhow::Result<Self> {
        let dir = std::env::temp_dir().join(format!("timada-cart-{}", new_id()));
        std::fs::create_dir_all(&dir)?;
        let url = format!("sqlite://{}?mode=rwc", dir.join("test.db").display());

        // One pool for both roles: these tests are single-threaded CLI-shaped
        // work, and the cart has no SQL read models to migrate.
        let pool = timada_core::db::create_pool(&url, 1).await?;
        let ctx = ServiceContext::new(pool.clone(), pool).await?;

        Ok(Self { dir, ctx })
    }

    async fn close(self) {
        self.ctx.read_pool.close().await;
        self.ctx.write_pool.close().await;
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A published product, built directly: `add_item` only ever reads a
/// `ProductView`, so importing through the catalog would test the catalog.
fn product(title: &str, price_cents: i64) -> ProductView {
    ProductView {
        id: new_id(),
        supplier_id: "mock".to_owned(),
        supplier_product_ref: format!("ref-{title}"),
        title: title.to_owned(),
        price_cents,
        currency: Currency::Eur,
        published: true,
        ..Default::default()
    }
}

#[tokio::test]
async fn adding_the_same_product_twice_merges_into_one_line() -> anyhow::Result<()> {
    let db = TestDb::new().await?;
    let cart_id = new_id();
    let mug = product("Mug", 1250);
    let socks = product("Socks", 799);

    add_item(&db.ctx.executor, &cart_id, &mug, mug.base_price(), 2).await?;
    add_item(&db.ctx.executor, &cart_id, &socks, socks.base_price(), 1).await?;
    add_item(&db.ctx.executor, &cart_id, &mug, mug.base_price(), 3).await?;

    let cart = load_cart(&db.ctx.executor, &cart_id)
        .await?
        .expect("cart exists after three adds");

    assert_eq!(cart.id, cart_id);
    assert_eq!(
        cart.lines.len(),
        2,
        "same product must not split into lines"
    );
    assert_eq!(cart.lines[0].product_id, mug.id);
    assert_eq!(cart.lines[0].quantity, 5);
    assert_eq!(cart.lines[0].supplier_id, "mock");
    assert_eq!(cart.lines[1].quantity, 1);
    assert_eq!(cart.item_count(), 6);

    // 5 × 12.50 + 1 × 7.99
    assert_eq!(cart.total(), Money::new(6250 + 799, Currency::Eur));
    assert_eq!(cart.lines[0].line_total(), Money::new(6250, Currency::Eur));

    db.close().await;
    Ok(())
}

#[tokio::test]
async fn add_rejects_bad_quantity_and_unavailable_products() -> anyhow::Result<()> {
    let db = TestDb::new().await?;
    let cart_id = new_id();
    let mug = product("Mug", 1250);

    let refused = add_item(&db.ctx.executor, &cart_id, &mug, mug.base_price(), 0).await;
    assert!(matches!(refused, Err(AddItemError::InvalidQuantity)));

    let mut draft = product("Draft", 100);
    draft.published = false;
    let refused = add_item(&db.ctx.executor, &cart_id, &draft, draft.base_price(), 1).await;
    assert!(matches!(refused, Err(AddItemError::ProductUnavailable)));

    let mut archived = product("Archived", 100);
    archived.archived = true;
    let refused = add_item(
        &db.ctx.executor,
        &cart_id,
        &archived,
        archived.base_price(),
        1,
    )
    .await;
    assert!(matches!(refused, Err(AddItemError::ProductUnavailable)));

    assert!(
        load_cart(&db.ctx.executor, &cart_id).await?.is_none(),
        "a refused add must not create the cart"
    );

    db.close().await;
    Ok(())
}

#[tokio::test]
async fn removing_drops_the_line_and_a_missing_line_is_a_no_op() -> anyhow::Result<()> {
    let db = TestDb::new().await?;
    let cart_id = new_id();
    let mug = product("Mug", 1250);
    let socks = product("Socks", 799);

    add_item(&db.ctx.executor, &cart_id, &mug, mug.base_price(), 2).await?;
    add_item(&db.ctx.executor, &cart_id, &socks, socks.base_price(), 1).await?;
    remove_item(&db.ctx.executor, &cart_id, &mug.id).await?;

    let cart = load_cart(&db.ctx.executor, &cart_id)
        .await?
        .expect("cart still exists");
    assert_eq!(cart.lines.len(), 1);
    assert_eq!(cart.lines[0].product_id, socks.id);
    let version_after_remove = cart.cursor.clone();

    // Removing what is not there, and removing from a cart that never
    // existed, both leave the world alone.
    remove_item(&db.ctx.executor, &cart_id, &mug.id).await?;
    remove_item(&db.ctx.executor, &new_id(), &mug.id).await?;

    let cart = load_cart(&db.ctx.executor, &cart_id)
        .await?
        .expect("cart still exists");
    assert_eq!(cart.lines.len(), 1);
    assert_eq!(
        cart.cursor, version_after_remove,
        "a no-op remove must not append an event"
    );

    db.close().await;
    Ok(())
}

#[tokio::test]
async fn checkout_closes_the_cart_and_is_idempotent() -> anyhow::Result<()> {
    let db = TestDb::new().await?;
    let cart_id = new_id();
    let mug = product("Mug", 1250);

    add_item(&db.ctx.executor, &cart_id, &mug, mug.base_price(), 1).await?;
    mark_checked_out(&db.ctx.executor, &cart_id).await?;

    let cart = load_cart(&db.ctx.executor, &cart_id)
        .await?
        .expect("cart exists");
    assert!(cart.checked_out);
    let version_after_checkout = cart.cursor.clone();

    mark_checked_out(&db.ctx.executor, &cart_id).await?;
    mark_checked_out(&db.ctx.executor, &new_id()).await?;

    let cart = load_cart(&db.ctx.executor, &cart_id)
        .await?
        .expect("cart exists");
    assert_eq!(
        cart.cursor, version_after_checkout,
        "checking out twice must not append a second event"
    );

    // A stale cookie must not reopen a cart that became an order.
    let refused = add_item(&db.ctx.executor, &cart_id, &mug, mug.base_price(), 1).await;
    assert!(matches!(refused, Err(AddItemError::CartCheckedOut)));
    // Removing from it is refused too, but silently — see `remove_item`.
    remove_item(&db.ctx.executor, &cart_id, &mug.id).await?;
    let cart = load_cart(&db.ctx.executor, &cart_id)
        .await?
        .expect("cart exists");
    assert_eq!(cart.lines.len(), 1);

    db.close().await;
    Ok(())
}

/// The whole point of the `original_version` handling: a cart written across
/// four commits replays to exactly what those commits said.
#[tokio::test]
async fn a_full_cart_lifecycle_replays_consistently() -> anyhow::Result<()> {
    let db = TestDb::new().await?;
    let cart_id = new_id();
    let mug = product("Mug", 1250);
    let socks = product("Socks", 799);

    add_item(&db.ctx.executor, &cart_id, &mug, mug.base_price(), 1).await?;
    add_item(&db.ctx.executor, &cart_id, &socks, socks.base_price(), 4).await?;
    remove_item(&db.ctx.executor, &cart_id, &mug.id).await?;
    mark_checked_out(&db.ctx.executor, &cart_id).await?;

    let cart = load_cart(&db.ctx.executor, &cart_id)
        .await?
        .expect("cart exists");

    assert_eq!(cart.lines.len(), 1);
    assert_eq!(cart.lines[0].product_id, socks.id);
    assert_eq!(cart.lines[0].quantity, 4);
    assert_eq!(cart.item_count(), 4);
    assert_eq!(cart.total(), Money::new(4 * 799, Currency::Eur));
    assert!(cart.checked_out);
    assert!(!cart.is_empty());

    db.close().await;
    Ok(())
}

#[tokio::test]
async fn a_cart_is_locked_to_its_first_lines_currency() -> anyhow::Result<()> {
    let db = TestDb::new().await?;
    let cart_id = new_id();
    let mug = product("Mug", 1250);
    let socks = product("Socks", 799);

    add_item(&db.ctx.executor, &cart_id, &mug, mug.base_price(), 1).await?;

    // The same product priced for a USD region must not slip into a EUR cart.
    let refused = add_item(
        &db.ctx.executor,
        &cart_id,
        &socks,
        Money::new(899, Currency::Usd),
        1,
    )
    .await;
    assert!(matches!(refused, Err(AddItemError::CurrencyMismatch(_, _))));

    // The cart is untouched by the refusal.
    let cart = load_cart(&db.ctx.executor, &cart_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("cart should exist"))?;
    assert_eq!(cart.lines.len(), 1);
    assert_eq!(cart.total(), Money::new(1250, Currency::Eur));

    db.close().await;
    Ok(())
}

#[tokio::test]
async fn price_in_prefers_explicit_prices_over_the_base() {
    let mut mug = product("Mug", 1250);
    assert_eq!(
        mug.price_in(Currency::Eur),
        Some(Money::new(1250, Currency::Eur)),
        "the base price answers for its own currency"
    );
    assert_eq!(mug.price_in(Currency::Usd), None);

    mug.prices.push(Money::new(1399, Currency::Usd));
    assert_eq!(
        mug.price_in(Currency::Usd),
        Some(Money::new(1399, Currency::Usd))
    );

    // An explicit price in the base currency overrides the import price.
    mug.prices.push(Money::new(1100, Currency::Eur));
    assert_eq!(
        mug.price_in(Currency::Eur),
        Some(Money::new(1100, Currency::Eur))
    );
}
