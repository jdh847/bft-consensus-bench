use std::collections::VecDeque;
use std::sync::Arc;

use crate::consensus::{CommittedEntry, ConsensusNode, ProposeResult};
use crate::network::NetworkMessage;
use crate::consensus::pbft::PbftNode;
use crate::consensus::raft::RaftNode;
use crate::types::{NodeId, Payload};

/// Drives a cluster of consensus nodes connected by a simulated network.
///
/// The orchestrator runs a synchronous step-based simulation: each step
/// delivers pending messages to their targets, collects outgoing messages,
/// and ticks all nodes for time-based events.
pub struct Cluster {
    nodes: Vec<Arc<dyn ConsensusNode>>,
    /// Messages waiting to be delivered.
    pending: VecDeque<NetworkMessage>,
}

impl Cluster {
    /// Create a PBFT cluster with `n` nodes.
    pub fn new_pbft(n: usize) -> Self {
        let nodes: Vec<Arc<dyn ConsensusNode>> = (0..n)
            .map(|i| Arc::new(PbftNode::new(NodeId(i as u64), n)) as Arc<dyn ConsensusNode>)
            .collect();

        Self {
            nodes,
            pending: VecDeque::new(),
        }
    }

    /// Create a Raft cluster with `n` nodes.
    /// If `bootstrap_leader` is Some, that node starts as leader.
    pub fn new_raft(n: usize) -> Self {
        let nodes: Vec<Arc<dyn ConsensusNode>> = (0..n)
            .map(|i| Arc::new(RaftNode::new(NodeId(i as u64), n)) as Arc<dyn ConsensusNode>)
            .collect();

        Self {
            nodes,
            pending: VecDeque::new(),
        }
    }

    /// Create a Raft cluster with node 0 pre-configured as leader.
    pub async fn new_raft_with_leader(n: usize) -> Self {
        let raft_nodes: Vec<Arc<RaftNode>> = (0..n)
            .map(|i| Arc::new(RaftNode::new(NodeId(i as u64), n)))
            .collect();

        // Bootstrap node 0 as leader
        raft_nodes[0].force_leader().await;

        let nodes: Vec<Arc<dyn ConsensusNode>> = raft_nodes
            .into_iter()
            .map(|n| n as Arc<dyn ConsensusNode>)
            .collect();

        Self {
            nodes,
            pending: VecDeque::new(),
        }
    }

    /// Propose a payload to the leader node (or first node for PBFT).
    pub async fn propose(&mut self, payload: Payload) -> ProposeResult {
        // Try node 0 first, follow redirects
        let (result, outgoing) = self.nodes[0].propose(payload.clone()).await;

        match &result {
            ProposeResult::Accepted => {
                self.enqueue(outgoing);
            }
            ProposeResult::NotLeader(Some(leader)) => {
                // Redirect to the actual leader
                if let Some(node) = self.find_node(*leader) {
                    let (r, out) = node.propose(payload).await;
                    self.enqueue(out);
                    return r;
                }
            }
            _ => {}
        }

        result
    }

    /// Run one step of the simulation:
    /// 1. Deliver all pending messages
    /// 2. Tick all nodes
    pub async fn step(&mut self) {
        // Deliver pending messages
        let to_deliver: Vec<NetworkMessage> = self.pending.drain(..).collect();
        let mut new_messages = Vec::new();

        for msg in to_deliver {
            if let Some(node) = self.find_node(msg.to) {
                let outgoing = node.handle_message(msg).await;
                new_messages.extend(outgoing);
            }
        }

        self.enqueue(new_messages);
    }

    /// Run one tick (time advancement) for all nodes.
    pub async fn tick(&mut self) {
        for node in &self.nodes {
            let outgoing = node.tick().await;
            for msg in outgoing {
                self.pending.push_back(msg);
            }
        }
    }

    /// Run steps until no more messages are pending or max_steps reached.
    pub async fn run_to_completion(&mut self, max_steps: usize) -> usize {
        let mut steps = 0;
        while !self.pending.is_empty() && steps < max_steps {
            self.step().await;
            steps += 1;
        }
        steps
    }

    /// Collect committed entries from all nodes.
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

    /// Run ticks until a leader is elected (Raft) or max ticks reached.
    pub async fn wait_for_leader(&mut self, max_ticks: usize) -> Option<NodeId> {
        for _ in 0..max_ticks {
            self.tick().await;
            self.run_to_completion(100).await;

            for node in &self.nodes {
                if node.is_leader() {
                    return Some(node.id());
                }
            }
        }
        None
    }

    /// Get the number of nodes.
    pub fn size(&self) -> usize {
        self.nodes.len()
    }

    fn find_node(&self, id: NodeId) -> Option<&Arc<dyn ConsensusNode>> {
        self.nodes.iter().find(|n| n.id() == id)
    }

    fn enqueue(&mut self, messages: Vec<NetworkMessage>) {
        for msg in messages {
            self.pending.push_back(msg);
        }
    }
}
