mod common;

use common::*;
use timada_tenancy::{
    AGENT_CALLS_SHOWN, AGENT_KEY_PREFIX, DefineRole, MAX_AGENT_KEYS, MintAgentKey,
    PERMISSION_ORDERS_READ, PERMISSION_PRODUCTS_MANAGE, PERMISSION_PRODUCTS_READ,
    PERMISSION_PROPOSALS_PREPARE, TenancyError, agent_key_id, agent_key_user,
};

/// 2026-11-25 12:00 UTC.
const NOW: u64 = 1_795_608_000;

fn key(label: &str, shops: &[(&str, &[&str])]) -> MintAgentKey {
    MintAgentKey {
        label: label.to_owned(),
        shops: shops
            .iter()
            .map(|(shop, permissions)| {
                (
                    (*shop).to_owned(),
                    permissions.iter().map(|p| (*p).to_owned()).collect(),
                )
            })
            .collect(),
    }
}

#[tokio::test]
async fn a_key_covers_what_its_user_chose_and_never_more_than_they_hold() {
    let cmd = command().await;
    let alice = new_id();
    let bob = new_id();
    let acme = create_tenant(&cmd, "Acme", &alice).await;
    let other = create_tenant(&cmd, "Other", &alice).await;
    cmd.define_role(
        &alice,
        &acme.id,
        DefineRole {
            role: "writer".to_owned(),
            label: "Writer".to_owned(),
            permissions: vec![
                PERMISSION_PRODUCTS_MANAGE.to_owned(),
                PERMISSION_PROPOSALS_PREPARE.to_owned(),
            ],
        },
    )
    .await
    .unwrap();
    cmd.grant_member(&alice, &acme.id, &bob, "writer")
        .await
        .unwrap();
    assert!(cmd.load_agent_keys(&bob).await.unwrap().is_none());

    // Bob gives a key less than he holds: reading the catalog, preparing.
    let minted = cmd
        .mint_agent_key(
            &bob,
            key(
                " Claude ",
                &[(
                    &acme.id,
                    &[PERMISSION_PROPOSALS_PREPARE, PERMISSION_PRODUCTS_READ],
                )],
            ),
        )
        .await
        .unwrap();
    assert!(minted.token.starts_with(AGENT_KEY_PREFIX));
    assert_eq!(agent_key_user(&minted.token), Some(bob.as_str()));
    assert_eq!(minted.key.id, agent_key_id(&minted.token));
    assert_eq!(minted.key.label, "Claude");
    // The page shows the start of the secret, never the user's id.
    assert!(!minted.key.hint.contains(&bob), "{}", minted.key.hint);
    assert!(format!("{minted:?}").contains("******"));
    // Kept in the order of the vocabulary.
    assert_eq!(
        minted.key.shops[0].permissions,
        vec![PERMISSION_PRODUCTS_READ, PERMISSION_PROPOSALS_PREPARE]
    );
    assert!(minted.key.allows(&acme.id, PERMISSION_PRODUCTS_READ));
    assert!(!minted.key.allows(&acme.id, PERMISSION_PRODUCTS_MANAGE));
    assert!(!minted.key.allows(&acme.id, PERMISSION_ORDERS_READ));
    assert!(!minted.key.allows(&other.id, PERMISSION_PRODUCTS_READ));

    // The token finds its user and its key; anything else finds nothing.
    let (user, found) = cmd.agent_key_of(&minted.token).await.unwrap().unwrap();
    assert_eq!(
        (user.as_str(), found.id.as_str()),
        (bob.as_str(), minted.key.id.as_str())
    );
    for token in [
        "",
        "mk_",
        "tk_abc",
        &format!("{}x", minted.token),
        &minted.token.replace(&bob, &alice),
    ] {
        assert!(cmd.agent_key_of(token).await.unwrap().is_none(), "{token}");
    }

    // Never wider than what the user holds: a permission he lacks, a shop
    // he is not in, nothing ticked, a shop named twice, a word not known.
    for refused in [
        key("x", &[(&acme.id, &[PERMISSION_ORDERS_READ])]),
        key("x", &[(&other.id, &[PERMISSION_PRODUCTS_READ])]),
        key("x", &[(&acme.id, &[])]),
        key("x", &[]),
        key(
            "x",
            &[
                (&acme.id, &[PERMISSION_PRODUCTS_READ]),
                (&acme.id, &[PERMISSION_PRODUCTS_READ]),
            ],
        ),
        key("x", &[(&acme.id, &["products:destroy"])]),
        key("x", &[("nowhere", &[PERMISSION_PRODUCTS_READ])]),
    ] {
        let err = cmd.mint_agent_key(&bob, refused.clone()).await.unwrap_err();
        assert!(
            matches!(err, TenancyError::InvalidAgentKeyScope),
            "{refused:?}: {err}"
        );
    }
    let err = cmd
        .mint_agent_key(&bob, key("  ", &[(&acme.id, &[PERMISSION_PRODUCTS_READ])]))
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::InvalidApiKeyLabel), "{err}");

    // Withdrawn: the token finds nothing; the key stays on the list.
    assert!(matches!(
        cmd.withdraw_agent_key(&alice, &minted.key.id).await,
        Err(TenancyError::AgentKeyNotFound)
    ));
    cmd.withdraw_agent_key(&bob, &minted.key.id).await.unwrap();
    cmd.withdraw_agent_key(&bob, &minted.key.id).await.unwrap();
    assert!(cmd.agent_key_of(&minted.token).await.unwrap().is_none());
    let keys = cmd.load_agent_keys(&bob).await.unwrap().unwrap();
    assert_eq!(keys.keys.len(), 1);
    assert!(!keys.keys[0].is_active());
}

#[tokio::test]
async fn a_user_holds_twenty_keys_at_most() {
    let cmd = command().await;
    let alice = new_id();
    let acme = create_tenant(&cmd, "Acme", &alice).await;
    let one = || key("Agent", &[(&acme.id, &[PERMISSION_PRODUCTS_READ])]);
    let mut first = None;
    for _ in 0..MAX_AGENT_KEYS {
        let minted = cmd.mint_agent_key(&alice, one()).await.unwrap();
        first.get_or_insert(minted.key.id);
    }
    assert!(matches!(
        cmd.mint_agent_key(&alice, one()).await,
        Err(TenancyError::TooManyAgentKeys)
    ));
    // One withdrawn makes room.
    cmd.withdraw_agent_key(&alice, &first.unwrap())
        .await
        .unwrap();
    cmd.mint_agent_key(&alice, one()).await.unwrap();
}

#[tokio::test]
async fn what_a_key_asked_for_is_logged_a_day_at_a_time() {
    let cmd = command().await;
    let alice = new_id();
    let acme = create_tenant(&cmd, "Acme", &alice).await;
    let minted = cmd
        .mint_agent_key(
            &alice,
            key("Agent", &[(&acme.id, &[PERMISSION_PRODUCTS_READ])]),
        )
        .await
        .unwrap();
    let id = minted.key.id.as_str();
    assert_eq!(cmd.agent_key_last_day(id).await.unwrap(), None);
    cmd.log_agent_call(id, "list_shops", "", true, NOW)
        .await
        .unwrap();
    cmd.log_agent_call(id, "sales_report", &acme.id, false, NOW + 60)
        .await
        .unwrap();
    assert_eq!(
        cmd.agent_key_last_day(id).await.unwrap().as_deref(),
        Some("2026-11-25")
    );
    let day = cmd
        .load_agent_calls(id, "2026-11-25")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(day.count, 2);
    assert_eq!(
        (
            day.last[1].tool.as_str(),
            day.last[1].tenant_id.as_str(),
            day.last[1].ok
        ),
        ("sales_report", acme.id.as_str(), false)
    );
    // The next day has its own log, and is the last day used.
    cmd.log_agent_call(id, "list_shops", "", true, NOW + 86_400)
        .await
        .unwrap();
    assert_eq!(
        cmd.agent_key_last_day(id).await.unwrap().as_deref(),
        Some("2026-11-26")
    );
    assert_eq!(
        cmd.load_agent_calls(id, "2026-11-26")
            .await
            .unwrap()
            .unwrap()
            .count,
        1
    );
    assert!(
        cmd.load_agent_calls(id, "2026-11-27")
            .await
            .unwrap()
            .is_none()
    );
    // A long day keeps its count, and the last calls to show.
    for n in 0..AGENT_CALLS_SHOWN + 5 {
        cmd.log_agent_call(id, &format!("tool_{n}"), "", true, NOW + 2 * 86_400)
            .await
            .unwrap();
    }
    let long = cmd
        .load_agent_calls(id, "2026-11-27")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(long.count as usize, AGENT_CALLS_SHOWN + 5);
    assert_eq!(long.last.len(), AGENT_CALLS_SHOWN);
    assert_eq!(long.last[0].tool, "tool_5");
}
