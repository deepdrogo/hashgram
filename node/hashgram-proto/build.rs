//! Compiles the Hashgram P2P protobuf definitions.
//!
//! Uses protox, a pure-Rust protobuf compiler, so a build does not depend on
//! a system `protoc` whose version could differ between the release builder
//! and an operator verifying the checksum. The definitions themselves live in
//! the repository's `proto/` tree alongside the chain's, so Go and Rust are
//! generated from one source.

use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let proto_root = manifest_dir
        .join("../../proto")
        .canonicalize()
        .map_err(|e| {
            format!(
                "proto root not found relative to {}: {e}",
                manifest_dir.display()
            )
        })?;

    let files = [
        "hashgram/p2p/v1/handshake.proto",
        "hashgram/p2p/v1/envelope.proto",
        "hashgram/p2p/v1/social.proto",
        "hashgram/p2p/v1/blob.proto",
        "hashgram/p2p/v1/announce.proto",
        "hashgram/p2p/v1/attestation.proto",
        "hashgram/p2p/v1/rpc.proto",
        "hashgram/chat/v1/chat.proto",
    ];

    for f in &files {
        println!("cargo:rerun-if-changed={}", proto_root.join(f).display());
    }

    let descriptors = protox::compile(files.iter().map(|f| proto_root.join(f)), [&proto_root])?;

    prost_build::Config::new()
        .type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]")
        .compile_fds(descriptors)?;

    Ok(())
}
