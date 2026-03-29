use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use super::{Phase, SlotState};
use crate::consensus::{CommittedEntry, ConsensusNode, ProposeResult};
use crate::network::{NetworkMessage, PbftMessage, ProtocolMessage};
use crate::types::{NodeId, Payload, SequenceNumber, ViewNumber};

const VIEW_CHANGE_TIMEOUT: u64 = 40;

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

    // View change state
    /// Ticks remaining before this node suspects the primary and starts view change.
    view_change_timeout: u64,
    /// Whether we've sent a ViewChange for the next view already.
    view_change_in_progress: bool,
    /// ViewChange messages collected for each proposed new view.
    view_change_votes: HashMap<ViewNumber, HashSet<NodeId>>,
    /// Highest committed sequence number (used in ViewChange messages).
    highest_committed_seq: SequenceNumber,
    /// Whether the current primary has shown activity this timeout period.
    primary_active: bool,
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
                view_change_timeout: VIEW_CHANGE_TIMEOUT,
                view_change_in_progress: false,
                view_change_votes: HashMap::new(),
                highest_committed_seq: 0,
                primary_active: false,
            })),
        }
    }

    fn primary_for_view(&self, view: ViewNumber) -> NodeId {
        NodeId(view % self.cluster_size as u64)
    }

    fn quorum(&self) -> usize {
        let f = (self.cluster_size - 1) / 3;
        2 * f + 1
    }

    fn prepare_threshold(&self) -> usize {
        let f = (self.cluster_size - 1) / 3;
        2 * f
    }

    fn msg_to(&self, to: NodeId, payload: ProtocolMessage) -> NetworkMessage {
        NetworkMessage {
            from: self.id,
            to,
            payload,
        }
    }

    fn broadcast_pbft(&self, pbft_msg: PbftMessage) -> Vec<NetworkMessage> {
        self.peer_ids
            .iter()
            .map(|&peer| self.msg_to(peer, ProtocolMessage::Pbft(pbft_msg.clone())))
            .collect()
    }

    fn enter_new_view(state: &mut PbftState, new_view: ViewNumber) {
        state.view = new_view;
        state.view_change_in_progress = false;
        state.view_change_timeout = VIEW_CHANGE_TIMEOUT;
        state.primary_active = false;
        // Keep committed slots but clear pending consensus slots
        state.slots.retain(|_, slot| slot.phase == Phase::Committed);
        state.pending_payloads.clear();
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

        if state.view_change_in_progress {
            return (ProposeResult::NotLeader(None), vec![]);
        }

        let seq = state.next_sequence;
        state.next_sequence += 1;
        let view = state.view;
        let digest = payload.digest();

        let mut slot = SlotState::new(view, seq);
        slot.phase = Phase::PrePrepared;
        state.slots.insert(seq, slot);
        state.pending_payloads.insert(seq, payload.clone());

        debug!(node = %self.id, seq, view, "primary assigned sequence");

        let mut outgoing = self.broadcast_pbft(PbftMessage::PrePrepare {
            view,
            sequence: seq,
            digest,
            payload,
        });

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
                if view != state.view || state.view_change_in_progress {
                    return vec![];
                }

                // Primary is active — reset view change timer
                state.primary_active = true;
                state.view_change_timeout = VIEW_CHANGE_TIMEOUT;

                let mut slot = SlotState::new(view, sequence);
                slot.phase = Phase::PrePrepared;
                state.slots.insert(sequence, slot);
                state.pending_payloads.insert(sequence, payload);

                debug!(node = %self.id, seq = sequence, "received pre-prepare, broadcasting prepare");

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
                            slot.commit_count = 1; // count own commit
                            debug!(
                                node = %self.id, seq = sequence,
                                prepares = slot.prepare_count,
                                "reached prepare threshold, broadcasting commit"
                            );

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
                    if slot.phase == Phase::Prepared || slot.phase == Phase::PrePrepared {
                        slot.commit_count += 1;

                        if slot.commit_count >= self.quorum() && slot.phase != Phase::Committed {
                            slot.phase = Phase::Committed;
                            if sequence > state.highest_committed_seq {
                                state.highest_committed_seq = sequence;
                            }
                            debug!(node = %self.id, seq = sequence, "committed");

                            if let Some(payload) = state.pending_payloads.remove(&sequence) {
                                state.committed.push(CommittedEntry { sequence, payload });
                            }
                        }
                    }
                }
                vec![]
            }

            PbftMessage::ViewChange {
                new_view,
                replica,
                last_committed_seq: _,
            } => {
                if new_view <= state.view {
                    return vec![];
                }

                let votes = state
                    .view_change_votes
                    .entry(new_view)
                    .or_insert_with(HashSet::new);
                votes.insert(replica);

                let vote_count = votes.len();
                debug!(
                    node = %self.id, new_view,
                    votes = vote_count, needed = self.quorum(),
                    "received view-change vote from {}", replica
                );

                // If we haven't voted yet for this view, join the view change
                if !votes.contains(&self.id) {
                    votes.insert(self.id);
                    let mut outgoing = self.broadcast_pbft(PbftMessage::ViewChange {
                        new_view,
                        replica: self.id,
                        last_committed_seq: state.highest_committed_seq,
                    });

                    // Re-check after adding our own vote
                    let votes = state.view_change_votes.get(&new_view).unwrap();
                    if votes.len() >= self.quorum() && self.primary_for_view(new_view) == self.id {
                        debug!(node = %self.id, new_view, "I am new primary, broadcasting NewView");
                        Self::enter_new_view(&mut state, new_view);
                        self.current_view.store(new_view, Ordering::Relaxed);
                        state.next_sequence = state.highest_committed_seq + 1;

                        outgoing.extend(self.broadcast_pbft(PbftMessage::NewView {
                            view: new_view,
                            next_sequence: state.next_sequence,
                        }));
                    }

                    return outgoing;
                }

                // Check if we have quorum and we're the new primary
                if vote_count >= self.quorum() && self.primary_for_view(new_view) == self.id {
                    debug!(node = %self.id, new_view, "quorum reached, I am new primary");
                    Self::enter_new_view(&mut state, new_view);
                    self.current_view.store(new_view, Ordering::Relaxed);
                    state.next_sequence = state.highest_committed_seq + 1;

                    return self.broadcast_pbft(PbftMessage::NewView {
                        view: new_view,
                        next_sequence: state.next_sequence,
                    });
                }

                vec![]
            }

            PbftMessage::NewView {
                view: new_view,
                next_sequence,
            } => {
                if new_view <= state.view {
                    return vec![];
                }

                debug!(
                    node = %self.id, new_view, next_sequence,
                    "received NewView, transitioning"
                );

                Self::enter_new_view(&mut state, new_view);
                self.current_view.store(new_view, Ordering::Relaxed);
                state.next_sequence = next_sequence;

                vec![]
            }
        }
    }

    async fn tick(&self) -> Vec<NetworkMessage> {
        let mut state = self.state.lock().await;

        // Primary doesn't need to monitor itself
        if self.primary_for_view(state.view) == self.id {
            return vec![];
        }

        // Already in a view change
        if state.view_change_in_progress {
            return vec![];
        }

        if state.view_change_timeout == 0 {
            // Timeout — suspect the primary
            let new_view = state.view + 1;
            state.view_change_in_progress = true;

            debug!(
                node = %self.id, current_view = state.view, new_view,
                "view change timeout, suspecting primary {}", self.primary_for_view(state.view)
            );

            // Vote for ourselves
            let votes = state
                .view_change_votes
                .entry(new_view)
                .or_insert_with(HashSet::new);
            votes.insert(self.id);

            return self.broadcast_pbft(PbftMessage::ViewChange {
                new_view,
                replica: self.id,
                last_committed_seq: state.highest_committed_seq,
            });
        }

        state.view_change_timeout -= 1;
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
        // pre-prepare (3) + prepare (3)
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

    #[tokio::test]
    async fn tick_triggers_view_change_on_timeout() {
        let node = PbftNode::new(NodeId(1), 4);

        // Tick until timeout expires
        let mut total_outgoing = vec![];
        for _ in 0..=VIEW_CHANGE_TIMEOUT {
            let out = node.tick().await;
            total_outgoing.extend(out);
        }

        // Should have broadcast ViewChange messages to 3 peers
        assert_eq!(
            total_outgoing.len(),
            3,
            "should broadcast ViewChange to all peers"
        );
    }
}
