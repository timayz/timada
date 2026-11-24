use std::sync::{Arc, Mutex};

use timada_google::{ACCESS_SEARCH_CONSOLE, Command, Connect, ConnectionState};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;
use crate::config::GoogleConfig;

pub(crate) const SHOP: &str = "tenant-shop";

/// One request as the stand-in read it.
#[derive(Debug, Clone)]
pub(crate) struct Request {
    /// `POST /token HTTP/1.1`.
    pub line: String,
    pub authorization: String,
    pub body: String,
}

/// A stand-in for Google: `answer` says what each request gets, and what
/// was asked is kept.
pub(crate) struct StandIn {
    pub url: String,
    pub requests: Arc<Mutex<Vec<Request>>>,
}

impl StandIn {
    pub async fn start(answer: impl Fn(&Request) -> (u16, String) + Send + Sync + 'static) -> Self {
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
                            if body.len() >= length {
                                break (head.to_owned(), body.to_owned());
                            }
                        }
                        if n == 0 {
                            break (text, String::new());
                        }
                    };
                    let request = Request {
                        line: head.lines().next().unwrap_or_default().to_owned(),
                        authorization: head
                            .lines()
                            .find_map(|line| {
                                line.to_lowercase()
                                    .starts_with("authorization:")
                                    .then(|| line[14..].trim().to_owned())
                            })
                            .unwrap_or_default(),
                        body,
                    };
                    let (status, body) = answer(&request);
                    requests.lock().unwrap().push(request);
                    let response = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                }
            }
        });
        Self { url, requests }
    }

    /// The request lines so far.
    pub fn lines(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|request| request.line.clone())
            .collect()
    }

    /// How many requests went to the token address.
    pub fn renewals(&self) -> usize {
        self.lines()
            .iter()
            .filter(|line| line.starts_with("POST /token"))
            .count()
    }

    /// Google pointed at the stand-in.
    pub fn google(&self) -> Google {
        Google::new(
            Some(Sealer::new(
                "google",
                "a-test-secret-of-at-least-32-characters",
            )),
            &GoogleConfig {
                client_id: Some("client-id".into()),
                client_secret: Some("client-s3cret".into()),
                authorize_url: Some(format!("{}/o/oauth2/v2/auth", self.url)),
                token_url: Some(format!("{}/token", self.url)),
                userinfo_url: Some(format!("{}/v1/userinfo", self.url)),
                search_console_url: Some(format!("{}/webmasters/v3", self.url)),
                ..GoogleConfig::default()
            },
        )
    }
}

/// What the token address answers a good request with.
pub(crate) fn token_answer(access: &str) -> (u16, String) {
    (
        200,
        format!(
            r#"{{"access_token":"{access}","expires_in":3599,"token_type":"Bearer","scope":"openid https://www.googleapis.com/auth/userinfo.email https://www.googleapis.com/auth/webmasters.readonly"}}"#
        ),
    )
}

/// A shop connected in `store` under a grant sealed by `google`.
pub(crate) async fn connected(
    store: &evento::Turso,
    google: &Google,
    tenant_id: &str,
    refresh_token: &str,
) -> ConnectionState {
    let command = Command(store.clone());
    command
        .connect(
            "user-alice",
            tenant_id,
            Connect {
                refresh_token_sealed: google.seal(tenant_id, refresh_token).unwrap(),
                account: "alice@example.com".into(),
                accesses: vec![ACCESS_SEARCH_CONSOLE.into()],
            },
        )
        .await
        .unwrap();
    command.load_connection(tenant_id).await.unwrap().unwrap()
}

#[test]
fn google_is_off_without_an_application_or_a_key() {
    let sealer = || {
        Some(Sealer::new(
            "google",
            "a-test-secret-of-at-least-32-characters",
        ))
    };
    let set = GoogleConfig {
        client_id: Some("client-id".into()),
        client_secret: Some("client-s3cret".into()),
        ..GoogleConfig::default()
    };
    assert!(Google::new(sealer(), &set).enabled());
    assert!(!Google::new(None, &set).enabled());
    let off = Google::new(sealer(), &GoogleConfig::default());
    assert!(!off.enabled());
    assert!(off.authorize_url("https://x/cb", "s", &[]).is_err());
}

#[test]
fn a_merchant_is_sent_to_say_yes_to_what_the_shop_turns_on() {
    let google = Google::new(
        Some(Sealer::new(
            "google",
            "a-test-secret-of-at-least-32-characters",
        )),
        &GoogleConfig {
            client_id: Some("client-id".into()),
            client_secret: Some("client-s3cret".into()),
            ..GoogleConfig::default()
        },
    );
    let url = google
        .authorize_url(
            "https://timada.example/google/callback",
            "state-1",
            &[ACCESS_SEARCH_CONSOLE.to_owned()],
        )
        .unwrap();
    let url = reqwest::Url::parse(&url).unwrap();
    assert_eq!(
        url.origin().ascii_serialization(),
        "https://accounts.google.com"
    );
    assert_eq!(url.path(), "/o/oauth2/v2/auth");
    let param = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
            .unwrap_or_default()
    };
    assert_eq!(param("client_id"), "client-id");
    assert_eq!(
        param("redirect_uri"),
        "https://timada.example/google/callback"
    );
    assert_eq!(param("response_type"), "code");
    assert_eq!(
        param("scope"),
        "openid email https://www.googleapis.com/auth/webmasters.readonly"
    );
    // What makes Google hand a refresh token back, and keep what the
    // account granted before.
    assert_eq!(param("access_type"), "offline");
    assert_eq!(param("prompt"), "consent");
    assert_eq!(param("include_granted_scopes"), "true");
    assert_eq!(param("state"), "state-1");
    // The secret never travels in the address.
    assert!(!url.as_str().contains("s3cret"));
    assert!(
        google
            .authorize_url("https://x/cb", "s", &["drive".to_owned()])
            .is_err()
    );
}

#[tokio::test]
async fn a_code_is_exchanged_for_a_grant_and_its_account() {
    let stand_in = StandIn::start(|request| {
        if request.line.starts_with("POST /token") {
            (
                200,
                r#"{"access_token":"access-1","expires_in":3599,"refresh_token":"refresh-1","scope":"https://www.googleapis.com/auth/webmasters.readonly openid https://www.googleapis.com/auth/userinfo.email","token_type":"Bearer","id_token":"x"}"#.into(),
            )
        } else {
            (200, r#"{"sub":"1","email":"alice@example.com","email_verified":true}"#.into())
        }
    })
    .await;
    let google = stand_in.google();
    let grant = google
        .exchange("code-1", "https://timada.example/google/callback")
        .await
        .unwrap();
    assert_eq!(
        grant,
        Grant {
            refresh_token: Some("refresh-1".into()),
            access_token: "access-1".into(),
            expires_in: 3599,
            accesses: vec![ACCESS_SEARCH_CONSOLE.into()],
        }
    );
    assert_eq!(
        google.account(&grant.access_token).await.unwrap(),
        "alice@example.com"
    );
    let requests = stand_in.requests.lock().unwrap().clone();
    assert_eq!(requests[0].line, "POST /token HTTP/1.1");
    for sent in [
        "grant_type=authorization_code",
        "code=code-1",
        "client_id=client-id",
        "client_secret=client-s3cret",
        "redirect_uri=https%3A%2F%2Ftimada.example%2Fgoogle%2Fcallback",
    ] {
        assert!(requests[0].body.contains(sent), "{}", requests[0].body);
    }
    assert_eq!(requests[1].line, "GET /v1/userinfo HTTP/1.1");
    assert_eq!(requests[1].authorization, "Bearer access-1");
}

#[tokio::test]
async fn a_code_google_turns_down_is_told_apart_from_no_answer() {
    let stand_in = StandIn::start(|request| {
        if request.body.contains("code=late") {
            (503, "busy".into())
        } else {
            (
                400,
                r#"{"error":"redirect_uri_mismatch","error_description":"Bad Request"}"#.into(),
            )
        }
    })
    .await;
    let google = stand_in.google();
    assert_eq!(
        google.exchange("code-1", "https://x/cb").await,
        Err(GoogleFault::Refused("Bad Request".into()))
    );
    assert!(matches!(
        google.exchange("late", "https://x/cb").await,
        Err(GoogleFault::Unavailable(_))
    ));
}

#[tokio::test]
async fn an_access_is_renewed_from_the_grant_and_held_while_it_is_good() {
    let stand_in = StandIn::start(|request| {
        if request.body.contains("refresh_token=refresh-2") {
            token_answer("access-2")
        } else {
            token_answer("access-1")
        }
    })
    .await;
    let google = stand_in.google();
    let store = evento::Turso::in_memory_concurrent().await.unwrap();
    let connection = connected(&store, &google, SHOP, "refresh-1").await;

    assert_eq!(
        google.access(&store, &connection).await.unwrap(),
        "access-1"
    );
    assert_eq!(
        google.access(&store, &connection).await.unwrap(),
        "access-1"
    );
    assert_eq!(stand_in.renewals(), 1);
    let sent = stand_in.requests.lock().unwrap()[0].body.clone();
    for part in [
        "grant_type=refresh_token",
        "refresh_token=refresh-1",
        "client_id=client-id",
        "client_secret=client-s3cret",
    ] {
        assert!(sent.contains(part), "{sent}");
    }

    // Another grant (the merchant connected again): the access held was
    // the earlier one's.
    let connection = connected(&store, &google, SHOP, "refresh-2").await;
    assert_eq!(
        google.access(&store, &connection).await.unwrap(),
        "access-2"
    );
    assert_eq!(stand_in.renewals(), 2);
}

#[tokio::test]
async fn the_access_a_grant_came_with_is_used_first() {
    let stand_in = StandIn::start(|_| token_answer("access-9")).await;
    let google = stand_in.google();
    let store = evento::Turso::in_memory_concurrent().await.unwrap();
    let connection = connected(&store, &google, SHOP, "refresh-1").await;
    google.keep(
        SHOP,
        &connection.refresh_token_sealed,
        &Grant {
            refresh_token: Some("refresh-1".into()),
            access_token: "access-1".into(),
            expires_in: 3599,
            accesses: vec![ACCESS_SEARCH_CONSOLE.into()],
        },
    );
    assert_eq!(
        google.access(&store, &connection).await.unwrap(),
        "access-1"
    );
    assert_eq!(stand_in.renewals(), 0);

    // One about to lapse is not handed out.
    google.keep(
        SHOP,
        &connection.refresh_token_sealed,
        &Grant {
            refresh_token: None,
            access_token: "access-2".into(),
            expires_in: 120,
            accesses: Vec::new(),
        },
    );
    assert_eq!(
        google.access(&store, &connection).await.unwrap(),
        "access-9"
    );
    assert_eq!(stand_in.renewals(), 1);
}

#[tokio::test]
async fn a_grant_google_no_longer_honours_is_recorded_as_lapsed() {
    let stand_in = StandIn::start(|_| {
        (
            400,
            r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#
                .into(),
        )
    })
    .await;
    let google = stand_in.google();
    let store = evento::Turso::in_memory_concurrent().await.unwrap();
    let connection = connected(&store, &google, SHOP, "refresh-1").await;

    assert_eq!(
        google.access(&store, &connection).await,
        Err(GoogleFault::ConnectAgain)
    );
    let connection = Command(store.clone())
        .load_connection(SHOP)
        .await
        .unwrap()
        .unwrap();
    assert!(connection.lapsed);
    assert_eq!(connection.aggregate_version, 2);
    // Known as lapsed: Google is not asked again.
    assert_eq!(
        google.access(&store, &connection).await,
        Err(GoogleFault::ConnectAgain)
    );
    assert_eq!(stand_in.renewals(), 1);
}

#[tokio::test]
async fn google_not_answering_says_nothing_of_the_grant() {
    let stand_in = StandIn::start(|_| (500, "{}".into())).await;
    let google = stand_in.google();
    let store = evento::Turso::in_memory_concurrent().await.unwrap();
    let connection = connected(&store, &google, SHOP, "refresh-1").await;
    assert!(matches!(
        google.access(&store, &connection).await,
        Err(GoogleFault::Unavailable(_))
    ));
    let connection = Command(store.clone())
        .load_connection(SHOP)
        .await
        .unwrap()
        .unwrap();
    assert!(!connection.lapsed);
}

#[tokio::test]
async fn a_grant_sealed_under_another_key_asks_to_connect_again() {
    let stand_in = StandIn::start(|_| token_answer("access-1")).await;
    let google = stand_in.google();
    let store = evento::Turso::in_memory_concurrent().await.unwrap();
    Command(store.clone())
        .connect(
            "user-alice",
            SHOP,
            Connect {
                refresh_token_sealed: Sealer::new(
                    "google",
                    "another-secret-of-at-least-32-characters",
                )
                .seal_for(SHOP, "refresh-1")
                .unwrap(),
                account: "alice@example.com".into(),
                accesses: vec![ACCESS_SEARCH_CONSOLE.into()],
            },
        )
        .await
        .unwrap();
    let connection = Command(store.clone())
        .load_connection(SHOP)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        google.access(&store, &connection).await,
        Err(GoogleFault::ConnectAgain)
    );
    assert_eq!(stand_in.renewals(), 0);
}

#[tokio::test]
async fn the_sites_an_account_reads_are_listed_without_the_unproved_ones() {
    let stand_in = StandIn::start(|request| {
        if request.line.starts_with("POST /token") {
            token_answer("access-1")
        } else {
            (
                200,
                r#"{"siteEntry":[
                    {"siteUrl":"sc-domain:shop.example","permissionLevel":"siteOwner"},
                    {"siteUrl":"https://other.example/","permissionLevel":"siteUnverifiedUser"},
                    {"siteUrl":"https://blog.example/","permissionLevel":"siteRestrictedUser"}
                ]}"#
                .into(),
            )
        }
    })
    .await;
    let google = stand_in.google();
    let store = evento::Turso::in_memory_concurrent().await.unwrap();
    let connection = connected(&store, &google, SHOP, "refresh-1").await;
    let sites = google
        .read(&store, &connection, |access| {
            let google = google.clone();
            async move { google.sites(&access).await }
        })
        .await
        .unwrap();
    assert_eq!(
        sites,
        vec![
            Site {
                site: "https://blog.example/".into(),
                owner: false
            },
            Site {
                site: "sc-domain:shop.example".into(),
                owner: true
            },
        ]
    );
    let requests = stand_in.requests.lock().unwrap().clone();
    assert_eq!(requests[1].line, "GET /webmasters/v3/sites HTTP/1.1");
    assert_eq!(requests[1].authorization, "Bearer access-1");

    // An account with no site answers an empty object.
    let empty = StandIn::start(|_| (200, "{}".into())).await;
    assert!(empty.google().sites("access").await.unwrap().is_empty());
}

#[tokio::test]
async fn an_access_turned_down_is_asked_for_again_once() {
    // The access held is turned down; the one renewed is taken.
    let stand_in = StandIn::start(|request| {
        if request.line.starts_with("POST /token") {
            token_answer("access-2")
        } else if request.authorization == "Bearer access-2" {
            (200, r#"{"siteEntry":[]}"#.into())
        } else {
            (
                401,
                r#"{"error":{"code":401,"message":"Invalid Credentials"}}"#.into(),
            )
        }
    })
    .await;
    let google = stand_in.google();
    let store = evento::Turso::in_memory_concurrent().await.unwrap();
    let connection = connected(&store, &google, SHOP, "refresh-1").await;
    google.keep(
        SHOP,
        &connection.refresh_token_sealed,
        &Grant {
            refresh_token: None,
            access_token: "access-1".into(),
            expires_in: 3599,
            accesses: Vec::new(),
        },
    );
    let sites = google
        .read(&store, &connection, |access| {
            let google = google.clone();
            async move { google.sites(&access).await }
        })
        .await
        .unwrap();
    assert!(sites.is_empty());
    assert_eq!(
        stand_in.lines(),
        vec![
            "GET /webmasters/v3/sites HTTP/1.1",
            "POST /token HTTP/1.1",
            "GET /webmasters/v3/sites HTTP/1.1",
        ]
    );

    // The grant itself withdrawn: the renewal is refused, and recorded.
    let withdrawn = StandIn::start(|request| {
        if request.line.starts_with("POST /token") {
            (400, r#"{"error":"invalid_grant"}"#.into())
        } else {
            (401, r#"{"error":{"message":"Invalid Credentials"}}"#.into())
        }
    })
    .await;
    let google = withdrawn.google();
    let connection = connected(&store, &google, SHOP, "refresh-3").await;
    google.keep(
        SHOP,
        &connection.refresh_token_sealed,
        &Grant {
            refresh_token: None,
            access_token: "access-1".into(),
            expires_in: 3599,
            accesses: Vec::new(),
        },
    );
    let read = google
        .read(&store, &connection, |access| {
            let google = google.clone();
            async move { google.sites(&access).await }
        })
        .await;
    assert_eq!(read, Err(GoogleFault::ConnectAgain));
    assert!(
        Command(store.clone())
            .load_connection(SHOP)
            .await
            .unwrap()
            .unwrap()
            .lapsed
    );
}

#[tokio::test]
async fn a_refusal_of_the_api_is_kept_in_its_words() {
    let stand_in = StandIn::start(|request| {
        if request.authorization == "Bearer slow" {
            (429, r#"{"error":{"message":"Quota exceeded"}}"#.into())
        } else {
            (
                403,
                r#"{"error":{"code":403,"message":"User does not have sufficient permission for site."}}"#.into(),
            )
        }
    })
    .await;
    let google = stand_in.google();
    assert_eq!(
        google.sites("access").await,
        Err(GoogleFault::Refused(
            "User does not have sufficient permission for site.".into()
        ))
    );
    assert_eq!(
        google.sites("slow").await,
        Err(GoogleFault::Unavailable("Quota exceeded".into()))
    );
}
