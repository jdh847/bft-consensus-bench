use std::time::Instant;

use bft_consensus_bench::cluster::Cluster;
use bft_consensus_bench::consensus::ProposeResult;
use bft_consensus_bench::types::Payload;

const NUM_PROPOSALS: usize = 500;
const WARMUP_PROPOSALS: usize = 10;

#[tokio::main]
async fn main() {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║          BFT Consensus Benchmark — PBFT vs Raft            ║");
    println!("╠══════════════════════════════════════════════════════════════╣");
    println!("║  Workload: {} proposals per run                           ║", NUM_PROPOSALS);
    println!("║  Metric:   time to commit all proposals (single-threaded)  ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();

    println!(
        "{:<12} {:<8} {:<12} {:<14} {:<14} {:<10}",
        "Protocol", "Nodes", "Fault Tol.", "Total (ms)", "Per-Op (μs)", "Msgs"
    );
    println!("{}", "─".repeat(70));

    for n in [3, 5, 7] {
        // PBFT
        let result = run_pbft_bench(n).await;
        println!(
            "{:<12} {:<8} {:<12} {:<14.2} {:<14.1} {:<10}",
            "PBFT", n, format!("f<{}", (n - 1) / 3),
            result.total_ms, result.per_op_us, result.total_messages
        );

        // Raft
        let result = run_raft_bench(n).await;
        println!(
            "{:<12} {:<8} {:<12} {:<14.2} {:<14.1} {:<10}",
            "Raft", n, format!("f<{}", (n - 1) / 2),
            result.total_ms, result.per_op_us, result.total_messages
        );

        println!("{}", "─".repeat(70));
    }

    println!();
    println!("Notes:");
    println!("  - PBFT tolerates Byzantine (arbitrary) faults: f < n/3");
    println!("  - Raft tolerates crash (fail-stop) faults: f < n/2");
    println!("  - PBFT has higher message complexity (O(n²) per commit)");
    println!("  - Raft has lower message complexity (O(n) per commit)");
}

struct BenchResult {
    total_ms: f64,
    per_op_us: f64,
    total_messages: u64,
}

async fn run_pbft_bench(n: usize) -> BenchResult {
    // Warmup
    let mut cluster = Cluster::new_pbft(n);
    for i in 0..WARMUP_PROPOSALS {
        let payload = Payload::new(format!("warmup-{i}").into_bytes());
        cluster.propose(payload).await;
        cluster.run_to_completion(30).await;
    }

    // Actual benchmark
    let mut cluster = Cluster::new_pbft(n);
    let start = Instant::now();

    for i in 0..NUM_PROPOSALS {
        let payload = Payload::new(format!("pbft-{i}").into_bytes());
        let result = cluster.propose(payload).await;
        assert_eq!(result, ProposeResult::Accepted);
        cluster.run_to_completion(30).await;
    }

    let elapsed = start.elapsed();
    let total_ms = elapsed.as_secs_f64() * 1000.0;

    BenchResult {
        total_ms,
        per_op_us: (elapsed.as_micros() as f64) / NUM_PROPOSALS as f64,
        total_messages: cluster.stats.messages_sent,
    }
}

async fn run_raft_bench(n: usize) -> BenchResult {
    // Warmup
    let mut cluster = Cluster::new_raft_with_leader(n).await;
    for i in 0..WARMUP_PROPOSALS {
        let payload = Payload::new(format!("warmup-{i}").into_bytes());
        cluster.propose(payload).await;
        cluster.run_to_completion(30).await;
    }

    // Actual benchmark
    let mut cluster = Cluster::new_raft_with_leader(n).await;
    let start = Instant::now();

    for i in 0..NUM_PROPOSALS {
        let payload = Payload::new(format!("raft-{i}").into_bytes());
        let result = cluster.propose(payload).await;
        assert_eq!(result, ProposeResult::Accepted);
        cluster.run_to_completion(30).await;
    }

    let elapsed = start.elapsed();
    let total_ms = elapsed.as_secs_f64() * 1000.0;

    BenchResult {
        total_ms,
        per_op_us: (elapsed.as_micros() as f64) / NUM_PROPOSALS as f64,
        total_messages: cluster.stats.messages_sent,
    }
}
