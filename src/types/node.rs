use serde::{Deserialize, Serialize};
use std::fmt;

/// Unique identifier for a node in the consensus cluster.
///
/// Wraps a `u64` rather than a UUID to keep message serialisation
/// compact — these get included in every protocol message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct NodeId(pub u64);

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "node-{}", self.0)
    }
}

impl From<u64> for NodeId {
    fn from(id: u64) -> Self {
        Self(id)
    }
}
