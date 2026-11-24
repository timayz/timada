//! Drafts the texts a member asked for: one paid call to the writer per
//! product or category, made off the page that asked, and prepared as a
//! proposal that waits for a member. Every node of the work group runs it;
//! the first to take a draft on makes the call, the others leave it.
//!
//! Room is held for the call in the day's spending (`timada-writer`'s
//! `SpendDay`), which every node writes: a call that would pass the shop's
//! cap or the platform's is not made, and the draft fails saying which cap
//! and when it frees.
//!
//! What comes back is checked against what the shop holds of the subject
//! ([`timada_writer::facts`]): a figure, a material or a claim the data
//! does not hold is listed on the proposal, for whoever reads it. Nothing
//! is published here: approving the proposal is what writes the product.
//!
//! A failure of the writer is the draft's outcome, not the pass's: it is
//! recorded on the draft with its reason, where the member reads it and
//! asks again. Only the store failing fails the pass. A node that stops
//! between taking a draft on and recording it leaves it there, and the
//! member may ask again once it is ten minutes old.

use evento::{
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_catalog::{
    MAX_CATEGORY_DESCRIPTION_LEN, MAX_DESCRIPTION_LEN, MAX_NAME_LEN, ProductFilter,
    categories_of_tenant, products_of_tenant,
};
use timada_core::page::PageFrom;
use timada_core::seo::{MAX_META_DESCRIPTION_LEN, MAX_META_TITLE_LEN};
use timada_proposals::{
    FIELD_DESCRIPTION, FIELD_META_DESCRIPTION, FIELD_META_TITLE, FIELD_NAME, KIND_CATEGORY,
    KIND_PRODUCT,
};
use timada_writer::aggregator::Requested;
use timada_writer::{
    Call, DRAFT_STALE_SECS, Room, Spend, SpendCap, WRITER_ACTOR, WRITER_SOURCE, facts::unfounded,
};

use super::{Ask, Limits, Spending, Texts, Writers};
use crate::cluster::Executor;
use crate::db::Db;
use crate::proposals::{Catalog, Draft, Refusal};
use crate::ui::format_time;

/// The cursor identity of the subscription, in the store.
pub const SUBSCRIPTION: &str = "timada-dashboard/text-drafts";

/// A request older than this is history being replayed (a node catching
/// up): left alone, as the member may ask again from then on.
const REQUEST_WINDOW_SECS: u64 = DRAFT_STALE_SECS;

/// How many of a category's products are named to the writer.
const PRODUCTS_NAMED: i64 = 12;

/// What the member reads when no writer is set.
pub const NO_WRITER: &str = "Text drafts are not set up on this platform.";

/// What the member reads when the shop is gone or suspended.
const NO_SHOP: &str = "The shop is suspended.";

/// What the member reads when what the draft was about is gone.
const NO_SUBJECT: &str = "What the draft was about is no longer there.";

/// The store, to write with.
#[derive(Clone)]
struct Store(Executor);

pub fn subscription<E: evento::Executor>(
    writers: Writers,
    store: Executor,
    db: Db,
    spending: Spending,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(writers)
        .data(Store(store))
        .data(db)
        .data(spending)
        .handler(on_requested())
        // A saga: it follows the node's own work group (the executor's
        // default routing key).
        .retry(5)
}

fn is_history(now: u64, requested_at: u64) -> bool {
    now.saturating_sub(requested_at) >= REQUEST_WINDOW_SECS
}

/// What the member reads when a cap keeps the call from being made.
pub fn cap_reached(cap: SpendCap, frees_at: u64) -> String {
    let whose = match cap {
        SpendCap::Shop => "The shop's",
        SpendCap::Platform => "The platform's",
    };
    format!(
        "{whose} budget for text drafts is spent for today. It frees at {}.",
        format_time(frees_at)
    )
}

/// Why the proposal of a draft is made, for whoever reads it: the search
/// it was written for, and what to check in it.
pub fn reason_of(keyword: &str, to_check: &[String]) -> String {
    let mut reason = format!("Drafted by the writer for the search “{keyword}”.");
    if !to_check.is_empty() {
        reason.push_str(" To check, not found in the shop's data: ");
        reason.push_str(&to_check.join(", "));
        reason.push('.');
    }
    reason
}

/// What a draft of the shop `tenant_id` is asked from: the subject as the
/// shop holds it now; `None` when it is not there any more.
pub async fn gather(
    store: &Executor,
    db: &Db,
    tenant_id: &str,
    request: &Requested,
) -> anyhow::Result<Option<Ask>> {
    let catalog = Catalog::new(store.clone(), tenant_id);
    let language = timada_pages::Command::new(store.clone(), tenant_id)
        .languages()
        .await?
        .map(|languages| languages.main)
        .unwrap_or_default();
    let brief = timada_writer::Command::new(store.clone(), tenant_id)
        .load_brief()
        .await?;
    let keyword = |fallback: &str| {
        if request.keyword.is_empty() {
            fallback.to_owned()
        } else {
            request.keyword.clone()
        }
    };
    if request.kind == KIND_PRODUCT {
        let Some(product) = catalog
            .load_product(&request.subject_id)
            .await?
            .filter(|product| !product.is_archived())
        else {
            return Ok(None);
        };
        let mut facts: Vec<(String, String)> = Vec::new();
        if let Some(brand) = &product.brand {
            facts.push(("Brand".to_owned(), brand.clone()));
        }
        facts.extend(
            product
                .attributes
                .iter()
                .map(|attribute| (attribute.name.clone(), attribute.value.clone())),
        );
        // What it comes in: the values a shopper reads.
        facts.extend(product.options.iter().map(|option| {
            let values: Vec<&str> = option
                .values
                .iter()
                .map(|value| option.value_labels.get(value).unwrap_or(value).as_str())
                .collect();
            (
                option.label.clone().unwrap_or_else(|| option.name.clone()),
                values.join(", "),
            )
        }));
        return Ok(Some(Ask {
            product: true,
            language,
            keyword: keyword(&product.name),
            now: Texts {
                name: product.name.clone(),
                description: product.description.clone(),
                meta_title: product.meta_title.clone(),
                meta_description: product.meta_description.clone(),
            },
            facts,
            holds: Vec::new(),
            tone: brief.tone,
            never: brief.never,
            limits: Limits {
                name: MAX_NAME_LEN,
                description: MAX_DESCRIPTION_LEN,
                meta_title: MAX_META_TITLE_LEN,
                meta_description: MAX_META_DESCRIPTION_LEN,
            },
        }));
    }
    let Some(category) = catalog
        .load_category(&request.subject_id)
        .await?
        .filter(|category| !category.archived)
    else {
        return Ok(None);
    };
    // What it holds: the categories right below it, then some of its
    // products. Read from the read model, a moment behind: a category
    // created just now may be missing, which only makes the draft plainer.
    let mut holds: Vec<String> = categories_of_tenant(db, tenant_id)
        .await?
        .into_iter()
        .filter(|row| !row.archived && row.parent_id.as_deref() == Some(category.id.as_str()))
        .map(|row| row.name)
        .collect();
    let filter = ProductFilter::default().category(Some(&category.id));
    let products =
        products_of_tenant(db, tenant_id, &filter, PageFrom::First, PRODUCTS_NAMED).await?;
    holds.extend(products.rows.into_iter().map(|row| row.name));
    Ok(Some(Ask {
        product: false,
        language,
        keyword: keyword(&category.name),
        now: Texts {
            name: category.name.clone(),
            description: category.description.clone(),
            meta_title: category.meta_title.clone(),
            meta_description: category.meta_description.clone(),
        },
        facts: Vec::new(),
        holds,
        tone: brief.tone,
        never: brief.never,
        limits: Limits {
            name: 0,
            description: MAX_CATEGORY_DESCRIPTION_LEN,
            meta_title: MAX_META_TITLE_LEN,
            meta_description: MAX_META_DESCRIPTION_LEN,
        },
    }))
}

/// The fields a draft's proposal would write: the texts the writer gave,
/// the empty ones left out; a category's name is never among them.
fn values_of(product: bool, texts: &Texts) -> Vec<(String, String)> {
    [
        (FIELD_NAME, &texts.name, product),
        (FIELD_DESCRIPTION, &texts.description, true),
        (FIELD_META_TITLE, &texts.meta_title, true),
        (FIELD_META_DESCRIPTION, &texts.meta_description, true),
    ]
    .into_iter()
    .filter(|(_, text, asked)| *asked && !text.trim().is_empty())
    .map(|(field, text, _)| (field.to_owned(), text.clone()))
    .collect()
}

#[evento::subscription]
async fn on_requested<E: evento::Executor>(
    context: &Context<'_, E>,
    event: Event<Requested>,
) -> anyhow::Result<()> {
    let writers: Writers = context.extract();
    let store: Store = context.extract();
    let db: Db = context.extract();
    let spending: Spending = context.extract();
    let now = crate::auth::now();
    if is_history(now, event.timestamp) {
        return Ok(());
    }
    let store = store.0;
    let tenant_id = event.tenant.as_str();
    let draft_id = event.aggregate_id.as_str();
    let request = &event.data;
    let drafts = timada_writer::Command::new(store.clone(), tenant_id);
    if !drafts.start_draft(WRITER_ACTOR, draft_id).await? {
        return Ok(());
    }
    let fail = async |reason: &str| -> anyhow::Result<()> {
        tracing::info!(tenant = %tenant_id, draft = %draft_id, %reason, "texts not drafted");
        drafts
            .record_draft_failure(WRITER_ACTOR, draft_id, reason)
            .await?;
        Ok(())
    };
    let Some(writer) = &writers.0 else {
        return fail(NO_WRITER).await;
    };
    let shop = timada_tenancy::Command(store.clone())
        .load_tenant(tenant_id)
        .await?;
    if shop.is_none_or(|shop| shop.is_suspended()) {
        return fail(NO_SHOP).await;
    }
    let Some(ask) = gather(&store, &db, tenant_id, request).await? else {
        return fail(NO_SUBJECT).await;
    };
    // The call is paid by the platform and counted to the shop it is for.
    let spend = Spend(store.clone());
    let room = spend
        .hold_spend(
            WRITER_ACTOR,
            Call {
                tenant_id,
                draft_id,
                most_usd_micros: writer.most_usd_micros(),
            },
            spending.caps,
            now,
        )
        .await?;
    let hold = match room {
        Room::Held(hold) => hold,
        Room::Refused { cap, frees_at } => return fail(&cap_reached(cap, frees_at)).await,
    };
    let answer = match writer.write(&ask).await {
        Ok(answer) => answer,
        Err(failed) => {
            // A call that gave nothing may have been paid for all the same.
            if failed.cost_usd_micros > 0 {
                spend
                    .charge_spend(WRITER_ACTOR, &hold, failed.cost_usd_micros)
                    .await?;
            } else {
                spend.release_spend(WRITER_ACTOR, &hold).await?;
            }
            return fail(&failed.fault.to_string()).await;
        }
    };
    // The charge first: what was paid counts whatever becomes of the texts.
    spend
        .charge_spend(WRITER_ACTOR, &hold, answer.cost_usd_micros)
        .await?;
    let to_check = unfounded(&answer.texts.all(), &ask.source());
    let kind = if ask.product {
        KIND_PRODUCT
    } else {
        KIND_CATEGORY
    };
    let prepared = crate::proposals::prepare(
        &store,
        tenant_id,
        &request.requested_by,
        Draft {
            kind: kind.to_owned(),
            subject_id: request.subject_id.clone(),
            values: values_of(ask.product, &answer.texts),
            reason: reason_of(&ask.keyword, &to_check),
            source: WRITER_SOURCE.to_owned(),
            valid_until: None,
        },
        now,
    )
    .await;
    match prepared {
        Ok(proposal_id) => {
            drafts
                .record_draft(
                    WRITER_ACTOR,
                    draft_id,
                    &proposal_id,
                    answer.cost_usd_micros,
                    to_check,
                )
                .await?;
            tracing::info!(tenant = %tenant_id, draft = %draft_id, proposal = %proposal_id, cost_usd_micros = answer.cost_usd_micros, "texts drafted");
            Ok(())
        }
        // The store, not the draft: the pass fails and is tried again.
        Err(Refusal::Other(err)) => Err(err),
        Err(Refusal::Catalog(err)) if err.is_store_fault() => Err(err.into()),
        Err(Refusal::Proposal(err)) if err.is_store_fault() => Err(err.into()),
        Err(refusal) => fail(&format!("The draft was not kept: {refusal}.")).await,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use evento::{Evento, Turso};
    use timada_catalog::{CreateCategory, CreateProduct};
    use timada_proposals::ProposalStatus;
    use timada_tenancy::CreateTenant;
    use timada_writer::{DraftState, DraftStatus, RequestDraft, SpendCaps, spend_day};

    use super::super::fake::{COST_USD_MICROS, FakeWriter, MOST_USD_MICROS};
    use super::super::{TextWriter, WriterFault};
    use super::*;
    use crate::proposals::Proposals;

    const ALICE: &str = "user-alice";

    const ROOMY: Spending = Spending {
        caps: SpendCaps {
            shop_usd_micros: 5_000_000,
            platform_usd_micros: 50_000_000,
        },
    };

    /// A store with the read models' tables, and Alice's shop in it.
    async fn shop() -> (Turso, Evento, String) {
        let turso = Turso::in_memory_concurrent().await.unwrap();
        crate::db::migrate_store_read_models(&turso).await.unwrap();
        let store = Evento::new(turso.clone());
        let shop = timada_tenancy::Command(store.clone())
            .create_tenant(CreateTenant {
                name: "Acme Moto".to_owned(),
                owner_user_id: ALICE.to_owned(),
            })
            .await
            .unwrap()
            .id;
        (turso, store, shop)
    }

    /// A product named "Gants" with a fact, and the shop's brief.
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
            .describe_product(ALICE, &product.id, "Gants en cuir, longueur 30 cm.")
            .await
            .unwrap();
        timada_writer::Command::new(store.clone(), shop)
            .set_brief(ALICE, "Direct.", "Jamais « le meilleur ».")
            .await
            .unwrap();
        product.id
    }

    async fn requested(store: &Evento, shop: &str, kind: &str, subject_id: &str) -> String {
        timada_writer::Command::new(store.clone(), shop)
            .request_draft(
                ALICE,
                RequestDraft {
                    kind: kind.to_owned(),
                    subject_id: subject_id.to_owned(),
                    subject_name: "Gants".to_owned(),
                    keyword: "gants moto été".to_owned(),
                },
            )
            .await
            .unwrap()
    }

    async fn pass(
        turso: &Turso,
        store: &Evento,
        writer: Option<Arc<FakeWriter>>,
        spending: Spending,
    ) {
        let writers = Writers(writer.map(|writer| writer as Arc<dyn TextWriter>));
        subscription::<Evento>(writers, store.clone(), Db::from(turso.clone()), spending)
            .retry(0)
            .run_once(store)
            .await
            .unwrap();
    }

    async fn draft(store: &Evento, shop: &str, id: &str) -> DraftState {
        timada_writer::Command::new(store.clone(), shop)
            .load_draft(id)
            .await
            .unwrap()
            .unwrap()
    }

    #[tokio::test]
    async fn a_draft_asked_for_waits_as_a_proposal_and_is_paid_once() {
        let (turso, store, shop) = shop().await;
        let product_id = product(&store, &shop).await;
        let id = requested(&store, &shop, KIND_PRODUCT, &product_id).await;
        let writer = Arc::new(FakeWriter::default());
        pass(&turso, &store, Some(writer.clone()), ROOMY).await;
        // Another node sees the request too: taken on already.
        pass(&turso, &store, Some(writer.clone()), ROOMY).await;
        assert_eq!(writer.calls(), 1);

        let asked = writer.asked.lock().unwrap()[0].clone();
        assert!(asked.product);
        assert_eq!(asked.keyword, "gants moto été");
        assert_eq!(asked.now.name, "Gants");
        assert_eq!(asked.tone, "Direct.");
        assert_eq!(asked.never, "Jamais « le meilleur ».");
        assert_eq!(asked.limits.meta_title, 70);

        let drafted = draft(&store, &shop, &id).await;
        assert_eq!(drafted.status, DraftStatus::Drafted);
        assert_eq!(drafted.cost_usd_micros, COST_USD_MICROS);
        assert!(drafted.to_check.is_empty());
        let proposal = Proposals::new(store.clone(), &shop)
            .load_proposal(&drafted.proposal_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(proposal.status, ProposalStatus::Waiting);
        assert_eq!(proposal.source, WRITER_SOURCE);
        assert_eq!(proposal.prepared_by, ALICE);
        assert_eq!(proposal.kind, KIND_PRODUCT);
        assert_eq!(
            proposal.change(FIELD_NAME).unwrap().after,
            "gants moto été drafted"
        );
        assert_eq!(proposal.changes.len(), 4);
        assert_eq!(
            proposal.reason,
            "Drafted by the writer for the search “gants moto été”."
        );
        // Nothing was published.
        let product = Catalog::new(store.clone(), &shop)
            .load_product(&product_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(product.name, "Gants");

        let day = Spend(store.clone())
            .load_spend_day(&spend_day(crate::auth::now()))
            .await
            .unwrap();
        assert_eq!(day.charged_to(&shop).usd_micros, COST_USD_MICROS);
        assert_eq!(day.charged_to(&shop).calls, 1);
        assert!(day.holds.is_empty());
    }

    #[tokio::test]
    async fn what_a_draft_states_beyond_the_data_is_listed_to_check() {
        let (turso, store, shop) = shop().await;
        let product_id = product(&store, &shop).await;
        let id = requested(&store, &shop, KIND_PRODUCT, &product_id).await;
        let writer = Arc::new(FakeWriter::writing(Texts {
            name: "Gants moto été".to_owned(),
            // The data holds 30 cm and leather; not 2 years, not waterproof.
            description: "Gants en cuir de 30 cm, étanches, garantis 2 ans.".to_owned(),
            ..Texts::default()
        }));
        pass(&turso, &store, Some(writer), ROOMY).await;
        let drafted = draft(&store, &shop, &id).await;
        assert_eq!(drafted.to_check, ["2 ans", "étanches", "garantis"]);
        let proposal = Proposals::new(store.clone(), &shop)
            .load_proposal(&drafted.proposal_id)
            .await
            .unwrap()
            .unwrap();
        // The texts the writer left empty are not part of the proposal.
        assert_eq!(proposal.changes.len(), 2);
        assert!(
            proposal
                .reason
                .ends_with("To check, not found in the shop's data: 2 ans, étanches, garantis."),
            "{}",
            proposal.reason
        );
    }

    #[tokio::test]
    async fn a_category_is_drafted_without_its_name() {
        let (turso, store, shop) = shop().await;
        let category = Catalog::new(store.clone(), &shop)
            .create_category(
                ALICE,
                CreateCategory {
                    name: "Gants moto".to_owned(),
                    ..CreateCategory::default()
                },
            )
            .await
            .unwrap();
        let id = requested(&store, &shop, KIND_CATEGORY, &category.id).await;
        let writer = Arc::new(FakeWriter::default());
        pass(&turso, &store, Some(writer.clone()), ROOMY).await;
        let asked = writer.asked.lock().unwrap()[0].clone();
        assert!(!asked.product);
        assert_eq!(asked.now.name, "Gants moto");
        let drafted = draft(&store, &shop, &id).await;
        let proposal = Proposals::new(store.clone(), &shop)
            .load_proposal(&drafted.proposal_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(proposal.kind, KIND_CATEGORY);
        assert!(proposal.change(FIELD_NAME).is_none());
        assert_eq!(proposal.changes.len(), 3);
    }

    #[tokio::test]
    async fn a_draft_that_cannot_be_made_says_why_and_costs_nothing() {
        let (turso, store, shop) = shop().await;
        let product_id = product(&store, &shop).await;
        let day = || async {
            Spend(store.clone())
                .load_spend_day(&spend_day(crate::auth::now()))
                .await
                .unwrap()
        };

        // No writer set.
        let off = requested(&store, &shop, KIND_PRODUCT, &product_id).await;
        pass(&turso, &store, None, ROOMY).await;
        assert_eq!(draft(&store, &shop, &off).await.reason, NO_WRITER);

        // No room under the shop's cap: the writer is not asked.
        let capped = requested(&store, &shop, KIND_PRODUCT, &product_id).await;
        let writer = Arc::new(FakeWriter::default());
        let tight = Spending {
            caps: SpendCaps {
                shop_usd_micros: MOST_USD_MICROS - 1,
                platform_usd_micros: 50_000_000,
            },
        };
        pass(&turso, &store, Some(writer.clone()), tight).await;
        assert_eq!(writer.calls(), 0);
        let failed = draft(&store, &shop, &capped).await;
        assert_eq!(failed.status, DraftStatus::Failed);
        assert!(
            failed
                .reason
                .starts_with("The shop's budget for text drafts is spent for today."),
            "{}",
            failed.reason
        );

        // The writer does not answer: the room held is given back.
        let down = requested(&store, &shop, KIND_PRODUCT, &product_id).await;
        let writer = Arc::new(FakeWriter::failing(WriterFault::Unavailable(
            "Overloaded".to_owned(),
        )));
        pass(&turso, &store, Some(writer), ROOMY).await;
        assert_eq!(
            draft(&store, &shop, &down).await.reason,
            "The writer did not answer: Overloaded"
        );
        assert!(day().await.holds.is_empty());
        assert_eq!(day().await.charged(), 0);

        // A text the shop would refuse: paid, and not kept.
        let long = requested(&store, &shop, KIND_PRODUCT, &product_id).await;
        let writer = Arc::new(FakeWriter::writing(Texts {
            name: "x".repeat(200),
            ..Texts::default()
        }));
        pass(&turso, &store, Some(writer), ROOMY).await;
        let failed = draft(&store, &shop, &long).await;
        assert!(
            failed.reason.starts_with("The draft was not kept:"),
            "{}",
            failed.reason
        );
        assert_eq!(day().await.charged(), COST_USD_MICROS);

        // What it was about is gone.
        let gone = requested(&store, &shop, KIND_PRODUCT, "nope").await;
        pass(&turso, &store, Some(Arc::new(FakeWriter::default())), ROOMY).await;
        assert_eq!(draft(&store, &shop, &gone).await.reason, NO_SUBJECT);
    }
}
