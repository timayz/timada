//! DataForSEO as the [`KeywordSource`]: its Labs API for Google, under the
//! platform's own login (`[dataforseo]` in the configuration).
//!
//! Two calls are made:
//!
//! - `dataforseo_labs/google/keyword_suggestions/live`, paid: the keywords
//!   that contain a seed's words, with what its database holds for each;
//! - `dataforseo_labs/locations_and_languages`, free: the countries it has
//!   keywords for. Read once a day ([`PLACES_FRESH_FOR`]) and kept, since a
//!   suggestion call names its country by the code this list gives.
//!
//! Each takes at most [`DATAFORSEO_TIMEOUT`]. DataForSEO answers most
//! failures with HTTP 200 and says what went wrong in the body, once for the
//! call and once for its one task, so the body's codes decide what a failure
//! is ([`failure`]), and the HTTP status only when there is no body to read.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::json;

use super::{Intent, Keyword, KeywordError, KeywordSource, Language, MAX_ROWS, Place, Suggestions};

/// DataForSEO's API.
pub const DATAFORSEO_API_URL: &str = "https://api.dataforseo.com";

/// How long one call may take, connection and answer together. A thousand
/// keywords come back in a few seconds; DataForSEO itself gives a call up
/// to two minutes before failing it, which nothing here waits for.
pub const DATAFORSEO_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the list of countries is kept before it is read again.
pub const PLACES_FRESH_FOR: Duration = Duration::from_secs(24 * 60 * 60);

/// How long a list that could not be read again goes on being used before
/// the next try.
const PLACES_RETRY_AFTER: Duration = Duration::from_secs(60);

/// The search engine the keywords are read for, among those a country's
/// language lists.
const SOURCE: &str = "google";

/// What a call and a keyword returned cost, in millionths of a US dollar, as
/// read on 2026-11-18. Only the bound on a call not made yet is reckoned
/// from them; what a call did cost is read from its answer.
const CALL_USD_MICROS: u64 = 12_000;
const ROW_USD_MICROS: u64 = 120;

/// The most one call may cost: 0.132 USD, for the thousand it may return.
pub const MOST_USD_MICROS: u64 = CALL_USD_MICROS + ROW_USD_MICROS * MAX_ROWS as u64;

const OK: u32 = 20000;
/// "No search results": an answer, with no rows.
const NO_RESULTS: u32 = 40102;

/// A country with the code DataForSEO knows it by.
struct Located {
    place: Place,
    code: u32,
}

#[derive(Default)]
struct Held {
    located: Arc<Vec<Located>>,
    fresh_until: Option<Instant>,
}

pub struct DataforseoClient {
    url: String,
    login: String,
    password: String,
    client: reqwest::Client,
    held: tokio::sync::Mutex<Held>,
}

impl DataforseoClient {
    /// `url`: DataForSEO's own ([`DATAFORSEO_API_URL`]) when `None`.
    pub fn new(login: &str, password: &str, url: Option<String>) -> Self {
        Self {
            url: url
                .unwrap_or_else(|| DATAFORSEO_API_URL.to_owned())
                .trim_end_matches('/')
                .to_owned(),
            login: login.to_owned(),
            password: password.to_owned(),
            client: reqwest::Client::builder()
                .timeout(DATAFORSEO_TIMEOUT)
                .build()
                .unwrap_or_default(),
            held: tokio::sync::Mutex::default(),
        }
    }

    /// Sends `request` under the login and opens the answer.
    async fn call<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<Answer<T>, KeywordError> {
        let response = request
            .basic_auth(&self.login, Some(&self.password))
            .send()
            .await
            // The error names the address, never the login.
            .map_err(|err| KeywordError::Unavailable(err.without_url().to_string()))?;
        let status = response.status().as_u16();
        let body = response
            .bytes()
            .await
            .map_err(|err| KeywordError::Unavailable(err.without_url().to_string()))?;
        open(status, &body)
    }

    /// The countries, read again when the list held is a day old. A list
    /// that cannot be read again goes on being used: the countries change a
    /// few times a year.
    async fn located(&self) -> Result<Arc<Vec<Located>>, KeywordError> {
        let mut held = self.held.lock().await;
        if held.fresh_until.is_some_and(|until| Instant::now() < until) {
            return Ok(held.located.clone());
        }
        let request = self.client.get(format!(
            "{}/v3/dataforseo_labs/locations_and_languages",
            self.url
        ));
        match self.call::<LocationEntry>(request).await {
            Ok(answer) => {
                held.located = Arc::new(located(answer.result));
                held.fresh_until = Some(Instant::now() + PLACES_FRESH_FOR);
                tracing::debug!(countries = held.located.len(), "keyword countries read");
            }
            Err(err) if held.fresh_until.is_some() => {
                tracing::warn!(%err, "the keyword countries could not be read again");
                held.fresh_until = Some(Instant::now() + PLACES_RETRY_AFTER);
            }
            Err(err) => return Err(err),
        }
        Ok(held.located.clone())
    }
}

#[async_trait::async_trait]
impl KeywordSource for DataforseoClient {
    async fn suggestions(
        &self,
        seed: &str,
        country: &str,
        language: &str,
    ) -> Result<Suggestions, KeywordError> {
        let seed = seed.trim();
        if seed.is_empty() {
            return Err(KeywordError::Refused("no seed keyword".to_owned()));
        }
        let located = self.located().await?;
        let language = language.trim().to_lowercase();
        let Some(code) = located
            .iter()
            .find(|located| located.place.country.eq_ignore_ascii_case(country.trim()))
            .filter(|located| located.place.languages.iter().any(|l| l.code == language))
            .map(|located| located.code)
        else {
            return Err(KeywordError::Refused(format!(
                "no keywords for the country '{country}' in the language '{language}'"
            )));
        };
        // One task a call: DataForSEO refuses more.
        let body = json!([{
            "keyword": seed,
            "location_code": code,
            "language_code": language,
            "include_seed_keyword": true,
            "limit": MAX_ROWS,
        }]);
        let request = self
            .client
            .post(format!(
                "{}/v3/dataforseo_labs/google/keyword_suggestions/live",
                self.url
            ))
            .json(&body);
        let answer = self.call::<SuggestionsResult>(request).await?;
        Ok(Suggestions {
            rows: rows(answer.result),
            cost_usd_micros: answer.cost_usd_micros,
        })
    }

    fn most_usd_micros(&self) -> u64 {
        MOST_USD_MICROS
    }

    async fn places(&self) -> Result<Vec<Place>, KeywordError> {
        let located = self.located().await?;
        Ok(located.iter().map(|l| l.place.clone()).collect())
    }
}

/// What every answer is wrapped in: the call's own verdict, then its task's.
#[derive(Deserialize)]
#[serde(bound = "T: DeserializeOwned")]
struct Envelope<T> {
    status_code: Option<u32>,
    status_message: Option<String>,
    cost: Option<f64>,
    tasks: Option<Vec<Task<T>>>,
}

#[derive(Deserialize)]
#[serde(bound = "T: DeserializeOwned")]
struct Task<T> {
    status_code: Option<u32>,
    status_message: Option<String>,
    cost: Option<f64>,
    result: Option<Vec<T>>,
}

/// A task that went through: what it gave and what it cost.
#[derive(Debug)]
struct Answer<T> {
    result: Vec<T>,
    cost_usd_micros: u64,
}

/// What DataForSEO's code for a failure means here. Asked too often
/// (40202, 40209), the search engine's failure (40101), a task not yet
/// taken (40601) and its own failures (5xxxx) may pass; every other one
/// (the login, the balance, a field it does not take) it will say again.
fn failure(code: u32, message: Option<String>) -> KeywordError {
    let reason = match message.filter(|m| !m.trim().is_empty()) {
        Some(message) => format!("{} (code {code})", message.trim()),
        None => format!("code {code}"),
    };
    if matches!(code, 40101 | 40202 | 40209 | 40601) || code >= 50000 {
        KeywordError::Unavailable(reason)
    } else {
        KeywordError::Refused(reason)
    }
}

/// The same from the HTTP status alone, for an answer with no body to read.
fn http_failure(status: u16, reason: String) -> KeywordError {
    if (400..500).contains(&status) && status != 408 && status != 429 {
        KeywordError::Refused(reason)
    } else {
        KeywordError::Unavailable(reason)
    }
}

/// Opens an answer: the result of its task and the cost, or what failed.
fn open<T: DeserializeOwned>(status: u16, body: &[u8]) -> Result<Answer<T>, KeywordError> {
    let envelope = match serde_json::from_slice::<Envelope<T>>(body) {
        Ok(envelope) => envelope,
        Err(_) if !(200..300).contains(&status) => {
            return Err(http_failure(status, format!("HTTP {status}")));
        }
        Err(err) => {
            return Err(KeywordError::Unavailable(format!(
                "an answer that cannot be read: {err}"
            )));
        }
    };
    match envelope.status_code {
        Some(OK) => {}
        Some(code) => return Err(failure(code, envelope.status_message)),
        None => return Err(http_failure(status, format!("HTTP {status}"))),
    }
    let Some(task) = envelope.tasks.and_then(|tasks| tasks.into_iter().next()) else {
        return Err(KeywordError::Unavailable(
            "an answer without its task".to_owned(),
        ));
    };
    match task.status_code {
        Some(OK | NO_RESULTS) => {}
        Some(code) => return Err(failure(code, task.status_message)),
        None => {
            return Err(KeywordError::Unavailable(
                "a task without its status".to_owned(),
            ));
        }
    }
    let cost = task.cost.or(envelope.cost).unwrap_or(0.0);
    Ok(Answer {
        result: task.result.unwrap_or_default(),
        cost_usd_micros: (cost.max(0.0) * 1_000_000.0).round() as u64,
    })
}

#[derive(Deserialize)]
struct SuggestionsResult {
    seed_keyword_data: Option<Item>,
    items: Option<Vec<Item>>,
}

// The figures are read as floats whatever they are: a whole number written
// `1900.0` one day must not lose the thousand rows around it.
#[derive(Deserialize)]
struct Item {
    keyword: Option<String>,
    keyword_info: Option<KeywordInfo>,
    keyword_properties: Option<KeywordProperties>,
    search_intent_info: Option<SearchIntentInfo>,
}

#[derive(Deserialize)]
struct KeywordInfo {
    search_volume: Option<f64>,
    cpc: Option<f64>,
}

#[derive(Deserialize)]
struct KeywordProperties {
    keyword_difficulty: Option<f64>,
}

#[derive(Deserialize)]
struct SearchIntentInfo {
    main_intent: Option<String>,
}

fn keyword(item: Item) -> Option<Keyword> {
    let keyword = item.keyword.filter(|k| !k.trim().is_empty())?;
    let whole = |figure: f64| figure.is_finite().then(|| figure.max(0.0).round() as u64);
    let info = item.keyword_info;
    Some(Keyword {
        keyword,
        volume: info.as_ref().and_then(|i| i.search_volume).and_then(whole),
        cpc_usd_cents: info
            .as_ref()
            .and_then(|i| i.cpc)
            .and_then(|cpc| whole(cpc * 100.0)),
        difficulty: item
            .keyword_properties
            .and_then(|p| p.keyword_difficulty)
            .and_then(whole)
            .map(|d| d.min(100) as u8),
        intent: item
            .search_intent_info
            .and_then(|i| i.main_intent)
            .and_then(|intent| Intent::parse(&intent)),
    })
}

/// The seed first, when DataForSEO knows it, then its suggestions in the
/// order they came (the most searched first), [`MAX_ROWS`] at most.
fn rows(result: Vec<SuggestionsResult>) -> Vec<Keyword> {
    let Some(result) = result.into_iter().next() else {
        return Vec::new();
    };
    let seed = result.seed_keyword_data.and_then(keyword);
    let mut rows: Vec<Keyword> = result
        .items
        .unwrap_or_default()
        .into_iter()
        .filter_map(keyword)
        .filter(|row| seed.as_ref().is_none_or(|seed| seed.keyword != row.keyword))
        .collect();
    if let Some(seed) = seed {
        rows.insert(0, seed);
    }
    rows.truncate(MAX_ROWS);
    rows
}

#[derive(Deserialize)]
struct LocationEntry {
    location_code: Option<u32>,
    location_name: Option<String>,
    country_iso_code: Option<String>,
    available_languages: Option<Vec<LanguageEntry>>,
}

#[derive(Deserialize)]
struct LanguageEntry {
    available_sources: Option<Vec<String>>,
    language_name: Option<String>,
    language_code: Option<String>,
}

/// The countries with at least one language Google's keywords are held in,
/// by name.
fn located(entries: Vec<LocationEntry>) -> Vec<Located> {
    let mut located: Vec<Located> = entries
        .into_iter()
        .filter_map(|entry| {
            let languages: Vec<Language> = entry
                .available_languages
                .unwrap_or_default()
                .into_iter()
                .filter(|language| {
                    language
                        .available_sources
                        .as_ref()
                        .is_some_and(|sources| sources.iter().any(|s| s == SOURCE))
                })
                .filter_map(|language| {
                    Some(Language {
                        code: language.language_code?.to_lowercase(),
                        name: language.language_name?,
                    })
                })
                .collect();
            if languages.is_empty() {
                return None;
            }
            Some(Located {
                code: entry.location_code?,
                place: Place {
                    country: entry.country_iso_code?.to_uppercase(),
                    name: entry.location_name?,
                    languages,
                },
            })
        })
        .collect();
    located.sort_by(|a, b| a.place.name.cmp(&b.place.name));
    located
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use base64::Engine;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    const PLACES: &str = r#"{
        "status_code": 20000, "status_message": "Ok.", "cost": 0, "tasks_count": 1,
        "tasks": [{
            "status_code": 20000, "status_message": "Ok.", "cost": 0,
            "result": [
                {"location_code": 2840, "location_name": "United States",
                 "location_code_parent": null, "country_iso_code": "US",
                 "location_type": "Country", "available_languages": [
                    {"available_sources": ["google", "bing"], "language_name": "English",
                     "language_code": "en", "keywords": 1, "serps": 1},
                    {"available_sources": ["bing"], "language_name": "Spanish",
                     "language_code": "es", "keywords": 1, "serps": 1}]},
                {"location_code": 2250, "location_name": "France",
                 "location_code_parent": null, "country_iso_code": "FR",
                 "location_type": "Country", "available_languages": [
                    {"available_sources": ["google"], "language_name": "French",
                     "language_code": "fr", "keywords": 1, "serps": 1}]},
                {"location_code": 2999, "location_name": "Bingland",
                 "location_code_parent": null, "country_iso_code": "BL",
                 "location_type": "Country", "available_languages": [
                    {"available_sources": ["bing"], "language_name": "English",
                     "language_code": "en", "keywords": 1, "serps": 1}]}
            ]
        }]
    }"#;

    const SUGGESTIONS: &str = r#"{
        "version": "0.1.20260101", "status_code": 20000, "status_message": "Ok.",
        "time": "0.4 sec.", "cost": 0.01236, "tasks_count": 1, "tasks_error": 0,
        "tasks": [{
            "id": "11181418-1535-0387-0000-8a36a3e3e2e1",
            "status_code": 20000, "status_message": "Ok.", "cost": 0.01236,
            "result_count": 1,
            "result": [{
                "se_type": "google", "seed_keyword": "arbre à chat",
                "seed_keyword_data": {
                    "keyword": "arbre à chat",
                    "keyword_info": {"search_volume": 110000, "cpc": 0.38},
                    "keyword_properties": {"keyword_difficulty": 41},
                    "search_intent_info": {"main_intent": "commercial"}
                },
                "location_code": 2250, "language_code": "fr",
                "total_count": 4821, "items_count": 3,
                "items": [
                    {"keyword": "arbre à chat xxl",
                     "keyword_info": {"search_volume": 9900, "cpc": 0.295,
                                      "competition_level": "HIGH"},
                     "keyword_properties": {"core_keyword": null, "keyword_difficulty": 12},
                     "serp_info": null,
                     "search_intent_info": {"main_intent": "transactional",
                                            "foreign_intent": ["commercial"]}},
                    {"keyword": "arbre à chat",
                     "keyword_info": {"search_volume": 110000, "cpc": 0.38},
                     "keyword_properties": {"keyword_difficulty": 41},
                     "search_intent_info": {"main_intent": "commercial"}},
                    {"keyword": "fabriquer un arbre à chat",
                     "keyword_info": {"search_volume": 1900.0, "cpc": null},
                     "keyword_properties": {"keyword_difficulty": null},
                     "search_intent_info": null}
                ]
            }]
        }]
    }"#;

    fn body(code: u32, message: &str) -> String {
        json!({
            "status_code": 20000, "status_message": "Ok.", "cost": 0,
            "tasks": [{"status_code": code, "status_message": message, "cost": 0, "result": null}],
        })
        .to_string()
    }

    #[test]
    fn the_seed_comes_first_and_a_missing_figure_stays_missing() {
        let answer = open::<SuggestionsResult>(200, SUGGESTIONS.as_bytes()).unwrap();
        assert_eq!(answer.cost_usd_micros, 12_360);
        let rows = rows(answer.result);
        assert_eq!(
            rows,
            vec![
                Keyword {
                    keyword: "arbre à chat".into(),
                    volume: Some(110_000),
                    cpc_usd_cents: Some(38),
                    difficulty: Some(41),
                    intent: Some(Intent::Commercial),
                },
                Keyword {
                    keyword: "arbre à chat xxl".into(),
                    volume: Some(9_900),
                    cpc_usd_cents: Some(30),
                    difficulty: Some(12),
                    intent: Some(Intent::Transactional),
                },
                Keyword {
                    keyword: "fabriquer un arbre à chat".into(),
                    volume: Some(1_900),
                    cpc_usd_cents: None,
                    difficulty: None,
                    intent: None,
                },
            ]
        );
    }

    #[test]
    fn a_seed_nobody_searches_gives_no_rows() {
        // Said either as a task with no result, or as a result with no items.
        let none = body(40102, "No Search Results.");
        let answer = open::<SuggestionsResult>(200, none.as_bytes()).unwrap();
        assert!(rows(answer.result).is_empty());
        let empty = r#"{"status_code": 20000, "cost": 0.012, "tasks": [{"status_code": 20000,
            "cost": 0.012, "result": [{"seed_keyword_data": null, "items": null}]}]}"#;
        let answer = open::<SuggestionsResult>(200, empty.as_bytes()).unwrap();
        assert_eq!(answer.cost_usd_micros, 12_000);
        assert!(rows(answer.result).is_empty());
    }

    #[test]
    fn a_failure_is_one_to_ask_again_or_one_it_will_say_again() {
        let fails = |status: u16, body: &str| {
            open::<SuggestionsResult>(status, body.as_bytes())
                .err()
                .expect("a failure")
        };
        let again = |err: KeywordError| matches!(err, KeywordError::Unavailable(_));
        let refused = |err: KeywordError| matches!(err, KeywordError::Refused(_));

        // The task's own verdict, under HTTP 200.
        assert!(again(fails(200, &body(40202, "Rate limit exceeded."))));
        assert!(again(fails(200, &body(40209, "Too many queries."))));
        assert!(again(fails(200, &body(50000, "Internal Error."))));
        assert!(again(fails(200, &body(50401, "Timeout."))));
        assert!(refused(fails(200, &body(40501, "Invalid Field: 'x'."))));
        assert!(refused(fails(200, &body(40210, "Insufficient Funds."))));
        assert_eq!(
            fails(200, &body(40501, "Invalid Field: 'language_code'.")),
            KeywordError::Refused("Invalid Field: 'language_code'. (code 40501)".into())
        );

        // The call's verdict, whatever the HTTP status beside it.
        let unauthorized = r#"{"status_code": 40100, "status_message":
            "You are not authorized to access this resource.", "cost": 0, "tasks": null}"#;
        assert!(refused(fails(401, unauthorized)));
        assert!(refused(fails(200, unauthorized)));
        let payment = r#"{"status_code": 40200, "status_message": "Payment Required."}"#;
        assert!(refused(fails(402, payment)));

        // No body to read: the HTTP status alone.
        assert!(again(fails(502, "<html>Bad Gateway</html>")));
        assert!(again(fails(429, "")));
        assert!(refused(fails(403, "Forbidden")));
        assert!(again(fails(200, "<html>a proxy's page</html>")));
        assert!(again(fails(200, r#"{"status_code": 20000, "tasks": []}"#)));
    }

    #[test]
    fn only_the_countries_google_is_read_for_are_offered() {
        let answer = open::<LocationEntry>(200, PLACES.as_bytes()).unwrap();
        let located = located(answer.result);
        let places: Vec<_> = located
            .iter()
            .map(|l| {
                let languages: Vec<_> = l.place.languages.iter().map(|l| l.code.as_str()).collect();
                (l.place.country.as_str(), l.code, languages)
            })
            .collect();
        assert_eq!(
            places,
            vec![("FR", 2250, vec!["fr"]), ("US", 2840, vec!["en"])]
        );
    }

    /// One request as the stand-in read it.
    struct Request {
        line: String,
        authorization: String,
        body: String,
    }

    /// A stand-in for DataForSEO: the countries on a GET, `suggestions` on a
    /// POST, keeping what it was sent.
    async fn stand_in(suggestions: &'static str) -> (String, Arc<Mutex<Vec<Request>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        tokio::spawn({
            let requests = requests.clone();
            async move {
                loop {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let mut read = Vec::new();
                    let mut chunk = [0; 4096];
                    // The head, then as much body as it announces.
                    let (head, body) = loop {
                        let n = socket.read(&mut chunk).await.unwrap();
                        read.extend_from_slice(&chunk[..n]);
                        let text = String::from_utf8_lossy(&read).into_owned();
                        if let Some((head, body)) = text.split_once("\r\n\r\n") {
                            let length = head
                                .lines()
                                .find_map(|line| {
                                    line.to_lowercase()
                                        .strip_prefix("content-length:")
                                        .map(|v| v.trim().parse::<usize>().unwrap())
                                })
                                .unwrap_or(0);
                            if body.len() >= length || n == 0 {
                                break (head.to_owned(), body.to_owned());
                            }
                        }
                    };
                    let line = head.lines().next().unwrap_or_default().to_owned();
                    let authorization = head
                        .lines()
                        .find_map(|line| {
                            line.to_lowercase()
                                .starts_with("authorization:")
                                .then(|| line["authorization:".len()..].trim().to_owned())
                        })
                        .unwrap_or_default();
                    let answer = if line.starts_with("GET ") {
                        PLACES
                    } else {
                        suggestions
                    };
                    requests.lock().unwrap().push(Request {
                        line,
                        authorization,
                        body,
                    });
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
                         content-length: {}\r\nconnection: close\r\n\r\n{answer}",
                        answer.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                }
            }
        });
        (url, requests)
    }

    #[tokio::test]
    async fn a_seed_is_asked_under_the_login_for_the_country_by_its_code() {
        let (url, requests) = stand_in(SUGGESTIONS).await;
        let client = DataforseoClient::new("ops@example.com", "s3cret", Some(format!("{url}/")));

        let found = client
            .suggestions(" arbre à chat ", "fr", "FR")
            .await
            .unwrap();
        assert_eq!(found.rows.len(), 3);
        assert_eq!(found.cost_usd_micros, 12_360);
        // A second seed reads the countries from what the first kept.
        client.suggestions("griffoir", "FR", "fr").await.unwrap();

        let requests = requests.lock().unwrap();
        let lines: Vec<_> = requests.iter().map(|r| r.line.as_str()).collect();
        assert_eq!(
            lines,
            vec![
                "GET /v3/dataforseo_labs/locations_and_languages HTTP/1.1",
                "POST /v3/dataforseo_labs/google/keyword_suggestions/live HTTP/1.1",
                "POST /v3/dataforseo_labs/google/keyword_suggestions/live HTTP/1.1",
            ]
        );
        let login = base64::engine::general_purpose::STANDARD.encode("ops@example.com:s3cret");
        for request in requests.iter() {
            assert_eq!(request.authorization, format!("Basic {login}"));
        }
        let sent: serde_json::Value = serde_json::from_str(&requests[1].body).unwrap();
        assert_eq!(
            sent,
            json!([{
                "keyword": "arbre à chat",
                "location_code": 2250,
                "language_code": "fr",
                "include_seed_keyword": true,
                "limit": 1000,
            }])
        );
    }

    #[tokio::test]
    async fn a_country_or_a_language_it_has_nothing_for_is_refused_unasked() {
        let (url, requests) = stand_in(SUGGESTIONS).await;
        let client = DataforseoClient::new("login", "password", Some(url));
        for (seed, country, language) in [
            ("griffoir", "ZZ", "fr"),
            ("griffoir", "FR", "en"),
            // Spanish in the United States is held for Bing alone.
            ("griffoir", "US", "es"),
            (" ", "FR", "fr"),
        ] {
            let err = client
                .suggestions(seed, country, language)
                .await
                .unwrap_err();
            assert!(matches!(err, KeywordError::Refused(_)), "{err}");
        }
        let posts = requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.line.starts_with("POST "))
            .count();
        assert_eq!(posts, 0);
    }

    #[tokio::test]
    async fn a_source_that_does_not_answer_is_unavailable() {
        // Bound, then closed: nothing listens there.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let client = DataforseoClient::new("login", "s3cret", Some(url));
        let err = client.places().await.unwrap_err();
        assert!(matches!(err, KeywordError::Unavailable(_)), "{err}");
        assert!(!err.to_string().contains("s3cret"), "{err}");
        let err = client
            .suggestions("griffoir", "FR", "fr")
            .await
            .unwrap_err();
        assert!(matches!(err, KeywordError::Unavailable(_)), "{err}");
    }

    #[tokio::test]
    async fn the_countries_are_read_once_and_kept() {
        let (url, requests) = stand_in(SUGGESTIONS).await;
        let client = DataforseoClient::new("login", "password", Some(url));
        for _ in 0..3 {
            let places = client.places().await.unwrap();
            assert_eq!(places.len(), 2);
            assert_eq!(places[0].name, "France");
        }
        assert_eq!(requests.lock().unwrap().len(), 1);
    }
}
