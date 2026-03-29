use bft_consensus_bench::cluster::fault::FaultConfig;
use bft_consensus_bench::cluster::Cluster;
use bft_consensus_bench::consensus::ProposeResult;
use bft_consensus_bench::types::{NodeId, Payload};

// ── Happy path ────────────────────────────────────────────────

#[tokio::test]
async fn pbft_4_nodes_commit_single_value() {
    let mut cluster = Cluster::new_pbft(4);

    let result = cluster.propose(Payload::new(b"hello pbft")).await;
    assert_eq!(result, ProposeResult::Accepted);

    let steps = cluster.run_to_completion(20).await;
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
        assert_eq!(entries[0].payload, Payload::new(b"hello pbft"));
    }
}

#[tokio::test]
async fn pbft_7_nodes_commit_multiple_values() {
    let mut cluster = Cluster::new_pbft(7);

    for i in 0..5 {
        let payload = Payload::new(format!("value-{i}").into_bytes());
        let result = cluster.propose(payload).await;
        assert_eq!(result, ProposeResult::Accepted);
        cluster.run_to_completion(20).await;
    }

    let committed = cluster.collect_committed().await;
    assert!(!committed.is_empty());

    let primary_committed = committed
        .iter()
        .find(|(id, _)| id.0 == 0)
        .map(|(_, entries)| entries);

    if let Some(entries) = primary_committed {
        assert_eq!(entries.len(), 5, "primary should commit all 5 values");
    }
}

#[tokio::test]
async fn raft_3_nodes_with_bootstrapped_leader() {
    let mut cluster = Cluster::new_raft_with_leader(3).await;

    let result = cluster.propose(Payload::new(b"hello raft")).await;
    assert_eq!(result, ProposeResult::Accepted);

    cluster.run_to_completion(20).await;

    let committed = cluster.collect_committed().await;
    assert!(
        !committed.is_empty(),
        "expected at least one node to commit"
    );
}

#[tokio::test]
async fn raft_leader_election_from_scratch() {
    let mut cluster = Cluster::new_raft(3);

    let leader = cluster.wait_for_leader(200).await;
    assert!(
        leader.is_some(),
        "a leader should be elected within 200 ticks"
    );

    let result = cluster.propose(Payload::new(b"after election")).await;
    assert_eq!(result, ProposeResult::Accepted);

    cluster.run_to_completion(20).await;

    let committed = cluster.collect_committed().await;
    assert!(!committed.is_empty(), "should commit after leader election");
}

// ── Fault injection ───────────────────────────────────────────

#[tokio::test]
async fn pbft_tolerates_one_isolated_node_in_4_node_cluster() {
    // n=4, f=1 — should still commit with 1 node isolated
    let mut cluster = Cluster::new_pbft(4);
    cluster.set_faults(FaultConfig::new().with_isolated(NodeId(3)));

    let result = cluster.propose(Payload::new(b"fault test")).await;
    assert_eq!(result, ProposeResult::Accepted);

    cluster.run_to_completion(30).await;

    let committed = cluster.collect_committed().await;
    assert!(
        !committed.is_empty(),
        "PBFT should commit with 1 of 4 nodes isolated (f=1)"
    );

    // Node 3 should NOT have committed (it's isolated)
    let node3_committed = committed.iter().find(|(id, _)| id.0 == 3);
    assert!(node3_committed.is_none(), "isolated node should not commit");
}

#[tokio::test]
async fn pbft_fails_with_two_isolated_in_4_node_cluster() {
    // n=4, f=1 — with 2 nodes isolated, should NOT reach quorum
    let mut cluster = Cluster::new_pbft(4);
    cluster.set_faults(
        FaultConfig::new()
            .with_isolated(NodeId(2))
            .with_isolated(NodeId(3)),
    );

    let result = cluster.propose(Payload::new(b"should fail")).await;
    assert_eq!(result, ProposeResult::Accepted);

    cluster.run_to_completion(30).await;

    let committed = cluster.collect_committed().await;
    assert!(
        committed.is_empty(),
        "PBFT should NOT commit with 2 of 4 nodes isolated"
    );
}

#[tokio::test]
async fn raft_survives_minority_partition() {
    // n=5, f=2 — isolate 2 nodes, majority (3) should still commit
    let mut cluster = Cluster::new_raft_with_leader(5).await;
    cluster.set_faults(
        FaultConfig::new()
            .with_isolated(NodeId(3))
            .with_isolated(NodeId(4)),
    );

    let result = cluster.propose(Payload::new(b"partition test")).await;
    assert_eq!(result, ProposeResult::Accepted);

    cluster.run_to_completion(30).await;

    let committed = cluster.collect_committed().await;
    assert!(
        !committed.is_empty(),
        "Raft should commit with majority (3/5) available"
    );
}

#[tokio::test]
async fn cluster_stats_track_dropped_messages() {
    let mut cluster = Cluster::new_pbft(4);
    cluster.set_faults(FaultConfig::new().with_isolated(NodeId(3)));

    cluster.propose(Payload::new(b"stats test")).await;
    cluster.run_to_completion(20).await;

    assert!(cluster.stats.messages_sent > 0);
    assert!(cluster.stats.messages_dropped > 0);
    assert!(cluster.stats.messages_delivered > 0);
    assert_eq!(
        cluster.stats.messages_sent,
        cluster.stats.messages_delivered + cluster.stats.messages_dropped
    );
}

// ── View change / leader recovery ─────────────────────────────

#[tokio::test]
async fn pbft_view_change_after_primary_isolated() {
    // n=4: commit a value, then isolate primary (node 0),
    // tick until view change happens, then commit with new primary (node 1)
    let mut cluster = Cluster::new_pbft(4);

    // First: commit normally with the original primary
    let result = cluster.propose(Payload::new(b"before-crash")).await;
    assert_eq!(result, ProposeResult::Accepted);
    cluster.run_to_completion(30).await;

    let committed = cluster.collect_committed().await;
    assert!(
        !committed.is_empty(),
        "should commit before primary failure"
    );

    // Now isolate the primary
    cluster.set_faults(FaultConfig::new().with_isolated(NodeId(0)));

    // Tick until view change completes — replicas will timeout,
    // broadcast ViewChange, and node 1 becomes new primary
    for _ in 0..100 {
        cluster.tick().await;
        cluster.run_to_completion(50).await;
    }

    // Try to propose to the new primary
    let result = cluster.propose(Payload::new(b"after-view-change")).await;
    assert_eq!(
        result,
        ProposeResult::Accepted,
        "new primary should accept proposals"
    );

    cluster.run_to_completion(50).await;

    let committed = cluster.collect_committed().await;
    assert!(
        !committed.is_empty(),
        "should commit after view change with new primary"
    );
}

#[tokio::test]
async fn raft_reelection_after_leader_crash() {
    // n=5: bootstrap leader, commit a value, isolate leader,
    // wait for re-election, commit again
    let mut cluster = Cluster::new_raft_with_leader(5).await;

    let result = cluster.propose(Payload::new(b"before-crash")).await;
    assert_eq!(result, ProposeResult::Accepted);
    cluster.run_to_completion(30).await;

    let committed = cluster.collect_committed().await;
    assert!(!committed.is_empty(), "should commit before leader crash");

    // Crash the leader
    cluster.set_faults(FaultConfig::new().with_isolated(NodeId(0)));

    // Tick until a new leader is elected
    let leader = cluster.wait_for_leader(300).await;
    assert!(leader.is_some(), "new leader should be elected after crash");
    assert_ne!(
        leader.unwrap(),
        NodeId(0),
        "crashed node should not be leader"
    );

    // Propose to new leader
    let result = cluster.propose(Payload::new(b"after-reelection")).await;
    assert_eq!(result, ProposeResult::Accepted);
    cluster.run_to_completion(30).await;

    let committed = cluster.collect_committed().await;
    assert!(
        !committed.is_empty(),
        "should commit after leader re-election"
    );
}

// ── Raft log conflict resolution ──────────────────────────────

#[tokio::test]
async fn raft_follower_catches_up_after_partition_heals() {
    // n=3: commit values with node 2 partitioned, then heal and verify sync
    let mut cluster = Cluster::new_raft_with_leader(3).await;

    // Partition node 2
    cluster.set_faults(FaultConfig::new().with_isolated(NodeId(2)));

    // Commit 3 values — only nodes 0 and 1 will have them
    for i in 0..3 {
        let payload = Payload::new(format!("partitioned-{i}").into_bytes());
        cluster.propose(payload).await;
        cluster.run_to_completion(30).await;
    }

    let committed_during_partition = cluster.collect_committed().await;
    // Only leader (node 0) and node 1 should have committed
    let node2_committed = committed_during_partition.iter().find(|(id, _)| id.0 == 2);
    assert!(
        node2_committed.is_none(),
        "partitioned node should not have committed"
    );

    // Heal the partition
    cluster.clear_faults();

    // Leader sends heartbeat on tick, which carries the entries to node 2
    for _ in 0..20 {
        cluster.tick().await;
        cluster.run_to_completion(30).await;
    }

    // Now node 2 should catch up
    let committed_after_heal = cluster.collect_committed().await;
    let node2_after = committed_after_heal.iter().find(|(id, _)| id.0 == 2);

    assert!(
        node2_after.is_some(),
        "node 2 should catch up after partition heals"
    );

    if let Some((_, entries)) = node2_after {
        assert_eq!(
            entries.len(),
            3,
            "node 2 should have all 3 entries after catching up"
        );
    }
}
