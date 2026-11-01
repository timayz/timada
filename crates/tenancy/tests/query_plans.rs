//! Probes of the per-request joins under Turso's planner (issue #336): each
//! seeds a few thousand rows in a database file opened for several writers,
//! prints `EXPLAIN QUERY PLAN` and a timing for the query as the page runs
//! it and for the alternatives, and asserts that the page's version answers
//! in a single pass over the rows it needs. Ignored: they take seconds and
//! their report is their point. Run them with
//! `cargo test -p timada-tenancy --test query_plans -- --ignored --nocapture`.

use std::time::Instant;

use timada_core::db::{
    Connection, Db, IntoParams, Turso, execute, fetch_all, params_from_iter, placeholders,
};
use timada_tenancy::{companies_of_user, tenants_of_user};

/// A database file opened for several writers, with the tenancy schema.
async fn database() -> (Turso, Db) {
    let store = Turso::temporary_concurrent().await.unwrap();
    let mut conn = store.connect().unwrap();
    timada_tenancy::MIGRATIONS
        .to_latest(&mut conn)
        .await
        .map_err(|(_, err)| err)
        .unwrap();
    (store.clone(), Db::from(store))
}

/// The `detail` column of `EXPLAIN QUERY PLAN sql`, one line per step.
async fn plan(conn: &Connection, sql: &str, params: impl IntoParams) -> Vec<String> {
    let rows: Vec<(i64, i64, i64, String)> =
        fetch_all(conn, &format!("EXPLAIN QUERY PLAN {sql}"), params)
            .await
            .unwrap();
    rows.into_iter().map(|(_, _, _, detail)| detail).collect()
}

/// Prints the plan of `sql` under `title`, and returns it.
async fn report_plan(
    conn: &Connection,
    title: &str,
    sql: &str,
    params: impl IntoParams,
) -> Vec<String> {
    println!("--- {title}");
    let plan = plan(conn, sql, params).await;
    for line in &plan {
        println!("    {line}");
    }
    plan
}

/// Fails unless one step of `plan` is `step`: the inner side of a join
/// looked up by its primary key, say.
fn assert_step(plan: &[String], step: &str) {
    assert!(
        plan.iter().any(|line| line == step),
        "no step {step:?} in the plan:\n{}",
        plan.join("\n")
    );
}

/// Runs `run` `rounds` times and prints the mean wall time under `title`.
async fn timed<F, Fut, T>(title: &str, rounds: u32, mut run: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let mut last = None;
    let started = Instant::now();
    for _ in 0..rounds {
        last = Some(run().await);
    }
    let each = started.elapsed() / rounds;
    println!("    {title}: {each:?} per run ({rounds} runs)");
    last.unwrap()
}

/// A user id that sorts and hashes like the real ones.
fn user(n: usize) -> String {
    format!("user-{n:06}")
}

const TENANTS: usize = 3_000;
const MEMBERS_PER_TENANT: usize = 3;
/// How many tenants the probed user belongs to.
const ALICE_TENANTS: usize = 40;

/// Seeds `TENANTS` tenants with `MEMBERS_PER_TENANT` members each, Alice
/// being a member of one tenant in `TENANTS / ALICE_TENANTS`.
async fn seed_tenants(conn: &Connection, alice: &str) {
    execute(conn, "BEGIN", ()).await.unwrap();
    for n in 0..TENANTS {
        let id = format!("tenant-{n:06}");
        let name = format!("Shop {}", (n * 7919) % TENANTS);
        execute(
            conn,
            "INSERT INTO tenancy_tenants (id, slug, name, created_by, created_at, suspended,
                suspended_reason, member_count, company_id, series_code, slug_search, name_search)
             VALUES (?1, ?2, ?3, ?4, ?5, 0, NULL, ?6, NULL, NULL, ?2, ?7)",
            (
                id.as_str(),
                format!("shop-{n}"),
                name.as_str(),
                user(n),
                n as i64,
                MEMBERS_PER_TENANT as i64,
                name.to_lowercase(),
            ),
        )
        .await
        .unwrap();
        for k in 0..MEMBERS_PER_TENANT {
            let member = if k == 0 && n % (TENANTS / ALICE_TENANTS) == 0 {
                alice.to_owned()
            } else {
                user(n * MEMBERS_PER_TENANT + k)
            };
            execute(
                conn,
                "INSERT INTO tenancy_members (tenant_id, user_id, role) VALUES (?1, ?2, ?3)",
                (id.as_str(), member, if k == 0 { "owner" } else { "member" }),
            )
            .await
            .unwrap();
        }
    }
    execute(conn, "COMMIT", ()).await.unwrap();
}

const JOIN_TENANTS_OF_USER: &str = "SELECT t.id AS tenant_id, t.slug, t.name, t.suspended, m.role
         FROM tenancy_members m JOIN tenancy_tenants t ON t.id = m.tenant_id
         WHERE m.user_id = ?1
         ORDER BY t.name COLLATE NOCASE, t.id";

#[tokio::test]
#[ignore = "a probe: seeds thousands of rows and prints plans and timings"]
async fn tenants_of_user_plan() {
    let (store, db) = database().await;
    let conn = store.connect().unwrap();
    let alice = "user-alice";
    seed_tenants(&conn, alice).await;

    println!(
        "tenants_of_user: {TENANTS} tenants, {} members, Alice in {ALICE_TENANTS}",
        TENANTS * MEMBERS_PER_TENANT
    );
    let plan = report_plan(
        &conn,
        "one join (members -> tenants)",
        JOIN_TENANTS_OF_USER,
        (alice,),
    )
    .await;
    // The members of the user by their index, then each tenant by its key.
    assert_step(
        &plan,
        "SEARCH m USING INDEX tenancy_members_user (user_id=?)",
    );
    assert_step(
        &plan,
        "SEARCH t USING INDEX sqlite_autoindex_tenancy_tenants_1 (id=?)",
    );
    let joined: Vec<(String, String, String, bool, String)> = timed("one join", 20, || {
        fetch_all(&conn, JOIN_TENANTS_OF_USER, (alice,))
    })
    .await
    .unwrap();
    assert_eq!(joined.len(), ALICE_TENANTS);

    report_plan(
        &conn,
        "memberships alone",
        "SELECT tenant_id, role FROM tenancy_members WHERE user_id = ?1",
        (alice,),
    )
    .await;
    let two_scans = timed("two scans joined here", 20, || async {
        let memberships: Vec<(String, String)> = fetch_all(
            &conn,
            "SELECT tenant_id, role FROM tenancy_members WHERE user_id = ?1",
            (alice,),
        )
        .await
        .unwrap();
        let ids: Vec<&str> = memberships.iter().map(|(id, _)| id.as_str()).collect();
        let tenants: Vec<(String, String, String, bool)> = fetch_all(
            &conn,
            &format!(
                "SELECT id, slug, name, suspended FROM tenancy_tenants WHERE id IN ({})",
                placeholders(ids.len())
            ),
            params_from_iter(ids),
        )
        .await
        .unwrap();
        tenants.len()
    })
    .await;
    assert_eq!(two_scans, ALICE_TENANTS);
    report_plan(
        &conn,
        "tenants by id list",
        &format!(
            "SELECT id, slug, name, suspended FROM tenancy_tenants WHERE id IN ({})",
            placeholders(3)
        ),
        ("tenant-000000", "tenant-000075", "tenant-000150"),
    )
    .await;

    let page = timed("the page's query", 20, || tenants_of_user(&db, alice))
        .await
        .unwrap();
    assert_eq!(page.len(), ALICE_TENANTS);
    let names: Vec<_> = joined.iter().map(|row| row.2.as_str()).collect();
    let page_names: Vec<_> = page.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(page_names, names);
}

const COMPANIES: usize = 3_000;
/// How many companies the probed user owns.
const ALICE_COMPANIES: usize = 30;

/// Seeds `COMPANIES` companies with one owner each and a second one on
/// every tenth, Alice owning one in `COMPANIES / ALICE_COMPANIES`.
async fn seed_companies(conn: &Connection, alice: &str) {
    execute(conn, "BEGIN", ()).await.unwrap();
    for n in 0..COMPANIES {
        let id = format!("company-{n:06}");
        execute(
            conn,
            "INSERT INTO tenancy_companies (id, siren, legal_name, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            (
                id.as_str(),
                format!("{:09}", 100_000_000 + n),
                format!("Company {}", (n * 7919) % COMPANIES),
                n as i64,
            ),
        )
        .await
        .unwrap();
        let owner = if n % (COMPANIES / ALICE_COMPANIES) == 0 {
            alice.to_owned()
        } else {
            user(n)
        };
        execute(
            conn,
            "INSERT INTO tenancy_company_owners (company_id, user_id) VALUES (?1, ?2)",
            (id.as_str(), owner),
        )
        .await
        .unwrap();
        if n % 10 == 0 {
            execute(
                conn,
                "INSERT INTO tenancy_company_owners (company_id, user_id) VALUES (?1, ?2)",
                (id.as_str(), user(COMPANIES + n)),
            )
            .await
            .unwrap();
        }
    }
    execute(conn, "COMMIT", ()).await.unwrap();
}

const JOIN_COMPANIES_OF_USER: &str =
    "SELECT c.id, c.siren, c.legal_name, c.created_at FROM tenancy_companies c
         JOIN tenancy_company_owners o ON o.company_id = c.id
         WHERE o.user_id = ?1 ORDER BY c.legal_name COLLATE NOCASE";

const JOIN_OWNERS_FIRST: &str =
    "SELECT c.id, c.siren, c.legal_name, c.created_at FROM tenancy_company_owners o
         JOIN tenancy_companies c ON c.id = o.company_id
         WHERE o.user_id = ?1 ORDER BY c.legal_name COLLATE NOCASE";

#[tokio::test]
#[ignore = "a probe: seeds thousands of rows and prints plans and timings"]
async fn companies_of_user_plan() {
    let (store, db) = database().await;
    let conn = store.connect().unwrap();
    let alice = "user-alice";
    seed_companies(&conn, alice).await;

    println!("companies_of_user: {COMPANIES} companies, Alice owns {ALICE_COMPANIES}");
    let plan = report_plan(
        &conn,
        "one join (companies -> owners)",
        JOIN_COMPANIES_OF_USER,
        (alice,),
    )
    .await;
    // The user's ownerships by their index, then each company by its key.
    assert_step(
        &plan,
        "SEARCH o USING INDEX tenancy_company_owners_user (user_id=?)",
    );
    assert_step(
        &plan,
        "SEARCH c USING INDEX sqlite_autoindex_tenancy_companies_1 (id=?)",
    );
    let joined: Vec<(String, String, String, i64)> = timed("one join", 20, || {
        fetch_all(&conn, JOIN_COMPANIES_OF_USER, (alice,))
    })
    .await
    .unwrap();
    assert_eq!(joined.len(), ALICE_COMPANIES);

    report_plan(
        &conn,
        "one join (owners -> companies)",
        JOIN_OWNERS_FIRST,
        (alice,),
    )
    .await;
    let owners_first: Vec<(String, String, String, i64)> = timed("owners first", 20, || {
        fetch_all(&conn, JOIN_OWNERS_FIRST, (alice,))
    })
    .await
    .unwrap();
    assert_eq!(owners_first, joined);

    let two_scans = timed("two scans joined here", 20, || async {
        let owned: Vec<(String,)> = fetch_all(
            &conn,
            "SELECT company_id FROM tenancy_company_owners WHERE user_id = ?1",
            (alice,),
        )
        .await
        .unwrap();
        let ids: Vec<&str> = owned.iter().map(|(id,)| id.as_str()).collect();
        let companies: Vec<(String, String, String, i64)> = fetch_all(
            &conn,
            &format!(
                "SELECT id, siren, legal_name, created_at FROM tenancy_companies WHERE id IN ({})",
                placeholders(ids.len())
            ),
            params_from_iter(ids),
        )
        .await
        .unwrap();
        companies.len()
    })
    .await;
    assert_eq!(two_scans, ALICE_COMPANIES);

    let page = timed("the page's query", 20, || companies_of_user(&db, alice))
        .await
        .unwrap();
    let names: Vec<_> = joined.iter().map(|row| row.2.as_str()).collect();
    let page_names: Vec<_> = page.iter().map(|row| row.legal_name.as_str()).collect();
    assert_eq!(page_names, names);
}
