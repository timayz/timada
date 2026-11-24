//! Makes the changes a member approved. `Approved` is the intent; this
//! subscription sees it on every node of the work group and makes each
//! change through the command the product or category page uses, under
//! the name of the member who approved, then says on the proposal how it
//! went.
//!
//! Each step is made once whoever makes it and however often: a field that
//! holds the approved value already is passed over, so a proposal seen by
//! two nodes, or by one that stopped half-way, ends on the same values, and
//! a category to create goes by an id made of the proposal's. A field that
//! holds neither what the proposal was prepared against nor the approved
//! value was changed in between: nothing is written over it and the
//! proposal ends as out of date.
//!
//! A refusal of the shop's (a product archived meanwhile, a slug taken) is
//! the proposal's outcome, recorded with its reason. Only the store
//! failing fails the pass.

use evento::{
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_catalog::{
    CatalogError, DraftCategory, FALLBACK_CATEGORY_SLUG, parse_amount, slugify_or,
};
use timada_proposals::aggregator::{Approved, Change};
use timada_proposals::{
    FIELD_BODY, FIELD_DESCRIPTION, FIELD_META_DESCRIPTION, FIELD_META_TITLE, FIELD_NAME,
    FIELD_PRICE, KIND_NEW_CATEGORY, KIND_PRODUCT, PROPOSALS_ACTOR, ProposalState, ProposalStatus,
    priced_variant, proposed_category_id,
};

use super::{Catalog, Proposals, REMOVED, Standing, Subject, permissions_of, standing, subject};
use crate::cluster::Executor;

/// The cursor identity of the subscription, in the store.
pub const SUBSCRIPTION: &str = "timada-dashboard/proposal-changes";

/// What the member reads when the shop was suspended before the change
/// was made.
const SUSPENDED: &str = "The shop is suspended.";

/// What the member reads of a kind nothing applies yet.
const NO_APPLIER: &str = "Nothing applies this kind of change yet.";

/// The store, to write with.
#[derive(Clone)]
struct Store(Executor);

pub fn subscription<E: evento::Executor>(store: Executor) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(Store(store))
        .handler(on_approved())
        // A saga: it follows the node's own work group (the executor's
        // default routing key).
        .retry(5)
}

/// The changes of `proposal` the subject does not hold yet.
fn left<'a>(held: &Subject, changes: &'a [Change]) -> Vec<&'a Change> {
    changes
        .iter()
        .filter(|change| held.value(&change.field) != Some(change.after.as_str()))
        .collect()
}

/// The value `field` ends at: the approved one when the proposal changes
/// it, what the subject holds otherwise.
fn ends_at<'a>(held: &'a Subject, changes: &'a [Change], field: &str) -> &'a str {
    changes
        .iter()
        .find(|change| change.field == field)
        .map(|change| change.after.as_str())
        .or_else(|| held.value(field))
        .unwrap_or_default()
}

/// Makes the changes of `proposal` its subject does not hold yet, as
/// `actor`. `held` is the subject as [`subject`] just read it, every
/// change [`Standing::Current`] against it.
pub async fn apply(
    catalog: &Catalog,
    actor: &str,
    proposal: &ProposalState,
    held: &Subject,
) -> Result<(), CatalogError> {
    let id = proposal.subject_id.as_str();
    let changes = proposal.changes.as_slice();
    let left = left(held, changes);
    let changes_field = |field: &str| left.iter().any(|change| change.field == field);
    if proposal.kind == KIND_NEW_CATEGORY {
        if left.is_empty() {
            return Ok(());
        }
        let name = ends_at(held, changes, FIELD_NAME);
        catalog
            .create_draft_category(
                actor,
                DraftCategory {
                    id: proposed_category_id(&proposal.id),
                    name: name.to_owned(),
                    slug: slugify_or(name, FALLBACK_CATEGORY_SLUG),
                    parent_id: (!id.is_empty()).then(|| id.to_owned()),
                    meta_title: ends_at(held, changes, FIELD_META_TITLE).to_owned(),
                    out_of_ads: false,
                    position: 0,
                },
            )
            .await?;
        return Ok(());
    }
    let product = proposal.kind == KIND_PRODUCT;
    for change in &left {
        let after = change.after.as_str();
        match (change.field.as_str(), product) {
            (FIELD_NAME, true) => catalog.rename_product(actor, id, after).await?,
            (FIELD_NAME, false) => catalog.rename_category(actor, id, after).await?,
            (FIELD_DESCRIPTION, true) => catalog.describe_product(actor, id, after).await?,
            (FIELD_DESCRIPTION, false) => catalog.describe_category(actor, id, after).await?,
            (FIELD_BODY, false) => catalog.write_category_body(actor, id, after).await?,
            (FIELD_PRICE, true) => {
                let currency = held
                    .currency
                    .as_deref()
                    .ok_or(CatalogError::InvalidCurrency)?;
                catalog
                    .price_product(actor, id, parse_amount(after)?, currency)
                    .await?
            }
            (field, true) if priced_variant(field).is_some() => {
                let variant_id = priced_variant(field).unwrap_or_default();
                let currency = held
                    .currency
                    .as_deref()
                    .ok_or(CatalogError::InvalidCurrency)?;
                catalog
                    .price_variant(actor, id, variant_id, parse_amount(after)?, currency)
                    .await?
            }
            // Written together, below.
            (FIELD_META_TITLE | FIELD_META_DESCRIPTION, _) => {}
            // No such field on this kind: the proposal would not hold it.
            _ => {}
        }
    }
    if changes_field(FIELD_META_TITLE) || changes_field(FIELD_META_DESCRIPTION) {
        let title = ends_at(held, changes, FIELD_META_TITLE);
        let description = ends_at(held, changes, FIELD_META_DESCRIPTION);
        if product {
            catalog
                .set_product_seo(actor, id, title, description)
                .await?;
        } else {
            catalog
                .set_category_seo(actor, id, title, description)
                .await?;
        }
    }
    // A name or a description the writer drafted is declared as written by
    // a machine, so the shopping feed says so, as Merchant Center asks.
    if product && proposal.source == timada_writer::WRITER_SOURCE {
        let names = changes.iter().any(|change| change.field == FIELD_NAME);
        let describes = changes
            .iter()
            .any(|change| change.field == FIELD_DESCRIPTION);
        if (names || describes)
            && let Some(now) = catalog.load_product(id).await?
        {
            catalog
                .declare_ai_writing(
                    actor,
                    id,
                    now.ai_title || names,
                    now.ai_description || describes,
                )
                .await?;
        }
    }
    Ok(())
}

#[evento::subscription]
async fn on_approved<E: evento::Executor>(
    context: &Context<'_, E>,
    event: Event<Approved>,
) -> anyhow::Result<()> {
    let store: Store = context.extract();
    settle(&store.0, &event.tenant, &event.aggregate_id).await
}

/// Applies the proposal `proposal_id` of the shop `tenant_id` if it is
/// approved and not settled yet, and records how it went.
pub(crate) async fn settle(
    store: &Executor,
    tenant_id: &str,
    proposal_id: &str,
) -> anyhow::Result<()> {
    let proposals = Proposals::new(store.clone(), tenant_id);
    let Some(proposal) = proposals.load_proposal(proposal_id).await? else {
        return Ok(());
    };
    // Another node settled it.
    if proposal.status != ProposalStatus::Approved {
        return Ok(());
    }
    let fail = async |reason: &str| -> anyhow::Result<()> {
        tracing::info!(tenant = %tenant_id, proposal = %proposal_id, %reason, "an approved change was not made");
        proposals
            .record_apply_failure(PROPOSALS_ACTOR, proposal_id, reason)
            .await?;
        Ok(())
    };
    if permissions_of(&proposal.kind).is_none() {
        return fail(NO_APPLIER).await;
    }
    let shop = timada_tenancy::Command(store.clone())
        .load_tenant(tenant_id)
        .await?;
    if shop.is_none_or(|shop| shop.is_suspended()) {
        return fail(SUSPENDED).await;
    }
    let catalog = Catalog::new(store.clone(), tenant_id);
    let held = subject(
        &catalog,
        &proposal.kind,
        &proposal.subject_id,
        Some(proposal_id),
    )
    .await?;
    let stale = match &held {
        None => Some((proposal.changes[0].field.clone(), REMOVED.to_owned())),
        Some(held) => match standing(held, &proposal.changes) {
            Standing::Current => None,
            Standing::OutOfDate { field, found } => Some((field, found)),
        },
    };
    if let Some((field, found)) = stale {
        tracing::info!(tenant = %tenant_id, proposal = %proposal_id, %field, "an approved change is out of date");
        proposals
            .record_out_of_date(PROPOSALS_ACTOR, proposal_id, &field, &found)
            .await?;
        return Ok(());
    }
    let held = held.expect("a subject that is there");
    match apply(&catalog, &proposal.answered_by, &proposal, &held).await {
        Ok(()) => {
            proposals
                .record_applied(PROPOSALS_ACTOR, proposal_id)
                .await?;
            tracing::info!(tenant = %tenant_id, proposal = %proposal_id, kind = %proposal.kind, "an approved change was made");
            Ok(())
        }
        Err(err) if err.is_store_fault() => Err(err.into()),
        Err(err) => fail(&format!("The shop refused it: {err}.")).await,
    }
}

#[cfg(test)]
mod tests {
    use evento::{Evento, Turso};
    use timada_catalog::CreateProduct;
    use timada_proposals::{KIND_PRODUCT, Prepare, ProposalError, SOURCE_PERSON};
    use timada_tenancy::CreateTenant;

    use super::*;
    use crate::auth::now;
    use crate::proposals::{Draft, Refusal, approve, prepare};

    const ALICE: &str = "user-alice";
    const BOB: &str = "user-bob";

    async fn store() -> Evento {
        Evento::new(Turso::in_memory_concurrent().await.unwrap())
    }

    /// A shop of Alice's; its id.
    async fn shop(store: &Evento) -> String {
        timada_tenancy::Command(store.clone())
            .create_tenant(CreateTenant {
                name: "Acme Moto".to_owned(),
                owner_user_id: ALICE.to_owned(),
            })
            .await
            .unwrap()
            .id
    }

    /// A product of the shop named "Gants", at 19.90 EUR; its id.
    async fn product(store: &Evento, shop: &str) -> String {
        let catalog = Catalog::new(store.clone(), shop);
        let product = catalog
            .create_product(
                ALICE,
                CreateProduct {
                    name: "Gants".to_owned(),
                    slug: None,
                },
            )
            .await
            .unwrap();
        catalog
            .price_product(ALICE, &product.id, 1990, "EUR")
            .await
            .unwrap();
        product.id
    }

    fn draft(kind: &str, subject_id: &str, values: &[(&str, &str)]) -> Draft {
        Draft {
            kind: kind.to_owned(),
            subject_id: subject_id.to_owned(),
            values: values
                .iter()
                .map(|(field, after)| ((*field).to_owned(), (*after).to_owned()))
                .collect(),
            reason: "Wording for the keyword.".to_owned(),
            source: SOURCE_PERSON.to_owned(),
            valid_until: None,
        }
    }

    async fn pass(store: &Evento) {
        subscription::<Evento>(store.clone())
            .retry(0)
            .run_once(store)
            .await
            .unwrap();
    }

    async fn proposal(store: &Evento, shop: &str, id: &str) -> ProposalState {
        Proposals::new(store.clone(), shop)
            .load_proposal(id)
            .await
            .unwrap()
            .unwrap()
    }

    /// Bob prepares, Alice approves, with what she changed.
    async fn approved(store: &Evento, shop: &str, draft: Draft, edits: &[(&str, &str)]) -> String {
        let id = prepare(store, shop, BOB, draft, now()).await.unwrap();
        let waiting = proposal(store, shop, &id).await;
        let edits = edits
            .iter()
            .map(|(field, after)| ((*field).to_owned(), (*after).to_owned()))
            .collect();
        approve(store, shop, ALICE, &waiting, edits, now())
            .await
            .unwrap();
        id
    }

    #[tokio::test]
    async fn approved_values_are_written_and_nothing_before() {
        let store = store().await;
        let shop = shop(&store).await;
        let product_id = product(&store, &shop).await;
        let catalog = Catalog::new(store.clone(), &shop);
        let id = prepare(
            &store,
            &shop,
            BOB,
            draft(
                KIND_PRODUCT,
                &product_id,
                &[
                    (FIELD_NAME, "  Gants moto été "),
                    (FIELD_DESCRIPTION, "Des gants légers."),
                    (FIELD_META_TITLE, "Gants moto été homologués"),
                    (FIELD_META_DESCRIPTION, ""),
                    (FIELD_PRICE, "24,5"),
                ],
            ),
            now(),
        )
        .await
        .unwrap();
        let waiting = proposal(&store, &shop, &id).await;
        // Recorded as the shop would keep them, against what it holds; the
        // field that would not change is left out.
        assert_eq!(waiting.subject_name, "Gants");
        assert_eq!(waiting.changes.len(), 4);
        assert_eq!(waiting.change(FIELD_NAME).unwrap().after, "Gants moto été");
        assert_eq!(waiting.change(FIELD_PRICE).unwrap().before, "19.90");
        assert_eq!(waiting.change(FIELD_PRICE).unwrap().after, "24.50");

        // Prepared is not applied: a pass finds nothing to do.
        pass(&store).await;
        let untouched = catalog.load_product(&product_id).await.unwrap().unwrap();
        assert_eq!(untouched.name, "Gants");
        assert_eq!(untouched.price.unwrap().amount_cents, 1990);

        approve(
            &store,
            &shop,
            ALICE,
            &waiting,
            vec![(FIELD_NAME.to_owned(), "Gants moto d'été".to_owned())],
            now(),
        )
        .await
        .unwrap();
        pass(&store).await;
        let changed = catalog.load_product(&product_id).await.unwrap().unwrap();
        assert_eq!(changed.name, "Gants moto d'été");
        assert_eq!(changed.slug, "gants-moto-d-ete");
        assert_eq!(changed.description, "Des gants légers.");
        assert_eq!(changed.meta_title, "Gants moto été homologués");
        assert_eq!(changed.price.unwrap().amount_cents, 2450);
        let applied = proposal(&store, &shop, &id).await;
        assert_eq!(applied.status, ProposalStatus::Applied);
        assert_eq!(applied.answered_by, ALICE);
        assert!(applied.edited);

        // Delivered again: nothing moves.
        let version = changed.aggregate_version;
        settle(&store, &shop, &id).await.unwrap();
        let again = catalog.load_product(&product_id).await.unwrap().unwrap();
        assert_eq!(again.aggregate_version, version);
    }

    #[tokio::test]
    async fn a_value_the_shop_would_refuse_is_refused_when_prepared() {
        let store = store().await;
        let shop = shop(&store).await;
        let product_id = product(&store, &shop).await;
        let refused = |values: &'static [(&'static str, &'static str)]| {
            let store = store.clone();
            let shop = shop.clone();
            let product_id = product_id.clone();
            async move {
                prepare(
                    &store,
                    &shop,
                    BOB,
                    draft(KIND_PRODUCT, &product_id, values),
                    now(),
                )
                .await
                .unwrap_err()
                .code()
            }
        };
        assert_eq!(
            refused(&[(FIELD_NAME, " ")]).await,
            Some("invalid_product_name")
        );
        assert_eq!(
            refused(&[(FIELD_PRICE, "cheap")]).await,
            Some("invalid_amount")
        );
        assert_eq!(
            refused(&[(FIELD_NAME, "Gants")]).await,
            Some("proposal_nothing_to_change")
        );
        // A product has no text below its products, and a variant it does
        // not have has no price.
        assert_eq!(
            refused(&[(FIELD_BODY, "Texte")]).await,
            Some("proposal_invalid_changes")
        );
        assert_eq!(
            refused(&[("price:nope", "12")]).await,
            Some("proposal_invalid_changes")
        );
        let missing = prepare(
            &store,
            &shop,
            BOB,
            draft(KIND_PRODUCT, "nope", &[(FIELD_NAME, "Gants moto")]),
            now(),
        )
        .await;
        assert!(matches!(missing, Err(Refusal::SubjectMissing)));
        let ads = prepare(
            &store,
            &shop,
            BOB,
            draft(
                timada_proposals::KIND_ADS_BID,
                "campaign-1",
                &[("max_cpc", "0.40")],
            ),
            now(),
        )
        .await;
        assert!(matches!(ads, Err(Refusal::NoApplier)));
    }

    #[tokio::test]
    async fn a_target_changed_meanwhile_is_not_written_over() {
        let store = store().await;
        let shop = shop(&store).await;
        let product_id = product(&store, &shop).await;
        let catalog = Catalog::new(store.clone(), &shop);

        // Changed between the approval and the pass.
        let late = approved(
            &store,
            &shop,
            draft(KIND_PRODUCT, &product_id, &[(FIELD_NAME, "Gants moto")]),
            &[],
        )
        .await;
        catalog
            .rename_product(ALICE, &product_id, "Gants cuir")
            .await
            .unwrap();
        pass(&store).await;
        let ended = proposal(&store, &shop, &late).await;
        assert_eq!(ended.status, ProposalStatus::OutOfDate);
        assert_eq!(ended.note, "name: Gants cuir");
        let product = catalog.load_product(&product_id).await.unwrap().unwrap();
        assert_eq!(product.name, "Gants cuir");

        // Changed before the member answers: they are told, and it ends.
        let id = prepare(
            &store,
            &shop,
            BOB,
            draft(
                KIND_PRODUCT,
                &product_id,
                &[(FIELD_NAME, "Gants cuir noir")],
            ),
            now(),
        )
        .await
        .unwrap();
        catalog
            .rename_product(ALICE, &product_id, "Gants cuir marron")
            .await
            .unwrap();
        let waiting = proposal(&store, &shop, &id).await;
        let answered = approve(&store, &shop, ALICE, &waiting, vec![], now()).await;
        assert!(matches!(answered, Err(Refusal::OutOfDate)));
        assert_eq!(
            proposal(&store, &shop, &id).await.status,
            ProposalStatus::OutOfDate
        );
        // Answered: not approved a second time.
        let again = approve(&store, &shop, ALICE, &waiting, vec![], now()).await;
        assert!(matches!(
            again,
            Err(Refusal::Proposal(ProposalError::NotWaiting)) | Err(Refusal::OutOfDate)
        ));
        pass(&store).await;
        let product = catalog.load_product(&product_id).await.unwrap().unwrap();
        assert_eq!(product.name, "Gants cuir marron");
    }

    /// A node stopped after one of two fields: the next pass makes the
    /// other and says so.
    #[tokio::test]
    async fn a_change_made_in_part_is_finished() {
        let store = store().await;
        let shop = shop(&store).await;
        let product_id = product(&store, &shop).await;
        let catalog = Catalog::new(store.clone(), &shop);
        let id = approved(
            &store,
            &shop,
            draft(
                KIND_PRODUCT,
                &product_id,
                &[
                    (FIELD_NAME, "Gants moto"),
                    (FIELD_DESCRIPTION, "Des gants."),
                ],
            ),
            &[],
        )
        .await;
        catalog
            .describe_product(ALICE, &product_id, "Des gants.")
            .await
            .unwrap();
        pass(&store).await;
        let product = catalog.load_product(&product_id).await.unwrap().unwrap();
        assert_eq!(product.name, "Gants moto");
        assert_eq!(product.description, "Des gants.");
        assert_eq!(
            proposal(&store, &shop, &id).await.status,
            ProposalStatus::Applied
        );
    }

    /// What the writer drafted is declared as machine-written once it is
    /// applied; what a member wrote is not.
    #[tokio::test]
    async fn a_text_the_writer_drafted_is_declared_as_such() {
        let store = store().await;
        let shop = shop(&store).await;
        let product_id = product(&store, &shop).await;
        let catalog = Catalog::new(store.clone(), &shop);
        approved(
            &store,
            &shop,
            draft(
                KIND_PRODUCT,
                &product_id,
                &[(FIELD_META_TITLE, "Gants moto")],
            ),
            &[],
        )
        .await;
        pass(&store).await;
        let product = catalog.load_product(&product_id).await.unwrap().unwrap();
        assert!(!product.ai_title && !product.ai_description);

        let mut drafted = draft(KIND_PRODUCT, &product_id, &[(FIELD_NAME, "Gants moto été")]);
        drafted.source = timada_writer::WRITER_SOURCE.to_owned();
        approved(&store, &shop, drafted, &[]).await;
        pass(&store).await;
        let product = catalog.load_product(&product_id).await.unwrap().unwrap();
        assert_eq!(product.name, "Gants moto été");
        assert!(product.ai_title);
        assert!(!product.ai_description);

        // A drafted description joins the name.
        let mut drafted = draft(
            KIND_PRODUCT,
            &product_id,
            &[(FIELD_DESCRIPTION, "Des gants légers.")],
        );
        drafted.source = timada_writer::WRITER_SOURCE.to_owned();
        approved(&store, &shop, drafted, &[]).await;
        pass(&store).await;
        let product = catalog.load_product(&product_id).await.unwrap().unwrap();
        assert!(product.ai_title && product.ai_description);
    }

    #[tokio::test]
    async fn a_category_is_created_once_as_a_draft() {
        let store = store().await;
        let shop = shop(&store).await;
        let catalog = Catalog::new(store.clone(), &shop);
        let new = || {
            draft(
                KIND_NEW_CATEGORY,
                "",
                &[
                    (FIELD_NAME, "Gants moto"),
                    (FIELD_META_TITLE, "Gants moto homologués"),
                ],
            )
        };
        let id = approved(&store, &shop, new(), &[]).await;
        assert_eq!(proposal(&store, &shop, &id).await.subject_name, "Top level");
        pass(&store).await;
        settle(&store, &shop, &id).await.unwrap();
        let category = catalog
            .load_category(&proposed_category_id(&id))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(category.name, "Gants moto");
        assert_eq!(category.slug, "gants-moto");
        assert_eq!(category.meta_title, "Gants moto homologués");
        assert!(category.draft);
        assert_eq!(category.parent_id, None);
        assert_eq!(
            proposal(&store, &shop, &id).await.status,
            ProposalStatus::Applied
        );

        // A child under it; and the same name again, whose address is
        // taken: the shop's refusal is the proposal's outcome.
        let child = approved(
            &store,
            &shop,
            draft(
                KIND_NEW_CATEGORY,
                &category.id,
                &[(FIELD_NAME, "Gants été")],
            ),
            &[],
        )
        .await;
        let twin = approved(&store, &shop, new(), &[]).await;
        pass(&store).await;
        let under = catalog
            .load_category(&proposed_category_id(&child))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(under.parent_id.as_deref(), Some(category.id.as_str()));
        assert_eq!(
            proposal(&store, &shop, &child).await.subject_name,
            "Gants moto"
        );
        let failed = proposal(&store, &shop, &twin).await;
        assert_eq!(failed.status, ProposalStatus::Failed);
        assert!(
            failed.note.starts_with("The shop refused it:"),
            "{}",
            failed.note
        );
    }

    #[tokio::test]
    async fn a_suspended_shop_or_a_kind_nothing_applies_fails_the_change() {
        let store = store().await;
        let shop = shop(&store).await;
        let product_id = product(&store, &shop).await;
        let id = approved(
            &store,
            &shop,
            draft(KIND_PRODUCT, &product_id, &[(FIELD_NAME, "Gants moto")]),
            &[],
        )
        .await;
        timada_tenancy::Command(store.clone())
            .suspend_tenant(&shop, "unpaid")
            .await
            .unwrap();
        pass(&store).await;
        let failed = proposal(&store, &shop, &id).await;
        assert_eq!(failed.status, ProposalStatus::Failed);
        assert_eq!(failed.note, SUSPENDED);
        let product = Catalog::new(store.clone(), &shop)
            .load_product(&product_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(product.name, "Gants");

        // An ads change, approved on the stream itself: nothing makes it.
        let proposals = Proposals::new(store.clone(), &shop);
        let bid = proposals
            .prepare(
                "rule:bids",
                Prepare {
                    kind: timada_proposals::KIND_ADS_BID.to_owned(),
                    subject_id: "campaign-1".to_owned(),
                    subject_name: "Campaign".to_owned(),
                    changes: vec![Change {
                        field: "max_cpc".to_owned(),
                        before: "0.30".to_owned(),
                        after: "0.40".to_owned(),
                    }],
                    reason: "ROAS 4.1 over 30 days.".to_owned(),
                    source: "bid rule".to_owned(),
                    valid_until: None,
                },
                now(),
            )
            .await
            .unwrap();
        let waiting = proposal(&store, &shop, &bid).await;
        let refused = approve(&store, &shop, ALICE, &waiting, vec![], now()).await;
        assert!(matches!(refused, Err(Refusal::NoApplier)));
        proposals.approve(ALICE, &bid, vec![], now()).await.unwrap();
        settle(&store, &shop, &bid).await.unwrap();
        let failed = proposal(&store, &shop, &bid).await;
        assert_eq!(failed.status, ProposalStatus::Failed);
        assert_eq!(failed.note, NO_APPLIER);
    }
}
