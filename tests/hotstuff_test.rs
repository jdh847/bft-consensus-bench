use bft_consensus_bench::cluster::fault::{ByzantineBehavior, FaultConfig, MessagePhase};
use bft_consensus_bench::cluster::invariant::CheckResult;
use bft_consensus_bench::cluster::Cluster;
use bft_consensus_bench::consensus::ProposeResult;
use bft_consensus_bench::types::{NodeId, Payload};

// ── Happy path ────────────────────────────────────────────────

#[tokio::test]
async fn hotstuff_4_nodes_commit_single_value() {
    let mut cluster = Cluster::new_hotstuff(4);

    let result = cluster.propose(Payload::new(b"hello hotstuff")).await;
    assert_eq!(result, ProposeResult::Accepted);

    let steps = cluster.run_to_completion(50).await;
    assert!(steps > 0, "expected at least one delivery step");

    let committed = cluster.collect_committed().await;
    assert!(
        !committed.is_empty(),
        "expected at least one node to commit"
    );

    for (node_id, entries) in &committed {
        assert_eq!(
            entries.len(),
            1,
            "node {node_id} should commit exactly 1 entry"
        );
        assert_eq!(entries[0].payload, Payload::new(b"hello hotstuff"));
    }
}

#[tokio::test]
async fn hotstuff_7_nodes_commit_multiple_values() {
    let mut cluster = Cluster::new_hotstuff(7);

    for i in 0..5 {
        let payload = Payload::new(format!("value-{i}").into_bytes());
        let result = cluster.propose(payload).await;
        assert_eq!(result, ProposeResult::Accepted);
        cluster.run_to_completion(50).await;
    }

    let committed = cluster.collect_committed().await;
    assert!(!committed.is_empty());

    let leader_committed = committed
        .iter()
        .find(|(id, _)| id.0 == 0)
        .map(|(_, entries)| entries);

    if let Some(entries) = leader_committed {
        assert_eq!(entries.len(), 5, "leader should commit all 5 values");
    }
}

// ── Fault injection ───────────────────────────────────────────

#[tokio::test]
async fn hotstuff_tolerates_one_isolated_node_in_4_node_cluster() {
    // n=4, f=1 — should still commit with 1 node isolated
    let mut cluster = Cluster::new_hotstuff(4);
    cluster.set_faults(FaultConfig::new().with_isolated(NodeId(3)));

    let result = cluster.propose(Payload::new(b"fault test")).await;
    assert_eq!(result, ProposeResult::Accepted);

    cluster.run_to_completion(50).await;

    let committed = cluster.collect_committed().await;
    assert!(
        !committed.is_empty(),
        "HotStuff should commit with 1 of 4 nodes isolated (f=1)"
    );

    let node3_committed = committed.iter().find(|(id, _)| id.0 == 3);
    assert!(node3_committed.is_none(), "isolated node should not commit");
}

#[tokio::test]
async fn hotstuff_fails_with_two_isolated_in_4_node_cluster() {
    // n=4, f=1 — with 2 nodes isolated, should NOT reach quorum
    let mut cluster = Cluster::new_hotstuff(4);
    cluster.set_faults(
        FaultConfig::new()
            .with_isolated(NodeId(2))
            .with_isolated(NodeId(3)),
    );

    let result = cluster.propose(Payload::new(b"should fail")).await;
    assert_eq!(result, ProposeResult::Accepted);

    cluster.run_to_completion(50).await;

    let committed = cluster.collect_committed().await;
    assert!(
        committed.is_empty(),
        "HotStuff should NOT commit with 2 of 4 nodes isolated"
    );
}

// ── View change / leader recovery ─────────────────────────────

#[tokio::test]
async fn hotstuff_view_change_after_leader_isolated() {
    // n=4: commit a value, then isolate leader (node 0),
    // tick until view change happens, then commit with new leader (node 1)
    let mut cluster = Cluster::new_hotstuff(4);

    // First: commit normally with the original leader
    let result = cluster.propose(Payload::new(b"before-crash")).await;
    assert_eq!(result, ProposeResult::Accepted);
    cluster.run_to_completion(50).await;

    let committed = cluster.collect_committed().await;
    assert!(!committed.is_empty(), "should commit before leader failure");

    // Now isolate the leader
    cluster.set_faults(FaultConfig::new().with_isolated(NodeId(0)));

    // Tick until view change completes — replicas will timeout,
    // send NewView, and node 1 becomes new leader
    for _ in 0..100 {
        cluster.tick().await;
        cluster.run_to_completion(50).await;
    }

    // Try to propose to the new leader
    let result = cluster.propose(Payload::new(b"after-view-change")).await;
    assert_eq!(
        result,
        ProposeResult::Accepted,
        "new leader should accept proposals"
    );

    cluster.run_to_completion(50).await;

    let committed = cluster.collect_committed().await;
    assert!(
        !committed.is_empty(),
        "should commit after view change with new leader"
    );
}

// ── Byzantine safety ──────────────────────────────────────────

#[tokio::test]
async fn hotstuff_4_nodes_1_byzantine_equivocator_still_agrees() {
    // n=4, f=1: node 3 equivocates. HotStuff should still reach agreement.
    let mut cluster = Cluster::new_hotstuff_seeded(4, 42);
    cluster.set_faults(
        FaultConfig::with_seed(42).with_byzantine(NodeId(3), vec![ByzantineBehavior::Equivocate]),
    );

    let result = cluster.propose(Payload::new(b"byz-test-1")).await;
    assert_eq!(result, ProposeResult::Accepted);
    cluster.run_to_completion(50).await;

    cluster.collect_and_record_commits().await;
    let invariants = cluster.check_invariants(1);

    assert_eq!(
        invariants.agreement,
        CheckResult::Pass,
        "HotStuff should maintain agreement with 1 Byzantine equivocator in n=4"
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
async fn hotstuff_7_nodes_2_byzantine_corrupt_digest_still_safe() {
    // n=7, f=2: nodes 5 and 6 corrupt digests.
    // HotStuff should still reach agreement among the 5 honest nodes.
    let mut cluster = Cluster::new_hotstuff_seeded(7, 99);
    cluster.set_faults(
        FaultConfig::with_seed(99)
            .with_byzantine(NodeId(5), vec![ByzantineBehavior::CorruptDigest])
            .with_byzantine(NodeId(6), vec![ByzantineBehavior::CorruptDigest]),
    );

    let result = cluster.propose(Payload::new(b"byz-test-2")).await;
    assert_eq!(result, ProposeResult::Accepted);
    cluster.run_to_completion(50).await;

    cluster.collect_and_record_commits().await;
    let invariants = cluster.check_invariants(1);

    assert_eq!(
        invariants.agreement,
        CheckResult::Pass,
        "HotStuff should maintain agreement with 2 digest-corrupting nodes in n=7"
    );
    assert_eq!(
        invariants.integrity,
        CheckResult::Pass,
        "No sequence should be committed twice"
    );
}

#[tokio::test]
async fn hotstuff_4_nodes_1_selective_dropper_still_commits() {
    // n=4, f=1: node 3 drops all HotStuff Vote messages.
    let mut cluster = Cluster::new_hotstuff_seeded(4, 77);
    cluster.set_faults(FaultConfig::with_seed(77).with_byzantine(
        NodeId(3),
        vec![ByzantineBehavior::SelectiveDrop {
            phases: vec![MessagePhase::HotStuffVote],
        }],
    ));

    let result = cluster.propose(Payload::new(b"byz-test-3")).await;
    assert_eq!(result, ProposeResult::Accepted);
    cluster.run_to_completion(50).await;

    cluster.collect_and_record_commits().await;
    let invariants = cluster.check_invariants(1);

    assert_eq!(
        invariants.agreement,
        CheckResult::Pass,
        "HotStuff should maintain agreement with 1 selective-dropping node in n=4"
    );
}

// ── Deterministic replay ──────────────────────────────────────

#[tokio::test]
async fn hotstuff_deterministic_replay_produces_identical_results() {
    let seed = 55555;

    let stats1 = {
        let mut cluster = Cluster::new_hotstuff_seeded(4, seed);
        cluster.set_faults(FaultConfig::with_seed(seed).with_drop_rate(0.1));
        cluster.propose(Payload::new(b"deterministic")).await;
        cluster.run_to_completion(50).await;
        cluster.stats.clone()
    };

    let stats2 = {
        let mut cluster = Cluster::new_hotstuff_seeded(4, seed);
        cluster.set_faults(FaultConfig::with_seed(seed).with_drop_rate(0.1));
        cluster.propose(Payload::new(b"deterministic")).await;
        cluster.run_to_completion(50).await;
        cluster.stats.clone()
    };

    assert_eq!(stats1.messages_sent, stats2.messages_sent);
    assert_eq!(stats1.messages_delivered, stats2.messages_delivered);
    assert_eq!(stats1.messages_dropped, stats2.messages_dropped);
    assert_eq!(stats1.total_steps, stats2.total_steps);
}

// ── Message complexity comparison ─────────────────────────────

#[tokio::test]
async fn hotstuff_message_count_lower_than_pbft() {
    // n=7: HotStuff O(n) should use fewer messages than PBFT O(n²)
    let hs_msgs = {
        let mut cluster = Cluster::new_hotstuff(7);
        cluster.propose(Payload::new(b"compare")).await;
        cluster.run_to_completion(50).await;
        cluster.stats.messages_sent
    };

    let pbft_msgs = {
        let mut cluster = Cluster::new_pbft(7);
        cluster.propose(Payload::new(b"compare")).await;
        cluster.run_to_completion(50).await;
        cluster.stats.messages_sent
    };

    assert!(
        hs_msgs < pbft_msgs,
        "HotStuff ({hs_msgs} msgs) should use fewer messages than PBFT ({pbft_msgs} msgs) at n=7"
    );
}

// ── Stats tracking ────────────────────────────────────────────

#[tokio::test]
async fn hotstuff_stats_track_dropped_messages() {
    let mut cluster = Cluster::new_hotstuff(4);
    cluster.set_faults(FaultConfig::new().with_isolated(NodeId(3)));

    cluster.propose(Payload::new(b"stats test")).await;
    cluster.run_to_completion(50).await;

    assert!(cluster.stats.messages_sent > 0);
    assert!(cluster.stats.messages_dropped > 0);
    assert!(cluster.stats.messages_delivered > 0);
    assert_eq!(
        cluster.stats.messages_sent,
        cluster.stats.messages_delivered + cluster.stats.messages_dropped
    );
}
