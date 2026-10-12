//! Events. Every variant and every field is frozen once it ships (bitcode is
//! positional); evolve by adding variants.

use crate::value_object::{ChallengePurpose, SignInMethod};

/// A person with an account. Holds the profile, the list of sign-in methods
/// (for the account page) and the sign-in history.
#[evento::aggregate(name = "timada-identity/User")]
pub enum User {
    Registered {
        account: String,
        label: String,
        method: SignInMethod,
    },
    ProfileUpdated {
        account: String,
        label: String,
    },
    AdminGranted,
    OAuth2AccountLinked {
        provider: String,
        provider_user_id: String,
        name: Option<String>,
        email: Option<String>,
        picture: Option<String>,
    },
    OAuth2AccountUnlinked {
        provider: String,
        provider_user_id: String,
    },
    PasskeyAdded {
        credential_id: String,
        name: String,
        aaguid: String,
    },
    PasskeyRenamed {
        credential_id: String,
        name: String,
    },
    PasskeyRemoved {
        credential_id: String,
    },
    SignedIn {
        method: SignInMethod,
        provider: Option<String>,
        credential_id: Option<String>,
        user_agent: Option<String>,
        ip: Option<String>,
    },
    SignedOut,
    Deleted,
    /// Every session started up to this event's time is void, except the one
    /// named (the browser that asked, when a user signs out everywhere else).
    SessionsRevoked {
        except_session: Option<String>,
    },
    AdminRevoked,
    /// A user created with an e-mail address and a password, after the
    /// address was proven with a mailed code. `password_hash` is a PHC string.
    EmailRegistered {
        email: String,
        label: String,
        password_hash: String,
    },
    PasswordChanged {
        password_hash: String,
    },
    SignedInWithPassword {
        user_agent: Option<String>,
        ip: Option<String>,
    },
}

/// Index entry: one provider identity (`provider`, `sub`) → one user. The
/// aggregate id is [`crate::oauth2_account_id`].
#[evento::aggregate(name = "timada-identity/OAuth2Account")]
pub enum OAuth2Account {
    Linked {
        user_id: String,
        provider: String,
        provider_user_id: String,
    },
    Unlinked,
}

/// Index entry and verification material for one passkey. The aggregate id is
/// [`crate::passkey_credential_id`]; `passkey_json` is the web layer's
/// serialized credential (public key, counter, backup flags).
#[evento::aggregate(name = "timada-identity/PasskeyCredential")]
pub enum PasskeyCredential {
    Created {
        user_id: String,
        credential_id: String,
        passkey_json: String,
        aaguid: String,
        rp_id: String,
        name: String,
    },
    Updated {
        passkey_json: String,
    },
    Removed,
}

/// A signed-in browser. The aggregate id is [`crate::session_id`]; the raw
/// token never reaches the domain. Times are unix seconds.
#[evento::aggregate(name = "timada-identity/Session")]
pub enum Session {
    Started {
        user_id: String,
        expires_at: u64,
        user_agent: Option<String>,
        ip: Option<String>,
    },
    Refreshed {
        expires_at: u64,
    },
    Ended,
}

/// Singleton (id [`crate::REGISTRY_ID`]) whose first event names the first
/// registered user, who becomes the administrator.
#[evento::aggregate(name = "timada-identity/Registry")]
pub enum Registry {
    FirstUserClaimed { user_id: String },
}

/// Index entry: one e-mail address → one user, for the password sign-in.
/// The aggregate id is [`crate::email_account_id`] of the normalized address.
#[evento::aggregate(name = "timada-identity/EmailAccount")]
pub enum EmailAccount {
    Claimed { user_id: String, email: String },
    Released,
}

/// The one-time codes mailed to an address, one stream per normalized
/// address (id [`crate::email_challenge_id`]). A new `CodeIssued` replaces
/// the previous code; attempts are counted here so that no client-held state
/// can reset them. Times are unix seconds.
#[evento::aggregate(name = "timada-identity/EmailChallenge")]
pub enum EmailChallenge {
    CodeIssued {
        purpose: ChallengePurpose,
        code_hash: String,
        issued_at: u64,
        expires_at: u64,
    },
    AttemptFailed,
    Consumed,
}
