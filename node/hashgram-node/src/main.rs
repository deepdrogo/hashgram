//! The Hashgram peer-to-peer node.
//!
//! Runs the libp2p transport, the Hashgram handshake, peer scoring and
//! connection limits. The messaging, social and storage protocols that ride
//! on top are the remainder of Phase 2.
//!
//! Configuration is read from `/etc/hashgram/node.toml` and validated before
//! anything starts listening, so a typo is a startup failure with a message
//! naming the field rather than a runtime surprise on the first connection.

#![forbid(unsafe_code)]

fn main() {
    // Deliberately minimal until the swarm is wired in. A binary that starts,
    // logs that it is incomplete and exits is more honest than one that
    // appears to run a node it does not yet run.
    eprintln!(
        "hashgram-node: the P2P swarm is not wired in yet.\n\
         \n\
         What is implemented and tested in this workspace:\n\
         \x20 hashgram-net   network identity, signing domains, the peer handshake\n\
         \x20                that verifies the genesis hash CometBFT does not\n\
         \x20 hashgram-p2p   peer scoring, connection limits, config validation\n\
         \n\
         See docs/PHASE1_REPORT.md for what runs today, and node/README.md\n\
         for what remains."
    );
    std::process::exit(1);
}
