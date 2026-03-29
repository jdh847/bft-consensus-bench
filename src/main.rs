use bft_consensus_bench::consensus::hotstuff::HotStuffNode;
use bft_consensus_bench::consensus::pbft::PbftNode;
use bft_consensus_bench::consensus::raft::RaftNode;
use bft_consensus_bench::consensus::ConsensusNode;
use bft_consensus_bench::types::NodeId;

fn main() {
    println!("bft-consensus-bench v{}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("Protocols:");
    println!("  PBFT     — Byzantine fault tolerant, O(n²) messages (f < n/3)");
    println!("  HotStuff — Byzantine fault tolerant, O(n) messages  (f < n/3)");
    println!("  Raft     — Crash fault tolerant, O(n) messages      (f < n/2)");
    println!();

    for n in [3, 5, 7] {
        let pbft = PbftNode::new(NodeId(0), n);
        let hs = HotStuffNode::new(NodeId(0), n);
        let raft = RaftNode::new(NodeId(0), n);

        println!(
            "  n={n}:  PBFT f≤{} | HotStuff f≤{} | Raft f≤{} crash",
            pbft.fault_tolerance(),
            hs.fault_tolerance(),
            raft.fault_tolerance(),
        );
    }

    println!();
    println!("Byzantine simulation:");
    println!("  Fault types: digest corruption, equivocation, selective message drop");
    println!("  Configurable per-node via FaultConfig::with_byzantine()");
    println!();
    println!("Run benchmarks:        cargo bench");
    println!("Run tests:             cargo test");
    println!("Run cluster sim:       cargo test -- --nocapture cluster");
    println!("Run Byzantine tests:   cargo test -- byzantine");
    println!("Run HotStuff tests:    cargo test -- hotstuff");
    println!("Byzantine overhead:    cargo run --release --bin bench_report");
}
