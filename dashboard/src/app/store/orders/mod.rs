//! `/store/orders/{order_id}`: one order of the key's tenant, for the
//! storefront that holds its id (the browser that placed it, typically, or a
//! review link's). The store API lets the key read no order stream
//! ([`crate::read_only_store`]); the dashboard answers for one id here.

pub mod order_id;
