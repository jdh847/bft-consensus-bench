use std::time::Instant;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

use bft_consensus_bench::cluster::Cluster;
use bft_consensus_bench::consensus::ProposeResult;
use bft_consensus_bench::types::Payload;

fn bench_pbft_single_commit(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("pbft_single_commit");
    for n in [4, 7] {
        group.bench_with_input(BenchmarkId::from_parameter(format!("n{n}")), &n, |b, &n| {
            b.iter(|| {
                rt.block_on(async {
                    let mut cluster = Cluster::new_pbft(n);
                    let result = cluster.propose(Payload::new(b"bench")).await;
                    assert_eq!(result, ProposeResult::Accepted);
                    cluster.run_to_completion(30).await;
                })
            })
        });
    }
    group.finish();
}

fn bench_raft_single_commit(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("raft_single_commit");
    for n in [3, 5] {
        group.bench_with_input(BenchmarkId::from_parameter(format!("n{n}")), &n, |b, &n| {
            b.iter(|| {
                rt.block_on(async {
                    let mut cluster = Cluster::new_raft_with_leader(n).await;
                    let result = cluster.propose(Payload::new(b"bench")).await;
                    assert_eq!(result, ProposeResult::Accepted);
                    cluster.run_to_completion(30).await;
                })
            })
        });
    }
    group.finish();
}

fn bench_throughput_comparison(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let num_proposals = 100;

    let mut group = c.benchmark_group("throughput_100_proposals");

    for n in [3, 5, 7] {
        group.bench_with_input(
            BenchmarkId::new("pbft", format!("n{n}")),
            &n,
            |b, &n| {
                b.iter(|| {
                    rt.block_on(async {
                        let mut cluster = Cluster::new_pbft(n);
                        for i in 0..num_proposals {
                            let payload = Payload::new(format!("v{i}").into_bytes());
                            cluster.propose(payload).await;
                            cluster.run_to_completion(30).await;
                        }
                    })
                })
            },
        );

        if n <= 5 {
            group.bench_with_input(
                BenchmarkId::new("raft", format!("n{n}")),
                &n,
                |b, &n| {
                    b.iter(|| {
                        rt.block_on(async {
                            let mut cluster = Cluster::new_raft_with_leader(n).await;
                            for i in 0..num_proposals {
                                let payload = Payload::new(format!("v{i}").into_bytes());
                                cluster.propose(payload).await;
                                cluster.run_to_completion(30).await;
                            }
                        })
                    })
                },
            );
        }
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_pbft_single_commit,
    bench_raft_single_commit,
    bench_throughput_comparison
);
criterion_main!(benches);
