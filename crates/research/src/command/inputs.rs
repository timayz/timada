//! What the owner says of the niche beside its keywords: one typical sale,
//! the brands and the words that drop a keyword, what they looked up by
//! hand, and the keywords they drop or bring back one by one. Nothing here
//! fetches: the study is scored again from the rows it holds.

use evento::Executor;
use timada_catalog::MAX_FEE_BPS;

use crate::aggregator::{InputsSet, KeywordDropped, KeywordRestored};
use crate::error::ResearchError;
use crate::score::Sale;
use crate::value_object::{MAX_HAND_SET, MAX_KEYWORD_LEN, MAX_STUDY_EVENTS, normalize_words};

use super::{Command, Step, StudyState};

/// What the owner says of a study, whole: it replaces what they said
/// before.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SetInputs {
    pub sale: Sale,
    pub brands: Vec<String>,
    pub excluded_words: Vec<String>,
    pub supplier_products: Option<u32>,
    pub specialist_first: Option<bool>,
    /// What one unit of the sale's currency is worth today in millionths
    /// of a US dollar; `None` when the caller knows no rate. It is kept
    /// with the rest, so every node scores the study alike.
    pub usd_micros: Option<u64>,
}

/// A keyword as a row spells it: on one line, [`MAX_KEYWORD_LEN`]
/// characters at most.
fn keyword_of(keyword: &str) -> Result<String, ResearchError> {
    let keyword = keyword.split_whitespace().collect::<Vec<_>>().join(" ");
    if keyword.is_empty() || keyword.chars().count() > MAX_KEYWORD_LEN {
        return Err(ResearchError::InvalidKeyword);
    }
    Ok(keyword)
}

/// Refuses a change to a study whose stream is nearly full.
fn room_for_a_change(study: &StudyState) -> Result<(), ResearchError> {
    if study.aggregate_version >= MAX_STUDY_EVENTS {
        return Err(ResearchError::TooManyChanges);
    }
    Ok(())
}

#[evento::command]
impl<E: Executor> Command<E> {
    /// Says what the study is scored from; the same as it stands is a
    /// no-op.
    pub async fn set_inputs(
        &self,
        actor_user_id: &str,
        study_id: &str,
        input: SetInputs,
        routing_key: Option<String>,
    ) -> Result<(), ResearchError> {
        let currency = input.sale.currency.trim().to_ascii_uppercase();
        if !timada_core::CURRENCIES.contains(&currency.as_str()) {
            return Err(ResearchError::InvalidCurrency);
        }
        if input.sale.vat_bps > MAX_FEE_BPS || input.sale.payment_fee_bps > MAX_FEE_BPS {
            return Err(ResearchError::InvalidRate);
        }
        let sale = Sale {
            currency,
            ..input.sale
        };
        let brands = normalize_words(&input.brands)?;
        let excluded_words = normalize_words(&input.excluded_words)?;
        let usd_micros = input.usd_micros.filter(|rate| *rate > 0);
        self.settle_owned(actor_user_id, study_id, routing_key, |study, write| {
            let said = &study.inputs;
            if study.inputs_set_at.is_some()
                && said.sale == sale
                && said.brands == brands
                && said.excluded_words == excluded_words
                && said.supplier_products == input.supplier_products
                && said.specialist_first == input.specialist_first
                && said.usd_micros == usd_micros
            {
                return Ok(Step::Leave(()));
            }
            room_for_a_change(study)?;
            write.event(&InputsSet {
                currency: sale.currency.clone(),
                price_cents: sale.price_cents,
                cost_cents: sale.cost_cents,
                shipping_cents: sale.shipping_cents,
                import_cents: sale.import_cents,
                vat_bps: sale.vat_bps,
                fixed_cents: sale.fixed_cents,
                payment_fee_bps: sale.payment_fee_bps,
                payment_fee_cents: sale.payment_fee_cents,
                brands: brands.clone(),
                excluded_words: excluded_words.clone(),
                supplier_products: input.supplier_products,
                specialist_first: input.specialist_first,
                usd_micros,
                set_by: actor_user_id.to_owned(),
            });
            Ok(Step::Write(()))
        })
        .await
    }

    /// Drops a keyword by hand, whatever the filters say; one already
    /// dropped by hand is a no-op.
    pub async fn drop_keyword(
        &self,
        actor_user_id: &str,
        study_id: &str,
        keyword: &str,
        routing_key: Option<String>,
    ) -> Result<(), ResearchError> {
        let keyword = keyword_of(keyword)?;
        self.settle_owned(actor_user_id, study_id, routing_key, |study, write| {
            if study.inputs.dropped.contains(&keyword) {
                return Ok(Step::Leave(()));
            }
            if study.inputs.dropped.len() >= MAX_HAND_SET {
                return Err(ResearchError::TooManyHandSet);
            }
            room_for_a_change(study)?;
            write.event(&KeywordDropped {
                keyword: keyword.clone(),
                dropped_by: actor_user_id.to_owned(),
            });
            Ok(Step::Write(()))
        })
        .await
    }

    /// Brings a keyword back, whatever dropped it: by hand or a filter.
    /// One already brought back is a no-op.
    pub async fn restore_keyword(
        &self,
        actor_user_id: &str,
        study_id: &str,
        keyword: &str,
        routing_key: Option<String>,
    ) -> Result<(), ResearchError> {
        let keyword = keyword_of(keyword)?;
        self.settle_owned(actor_user_id, study_id, routing_key, |study, write| {
            if study.inputs.restored.contains(&keyword) {
                return Ok(Step::Leave(()));
            }
            if study.inputs.restored.len() >= MAX_HAND_SET {
                return Err(ResearchError::TooManyHandSet);
            }
            room_for_a_change(study)?;
            write.event(&KeywordRestored {
                keyword: keyword.clone(),
                restored_by: actor_user_id.to_owned(),
            });
            Ok(Step::Write(()))
        })
        .await
    }
}
