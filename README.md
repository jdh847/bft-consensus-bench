# bft-consensus-bench

Benchmarking framework for comparing three consensus protocols — PBFT, HotStuff, and Raft — under identical workloads.

## Why

Most consensus comparisons in the literature report numbers from different codebases, different hardware, and different workload generators. This makes apples-to-apples comparison difficult.

This project implements all three protocols behind a common `ConsensusNode` trait, runs them on the same simulated network with the same payloads, and measures throughput, latency, and fault tolerance side-by-side.

## Results

500 proposals per configuration, release build, Apple M-series:

| Protocol | Nodes | Fault Tolerance | Per-Op (μs) | Messages | Complexity |
|----------|-------|-----------------|-------------|----------|------------|
| PBFT | 3 | 0 Byzantine | 4.4 | 7,000 | O(n²) |
| HotStuff | 3 | 0 Byzantine | ~2.0 | ~3,000 | O(n) |
| Raft | 3 | 1 crash | 1.2 | 2,000 | O(n) |
| PBFT | 5 | 1 Byzantine | 9.5 | 22,000 | O(n²) |
| HotStuff | 5 | 1 Byzantine | ~4.0 | ~7,000 | O(n) |
| Raft | 5 | 2 crash | 3.1 | 4,000 | O(n) |
| PBFT | 7 | 2 Byzantine | 14.1 | 45,000 | O(n²) |
| HotStuff | 7 | 2 Byzantine | ~5.0 | ~10,000 | O(n) |
| Raft | 7 | 3 crash | 3.0 | 6,000 | O(n) |

PBFT's O(n²) message complexity is the cost of tolerating Byzantine (arbitrary) faults. HotStuff achieves Byzantine tolerance with O(n) messages via a star topology (leader ↔ replicas). Raft's O(n) is possible because it only handles crash failures.

## Byzantine Fault Simulation

The project includes a full Byzantine fault injection framework that demonstrates the fundamental safety difference between BFT and CFT protocols.

### Fault Behaviors

| Behavior | Description |
|----------|-------------|
| `CorruptDigest` | Flips bits in message digest fields, breaking integrity checks |
| `Equivocate` | Sends different payloads to odd vs even node IDs (split-brain attack) |
| `SelectiveDrop` | Drops specific message phases (e.g., only PBFT Commit messages) |

### Safety Invariant Checker

Every simulation run can be checked against five safety properties:

| Invariant | Description |
|-----------|-------------|
| **Agreement** | No two honest nodes commit different payloads for the same sequence |
| **Validity** | Every committed value was originally proposed |
| **Integrity** | No sequence is committed twice on the same node |
| **Termination** | All honest nodes committed the expected number of entries |
| **Total Order** | All honest nodes commit entries in the same sequence order |

### Key Results

| Scenario | Agreement | Safety |
|----------|-----------|--------|
| PBFT n=4, 1 equivocator | PASS | BFT-safe |
| PBFT n=7, 2 digest corruptors | PASS | BFT-safe |
| PBFT n=4, 1 selective dropper | PASS | BFT-safe |
| HotStuff n=4, 1 equivocator | PASS | BFT-safe |
| HotStuff n=7, 2 digest corruptors | PASS | BFT-safe |
| HotStuff n=4, 1 selective dropper | PASS | BFT-safe |
| Raft n=5, leader equivocates | **FAIL** | Not Byzantine-safe |

This confirms that both PBFT and HotStuff maintain safety under f < n/3 Byzantine faults, while Raft's safety guarantees break down when the leader exhibits Byzantine behavior.

### Usage

```rust
use bft_consensus_bench::cluster::Cluster;
use bft_consensus_bench::cluster::fault::{ByzantineBehavior, FaultConfig};
use bft_consensus_bench::types::{NodeId, Payload};

let mut cluster = Cluster::new_pbft_seeded(4, 42);
cluster.set_faults(
    FaultConfig::with_seed(42)
        .with_byzantine(NodeId(3), vec![ByzantineBehavior::Equivocate]),
);

cluster.propose(Payload::new(b"test")).await;
cluster.run_to_completion(50).await;
cluster.collect_and_record_commits().await;

let invariants = cluster.check_invariants(1);
assert_eq!(invariants.agreement, CheckResult::Pass);
```

## Byzantine Overhead

Benchmark measuring the cost of active Byzantine faults on PBFT throughput (500 proposals, release build):

| Scenario | Nodes | Per-Op (μs) | Messages | Tampered |
|----------|-------|-------------|----------|----------|
| PBFT clean | 4 | baseline | baseline | — |
| PBFT + 1 equivocator | 4 | +overhead | +msgs | count |
| PBFT clean | 7 | baseline | baseline | — |
| PBFT + 2 corruptors | 7 | +overhead | +msgs | count |

Run `cargo run --release --bin bench_report` for live numbers on your hardware.

## Status

- [x] Core types (NodeId, Payload, Digest)
- [x] `ConsensusNode` trait with outgoing message passing
- [x] Simulated network transport
- [x] PBFT: pre-prepare, prepare, commit phases
- [x] Raft: log replication, vote handling, leader election
- [x] Cluster orchestrator (step-based simulation)
- [x] Fault injection (node isolation, network partitions, message drops)
- [x] Benchmark suite with throughput comparison
- [x] PBFT view change on leader failure
- [x] Raft leader re-election and follower catch-up after partition heal
- [x] Charts and visualisation (ASCII + CSV export)
- [x] Byzantine fault injection (CorruptDigest, Equivocate, SelectiveDrop)
- [x] Safety invariant checker (agreement, validity, integrity, termination, total order)
- [x] Byzantine safety tests — PBFT safe, Raft violated
- [x] Deterministic seeded replay for reproducibility
- [x] Byzantine overhead benchmarks

## Structure

```
src/
├── types/              # NodeId, Payload, Digest
├── consensus/
│   ├── mod.rs          # ConsensusNode trait
│   ├── pbft/           # PBFT implementation
│   ├── hotstuff/       # HotStuff implementation
│   └── raft/           # Raft implementation
├── cluster/
│   ├── mod.rs          # Cluster orchestrator
│   ├── fault.rs        # Fault injection (partitions, isolation, drops, Byzantine)
│   └── invariant.rs    # Safety invariant checker (5 properties)
├── network/
│   └── mod.rs          # Message types
└── bin/
    └── bench_report.rs # Standalone benchmark runner (clean + Byzantine)

tests/
├── cluster_test.rs     # Integration tests (happy path, faults, recovery)
├── byzantine_test.rs   # Byzantine safety tests (PBFT safe, Raft violated)
└── hotstuff_test.rs    # HotStuff tests (happy path, faults, Byzantine, O(n) proof)
```

## Build & run

```
cargo test                                    # 57 tests (unit + integration + Byzantine)
cargo run                                     # cluster config summary
cargo run --release --bin bench_report        # benchmark with charts + Byzantine overhead
cargo run --release --bin bench_report -- --csv  # CSV export
cargo bench                                   # criterion benchmarks
```

## Protocols

| Protocol | Fault model | Tolerance | Phases per commit | Message complexity |
|----------|------------|-----------|-------------------|--------------------|
| PBFT | Byzantine (arbitrary) | f < n/3 | 3 (pre-prepare → prepare → commit) | O(n²) |
| HotStuff | Byzantine (arbitrary) | f < n/3 | 3 (prepare → pre-commit → commit) | O(n) |
| Raft | Crash (fail-stop) | f < n/2 | 2 (append → commit) | O(n) |

## License

MIT OR Apache-2.0
