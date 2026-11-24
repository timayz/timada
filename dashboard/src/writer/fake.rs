//! A writer for the tests: it answers from what it is asked and counts its
//! calls, or fails the way it is told to.

use std::sync::Mutex;

use super::{Answer, Ask, Failed, TextWriter, Texts, WriterFault};

/// What one call of the fake costs, and the most it may.
pub const COST_USD_MICROS: u64 = 21_500;
pub const MOST_USD_MICROS: u64 = 400_000;

#[derive(Default)]
pub struct FakeWriter {
    /// Every ask it got, in order.
    pub asked: Mutex<Vec<Ask>>,
    /// What it answers with instead of writing.
    pub fault: Mutex<Option<WriterFault>>,
    /// What it writes instead of its own texts.
    pub texts: Mutex<Option<Texts>>,
}

impl FakeWriter {
    pub fn failing(fault: WriterFault) -> Self {
        Self {
            fault: Mutex::new(Some(fault)),
            ..Self::default()
        }
    }

    pub fn writing(texts: Texts) -> Self {
        Self {
            texts: Mutex::new(Some(texts)),
            ..Self::default()
        }
    }

    pub fn calls(&self) -> usize {
        self.asked.lock().unwrap().len()
    }
}

/// What the fake writes for `keyword`, unless told otherwise.
pub fn texts_for(keyword: &str, product: bool) -> Texts {
    Texts {
        name: if product {
            format!("{keyword} drafted")
        } else {
            String::new()
        },
        description: format!("A text about {keyword}."),
        meta_title: format!("{keyword} | drafted"),
        meta_description: format!("Everything about {keyword}."),
    }
}

#[async_trait::async_trait]
impl TextWriter for FakeWriter {
    async fn write(&self, ask: &Ask) -> Result<Answer, Failed> {
        self.asked.lock().unwrap().push(ask.clone());
        if let Some(fault) = self.fault.lock().unwrap().clone() {
            return Err(fault.into());
        }
        let texts = self
            .texts
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| texts_for(&ask.keyword, ask.product));
        Ok(Answer {
            texts,
            cost_usd_micros: COST_USD_MICROS,
        })
    }

    fn most_usd_micros(&self) -> u64 {
        MOST_USD_MICROS
    }
}
