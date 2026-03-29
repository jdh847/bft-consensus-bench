use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use super::{Phase, SlotState};
use crate::consensus::{CommittedEntry, ConsensusNode, ProposeResult};
use crate::network::{HotStuffMessage, HotStuffPhaseTag, NetworkMessage, ProtocolMessage};
use crate::types::{NodeId, Payload, SequenceNumber, ViewNumber};

const VIEW_CHANGE_TIMEOUT: u64 = 40;

pub struct HotStuffNode {
    id: NodeId,
    cluster_size: usize,
    peer_ids: Vec<NodeId>,
    current_view: AtomicU64,
    state: Arc<Mutex<HotStuffState>>,
}

struct HotStuffState {
    view: ViewNumber,
    next_sequence: SequenceNumber,
    slots: HashMap<SequenceNumber, SlotState>,
    pending_payloads: HashMap<SequenceNumber, Payload>,
    committed: Vec<CommittedEntry>,

    // View change state
    highest_qc_view: ViewNumber,
    highest_qc_phase: HotStuffPhaseTag,
    view_change_timeout: u64,
    /// NewView messages collected for each proposed new view.
    new_view_messages: HashMap<ViewNumber, HashSet<NodeId>>,
    /// Whether the current leader has shown activity this timeout period.
    leader_active: bool,
}

impl HotStuffNode {
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
            state: Arc::new(Mutex::new(HotStuffState {
                view: 0,
                next_sequence: 1,
                slots: HashMap::new(),
                pending_payloads: HashMap::new(),
                committed: Vec::new(),
                highest_qc_view: 0,
                highest_qc_phase: HotStuffPhaseTag::Prepare,
                view_change_timeout: VIEW_CHANGE_TIMEOUT,
                new_view_messages: HashMap::new(),
                leader_active: false,
            })),
        }
    }

    fn leader_for_view(&self, view: ViewNumber) -> NodeId {
        NodeId(view % self.cluster_size as u64)
    }

    fn quorum(&self) -> usize {
        let f = (self.cluster_size - 1) / 3;
        2 * f + 1
    }

    fn msg_to(&self, to: NodeId, payload: ProtocolMessage) -> NetworkMessage {
        NetworkMessage {
            from: self.id,
            to,
            payload,
        }
    }

    fn broadcast_hotstuff(&self, hs_msg: HotStuffMessage) -> Vec<NetworkMessage> {
        self.peer_ids
            .iter()
            .map(|&peer| self.msg_to(peer, ProtocolMessage::HotStuff(hs_msg.clone())))
            .collect()
    }

    fn enter_new_view(state: &mut HotStuffState, new_view: ViewNumber) {
        state.view = new_view;
        state.view_change_timeout = VIEW_CHANGE_TIMEOUT;
        state.leader_active = false;
        // Keep decided slots but clear pending consensus slots
        state.slots.retain(|_, slot| slot.phase == Phase::Decided);
        state.pending_payloads.clear();
    }
}

#[async_trait]
impl ConsensusNode for HotStuffNode {
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
        let leader = self.leader_for_view(state.view);

        if leader != self.id {
            return (ProposeResult::NotLeader(Some(leader)), vec![]);
        }

        let seq = state.next_sequence;
        state.next_sequence += 1;
        let view = state.view;
        let digest = payload.digest();

        let mut slot = SlotState::new(view, seq);
        slot.phase = Phase::Prepare;
        slot.prepare_votes = 1; // self-vote
        state.slots.insert(seq, slot);
        state.pending_payloads.insert(seq, payload.clone());

        debug!(node = %self.id, seq, view, "leader broadcasting propose");

        // O(n) — leader sends Propose to all replicas (star topology)
        let outgoing = self.broadcast_hotstuff(HotStuffMessage::Propose {
            view,
            sequence: seq,
            digest,
            payload,
            justify_view: state.highest_qc_view,
            justify_phase: state.highest_qc_phase,
        });

        (ProposeResult::Accepted, outgoing)
    }

    async fn handle_message(&self, msg: NetworkMessage) -> Vec<NetworkMessage> {
        let hs_msg = match msg.payload {
            ProtocolMessage::HotStuff(m) => m,
            _ => {
                warn!(node = %self.id, "received non-HotStuff message");
                return vec![];
            }
        };

        let mut state = self.state.lock().await;

        match hs_msg {
            HotStuffMessage::Propose {
                view,
                sequence,
                digest,
                payload,
                justify_view: _,
                justify_phase: _,
            } => {
                // Auto-advance view if we see a higher view
                if view > state.view {
                    Self::enter_new_view(&mut state, view);
                    self.current_view.store(view, Ordering::Relaxed);
                }

                if view != state.view {
                    return vec![];
                }

                // Leader is active — reset view change timer
                state.leader_active = true;
                state.view_change_timeout = VIEW_CHANGE_TIMEOUT;

                let mut slot = SlotState::new(view, sequence);
                slot.phase = Phase::Prepare;
                state.slots.insert(sequence, slot);
                state.pending_payloads.insert(sequence, payload);

                debug!(node = %self.id, seq = sequence, "received propose, sending vote to leader");

                // O(1) — replica sends Vote to leader only (star topology)
                let leader = self.leader_for_view(view);
                vec![self.msg_to(
                    leader,
                    ProtocolMessage::HotStuff(HotStuffMessage::Vote {
                        view,
                        sequence,
                        digest,
                        phase: HotStuffPhaseTag::Prepare,
                        voter: self.id,
                    }),
                )]
            }

            HotStuffMessage::Vote {
                view,
                sequence,
                digest,
                phase,
                voter: _,
            } => {
                // Only leader processes votes
                if self.leader_for_view(view) != self.id {
                    return vec![];
                }

                if view != state.view {
                    return vec![];
                }

                let quorum = self.quorum();

                // Extract slot info, update counts, then decide what to broadcast.
                // Two-phase access avoids double-mutable-borrow on `state`.
                let action = {
                    let slot = match state.slots.get_mut(&sequence) {
                        Some(s) => s,
                        None => return vec![],
                    };
                    match phase {
                        HotStuffPhaseTag::Prepare => {
                            if slot.phase != Phase::Prepare {
                                return vec![];
                            }
                            slot.prepare_votes += 1;
                            if slot.prepare_votes >= quorum {
                                slot.phase = Phase::PreCommit;
                                slot.precommit_votes = 1;
                                let vc = slot.prepare_votes;
                                debug!(
                                    node = %self.id, seq = sequence, votes = vc,
                                    "PrepareQC formed, broadcasting"
                                );
                                Some((HotStuffPhaseTag::Prepare, vc, false))
                            } else {
                                None
                            }
                        }
                        HotStuffPhaseTag::PreCommit => {
                            if slot.phase != Phase::PreCommit {
                                return vec![];
                            }
                            slot.precommit_votes += 1;
                            if slot.precommit_votes >= quorum {
                                slot.phase = Phase::Commit;
                                slot.commit_votes = 1;
                                let vc = slot.precommit_votes;
                                debug!(
                                    node = %self.id, seq = sequence, votes = vc,
                                    "PreCommitQC formed, broadcasting"
                                );
                                Some((HotStuffPhaseTag::PreCommit, vc, false))
                            } else {
                                None
                            }
                        }
                        HotStuffPhaseTag::Commit => {
                            if slot.phase != Phase::Commit {
                                return vec![];
                            }
                            slot.commit_votes += 1;
                            if slot.commit_votes >= quorum {
                                slot.phase = Phase::Decided;
                                let vc = slot.commit_votes;
                                debug!(node = %self.id, seq = sequence, "CommitQC formed, committed");
                                Some((HotStuffPhaseTag::Commit, vc, true))
                            } else {
                                None
                            }
                        }
                    }
                };

                if let Some((qc_phase, vote_count, do_commit)) = action {
                    state.highest_qc_view = view;
                    state.highest_qc_phase = qc_phase;

                    if do_commit {
                        if let Some(payload) = state.pending_payloads.remove(&sequence) {
                            state.committed.push(CommittedEntry { sequence, payload });
                        }
                    }

                    return self.broadcast_hotstuff(HotStuffMessage::QuorumCertificate {
                        view,
                        sequence,
                        digest,
                        phase: qc_phase,
                        vote_count,
                    });
                }

                vec![]
            }

            HotStuffMessage::QuorumCertificate {
                view,
                sequence,
                digest,
                phase,
                vote_count: _,
            } => {
                // Update highest QC tracking
                if view > state.highest_qc_view
                    || (view == state.highest_qc_view
                        && phase_ord(phase) > phase_ord(state.highest_qc_phase))
                {
                    state.highest_qc_view = view;
                    state.highest_qc_phase = phase;
                }

                if view != state.view {
                    return vec![];
                }

                let slot = state.slots.get_mut(&sequence);
                let leader = self.leader_for_view(view);

                match phase {
                    HotStuffPhaseTag::Prepare => {
                        // Advance slot to PreCommit, send PreCommit vote to leader
                        if let Some(slot) = slot {
                            if slot.phase == Phase::Prepare {
                                slot.phase = Phase::PreCommit;
                            }
                        }
                        vec![self.msg_to(
                            leader,
                            ProtocolMessage::HotStuff(HotStuffMessage::Vote {
                                view,
                                sequence,
                                digest,
                                phase: HotStuffPhaseTag::PreCommit,
                                voter: self.id,
                            }),
                        )]
                    }
                    HotStuffPhaseTag::PreCommit => {
                        // Advance slot to Commit, send Commit vote to leader
                        if let Some(slot) = slot {
                            if slot.phase == Phase::PreCommit {
                                slot.phase = Phase::Commit;
                            }
                        }
                        vec![self.msg_to(
                            leader,
                            ProtocolMessage::HotStuff(HotStuffMessage::Vote {
                                view,
                                sequence,
                                digest,
                                phase: HotStuffPhaseTag::Commit,
                                voter: self.id,
                            }),
                        )]
                    }
                    HotStuffPhaseTag::Commit => {
                        // CommitQC — entry is decided on this replica too
                        if let Some(slot) = slot {
                            if slot.phase != Phase::Decided {
                                slot.phase = Phase::Decided;
                                debug!(
                                    node = %self.id, seq = sequence,
                                    "received CommitQC, committed"
                                );
                                if let Some(payload) = state.pending_payloads.remove(&sequence) {
                                    state.committed.push(CommittedEntry { sequence, payload });
                                }
                            }
                        }
                        vec![]
                    }
                }
            }

            HotStuffMessage::NewView {
                new_view,
                highest_qc_view: _,
                highest_qc_phase: _,
                replica,
            } => {
                if new_view <= state.view {
                    return vec![];
                }

                let votes = state.new_view_messages.entry(new_view).or_default();
                votes.insert(replica);

                let vote_count = votes.len();

                debug!(
                    node = %self.id, new_view,
                    votes = vote_count, needed = self.quorum(),
                    "received NewView from {}", replica
                );

                // If we're the next leader and have quorum, enter new view
                if vote_count >= self.quorum() && self.leader_for_view(new_view) == self.id {
                    debug!(
                        node = %self.id, new_view,
                        "NewView quorum reached, entering new view as leader"
                    );
                    Self::enter_new_view(&mut state, new_view);
                    self.current_view.store(new_view, Ordering::Relaxed);
                }

                vec![]
            }
        }
    }

    async fn tick(&self) -> Vec<NetworkMessage> {
        let mut state = self.state.lock().await;

        // Leader doesn't need to monitor itself
        if self.leader_for_view(state.view) == self.id {
            return vec![];
        }

        if state.view_change_timeout == 0 {
            // Timeout — suspect the leader
            let new_view = state.view + 1;
            let next_leader = self.leader_for_view(new_view);

            debug!(
                node = %self.id, current_view = state.view, new_view,
                "view change timeout, sending NewView to {}", next_leader
            );

            // Self-vote
            state
                .new_view_messages
                .entry(new_view)
                .or_default()
                .insert(self.id);

            // Reset timeout so we don't spam
            state.view_change_timeout = VIEW_CHANGE_TIMEOUT;

            // Send NewView to next leader
            return vec![self.msg_to(
                next_leader,
                ProtocolMessage::HotStuff(HotStuffMessage::NewView {
                    new_view,
                    highest_qc_view: state.highest_qc_view,
                    highest_qc_phase: state.highest_qc_phase,
                    replica: self.id,
                }),
            )];
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
        Some(self.leader_for_view(view))
    }
}

/// Map phase tags to ordinals for comparison.
fn phase_ord(phase: HotStuffPhaseTag) -> u8 {
    match phase {
        HotStuffPhaseTag::Prepare => 0,
        HotStuffPhaseTag::PreCommit => 1,
        HotStuffPhaseTag::Commit => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn leader_accepts_and_broadcasts_propose() {
        let node = HotStuffNode::new(NodeId(0), 4);
        let payload = Payload::new(b"hello".to_vec());

        let (result, outgoing) = node.propose(payload).await;
        assert_eq!(result, ProposeResult::Accepted);
        // Propose to 3 peers (star topology, O(n))
        assert_eq!(outgoing.len(), 3);
    }

    #[tokio::test]
    async fn non_leader_rejects_proposal() {
        let node = HotStuffNode::new(NodeId(1), 4);
        let payload = Payload::new(b"hello".to_vec());

        let (result, outgoing) = node.propose(payload).await;
        assert!(matches!(result, ProposeResult::NotLeader(Some(NodeId(0)))));
        assert!(outgoing.is_empty());
    }

    #[tokio::test]
    async fn replica_responds_with_vote_on_propose() {
        let node = HotStuffNode::new(NodeId(1), 4);
        let payload = Payload::new(b"hello".to_vec());
        let digest = payload.digest();

        let msg = NetworkMessage {
            from: NodeId(0),
            to: NodeId(1),
            payload: ProtocolMessage::HotStuff(HotStuffMessage::Propose {
                view: 0,
                sequence: 1,
                digest,
                payload,
                justify_view: 0,
                justify_phase: HotStuffPhaseTag::Prepare,
            }),
        };

        let outgoing = node.handle_message(msg).await;
        // Should send exactly 1 Vote to leader (star topology)
        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0].to, NodeId(0));
    }

    #[tokio::test]
    async fn quorum_size_is_correct() {
        let node4 = HotStuffNode::new(NodeId(0), 4);
        assert_eq!(node4.quorum(), 3);
        assert_eq!(node4.fault_tolerance(), 1);

        let node7 = HotStuffNode::new(NodeId(0), 7);
        assert_eq!(node7.quorum(), 5);
        assert_eq!(node7.fault_tolerance(), 2);
    }

    #[tokio::test]
    async fn tick_triggers_view_change_on_timeout() {
        let node = HotStuffNode::new(NodeId(1), 4);

        // Tick until timeout expires
        let mut total_outgoing = vec![];
        for _ in 0..=VIEW_CHANGE_TIMEOUT {
            let out = node.tick().await;
            total_outgoing.extend(out);
        }

        // Should have sent NewView to next leader (node 1 in view 1)
        assert_eq!(
            total_outgoing.len(),
            1,
            "should send NewView to next leader"
        );
    }

    #[tokio::test]
    async fn leader_forms_qc_at_quorum() {
        // n=4, quorum=3. Leader self-votes (1) + needs 2 more votes.
        let leader = HotStuffNode::new(NodeId(0), 4);
        let payload = Payload::new(b"test".to_vec());
        let digest = payload.digest();

        // Leader proposes (self-votes prepare)
        let (result, _) = leader.propose(payload).await;
        assert_eq!(result, ProposeResult::Accepted);

        // Send 2 Prepare votes from replicas
        let vote1 = NetworkMessage {
            from: NodeId(1),
            to: NodeId(0),
            payload: ProtocolMessage::HotStuff(HotStuffMessage::Vote {
                view: 0,
                sequence: 1,
                digest,
                phase: HotStuffPhaseTag::Prepare,
                voter: NodeId(1),
            }),
        };
        let out1 = leader.handle_message(vote1).await;
        assert!(out1.is_empty(), "2 votes not enough for quorum of 3");

        let vote2 = NetworkMessage {
            from: NodeId(2),
            to: NodeId(0),
            payload: ProtocolMessage::HotStuff(HotStuffMessage::Vote {
                view: 0,
                sequence: 1,
                digest,
                phase: HotStuffPhaseTag::Prepare,
                voter: NodeId(2),
            }),
        };
        let out2 = leader.handle_message(vote2).await;
        // 3 votes = quorum → broadcasts PrepareQC to 3 peers
        assert_eq!(out2.len(), 3, "should broadcast PrepareQC at quorum");
    }
}
