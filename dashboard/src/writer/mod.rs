//! First drafts of a shop's texts, written by a model for a person to
//! correct: a product's or a category's name, its description, and what
//! search engines show.
//!
//! [`TextWriter`] hides whose model writes; [`claude`] is the one there is,
//! set in the configuration's `[writer]` section. Without a key there is
//! no writer and nothing here calls anyone.
//!
//! A draft is never published. [`drafts`] asks the model off the page that
//! asked, inside the day's caps (`timada-writer`'s `SpendDay`), and
//! prepares what comes back as a proposal, which waits for a member
//! ([`crate::proposals`]).
//!
//! Every call costs money, so an answer carries what it cost for a cap to
//! count, and a failure says which kind it is ([`WriterFault`]): asking
//! again may do, or it will not.

pub mod claude;
pub mod drafts;
#[cfg(test)]
pub mod fake;

use std::sync::Arc;

use timada_writer::SpendCaps;

use crate::config::WriterConfig;

/// The four texts of a product or a category. One left empty is not
/// drafted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Texts {
    pub name: String,
    pub description: String,
    /// The title search engines show.
    pub meta_title: String,
    /// The description search engines show.
    pub meta_description: String,
}

impl Texts {
    /// Everything it says, as one text.
    pub fn all(&self) -> String {
        [
            self.name.as_str(),
            self.description.as_str(),
            self.meta_title.as_str(),
            self.meta_description.as_str(),
        ]
        .join("\n")
    }
}

/// The most characters each text may be: what the shop would refuse
/// beyond.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub name: usize,
    pub description: usize,
    pub meta_title: usize,
    pub meta_description: usize,
}

/// What a draft is asked from: everything the shop holds of the subject,
/// and how it wants to be written for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    /// Whether a product's texts are asked for, or a category's.
    pub product: bool,
    /// The language to write in, two letters (`fr`); empty for the one
    /// the subject is written in now, when the shop never said.
    pub language: String,
    /// The search the texts are written for.
    pub keyword: String,
    /// What the subject holds now. For an imported product, the supplier's
    /// own wording.
    pub now: Texts,
    /// What is known of it, each as a name and a value (`Material`,
    /// `Leather`): the only facts a draft may state.
    pub facts: Vec<(String, String)>,
    /// For a category: what it holds, by name (its sub-categories, some of
    /// its products).
    pub holds: Vec<String>,
    /// The shop's voice, in the merchant's words; empty for none.
    pub tone: String,
    /// What a text must never claim; empty for nothing.
    pub never: String,
    pub limits: Limits,
}

impl Ask {
    /// Everything the shop holds of the subject, as one text: what a
    /// draft's facts are looked for in.
    pub fn source(&self) -> String {
        let mut source = vec![self.keyword.clone(), self.now.all()];
        source.extend(
            self.facts
                .iter()
                .map(|(name, value)| format!("{name}: {value}")),
        );
        source.extend(self.holds.iter().cloned());
        source.join("\n")
    }
}

/// What the model wrote, and what the call cost.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Answer {
    pub texts: Texts,
    /// In millionths of a US dollar.
    pub cost_usd_micros: u64,
}

/// Why a call gave nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriterFault {
    /// The model could not answer (no answer in time, its own failure,
    /// asked too often, an answer cut short): asking again later may do.
    Unavailable(String),
    /// The model will say the same again: the key is wrong, the account is
    /// out of credit, it declined to write this.
    Refused(String),
}

impl std::fmt::Display for WriterFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(reason) => write!(f, "The writer did not answer: {reason}"),
            Self::Refused(reason) => write!(f, "The writer refused: {reason}"),
        }
    }
}

impl std::error::Error for WriterFault {}

/// A call that gave no text: why, and what it cost all the same (a call
/// the model declined or was cut short in is paid for), in millionths of a
/// US dollar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failed {
    pub fault: WriterFault,
    pub cost_usd_micros: u64,
}

impl From<WriterFault> for Failed {
    /// A failure that cost nothing: the model never answered.
    fn from(fault: WriterFault) -> Self {
        Self {
            fault,
            cost_usd_micros: 0,
        }
    }
}

#[async_trait::async_trait]
pub trait TextWriter: Send + Sync {
    /// The texts of `ask`'s subject, written for its keyword in its
    /// language from its facts alone. One paid call.
    async fn write(&self, ask: &Ask) -> Result<Answer, Failed>;

    /// The most one call may cost, in millionths of a US dollar: the room
    /// a cap keeps for a call before it is made. What a call did cost is
    /// the answer's to say.
    fn most_usd_micros(&self) -> u64;
}

/// The writer as the pages and the saga reach it; `None` inside when text
/// drafts are off.
#[derive(Clone)]
pub struct Writers(pub Option<Arc<dyn TextWriter>>);

impl Writers {
    /// The writer the configuration sets; none without a `[writer]` key.
    pub fn new(config: &WriterConfig) -> Self {
        Self(config.key().map(|key| {
            Arc::new(claude::Claude::new(
                key,
                config.api_url.clone(),
                &config.model,
                claude::Prices {
                    input_usd_per_mtok: config.input_usd_per_mtok,
                    output_usd_per_mtok: config.output_usd_per_mtok,
                },
            )) as Arc<dyn TextWriter>
        }))
    }

    /// Whether there is a model to ask.
    pub fn enabled(&self) -> bool {
        self.0.is_some()
    }
}

/// What the drafts may spend in a day (the configuration's `[writer]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spending {
    pub caps: SpendCaps,
}

impl Spending {
    pub fn new(config: &WriterConfig) -> Self {
        Self {
            caps: config.caps(),
        }
    }
}

/// An amount in millionths of a US dollar, for a person: `0.02 USD`, and
/// to the tenth of a cent under a cent.
pub fn format_usd(usd_micros: u64) -> String {
    if usd_micros > 0 && usd_micros < 10_000 {
        format!("{:.3} USD", usd_micros as f64 / 1_000_000.0)
    } else {
        format!("{:.2} USD", usd_micros as f64 / 1_000_000.0)
    }
}
