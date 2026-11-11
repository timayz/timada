//! Which of the nodes up at the moment this one is, for the work every node
//! could do but one should: each linked item and each order placed on
//! AliExpress is read by one node, so the app's quota is spent once; each
//! subscriber of a campaign going out is mailed from one node, so the nodes
//! send side by side rather than race for the same mails. Two nodes that
//! disagree for a moment on who is up do one piece twice or leave it for
//! later; the work itself makes sure neither harms it (a claim, a fence).

/// This node's place among the nodes up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Share {
    /// This node's place among them, from 0.
    pub index: u64,
    /// How many there are, at least 1.
    pub of: u64,
}

impl Share {
    /// A node on its own: everything is its.
    pub const ALONE: Self = Self { index: 0, of: 1 };

    /// Whether the work `id` names belongs to this node.
    pub fn owns(&self, id: &str) -> bool {
        u64::from(shard_of(id)) % self.of == self.index
    }
}

/// Where the work `id` names falls among the nodes (FNV-1a of the id): the
/// same on every node and in every build, which the standard hasher does not
/// promise. A read model that hands work out keeps it in a column, so a query
/// takes a node's part with `shard % of = index`.
pub fn shard_of(id: &str) -> u32 {
    id.bytes().fold(0x811c_9dc5, |hash: u32, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_item_belongs_to_one_node() {
        let ids: Vec<String> = (0..300).map(|n| format!("link-{n}")).collect();
        for of in 1..=5 {
            let mut held = 0;
            for index in 0..of {
                let share = Share { index, of };
                let own = ids.iter().filter(|id| share.owns(id)).count();
                // Each node holds its part, near enough.
                assert!(own >= 300 / of as usize / 2, "{own} of 300 on {index}/{of}");
                held += own;
            }
            assert_eq!(held, ids.len());
        }
    }

    #[test]
    fn a_node_alone_owns_everything() {
        assert!(Share::ALONE.owns("anything"));
        assert!(Share::ALONE.owns(""));
    }

    #[test]
    fn where_an_item_falls_does_not_depend_on_the_build() {
        assert_eq!(shard_of(""), 0x811c_9dc5);
        assert_eq!(shard_of("a"), 0xe40c_292c);
    }
}
