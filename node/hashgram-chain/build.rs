//! Compiles the Hashgram chain module protobufs for the client.
//!
//! Only the message and query types are generated (no gRPC services): the
//! client talks to the REST gateway with JSON for queries and submits
//! protobuf-encoded transactions, so it needs the `Msg*` types and nothing
//! else. gogoproto and cosmos_proto options are parsed and ignored.

use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let proto_root = manifest_dir.join("../../proto").canonicalize()?;

    let files = [
        "hashgram/identity/v1/identity.proto",
        "hashgram/identity/v1/tx.proto",
        "hashgram/username/v1/tx.proto",
        "hashgram/welcome/v1/tx.proto",
        "hashgram/serviceproof/v1/evidence.proto",
        "hashgram/serviceproof/v1/provider.proto",
        "hashgram/serviceproof/v1/tx.proto",
    ];
    for f in &files {
        println!("cargo:rerun-if-changed={}", proto_root.join(f).display());
    }
    let descriptors = protox::compile(files.iter().map(|f| proto_root.join(f)), [&proto_root])?;
    prost_build::Config::new().compile_fds(descriptors)?;
    Ok(())
}
