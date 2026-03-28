use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use rand::Rng;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use super::Role;
use crate::consensus::{CommittedEntry, ConsensusNode, ProposeResult};
use crate::network::{LogEntry, NetworkMessage, ProtocolMessage, RaftMessage};
use crate::types::{NodeId, Payload};

pub struct RaftNode {
    id: NodeId,
    cluster_size: usize,
    peer_ids: Vec<NodeId>,
    leader_id_atomic: AtomicU64,
    state: Arc<Mutex<RaftState>>,
}

struct RaftState {
    // Persistent state
    current_term: u64,
    voted_for: Option<NodeId>,
    log: Vec<LogEntry>,

    // Volatile state
    commit_index: u64,
    role: Role,
    leader_id: Option<NodeId>,

    // Leader-only volatile state
    next_index: HashMap<NodeId, u64>,
    match_index: HashMap<NodeId, u64>,

    // Election state
    votes_received: usize,
    election_ticks_remaining: u64,
    heartbeat_ticks_remaining: u64,

    // Output
    committed: Vec<CommittedEntry>,
}

const HEARTBEAT_INTERVAL: u64 = 5;
const ELECTION_TIMEOUT_MIN: u64 = 15;
const ELECTION_TIMEOUT_MAX: u64 = 30;

impl RaftNode {
    pub fn new(id: NodeId, cluster_size: usize) -> Self {
        let peer_ids: Vec<NodeId> = (0..cluster_size as u64)
            .map(NodeId)
            .filter(|&pid| pid != id)
            .collect();

        let election_timeout = rand::thread_rng()
            .gen_range(ELECTION_TIMEOUT_MIN..=ELECTION_TIMEOUT_MAX);

        Self {
            id,
            cluster_size,
            peer_ids,
            leader_id_atomic: AtomicU64::new(u64::MAX),
            state: Arc::new(Mutex::new(RaftState {
                current_term: 0,
                voted_for: None,
                log: Vec::new(),
                commit_index: 0,
                role: Role::Follower,
                leader_id: None,
                next_index: HashMap::new(),
                match_index: HashMap::new(),
                votes_received: 0,
                election_ticks_remaining: election_timeout,
                heartbeat_ticks_remaining: 0,
                committed: Vec::new(),
            })),
        }
    }

    /// Create a node pre-configured as leader (for testing).
    pub fn new_as_leader(id: NodeId, cluster_size: usize) -> Self {
        let node = Self::new(id, cluster_size);
        // We'll force leader state after creation
        let rt = tokio::runtime::Handle::try_current();
        if rt.is_ok() {
            // Can't block_on inside a runtime, so we return and let
            // the caller set state via force_leader()
        }
        node
    }

    pub async fn force_leader(&self) {
        let mut state = self.state.lock().await;
        state.role = Role::Leader;
        state.current_term = 1;
        state.leader_id = Some(self.id);
        self.leader_id_atomic.store(self.id.0, Ordering::Relaxed);

        // Initialise next_index and match_index for all peers
        let log_len = state.log.len() as u64 + 1;
        for &peer in &self.peer_ids {
            state.next_index.insert(peer, log_len);
            state.match_index.insert(peer, 0);
        }
    }

    fn majority(&self) -> usize {
        self.cluster_size / 2 + 1
    }

    fn msg_to(&self, to: NodeId, payload: ProtocolMessage) -> NetworkMessage {
        NetworkMessage {
            from: self.id,
            to,
            payload,
        }
    }

    fn reset_election_timeout(state: &mut RaftState) {
        state.election_ticks_remaining = rand::thread_rng()
            .gen_range(ELECTION_TIMEOUT_MIN..=ELECTION_TIMEOUT_MAX);
    }

    /// Build AppendEntries for a specific follower.
    fn build_append_entries(&self, state: &RaftState, peer: NodeId) -> NetworkMessage {
        let next = *state.next_index.get(&peer).unwrap_or(&1);
        let prev_log_index = next.saturating_sub(1);
        let prev_log_term = if prev_log_index > 0 {
            state.log.get((prev_log_index - 1) as usize)
                .map(|e| e.term)
                .unwrap_or(0)
        } else {
            0
        };

        let entries: Vec<LogEntry> = state.log
            .iter()
            .skip((next - 1) as usize)
            .cloned()
            .collect();

        self.msg_to(peer, ProtocolMessage::Raft(RaftMessage::AppendEntries {
            term: state.current_term,
            leader_id: self.id,
            prev_log_index,
            prev_log_term,
            entries,
            leader_commit: state.commit_index,
        }))
    }
}

#[async_trait]
impl ConsensusNode for RaftNode {
    fn id(&self) -> NodeId {
        self.id
    }

    fn cluster_size(&self) -> usize {
        self.cluster_size
    }

    fn fault_tolerance(&self) -> usize {
        (self.cluster_size - 1) / 2
    }

    fn peers(&self) -> Vec<NodeId> {
        self.peer_ids.clone()
    }

    async fn propose(&self, payload: Payload) -> (ProposeResult, Vec<NetworkMessage>) {
        let mut state = self.state.lock().await;

        if state.role != Role::Leader {
            return (ProposeResult::NotLeader(state.leader_id), vec![]);
        }

        let index = state.log.len() as u64 + 1;
        let entry = LogEntry {
            term: state.current_term,
            index,
            payload,
        };

        debug!(node = %self.id, index, term = state.current_term, "leader appending entry");
        state.log.push(entry);

        // Send AppendEntries to all followers
        let outgoing: Vec<NetworkMessage> = self.peer_ids
            .iter()
            .map(|&peer| self.build_append_entries(&state, peer))
            .collect();

        (ProposeResult::Accepted, outgoing)
    }

    async fn handle_message(&self, msg: NetworkMessage) -> Vec<NetworkMessage> {
        let raft_msg = match msg.payload {
            ProtocolMessage::Raft(m) => m,
            _ => {
                warn!(node = %self.id, "received non-Raft message");
                return vec![];
            }
        };

        let mut state = self.state.lock().await;

        match raft_msg {
            RaftMessage::RequestVote {
                term,
                candidate_id,
                last_log_index,
                last_log_term,
            } => {
                if term > state.current_term {
                    state.current_term = term;
                    state.role = Role::Follower;
                    state.voted_for = None;
                    state.leader_id = None;
                }

                let vote_granted = if term < state.current_term {
                    false
                } else if state.voted_for.is_some() && state.voted_for != Some(candidate_id) {
                    false
                } else {
                    let our_last_term = state.log.last().map(|e| e.term).unwrap_or(0);
                    let our_last_index = state.log.len() as u64;

                    if last_log_term > our_last_term {
                        true
                    } else if last_log_term == our_last_term {
                        last_log_index >= our_last_index
                    } else {
                        false
                    }
                };

                if vote_granted {
                    state.voted_for = Some(candidate_id);
                    Self::reset_election_timeout(&mut state);
                    debug!(node = %self.id, candidate = %candidate_id, term, "granted vote");
                }

                vec![self.msg_to(msg.from, ProtocolMessage::Raft(
                    RaftMessage::RequestVoteResponse {
                        term: state.current_term,
                        vote_granted,
                    },
                ))]
            }

            RaftMessage::RequestVoteResponse { term, vote_granted } => {
                if term > state.current_term {
                    state.current_term = term;
                    state.role = Role::Follower;
                    return vec![];
                }

                if vote_granted && state.role == Role::Candidate {
                    state.votes_received += 1;
                    debug!(
                        node = %self.id, from = %msg.from,
                        votes = state.votes_received, needed = self.majority(),
                        "received vote"
                    );

                    if state.votes_received >= self.majority() {
                        // Won the election
                        state.role = Role::Leader;
                        state.leader_id = Some(self.id);
                        self.leader_id_atomic.store(self.id.0, Ordering::Relaxed);

                        let log_len = state.log.len() as u64 + 1;
                        for &peer in &self.peer_ids {
                            state.next_index.insert(peer, log_len);
                            state.match_index.insert(peer, 0);
                        }

                        debug!(node = %self.id, term = state.current_term, "won election, now leader");

                        // Send initial empty AppendEntries (heartbeat) to assert leadership
                        return self.peer_ids.iter()
                            .map(|&peer| self.build_append_entries(&state, peer))
                            .collect();
                    }
                }
                vec![]
            }

            RaftMessage::AppendEntries {
                term,
                leader_id,
                prev_log_index,
                prev_log_term,
                entries,
                leader_commit,
            } => {
                if term < state.current_term {
                    return vec![self.msg_to(msg.from, ProtocolMessage::Raft(
                        RaftMessage::AppendEntriesResponse {
                            term: state.current_term,
                            success: false,
                            match_index: 0,
                        },
                    ))];
                }

                // Valid leader — reset election timeout
                state.current_term = term;
                state.role = Role::Follower;
                state.leader_id = Some(leader_id);
                self.leader_id_atomic.store(leader_id.0, Ordering::Relaxed);
                Self::reset_election_timeout(&mut state);

                // Check prev_log consistency
                if prev_log_index > 0 {
                    let prev_entry = state.log.get((prev_log_index - 1) as usize);
                    match prev_entry {
                        None => {
                            return vec![self.msg_to(msg.from, ProtocolMessage::Raft(
                                RaftMessage::AppendEntriesResponse {
                                    term: state.current_term,
                                    success: false,
                                    match_index: state.log.len() as u64,
                                },
                            ))];
                        }
                        Some(entry) if entry.term != prev_log_term => {
                            // Conflict — truncate from here
                            state.log.truncate((prev_log_index - 1) as usize);
                            return vec![self.msg_to(msg.from, ProtocolMessage::Raft(
                                RaftMessage::AppendEntriesResponse {
                                    term: state.current_term,
                                    success: false,
                                    match_index: state.log.len() as u64,
                                },
                            ))];
                        }
                        _ => {}
                    }
                }

                // Append new entries
                for entry in &entries {
                    let idx = (entry.index - 1) as usize;
                    if idx < state.log.len() {
                        if state.log[idx].term != entry.term {
                            state.log.truncate(idx);
                            state.log.push(entry.clone());
                        }
                    } else {
                        state.log.push(entry.clone());
                    }
                }

                let match_idx = state.log.len() as u64;

                // Advance commit index
                if leader_commit > state.commit_index {
                    let new_commit = leader_commit.min(state.log.len() as u64);
                    let old_commit = state.commit_index;
                    state.commit_index = new_commit;

                    let new_entries: Vec<CommittedEntry> = state.log
                        [(old_commit as usize)..(new_commit as usize)]
                        .iter()
                        .map(|e| CommittedEntry {
                            sequence: e.index,
                            payload: e.payload.clone(),
                        })
                        .collect();
                    state.committed.extend(new_entries);
                }

                vec![self.msg_to(msg.from, ProtocolMessage::Raft(
                    RaftMessage::AppendEntriesResponse {
                        term: state.current_term,
                        success: true,
                        match_index: match_idx,
                    },
                ))]
            }

            RaftMessage::AppendEntriesResponse {
                term,
                success,
                match_index,
            } => {
                if term > state.current_term {
                    state.current_term = term;
                    state.role = Role::Follower;
                    return vec![];
                }

                if state.role != Role::Leader {
                    return vec![];
                }

                if success {
                    state.match_index.insert(msg.from, match_index);
                    state.next_index.insert(msg.from, match_index + 1);

                    // Try to advance commit_index
                    let log_len = state.log.len() as u64;
                    for n in ((state.commit_index + 1)..=log_len).rev() {
                        // Only commit entries from the current term (Raft safety)
                        if let Some(entry) = state.log.get((n - 1) as usize) {
                            if entry.term != state.current_term {
                                continue;
                            }
                        }

                        let replicated_on = state
                            .match_index
                            .values()
                            .filter(|&&mi| mi >= n)
                            .count()
                            + 1;

                        if replicated_on >= self.majority() {
                            let old = state.commit_index;
                            state.commit_index = n;

                            let new_entries: Vec<CommittedEntry> = state.log
                                [(old as usize)..(n as usize)]
                                .iter()
                                .map(|e| CommittedEntry {
                                    sequence: e.index,
                                    payload: e.payload.clone(),
                                })
                                .collect();
                            state.committed.extend(new_entries);

                            debug!(node = %self.id, commit = n, "advanced commit index");
                            break;
                        }
                    }
                } else {
                    // Decrement next_index and retry
                    let next = state.next_index.get(&msg.from).copied().unwrap_or(1);
                    if next > 1 {
                        state.next_index.insert(msg.from, next - 1);
                    }
                    // Send updated AppendEntries
                    return vec![self.build_append_entries(&state, msg.from)];
                }

                vec![]
            }
        }
    }

    async fn tick(&self) -> Vec<NetworkMessage> {
        let mut state = self.state.lock().await;

        match state.role {
            Role::Leader => {
                // Heartbeat
                if state.heartbeat_ticks_remaining == 0 {
                    state.heartbeat_ticks_remaining = HEARTBEAT_INTERVAL;
                    return self.peer_ids.iter()
                        .map(|&peer| self.build_append_entries(&state, peer))
                        .collect();
                }
                state.heartbeat_ticks_remaining -= 1;
                vec![]
            }
            Role::Follower | Role::Candidate => {
                if state.election_ticks_remaining == 0 {
                    // Start election
                    state.current_term += 1;
                    state.role = Role::Candidate;
                    state.voted_for = Some(self.id);
                    state.votes_received = 1; // vote for self
                    state.leader_id = None;
                    Self::reset_election_timeout(&mut state);

                    let last_log_index = state.log.len() as u64;
                    let last_log_term = state.log.last().map(|e| e.term).unwrap_or(0);

                    debug!(
                        node = %self.id, term = state.current_term,
                        "election timeout, starting election"
                    );

                    return self.peer_ids.iter()
                        .map(|&peer| self.msg_to(peer, ProtocolMessage::Raft(
                            RaftMessage::RequestVote {
                                term: state.current_term,
                                candidate_id: self.id,
                                last_log_index,
                                last_log_term,
                            },
                        )))
                        .collect();
                }
                state.election_ticks_remaining -= 1;
                vec![]
            }
        }
    }

    async fn poll_committed(&self) -> Vec<CommittedEntry> {
        let mut state = self.state.lock().await;
        std::mem::take(&mut state.committed)
    }

    fn leader(&self) -> Option<NodeId> {
        let id = self.leader_id_atomic.load(Ordering::Relaxed);
        if id == u64::MAX {
            None
        } else {
            Some(NodeId(id))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn leader_accepts_and_broadcasts() {
        let node = RaftNode::new(NodeId(0), 3);
        node.force_leader().await;

        let (result, outgoing) = node.propose(Payload::new(b"test")).await;
        assert_eq!(result, ProposeResult::Accepted);
        assert_eq!(outgoing.len(), 2); // 2 peers
    }

    #[tokio::test]
    async fn follower_rejects_proposal() {
        let node = RaftNode::new(NodeId(1), 3);
        let (result, outgoing) = node.propose(Payload::new(b"test")).await;
        assert!(matches!(result, ProposeResult::NotLeader(_)));
        assert!(outgoing.is_empty());
    }

    #[tokio::test]
    async fn fault_tolerance_is_correct() {
        let node = RaftNode::new(NodeId(0), 3);
        assert_eq!(node.fault_tolerance(), 1);

        let node = RaftNode::new(NodeId(0), 5);
        assert_eq!(node.fault_tolerance(), 2);
    }
}
