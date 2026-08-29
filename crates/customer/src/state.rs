use timada_auth::AuthState;
use timada_core::ServiceContext;

/// Everything the customer routes need. Auth is here because login and
/// registration mint sessions through `timada-auth`'s machinery.
#[derive(Clone)]
pub struct CustomerState {
    pub ctx: ServiceContext,
    pub auth: AuthState,
}
