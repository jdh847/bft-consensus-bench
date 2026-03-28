use std::collections::HashSet;

use rand::Rng;

use crate::network::NetworkMessage;
use crate::types::NodeId;

/// Fault injection configuration for the simulated cluster.
#[derive(Debug, Clone)]
pub struct FaultConfig {
    /// Probability (0.0–1.0) that a message is dropped.
    pub drop_rate: f64,
    /// Set of (from, to) pairs representing network partitions.
    /// Messages between partitioned nodes are silently dropped.
    pub partitions: HashSet<(NodeId, NodeId)>,
    /// Nodes that are completely isolated (all messages to/from dropped).
    pub isolated_nodes: HashSet<NodeId>,
}

impl Default for FaultConfig {
    fn default() -> Self {
        Self {
            drop_rate: 0.0,
            partitions: HashSet::new(),
            isolated_nodes: HashSet::new(),
        }
    }
}

impl FaultConfig {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set random message drop rate.
    pub fn with_drop_rate(mut self, rate: f64) -> Self {
        self.drop_rate = rate.clamp(0.0, 1.0);
        self
    }

    /// Partition two nodes from each other (bidirectional).
    pub fn with_partition(mut self, a: NodeId, b: NodeId) -> Self {
        self.partitions.insert((a, b));
        self.partitions.insert((b, a));
        self
    }

    /// Completely isolate a node from the network.
    pub fn with_isolated(mut self, node: NodeId) -> Self {
        self.isolated_nodes.insert(node);
        self
    }

    /// Returns true if a node is completely isolated from the network.
    pub fn is_isolated(&self, node: NodeId) -> bool {
        self.isolated_nodes.contains(&node)
    }

    /// Returns true if this message should be delivered.
    pub fn should_deliver(&self, msg: &NetworkMessage) -> bool {
        // Isolated nodes can't send or receive
        if self.isolated_nodes.contains(&msg.from) || self.isolated_nodes.contains(&msg.to) {
            return false;
        }

        // Check partition
        if self.partitions.contains(&(msg.from, msg.to)) {
            return false;
        }

        // Random drop
        if self.drop_rate > 0.0 {
            let roll: f64 = rand::thread_rng().gen();
            if roll < self.drop_rate {
                return false;
            }
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::ProtocolMessage;
    use crate::network::PbftMessage;
    use crate::types::Payload;

    fn dummy_msg(from: u64, to: u64) -> NetworkMessage {
        NetworkMessage {
            from: NodeId(from),
            to: NodeId(to),
            payload: ProtocolMessage::Pbft(PbftMessage::Commit {
                view: 0,
                sequence: 1,
                digest: Payload::new(b"x").digest(),
                replica: NodeId(from),
            }),
        }
    }

    #[test]
    fn no_faults_delivers_everything() {
        let config = FaultConfig::new();
        let msg = dummy_msg(0, 1);
        assert!(config.should_deliver(&msg));
    }

    #[test]
    fn partition_blocks_both_directions() {
        let config = FaultConfig::new()
            .with_partition(NodeId(0), NodeId(1));

        assert!(!config.should_deliver(&dummy_msg(0, 1)));
        assert!(!config.should_deliver(&dummy_msg(1, 0)));
        // Other links still work
        assert!(config.should_deliver(&dummy_msg(0, 2)));
    }

    #[test]
    fn isolated_node_is_unreachable() {
        let config = FaultConfig::new()
            .with_isolated(NodeId(2));

        assert!(!config.should_deliver(&dummy_msg(0, 2)));
        assert!(!config.should_deliver(&dummy_msg(2, 0)));
        assert!(config.should_deliver(&dummy_msg(0, 1)));
    }

    #[test]
    fn full_drop_rate_drops_everything() {
        let config = FaultConfig::new().with_drop_rate(1.0);
        let msg = dummy_msg(0, 1);

        // With 100% drop rate, nothing should get through
        for _ in 0..100 {
            assert!(!config.should_deliver(&msg));
        }
    }
}
