use std::time::Instant;

use bft_consensus_bench::cluster::fault::{ByzantineBehavior, FaultConfig};
use bft_consensus_bench::cluster::Cluster;
use bft_consensus_bench::consensus::ProposeResult;
use bft_consensus_bench::types::{NodeId, Payload};

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
        println!("║      BFT Consensus Benchmark — PBFT vs HotStuff vs Raft   ║");
        println!("╠══════════════════════════════════════════════════════════════╣");
        println!(
            "║  Workload: {} proposals per run                           ║",
            NUM_PROPOSALS
        );
        println!("║  Metric:   time to commit all proposals (single-threaded)  ║");
        println!("╚══════════════════════════════════════════════════════════════╝");
        println!();
    }

    // ── Clean benchmarks ────────────────────────────────────────
    for n in [3, 5, 7] {
        let pbft = run_pbft_bench(n).await;
        results.push(RunResult {
            protocol: "PBFT",
            nodes: n,
            byzantine: false,
            bench: pbft,
        });

        let hotstuff = run_hotstuff_bench(n).await;
        results.push(RunResult {
            protocol: "HotStuff",
            nodes: n,
            byzantine: false,
            bench: hotstuff,
        });

        let raft = run_raft_bench(n).await;
        results.push(RunResult {
            protocol: "Raft",
            nodes: n,
            byzantine: false,
            bench: raft,
        });
    }

    // ── Byzantine overhead benchmarks ───────────────────────────
    let mut byz_results: Vec<RunResult> = Vec::new();

    // PBFT n=4 clean vs 1 equivocator
    let pbft4_clean = run_pbft_bench(4).await;
    byz_results.push(RunResult {
        protocol: "PBFT",
        nodes: 4,
        byzantine: false,
        bench: pbft4_clean,
    });

    let pbft4_byz = run_pbft_byzantine_bench(
        4,
        42,
        vec![(NodeId(3), vec![ByzantineBehavior::Equivocate])],
    )
    .await;
    byz_results.push(RunResult {
        protocol: "PBFT",
        nodes: 4,
        byzantine: true,
        bench: pbft4_byz,
    });

    // HotStuff n=4 clean vs 1 equivocator
    let hs4_clean = run_hotstuff_bench(4).await;
    byz_results.push(RunResult {
        protocol: "HotStuff",
        nodes: 4,
        byzantine: false,
        bench: hs4_clean,
    });

    let hs4_byz = run_hotstuff_byzantine_bench(
        4,
        42,
        vec![(NodeId(3), vec![ByzantineBehavior::Equivocate])],
    )
    .await;
    byz_results.push(RunResult {
        protocol: "HotStuff",
        nodes: 4,
        byzantine: true,
        bench: hs4_byz,
    });

    // HotStuff n=7 clean vs 2 digest corruptors
    let hs7_clean = run_hotstuff_bench(7).await;
    byz_results.push(RunResult {
        protocol: "HotStuff",
        nodes: 7,
        byzantine: false,
        bench: hs7_clean,
    });

    let hs7_byz = run_hotstuff_byzantine_bench(
        7,
        99,
        vec![
            (NodeId(5), vec![ByzantineBehavior::CorruptDigest]),
            (NodeId(6), vec![ByzantineBehavior::CorruptDigest]),
        ],
    )
    .await;
    byz_results.push(RunResult {
        protocol: "HotStuff",
        nodes: 7,
        byzantine: true,
        bench: hs7_byz,
    });

    // PBFT n=7 clean vs 2 digest corruptors
    let pbft7_clean = run_pbft_bench(7).await;
    byz_results.push(RunResult {
        protocol: "PBFT",
        nodes: 7,
        byzantine: false,
        bench: pbft7_clean,
    });

    let pbft7_byz = run_pbft_byzantine_bench(
        7,
        99,
        vec![
            (NodeId(5), vec![ByzantineBehavior::CorruptDigest]),
            (NodeId(6), vec![ByzantineBehavior::CorruptDigest]),
        ],
    )
    .await;
    byz_results.push(RunResult {
        protocol: "PBFT",
        nodes: 7,
        byzantine: true,
        bench: pbft7_byz,
    });

    if csv_mode {
        print_csv(&results, &byz_results);
    } else {
        print_table(&results);
        println!();
        print_throughput_chart(&results);
        println!();
        print_message_chart(&results);
        println!();
        print_scaling_chart(&results);
        println!();
        print_byzantine_overhead(&byz_results);
        println!();
        println!("Notes:");
        println!("  - PBFT tolerates Byzantine (arbitrary) faults: f < n/3, O(n²) messages");
        println!("  - HotStuff tolerates Byzantine faults: f < n/3, O(n) messages (star topology)");
        println!("  - Raft tolerates crash (fail-stop) faults: f < n/2, O(n) messages");
        println!();
        println!("Export CSV: cargo run --release --bin bench_report -- --csv");
    }
}

struct BenchResult {
    total_ms: f64,
    per_op_us: f64,
    total_messages: u64,
    messages_tampered: u64,
}

struct RunResult {
    protocol: &'static str,
    nodes: usize,
    byzantine: bool,
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
            "PBFT" | "HotStuff" => format!("f<{}", (r.nodes - 1) / 3),
            _ => format!("f<{}", (r.nodes - 1) / 2),
        };
        println!(
            "{:<12} {:<8} {:<12} {:<14.2} {:<14.1} {:<10}",
            r.protocol, r.nodes, ft, r.bench.total_ms, r.bench.per_op_us, r.bench.total_messages
        );
        if i % 3 == 2 {
            println!("{}", "─".repeat(70));
        }
    }
}

// ── ASCII bar charts ─────────────────────────────────────────

fn print_throughput_chart(results: &[RunResult]) {
    println!("Throughput (μs per operation, lower is better)");
    println!("═══════════════════════════════════════════════════════════");

    let max_us = results
        .iter()
        .map(|r| r.bench.per_op_us)
        .fold(0.0_f64, f64::max);

    for r in results {
        let bar_len = ((r.bench.per_op_us / max_us) * BAR_WIDTH as f64) as usize;
        let bar: String = "█".repeat(bar_len);
        println!(
            "  {:<4} n={} │{:<width$}│ {:.1} μs",
            r.protocol,
            r.nodes,
            bar,
            r.bench.per_op_us,
            width = BAR_WIDTH
        );
    }
}

fn print_message_chart(results: &[RunResult]) {
    println!(
        "Message count ({} proposals, lower is better)",
        NUM_PROPOSALS
    );
    println!("═══════════════════════════════════════════════════════════");

    let max_msgs = results
        .iter()
        .map(|r| r.bench.total_messages)
        .max()
        .unwrap_or(1);

    for r in results {
        let bar_len =
            ((r.bench.total_messages as f64 / max_msgs as f64) * BAR_WIDTH as f64) as usize;
        let bar: String = "▓".repeat(bar_len);
        println!(
            "  {:<4} n={} │{:<width$}│ {:>6}",
            r.protocol,
            r.nodes,
            bar,
            r.bench.total_messages,
            width = BAR_WIDTH
        );
    }
}

fn print_scaling_chart(results: &[RunResult]) {
    println!("Message scaling: PBFT O(n²) vs HotStuff O(n) vs Raft O(n)");
    println!("═══════════════════════════════════════════════════════════");

    let pbft_results: Vec<&RunResult> = results.iter().filter(|r| r.protocol == "PBFT").collect();
    let hs_results: Vec<&RunResult> = results
        .iter()
        .filter(|r| r.protocol == "HotStuff")
        .collect();
    let raft_results: Vec<&RunResult> = results.iter().filter(|r| r.protocol == "Raft").collect();

    if pbft_results.len() < 2 || raft_results.len() < 2 {
        return;
    }

    // Show messages-per-op ratio (how many msgs per proposal)
    println!("  Messages per proposal:");
    println!(
        "  {:<10} {:>6} {:>6} {:>6}",
        "Protocol", "n=3", "n=5", "n=7"
    );
    println!("  {}", "─".repeat(34));

    for proto in ["PBFT", "HotStuff", "Raft"] {
        let vals: Vec<String> = results
            .iter()
            .filter(|r| r.protocol == proto)
            .map(|r| {
                format!(
                    "{:.1}",
                    r.bench.total_messages as f64 / NUM_PROPOSALS as f64
                )
            })
            .collect();
        println!(
            "  {:<10} {:>6} {:>6} {:>6}",
            proto, vals[0], vals[1], vals[2]
        );
    }

    // Show PBFT/HotStuff/Raft ratios
    println!();
    println!("  PBFT/Raft message ratio:");
    for (p, r) in pbft_results.iter().zip(raft_results.iter()) {
        let ratio = p.bench.total_messages as f64 / r.bench.total_messages as f64;
        println!("    n={}: {:.1}x more messages in PBFT", p.nodes, ratio);
    }

    if !hs_results.is_empty() {
        println!();
        println!("  PBFT/HotStuff message ratio:");
        for (p, h) in pbft_results.iter().zip(hs_results.iter()) {
            let ratio = p.bench.total_messages as f64 / h.bench.total_messages as f64;
            println!("    n={}: {:.1}x more messages in PBFT", p.nodes, ratio);
        }
    }
}

// ── Byzantine overhead ──────────────────────────────────────

fn print_byzantine_overhead(byz_results: &[RunResult]) {
    println!("Byzantine fault overhead (PBFT with active faults)");
    println!("═══════════════════════════════════════════════════════════");

    println!(
        "  {:<10} {:<8} {:<14} {:<14} {:<10} {:<10}",
        "Scenario", "Nodes", "Per-Op (μs)", "Messages", "Tampered", "Overhead"
    );
    println!("  {}", "─".repeat(66));

    // Process pairs: clean then byzantine
    for pair in byz_results.chunks(2) {
        if pair.len() < 2 {
            continue;
        }
        let clean = &pair[0];
        let byz = &pair[1];

        let overhead_pct = if clean.bench.per_op_us > 0.0 {
            ((byz.bench.per_op_us - clean.bench.per_op_us) / clean.bench.per_op_us) * 100.0
        } else {
            0.0
        };

        println!(
            "  {:<10} {:<8} {:<14.1} {:<14} {:<10} {:<10}",
            "clean", clean.nodes, clean.bench.per_op_us, clean.bench.total_messages, "-", "-"
        );
        println!(
            "  {:<10} {:<8} {:<14.1} {:<14} {:<10} {:<10}",
            "byzantine",
            byz.nodes,
            byz.bench.per_op_us,
            byz.bench.total_messages,
            byz.bench.messages_tampered,
            format!("+{:.1}%", overhead_pct)
        );
        println!("  {}", "─".repeat(66));
    }

    // ASCII chart comparing clean vs byzantine
    println!();
    println!("  Latency comparison (μs per operation):");

    let max_us = byz_results
        .iter()
        .map(|r| r.bench.per_op_us)
        .fold(0.0_f64, f64::max);

    for r in byz_results {
        let label = if r.byzantine { "byz" } else { "clean" };
        let bar_len = ((r.bench.per_op_us / max_us) * BAR_WIDTH as f64) as usize;
        let bar: String = if r.byzantine {
            "░".repeat(bar_len)
        } else {
            "█".repeat(bar_len)
        };
        println!(
            "    n={} {:<5} │{:<width$}│ {:.1} μs",
            r.nodes,
            label,
            bar,
            r.bench.per_op_us,
            width = BAR_WIDTH
        );
    }
}

// ── CSV output ───────────────────────────────────────────────

fn print_csv(results: &[RunResult], byz_results: &[RunResult]) {
    println!("protocol,nodes,fault_tolerance,total_ms,per_op_us,total_messages,msgs_per_op,byzantine,messages_tampered");
    for r in results.iter().chain(byz_results.iter()) {
        let ft = match r.protocol {
            "PBFT" | "HotStuff" => (r.nodes - 1) / 3,
            _ => (r.nodes - 1) / 2,
        };
        println!(
            "{},{},{},{:.2},{:.1},{},{:.1},{},{}",
            r.protocol,
            r.nodes,
            ft,
            r.bench.total_ms,
            r.bench.per_op_us,
            r.bench.total_messages,
            r.bench.total_messages as f64 / NUM_PROPOSALS as f64,
            r.byzantine,
            r.bench.messages_tampered
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
        messages_tampered: cluster.stats.messages_tampered,
    }
}

async fn run_hotstuff_bench(n: usize) -> BenchResult {
    // Warmup
    let mut cluster = Cluster::new_hotstuff(n);
    for i in 0..WARMUP_PROPOSALS {
        let payload = Payload::new(format!("warmup-{i}").into_bytes());
        cluster.propose(payload).await;
        cluster.run_to_completion(50).await;
    }

    // Actual benchmark
    let mut cluster = Cluster::new_hotstuff(n);
    let start = Instant::now();

    for i in 0..NUM_PROPOSALS {
        let payload = Payload::new(format!("hotstuff-{i}").into_bytes());
        let result = cluster.propose(payload).await;
        assert_eq!(result, ProposeResult::Accepted);
        cluster.run_to_completion(50).await;
    }

    let elapsed = start.elapsed();
    let total_ms = elapsed.as_secs_f64() * 1000.0;

    BenchResult {
        total_ms,
        per_op_us: (elapsed.as_micros() as f64) / NUM_PROPOSALS as f64,
        total_messages: cluster.stats.messages_sent,
        messages_tampered: cluster.stats.messages_tampered,
    }
}

async fn run_hotstuff_byzantine_bench(
    n: usize,
    seed: u64,
    behaviors: Vec<(NodeId, Vec<ByzantineBehavior>)>,
) -> BenchResult {
    // Warmup
    let mut cluster = Cluster::new_hotstuff_seeded(n, seed);
    let mut faults = FaultConfig::with_seed(seed);
    for (node, beh) in &behaviors {
        faults = faults.with_byzantine(*node, beh.clone());
    }
    cluster.set_faults(faults);
    for i in 0..WARMUP_PROPOSALS {
        let payload = Payload::new(format!("warmup-{i}").into_bytes());
        cluster.propose(payload).await;
        cluster.run_to_completion(50).await;
    }

    // Actual benchmark
    let mut cluster = Cluster::new_hotstuff_seeded(n, seed);
    let mut faults = FaultConfig::with_seed(seed);
    for (node, beh) in &behaviors {
        faults = faults.with_byzantine(*node, beh.clone());
    }
    cluster.set_faults(faults);
    let start = Instant::now();

    for i in 0..NUM_PROPOSALS {
        let payload = Payload::new(format!("byz-{i}").into_bytes());
        let result = cluster.propose(payload).await;
        assert_eq!(result, ProposeResult::Accepted);
        cluster.run_to_completion(50).await;
    }

    let elapsed = start.elapsed();
    let total_ms = elapsed.as_secs_f64() * 1000.0;

    BenchResult {
        total_ms,
        per_op_us: (elapsed.as_micros() as f64) / NUM_PROPOSALS as f64,
        total_messages: cluster.stats.messages_sent,
        messages_tampered: cluster.stats.messages_tampered,
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
        messages_tampered: cluster.stats.messages_tampered,
    }
}

async fn run_pbft_byzantine_bench(
    n: usize,
    seed: u64,
    behaviors: Vec<(NodeId, Vec<ByzantineBehavior>)>,
) -> BenchResult {
    // Warmup
    let mut cluster = Cluster::new_pbft_seeded(n, seed);
    let mut faults = FaultConfig::with_seed(seed);
    for (node, beh) in &behaviors {
        faults = faults.with_byzantine(*node, beh.clone());
    }
    cluster.set_faults(faults);
    for i in 0..WARMUP_PROPOSALS {
        let payload = Payload::new(format!("warmup-{i}").into_bytes());
        cluster.propose(payload).await;
        cluster.run_to_completion(50).await;
    }

    // Actual benchmark
    let mut cluster = Cluster::new_pbft_seeded(n, seed);
    let mut faults = FaultConfig::with_seed(seed);
    for (node, beh) in &behaviors {
        faults = faults.with_byzantine(*node, beh.clone());
    }
    cluster.set_faults(faults);
    let start = Instant::now();

    for i in 0..NUM_PROPOSALS {
        let payload = Payload::new(format!("byz-{i}").into_bytes());
        let result = cluster.propose(payload).await;
        assert_eq!(result, ProposeResult::Accepted);
        cluster.run_to_completion(50).await;
    }

    let elapsed = start.elapsed();
    let total_ms = elapsed.as_secs_f64() * 1000.0;

    BenchResult {
        total_ms,
        per_op_us: (elapsed.as_micros() as f64) / NUM_PROPOSALS as f64,
        total_messages: cluster.stats.messages_sent,
        messages_tampered: cluster.stats.messages_tampered,
    }
}
