pub mod fault;

use std::collections::VecDeque;
use std::sync::Arc;

use crate::consensus::{CommittedEntry, ConsensusNode, ProposeResult};
use crate::network::NetworkMessage;
use crate::consensus::pbft::PbftNode;
use crate::consensus::raft::RaftNode;
use crate::types::{NodeId, Payload};

use fault::FaultConfig;

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
}

#[derive(Debug, Clone, Default)]
pub struct ClusterStats {
    pub messages_sent: u64,
    pub messages_delivered: u64,
    pub messages_dropped: u64,
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

            // Look up by index to avoid borrowing all of self
            let node_idx = self.nodes.iter().position(|n| n.id() == msg.to);
            if let Some(idx) = node_idx {
                self.stats.messages_delivered += 1;
                let outgoing = self.nodes[idx].handle_message(msg).await;
                new_messages.extend(outgoing);
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
