//! Read side. The write models already carry everything the account page
//! needs, so they double as views for now; SQL read models built by
//! subscriptions (user lists for administrators) will live here.

pub use crate::command::UserState as AccountView;
