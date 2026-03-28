use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use super::{Phase, SlotState};
use crate::consensus::{CommittedEntry, ConsensusNode, ProposeResult};
use crate::network::{NetworkMessage, PbftMessage, ProtocolMessage};
use crate::types::{NodeId, Payload, SequenceNumber, ViewNumber};

pub struct PbftNode {
    id: NodeId,
    cluster_size: usize,
    state: Arc<Mutex<PbftState>>,
}

struct PbftState {
    view: ViewNumber,
    next_sequence: SequenceNumber,
    slots: HashMap<SequenceNumber, SlotState>,
    /// Payloads held until the slot commits.
    pending_payloads: HashMap<SequenceNumber, Payload>,
    committed: Vec<CommittedEntry>,
}

impl PbftNode {
    pub fn new(id: NodeId, cluster_size: usize) -> Self {
        Self {
            id,
            cluster_size,
            state: Arc::new(Mutex::new(PbftState {
                view: 0,
                next_sequence: 1,
                slots: HashMap::new(),
                pending_payloads: HashMap::new(),
                committed: Vec::new(),
            })),
        }
    }

    /// The primary for a given view is determined by `view % n`.
    fn primary_for_view(&self, view: ViewNumber) -> NodeId {
        NodeId((view % self.cluster_size as u64) as u64)
    }

    /// Quorum size: 2f + 1 where f = floor((n-1)/3)
    fn quorum(&self) -> usize {
        let f = (self.cluster_size - 1) / 3;
        2 * f + 1
    }

    /// Number of matching messages needed to advance from pre-prepare
    /// to prepared: 2f (excluding the pre-prepare itself).
    fn prepare_threshold(&self) -> usize {
        let f = (self.cluster_size - 1) / 3;
        2 * f
    }
}

#[async_trait]
impl ConsensusNode for PbftNode {
    fn id(&self) -> NodeId {
        self.id
    }

    fn cluster_size(&self) -> usize {
        self.cluster_size
    }

    fn fault_tolerance(&self) -> usize {
        (self.cluster_size - 1) / 3
    }

    async fn propose(&self, payload: Payload) -> ProposeResult {
        let mut state = self.state.lock().await;
        let primary = self.primary_for_view(state.view);

        if primary != self.id {
            return ProposeResult::NotLeader(Some(primary));
        }

        let seq = state.next_sequence;
        state.next_sequence += 1;

        let mut slot = SlotState::new(state.view, seq);
        slot.phase = Phase::PrePrepared;
        state.slots.insert(seq, slot);
        state.pending_payloads.insert(seq, payload);

        debug!(node = %self.id, seq, view = state.view, "primary assigned sequence");

        ProposeResult::Accepted
    }

    async fn handle_message(&self, msg: NetworkMessage) {
        let pbft_msg = match msg.payload {
            ProtocolMessage::Pbft(m) => m,
            _ => {
                warn!(node = %self.id, "received non-PBFT message");
                return;
            }
        };

        let mut state = self.state.lock().await;

        match pbft_msg {
            PbftMessage::PrePrepare {
                view,
                sequence,
                digest: _,
                payload,
            } => {
                if view != state.view {
                    return;
                }

                let mut slot = SlotState::new(view, sequence);
                slot.phase = Phase::PrePrepared;
                state.slots.insert(sequence, slot);
                state.pending_payloads.insert(sequence, payload);

                debug!(node = %self.id, seq = sequence, "received pre-prepare, moving to PrePrepared");
            }

            PbftMessage::Prepare {
                view,
                sequence,
                digest: _,
                replica: _,
            } => {
                if view != state.view {
                    return;
                }

                if let Some(slot) = state.slots.get_mut(&sequence) {
                    if slot.phase == Phase::PrePrepared {
                        slot.prepare_count += 1;

                        if slot.prepare_count >= self.prepare_threshold() {
                            slot.phase = Phase::Prepared;
                            debug!(
                                node = %self.id, seq = sequence,
                                prepares = slot.prepare_count,
                                "reached prepare threshold, moving to Prepared"
                            );
                        }
                    }
                }
            }

            PbftMessage::Commit {
                view,
                sequence,
                digest: _,
                replica: _,
            } => {
                if view != state.view {
                    return;
                }

                if let Some(slot) = state.slots.get_mut(&sequence) {
                    if slot.phase == Phase::Prepared {
                        slot.commit_count += 1;

                        if slot.commit_count >= self.quorum() {
                            slot.phase = Phase::Committed;
                            debug!(node = %self.id, seq = sequence, "committed");

                            if let Some(payload) = state.pending_payloads.remove(&sequence) {
                                state.committed.push(CommittedEntry {
                                    sequence,
                                    payload,
                                });
                            }
                        }
                    }
                }
            }

            PbftMessage::ViewChange { .. } | PbftMessage::NewView { .. } => {
                // View change protocol — will implement in week 2
                debug!(node = %self.id, "view change message received (not yet implemented)");
            }
        }
    }

    async fn poll_committed(&self) -> Vec<CommittedEntry> {
        let mut state = self.state.lock().await;
        std::mem::take(&mut state.committed)
    }

    fn leader(&self) -> Option<NodeId> {
        // Can't read async state here, so we return None for now.
        // TODO: expose view via an atomic or separate lock
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn primary_accepts_proposal() {
        // Node 0 is primary in view 0
        let node = PbftNode::new(NodeId(0), 4);
        let payload = Payload::new(b"hello".to_vec());

        let result = node.propose(payload).await;
        assert_eq!(result, ProposeResult::Accepted);
    }

    #[tokio::test]
    async fn non_primary_rejects_proposal() {
        // Node 1 is not primary in view 0
        let node = PbftNode::new(NodeId(1), 4);
        let payload = Payload::new(b"hello".to_vec());

        let result = node.propose(payload).await;
        assert!(matches!(result, ProposeResult::NotLeader(Some(NodeId(0)))));
    }

    #[tokio::test]
    async fn quorum_size_is_correct() {
        // n=4, f=1, quorum=3
        let node = PbftNode::new(NodeId(0), 4);
        assert_eq!(node.quorum(), 3);
        assert_eq!(node.fault_tolerance(), 1);

        // n=7, f=2, quorum=5
        let node = PbftNode::new(NodeId(0), 7);
        assert_eq!(node.quorum(), 5);
        assert_eq!(node.fault_tolerance(), 2);
    }
}
