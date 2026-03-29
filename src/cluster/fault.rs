use std::collections::{HashMap, HashSet};

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use crate::network::{HotStuffMessage, NetworkMessage, PbftMessage, ProtocolMessage, RaftMessage};
use crate::types::{Digest, NodeId, Payload};

/// Types of Byzantine misbehavior a node can exhibit.
#[derive(Debug, Clone)]
pub enum ByzantineBehavior {
    /// Flip bits in the digest field, corrupting message integrity.
    CorruptDigest,
    /// Send different payloads to different targets (odd vs even node IDs).
    Equivocate,
    /// Drop specific message phases while forwarding others.
    SelectiveDrop { phases: Vec<MessagePhase> },
}

/// Message phases that can be selectively targeted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessagePhase {
    PbftPrePrepare,
    PbftPrepare,
    PbftCommit,
    RaftAppendEntries,
    RaftRequestVote,
    HotStuffPropose,
    HotStuffVote,
    HotStuffQC,
}

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
    /// Byzantine nodes and their misbehaviors.
    pub byzantine_nodes: HashMap<NodeId, Vec<ByzantineBehavior>>,
    /// Deterministic RNG for reproducible fault injection.
    rng: StdRng,
}

impl Default for FaultConfig {
    fn default() -> Self {
        Self {
            drop_rate: 0.0,
            partitions: HashSet::new(),
            isolated_nodes: HashSet::new(),
            byzantine_nodes: HashMap::new(),
            rng: StdRng::from_entropy(),
        }
    }
}

impl FaultConfig {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a FaultConfig with a deterministic seed for reproducibility.
    pub fn with_seed(seed: u64) -> Self {
        Self {
            rng: StdRng::seed_from_u64(seed),
            ..Self::default()
        }
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

    /// Register a node as Byzantine with specified misbehaviors.
    pub fn with_byzantine(mut self, node: NodeId, behaviors: Vec<ByzantineBehavior>) -> Self {
        self.byzantine_nodes.insert(node, behaviors);
        self
    }

    /// Returns true if a node is configured as Byzantine.
    pub fn is_byzantine(&self, node: NodeId) -> bool {
        self.byzantine_nodes.contains_key(&node)
    }

    /// Returns true if this message should be delivered.
    /// Uses stored RNG for deterministic behavior.
    pub fn should_deliver(&mut self, msg: &NetworkMessage) -> bool {
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
            let roll: f64 = self.rng.gen();
            if roll < self.drop_rate {
                return false;
            }
        }

        true
    }

    /// Apply Byzantine tampering to a message from a Byzantine node.
    /// Returns 0+ messages: corruption modifies in place, equivocation forks
    /// by target parity, selective drop filters by phase.
    pub fn tamper(&mut self, msg: NetworkMessage) -> Vec<NetworkMessage> {
        let behaviors = match self.byzantine_nodes.get(&msg.from) {
            Some(b) => b.clone(),
            None => return vec![msg],
        };

        let mut messages = vec![msg];

        for behavior in &behaviors {
            let mut next = Vec::new();
            for m in messages {
                match behavior {
                    ByzantineBehavior::CorruptDigest => {
                        next.push(Self::corrupt_digest(m));
                    }
                    ByzantineBehavior::Equivocate => {
                        next.extend(self.equivocate(m));
                    }
                    ByzantineBehavior::SelectiveDrop { phases } => {
                        if !Self::matches_phase(&m, phases) {
                            next.push(m);
                        }
                        // else: dropped
                    }
                }
            }
            messages = next;
        }

        messages
    }

    /// Corrupt the digest field of a message by flipping bits.
    fn corrupt_digest(mut msg: NetworkMessage) -> NetworkMessage {
        if let ProtocolMessage::Pbft(
            PbftMessage::PrePrepare { ref mut digest, .. }
            | PbftMessage::Prepare { ref mut digest, .. }
            | PbftMessage::Commit { ref mut digest, .. },
        ) = &mut msg.payload
        {
            let mut bytes = digest.0;
            bytes[0] ^= 0xFF;
            *digest = Digest(bytes);
        } else if let ProtocolMessage::HotStuff(
            HotStuffMessage::Propose { ref mut digest, .. }
            | HotStuffMessage::Vote { ref mut digest, .. }
            | HotStuffMessage::QuorumCertificate { ref mut digest, .. },
        ) = &mut msg.payload
        {
            let mut bytes = digest.0;
            bytes[0] ^= 0xFF;
            *digest = Digest(bytes);
        }
        msg
    }

    /// Equivocate: odd-numbered targets get a message with a different payload.
    fn equivocate(&mut self, msg: NetworkMessage) -> Vec<NetworkMessage> {
        if msg.to.0.is_multiple_of(2) {
            // Even targets get the original
            return vec![msg];
        }

        // Odd targets get a tampered version
        let mut tampered = msg;
        if let ProtocolMessage::Pbft(PbftMessage::PrePrepare {
            ref mut payload,
            ref mut digest,
            ..
        }) = &mut tampered.payload
        {
            let mut alt_data = payload.0.clone();
            alt_data.push(0xFF); // append byte to create different payload
            let alt_payload = Payload(alt_data);
            *digest = alt_payload.digest();
            *payload = alt_payload;
        } else if let ProtocolMessage::Raft(RaftMessage::AppendEntries {
            ref mut entries, ..
        }) = &mut tampered.payload
        {
            for entry in entries.iter_mut() {
                let mut alt_data = entry.payload.0.clone();
                alt_data.push(0xFF);
                entry.payload = Payload(alt_data);
            }
        } else if let ProtocolMessage::HotStuff(HotStuffMessage::Propose {
            ref mut payload,
            ref mut digest,
            ..
        }) = &mut tampered.payload
        {
            let mut alt_data = payload.0.clone();
            alt_data.push(0xFF);
            let alt_payload = Payload(alt_data);
            *digest = alt_payload.digest();
            *payload = alt_payload;
        }

        vec![tampered]
    }

    /// Check if a message matches any of the specified phases.
    fn matches_phase(msg: &NetworkMessage, phases: &[MessagePhase]) -> bool {
        let msg_phase = match &msg.payload {
            ProtocolMessage::Pbft(pbft) => match pbft {
                PbftMessage::PrePrepare { .. } => MessagePhase::PbftPrePrepare,
                PbftMessage::Prepare { .. } => MessagePhase::PbftPrepare,
                PbftMessage::Commit { .. } => MessagePhase::PbftCommit,
                _ => return false,
            },
            ProtocolMessage::Raft(raft) => match raft {
                RaftMessage::AppendEntries { .. } => MessagePhase::RaftAppendEntries,
                RaftMessage::RequestVote { .. } => MessagePhase::RaftRequestVote,
                _ => return false,
            },
            ProtocolMessage::HotStuff(hs) => match hs {
                HotStuffMessage::Propose { .. } => MessagePhase::HotStuffPropose,
                HotStuffMessage::Vote { .. } => MessagePhase::HotStuffVote,
                HotStuffMessage::QuorumCertificate { .. } => MessagePhase::HotStuffQC,
                HotStuffMessage::NewView { .. } => return false,
            },
        };
        phases.contains(&msg_phase)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::PbftMessage;
    use crate::network::ProtocolMessage;
    use crate::types::Payload;

    fn pre_prepare_msg(from: u64, to: u64) -> NetworkMessage {
        let payload = Payload::new(b"data");
        NetworkMessage {
            from: NodeId(from),
            to: NodeId(to),
            payload: ProtocolMessage::Pbft(PbftMessage::PrePrepare {
                view: 0,
                sequence: 1,
                digest: payload.digest(),
                payload,
            }),
        }
    }

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
        let mut config = FaultConfig::new();
        let msg = dummy_msg(0, 1);
        assert!(config.should_deliver(&msg));
    }

    #[test]
    fn partition_blocks_both_directions() {
        let mut config = FaultConfig::new().with_partition(NodeId(0), NodeId(1));

        assert!(!config.should_deliver(&dummy_msg(0, 1)));
        assert!(!config.should_deliver(&dummy_msg(1, 0)));
        // Other links still work
        assert!(config.should_deliver(&dummy_msg(0, 2)));
    }

    #[test]
    fn isolated_node_is_unreachable() {
        let mut config = FaultConfig::new().with_isolated(NodeId(2));

        assert!(!config.should_deliver(&dummy_msg(0, 2)));
        assert!(!config.should_deliver(&dummy_msg(2, 0)));
        assert!(config.should_deliver(&dummy_msg(0, 1)));
    }

    #[test]
    fn full_drop_rate_drops_everything() {
        let mut config = FaultConfig::new().with_drop_rate(1.0);
        let msg = dummy_msg(0, 1);

        // With 100% drop rate, nothing should get through
        for _ in 0..100 {
            assert!(!config.should_deliver(&msg));
        }
    }

    #[test]
    fn corrupt_digest_flips_first_byte() {
        let msg = pre_prepare_msg(0, 1);
        let original_digest = match &msg.payload {
            ProtocolMessage::Pbft(PbftMessage::PrePrepare { digest, .. }) => digest.0,
            _ => panic!("expected PrePrepare"),
        };

        let corrupted = FaultConfig::corrupt_digest(msg);
        let new_digest = match &corrupted.payload {
            ProtocolMessage::Pbft(PbftMessage::PrePrepare { digest, .. }) => digest.0,
            _ => panic!("expected PrePrepare"),
        };

        assert_eq!(new_digest[0], original_digest[0] ^ 0xFF);
        assert_eq!(new_digest[1..], original_digest[1..]);
    }

    #[test]
    fn equivocate_splits_by_target_parity() {
        let mut config = FaultConfig::with_seed(42)
            .with_byzantine(NodeId(0), vec![ByzantineBehavior::Equivocate]);

        // Even target (NodeId(2)) gets original payload
        let msg_even = pre_prepare_msg(0, 2);
        let original_payload = match &msg_even.payload {
            ProtocolMessage::Pbft(PbftMessage::PrePrepare { payload, .. }) => payload.clone(),
            _ => panic!("expected PrePrepare"),
        };
        let result_even = config.tamper(msg_even);
        assert_eq!(result_even.len(), 1);
        match &result_even[0].payload {
            ProtocolMessage::Pbft(PbftMessage::PrePrepare { payload, .. }) => {
                assert_eq!(*payload, original_payload);
            }
            _ => panic!("expected PrePrepare"),
        }

        // Odd target (NodeId(1)) gets mutated payload
        let msg_odd = pre_prepare_msg(0, 1);
        let result_odd = config.tamper(msg_odd);
        assert_eq!(result_odd.len(), 1);
        match &result_odd[0].payload {
            ProtocolMessage::Pbft(PbftMessage::PrePrepare { payload, .. }) => {
                assert_ne!(*payload, original_payload);
            }
            _ => panic!("expected PrePrepare"),
        }
    }

    #[test]
    fn selective_drop_filters_matching_phases() {
        let mut config = FaultConfig::with_seed(42).with_byzantine(
            NodeId(0),
            vec![ByzantineBehavior::SelectiveDrop {
                phases: vec![MessagePhase::PbftCommit],
            }],
        );

        // Commit phase message should be dropped
        let commit_msg = dummy_msg(0, 1); // dummy_msg creates a Commit
        let result = config.tamper(commit_msg);
        assert!(result.is_empty());

        // PrePrepare phase message should pass through
        let pp_msg = pre_prepare_msg(0, 1);
        let result = config.tamper(pp_msg);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn non_byzantine_node_passes_through_untampered() {
        let mut config = FaultConfig::with_seed(42)
            .with_byzantine(NodeId(9), vec![ByzantineBehavior::CorruptDigest]);

        // Node 0 is NOT byzantine — message should pass through unchanged
        let msg = pre_prepare_msg(0, 1);
        let original_digest = match &msg.payload {
            ProtocolMessage::Pbft(PbftMessage::PrePrepare { digest, .. }) => digest.0,
            _ => panic!("expected PrePrepare"),
        };

        let result = config.tamper(msg);
        assert_eq!(result.len(), 1);
        match &result[0].payload {
            ProtocolMessage::Pbft(PbftMessage::PrePrepare { digest, .. }) => {
                assert_eq!(digest.0, original_digest);
            }
            _ => panic!("expected PrePrepare"),
        }
    }
}
