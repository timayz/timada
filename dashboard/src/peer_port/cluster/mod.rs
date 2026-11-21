//! `/cluster`: what the nodes of a cluster ask of each other, on the port
//! they keep for it ([`super::serve`]); the public port serves none of it.
//! That port takes a connection from a node of the cluster only, so the
//! routes check nothing more.

pub mod keys;
pub mod limits;
pub mod media;
