//! A source for the crate's own tests: it gives the rows it was told to
//! give for a seed, at DataForSEO's price, and remembers what it was asked.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::{
    FirstResults, Keyword, KeywordError, KeywordSource, Language, Place, SearchResult, Suggestions,
};

/// What one call costs, and one keyword returned, in millionths of a dollar.
const CALL_MICROS: u64 = 12_000;
const ROW_MICROS: u64 = 120;

/// What a search of one page costs.
const SEARCH_MICROS: u64 = 2_000;

/// One call as the fake was asked it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asked {
    pub seed: String,
    pub country: String,
    pub language: String,
}

#[derive(Default)]
pub struct FakeKeywords {
    rows: Mutex<HashMap<String, Vec<Keyword>>>,
    /// Answers every question with this error instead.
    failing: Mutex<Option<KeywordError>>,
    asked: Mutex<Vec<Asked>>,
    /// The sites a search for a keyword gives, in their order.
    sites: Mutex<HashMap<String, Vec<String>>>,
    searched: Mutex<Vec<Asked>>,
}

impl FakeKeywords {
    pub fn new() -> Arc<Self> {
        Arc::default()
    }

    /// The rows `seed` gives from now on; a seed never told of gives none.
    pub fn give(&self, seed: &str, rows: Vec<Keyword>) {
        self.rows.lock().unwrap().insert(seed.to_owned(), rows);
    }

    /// The sites a search for `keyword` gives from now on, the first
    /// first; a keyword never told of gives none.
    pub fn find(&self, keyword: &str, sites: &[&str]) {
        self.sites.lock().unwrap().insert(
            keyword.to_owned(),
            sites.iter().map(|site| site.to_string()).collect(),
        );
    }

    /// The paid searches made so far, failed ones included; `seed` is the
    /// keyword searched.
    pub fn searched(&self) -> Vec<Asked> {
        self.searched.lock().unwrap().clone()
    }

    /// Answers everything with `error` from now on; `None` answers again.
    pub fn fail(&self, error: Option<KeywordError>) {
        *self.failing.lock().unwrap() = error;
    }

    /// The paid calls made so far, failed ones included.
    pub fn asked(&self) -> Vec<Asked> {
        self.asked.lock().unwrap().clone()
    }

    fn answering(&self) -> Result<(), KeywordError> {
        match self.failing.lock().unwrap().clone() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

/// A keyword with every figure set.
pub fn keyword(keyword: &str, volume: u64, cpc_usd_cents: u64, difficulty: u8) -> Keyword {
    Keyword {
        keyword: keyword.to_owned(),
        volume: Some(volume),
        cpc_usd_cents: Some(cpc_usd_cents),
        difficulty: Some(difficulty),
        intent: Some(super::Intent::Commercial),
    }
}

#[async_trait::async_trait]
impl KeywordSource for FakeKeywords {
    async fn suggestions(
        &self,
        seed: &str,
        country: &str,
        language: &str,
    ) -> Result<Suggestions, KeywordError> {
        self.asked.lock().unwrap().push(Asked {
            seed: seed.to_owned(),
            country: country.to_owned(),
            language: language.to_owned(),
        });
        self.answering()?;
        let rows = self
            .rows
            .lock()
            .unwrap()
            .get(seed)
            .cloned()
            .unwrap_or_default();
        Ok(Suggestions {
            cost_usd_micros: CALL_MICROS + ROW_MICROS * rows.len() as u64,
            rows,
        })
    }

    fn most_usd_micros(&self) -> u64 {
        CALL_MICROS + ROW_MICROS * super::MAX_ROWS as u64
    }

    async fn first_results(
        &self,
        keyword: &str,
        country: &str,
        language: &str,
    ) -> Result<FirstResults, KeywordError> {
        self.searched.lock().unwrap().push(Asked {
            seed: keyword.to_owned(),
            country: country.to_owned(),
            language: language.to_owned(),
        });
        self.answering()?;
        let sites = self
            .sites
            .lock()
            .unwrap()
            .get(keyword)
            .cloned()
            .unwrap_or_default();
        Ok(FirstResults {
            results: sites
                .into_iter()
                .enumerate()
                .map(|(at, site)| SearchResult {
                    position: at as u8 + 1,
                    url: format!("https://{site}/"),
                    title: format!("{keyword} - {site}"),
                    domain: site,
                })
                .collect(),
            cost_usd_micros: SEARCH_MICROS,
        })
    }

    fn most_results_usd_micros(&self) -> u64 {
        SEARCH_MICROS
    }

    async fn places(&self) -> Result<Vec<Place>, KeywordError> {
        self.answering()?;
        let language = |code: &str, name: &str| Language {
            code: code.to_owned(),
            name: name.to_owned(),
        };
        Ok(vec![
            Place {
                country: "FR".to_owned(),
                name: "France".to_owned(),
                languages: vec![language("fr", "French")],
            },
            Place {
                country: "US".to_owned(),
                name: "United States".to_owned(),
                languages: vec![language("en", "English"), language("es", "Spanish")],
            },
        ])
    }
}
