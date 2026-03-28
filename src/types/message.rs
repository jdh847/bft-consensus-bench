use serde::{Deserialize, Serialize};
use sha2::{Digest as Sha2Digest, Sha256};

pub type ViewNumber = u64;
pub type SequenceNumber = u64;

/// Opaque client payload. The consensus layer doesn't interpret this —
/// it just needs to agree on the ordering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Payload(pub Vec<u8>);

impl Payload {
    pub fn new(data: impl Into<Vec<u8>>) -> Self {
        Self(data.into())
    }

    pub fn digest(&self) -> Digest {
        let mut hasher = Sha256::new();
        hasher.update(&self.0);
        let result = hasher.finalize();
        Digest(result.into())
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// SHA-256 digest of a payload or message. Used for deduplication
/// and integrity checks without copying full payloads around.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Digest(pub [u8; 32]);

impl Digest {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}
