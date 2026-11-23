//! `keywords`: asks the keyword source for one seed and prints what came
//! back with what the call cost, or lists the countries the source has
//! keywords for. It is how an operator checks the login, and how a figure
//! is compared with the one another tool gives for the same keyword.
//!
//! A seed is one paid call, whatever `--limit` prints of it.

use std::fmt::Write as _;
use std::io::Write as _;

use anyhow::{Context, Result};

use crate::config::Config;
use crate::keywords::{KeywordSource, Place, Suggestions};

#[derive(clap::Args)]
pub struct KeywordsArgs {
    /// The seed keyword; without one, the countries and their languages
    pub seed: Option<String>,
    /// The country searched from, two letters
    #[arg(long, default_value = "FR")]
    pub country: String,
    /// The language searched in, two letters
    #[arg(long, default_value = "fr")]
    pub language: String,
    /// How many of the keywords to print, the most searched first
    #[arg(long, default_value_t = 50)]
    pub limit: usize,
}

pub async fn keywords(config: Config, args: KeywordsArgs) -> Result<()> {
    let Some(source) = crate::keywords::source(&config.dataforseo) else {
        anyhow::bail!(
            "no keyword source: set the login and the password of [dataforseo] \
             (TIMADA__DATAFORSEO__LOGIN, TIMADA__DATAFORSEO__PASSWORD)"
        );
    };
    let printed = print(source.as_ref(), &args).await?;
    std::io::stdout()
        .write_all(printed.as_bytes())
        .context("cannot write the keywords")
}

async fn print(source: &dyn KeywordSource, args: &KeywordsArgs) -> Result<String> {
    Ok(match &args.seed {
        Some(seed) => table(
            &source
                .suggestions(seed, &args.country, &args.language)
                .await?,
            args.limit,
        ),
        None => places(&source.places().await?),
    })
}

fn places(places: &[Place]) -> String {
    let mut out = String::new();
    for place in places {
        let languages: Vec<_> = place.languages.iter().map(|l| l.code.as_str()).collect();
        let _ = writeln!(
            out,
            "{}  {:<32} {}",
            place.country,
            place.name,
            languages.join(" ")
        );
    }
    out
}

/// The first `limit` rows, then what the whole answer holds: how many rows
/// have each figure tells whether the source knows this country well.
fn table(found: &Suggestions, limit: usize) -> String {
    let dash = || "-".to_owned();
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:>9} {:>8} {:>4}  {:<13} keyword",
        "volume", "cpc USD", "kd", "intent"
    );
    for row in found.rows.iter().take(limit) {
        let _ = writeln!(
            out,
            "{:>9} {:>8} {:>4}  {:<13} {}",
            row.volume.map_or_else(dash, |v| v.to_string()),
            row.cpc_usd_cents
                .map_or_else(dash, |c| format!("{}.{:02}", c / 100, c % 100)),
            row.difficulty.map_or_else(dash, |d| d.to_string()),
            row.intent.map_or("-", |i| i.as_str()),
            row.keyword,
        );
    }
    let rows = found.rows.len();
    let with = |has: fn(&crate::keywords::Keyword) -> bool| {
        found.rows.iter().filter(|row| has(row)).count()
    };
    let _ = writeln!(
        out,
        "\n{rows} keywords: {} with a volume, {} with a cost per click, {} with a difficulty, {} with an intent",
        with(|row| row.volume.is_some()),
        with(|row| row.cpc_usd_cents.is_some()),
        with(|row| row.difficulty.is_some()),
        with(|row| row.intent.is_some()),
    );
    let _ = writeln!(
        out,
        "cost of the call: {:.5} USD",
        found.cost_usd_micros as f64 / 1_000_000.0
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keywords::fake::{Asked, FakeKeywords, keyword};
    use crate::keywords::{Keyword, KeywordError};

    fn args(seed: Option<&str>) -> KeywordsArgs {
        KeywordsArgs {
            seed: seed.map(str::to_owned),
            country: "FR".into(),
            language: "fr".into(),
            limit: 2,
        }
    }

    #[tokio::test]
    async fn a_seed_prints_its_first_rows_and_counts_them_all() {
        let source = FakeKeywords::new();
        source.give(
            "arbre à chat",
            vec![
                keyword("arbre à chat", 110_000, 38, 41),
                keyword("arbre à chat xxl", 9_900, 105, 12),
                Keyword {
                    keyword: "fabriquer un arbre à chat".into(),
                    volume: Some(1_900),
                    cpc_usd_cents: None,
                    difficulty: None,
                    intent: None,
                },
            ],
        );
        let printed = print(source.as_ref(), &args(Some("arbre à chat")))
            .await
            .unwrap();
        assert_eq!(
            source.asked(),
            vec![Asked {
                seed: "arbre à chat".into(),
                country: "FR".into(),
                language: "fr".into(),
            }]
        );
        assert!(
            printed.contains("     9900     1.05   12  commercial    arbre à chat xxl\n"),
            "{printed}"
        );
        assert!(!printed.contains("fabriquer"), "{printed}");
        assert!(
            printed.contains(
                "3 keywords: 3 with a volume, 2 with a cost per click, 2 with a difficulty, 2 with an intent"
            ),
            "{printed}"
        );
        assert!(
            printed.contains("cost of the call: 0.01236 USD"),
            "{printed}"
        );
    }

    #[tokio::test]
    async fn without_a_seed_the_countries_are_listed() {
        let source = FakeKeywords::new();
        let printed = print(source.as_ref(), &args(None)).await.unwrap();
        assert!(printed.contains("FR  France"), "{printed}");
        assert!(printed.contains("en es"), "{printed}");
        assert!(source.asked().is_empty());
    }

    #[tokio::test]
    async fn a_failure_of_the_source_is_the_commands_own() {
        let source = FakeKeywords::new();
        source.fail(Some(KeywordError::Refused("Insufficient Funds.".into())));
        let err = print(source.as_ref(), &args(Some("griffoir")))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Insufficient Funds."), "{err}");
        source.fail(None);
        assert!(
            print(source.as_ref(), &args(Some("griffoir")))
                .await
                .is_ok()
        );
    }
}
