use std::sync::Arc;

use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, context},
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::cluster_url;
use crate::app::admin::require_admin;
use crate::cluster::{Cluster, NodeId};

#[derive(Deserialize)]
pub struct AddForm {
    /// The node's number: its place among the peers of its configuration.
    node: String,
    /// Where the members dial it, `host:port` or `ip:port`.
    address: String,
}

/// Adds a node to the cluster: from here on it counts. The node itself is a
/// process started over an empty store, which waits for this, copies the
/// store from a member, then serves.
#[route(POST)]
pub async fn add(cx: &Cx, Form(form): Form<AddForm>) -> Result<SeeOther> {
    require_admin(cx).await?;
    let Ok(node) = form.node.trim().parse::<u64>() else {
        return Ok(see_other(cluster_url(cx, Some("invalid_node"), None)));
    };
    let Ok(address) = form.address.trim().parse::<evento::cluster::PeerAddr>() else {
        return Ok(see_other(cluster_url(
            cx,
            Some("invalid_node_address"),
            None,
        )));
    };
    match context::<Arc<Cluster>>(cx)
        .add_node(NodeId(node), address)
        .await
    {
        Ok(_) => Ok(see_other(cluster_url(
            cx,
            None,
            Some(&format!(
                "Node {node} is a member: it copies the store, then serves."
            )),
        ))),
        Err(err) => {
            tracing::warn!(node, error = %format!("{err:#}"), "the node was not added to the cluster");
            Ok(see_other(cluster_url(
                cx,
                Some("members_not_changed"),
                None,
            )))
        }
    }
}
