use bitcode::{Decode, Encode};

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

/// Why a passkey registration ceremony was started. Not persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationMode {
    /// A visitor without a session creates an account with this passkey.
    CreateUser,
    /// A signed-in user adds a passkey to their account.
    AddToUser,
}

impl RegistrationMode {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "create_user" => Self::CreateUser,
            "add_to_user" => Self::AddToUser,
            _ => return None,
        })
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CreateUser => "create_user",
            Self::AddToUser => "add_to_user",
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

/// Aggregate id of the singleton that remembers who registered first.
pub const REGISTRY_ID: &str = "registry";

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
    fn hex_is_lowercase_two_digits_per_byte() {
        assert_eq!(hex(&[0, 15, 255]), "000fff");
    }
}
