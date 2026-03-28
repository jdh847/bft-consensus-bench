mod state;

pub use state::RaftNode;

/// Raft node role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Follower,
    Candidate,
    Leader,
}
