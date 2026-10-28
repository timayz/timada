//! Write side: the command gateway and the strict write models it loads.
//!
//! A write model folds *all* events of its stream (`strict()`), so the
//! version it observes, and `write()` sends as `original_version`, never
//! goes stale. None is snapshotted: a scenario is saved a handful of times.

mod run;
mod scenario;

use std::ops::Deref;

pub use run::*;
pub use scenario::*;

use evento::Executor;

/// The command gateway. Cheap to clone when `E` is.
pub struct Command<E: Executor> {
    executor: E,
    tenant: String,
}

impl<E: Executor> Command<E> {
    /// The gateway of one tenant: every stream it writes, loads or reads is
    /// that tenant's. `tenant` must not be empty (the default tenant holds
    /// the platform's own streams).
    pub fn new(executor: E, tenant: impl Into<String>) -> Self {
        let tenant = tenant.into();
        debug_assert!(!tenant.is_empty(), "a tenant gateway needs a tenant");
        Self { executor, tenant }
    }

    /// The tenant every call acts in.
    pub fn tenant(&self) -> &str {
        &self.tenant
    }

    /// The store the tenant's streams live in.
    pub fn executor(&self) -> &E {
        &self.executor
    }
}

impl<E: Executor> Deref for Command<E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        &self.executor
    }
}
