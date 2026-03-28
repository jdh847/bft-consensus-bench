use bft_consensus_bench::cluster::Cluster;
use bft_consensus_bench::consensus::ProposeResult;
use bft_consensus_bench::types::Payload;

#[tokio::test]
async fn pbft_4_nodes_commit_single_value() {
    let mut cluster = Cluster::new_pbft(4);

    let result = cluster.propose(Payload::new(b"hello pbft")).await;
    assert_eq!(result, ProposeResult::Accepted);

    // Run message delivery until quiescent
    let steps = cluster.run_to_completion(20).await;
    assert!(steps > 0, "expected at least one delivery step");

    let committed = cluster.collect_committed().await;
    assert!(
        !committed.is_empty(),
        "expected at least one node to commit"
    );

    // Verify all committing nodes agree on the payload
    for (node_id, entries) in &committed {
        assert_eq!(entries.len(), 1, "node {node_id} should commit exactly 1 entry");
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

    // Primary (node 0) should have all 5 committed
    let primary_committed = committed.iter()
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

    // No leader yet — tick until one is elected
    let leader = cluster.wait_for_leader(200).await;
    assert!(leader.is_some(), "a leader should be elected within 200 ticks");

    // Now propose a value
    let result = cluster.propose(Payload::new(b"after election")).await;
    assert_eq!(result, ProposeResult::Accepted);

    cluster.run_to_completion(20).await;

    let committed = cluster.collect_committed().await;
    assert!(
        !committed.is_empty(),
        "should commit after leader election"
    );
}
