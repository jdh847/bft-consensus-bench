use std::collections::{HashMap, HashSet};

use crate::consensus::CommittedEntry;
use crate::types::{NodeId, Payload};

/// Accumulates committed entries across the simulation for invariant checking.
#[derive(Debug, Clone, Default)]
pub struct CommitLog {
    /// All payloads that were proposed (for validity checking).
    proposed: Vec<Payload>,
    /// Per-node list of committed entries in order.
    commits: HashMap<NodeId, Vec<CommittedEntry>>,
}

/// Result of checking safety invariants.
#[derive(Debug, Clone)]
pub struct InvariantResult {
    /// No two honest nodes commit different payloads for the same sequence.
    pub agreement: CheckResult,
    /// Every committed value was originally proposed.
    pub validity: CheckResult,
    /// No sequence is committed twice on the same node.
    pub integrity: CheckResult,
    /// All honest nodes committed the expected number of entries.
    pub termination: CheckResult,
    /// For every pair of honest nodes, committed entries appear in the same sequence order.
    pub total_order: CheckResult,
}

/// Outcome of a single invariant check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckResult {
    Pass,
    Fail(String),
}

impl CommitLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a payload that was proposed to the cluster.
    pub fn record_proposal(&mut self, payload: &Payload) {
        self.proposed.push(payload.clone());
    }

    /// Record committed entries from a specific node.
    pub fn record_commits(&mut self, node: NodeId, entries: Vec<CommittedEntry>) {
        self.commits.entry(node).or_default().extend(entries);
    }

    /// Check all safety invariants.
    /// `expected_count` is the number of proposals that should have been committed.
    /// `honest_nodes` is the set of non-Byzantine nodes.
    pub fn check(&self, expected_count: usize, honest_nodes: &HashSet<NodeId>) -> InvariantResult {
        InvariantResult {
            agreement: self.check_agreement(honest_nodes),
            validity: self.check_validity(honest_nodes),
            integrity: self.check_integrity(honest_nodes),
            termination: self.check_termination(expected_count, honest_nodes),
            total_order: self.check_total_order(honest_nodes),
        }
    }

    /// Agreement: no two honest nodes commit different payloads for the same sequence.
    fn check_agreement(&self, honest_nodes: &HashSet<NodeId>) -> CheckResult {
        // Build a map: sequence -> set of payloads committed by honest nodes
        let mut seq_payloads: HashMap<u64, Vec<(NodeId, Payload)>> = HashMap::new();

        for (&node, entries) in &self.commits {
            if !honest_nodes.contains(&node) {
                continue;
            }
            for entry in entries {
                seq_payloads
                    .entry(entry.sequence)
                    .or_default()
                    .push((node, entry.payload.clone()));
            }
        }

        for (seq, payloads) in &seq_payloads {
            if payloads.len() < 2 {
                continue;
            }
            let first = &payloads[0].1;
            for (node, payload) in &payloads[1..] {
                if payload != first {
                    return CheckResult::Fail(format!(
                        "Agreement violated at sequence {}: node {} committed {:?} but node {} committed {:?}",
                        seq, payloads[0].0, first.0, node, payload.0
                    ));
                }
            }
        }

        CheckResult::Pass
    }

    /// Validity: every committed value was originally proposed.
    fn check_validity(&self, honest_nodes: &HashSet<NodeId>) -> CheckResult {
        for (&node, entries) in &self.commits {
            if !honest_nodes.contains(&node) {
                continue;
            }
            for entry in entries {
                if !self.proposed.contains(&entry.payload) {
                    return CheckResult::Fail(format!(
                        "Validity violated: node {} committed {:?} which was never proposed",
                        node, entry.payload.0
                    ));
                }
            }
        }

        CheckResult::Pass
    }

    /// Integrity: no sequence is committed twice on the same node.
    fn check_integrity(&self, honest_nodes: &HashSet<NodeId>) -> CheckResult {
        for (&node, entries) in &self.commits {
            if !honest_nodes.contains(&node) {
                continue;
            }
            let mut seen: HashSet<u64> = HashSet::new();
            for entry in entries {
                if !seen.insert(entry.sequence) {
                    return CheckResult::Fail(format!(
                        "Integrity violated: node {} committed sequence {} more than once",
                        node, entry.sequence
                    ));
                }
            }
        }

        CheckResult::Pass
    }

    /// Termination: all honest nodes committed the expected number of entries.
    fn check_termination(
        &self,
        expected_count: usize,
        honest_nodes: &HashSet<NodeId>,
    ) -> CheckResult {
        for &node in honest_nodes {
            let count = self.commits.get(&node).map(|e| e.len()).unwrap_or(0);
            if count != expected_count {
                return CheckResult::Fail(format!(
                    "Termination violated: honest node {} committed {} entries, expected {}",
                    node, count, expected_count
                ));
            }
        }

        CheckResult::Pass
    }

    /// Total order: for every pair of honest nodes, committed entries appear in the same
    /// sequence order. This ensures a globally consistent ordering across all nodes.
    fn check_total_order(&self, honest_nodes: &HashSet<NodeId>) -> CheckResult {
        // Collect the sequence ordering from each honest node
        let orderings: Vec<(NodeId, Vec<u64>)> = honest_nodes
            .iter()
            .filter_map(|&node| {
                self.commits
                    .get(&node)
                    .map(|entries| (node, entries.iter().map(|e| e.sequence).collect()))
            })
            .collect();

        if orderings.len() < 2 {
            return CheckResult::Pass;
        }

        let (ref_node, ref_order) = &orderings[0];
        for (node, order) in &orderings[1..] {
            // Compare the common prefix: sequences that both nodes committed
            // must appear in the same relative order
            let min_len = ref_order.len().min(order.len());
            for i in 0..min_len {
                if ref_order[i] != order[i] {
                    return CheckResult::Fail(format!(
                        "Total order violated: node {} has sequence {} at position {} \
                         but node {} has sequence {} at that position",
                        ref_node, ref_order[i], i, node, order[i]
                    ));
                }
            }
        }

        CheckResult::Pass
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agreement_passes_when_all_nodes_agree() {
        let mut log = CommitLog::new();
        let payload = Payload::new(b"hello");
        log.record_proposal(&payload);

        log.record_commits(
            NodeId(0),
            vec![CommittedEntry {
                sequence: 1,
                payload: payload.clone(),
            }],
        );
        log.record_commits(
            NodeId(1),
            vec![CommittedEntry {
                sequence: 1,
                payload: payload.clone(),
            }],
        );

        let honest: HashSet<NodeId> = [NodeId(0), NodeId(1)].into();
        let result = log.check(1, &honest);
        assert_eq!(result.agreement, CheckResult::Pass);
    }

    #[test]
    fn agreement_fails_when_nodes_disagree() {
        let mut log = CommitLog::new();
        let p1 = Payload::new(b"hello");
        let p2 = Payload::new(b"world");
        log.record_proposal(&p1);
        log.record_proposal(&p2);

        log.record_commits(
            NodeId(0),
            vec![CommittedEntry {
                sequence: 1,
                payload: p1,
            }],
        );
        log.record_commits(
            NodeId(1),
            vec![CommittedEntry {
                sequence: 1,
                payload: p2,
            }],
        );

        let honest: HashSet<NodeId> = [NodeId(0), NodeId(1)].into();
        let result = log.check(1, &honest);
        assert!(matches!(result.agreement, CheckResult::Fail(_)));
    }

    #[test]
    fn total_order_passes_when_sequences_match() {
        let mut log = CommitLog::new();
        let p1 = Payload::new(b"first");
        let p2 = Payload::new(b"second");
        log.record_proposal(&p1);
        log.record_proposal(&p2);

        // Both nodes commit sequences 1, 2 in the same order
        log.record_commits(
            NodeId(0),
            vec![
                CommittedEntry {
                    sequence: 1,
                    payload: p1.clone(),
                },
                CommittedEntry {
                    sequence: 2,
                    payload: p2.clone(),
                },
            ],
        );
        log.record_commits(
            NodeId(1),
            vec![
                CommittedEntry {
                    sequence: 1,
                    payload: p1.clone(),
                },
                CommittedEntry {
                    sequence: 2,
                    payload: p2.clone(),
                },
            ],
        );

        let honest: HashSet<NodeId> = [NodeId(0), NodeId(1)].into();
        let result = log.check(2, &honest);
        assert_eq!(result.total_order, CheckResult::Pass);
    }

    #[test]
    fn total_order_fails_when_sequences_differ() {
        let mut log = CommitLog::new();
        let p1 = Payload::new(b"first");
        let p2 = Payload::new(b"second");
        log.record_proposal(&p1);
        log.record_proposal(&p2);

        // Node 0 commits [1, 2], node 1 commits [2, 1] — different order
        log.record_commits(
            NodeId(0),
            vec![
                CommittedEntry {
                    sequence: 1,
                    payload: p1.clone(),
                },
                CommittedEntry {
                    sequence: 2,
                    payload: p2.clone(),
                },
            ],
        );
        log.record_commits(
            NodeId(1),
            vec![
                CommittedEntry {
                    sequence: 2,
                    payload: p2.clone(),
                },
                CommittedEntry {
                    sequence: 1,
                    payload: p1.clone(),
                },
            ],
        );

        let honest: HashSet<NodeId> = [NodeId(0), NodeId(1)].into();
        let result = log.check(2, &honest);
        assert!(matches!(result.total_order, CheckResult::Fail(_)));
    }

    #[test]
    fn integrity_detects_duplicate_sequence() {
        let mut log = CommitLog::new();
        let payload = Payload::new(b"data");
        log.record_proposal(&payload);

        // Same sequence committed twice on one node
        log.record_commits(
            NodeId(0),
            vec![
                CommittedEntry {
                    sequence: 1,
                    payload: payload.clone(),
                },
                CommittedEntry {
                    sequence: 1,
                    payload: payload.clone(),
                },
            ],
        );

        let honest: HashSet<NodeId> = [NodeId(0)].into();
        let result = log.check(2, &honest);
        assert!(matches!(result.integrity, CheckResult::Fail(_)));
    }

    #[test]
    fn termination_fails_when_node_under_commits() {
        let mut log = CommitLog::new();
        let p1 = Payload::new(b"a");
        let p2 = Payload::new(b"b");
        log.record_proposal(&p1);
        log.record_proposal(&p2);

        // Node 0 commits both, node 1 only commits one
        log.record_commits(
            NodeId(0),
            vec![
                CommittedEntry {
                    sequence: 1,
                    payload: p1.clone(),
                },
                CommittedEntry {
                    sequence: 2,
                    payload: p2.clone(),
                },
            ],
        );
        log.record_commits(
            NodeId(1),
            vec![CommittedEntry {
                sequence: 1,
                payload: p1.clone(),
            }],
        );

        let honest: HashSet<NodeId> = [NodeId(0), NodeId(1)].into();
        let result = log.check(2, &honest);
        assert!(matches!(result.termination, CheckResult::Fail(_)));
    }

    #[test]
    fn empty_log_passes_all_checks() {
        let log = CommitLog::new();
        let honest: HashSet<NodeId> = [NodeId(0), NodeId(1), NodeId(2)].into();
        let result = log.check(0, &honest);
        assert_eq!(result.agreement, CheckResult::Pass);
        assert_eq!(result.validity, CheckResult::Pass);
        assert_eq!(result.integrity, CheckResult::Pass);
        assert_eq!(result.termination, CheckResult::Pass);
        assert_eq!(result.total_order, CheckResult::Pass);
    }

    #[test]
    fn validity_fails_for_uncommitted_payload() {
        let mut log = CommitLog::new();
        let proposed = Payload::new(b"proposed");
        let rogue = Payload::new(b"rogue");
        log.record_proposal(&proposed);

        log.record_commits(
            NodeId(0),
            vec![CommittedEntry {
                sequence: 1,
                payload: rogue,
            }],
        );

        let honest: HashSet<NodeId> = [NodeId(0)].into();
        let result = log.check(1, &honest);
        assert!(matches!(result.validity, CheckResult::Fail(_)));
    }
}
