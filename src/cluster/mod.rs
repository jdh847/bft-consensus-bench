pub mod fault;
pub mod invariant;

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use rand::rngs::StdRng;
use rand::SeedableRng;

use crate::consensus::hotstuff::HotStuffNode;
use crate::consensus::pbft::PbftNode;
use crate::consensus::raft::RaftNode;
use crate::consensus::{CommittedEntry, ConsensusNode, ProposeResult};
use crate::network::NetworkMessage;
use crate::types::{NodeId, Payload};

use fault::FaultConfig;
use invariant::{CommitLog, InvariantResult};

/// Drives a cluster of consensus nodes connected by a simulated network.
///
/// The orchestrator runs a synchronous step-based simulation: each step
/// delivers pending messages to their targets, collects outgoing messages,
/// and ticks all nodes for time-based events.
pub struct Cluster {
    nodes: Vec<Arc<dyn ConsensusNode>>,
    pending: VecDeque<NetworkMessage>,
    faults: FaultConfig,
    /// Stats for analysis
    pub stats: ClusterStats,
    /// Commit log for safety invariant checking
    commit_log: CommitLog,
}

#[derive(Debug, Clone, Default)]
pub struct ClusterStats {
    pub messages_sent: u64,
    pub messages_delivered: u64,
    pub messages_dropped: u64,
    pub messages_tampered: u64,
    pub total_steps: u64,
}

impl Cluster {
    pub fn new_pbft(n: usize) -> Self {
        let nodes: Vec<Arc<dyn ConsensusNode>> = (0..n)
            .map(|i| Arc::new(PbftNode::new(NodeId(i as u64), n)) as Arc<dyn ConsensusNode>)
            .collect();

        Self {
            nodes,
            pending: VecDeque::new(),
            faults: FaultConfig::default(),
            stats: ClusterStats::default(),
            commit_log: CommitLog::new(),
        }
    }

    pub fn new_hotstuff(n: usize) -> Self {
        let nodes: Vec<Arc<dyn ConsensusNode>> = (0..n)
            .map(|i| Arc::new(HotStuffNode::new(NodeId(i as u64), n)) as Arc<dyn ConsensusNode>)
            .collect();

        Self {
            nodes,
            pending: VecDeque::new(),
            faults: FaultConfig::default(),
            stats: ClusterStats::default(),
            commit_log: CommitLog::new(),
        }
    }

    /// Create a HotStuff cluster with deterministic seeded RNG.
    pub fn new_hotstuff_seeded(n: usize, seed: u64) -> Self {
        let mut master = StdRng::seed_from_u64(seed);
        let nodes: Vec<Arc<dyn ConsensusNode>> = (0..n)
            .map(|i| Arc::new(HotStuffNode::new(NodeId(i as u64), n)) as Arc<dyn ConsensusNode>)
            .collect();

        let fault_seed: u64 = rand::Rng::gen(&mut master);

        Self {
            nodes,
            pending: VecDeque::new(),
            faults: FaultConfig::with_seed(fault_seed),
            stats: ClusterStats::default(),
            commit_log: CommitLog::new(),
        }
    }

    pub fn new_raft(n: usize) -> Self {
        let nodes: Vec<Arc<dyn ConsensusNode>> = (0..n)
            .map(|i| Arc::new(RaftNode::new(NodeId(i as u64), n)) as Arc<dyn ConsensusNode>)
            .collect();

        Self {
            nodes,
            pending: VecDeque::new(),
            faults: FaultConfig::default(),
            stats: ClusterStats::default(),
            commit_log: CommitLog::new(),
        }
    }

    pub async fn new_raft_with_leader(n: usize) -> Self {
        let raft_nodes: Vec<Arc<RaftNode>> = (0..n)
            .map(|i| Arc::new(RaftNode::new(NodeId(i as u64), n)))
            .collect();

        raft_nodes[0].force_leader().await;

        let nodes: Vec<Arc<dyn ConsensusNode>> = raft_nodes
            .into_iter()
            .map(|n| n as Arc<dyn ConsensusNode>)
            .collect();

        Self {
            nodes,
            pending: VecDeque::new(),
            faults: FaultConfig::default(),
            stats: ClusterStats::default(),
            commit_log: CommitLog::new(),
        }
    }

    /// Create a PBFT cluster with deterministic seeded RNG.
    pub fn new_pbft_seeded(n: usize, seed: u64) -> Self {
        let mut master = StdRng::seed_from_u64(seed);
        let nodes: Vec<Arc<dyn ConsensusNode>> = (0..n)
            .map(|i| Arc::new(PbftNode::new(NodeId(i as u64), n)) as Arc<dyn ConsensusNode>)
            .collect();

        let fault_seed: u64 = rand::Rng::gen(&mut master);

        Self {
            nodes,
            pending: VecDeque::new(),
            faults: FaultConfig::with_seed(fault_seed),
            stats: ClusterStats::default(),
            commit_log: CommitLog::new(),
        }
    }

    /// Create a Raft cluster with deterministic seeded RNG.
    pub fn new_raft_seeded(n: usize, seed: u64) -> Self {
        let mut master = StdRng::seed_from_u64(seed);
        let nodes: Vec<Arc<dyn ConsensusNode>> = (0..n)
            .map(|i| {
                let child_rng = StdRng::seed_from_u64(rand::Rng::gen(&mut master));
                Arc::new(RaftNode::new_seeded(NodeId(i as u64), n, child_rng))
                    as Arc<dyn ConsensusNode>
            })
            .collect();

        let fault_seed: u64 = rand::Rng::gen(&mut master);

        Self {
            nodes,
            pending: VecDeque::new(),
            faults: FaultConfig::with_seed(fault_seed),
            stats: ClusterStats::default(),
            commit_log: CommitLog::new(),
        }
    }

    /// Create a Raft cluster with a pre-elected leader and deterministic seed.
    pub async fn new_raft_with_leader_seeded(n: usize, seed: u64) -> Self {
        let mut master = StdRng::seed_from_u64(seed);
        let raft_nodes: Vec<Arc<RaftNode>> = (0..n)
            .map(|i| {
                let child_rng = StdRng::seed_from_u64(rand::Rng::gen(&mut master));
                Arc::new(RaftNode::new_seeded(NodeId(i as u64), n, child_rng))
            })
            .collect();

        raft_nodes[0].force_leader().await;

        let nodes: Vec<Arc<dyn ConsensusNode>> = raft_nodes
            .into_iter()
            .map(|n| n as Arc<dyn ConsensusNode>)
            .collect();

        let fault_seed: u64 = rand::Rng::gen(&mut master);

        Self {
            nodes,
            pending: VecDeque::new(),
            faults: FaultConfig::with_seed(fault_seed),
            stats: ClusterStats::default(),
            commit_log: CommitLog::new(),
        }
    }

    /// Apply a fault configuration.
    pub fn set_faults(&mut self, faults: FaultConfig) {
        self.faults = faults;
    }

    /// Clear all fault injection.
    pub fn clear_faults(&mut self) {
        self.faults = FaultConfig::default();
    }

    pub async fn propose(&mut self, payload: Payload) -> ProposeResult {
        // Record for invariant checking
        self.commit_log.record_proposal(&payload);

        // Try each node until one accepts (handles leader changes).
        // Skip isolated nodes — a real client can't reach them.
        for i in 0..self.nodes.len() {
            if self.faults.is_isolated(self.nodes[i].id()) {
                continue;
            }
            let (result, outgoing) = self.nodes[i].propose(payload.clone()).await;

            match &result {
                ProposeResult::Accepted => {
                    self.enqueue(outgoing);
                    return ProposeResult::Accepted;
                }
                ProposeResult::NotLeader(Some(leader)) => {
                    // Try the suggested leader
                    let leader_idx = self.nodes.iter().position(|n| n.id() == *leader);
                    if let Some(idx) = leader_idx {
                        let (r, out) = self.nodes[idx].propose(payload).await;
                        self.enqueue(out);
                        return r;
                    }
                }
                _ => continue,
            }
        }

        ProposeResult::NotLeader(None)
    }

    pub async fn step(&mut self) {
        let to_deliver: Vec<NetworkMessage> = self.pending.drain(..).collect();
        let mut new_messages = Vec::new();

        for msg in to_deliver {
            self.stats.messages_sent += 1;

            if !self.faults.should_deliver(&msg) {
                self.stats.messages_dropped += 1;
                continue;
            }

            // Apply Byzantine tampering if the sender is Byzantine
            let is_byzantine = self.faults.is_byzantine(msg.from);
            let tampered = self.faults.tamper(msg);
            if tampered.is_empty() {
                self.stats.messages_dropped += 1;
                continue;
            }

            for tmsg in tampered {
                if is_byzantine {
                    self.stats.messages_tampered += 1;
                }

                let node_idx = self.nodes.iter().position(|n| n.id() == tmsg.to);
                if let Some(idx) = node_idx {
                    self.stats.messages_delivered += 1;
                    let outgoing = self.nodes[idx].handle_message(tmsg).await;
                    new_messages.extend(outgoing);
                }
            }
        }

        self.stats.total_steps += 1;
        self.enqueue(new_messages);
    }

    pub async fn tick(&mut self) {
        for node in &self.nodes {
            let outgoing = node.tick().await;
            for msg in outgoing {
                self.pending.push_back(msg);
            }
        }
    }

    pub async fn run_to_completion(&mut self, max_steps: usize) -> usize {
        let mut steps = 0;
        while !self.pending.is_empty() && steps < max_steps {
            self.step().await;
            steps += 1;
        }
        steps
    }

    pub async fn collect_committed(&self) -> Vec<(NodeId, Vec<CommittedEntry>)> {
        let mut results = Vec::new();
        for node in &self.nodes {
            let committed = node.poll_committed().await;
            if !committed.is_empty() {
                results.push((node.id(), committed));
            }
        }
        results
    }

    /// Drain committed entries from all nodes AND record them to the commit log.
    pub async fn collect_and_record_commits(&mut self) -> Vec<(NodeId, Vec<CommittedEntry>)> {
        let mut results = Vec::new();
        for node in &self.nodes {
            let committed = node.poll_committed().await;
            if !committed.is_empty() {
                self.commit_log.record_commits(node.id(), committed.clone());
                results.push((node.id(), committed));
            }
        }
        results
    }

    /// Check safety invariants against the accumulated commit log.
    /// Derives honest nodes from the fault config (non-Byzantine, non-isolated).
    pub fn check_invariants(&self, expected_count: usize) -> InvariantResult {
        let honest_nodes: HashSet<NodeId> = (0..self.nodes.len())
            .map(|i| NodeId(i as u64))
            .filter(|id| !self.faults.is_byzantine(*id) && !self.faults.is_isolated(*id))
            .collect();
        self.commit_log.check(expected_count, &honest_nodes)
    }

    pub async fn wait_for_leader(&mut self, max_ticks: usize) -> Option<NodeId> {
        for _ in 0..max_ticks {
            self.tick().await;
            self.run_to_completion(100).await;

            for node in &self.nodes {
                if node.is_leader() && !self.faults.is_isolated(node.id()) {
                    return Some(node.id());
                }
            }
        }
        None
    }

    pub fn size(&self) -> usize {
        self.nodes.len()
    }

    fn enqueue(&mut self, messages: Vec<NetworkMessage>) {
        for msg in messages {
            self.pending.push_back(msg);
        }
    }
}
