# bft-consensus-bench

Benchmarking framework for comparing Byzantine fault-tolerant (PBFT) and crash fault-tolerant (Raft) consensus protocols under identical workloads.

## Why

Most consensus comparisons in the literature report numbers from different codebases, different hardware, and different workload generators. This makes apples-to-apples comparison difficult.

This project implements both protocols behind a common `ConsensusNode` trait, runs them on the same simulated network with the same payloads, and measures throughput, latency, and fault tolerance side-by-side.

## Results

500 proposals per configuration, release build, Apple M-series:

| Protocol | Nodes | Fault Tolerance | Per-Op (μs) | Messages | Complexity |
|----------|-------|-----------------|-------------|----------|------------|
| PBFT | 3 | 0 Byzantine | 4.4 | 7,000 | O(n²) |
| Raft | 3 | 1 crash | 1.2 | 2,000 | O(n) |
| PBFT | 5 | 1 Byzantine | 9.5 | 22,000 | O(n²) |
| Raft | 5 | 2 crash | 3.1 | 4,000 | O(n) |
| PBFT | 7 | 2 Byzantine | 14.1 | 45,000 | O(n²) |
| Raft | 7 | 3 crash | 3.0 | 6,000 | O(n) |

PBFT's O(n²) message complexity is the cost of tolerating Byzantine (arbitrary) faults. Raft's O(n) is possible because it only handles crash failures.

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
- [ ] Charts and visualisation

## Structure

```
src/
├── types/          # NodeId, Payload, Digest
├── consensus/
│   ├── mod.rs      # ConsensusNode trait
│   ├── pbft/       # PBFT implementation
│   └── raft/       # Raft implementation
├── cluster/
│   ├── mod.rs      # Cluster orchestrator
│   └── fault.rs    # Fault injection (partitions, isolation, drops)
├── network/
│   ├── mod.rs      # Message types
│   └── transport.rs # Simulated network
└── bin/
    └── bench_report.rs  # Standalone benchmark runner
```

## Build & run

```
cargo test                         # 23 tests (unit + integration)
cargo run                          # cluster config summary
cargo run --release --bin bench_report  # benchmark comparison
cargo bench                        # criterion benchmarks
```

## Protocols

| Protocol | Fault model | Tolerance | Phases per commit | Message complexity |
|----------|------------|-----------|-------------------|--------------------|
| PBFT | Byzantine (arbitrary) | f < n/3 | 3 (pre-prepare → prepare → commit) | O(n²) |
| Raft | Crash (fail-stop) | f < n/2 | 2 (append → commit) | O(n) |

## License

MIT OR Apache-2.0
