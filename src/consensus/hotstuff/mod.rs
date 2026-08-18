mod state;

pub use state::HotStuffNode;

use std::collections::HashSet;

use crate::types::{NodeId, SequenceNumber, ViewNumber};

/// HotStuff consensus phase for a given sequence number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Waiting for a proposal from the leader.
    Idle,
    /// Proposal received, collecting Prepare votes.
    Prepare,
    /// PrepareQC formed, collecting PreCommit votes.
    PreCommit,
    /// PreCommitQC formed, collecting Commit votes.
    Commit,
    /// CommitQC formed, entry is committed.
    Decided,
}

/// Tracks the state of a single consensus slot.
#[derive(Debug, Clone)]
pub struct SlotState {
    pub view: ViewNumber,
    pub sequence: SequenceNumber,
    pub phase: Phase,
    pub prepare_voters: HashSet<NodeId>,
    pub precommit_voters: HashSet<NodeId>,
    pub commit_voters: HashSet<NodeId>,
}

impl SlotState {
    pub fn new(view: ViewNumber, sequence: SequenceNumber) -> Self {
        Self {
            view,
            sequence,
            phase: Phase::Idle,
            prepare_voters: HashSet::new(),
            precommit_voters: HashSet::new(),
            commit_voters: HashSet::new(),
        }
    }
}
