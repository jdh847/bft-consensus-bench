use bft_consensus_bench::consensus::pbft::PbftNode;
use bft_consensus_bench::consensus::raft::RaftNode;
use bft_consensus_bench::consensus::ConsensusNode;
use bft_consensus_bench::types::NodeId;

fn main() {
    println!("bft-consensus-bench v{}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("Protocols:");
    println!("  PBFT  — Byzantine fault tolerant (tolerates f < n/3 malicious nodes)");
    println!("  Raft  — Crash fault tolerant (tolerates f < n/2 crashed nodes)");
    println!();

    for n in [3, 5, 7] {
        let pbft = PbftNode::new(NodeId(0), n);
        let raft = RaftNode::new(NodeId(0), n);

        println!(
            "  n={n}:  PBFT tolerates {} Byzantine | Raft tolerates {} crash",
            pbft.fault_tolerance(),
            raft.fault_tolerance(),
        );
    }

    println!();
    println!("Run benchmarks:  cargo bench");
    println!("Run tests:       cargo test");
    println!("Run cluster sim: cargo test -- --nocapture cluster");
}
