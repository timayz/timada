use evento::Evento;
use serde_json::{Value, json};
use timada_catalog::CreateProduct;
use timada_proposals::ProposalStatus;
use timada_tenancy::{
    CreateTenant, DefineRole, MintAgentKey, PERMISSION_ORDERS_READ, PERMISSION_PRODUCTS_READ,
    PERMISSION_PROPOSALS_PREPARE,
};

use super::*;

const ALICE: &str = "user-alice";
const BOB: &str = "user-bob";

struct Rig {
    store: Evento,
    db: Db,
    limits: RateLimiter,
    shop: String,
    product: String,
    /// Bob's key: the catalog read, proposals prepared, in his one shop.
    token: String,
    key_id: String,
    now: u64,
}

impl Rig {
    /// A shop of Alice's where Bob reads the catalog, the orders and
    /// prepares proposals, with one product at 19.90, and a key of Bob's
    /// that covers the catalog and proposals only.
    async fn new() -> Self {
        let turso = evento::Turso::in_memory_concurrent().await.unwrap();
        crate::db::migrate_store_read_models(&turso).await.unwrap();
        let db = Db::from(turso.clone());
        let store = Evento::new(turso);
        let tenancy = timada_tenancy::Command(store.clone());
        let shop = tenancy
            .create_tenant(CreateTenant {
                name: "Acme Moto".to_owned(),
                owner_user_id: ALICE.to_owned(),
            })
            .await
            .unwrap();
        tenancy
            .define_role(
                ALICE,
                &shop.id,
                DefineRole {
                    role: "helper".to_owned(),
                    label: "Helper".to_owned(),
                    permissions: vec![
                        PERMISSION_PRODUCTS_READ.to_owned(),
                        PERMISSION_ORDERS_READ.to_owned(),
                        PERMISSION_PROPOSALS_PREPARE.to_owned(),
                    ],
                },
            )
            .await
            .unwrap();
        tenancy
            .grant_member(ALICE, &shop.id, BOB, "helper")
            .await
            .unwrap();
        let catalog = timada_catalog::Command::new(store.clone(), &shop.id);
        let product = catalog
            .create_product(
                ALICE,
                CreateProduct {
                    name: "Gants".to_owned(),
                    slug: None,
                },
            )
            .await
            .unwrap()
            .id;
        catalog
            .price_product(ALICE, &product, 1990, "EUR")
            .await
            .unwrap();
        let minted = tenancy
            .mint_agent_key(
                BOB,
                MintAgentKey {
                    label: "Claude".to_owned(),
                    shops: vec![(
                        shop.id.clone(),
                        vec![
                            PERMISSION_PRODUCTS_READ.to_owned(),
                            PERMISSION_PROPOSALS_PREPARE.to_owned(),
                        ],
                    )],
                },
            )
            .await
            .unwrap();
        Self {
            store,
            db,
            limits: RateLimiter::new(None),
            shop: shop.id,
            product,
            token: minted.token,
            key_id: minted.key.id,
            now: crate::auth::now(),
        }
    }

    async fn ask(&self, token: Option<&str>, body: Value) -> Answer {
        let authorization = token.map(|token| format!("Bearer {token}"));
        handle(
            &self.store,
            &self.db,
            &self.limits,
            authorization.as_deref(),
            body.to_string().as_bytes(),
            self.now,
        )
        .await
        .unwrap()
    }

    /// Calls a tool with Bob's key: whether it was refused, and what it
    /// answered (the JSON it said, or the sentence it refused with).
    async fn call(&self, name: &str, arguments: Value) -> (bool, Value) {
        let answer = self
            .ask(
                Some(&self.token),
                json!({
                    "jsonrpc": "2.0", "id": 7, "method": "tools/call",
                    "params": { "name": name, "arguments": arguments },
                }),
            )
            .await;
        assert_eq!(answer.status, 200, "{answer:?}");
        let body = answer.body.unwrap();
        assert_eq!(body["id"], 7);
        let result = &body["result"];
        let said = result["content"][0]["text"].as_str().unwrap_or_else(|| {
            panic!("no text in {body}");
        });
        let refused = result["isError"].as_bool().unwrap();
        let value = serde_json::from_str(said).unwrap_or_else(|_| json!(said));
        (refused, value)
    }
}

#[tokio::test]
async fn nothing_is_answered_without_a_key_that_stands() {
    let rig = Rig::new().await;
    let ping = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" });
    for token in [
        None,
        Some("mk_nobody.0123456789012345678901234567890123"),
        Some(""),
    ] {
        let answer = rig.ask(token, ping.clone()).await;
        assert_eq!(answer.status, 401, "{token:?}");
        assert_eq!(answer.body.unwrap()["error"]["code"], -32001);
    }
    assert_eq!(rig.ask(Some(&rig.token), ping.clone()).await.status, 200);
    assert_eq!(bearer(Some("bearer  abc ")), Some("abc"));
    assert_eq!(bearer(Some("Basic abc")), None);
    assert_eq!(bearer(Some("Bearer ")), None);

    // Withdrawn: turned away from the next call on.
    timada_tenancy::Command(rig.store.clone())
        .withdraw_agent_key(BOB, &rig.key_id)
        .await
        .unwrap();
    assert_eq!(rig.ask(Some(&rig.token), ping).await.status, 401);
}

#[tokio::test]
async fn a_client_is_told_what_the_server_speaks_and_offers() {
    let rig = Rig::new().await;
    let token = Some(rig.token.as_str());
    let hello = rig
        .ask(
            token,
            json!({
                "jsonrpc": "2.0", "id": "a", "method": "initialize",
                "params": { "protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": { "name": "test", "version": "1" } },
            }),
        )
        .await;
    let body = hello.body.unwrap();
    assert_eq!(body["id"], "a");
    // The revision the client asked for, when the server knows it.
    assert_eq!(body["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(body["result"]["serverInfo"]["name"], "timada");
    assert!(body["result"]["capabilities"]["tools"].is_object());
    let newer = rig
        .ask(
            token,
            json!({ "jsonrpc": "2.0", "id": 2, "method": "initialize", "params": { "protocolVersion": "2099-01-01" } }),
        )
        .await;
    assert_eq!(newer.body.unwrap()["result"]["protocolVersion"], PROTOCOL);

    // A notification is taken, and answered with nothing.
    let noted = rig
        .ask(
            token,
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        )
        .await;
    assert_eq!(
        noted,
        Answer {
            status: 202,
            body: None
        }
    );

    let listed = rig
        .ask(
            token,
            json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" }),
        )
        .await;
    let body = listed.body.unwrap();
    let tools = body["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), TOOLS.len());
    let change = tools
        .iter()
        .find(|tool| tool["name"] == "propose_product_change")
        .unwrap();
    assert_eq!(change["annotations"]["readOnlyHint"], false);
    assert_eq!(
        change["inputSchema"]["required"],
        json!(["shop", "product_id", "reason"])
    );
    assert_eq!(
        change["inputSchema"]["properties"]["price"]["type"],
        "string"
    );
    // No tool approves, publishes, refunds or pays.
    for tool in TOOLS {
        for word in ["approve", "publish", "refund", "pay"] {
            assert!(!tool.name.contains(word), "{}", tool.name);
        }
        // A tool about a shop takes its slug first.
        assert_eq!(
            tool.permissions.is_empty(),
            tool.arguments.first().is_none_or(|first| first.0 != "shop"),
            "{}",
            tool.name
        );
    }

    let unknown = rig
        .ask(
            token,
            json!({ "jsonrpc": "2.0", "id": 4, "method": "resources/list" }),
        )
        .await;
    assert_eq!(unknown.body.unwrap()["error"]["code"], -32601);
    let garbage = handle(
        &rig.store,
        &rig.db,
        &rig.limits,
        Some(&format!("Bearer {}", rig.token)),
        b"{",
        rig.now,
    )
    .await
    .unwrap();
    assert_eq!(garbage.status, 400);
    let batch = rig
        .ask(
            token,
            json!([{ "jsonrpc": "2.0", "id": 1, "method": "ping" }]),
        )
        .await;
    assert_eq!(batch.status, 400);
}

#[tokio::test]
async fn a_key_reads_what_it_covers_and_no_more() {
    let rig = Rig::new().await;
    let (refused, shops) = rig.call("list_shops", json!({})).await;
    assert!(!refused);
    assert_eq!(
        shops,
        json!({ "shops": [{
            "slug": "acme-moto",
            "name": "Acme Moto",
            "permissions": [PERMISSION_PRODUCTS_READ, PERMISSION_PROPOSALS_PREPARE],
        }] })
    );
    let (refused, product) = rig
        .call(
            "get_product",
            json!({ "shop": "acme-moto", "product_id": rig.product }),
        )
        .await;
    assert!(!refused, "{product}");
    assert_eq!(product["name"], "Gants");
    assert_eq!(product["price"], "19.90");
    assert_eq!(product["currency"], "EUR");
    assert_eq!(product["kept_out_of_ads"], false);
    let (refused, categories) = rig
        .call("list_categories", json!({ "shop": "acme-moto" }))
        .await;
    assert!(!refused);
    assert_eq!(categories, json!({ "categories": [] }));

    // Bob reads the orders; his key was not given them.
    let (refused, why) = rig
        .call("sales_report", json!({ "shop": "acme-moto" }))
        .await;
    assert!(refused);
    assert_eq!(
        why,
        json!("The key does not cover orders:read in « acme-moto ».")
    );
    let (refused, why) = rig
        .call("list_products", json!({ "shop": "elsewhere" }))
        .await;
    assert!(refused);
    assert!(
        why.as_str()
            .unwrap()
            .starts_with("The key covers no shop « elsewhere »")
    );
    let (refused, why) = rig
        .call(
            "get_product",
            json!({ "shop": "acme-moto", "product_id": "nope" }),
        )
        .await;
    assert!(refused);
    assert_eq!(why, json!("No such product."));

    // What Bob loses, his key loses: a role without the catalog.
    let tenancy = timada_tenancy::Command(rig.store.clone());
    tenancy
        .define_role(
            ALICE,
            &rig.shop,
            DefineRole {
                role: "preparer".to_owned(),
                label: "Preparer".to_owned(),
                permissions: vec![PERMISSION_PROPOSALS_PREPARE.to_owned()],
            },
        )
        .await
        .unwrap();
    tenancy
        .change_member_role(ALICE, &rig.shop, BOB, "preparer")
        .await
        .unwrap();
    let (refused, why) = rig
        .call("list_categories", json!({ "shop": "acme-moto" }))
        .await;
    assert!(refused);
    assert_eq!(
        why,
        json!("The key's user no longer holds products:read in « acme-moto ».")
    );
    let (_, shops) = rig.call("list_shops", json!({})).await;
    assert_eq!(
        shops["shops"][0]["permissions"],
        json!([PERMISSION_PROPOSALS_PREPARE])
    );
    // And out of the shop, the key covers nothing.
    tenancy.revoke_member(ALICE, &rig.shop, BOB).await.unwrap();
    let (_, shops) = rig.call("list_shops", json!({})).await;
    assert_eq!(shops, json!({ "shops": [] }));

    // Every call is on the key's log of the day, refused or not.
    let day = timada_core::civil::format_date(rig.now);
    let log = tenancy
        .load_agent_calls(&rig.key_id, &day)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(log.count, 9);
    assert_eq!(log.last[0].tool, "list_shops");
    assert_eq!(log.last[3].tool, "sales_report");
    assert_eq!(
        (log.last[3].tenant_id.as_str(), log.last[3].ok),
        (rig.shop.as_str(), false)
    );
    assert_eq!(
        tenancy.agent_key_last_day(&rig.key_id).await.unwrap(),
        Some(day)
    );
}

#[tokio::test]
async fn what_an_agent_prepares_waits_for_a_person() {
    let rig = Rig::new().await;
    let (refused, prepared) = rig
        .call(
            "propose_product_change",
            json!({
                "shop": "acme-moto",
                "product_id": rig.product,
                "name": "Gants moto été",
                "price": "24,5",
                "reason": "The keyword « gants moto été » has 2,400 searches a month.",
            }),
        )
        .await;
    assert!(!refused, "{prepared}");
    assert_eq!(prepared["status"], "waiting");
    let id = prepared["proposal_id"].as_str().unwrap();
    let proposal = timada_proposals::Command::new(rig.store.clone(), &rig.shop)
        .load_proposal(id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(proposal.status, ProposalStatus::Waiting);
    // Under the user's name, with the key as what prepared it.
    assert_eq!(proposal.prepared_by, BOB);
    assert_eq!(proposal.source, "agent Claude");
    assert_eq!(proposal.changes.len(), 2);
    assert_eq!(proposal.change("price").unwrap().after, "24.50");
    // Nothing changed in the shop.
    let product = timada_catalog::Command::new(rig.store.clone(), &rig.shop)
        .load_product(&rig.product)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(product.name, "Gants");
    assert_eq!(product.price.unwrap().amount_cents, 1990);

    // A new category, to be created as a draft once approved.
    let (refused, prepared) = rig
        .call(
            "propose_category",
            json!({ "shop": "acme-moto", "name": "Gants moto", "reason": "From the study's tree." }),
        )
        .await;
    assert!(!refused, "{prepared}");

    // What the shop would refuse is told to the agent, in its words.
    let (refused, why) = rig
        .call(
            "propose_product_change",
            json!({ "shop": "acme-moto", "product_id": rig.product, "price": "cheap", "reason": "x" }),
        )
        .await;
    assert!(refused);
    assert!(why.as_str().unwrap().starts_with("Not prepared:"), "{why}");
    // Keeping a product out of ads needs the ads' numbers, which this key
    // was not given.
    let (refused, why) = rig
        .call(
            "propose_ads_exclusion",
            json!({ "shop": "acme-moto", "product_id": rig.product, "reason": "x" }),
        )
        .await;
    assert!(refused);
    assert_eq!(
        why,
        json!("The key does not cover orders:read in « acme-moto ».")
    );

    // Arguments that do not fit are the client's mistake, not an answer.
    let answer = rig
        .ask(
            Some(&rig.token),
            json!({
                "jsonrpc": "2.0", "id": 9, "method": "tools/call",
                "params": { "name": "propose_product_change", "arguments": { "shop": "acme-moto", "product_id": rig.product, "reason": "x" } },
            }),
        )
        .await;
    assert_eq!(answer.body.unwrap()["error"]["code"], -32602);
    let answer = rig
        .ask(
            Some(&rig.token),
            json!({ "jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": { "name": "approve_proposal" } }),
        )
        .await;
    assert_eq!(answer.body.unwrap()["error"]["message"], "No such tool.");
}

#[tokio::test]
async fn a_key_is_held_to_its_calls_a_minute() {
    let rig = Rig::new().await;
    for _ in 0..CALLS_A_MINUTE {
        let (refused, _) = rig.call("list_shops", json!({})).await;
        assert!(!refused);
    }
    let answer = rig
        .ask(
            Some(&rig.token),
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "list_shops" } }),
        )
        .await;
    assert_eq!(answer.status, 429);
    // Listing the tools is not a call of one.
    let listed = rig
        .ask(
            Some(&rig.token),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        )
        .await;
    assert_eq!(listed.status, 200);
}
