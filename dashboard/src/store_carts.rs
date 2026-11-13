//! What the `/store/carts/*` and `/store/orders/*` endpoints answer a
//! storefront, without the HTTP: each call acts in the key's tenant through
//! the orders gateway and answers for the one cart or order whose id it was
//! given. The store API lets a storefront's key read neither the carts nor
//! the orders of its tenant, nor the invoices ([`crate::read_only_store`]),
//! so this is the only way a storefront learns about a cart (its own, by the
//! id it keeps in the shopper's browser), an order (the one its cart became,
//! or one it remembers) or the invoice and credit notes of that order, and
//! the only way it fills a cart and asks to pay: a cart's commands load the
//! cart first, which its key may not.
//!
//! An id the tenant has no cart or order under is 404 `{"error":
//! "cart_not_found"}` (or `order_not_found`), another tenant's included: the
//! gateway is pinned to the key's tenant, so an id from elsewhere is simply
//! not there. So is a cart dropped after lying idle (`Cart::Expired`, see
//! [`crate::idle_carts`]). A refusal to show the shopper is 422 with
//! [`timada_orders::wire::Refusal`]; the shop not taking writes for the
//! moment is 503 with `writes_paused`. Bodies and answers are JSON, the
//! shapes those of [`timada_orders::wire`]. The routes under
//! `crate::app::store::carts` and `crate::app::store::orders` do the HTTP;
//! the storefront library's `cart` module calls them.

use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use timada_invoicing::IssuedDocument;
use timada_orders::wire::{CartAnswer, Refusal, RequestAnswer, Started};
use timada_orders::{
    CartItem, CatalogReadModel, Checkout, Command, OrderError, STOREFRONT_ACTOR, StoreCatalog,
};
use topcoat::router::StatusCode;

use crate::cluster::{Executor, LocalReads};
use crate::db::Db;

/// How long a request to pay waits for the payments subscription's answer
/// before the storefront is told nobody answered.
pub const PAYMENT_ANSWER_TIMEOUT: Duration = Duration::from_secs(15);
/// How often it looks again when no write wakes it.
const PAYMENT_ANSWER_POLL: Duration = Duration::from_millis(250);

/// The status and JSON body to answer.
pub type Answer = (StatusCode, Value);

/// The orders gateway in the key's tenant.
pub fn gateway(executor: Executor, tenant_id: &str) -> Command<Executor> {
    Command::new(executor, tenant_id)
}

/// A JSON body as the type a call takes; `None` for one that is not.
pub fn parse<T: DeserializeOwned>(body: &[u8]) -> Option<T> {
    serde_json::from_slice(body).ok()
}

/// 400: the body is not what the call takes.
pub fn bad_request() -> Answer {
    (StatusCode::BAD_REQUEST, json!({ "error": "bad_request" }))
}

fn ok<T: serde::Serialize>(body: &T) -> anyhow::Result<Answer> {
    Ok((StatusCode::OK, serde_json::to_value(body)?))
}

fn not_found(code: &str) -> Answer {
    (StatusCode::NOT_FOUND, json!({ "error": code }))
}

/// The refusal as its status and body, or the failure itself for one that is
/// not the shopper's doing.
pub fn refused(err: OrderError) -> anyhow::Result<Answer> {
    match err {
        OrderError::CartNotFound => Ok(not_found("cart_not_found")),
        OrderError::OrderNotFound => Ok(not_found("order_not_found")),
        err => match Refusal::of(&err) {
            Some(refusal) if refusal.error == timada_core::WRITES_PAUSED => Ok((
                StatusCode::SERVICE_UNAVAILABLE,
                serde_json::to_value(refusal)?,
            )),
            Some(refusal) => Ok((
                StatusCode::UNPROCESSABLE_ENTITY,
                serde_json::to_value(refusal)?,
            )),
            None => Err(err.into()),
        },
    }
}

/// `GET /store/carts/{cart_id}`: the cart priced as it stands, with the
/// shop's shipping methods ([`CartAnswer`]), or 404. The cart is the
/// shopper's own stream, read through the cluster (what they just put in it
/// shows at once, whichever node took it); its lines are priced from the
/// catalog's read model (`db`), a moment behind the store, which the
/// payment's exact pricing guards: a price that moved in between is refused
/// as `price_changed`, never charged unseen. So the shop's offers, its codes
/// and its ways to ship, which no shopper writes, are read from this node's
/// file (`local`). A line the read model cannot sell is read again through
/// the cluster before the shopper is told (#558): the cart took it after
/// reading the store there, and a product put on sale a second ago may not
/// be in this node's read model yet.
pub async fn cart(
    orders: &Command<Executor>,
    local: &LocalReads,
    db: &Db,
    cart_id: &str,
) -> anyhow::Result<Answer> {
    let Some(cart) = orders.load_cart(cart_id).await? else {
        return Ok(not_found("cart_not_found"));
    };
    let shop = Command::new(local.0.clone(), orders.tenant());
    let catalog = CatalogReadModel::new(db, orders.tenant());
    let quote = match shop
        .quote_from(&cart, &catalog, &StoreCatalog(orders))
        .await
    {
        Ok(quote) => quote,
        Err(err) => return refused(err),
    };
    let shipping_methods = shop.shipping_methods().await?;
    ok(&CartAnswer {
        cart,
        quote,
        shipping_methods,
    })
}

/// `POST /store/carts` with a [`CartItem`]: starts a cart holding it and
/// answers 201 with its id ([`Started`]). The item is checked before the
/// cart is written, in one write: a refusal of the item is answered as such
/// and leaves nothing in the tenant's store. How many carts a storefront
/// may start is the route's to hold ([`crate::rate_limit::allow_new_cart`]).
pub async fn start(orders: &Command<Executor>, item: CartItem) -> anyhow::Result<Answer> {
    match orders.start_cart(STOREFRONT_ACTOR, item).await {
        Ok(cart_id) => Ok((
            StatusCode::CREATED,
            serde_json::to_value(Started { id: cart_id })?,
        )),
        Err(err) => refused(err),
    }
}

/// `POST /store/carts/{cart_id}/items` with a [`CartItem`]: adds it.
pub async fn add_item(
    orders: &Command<Executor>,
    cart_id: &str,
    item: CartItem,
) -> anyhow::Result<Answer> {
    match orders.add_to_cart(STOREFRONT_ACTOR, cart_id, item).await {
        Ok(_) => ok(&json!({})),
        Err(err) => refused(err),
    }
}

/// `POST /store/carts/{cart_id}/quantity` with a [`CartItem`]: sets how many
/// of that line the cart holds; zero removes it.
pub async fn set_quantity(
    orders: &Command<Executor>,
    cart_id: &str,
    item: CartItem,
) -> anyhow::Result<Answer> {
    match orders
        .set_cart_quantity(STOREFRONT_ACTOR, cart_id, item)
        .await
    {
        Ok(_) => ok(&json!({})),
        Err(err) => refused(err),
    }
}

/// `POST /store/carts/{cart_id}/discount` with a code: puts it on the cart.
pub async fn apply_discount(
    orders: &Command<Executor>,
    cart_id: &str,
    code: &str,
) -> anyhow::Result<Answer> {
    match orders.apply_discount(STOREFRONT_ACTOR, cart_id, code).await {
        Ok(_) => ok(&json!({})),
        Err(err) => refused(err),
    }
}

/// `POST /store/carts/{cart_id}/discount/remove`: takes the code off.
pub async fn remove_discount(orders: &Command<Executor>, cart_id: &str) -> anyhow::Result<Answer> {
    match orders.remove_discount(STOREFRONT_ACTOR, cart_id).await {
        Ok(_) => ok(&json!({})),
        Err(err) => refused(err),
    }
}

/// `POST /store/carts/{cart_id}/checkout` with a [`Checkout`]: asks to pay
/// and waits up to [`PAYMENT_ANSWER_TIMEOUT`] for the payments subscription
/// to open a payment or refuse; answers the request and what it got
/// ([`RequestAnswer`], its `answer` `None` when nobody answered in time).
///
/// The answer is written on whichever node takes the request and wakes this
/// node's write watch once applied here, so the cart is looked at in this
/// node's file (`local`) while waiting, every write in the store waking the
/// loop, and through the cluster once more before nobody answered.
pub async fn checkout(
    orders: &Command<Executor>,
    local: &LocalReads,
    cart_id: &str,
    checkout: Checkout,
) -> anyhow::Result<Answer> {
    let request_id = match orders
        .request_checkout(STOREFRONT_ACTOR, cart_id, checkout)
        .await
    {
        Ok(id) => id,
        Err(err) => return refused(err),
    };
    let waiting = Command::new(local.0.clone(), orders.tenant());
    let mut watch = evento::Executor::write_watch(orders.executor());
    let deadline = tokio::time::Instant::now() + PAYMENT_ANSWER_TIMEOUT;
    loop {
        let last = tokio::time::Instant::now() >= deadline;
        let cart = if last { orders } else { &waiting }
            .load_cart(cart_id)
            .await?;
        let answer = cart
            .as_ref()
            .and_then(|cart| cart.request(&request_id))
            .and_then(|request| request.answer.clone());
        if answer.is_some() || last {
            if answer.is_none() {
                tracing::warn!(cart_id, request_id, "no answer to a request to pay");
            }
            return ok(&RequestAnswer { request_id, answer });
        }
        // A write to the store wakes us; the poll covers a missed one.
        match watch.as_mut() {
            Some(watch) => {
                let _ = tokio::time::timeout(PAYMENT_ANSWER_POLL, watch.changed()).await;
            }
            None => tokio::time::sleep(PAYMENT_ANSWER_POLL).await,
        }
    }
}

/// `GET /store/carts/{cart_id}/requests/{request_id}`: a request to pay and
/// its answer so far, or 404 for a cart or request the tenant has not. The
/// storefront asks while the shopper waits to pay: read from this node's file
/// once it holds the answer, through the cluster until then, so the request
/// it made a moment ago and the answer another node gave both show.
pub async fn request(
    orders: &Command<Executor>,
    local: &LocalReads,
    cart_id: &str,
    request_id: &str,
) -> anyhow::Result<Answer> {
    let cart = read_first(
        orders,
        local,
        |reads| async move { reads.load_cart(cart_id).await },
        |cart| {
            cart.as_ref().is_ok_and(|cart| {
                cart.as_ref()
                    .and_then(|cart| cart.request(request_id))
                    .is_some_and(|request| request.answer.is_some())
            })
        },
    )
    .await?;
    let Some(cart) = cart else {
        return Ok(not_found("cart_not_found"));
    };
    let Some(request) = cart.request(request_id) else {
        return Ok(not_found("payment_not_found"));
    };
    ok(&RequestAnswer {
        request_id: request.request_id.clone(),
        answer: request.answer.clone(),
    })
}

/// `GET /store/carts/{cart_id}/order`: the order the cart became, or 404
/// while it is none. The storefront asks until there is one, which a
/// payment's confirmation places on any node: read from this node's file
/// once it holds the order, through the cluster before saying there is none.
pub async fn order_of_cart(
    orders: &Command<Executor>,
    local: &LocalReads,
    cart_id: &str,
) -> anyhow::Result<Answer> {
    let order = read_first(
        orders,
        local,
        |reads| async move { reads.load_order_of_cart(cart_id).await },
        |order| matches!(order, Ok(Some(_))),
    )
    .await?;
    match order {
        Some(order) => ok(&order),
        None => Ok(not_found("order_not_found")),
    }
}

/// `GET /store/orders/{order_id}`: the order, or 404. Read through the
/// cluster: the storefront sends the shopper back to their order once they
/// cancelled it, whichever node took the cancellation, and the order must
/// show it.
pub async fn order(orders: &Command<Executor>, order_id: &str) -> anyhow::Result<Answer> {
    match orders.load_order(order_id).await? {
        Some(order) => ok(&order),
        None => Ok(not_found("order_not_found")),
    }
}

/// `GET /store/orders/{order_id}/parcels`: where the order's parcels are, as
/// their carrier last said (a `Vec<ParcelState>`, empty until the first
/// news); 404 for an order the tenant has not. Read from this node's file
/// once it holds the order, through the cluster before saying there is none:
/// news another node recorded a moment ago shows at the next look.
pub async fn parcels(
    orders: &Command<Executor>,
    local: &LocalReads,
    order_id: &str,
) -> anyhow::Result<Answer> {
    let parcels = read_first(
        orders,
        local,
        |reads| async move {
            if reads.load_order(order_id).await?.is_none() {
                return anyhow::Ok(None);
            }
            let parcels = reads
                .load_shipment(order_id)
                .await?
                .map(|shipment| shipment.parcels)
                .unwrap_or_default();
            Ok(Some(parcels))
        },
        |parcels| matches!(parcels, Ok(Some(_))),
    )
    .await?;
    match parcels {
        Some(parcels) => ok(&parcels),
        None => Ok(not_found("order_not_found")),
    }
}

/// `GET /store/orders/{order_id}/documents`: the invoice of the order, then
/// the credit notes of its refunds, as far as the dashboard has issued them
/// (a `Vec<IssuedDocument>`, empty for an order just placed); 404 for an
/// order the tenant has not. Each document is read whole from its own index
/// stream (the storefront draws the PDF, so it needs all of it; the
/// invoicing read model keeps a summary row only). The documents live in
/// the invoicing streams, every buyer's name and address in them, which is
/// why a storefront's key reads none of those and is answered one order's
/// here. Read from this node's file once it holds the order, through the
/// cluster before saying there is none: a document issued a moment ago on
/// another node shows at the next look.
pub async fn documents(
    orders: &Command<Executor>,
    local: &LocalReads,
    order_id: &str,
) -> anyhow::Result<Answer> {
    let documents = read_first(
        orders,
        local,
        |reads| async move {
            let Some(order) = reads.load_order(order_id).await? else {
                return anyhow::Ok(None);
            };
            let invoicing =
                timada_invoicing::Command::new(reads.executor().clone(), reads.tenant());
            let mut documents: Vec<IssuedDocument> = Vec::new();
            documents.extend(invoicing.load_invoice_of_order(order_id).await?);
            for refund_id in &order.refund_ids {
                documents.extend(invoicing.load_credit_note_of_refund(refund_id).await?);
            }
            Ok(Some(documents))
        },
        |documents| matches!(documents, Ok(Some(_))),
    )
    .await?;
    match documents {
        Some(documents) => ok(&documents),
        None => Ok(not_found("order_not_found")),
    }
}

/// Reads with `read`, handed the orders gateway in `orders`' tenant over
/// this node's file (`local`), and again over the cluster (`orders`' store)
/// unless `settled` lets the first answer stand ([`LocalReads::first`]).
async fn read_first<T, F, Fut>(
    orders: &Command<Executor>,
    local: &LocalReads,
    read: F,
    settled: impl FnOnce(&T) -> bool,
) -> T
where
    F: Fn(Command<Executor>) -> Fut,
    Fut: Future<Output = T>,
{
    local
        .first(
            orders.executor(),
            |store| read(Command::new(store, orders.tenant())),
            settled,
        )
        .await
}

#[cfg(test)]
mod tests {
    use evento::Evento;
    use timada_catalog::CreateProduct;

    use super::*;
    use crate::cluster::lagging::{behind, events};

    const SHOP: &str = "acme";

    async fn answer(
        orders: &Command<Executor>,
        local: &LocalReads,
        db: &Db,
        cart_id: &str,
    ) -> CartAnswer {
        let (status, body) = cart(orders, local, db, cart_id).await.unwrap();
        assert_eq!(status, StatusCode::OK, "{body}");
        serde_json::from_value(body).unwrap()
    }

    /// A mug put on sale a moment ago through another node: neither this
    /// node's read model nor its file has it yet, and the cart that took it
    /// prices it all the same, read through the cluster. Off sale again, the
    /// cluster says so too.
    #[tokio::test]
    async fn a_line_this_node_has_not_caught_up_on_is_read_through_the_cluster() {
        let turso = evento::Turso::in_memory_concurrent().await.unwrap();
        crate::db::migrate_store_read_models(&turso).await.unwrap();
        // No subscription runs: the read models stay empty.
        let db = Db::from(turso.clone());
        let store = Evento::new(turso);
        let catalog = timada_catalog::Command::new(store.clone(), SHOP);
        let mug = catalog
            .create_product(
                "alice",
                CreateProduct {
                    name: "Mug".into(),
                    slug: None,
                },
            )
            .await
            .unwrap()
            .id;
        catalog
            .price_product("alice", &mug, 1250, "EUR")
            .await
            .unwrap();
        catalog.publish_product("alice", &mug).await.unwrap();
        let orders = gateway(store, SHOP);
        let cart_id = orders
            .start_cart(
                STOREFRONT_ACTOR,
                CartItem {
                    product_id: mug.clone(),
                    variant_id: None,
                    quantity: 2,
                },
            )
            .await
            .unwrap();
        let local = LocalReads(Evento::new(
            evento::Turso::in_memory_concurrent().await.unwrap(),
        ));

        let shown = answer(&orders, &local, &db, &cart_id).await.quote;
        assert!(shown.unavailable.is_empty());
        assert_eq!(shown.total_cents, 2 * 1250);

        catalog.unpublish_product("alice", &mug).await.unwrap();
        let shown = answer(&orders, &local, &db, &cart_id).await.quote;
        assert!(shown.lines.is_empty());
        assert_eq!(shown.unavailable.len(), 1);
    }

    /// A store with the read models' tables, nothing in it yet.
    async fn store() -> evento::Turso {
        let turso = evento::Turso::in_memory_concurrent().await.unwrap();
        crate::db::migrate_store_read_models(&turso).await.unwrap();
        turso
    }

    /// A mug on sale in `store`: its id.
    async fn mug_on_sale(store: &Evento) -> String {
        let catalog = timada_catalog::Command::new(store.clone(), SHOP);
        let mug = catalog
            .create_product(
                "alice",
                CreateProduct {
                    name: "Mug".into(),
                    slug: None,
                },
            )
            .await
            .unwrap()
            .id;
        catalog
            .price_product("alice", &mug, 1250, "EUR")
            .await
            .unwrap();
        catalog.publish_product("alice", &mug).await.unwrap();
        mug
    }

    /// A cart of two of `mug` in `store`: its id.
    async fn cart_of(store: &Evento, mug: &str) -> String {
        gateway(store.clone(), SHOP)
            .start_cart(
                STOREFRONT_ACTOR,
                CartItem {
                    product_id: mug.to_owned(),
                    variant_id: None,
                    quantity: 2,
                },
            )
            .await
            .unwrap()
    }

    /// The cart asked to be paid for: the request's id.
    async fn ask_to_pay(store: &Evento, cart_id: &str) -> String {
        let checkout = Checkout {
            email: "ada@example.org".into(),
            shipping: timada_orders::ShippingAddress {
                name: "Ada Lovelace".into(),
                line1: "1 rue de la Paix".into(),
                postal_code: "75002".into(),
                city: "Paris".into(),
                country: "FR".into(),
                phone: Some("06 12 34 56 78".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        gateway(store.clone(), SHOP)
            .request_checkout(STOREFRONT_ACTOR, cart_id, checkout)
            .await
            .unwrap()
    }

    /// The answer to the request: refused, as a node that cannot pay
    /// answers it.
    async fn refuse(store: &Evento, cart_id: &str, request_id: &str) {
        assert!(
            gateway(store.clone(), SHOP)
                .refuse_payment_request(
                    "timada-dashboard",
                    cart_id,
                    request_id,
                    "payments_unavailable"
                )
                .await
                .unwrap()
        );
    }

    /// The order `cart_id` became, as a payment's confirmation places it.
    async fn place(store: &Evento, cart_id: &str) {
        evento::append(timada_orders::order_id(cart_id))
            .tenant(SHOP)
            .original_version(0)
            .event(&timada_orders::aggregator::Placed {
                reference: "K7Q4-MZP2".into(),
                cart_id: cart_id.to_owned(),
                email: "ada@example.org".into(),
                shipping: Default::default(),
                note: None,
                lines: Vec::new(),
                currency: "EUR".into(),
                total_cents: 2500,
                payment: Default::default(),
                supplier: None,
                placed_by: STOREFRONT_ACTOR.into(),
            })
            .commit(store)
            .await
            .unwrap();
    }

    /// What `answer` says of the request: its code once refused, `None`
    /// while it waits; the status when it is no request's.
    fn refusal(answer: Answer) -> Result<Option<String>, StatusCode> {
        let (status, body) = answer;
        if status != StatusCode::OK {
            return Err(status);
        }
        let request: RequestAnswer = serde_json::from_value(body).unwrap();
        Ok(request.answer.map(|answer| match answer {
            timada_orders::PaymentAnswer::Refused(code) => code,
            other => panic!("not a refusal: {other:?}"),
        }))
    }

    /// A request this node holds the answer of is read from its file: the
    /// cluster, asked, would know of no such cart.
    #[tokio::test]
    async fn an_answered_request_this_node_holds_is_read_from_its_file() {
        let store = Evento::new(store().await);
        let mug = mug_on_sale(&store).await;
        let cart_id = cart_of(&store, &mug).await;
        let request_id = ask_to_pay(&store, &cart_id).await;
        refuse(&store, &cart_id, &request_id).await;
        let nothing = gateway(
            Evento::new(evento::Turso::in_memory_concurrent().await.unwrap()),
            SHOP,
        );

        let answer = request(&nothing, &LocalReads(store), &cart_id, &request_id).await;
        assert_eq!(
            refusal(answer.unwrap()),
            Ok(Some("payments_unavailable".to_owned()))
        );
    }

    /// The request a storefront made a moment ago, and the answer another
    /// node gave it, are read through the cluster while this node has not
    /// got them: the cart, the request or the answer.
    #[tokio::test]
    async fn a_request_this_node_has_no_answer_to_is_read_through_the_cluster() {
        let turso = store().await;
        let store = Evento::new(turso.clone());
        let mug = mug_on_sale(&store).await;
        let before_the_cart = events(&turso).await.len();
        let cart_id = cart_of(&store, &mug).await;
        let before_the_request = events(&turso).await.len();
        let request_id = ask_to_pay(&store, &cart_id).await;
        let before_the_answer = events(&turso).await.len();
        refuse(&store, &cart_id, &request_id).await;
        let orders = gateway(store.clone(), SHOP);

        for held in [before_the_cart, before_the_request, before_the_answer] {
            let local = behind(&turso, held).await;
            let answer = request(&orders, &local, &cart_id, &request_id).await;
            assert_eq!(
                refusal(answer.unwrap()),
                Ok(Some("payments_unavailable".to_owned())),
                "with the first {held} events here"
            );
        }
        // Waiting on the cluster too: it waits, and says so; a request the
        // cart never made is none.
        let waiting_cart = cart_of(&store, &mug).await;
        let waiting = ask_to_pay(&store, &waiting_cart).await;
        let local = behind(&turso, before_the_answer).await;
        let answer = request(&orders, &local, &waiting_cart, &waiting).await;
        assert_eq!(refusal(answer.unwrap()), Ok(None));
        let answer = request(&orders, &local, &waiting_cart, "no-such-request").await;
        assert_eq!(refusal(answer.unwrap()), Err(StatusCode::NOT_FOUND));
    }

    /// The order a cart became, and its parcels and documents, are read from
    /// this node's file once it holds the order, and through the cluster
    /// while it does not: placed a moment ago on another node, or not at all.
    #[tokio::test]
    async fn an_order_this_node_lacks_is_read_through_the_cluster() {
        let turso = store().await;
        let store = Evento::new(turso.clone());
        let mug = mug_on_sale(&store).await;
        let cart_id = cart_of(&store, &mug).await;
        ask_to_pay(&store, &cart_id).await;
        let before_the_order = events(&turso).await.len();
        place(&store, &cart_id).await;
        let order_id = timada_orders::order_id(&cart_id);
        let orders = gateway(store.clone(), SHOP);
        let nothing = gateway(
            Evento::new(evento::Turso::in_memory_concurrent().await.unwrap()),
            SHOP,
        );
        let status = |answer: anyhow::Result<Answer>| answer.unwrap().0;

        let here = LocalReads(store);
        let (found, order) = order_of_cart(&nothing, &here, &cart_id).await.unwrap();
        assert_eq!(found, StatusCode::OK);
        assert_eq!(order["reference"], json!("K7Q4-MZP2"));
        let (found, parcels_here) = parcels(&nothing, &here, &order_id).await.unwrap();
        assert_eq!((found, parcels_here), (StatusCode::OK, json!([])));
        let (found, documents_here) = documents(&nothing, &here, &order_id).await.unwrap();
        assert_eq!((found, documents_here), (StatusCode::OK, json!([])));

        let behind = behind(&turso, before_the_order).await;
        let (found, order) = order_of_cart(&orders, &behind, &cart_id).await.unwrap();
        assert_eq!(found, StatusCode::OK);
        assert_eq!(order["reference"], json!("K7Q4-MZP2"));
        assert_eq!(
            status(parcels(&orders, &behind, &order_id).await),
            StatusCode::OK
        );
        assert_eq!(
            status(documents(&orders, &behind, &order_id).await),
            StatusCode::OK
        );

        // Nowhere: there is none.
        let unpaid = cart_of(&orders.executor().clone(), &mug).await;
        assert_eq!(
            status(order_of_cart(&orders, &behind, &unpaid).await),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status(parcels(&orders, &behind, "order-none").await),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status(documents(&orders, &behind, "order-none").await),
            StatusCode::NOT_FOUND
        );
    }
}
