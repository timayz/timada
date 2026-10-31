use bitcode::{Decode, Encode};

use crate::error::IdentityError;

/// How a user proved who they are. Persisted in events: add variants, never
/// reorder or remove them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Encode, Decode)]
pub enum SignInMethod {
    #[default]
    OAuth2,
    Passkey,
}

/// What the browser asked for when it started an OIDC flow. Mirrors
/// oauth2-passkey's `OAuth2Mode`; not persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuth2Mode {
    /// Sign in an existing user; fail if the provider identity is unknown.
    Login,
    /// Create a user; fail if the provider identity is already registered.
    CreateUser,
    /// Sign in if known, otherwise create. The login page default.
    CreateUserOrLogin,
    /// Link the provider identity to the signed-in user.
    AddToUser,
}

impl OAuth2Mode {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "login" => Self::Login,
            "create_user" => Self::CreateUser,
            "create_user_or_login" => Self::CreateUserOrLogin,
            "add_to_user" => Self::AddToUser,
            _ => return None,
        })
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::CreateUser => "create_user",
            Self::CreateUserOrLogin => "create_user_or_login",
            Self::AddToUser => "add_to_user",
        }
    }
}

/// What a mailed one-time code proves. Persisted in events: add variants,
/// never reorder or remove them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Encode, Decode)]
pub enum ChallengePurpose {
    /// A visitor proves an address before an account is created for it.
    #[default]
    Register,
    /// A user who gave the right password proves they still hold the address.
    SignIn,
    /// A user who forgot their password proves they hold the address.
    ResetPassword,
}

impl ChallengePurpose {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "register" => Self::Register,
            "sign_in" => Self::SignIn,
            "reset_password" => Self::ResetPassword,
            _ => return None,
        })
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Register => "register",
            Self::SignIn => "sign_in",
            Self::ResetPassword => "reset_password",
        }
    }
}

/// Every way a user can sign in, for the sign-in history. Not persisted:
/// the password method has its own events, the others carry [`SignInMethod`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SignInKind {
    #[default]
    OAuth2,
    Passkey,
    Password,
}

impl From<SignInMethod> for SignInKind {
    fn from(method: SignInMethod) -> Self {
        match method {
            SignInMethod::OAuth2 => Self::OAuth2,
            SignInMethod::Passkey => Self::Passkey,
        }
    }
}

/// The verified claims of an OIDC identity token, reduced to what the domain
/// keeps. `provider` is the configured provider key, `provider_user_id` the
/// token's `sub`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderIdentity {
    pub provider: String,
    pub provider_user_id: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub picture: Option<String>,
}

impl ProviderIdentity {
    /// The login identifier a new user gets from this identity.
    pub fn account(&self) -> String {
        self.email
            .clone()
            .unwrap_or_else(|| format!("{}:{}", self.provider, self.provider_user_id))
    }

    /// The display name a new user gets from this identity.
    pub fn label(&self) -> String {
        self.name.clone().unwrap_or_else(|| self.account())
    }
}

/// Where a sign-in came from, recorded in the login history.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoginContext {
    pub user_agent: Option<String>,
    pub ip: Option<String>,
}

/// Aggregate id of the index entry for one provider identity.
pub fn oauth2_account_id(provider: &str, provider_user_id: &str) -> String {
    evento::hash_ids(vec!["oauth2", provider, provider_user_id])
}

/// Aggregate id of the index entry for one passkey, from its base64url
/// credential id (raw credential ids can be up to 1023 bytes; aggregate ids
/// are capped at 64 characters).
pub fn passkey_credential_id(credential_id: &str) -> String {
    evento::hash_ids(vec!["passkey", credential_id])
}

/// Aggregate id of a session, from the SHA-256 of its token. Only the hash
/// ever reaches the domain.
pub fn session_id(token_hash: &[u8; 32]) -> String {
    evento::hash_ids(vec!["session", &hex(token_hash)])
}

/// Aggregate id of the index entry for one e-mail address (normalized with
/// [`normalize_email`]).
pub fn email_account_id(email: &str) -> String {
    evento::hash_ids(vec!["email", email])
}

/// Aggregate id of the one-time codes mailed to one address.
pub fn email_challenge_id(email: &str) -> String {
    evento::hash_ids(vec!["email-challenge", email])
}

/// The canonical form of an address: trimmed, lower-cased, one `@` with
/// something on both sides, nothing that is not printable.
pub fn normalize_email(email: &str) -> Result<String, IdentityError> {
    timada_core::normalize_email(email).ok_or(IdentityError::InvalidEmail)
}

/// Aggregate id of the singleton that remembers who registered first.
pub const REGISTRY_ID: &str = "registry";

/// Longest address accepted (RFC 5321's path limit).
pub use timada_core::MAX_EMAIL_LEN;

/// How long a mailed code stays valid, in seconds.
pub const CODE_TTL_SECS: u64 = 10 * 60;

/// How long after a code is mailed another one may be requested for the
/// same address, in seconds.
pub const CODE_RESEND_AFTER_SECS: u64 = 60;

/// Wrong guesses after which a code is dead and a new one must be mailed.
pub const CODE_MAX_ATTEMPTS: u32 = 5;

/// Password length policy (NIST 800-63B: length, no composition rules).
pub const PASSWORD_MIN_LEN: usize = 8;
pub const PASSWORD_MAX_LEN: usize = 128;

/// Most passkeys one account may hold. Keeps the registration ceremony's
/// `excludeCredentials` list, which travels in a cookie, small.
pub const MAX_PASSKEYS_PER_USER: usize = 10;

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_fit_the_store() {
        assert_eq!(oauth2_account_id("google", "123").len(), 64);
        assert_eq!(passkey_credential_id(&"a".repeat(1500)).len(), 64);
        assert_eq!(session_id(&[7u8; 32]).len(), 64);
        assert_ne!(
            oauth2_account_id("google", "123"),
            oauth2_account_id("auth0", "123")
        );
    }

    #[test]
    fn emails_are_normalized_and_checked() {
        assert_eq!(
            normalize_email("  Alice@Example.COM ").unwrap(),
            "alice@example.com"
        );
        for bad in [
            "",
            "alice",
            "@example.com",
            "alice@",
            "a b@example.com",
            "a@b@c",
        ] {
            assert!(normalize_email(bad).is_err(), "{bad:?}");
        }
        assert!(normalize_email(&format!("{}@x.y", "a".repeat(260))).is_err());
        assert_eq!(email_account_id("alice@example.com").len(), 64);
        assert_ne!(
            email_challenge_id("alice@example.com"),
            email_account_id("alice@example.com")
        );
    }

    #[test]
    fn hex_is_lowercase_two_digits_per_byte() {
        assert_eq!(hex(&[0, 15, 255]), "000fff");
    }
}
