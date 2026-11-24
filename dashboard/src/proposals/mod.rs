//! What a proposal means on this host: which of the shop's things a kind
//! names, what its fields hold now, and the commands that write them.
//!
//! `timada-proposals` keeps a proposal as a kind, an id and field names.
//! Here a kind is tied to the catalog: [`subject`] reads what a product or
//! a category holds, [`standing`] says whether a proposal can still apply
//! to it, and [`apply`](apply::apply) makes the change through the
//! commands the product and category pages use, so an approved change
//! meets the checks a typed one does. The kinds about ads are carried, and
//! nothing applies them yet.
//!
//! A value is kept as a person reads and types it: a text as it is, a
//! price as `19.90` in the product's currency. It is normalized before it
//! is recorded ([`normalized`]), so what a proposal says it would write is
//! what the shop holds once it is applied, and a proposal applied twice is
//! told from one whose target changed.

pub mod apply;
pub mod notices;

use std::collections::BTreeMap;

use timada_catalog::{
    CatalogError, CategoryState, ProductState, normalize_category_body,
    normalize_category_description, normalize_category_name, normalize_description, normalize_name,
    normalize_seo, parse_amount,
};
use timada_core::money::format_amount;
use timada_proposals::aggregator::Change;
use timada_proposals::{
    CATEGORY_FIELDS, FIELD_BODY, FIELD_DESCRIPTION, FIELD_META_DESCRIPTION, FIELD_META_TITLE,
    FIELD_NAME, FIELD_PRICE, KIND_CATEGORY, KIND_NEW_CATEGORY, KIND_PRODUCT, NEW_CATEGORY_FIELDS,
    PRODUCT_FIELDS, Prepare, ProposalError, ProposalState, ProposalStatus, priced_variant,
    proposed_category_id, variant_price_field,
};
use timada_tenancy::{PERMISSION_PRODUCTS_MANAGE, PERMISSION_PRODUCTS_READ};

use crate::cluster::Executor;

/// The catalog of one shop.
pub type Catalog = timada_catalog::Command<Executor>;
/// The proposals of one shop.
pub type Proposals = timada_proposals::Command<Executor>;

/// What a category to create at the top level is listed under.
pub const TOP_LEVEL: &str = "Top level";

/// What a field holds when the variant it prices is no longer there.
const REMOVED: &str = "(removed)";

/// The permissions a change of `kind` needs over the proposal's own: to
/// see its subject (to prepare one) and to change it (to approve one).
/// `None` for a kind nothing applies yet.
pub fn permissions_of(kind: &str) -> Option<(&'static str, &'static str)> {
    match kind {
        KIND_PRODUCT | KIND_CATEGORY | KIND_NEW_CATEGORY => {
            Some((PERMISSION_PRODUCTS_READ, PERMISSION_PRODUCTS_MANAGE))
        }
        _ => None,
    }
}

/// The fields a proposal of `kind` may be prepared with by hand, in the
/// order a form shows them.
pub fn fields_of(kind: &str) -> &'static [&'static str] {
    match kind {
        KIND_PRODUCT => &PRODUCT_FIELDS,
        KIND_CATEGORY => &CATEGORY_FIELDS,
        KIND_NEW_CATEGORY => &NEW_CATEGORY_FIELDS,
        _ => &[],
    }
}

/// A field's name for a person.
pub fn field_label(field: &str) -> String {
    match field {
        FIELD_NAME => "Name".to_owned(),
        FIELD_DESCRIPTION => "Description".to_owned(),
        FIELD_BODY => "Text below the products".to_owned(),
        FIELD_META_TITLE => "Search engines' title".to_owned(),
        FIELD_META_DESCRIPTION => "Search engines' description".to_owned(),
        FIELD_PRICE => "Price".to_owned(),
        _ => match priced_variant(field) {
            Some(_) => "Variant price".to_owned(),
            None => field.replace('_', " "),
        },
    }
}

/// A kind's name for a person.
pub fn kind_label(kind: &str) -> &'static str {
    match kind {
        KIND_PRODUCT => "Product",
        KIND_CATEGORY => "Category",
        KIND_NEW_CATEGORY => "New category",
        timada_proposals::KIND_ADS_BID => "Bid",
        timada_proposals::KIND_ADS_EXCLUSION => "Ads exclusion",
        timada_proposals::KIND_ADS_NEGATIVE_TERM => "Negative search term",
        _ => "Change",
    }
}

/// What prepared a proposal, for a person.
pub fn source_label(source: &str) -> String {
    match source {
        timada_proposals::SOURCE_PERSON => "Written by a member".to_owned(),
        other => {
            let mut label = other.replace(['_', '-'], " ");
            if let Some(first) = label.get_mut(0..1) {
                first.make_ascii_uppercase();
            }
            label
        }
    }
}

/// `value` as the shop would keep it in `field` of a subject of `kind`:
/// what a proposal records, so it compares equal to what the subject holds
/// once applied. Refused as the command that writes it would refuse it.
pub fn normalized(kind: &str, field: &str, value: &str) -> Result<String, CatalogError> {
    let product = kind == KIND_PRODUCT;
    match field {
        FIELD_NAME if product => normalize_name(value),
        FIELD_NAME => normalize_category_name(value),
        FIELD_DESCRIPTION if product => normalize_description(value),
        FIELD_DESCRIPTION => normalize_category_description(value),
        FIELD_BODY => normalize_category_body(value),
        FIELD_META_TITLE => Ok(normalize_seo(value, "")?.0),
        FIELD_META_DESCRIPTION => Ok(normalize_seo("", value)?.1),
        _ if field == FIELD_PRICE || priced_variant(field).is_some() => {
            Ok(format_amount(parse_amount(value)?))
        }
        _ => Ok(value.trim().to_owned()),
    }
}

/// What a proposal is about, as the shop holds it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subject {
    /// Its name, for the list.
    pub name: String,
    /// Each field of its kind with what it holds; a price as `19.90`, empty
    /// for none. A product has one more field per variant, its price.
    pub values: BTreeMap<String, String>,
    /// The currency its prices are in, once it has one.
    pub currency: Option<String>,
}

impl Subject {
    /// What `field` holds; `None` for a variant that is no longer there.
    pub fn value(&self, field: &str) -> Option<&str> {
        self.values.get(field).map(String::as_str)
    }

    fn of_product(product: &ProductState) -> Self {
        let mut values = BTreeMap::from([
            (FIELD_NAME.to_owned(), product.name.clone()),
            (FIELD_DESCRIPTION.to_owned(), product.description.clone()),
            (FIELD_META_TITLE.to_owned(), product.meta_title.clone()),
            (
                FIELD_META_DESCRIPTION.to_owned(),
                product.meta_description.clone(),
            ),
            (
                FIELD_PRICE.to_owned(),
                product
                    .price
                    .as_ref()
                    .map(|price| format_amount(price.amount_cents))
                    .unwrap_or_default(),
            ),
        ]);
        for variant in &product.variants {
            values.insert(
                variant_price_field(&variant.id),
                format_amount(variant.price.amount_cents),
            );
        }
        Self {
            name: product.name.clone(),
            values,
            currency: product.currency().map(str::to_owned),
        }
    }

    fn of_category(category: &CategoryState) -> BTreeMap<String, String> {
        BTreeMap::from([
            (FIELD_NAME.to_owned(), category.name.clone()),
            (FIELD_DESCRIPTION.to_owned(), category.description.clone()),
            (FIELD_BODY.to_owned(), category.body.clone()),
            (FIELD_META_TITLE.to_owned(), category.meta_title.clone()),
            (
                FIELD_META_DESCRIPTION.to_owned(),
                category.meta_description.clone(),
            ),
        ])
    }
}

/// The subject of a proposal of `kind` about `subject_id`, as the shop
/// holds it now; `None` when it is not there (deleted, or never was), or
/// when nothing applies the kind. For a category to create, the subject is
/// the category above it (none for an empty id), and the fields are those
/// of the category itself once `proposal_id`'s was created: empty before.
pub async fn subject(
    catalog: &Catalog,
    kind: &str,
    subject_id: &str,
    proposal_id: Option<&str>,
) -> anyhow::Result<Option<Subject>> {
    match kind {
        KIND_PRODUCT => Ok(catalog
            .load_product(subject_id)
            .await?
            .as_ref()
            .map(Subject::of_product)),
        KIND_CATEGORY => Ok(catalog
            .load_category(subject_id)
            .await?
            .map(|category| Subject {
                name: category.name.clone(),
                values: Subject::of_category(&category),
                currency: None,
            })),
        KIND_NEW_CATEGORY => {
            let name = if subject_id.is_empty() {
                TOP_LEVEL.to_owned()
            } else {
                match catalog.load_category(subject_id).await? {
                    Some(parent) if !parent.archived => parent.name,
                    _ => return Ok(None),
                }
            };
            let created = match proposal_id {
                Some(proposal_id) => {
                    catalog
                        .load_category(&proposed_category_id(proposal_id))
                        .await?
                }
                None => None,
            };
            let held = created.as_ref().map(Subject::of_category);
            let values = NEW_CATEGORY_FIELDS
                .iter()
                .map(|field| {
                    let value = held
                        .as_ref()
                        .and_then(|held| held.get(*field).cloned())
                        .unwrap_or_default();
                    ((*field).to_owned(), value)
                })
                .collect();
            Ok(Some(Subject {
                name,
                values,
                currency: None,
            }))
        }
        _ => Ok(None),
    }
}

/// Whether a proposal's changes can still be made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Standing {
    /// Every field holds what the proposal was prepared against, or what it
    /// would write already (a change made in part, or by another node).
    Current,
    /// `field` holds `found`, which is neither: someone changed it since.
    OutOfDate { field: String, found: String },
}

/// Where `changes` stand against what the subject holds.
pub fn standing(subject: &Subject, changes: &[Change]) -> Standing {
    for change in changes {
        let held = subject.value(&change.field).unwrap_or(REMOVED);
        if held != change.before && held != change.after {
            return Standing::OutOfDate {
                field: change.field.clone(),
                found: held.to_owned(),
            };
        }
    }
    Standing::Current
}

/// Why a proposal was not prepared or not approved, for the member.
#[derive(Debug)]
pub enum Refusal {
    /// Nothing applies this kind of change yet.
    NoApplier,
    /// What the proposal is about is not there.
    SubjectMissing,
    /// What the proposal is about changed since it was prepared.
    OutOfDate,
    Catalog(CatalogError),
    Proposal(ProposalError),
    Other(anyhow::Error),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoApplier => f.write_str("nothing applies this kind of change yet"),
            Self::SubjectMissing => f.write_str("what the proposal is about is not there"),
            Self::OutOfDate => {
                f.write_str("what the proposal is about changed since it was prepared")
            }
            Self::Catalog(err) => err.fmt(f),
            Self::Proposal(err) => err.fmt(f),
            Self::Other(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for Refusal {}

impl From<CatalogError> for Refusal {
    fn from(err: CatalogError) -> Self {
        Self::Catalog(err)
    }
}

impl From<ProposalError> for Refusal {
    fn from(err: ProposalError) -> Self {
        Self::Proposal(err)
    }
}

impl From<anyhow::Error> for Refusal {
    fn from(err: anyhow::Error) -> Self {
        Self::Other(err)
    }
}

impl Refusal {
    /// A stable code for a page to carry in a URL; `None` for an
    /// infrastructure failure.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::NoApplier => Some("proposal_no_applier"),
            Self::SubjectMissing => Some("proposal_subject_missing"),
            Self::OutOfDate => Some("proposal_out_of_date"),
            Self::Catalog(err) => err.code(),
            Self::Proposal(err) => err.code(),
            Self::Other(_) => None,
        }
    }
}

/// A change to prepare, as whoever prepares it gives it: the values the
/// fields would take, not yet normalized, and nothing of what they hold.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Draft {
    pub kind: String,
    pub subject_id: String,
    /// Each field with the value it would take.
    pub values: Vec<(String, String)>,
    pub reason: String,
    pub source: String,
    /// Unix seconds; `None` for the default.
    pub valid_until: Option<u64>,
}

/// Prepares `draft` in the shop `tenant_id` at `now` (unix seconds): reads
/// what the subject holds, which the proposal is recorded against, and
/// leaves out the fields that would not change. The new proposal's id.
pub async fn prepare(
    store: &Executor,
    tenant_id: &str,
    actor: &str,
    draft: Draft,
    now: u64,
) -> Result<String, Refusal> {
    if permissions_of(&draft.kind).is_none() {
        return Err(Refusal::NoApplier);
    }
    let catalog = Catalog::new(store.clone(), tenant_id);
    let held = subject(&catalog, &draft.kind, &draft.subject_id, None)
        .await?
        .ok_or(Refusal::SubjectMissing)?;
    let mut changes = Vec::with_capacity(draft.values.len());
    for (field, after) in draft.values {
        let before = held
            .value(&field)
            .ok_or(ProposalError::InvalidChanges)?
            .to_owned();
        let after = normalized(&draft.kind, &field, &after)?;
        changes.push(Change {
            field,
            before,
            after,
        });
    }
    let id = Proposals::new(store.clone(), tenant_id)
        .prepare(
            actor,
            Prepare {
                kind: draft.kind,
                subject_id: draft.subject_id,
                subject_name: held.name,
                changes,
                reason: draft.reason,
                source: draft.source,
                valid_until: draft.valid_until,
            },
            now,
        )
        .await?;
    Ok(id)
}

/// Says yes, for `actor`, to the proposal `proposal_id` of the shop
/// `tenant_id` at `now`, with the values the member changed. A proposal
/// whose subject changed since it was prepared is ended as out of date
/// instead, and the member told. The change itself is made after, by
/// [`apply::subscription`].
pub async fn approve(
    store: &Executor,
    tenant_id: &str,
    actor: &str,
    proposal: &ProposalState,
    edits: Vec<(String, String)>,
    now: u64,
) -> Result<(), Refusal> {
    if permissions_of(&proposal.kind).is_none() {
        return Err(Refusal::NoApplier);
    }
    if proposal.status != ProposalStatus::Waiting {
        return Err(ProposalError::NotWaiting.into());
    }
    let mut changes: Vec<Change> = Vec::with_capacity(proposal.changes.len());
    let mut normalized_edits = Vec::with_capacity(edits.len());
    for change in &proposal.changes {
        let mut change = change.clone();
        if let Some((_, after)) = edits.iter().find(|(field, _)| field == &change.field) {
            change.after = normalized(&proposal.kind, &change.field, after)?;
            normalized_edits.push((change.field.clone(), change.after.clone()));
        }
        changes.push(change);
    }
    let proposals = Proposals::new(store.clone(), tenant_id);
    let catalog = Catalog::new(store.clone(), tenant_id);
    let held = subject(
        &catalog,
        &proposal.kind,
        &proposal.subject_id,
        Some(&proposal.id),
    )
    .await?;
    let stale = match &held {
        None => Some((changes[0].field.clone(), REMOVED.to_owned())),
        Some(held) => match standing(held, &changes) {
            Standing::Current => None,
            Standing::OutOfDate { field, found } => Some((field, found)),
        },
    };
    if let Some((field, found)) = stale {
        proposals
            .record_out_of_date(actor, &proposal.id, &field, &found)
            .await?;
        return Err(Refusal::OutOfDate);
    }
    proposals
        .approve(actor, &proposal.id, normalized_edits, now)
        .await?;
    Ok(())
}
