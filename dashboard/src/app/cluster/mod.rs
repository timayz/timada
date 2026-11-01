//! `/cluster`: what the nodes of a cluster ask of each other over the web
//! port. Nothing here is for a browser; every route checks the cluster's
//! shared secret.

pub mod keys;
pub mod limits;
pub mod media;
