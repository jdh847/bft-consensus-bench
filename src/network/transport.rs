use std::collections::HashMap;
use std::sync::Arc;

use rand::Rng;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, trace};

use super::NetworkMessage;
use crate::types::NodeId;

/// Configuration for the simulated network.
#[derive(Debug, Clone)]
pub struct NetworkConfig {
    /// Per-node channel buffer size.
    pub channel_capacity: usize,
    /// Base latency in microseconds. Actual latency is base + jitter.
    pub base_latency_us: u64,
    /// Maximum random jitter added to base latency, in microseconds.
    pub jitter_us: u64,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            channel_capacity: 4096,
            base_latency_us: 100,
            jitter_us: 50,
        }
    }
}

/// A simulated in-process network for benchmarking consensus protocols.
///
/// Each node gets an mpsc channel. Messages are delivered with configurable
/// latency and jitter to approximate real network conditions without the
/// noise of actual TCP.
///
/// Fault injection (partitions, message drops, Byzantine behaviour) will
/// be layered on top of this in later iterations.
pub struct SimulatedNetwork {
    config: NetworkConfig,
    senders: HashMap<NodeId, mpsc::Sender<NetworkMessage>>,
    receivers: Arc<Mutex<HashMap<NodeId, mpsc::Receiver<NetworkMessage>>>>,
}

impl SimulatedNetwork {
    /// Create a new network connecting the given set of nodes.
    pub fn new(node_ids: &[NodeId], config: NetworkConfig) -> Self {
        let mut senders = HashMap::new();
        let mut receivers = HashMap::new();

        for &id in node_ids {
            let (tx, rx) = mpsc::channel(config.channel_capacity);
            senders.insert(id, tx);
            receivers.insert(id, rx);
        }

        Self {
            config,
            senders,
            receivers: Arc::new(Mutex::new(receivers)),
        }
    }

    /// Send a message from one node to another.
    ///
    /// Applies simulated latency before delivery. Returns an error
    /// if the target node's channel is full (back-pressure) or if
    /// the target doesn't exist.
    pub async fn send(&self, msg: NetworkMessage) -> Result<(), SendError> {
        let sender = self
            .senders
            .get(&msg.to)
            .ok_or(SendError::UnknownTarget(msg.to))?;

        // Simulate network delay
        if self.config.base_latency_us > 0 || self.config.jitter_us > 0 {
            let jitter = if self.config.jitter_us > 0 {
                rand::thread_rng().gen_range(0..self.config.jitter_us)
            } else {
                0
            };
            let delay = std::time::Duration::from_micros(self.config.base_latency_us + jitter);
            tokio::time::sleep(delay).await;
        }

        trace!(from = %msg.from, to = %msg.to, "delivering message");

        sender
            .try_send(msg)
            .map_err(|_| SendError::ChannelFull)?;

        Ok(())
    }

    /// Broadcast a message from one node to all other nodes.
    pub async fn broadcast(&self, from: NodeId, msg_factory: impl Fn(NodeId) -> NetworkMessage) {
        for &target in self.senders.keys() {
            if target != from {
                let msg = msg_factory(target);
                if let Err(e) = self.send(msg).await {
                    debug!(from = %from, to = %target, error = %e, "broadcast send failed");
                }
            }
        }
    }

    /// Take ownership of a node's receive channel. Called once during
    /// node initialisation. Panics if the receiver was already taken.
    pub async fn take_receiver(&self, id: NodeId) -> mpsc::Receiver<NetworkMessage> {
        self.receivers
            .lock()
            .await
            .remove(&id)
            .unwrap_or_else(|| panic!("receiver for {id} already taken or does not exist"))
    }

    pub fn node_count(&self) -> usize {
        self.senders.len()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SendError {
    #[error("unknown target node {0}")]
    UnknownTarget(NodeId),
    #[error("target channel full (back-pressure)")]
    ChannelFull,
}
