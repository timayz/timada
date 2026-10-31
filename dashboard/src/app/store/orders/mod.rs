//! `/store/orders/{order_id}`: one order of the key's tenant, and its
//! invoice and credit notes (`…/documents`), for the storefront that holds
//! its id (the browser that placed it, typically, or a review link's). The
//! store API lets the key read no order stream and no invoice series
//! ([`crate::read_only_store`]); the dashboard answers for one id here.

pub mod order_id;
