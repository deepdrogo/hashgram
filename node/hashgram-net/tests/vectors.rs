//! Cross-language parity with the Go implementation.
//!
//! These tests check the Rust signing framing against vectors generated from
//! `app/params` by `tools/signing-vectors`. That direction matters: the
//! vectors come from the code the chain actually runs, so this is a parity
//! test rather than a second reading of the specification. A specification can
//! be misread twice in the same way; a generated vector cannot.
//!
//! Regenerate after any framing change:
//!
//! ```sh
//! go run ./tools/signing-vectors > node/testdata/signing-vectors.json
//! ```
//!
//! If a framing change makes these fail, that is correct behaviour: the change
//! invalidates every signature ever produced, and both implementations have to
//! adopt it deliberately.

// This is a test crate. The workspace denies panicking constructs to protect
// production code that parses attacker-controlled bytes; a test that unwraps a
// fixture it just loaded is not that, and a panic here is the test failing.
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;

use hashgram_net::{NetworkIdentity, SigningPurpose};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Vector {
    purpose: String,
    domain: String,
    payload_hex: String,
    preimage_hex: String,
    digest_hex: String,
}

#[derive(Debug, Deserialize)]
struct IdentityVectors {
    network_name: String,
    network_id: String,
    chain_id: String,
    network_magic: String,
    network_magic_hex: String,
    protocol_major_version: u32,
    genesis_hash: String,
    vectors: Vec<Vector>,
}

fn load() -> BTreeMap<String, IdentityVectors> {
    let raw = include_str!("../../testdata/signing-vectors.json");
    serde_json::from_str(raw).expect("signing-vectors.json is not valid JSON")
}

fn identity_for(name: &str, genesis: &str) -> NetworkIdentity {
    match name {
        "mainnet" => NetworkIdentity::mainnet(genesis),
        "devnet" => NetworkIdentity::devnet(genesis),
        other => panic!("the vector file names an unknown network {other:?}"),
    }
}

#[test]
fn identity_constants_match_go() {
    let all = load();
    assert_eq!(all.len(), 2, "expected mainnet and devnet vectors");

    for (name, want) in &all {
        let got = identity_for(name, &want.genesis_hash);

        assert_eq!(got.network_name, want.network_name, "{name} network_name");
        assert_eq!(got.network_id, want.network_id, "{name} network_id");
        assert_eq!(got.chain_id, want.chain_id, "{name} chain_id");
        assert_eq!(
            got.protocol_major_version, want.protocol_major_version,
            "{name} protocol_major_version"
        );
        assert_eq!(got.genesis_hash, want.genesis_hash, "{name} genesis_hash");

        assert_eq!(
            hex::encode(got.network_magic),
            want.network_magic_hex,
            "{name} network magic bytes"
        );
        assert_eq!(
            String::from_utf8_lossy(&got.network_magic),
            want.network_magic,
            "{name} network magic as text"
        );
    }
}

#[test]
fn signing_domains_match_go() {
    let all = load();
    let mut checked = 0;

    for (name, want) in &all {
        let id = identity_for(name, &want.genesis_hash);

        for vector in &want.vectors {
            let purpose: SigningPurpose = vector
                .purpose
                .parse()
                .unwrap_or_else(|e| panic!("{name}: {e}"));

            assert_eq!(
                id.signing_domain(purpose),
                vector.domain,
                "{name}/{} domain",
                vector.purpose
            );
            checked += 1;
        }
    }

    assert!(checked >= 90, "only {checked} domains were checked");
}

#[test]
fn signing_preimages_match_go_byte_for_byte() {
    let all = load();
    let mut checked = 0;

    for (name, want) in &all {
        let id = identity_for(name, &want.genesis_hash);

        for vector in &want.vectors {
            let purpose: SigningPurpose = vector
                .purpose
                .parse()
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            let payload = hex::decode(&vector.payload_hex).expect("payload_hex is not valid hex");

            let got = hex::encode(id.signing_preimage(purpose, &payload));

            assert_eq!(
                got, vector.preimage_hex,
                "{name}/{} preimage for payload {:?} diverged from Go.\n\
                 A divergence here means a signature produced by one \
                 implementation cannot be verified by the other.",
                vector.purpose, vector.payload_hex
            );
            checked += 1;
        }
    }

    assert!(checked >= 90, "only {checked} preimages were checked");
}

#[test]
fn signing_digests_match_go() {
    let all = load();
    let mut checked = 0;

    for (name, want) in &all {
        let id = identity_for(name, &want.genesis_hash);

        for vector in &want.vectors {
            let purpose: SigningPurpose = vector
                .purpose
                .parse()
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            let payload = hex::decode(&vector.payload_hex).expect("payload_hex is not valid hex");

            let got = hex::encode(id.signing_digest(purpose, &payload));

            assert_eq!(
                got, vector.digest_hex,
                "{name}/{} digest for payload {:?} diverged from Go",
                vector.purpose, vector.payload_hex
            );
            checked += 1;
        }
    }

    assert!(checked >= 90, "only {checked} digests were checked");
}

#[test]
fn every_purpose_appears_in_the_vectors() {
    // A purpose added to one implementation and not the other is exactly the
    // drift these vectors exist to catch, and it would otherwise pass
    // silently because the loop above only checks what the file contains.
    let all = load();

    for (name, want) in &all {
        let present: std::collections::BTreeSet<&str> =
            want.vectors.iter().map(|v| v.purpose.as_str()).collect();

        for purpose in hashgram_net::ALL_PURPOSES {
            assert!(
                present.contains(purpose.as_str()),
                "{name}: purpose {purpose} exists in Rust but has no Go vector. \
                 Regenerate with: go run ./tools/signing-vectors"
            );
        }

        for purpose in &present {
            assert!(
                purpose.parse::<SigningPurpose>().is_ok(),
                "{name}: Go defines purpose {purpose:?} which Rust does not know"
            );
        }
    }
}

#[test]
fn mainnet_and_devnet_digests_never_collide() {
    // Cross-checked against the generated vectors rather than computed twice
    // in Rust, so this compares what both implementations actually produce.
    let all = load();

    let mainnet = all.get("mainnet").expect("mainnet vectors");
    let devnet = all.get("devnet").expect("devnet vectors");

    let mainnet_digests: std::collections::HashSet<&str> = mainnet
        .vectors
        .iter()
        .map(|v| v.digest_hex.as_str())
        .collect();

    for vector in &devnet.vectors {
        assert!(
            !mainnet_digests.contains(vector.digest_hex.as_str()),
            "a devnet digest for {} collides with a mainnet digest; \
             a devnet signature would verify on mainnet",
            vector.purpose
        );
    }
}

#[test]
fn payloads_differing_by_one_byte_produce_different_preimages() {
    // "ab" and "abc" are in the vector set specifically because a missing
    // length prefix makes them collide once concatenated with a domain.
    let all = load();

    for (name, want) in &all {
        let ab: Vec<&Vector> = want
            .vectors
            .iter()
            .filter(|v| v.payload_hex == hex::encode("ab"))
            .collect();
        let abc: Vec<&Vector> = want
            .vectors
            .iter()
            .filter(|v| v.payload_hex == hex::encode("abc"))
            .collect();

        assert!(
            !ab.is_empty() && !abc.is_empty(),
            "{name}: missing fixtures"
        );

        for (a, b) in ab.iter().zip(abc.iter()) {
            assert_eq!(a.purpose, b.purpose, "fixture ordering changed");
            assert_ne!(
                a.preimage_hex, b.preimage_hex,
                "{name}/{}: payload length is not committed to",
                a.purpose
            );
        }
    }
}
