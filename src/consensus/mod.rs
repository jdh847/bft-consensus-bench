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
/// The benchmark harness programs against this trait, so PBFT and Raft
/// can be compared on identical workloads without protocol-specific code
/// leaking into the measurement layer.
#[async_trait]
pub trait ConsensusNode: Send + Sync {
    /// Unique identifier for this node.
    fn id(&self) -> NodeId;

    /// Total number of nodes in the cluster.
    fn cluster_size(&self) -> usize;

    /// Maximum number of faulty nodes this configuration tolerates.
    fn fault_tolerance(&self) -> usize;

    /// Propose a client payload for consensus. Returns immediately —
    /// the caller should await commitment via `poll_committed`.
    async fn propose(&self, payload: Payload) -> ProposeResult;

    /// Handle an incoming protocol message from another node.
    async fn handle_message(&self, msg: NetworkMessage);

    /// Drain committed entries since the last call. Returns entries
    /// in sequence order. Non-blocking — returns empty vec if nothing
    /// new has been committed.
    async fn poll_committed(&self) -> Vec<CommittedEntry>;

    /// Current leader, if known.
    fn leader(&self) -> Option<NodeId>;

    /// Whether this node believes it is the current leader.
    fn is_leader(&self) -> bool {
        self.leader() == Some(self.id())
    }
}
