//! OpenID Connect through discovery. Every configured provider is resolved
//! once at startup; a misconfigured one stops the server with its name. A
//! provider's signing keys are read again as it rotates them (#556).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use openidconnect::core::{
    CoreAuthenticationFlow, CoreClient, CoreIdToken, CoreIdTokenVerifier, CoreJsonWebKeySet,
    CoreJwsSigningAlgorithm, CoreProviderMetadata,
};
use openidconnect::{
    AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointMaybeSet, EndpointNotSet,
    EndpointSet, IssuerUrl, JsonWebKeySetUrl, JwsSigningAlgorithm, Nonce, OAuth2TokenResponse,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope, SignatureVerificationError,
    TokenResponse, reqwest,
};
use timada_identity::ProviderIdentity;
use tokio::sync::Mutex;

use crate::config::AuthConfig;

/// A client built from discovery metadata: authorization endpoint known,
/// token and userinfo endpoints as the provider advertises them.
pub type Client = CoreClient<
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

pub struct Provider {
    pub name: String,
    pub label: String,
    pub client: Client,
    keys: Keyring,
}

/// How long one read of a provider's signing keys serves at most, whatever
/// its `Cache-Control` says: a key the provider retired stops being
/// accepted within this.
const KEYS_FRESH_FOR: Duration = Duration::from_secs(60 * 60);
/// The shortest wait between two reads of a provider's keys, so that tokens
/// naming unknown keys cannot make the node hammer the provider.
const KEYS_REREAD_FLOOR: Duration = Duration::from_secs(60);
/// How long one read of the keys may take: sign-ins through the provider
/// wait on it.
const KEYS_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// A provider's signing keys and what the ID token verifier needs besides.
/// The keys are read again once they are stale, and as soon as a token
/// names a key they do not hold: the provider rotated them.
struct Keyring {
    client_id: ClientId,
    client_secret: ClientSecret,
    issuer: IssuerUrl,
    algs: Vec<CoreJwsSigningAlgorithm>,
    uri: JsonWebKeySetUrl,
    state: Mutex<Keys>,
}

struct Keys {
    set: CoreJsonWebKeySet,
    fresh_until: Instant,
    /// When the keys were last read, or a read last failed.
    tried_at: Instant,
}

/// Every enabled provider plus the HTTP client used to talk to them.
/// Registered in topcoat's app context.
pub struct Providers {
    http: reqwest::Client,
    by_name: BTreeMap<String, Provider>,
}

/// What the browser needs to remember until the callback.
pub struct Authorization {
    pub url: String,
    pub state: String,
    pub nonce: String,
    pub pkce_verifier: String,
}

/// The redirect URI registered at the provider for `name`. One place, so the
/// startup configuration and the callback route cannot drift apart.
pub fn callback_url(base_url: &str, name: &str) -> String {
    format!(
        "{}/auth/oauth2/{name}/callback",
        base_url.trim_end_matches('/')
    )
}

impl Providers {
    pub async fn discover(config: &AuthConfig, base_url: &str) -> Result<Self> {
        // Following redirects would open the token exchange to SSRF.
        let http = reqwest::ClientBuilder::new()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("cannot build the HTTP client for sign-in providers")?;

        let mut by_name = BTreeMap::new();
        for (name, provider) in &config.providers {
            let issuer = IssuerUrl::new(provider.issuer.clone())
                .with_context(|| format!("auth.providers.{name}.issuer is not a valid URL"))?;
            let metadata = CoreProviderMetadata::discover_async(issuer, &http)
                .await
                .with_context(|| format!("auth.providers.{name}: discovery failed"))?;
            let now = Instant::now();
            let keys = Keyring {
                client_id: ClientId::new(provider.client_id.clone()),
                client_secret: ClientSecret::new(provider.client_secret.clone()),
                issuer: metadata.issuer().clone(),
                algs: metadata.id_token_signing_alg_values_supported().clone(),
                uri: metadata.jwks_uri().clone(),
                state: Mutex::new(Keys {
                    set: metadata.jwks().clone(),
                    fresh_until: now + KEYS_FRESH_FOR,
                    tried_at: now,
                }),
            };
            let client = CoreClient::from_provider_metadata(
                metadata,
                ClientId::new(provider.client_id.clone()),
                Some(ClientSecret::new(provider.client_secret.clone())),
            )
            .set_redirect_uri(
                RedirectUrl::new(callback_url(base_url, name))
                    .with_context(|| format!("auth.providers.{name}: invalid redirect URI"))?,
            );
            tracing::info!(provider = %name, issuer = %provider.issuer, "sign-in provider ready");
            by_name.insert(
                name.clone(),
                Provider {
                    name: name.clone(),
                    label: provider.label.clone(),
                    client,
                    keys,
                },
            );
        }
        Ok(Self { http, by_name })
    }

    pub fn get(&self, name: &str) -> Option<&Provider> {
        self.by_name.get(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Provider> {
        self.by_name.values()
    }

    /// Builds the authorization URL with PKCE, a fresh `state` and a fresh
    /// `nonce`.
    pub fn authorize(&self, provider: &Provider) -> Authorization {
        let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
        let (url, state, nonce) = provider
            .client
            .authorize_url(
                CoreAuthenticationFlow::AuthorizationCode,
                CsrfToken::new_random,
                Nonce::new_random,
            )
            .add_scope(Scope::new("email".to_owned()))
            .add_scope(Scope::new("profile".to_owned()))
            .set_pkce_challenge(pkce_challenge)
            .url();
        Authorization {
            url: url.to_string(),
            state: state.secret().to_owned(),
            nonce: nonce.secret().to_owned(),
            pkce_verifier: pkce_verifier.secret().to_owned(),
        }
    }

    /// Exchanges the authorization code and verifies the ID token (signature,
    /// issuer, audience, expiry and the nonce from the flow cookie).
    pub async fn exchange(
        &self,
        provider: &Provider,
        code: String,
        pkce_verifier: String,
        nonce: String,
    ) -> Result<ProviderIdentity> {
        let response = provider
            .client
            .exchange_code(AuthorizationCode::new(code))
            .context("the provider advertises no token endpoint")?
            .set_pkce_verifier(PkceCodeVerifier::new(pkce_verifier))
            .request_async(&self.http)
            .await
            .context("token exchange failed")?;
        let id_token = response
            .id_token()
            .context("the provider returned no ID token")?;
        let verifier = provider
            .keys
            .verifier(&self.http, &provider.name, id_token, Instant::now())
            .await;
        let claims = id_token
            .claims(&verifier, &Nonce::new(nonce))
            .context("ID token verification failed")?;

        // The access token must belong to this ID token when the token
        // carries an `at_hash` claim.
        if let Some(expected) = claims.access_token_hash() {
            let actual = openidconnect::AccessTokenHash::from_token(
                response.access_token(),
                id_token.signing_alg()?,
                id_token.signing_key(&verifier)?,
            )?;
            anyhow::ensure!(&actual == expected, "access token hash mismatch");
        }

        Ok(ProviderIdentity {
            provider: provider.name.clone(),
            provider_user_id: claims.subject().as_str().to_owned(),
            email: claims.email().map(|e| e.as_str().to_owned()),
            name: claims
                .name()
                .and_then(|n| n.get(None))
                .map(|n| n.as_str().to_owned()),
            picture: claims
                .picture()
                .and_then(|p| p.get(None))
                .map(|p| p.as_str().to_owned()),
        })
    }
}

impl Keyring {
    /// A verifier holding the provider's current keys. They are read again
    /// first when they are stale, or when `id_token` names a key they do not
    /// hold and the last read is older than the floor. A failed read keeps
    /// the keys held: the check then fails or passes on them.
    async fn verifier(
        &self,
        http: &reqwest::Client,
        provider: &str,
        id_token: &CoreIdToken,
        now: Instant,
    ) -> CoreIdTokenVerifier<'static> {
        let mut keys = self.state.lock().await;
        let may_read = now >= keys.tried_at + KEYS_REREAD_FLOOR;
        if may_read && now >= keys.fresh_until {
            self.read(http, provider, &mut keys, now).await;
        } else if may_read {
            let verifier = self.verifier_with(keys.set.clone());
            if unknown_key(id_token, &verifier) {
                self.read(http, provider, &mut keys, now).await;
            } else {
                return verifier;
            }
        }
        self.verifier_with(keys.set.clone())
    }

    fn verifier_with(&self, set: CoreJsonWebKeySet) -> CoreIdTokenVerifier<'static> {
        CoreIdTokenVerifier::new_confidential_client(
            self.client_id.clone(),
            self.client_secret.clone(),
            self.issuer.clone(),
            set,
        )
        .set_allowed_algs(self.algs.clone())
    }

    async fn read(&self, http: &reqwest::Client, provider: &str, keys: &mut Keys, now: Instant) {
        keys.tried_at = now;
        match fetch_keys(http, &self.uri).await {
            Ok((set, fresh_for)) => {
                tracing::info!(provider, keys = set.keys().len(), "signing keys read");
                keys.set = set;
                keys.fresh_until = now + fresh_for;
            }
            Err(error) => {
                tracing::warn!(
                    provider,
                    error = format!("{error:#}"),
                    "cannot read signing keys"
                );
            }
        }
    }
}

/// Whether `id_token` is signed with a key the verifier does not hold. A
/// token signed with the client secret needs none.
fn unknown_key(id_token: &CoreIdToken, verifier: &CoreIdTokenVerifier) -> bool {
    match id_token.signing_alg() {
        Ok(alg) if !alg.uses_shared_secret() => matches!(
            id_token.signing_key(verifier),
            Err(SignatureVerificationError::NoMatchingKey)
        ),
        _ => false,
    }
}

/// Reads a key set and how long it may serve: the response's
/// `Cache-Control: max-age`, between the floor and an hour.
async fn fetch_keys(
    http: &reqwest::Client,
    uri: &JsonWebKeySetUrl,
) -> Result<(CoreJsonWebKeySet, Duration)> {
    let response = http
        .get(uri.url().clone())
        .header(reqwest::header::ACCEPT, "application/json")
        .timeout(KEYS_READ_TIMEOUT)
        .send()
        .await?
        .error_for_status()?;
    let fresh_for = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .and_then(max_age)
        .map_or(KEYS_FRESH_FOR, |age| {
            age.clamp(KEYS_REREAD_FLOOR, KEYS_FRESH_FOR)
        });
    let body = response.bytes().await?;
    let set = serde_json::from_slice(&body).context("the key set does not parse")?;
    Ok((set, fresh_for))
}

/// The `max-age` of a `Cache-Control` value; `no-cache` and `no-store`
/// count as none left.
fn max_age(cache_control: &str) -> Option<Duration> {
    let mut age = None;
    for directive in cache_control.split(',').map(str::trim) {
        let lower = directive.to_ascii_lowercase();
        if lower == "no-cache" || lower == "no-store" {
            return Some(Duration::ZERO);
        }
        if let Some(seconds) = lower.strip_prefix("max-age=") {
            age = seconds
                .trim_matches('"')
                .parse()
                .ok()
                .map(Duration::from_secs);
        }
    }
    age
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use openidconnect::core::{CoreEdDsaPrivateSigningKey, CoreIdTokenClaims};
    use openidconnect::{JsonWebKeyId, PrivateSigningKey};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    const ISSUER: &str = "https://issuer.test";
    const CLIENT: &str = "timada";

    fn key(kid: &str) -> CoreEdDsaPrivateSigningKey {
        let pem = rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519)
            .unwrap()
            .serialize_pem();
        CoreEdDsaPrivateSigningKey::from_ed25519_pem(&pem, Some(JsonWebKeyId::new(kid.into())))
            .unwrap()
    }

    fn set(keys: &[&CoreEdDsaPrivateSigningKey]) -> CoreJsonWebKeySet {
        CoreJsonWebKeySet::new(keys.iter().map(|k| k.as_verification_key()).collect())
    }

    fn token(key: &CoreEdDsaPrivateSigningKey) -> CoreIdToken {
        let now = super::super::now();
        let claims: CoreIdTokenClaims = serde_json::from_value(serde_json::json!({
            "iss": ISSUER,
            "aud": CLIENT,
            "sub": "user",
            "iat": now,
            "exp": now + 300,
        }))
        .unwrap();
        CoreIdToken::new(claims, key, CoreJwsSigningAlgorithm::EdDsa, None, None).unwrap()
    }

    fn accepts(verifier: &CoreIdTokenVerifier, token: &CoreIdToken) -> bool {
        token
            .claims(verifier, |_: Option<&Nonce>| Ok::<(), String>(()))
            .is_ok()
    }

    /// Serves `body` with `cache_control` at `/keys`, counting the reads.
    async fn provider(
        body: CoreJsonWebKeySet,
        cache_control: &str,
    ) -> (JsonWebKeySetUrl, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/keys", listener.local_addr().unwrap());
        let reads = Arc::new(AtomicUsize::new(0));
        let body = serde_json::to_string(&body).unwrap();
        let cache_control = cache_control.to_owned();
        tokio::spawn({
            let reads = reads.clone();
            async move {
                loop {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let mut request = [0; 4096];
                    let _ = socket.read(&mut request).await;
                    reads.fetch_add(1, Ordering::SeqCst);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
                         cache-control: {cache_control}\r\ncontent-length: {}\r\n\
                         connection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                }
            }
        });
        (JsonWebKeySetUrl::new(url).unwrap(), reads)
    }

    fn keyring(
        uri: JsonWebKeySetUrl,
        held: CoreJsonWebKeySet,
        now: Instant,
        fresh_for: Duration,
    ) -> Keyring {
        Keyring {
            client_id: ClientId::new(CLIENT.into()),
            client_secret: ClientSecret::new("secret".into()),
            issuer: IssuerUrl::new(ISSUER.into()).unwrap(),
            algs: vec![CoreJwsSigningAlgorithm::EdDsa],
            uri,
            state: Mutex::new(Keys {
                set: held,
                fresh_until: now + fresh_for,
                tried_at: now - KEYS_REREAD_FLOOR,
            }),
        }
    }

    #[tokio::test]
    async fn a_token_signed_with_a_new_key_reads_the_keys_again() {
        let (old, new) = (key("old"), key("new"));
        let (uri, reads) = provider(set(&[&old, &new]), "max-age=3600").await;
        let now = Instant::now();
        let keys = keyring(uri, set(&[&old]), now, KEYS_FRESH_FOR);
        let http = reqwest::Client::new();

        let verifier = keys.verifier(&http, "dex", &token(&new), now).await;
        assert!(accepts(&verifier, &token(&new)));
        assert_eq!(reads.load(Ordering::SeqCst), 1);

        // The key held now: no read.
        let verifier = keys.verifier(&http, "dex", &token(&new), now).await;
        assert!(accepts(&verifier, &token(&old)));
        assert_eq!(reads.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn unknown_keys_read_the_keys_at_most_once_a_floor() {
        let (held, forged) = (key("held"), key("forged"));
        let (uri, reads) = provider(set(&[&held]), "max-age=3600").await;
        let now = Instant::now();
        let keys = keyring(uri, set(&[&held]), now, KEYS_FRESH_FOR);
        let http = reqwest::Client::new();

        let verifier = keys.verifier(&http, "dex", &token(&forged), now).await;
        assert!(!accepts(&verifier, &token(&forged)));
        assert_eq!(reads.load(Ordering::SeqCst), 1);

        let soon = now + KEYS_REREAD_FLOOR - Duration::from_secs(1);
        let verifier = keys.verifier(&http, "dex", &token(&forged), soon).await;
        assert!(!accepts(&verifier, &token(&forged)));
        assert!(accepts(&verifier, &token(&held)));
        assert_eq!(reads.load(Ordering::SeqCst), 1);

        let later = now + KEYS_REREAD_FLOOR;
        keys.verifier(&http, "dex", &token(&forged), later).await;
        assert_eq!(reads.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn stale_keys_are_read_again_and_a_retired_key_is_refused() {
        let (retired, current) = (key("retired"), key("current"));
        let (uri, reads) = provider(set(&[&current]), "public, max-age=120").await;
        let now = Instant::now();
        let keys = keyring(uri, set(&[&retired, &current]), now, Duration::ZERO);
        let http = reqwest::Client::new();

        let verifier = keys.verifier(&http, "dex", &token(&retired), now).await;
        assert!(!accepts(&verifier, &token(&retired)));
        assert!(accepts(&verifier, &token(&current)));
        assert_eq!(reads.load(Ordering::SeqCst), 1);

        // Fresh for the provider's two minutes, then read again.
        let fresh = now + Duration::from_secs(119);
        keys.verifier(&http, "dex", &token(&current), fresh).await;
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        let stale = now + Duration::from_secs(120);
        keys.verifier(&http, "dex", &token(&current), stale).await;
        assert_eq!(reads.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_failed_read_keeps_the_keys_held() {
        let (held, new) = (key("held"), key("new"));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let uri = JsonWebKeySetUrl::new(format!("http://{}/keys", listener.local_addr().unwrap()))
            .unwrap();
        drop(listener);
        let now = Instant::now();
        let keys = keyring(uri, set(&[&held]), now, Duration::ZERO);

        let verifier = keys
            .verifier(&reqwest::Client::new(), "dex", &token(&new), now)
            .await;
        assert!(accepts(&verifier, &token(&held)));
        assert!(!accepts(&verifier, &token(&new)));
    }

    #[test]
    fn max_age_reads_cache_control() {
        assert_eq!(
            max_age("public, max-age=21600, must-revalidate"),
            Some(Duration::from_secs(21600))
        );
        assert_eq!(max_age("Max-Age=60"), Some(Duration::from_secs(60)));
        assert_eq!(max_age("no-store, max-age=60"), Some(Duration::ZERO));
        assert_eq!(max_age("public"), None);
        assert_eq!(max_age("max-age=soon"), None);
    }
}
