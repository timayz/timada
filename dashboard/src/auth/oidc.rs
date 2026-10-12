//! OpenID Connect through discovery. Every configured provider is resolved
//! once at startup; a misconfigured one stops the server with its name.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use openidconnect::core::{
    CoreAuthenticationFlow, CoreClient, CoreIdTokenVerifier, CoreProviderMetadata,
};
use openidconnect::{
    AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointMaybeSet, EndpointNotSet,
    EndpointSet, IssuerUrl, Nonce, OAuth2TokenResponse, PkceCodeChallenge, PkceCodeVerifier,
    RedirectUrl, Scope, TokenResponse, reqwest,
};
use timada_identity::ProviderIdentity;

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
        let verifier: CoreIdTokenVerifier = provider.client.id_token_verifier();
        let id_token = response
            .id_token()
            .context("the provider returned no ID token")?;
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
