//! The icons the pages draw, from the Lucide set `build.rs` stages (ISC
//! licence). One constant per use keeps the names in one place; an unknown
//! name fails the build with the Lucide name in the error.

use topcoat::icon::{IconData, iconify::iconify_icon};

// Navigation.
pub const DASHBOARD: IconData = iconify_icon!("lucide:layout-dashboard");
pub const COMPANIES: IconData = iconify_icon!("lucide:building-2");
pub const ACCOUNT: IconData = iconify_icon!("lucide:user-round");
pub const TENANTS: IconData = iconify_icon!("lucide:store");
pub const USERS: IconData = iconify_icon!("lucide:users");
pub const PRODUCTS: IconData = iconify_icon!("lucide:package");
pub const CATEGORIES: IconData = iconify_icon!("lucide:folder-tree");
pub const ORDERS: IconData = iconify_icon!("lucide:shopping-bag");
pub const CARTS: IconData = iconify_icon!("lucide:shopping-cart");
pub const CUSTOMERS: IconData = iconify_icon!("lucide:contact");
pub const INVOICES: IconData = iconify_icon!("lucide:file-text");
pub const DISCOUNTS: IconData = iconify_icon!("lucide:ticket-percent");
pub const OFFERS: IconData = iconify_icon!("lucide:tag");
pub const REVIEWS: IconData = iconify_icon!("lucide:star");
pub const MESSAGES: IconData = iconify_icon!("lucide:message-square");
pub const PAYMENTS: IconData = iconify_icon!("lucide:credit-card");
pub const ALIEXPRESS: IconData = iconify_icon!("lucide:package-plus");
pub const SHIPPING: IconData = iconify_icon!("lucide:truck");
pub const MAIL: IconData = iconify_icon!("lucide:mail");
pub const INVOICING: IconData = iconify_icon!("lucide:receipt");
pub const MEMBERS: IconData = iconify_icon!("lucide:users-round");
pub const ROLES: IconData = iconify_icon!("lucide:shield");
pub const API_KEYS: IconData = iconify_icon!("lucide:key-round");
pub const INVITATIONS: IconData = iconify_icon!("lucide:mail-open");

// Chrome.
pub const SUN: IconData = iconify_icon!("lucide:sun");
pub const MOON: IconData = iconify_icon!("lucide:moon");
pub const MONITOR: IconData = iconify_icon!("lucide:monitor");
pub const LOG_OUT: IconData = iconify_icon!("lucide:log-out");
pub const LOG_IN: IconData = iconify_icon!("lucide:log-in");
pub const CLOSE: IconData = iconify_icon!("lucide:x");
pub const PLUS: IconData = iconify_icon!("lucide:plus");

// Notices and states.
pub const CHECK: IconData = iconify_icon!("lucide:circle-check");
pub const WARNING: IconData = iconify_icon!("lucide:triangle-alert");
pub const INFO: IconData = iconify_icon!("lucide:info");
pub const ERROR: IconData = iconify_icon!("lucide:circle-x");
pub const CLOCK: IconData = iconify_icon!("lucide:clock");
