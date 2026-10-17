//! What every bounded context shares: the refusal code of a write the
//! store is not taking right now.

use evento::WriteError;

/// The refusal code pages and the store API carry when a tenant is not
/// taking writes for the moment: a node of the cluster is away and the
/// tenant keeps its writes for it up to the backlog, or this node is being
/// copied back from a peer. Nothing about the request is wrong; it goes
/// through again once the node is back or given up.
pub const WRITES_PAUSED: &str = "writes_paused";

/// `Some(WRITES_PAUSED)` when `err` is a write the store refused for the
/// moment ([`WriteError::Unavailable`]), `None` for any other write failure.
pub fn write_refusal(err: &WriteError) -> Option<&'static str> {
    match err {
        WriteError::Unavailable(_) => Some(WRITES_PAUSED),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_paused_store_is_a_refusal() {
        let paused = WriteError::Unavailable(anyhow::anyhow!("backlog at cap"));
        assert_eq!(write_refusal(&paused), Some(WRITES_PAUSED));
        let broken = WriteError::Unknown(anyhow::anyhow!("disk gone"));
        assert_eq!(write_refusal(&broken), None);
        assert_eq!(write_refusal(&WriteError::InvalidOriginalVersion), None);
    }
}
