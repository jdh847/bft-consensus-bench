use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use super::Role;
use crate::consensus::{CommittedEntry, ConsensusNode, ProposeResult};
use crate::network::{LogEntry, NetworkMessage, ProtocolMessage, RaftMessage};
use crate::types::{NodeId, Payload};

pub struct RaftNode {
    id: NodeId,
    cluster_size: usize,
    state: Arc<Mutex<RaftState>>,
}

struct RaftState {
    // Persistent state
    current_term: u64,
    voted_for: Option<NodeId>,
    log: Vec<LogEntry>,

    // Volatile state
    commit_index: u64,
    last_applied: u64,
    role: Role,
    leader_id: Option<NodeId>,

    // Leader-only volatile state
    next_index: HashMap<NodeId, u64>,
    match_index: HashMap<NodeId, u64>,

    // Output
    committed: Vec<CommittedEntry>,
}

impl RaftNode {
    pub fn new(id: NodeId, cluster_size: usize) -> Self {
        Self {
            id,
            cluster_size,
            state: Arc::new(Mutex::new(RaftState {
                current_term: 0,
                voted_for: None,
                log: Vec::new(),
                commit_index: 0,
                last_applied: 0,
                role: Role::Follower,
                leader_id: None,
                next_index: HashMap::new(),
                match_index: HashMap::new(),
                committed: Vec::new(),
            })),
        }
    }

    fn majority(&self) -> usize {
        self.cluster_size / 2 + 1
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
        // Raft tolerates f failures in a 2f+1 cluster
        (self.cluster_size - 1) / 2
    }

    async fn propose(&self, payload: Payload) -> ProposeResult {
        let mut state = self.state.lock().await;

        if state.role != Role::Leader {
            return ProposeResult::NotLeader(state.leader_id);
        }

        let index = state.log.len() as u64 + 1;
        let entry = LogEntry {
            term: state.current_term,
            index,
            payload,
        };

        debug!(node = %self.id, index, term = state.current_term, "leader appending entry");
        state.log.push(entry);

        ProposeResult::Accepted
    }

    async fn handle_message(&self, msg: NetworkMessage) {
        let raft_msg = match msg.payload {
            ProtocolMessage::Raft(m) => m,
            _ => {
                warn!(node = %self.id, "received non-Raft message");
                return;
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
                // Step down if we see a higher term
                if term > state.current_term {
                    state.current_term = term;
                    state.role = Role::Follower;
                    state.voted_for = None;
                }

                let vote_granted = if term < state.current_term {
                    false
                } else if state.voted_for.is_some() && state.voted_for != Some(candidate_id) {
                    false
                } else {
                    // Check log is at least as up-to-date
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
                    debug!(node = %self.id, candidate = %candidate_id, term, "granted vote");
                }
            }

            RaftMessage::RequestVoteResponse { term, vote_granted } => {
                if term > state.current_term {
                    state.current_term = term;
                    state.role = Role::Follower;
                    return;
                }

                if vote_granted && state.role == Role::Candidate {
                    debug!(node = %self.id, from = %msg.from, "received vote");
                    // Vote counting will be handled by the election driver
                    // (not yet implemented — comes with the election timeout logic)
                }
            }

            RaftMessage::AppendEntries {
                term,
                leader_id,
                prev_log_index,
                prev_log_term: _,
                entries,
                leader_commit,
            } => {
                if term < state.current_term {
                    return;
                }

                if term >= state.current_term {
                    state.current_term = term;
                    state.role = Role::Follower;
                    state.leader_id = Some(leader_id);
                }

                // Append entries (simplified — no conflict resolution yet)
                for entry in entries {
                    let idx = entry.index as usize;
                    if idx > state.log.len() {
                        state.log.push(entry);
                    }
                }

                // Advance commit index
                if leader_commit > state.commit_index {
                    let new_commit = leader_commit.min(state.log.len() as u64);
                    let old_commit = state.commit_index;
                    state.commit_index = new_commit;

                    // Deliver newly committed entries
                    let new_entries: Vec<CommittedEntry> = state.log
                        [(old_commit as usize)..(new_commit as usize)]
                        .iter()
                        .map(|entry| CommittedEntry {
                            sequence: entry.index,
                            payload: entry.payload.clone(),
                        })
                        .collect();
                    state.committed.extend(new_entries);

                    debug!(
                        node = %self.id,
                        prev = prev_log_index,
                        commit = state.commit_index,
                        "updated commit index"
                    );
                }
            }

            RaftMessage::AppendEntriesResponse {
                term,
                success,
                match_index,
            } => {
                if term > state.current_term {
                    state.current_term = term;
                    state.role = Role::Follower;
                    return;
                }

                if state.role == Role::Leader && success {
                    state.match_index.insert(msg.from, match_index);
                    state.next_index.insert(msg.from, match_index + 1);

                    // Check if we can advance commit_index
                    // A log entry is committed once replicated on a majority
                    let log_len = state.log.len() as u64;
                    for n in ((state.commit_index + 1)..=log_len).rev() {
                        let replicated_on = state
                            .match_index
                            .values()
                            .filter(|&&mi| mi >= n)
                            .count()
                            + 1; // +1 for the leader itself

                        if replicated_on >= self.majority() {
                            let old = state.commit_index;
                            state.commit_index = n;

                            let new_entries: Vec<CommittedEntry> = state.log
                                [(old as usize)..(n as usize)]
                                .iter()
                                .map(|entry| CommittedEntry {
                                    sequence: entry.index,
                                    payload: entry.payload.clone(),
                                })
                                .collect();
                            state.committed.extend(new_entries);

                            debug!(node = %self.id, commit = n, "advanced commit index");
                            break;
                        }
                    }
                }
            }
        }
    }

    async fn poll_committed(&self) -> Vec<CommittedEntry> {
        let mut state = self.state.lock().await;
        std::mem::take(&mut state.committed)
    }

    fn leader(&self) -> Option<NodeId> {
        // Same issue as PBFT — can't read async state from sync fn.
        // TODO: use an AtomicU64 for leader_id
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn leader_accepts_proposal() {
        let node = RaftNode::new(NodeId(0), 3);
        // Force node to be leader
        {
            let mut state = node.state.lock().await;
            state.role = Role::Leader;
            state.current_term = 1;
        }

        let result = node.propose(Payload::new(b"test")).await;
        assert_eq!(result, ProposeResult::Accepted);
    }

    #[tokio::test]
    async fn follower_rejects_proposal() {
        let node = RaftNode::new(NodeId(1), 3);
        let result = node.propose(Payload::new(b"test")).await;
        assert!(matches!(result, ProposeResult::NotLeader(_)));
    }

    #[tokio::test]
    async fn fault_tolerance_is_correct() {
        // n=3, f=1
        let node = RaftNode::new(NodeId(0), 3);
        assert_eq!(node.fault_tolerance(), 1);

        // n=5, f=2
        let node = RaftNode::new(NodeId(0), 5);
        assert_eq!(node.fault_tolerance(), 2);
    }
}
