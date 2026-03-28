# bft-consensus-bench

Benchmarking framework for comparing Byzantine fault-tolerant (PBFT) and crash fault-tolerant (Raft) consensus protocols under identical workloads.

## Why

Most consensus comparisons in the literature report numbers from different codebases, different hardware, and different workload generators. This makes apples-to-apples comparison difficult.

This project implements both protocols behind a common `ConsensusNode` trait, runs them on the same simulated network with the same payloads, and measures throughput, latency, and fault tolerance side-by-side.

## Status

**Work in progress.** Current state:

- [x] Core types (NodeId, Payload, Digest)
- [x] `ConsensusNode` trait
- [x] Simulated network transport with configurable latency/jitter
- [x] PBFT node — pre-prepare, prepare, commit phases
- [x] Raft node — log replication, leader election (vote handling)
- [ ] End-to-end cluster orchestration
- [ ] View change (PBFT) / election timeout (Raft)
- [ ] Fault injection (message drops, partitions, Byzantine behaviour)
- [ ] Criterion benchmarks with cluster runs
- [ ] Results and charts

## Structure

```
src/
├── types/          # NodeId, Payload, Digest, sequence/view numbers
├── consensus/
│   ├── mod.rs      # ConsensusNode trait
│   ├── pbft/       # PBFT implementation
│   └── raft/       # Raft implementation
└── network/
    ├── mod.rs      # Message types (PBFT + Raft envelopes)
    └── transport.rs # Simulated in-process network
```

## Build & test

```
cargo build
cargo test
cargo run       # prints cluster configuration summary
cargo bench     # criterion benchmarks (WIP)
```

## Protocols

| Protocol | Fault model | Tolerance | Phases per commit |
|----------|------------|-----------|-------------------|
| PBFT | Byzantine (arbitrary) | f < n/3 | 3 (pre-prepare → prepare → commit) |
| Raft | Crash (fail-stop) | f < n/2 | 2 (append → commit) |

## License

MIT OR Apache-2.0
