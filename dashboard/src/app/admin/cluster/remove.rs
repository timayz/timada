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
pub struct RemoveForm {
    node: String,
}

/// Removes a node from the cluster: it counts no more, whether it runs or
/// is gone for good, and is to be stopped. Asked of another node than the
/// one removed: the page that answers must go on serving.
#[route(POST)]
pub async fn remove(cx: &Cx, Form(form): Form<RemoveForm>) -> Result<SeeOther> {
    require_admin(cx).await?;
    let Ok(node) = form.node.trim().parse::<u64>() else {
        return Ok(see_other(cluster_url(cx, Some("invalid_node"), None)));
    };
    let cluster = context::<Arc<Cluster>>(cx);
    if cluster.id() == NodeId(node) {
        return Ok(see_other(cluster_url(cx, Some("remove_this_node"), None)));
    }
    match cluster.remove_node(NodeId(node)).await {
        Ok(_) => Ok(see_other(cluster_url(
            cx,
            None,
            Some(&format!(
                "Node {node} is no longer a member. Stop it: it takes no part any more."
            )),
        ))),
        Err(err) => {
            tracing::warn!(node, error = %format!("{err:#}"), "the node was not removed from the cluster");
            Ok(see_other(cluster_url(
                cx,
                Some("members_not_changed"),
                None,
            )))
        }
    }
}
