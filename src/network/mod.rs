mod transport;

pub use transport::SimulatedNetwork;

use serde::{Deserialize, Serialize};

use crate::types::{Digest, NodeId, Payload, SequenceNumber, ViewNumber};

/// Envelope for all protocol messages sent between nodes.
///
/// Using a single enum keeps the network layer protocol-agnostic —
/// it just routes `NetworkMessage` values without knowing whether
/// the cluster is running PBFT or Raft.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkMessage {
    pub from: NodeId,
    pub to: NodeId,
    pub payload: ProtocolMessage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProtocolMessage {
    Pbft(PbftMessage),
    Raft(RaftMessage),
}

// ── PBFT messages ────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PbftMessage {
    PrePrepare {
        view: ViewNumber,
        sequence: SequenceNumber,
        digest: Digest,
        payload: Payload,
    },
    Prepare {
        view: ViewNumber,
        sequence: SequenceNumber,
        digest: Digest,
        replica: NodeId,
    },
    Commit {
        view: ViewNumber,
        sequence: SequenceNumber,
        digest: Digest,
        replica: NodeId,
    },
    ViewChange {
        new_view: ViewNumber,
        replica: NodeId,
    },
    NewView {
        view: ViewNumber,
    },
}

// ── Raft messages ────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RaftMessage {
    RequestVote {
        term: u64,
        candidate_id: NodeId,
        last_log_index: u64,
        last_log_term: u64,
    },
    RequestVoteResponse {
        term: u64,
        vote_granted: bool,
    },
    AppendEntries {
        term: u64,
        leader_id: NodeId,
        prev_log_index: u64,
        prev_log_term: u64,
        entries: Vec<LogEntry>,
        leader_commit: u64,
    },
    AppendEntriesResponse {
        term: u64,
        success: bool,
        match_index: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub term: u64,
    pub index: u64,
    pub payload: Payload,
}
