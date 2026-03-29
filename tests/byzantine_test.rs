use bft_consensus_bench::cluster::fault::{ByzantineBehavior, FaultConfig, MessagePhase};
use bft_consensus_bench::cluster::invariant::CheckResult;
use bft_consensus_bench::cluster::Cluster;
use bft_consensus_bench::types::{NodeId, Payload};

// ── PBFT Byzantine safety ───────────────────────────────────────

#[tokio::test]
async fn pbft_4_nodes_1_byzantine_equivocator_still_agrees() {
    // n=4, f=1: node 3 equivocates (sends different payloads to odd/even targets).
    // PBFT should still reach agreement among the 3 honest nodes.
    let mut cluster = Cluster::new_pbft_seeded(4, 42);
    cluster.set_faults(
        FaultConfig::with_seed(42).with_byzantine(NodeId(3), vec![ByzantineBehavior::Equivocate]),
    );

    let result = cluster.propose(Payload::new(b"byz-test-1")).await;
    assert_eq!(
        result,
        bft_consensus_bench::consensus::ProposeResult::Accepted
    );
    cluster.run_to_completion(50).await;

    cluster.collect_and_record_commits().await;
    let invariants = cluster.check_invariants(1);

    assert_eq!(
        invariants.agreement,
        CheckResult::Pass,
        "PBFT should maintain agreement with 1 Byzantine equivocator in n=4"
    );
    assert_eq!(
        invariants.validity,
        CheckResult::Pass,
        "All committed values should have been proposed"
    );
    assert_eq!(
        invariants.integrity,
        CheckResult::Pass,
        "No sequence should be committed twice"
    );
}

#[tokio::test]
async fn pbft_7_nodes_2_byzantine_corrupt_digest_still_safe() {
    // n=7, f=2: nodes 5 and 6 corrupt digests.
    // PBFT should still reach agreement among the 5 honest nodes.
    let mut cluster = Cluster::new_pbft_seeded(7, 99);
    cluster.set_faults(
        FaultConfig::with_seed(99)
            .with_byzantine(NodeId(5), vec![ByzantineBehavior::CorruptDigest])
            .with_byzantine(NodeId(6), vec![ByzantineBehavior::CorruptDigest]),
    );

    let result = cluster.propose(Payload::new(b"byz-test-2")).await;
    assert_eq!(
        result,
        bft_consensus_bench::consensus::ProposeResult::Accepted
    );
    cluster.run_to_completion(50).await;

    cluster.collect_and_record_commits().await;
    let invariants = cluster.check_invariants(1);

    assert_eq!(
        invariants.agreement,
        CheckResult::Pass,
        "PBFT should maintain agreement with 2 digest-corrupting nodes in n=7"
    );
    assert_eq!(
        invariants.integrity,
        CheckResult::Pass,
        "No sequence should be committed twice"
    );
}

#[tokio::test]
async fn pbft_4_nodes_1_selective_dropper_still_commits() {
    // n=4, f=1: node 3 drops all Commit messages.
    // PBFT should still reach agreement — Byzantine node's commits aren't needed for quorum.
    let mut cluster = Cluster::new_pbft_seeded(4, 77);
    cluster.set_faults(FaultConfig::with_seed(77).with_byzantine(
        NodeId(3),
        vec![ByzantineBehavior::SelectiveDrop {
            phases: vec![MessagePhase::PbftCommit],
        }],
    ));

    let result = cluster.propose(Payload::new(b"byz-test-3")).await;
    assert_eq!(
        result,
        bft_consensus_bench::consensus::ProposeResult::Accepted
    );
    cluster.run_to_completion(50).await;

    cluster.collect_and_record_commits().await;
    let invariants = cluster.check_invariants(1);

    assert_eq!(
        invariants.agreement,
        CheckResult::Pass,
        "PBFT should maintain agreement with 1 selective-dropping node in n=4"
    );
}

// ── Raft Byzantine failure — THE KEY RESULT ─────────────────────

#[tokio::test]
async fn raft_byzantine_leader_equivocation_violates_agreement() {
    // n=5, leader (node 0) equivocates: sends payload A to even followers,
    // payload B (mutated) to odd followers.
    // Raft has NO Byzantine protection, so followers commit different values.
    // The invariant checker catches the agreement violation.
    let mut cluster = Cluster::new_raft_with_leader_seeded(5, 123).await;
    cluster.set_faults(
        FaultConfig::with_seed(123).with_byzantine(NodeId(0), vec![ByzantineBehavior::Equivocate]),
    );

    // Propose — the leader will equivocate on the AppendEntries it sends
    let result = cluster.propose(Payload::new(b"raft-equivocate")).await;
    assert_eq!(
        result,
        bft_consensus_bench::consensus::ProposeResult::Accepted
    );

    // Run enough steps for followers to replicate and commit
    for _ in 0..30 {
        cluster.step().await;
        cluster.tick().await;
    }
    cluster.run_to_completion(100).await;

    cluster.collect_and_record_commits().await;

    // Check invariants — agreement SHOULD be violated
    let invariants = cluster.check_invariants(1);

    assert!(
        matches!(invariants.agreement, CheckResult::Fail(_)),
        "Raft SHOULD violate agreement when leader equivocates — \
         this demonstrates Raft is not Byzantine fault tolerant. \
         Got: {:?}",
        invariants.agreement
    );
}

// ── Deterministic replay ────────────────────────────────────────

#[tokio::test]
async fn deterministic_replay_produces_identical_results() {
    // Run the same seeded simulation twice and verify identical message counts.
    let seed = 55555;

    let stats1 = {
        let mut cluster = Cluster::new_pbft_seeded(4, seed);
        cluster.set_faults(FaultConfig::with_seed(seed).with_drop_rate(0.1));
        cluster.propose(Payload::new(b"deterministic")).await;
        cluster.run_to_completion(50).await;
        cluster.stats.clone()
    };

    let stats2 = {
        let mut cluster = Cluster::new_pbft_seeded(4, seed);
        cluster.set_faults(FaultConfig::with_seed(seed).with_drop_rate(0.1));
        cluster.propose(Payload::new(b"deterministic")).await;
        cluster.run_to_completion(50).await;
        cluster.stats.clone()
    };

    assert_eq!(
        stats1.messages_sent, stats2.messages_sent,
        "Same seed should produce identical messages_sent"
    );
    assert_eq!(
        stats1.messages_delivered, stats2.messages_delivered,
        "Same seed should produce identical messages_delivered"
    );
    assert_eq!(
        stats1.messages_dropped, stats2.messages_dropped,
        "Same seed should produce identical messages_dropped"
    );
    assert_eq!(
        stats1.total_steps, stats2.total_steps,
        "Same seed should produce identical total_steps"
    );
}
