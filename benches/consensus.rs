use criterion::{criterion_group, criterion_main, Criterion};

use bft_consensus_bench::consensus::pbft::PbftNode;
use bft_consensus_bench::consensus::raft::RaftNode;
use bft_consensus_bench::consensus::ConsensusNode;
use bft_consensus_bench::types::{NodeId, Payload};

fn bench_pbft_propose(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("pbft_propose_n4", |b| {
        b.iter(|| {
            rt.block_on(async {
                let node = PbftNode::new(NodeId(0), 4);
                let payload = Payload::new(b"benchmark-payload".to_vec());
                node.propose(payload).await
            })
        })
    });
}

fn bench_raft_propose(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("raft_propose_n3", |b| {
        b.iter(|| {
            rt.block_on(async {
                let node = RaftNode::new(NodeId(0), 3);
                node.force_leader().await;
                let payload = Payload::new(b"benchmark-payload".to_vec());
                node.propose(payload).await
            })
        })
    });
}

criterion_group!(benches, bench_pbft_propose, bench_raft_propose);
criterion_main!(benches);
