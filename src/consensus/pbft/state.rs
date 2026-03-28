use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
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
    peer_ids: Vec<NodeId>,
    current_view: AtomicU64,
    state: Arc<Mutex<PbftState>>,
}

struct PbftState {
    view: ViewNumber,
    next_sequence: SequenceNumber,
    slots: HashMap<SequenceNumber, SlotState>,
    pending_payloads: HashMap<SequenceNumber, Payload>,
    committed: Vec<CommittedEntry>,
}

impl PbftNode {
    pub fn new(id: NodeId, cluster_size: usize) -> Self {
        let peer_ids: Vec<NodeId> = (0..cluster_size as u64)
            .map(NodeId)
            .filter(|&pid| pid != id)
            .collect();

        Self {
            id,
            cluster_size,
            peer_ids,
            current_view: AtomicU64::new(0),
            state: Arc::new(Mutex::new(PbftState {
                view: 0,
                next_sequence: 1,
                slots: HashMap::new(),
                pending_payloads: HashMap::new(),
                committed: Vec::new(),
            })),
        }
    }

    fn primary_for_view(&self, view: ViewNumber) -> NodeId {
        NodeId(view % self.cluster_size as u64)
    }

    /// Quorum size: 2f + 1 where f = floor((n-1)/3)
    fn quorum(&self) -> usize {
        let f = (self.cluster_size - 1) / 3;
        2 * f + 1
    }

    /// Prepares needed: 2f (not counting the pre-prepare itself)
    fn prepare_threshold(&self) -> usize {
        let f = (self.cluster_size - 1) / 3;
        2 * f
    }

    /// Build a message from self to a target.
    fn msg_to(&self, to: NodeId, payload: ProtocolMessage) -> NetworkMessage {
        NetworkMessage {
            from: self.id,
            to,
            payload,
        }
    }

    /// Broadcast a PBFT message to all peers.
    fn broadcast_pbft(&self, pbft_msg: PbftMessage) -> Vec<NetworkMessage> {
        self.peer_ids
            .iter()
            .map(|&peer| self.msg_to(peer, ProtocolMessage::Pbft(pbft_msg.clone())))
            .collect()
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

    fn peers(&self) -> Vec<NodeId> {
        self.peer_ids.clone()
    }

    async fn propose(&self, payload: Payload) -> (ProposeResult, Vec<NetworkMessage>) {
        let mut state = self.state.lock().await;
        let primary = self.primary_for_view(state.view);

        if primary != self.id {
            return (ProposeResult::NotLeader(Some(primary)), vec![]);
        }

        let seq = state.next_sequence;
        state.next_sequence += 1;
        let view = state.view;
        let digest = payload.digest();

        let mut slot = SlotState::new(view, seq);
        slot.phase = Phase::PrePrepared;
        state.slots.insert(seq, slot);
        state.pending_payloads.insert(seq, payload.clone());

        debug!(node = %self.id, seq, view, "primary assigned sequence, broadcasting pre-prepare + prepare");

        // Broadcast pre-prepare to all replicas
        let mut outgoing = self.broadcast_pbft(PbftMessage::PrePrepare {
            view,
            sequence: seq,
            digest,
            payload,
        });

        // Primary also broadcasts its own Prepare (per PBFT spec,
        // the primary participates in the prepare phase too)
        outgoing.extend(self.broadcast_pbft(PbftMessage::Prepare {
            view,
            sequence: seq,
            digest,
            replica: self.id,
        }));

        (ProposeResult::Accepted, outgoing)
    }

    async fn handle_message(&self, msg: NetworkMessage) -> Vec<NetworkMessage> {
        let pbft_msg = match msg.payload {
            ProtocolMessage::Pbft(m) => m,
            _ => {
                warn!(node = %self.id, "received non-PBFT message");
                return vec![];
            }
        };

        let mut state = self.state.lock().await;

        match pbft_msg {
            PbftMessage::PrePrepare {
                view,
                sequence,
                digest,
                payload,
            } => {
                if view != state.view {
                    return vec![];
                }

                let mut slot = SlotState::new(view, sequence);
                slot.phase = Phase::PrePrepared;
                state.slots.insert(sequence, slot);
                state.pending_payloads.insert(sequence, payload);

                debug!(node = %self.id, seq = sequence, "received pre-prepare, broadcasting prepare");

                // Respond with prepare to all peers
                self.broadcast_pbft(PbftMessage::Prepare {
                    view,
                    sequence,
                    digest,
                    replica: self.id,
                })
            }

            PbftMessage::Prepare {
                view,
                sequence,
                digest,
                replica: _,
            } => {
                if view != state.view {
                    return vec![];
                }

                if let Some(slot) = state.slots.get_mut(&sequence) {
                    if slot.phase == Phase::PrePrepared {
                        slot.prepare_count += 1;

                        if slot.prepare_count >= self.prepare_threshold() {
                            slot.phase = Phase::Prepared;
                            // Count our own commit
                            slot.commit_count = 1;
                            debug!(
                                node = %self.id, seq = sequence,
                                prepares = slot.prepare_count,
                                "reached prepare threshold, broadcasting commit"
                            );

                            // Broadcast commit
                            return self.broadcast_pbft(PbftMessage::Commit {
                                view,
                                sequence,
                                digest,
                                replica: self.id,
                            });
                        }
                    }
                }
                vec![]
            }

            PbftMessage::Commit {
                view,
                sequence,
                digest: _,
                replica: _,
            } => {
                if view != state.view {
                    return vec![];
                }

                if let Some(slot) = state.slots.get_mut(&sequence) {
                    // Accept commits in both Prepared and PrePrepared phases
                    // (commits can arrive before we've collected enough prepares)
                    if slot.phase == Phase::Prepared || slot.phase == Phase::PrePrepared {
                        slot.commit_count += 1;

                        if slot.commit_count >= self.quorum() && slot.phase != Phase::Committed {
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
                vec![]
            }

            PbftMessage::ViewChange { .. } | PbftMessage::NewView { .. } => {
                // View change — will implement next
                debug!(node = %self.id, "view change not yet implemented");
                vec![]
            }
        }
    }

    async fn tick(&self) -> Vec<NetworkMessage> {
        // PBFT doesn't have periodic ticks (no heartbeat).
        // View change timeout will go here.
        vec![]
    }

    async fn poll_committed(&self) -> Vec<CommittedEntry> {
        let mut state = self.state.lock().await;
        std::mem::take(&mut state.committed)
    }

    fn leader(&self) -> Option<NodeId> {
        let view = self.current_view.load(Ordering::Relaxed);
        Some(self.primary_for_view(view))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn primary_accepts_and_broadcasts_pre_prepare() {
        let node = PbftNode::new(NodeId(0), 4);
        let payload = Payload::new(b"hello".to_vec());

        let (result, outgoing) = node.propose(payload).await;
        assert_eq!(result, ProposeResult::Accepted);
        // Should broadcast pre-prepare (3) + prepare (3) to peers
        assert_eq!(outgoing.len(), 6);
    }

    #[tokio::test]
    async fn non_primary_rejects_proposal() {
        let node = PbftNode::new(NodeId(1), 4);
        let payload = Payload::new(b"hello".to_vec());

        let (result, outgoing) = node.propose(payload).await;
        assert!(matches!(result, ProposeResult::NotLeader(Some(NodeId(0)))));
        assert!(outgoing.is_empty());
    }

    #[tokio::test]
    async fn replica_responds_with_prepare_on_pre_prepare() {
        let node = PbftNode::new(NodeId(1), 4);
        let payload = Payload::new(b"hello".to_vec());
        let digest = payload.digest();

        let msg = NetworkMessage {
            from: NodeId(0),
            to: NodeId(1),
            payload: ProtocolMessage::Pbft(PbftMessage::PrePrepare {
                view: 0,
                sequence: 1,
                digest,
                payload,
            }),
        };

        let outgoing = node.handle_message(msg).await;
        // Should broadcast prepare to 3 peers
        assert_eq!(outgoing.len(), 3);
    }

    #[tokio::test]
    async fn quorum_size_is_correct() {
        let node = PbftNode::new(NodeId(0), 4);
        assert_eq!(node.quorum(), 3);
        assert_eq!(node.fault_tolerance(), 1);

        let node = PbftNode::new(NodeId(0), 7);
        assert_eq!(node.quorum(), 5);
        assert_eq!(node.fault_tolerance(), 2);
    }
}
