mod state;

pub use state::PbftNode;

use std::collections::HashSet;

use crate::types::{NodeId, SequenceNumber, ViewNumber};

/// PBFT phase for a given sequence number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Waiting for pre-prepare from the primary.
    Idle,
    /// Pre-prepare received, collecting prepare messages.
    PrePrepared,
    /// 2f matching prepares collected, broadcasting commit.
    Prepared,
    /// 2f+1 commits collected, entry is committed.
    Committed,
}

/// Tracks the state of a single consensus slot.
#[derive(Debug, Clone)]
pub struct SlotState {
    pub view: ViewNumber,
    pub sequence: SequenceNumber,
    pub phase: Phase,
    pub prepare_voters: HashSet<NodeId>,
    pub commit_voters: HashSet<NodeId>,
}

impl SlotState {
    pub fn new(view: ViewNumber, sequence: SequenceNumber) -> Self {
        Self {
            view,
            sequence,
            phase: Phase::Idle,
            prepare_voters: HashSet::new(),
            commit_voters: HashSet::new(),
        }
    }
}
