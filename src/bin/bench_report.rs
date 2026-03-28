use std::time::Instant;

use bft_consensus_bench::cluster::Cluster;
use bft_consensus_bench::consensus::ProposeResult;
use bft_consensus_bench::types::Payload;

const NUM_PROPOSALS: usize = 500;
const WARMUP_PROPOSALS: usize = 10;
const BAR_WIDTH: usize = 40;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let csv_mode = args.iter().any(|a| a == "--csv");

    let mut results: Vec<RunResult> = Vec::new();

    if !csv_mode {
        println!("╔══════════════════════════════════════════════════════════════╗");
        println!("║          BFT Consensus Benchmark — PBFT vs Raft            ║");
        println!("╠══════════════════════════════════════════════════════════════╣");
        println!("║  Workload: {} proposals per run                           ║", NUM_PROPOSALS);
        println!("║  Metric:   time to commit all proposals (single-threaded)  ║");
        println!("╚══════════════════════════════════════════════════════════════╝");
        println!();
    }

    for n in [3, 5, 7] {
        let pbft = run_pbft_bench(n).await;
        results.push(RunResult { protocol: "PBFT", nodes: n, bench: pbft });

        let raft = run_raft_bench(n).await;
        results.push(RunResult { protocol: "Raft", nodes: n, bench: raft });
    }

    if csv_mode {
        print_csv(&results);
    } else {
        print_table(&results);
        println!();
        print_throughput_chart(&results);
        println!();
        print_message_chart(&results);
        println!();
        print_scaling_chart(&results);
        println!();
        println!("Notes:");
        println!("  - PBFT tolerates Byzantine (arbitrary) faults: f < n/3");
        println!("  - Raft tolerates crash (fail-stop) faults: f < n/2");
        println!("  - PBFT message complexity: O(n²) per commit");
        println!("  - Raft message complexity: O(n) per commit");
        println!();
        println!("Export CSV: cargo run --release --bin bench_report -- --csv");
    }
}

struct BenchResult {
    total_ms: f64,
    per_op_us: f64,
    total_messages: u64,
}

struct RunResult {
    protocol: &'static str,
    nodes: usize,
    bench: BenchResult,
}

// ── Table output ─────────────────────────────────────────────

fn print_table(results: &[RunResult]) {
    println!(
        "{:<12} {:<8} {:<12} {:<14} {:<14} {:<10}",
        "Protocol", "Nodes", "Fault Tol.", "Total (ms)", "Per-Op (μs)", "Msgs"
    );
    println!("{}", "─".repeat(70));

    for (i, r) in results.iter().enumerate() {
        let ft = match r.protocol {
            "PBFT" => format!("f<{}", (r.nodes - 1) / 3),
            _ => format!("f<{}", (r.nodes - 1) / 2),
        };
        println!(
            "{:<12} {:<8} {:<12} {:<14.2} {:<14.1} {:<10}",
            r.protocol, r.nodes, ft,
            r.bench.total_ms, r.bench.per_op_us, r.bench.total_messages
        );
        if i % 2 == 1 {
            println!("{}", "─".repeat(70));
        }
    }
}

// ── ASCII bar charts ─────────────────────────────────────────

fn print_throughput_chart(results: &[RunResult]) {
    println!("Throughput (μs per operation, lower is better)");
    println!("═══════════════════════════════════════════════════════════");

    let max_us = results.iter()
        .map(|r| r.bench.per_op_us)
        .fold(0.0_f64, f64::max);

    for r in results {
        let bar_len = ((r.bench.per_op_us / max_us) * BAR_WIDTH as f64) as usize;
        let bar: String = "█".repeat(bar_len);
        println!(
            "  {:<4} n={} │{:<width$}│ {:.1} μs",
            r.protocol, r.nodes, bar,
            r.bench.per_op_us,
            width = BAR_WIDTH
        );
    }
}

fn print_message_chart(results: &[RunResult]) {
    println!("Message count ({} proposals, lower is better)", NUM_PROPOSALS);
    println!("═══════════════════════════════════════════════════════════");

    let max_msgs = results.iter()
        .map(|r| r.bench.total_messages)
        .max()
        .unwrap_or(1);

    for r in results {
        let bar_len = ((r.bench.total_messages as f64 / max_msgs as f64) * BAR_WIDTH as f64) as usize;
        let bar: String = "▓".repeat(bar_len);
        println!(
            "  {:<4} n={} │{:<width$}│ {:>6}",
            r.protocol, r.nodes, bar,
            r.bench.total_messages,
            width = BAR_WIDTH
        );
    }
}

fn print_scaling_chart(results: &[RunResult]) {
    println!("Message scaling: PBFT O(n²) vs Raft O(n)");
    println!("═══════════════════════════════════════════════════════════");

    let pbft_results: Vec<&RunResult> = results.iter().filter(|r| r.protocol == "PBFT").collect();
    let raft_results: Vec<&RunResult> = results.iter().filter(|r| r.protocol == "Raft").collect();

    if pbft_results.len() < 2 || raft_results.len() < 2 {
        return;
    }

    // Show messages-per-op ratio (how many msgs per proposal)
    println!("  Messages per proposal:");
    println!("  {:<8} {:>6} {:>6} {:>6}", "Nodes", "n=3", "n=5", "n=7");
    println!("  {}", "─".repeat(30));

    for proto in ["PBFT", "Raft"] {
        let vals: Vec<String> = results.iter()
            .filter(|r| r.protocol == proto)
            .map(|r| format!("{:.1}", r.bench.total_messages as f64 / NUM_PROPOSALS as f64))
            .collect();
        println!("  {:<8} {:>6} {:>6} {:>6}", proto, vals[0], vals[1], vals[2]);
    }

    // Show PBFT/Raft ratio
    println!();
    println!("  PBFT/Raft message ratio:");
    for (p, r) in pbft_results.iter().zip(raft_results.iter()) {
        let ratio = p.bench.total_messages as f64 / r.bench.total_messages as f64;
        println!("    n={}: {:.1}x more messages in PBFT", p.nodes, ratio);
    }
}

// ── CSV output ───────────────────────────────────────────────

fn print_csv(results: &[RunResult]) {
    println!("protocol,nodes,fault_tolerance,total_ms,per_op_us,total_messages,msgs_per_op");
    for r in results {
        let ft = match r.protocol {
            "PBFT" => (r.nodes - 1) / 3,
            _ => (r.nodes - 1) / 2,
        };
        println!(
            "{},{},{},{:.2},{:.1},{},{:.1}",
            r.protocol, r.nodes, ft,
            r.bench.total_ms, r.bench.per_op_us,
            r.bench.total_messages,
            r.bench.total_messages as f64 / NUM_PROPOSALS as f64
        );
    }
}

// ── Benchmark runners ────────────────────────────────────────

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
