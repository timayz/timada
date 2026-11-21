//! `/cluster`: what the nodes of a cluster ask of each other, on the port
//! they keep for it ([`super::serve`]); the public port serves none of it.
//! Every route checks the cluster's shared secret.

pub mod keys;
pub mod limits;
pub mod media;
