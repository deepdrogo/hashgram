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

    // The application package (hashgram.app.v1) is compiled on its own
    // first; the chat package embeds it, and is told to refer to it as
    // `crate::app` rather than the nested `super::super::app::v1` path
    // prost would otherwise assume, because this crate exposes the packages
    // as flat modules (`pb`, `chat`, `app`).
    let app_files = ["hashgram/app/v1/app.proto"];
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

    for f in app_files.iter().chain(files.iter()) {
        println!("cargo:rerun-if-changed={}", proto_root.join(f).display());
    }

    let app_descriptors =
        protox::compile(app_files.iter().map(|f| proto_root.join(f)), [&proto_root])?;
    prost_build::Config::new()
        .type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]")
        .compile_fds(app_descriptors)?;

    let descriptors = protox::compile(files.iter().map(|f| proto_root.join(f)), [&proto_root])?;
    prost_build::Config::new()
        .type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]")
        .extern_path(".hashgram.app.v1", "crate::app")
        .compile_fds(descriptors)?;

    Ok(())
}
