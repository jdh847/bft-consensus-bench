pub mod pbft;
pub mod raft;

use async_trait::async_trait;

use crate::network::NetworkMessage;
use crate::types::{NodeId, Payload, SequenceNumber};

/// Result of attempting to propose a value to the consensus protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProposeResult {
    /// This node is the leader and the proposal has entered the protocol.
    Accepted,
    /// This node is not the current leader. Redirect to the given node.
    NotLeader(Option<NodeId>),
}

/// A value that was committed by the consensus protocol.
#[derive(Debug, Clone)]
pub struct CommittedEntry {
    pub sequence: SequenceNumber,
    pub payload: Payload,
}

/// The core trait every consensus implementation must satisfy.
///
/// Methods return `Vec<NetworkMessage>` — the outgoing messages the
/// node wants to send. The cluster orchestrator is responsible for
/// actually delivering them via the network.
#[async_trait]
pub trait ConsensusNode: Send + Sync {
    /// Unique identifier for this node.
    fn id(&self) -> NodeId;

    /// Total number of nodes in the cluster.
    fn cluster_size(&self) -> usize;

    /// Maximum number of faulty nodes this configuration tolerates.
    fn fault_tolerance(&self) -> usize;

    /// All node IDs in the cluster (needed for broadcasting).
    fn peers(&self) -> Vec<NodeId>;

    /// Propose a client payload for consensus. Returns the result
    /// and any outgoing protocol messages to broadcast.
    async fn propose(&self, payload: Payload) -> (ProposeResult, Vec<NetworkMessage>);

    /// Handle an incoming protocol message. Returns outgoing messages
    /// to send in response.
    async fn handle_message(&self, msg: NetworkMessage) -> Vec<NetworkMessage>;

    /// Periodic tick for time-driven events (election timeouts,
    /// heartbeats). Returns outgoing messages if any.
    async fn tick(&self) -> Vec<NetworkMessage>;

    /// Drain committed entries since the last call.
    async fn poll_committed(&self) -> Vec<CommittedEntry>;

    /// Current leader, if known.
    fn leader(&self) -> Option<NodeId>;

    /// Whether this node believes it is the current leader.
    fn is_leader(&self) -> bool {
        self.leader() == Some(self.id())
    }
}
