//! Stable-toolchain fuzz smoke: random and mutated inputs through every
//! decoder and validator. Not a substitute for `cargo fuzz` (see
//! `node/fuzz`), but it runs in every CI job and catches the panics that
//! matter most: an unwrap on attacker bytes.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use hashgram_net::{Handshake, NetworkIdentity};
use hashgram_proto::frame::{decode_bounded, decode_frame};
use hashgram_proto::limits::{MAX_GOSSIP_FRAME, MAX_RPC_FRAME};
use hashgram_proto::{blob, chat, pb, signing, validate};
use prost::Message;

const GENESIS: &str = "9348af00681eecefb8d6329d5ba101c13bc3c8943f2c610295026f6503654287";

/// xorshift, deterministic so a failure reproduces.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn bytes(&mut self, max: usize) -> Vec<u8> {
        let n = (self.next() % (max as u64 + 1)) as usize;
        (0..n).map(|_| self.next() as u8).collect()
    }
    fn mutate(&mut self, base: &[u8]) -> Vec<u8> {
        let mut v = base.to_vec();
        if v.is_empty() {
            return v;
        }
        for _ in 0..(1 + self.next() % 4) {
            let i = (self.next() as usize) % v.len();
            match self.next() % 4 {
                0 => v[i] ^= 1 << (self.next() % 8),
                1 => v[i] = self.next() as u8,
                2 => {
                    v.remove(i);
                    if v.is_empty() {
                        return v;
                    }
                }
                _ => v.insert(i, self.next() as u8),
            }
        }
        v
    }
}

fn exercise(id: &NetworkIdentity, data: &[u8]) {
    let now = 1_700_000_000;
    let _ = decode_frame::<pb::Request>(data, MAX_RPC_FRAME);
    let _ = decode_frame::<pb::Response>(data, MAX_RPC_FRAME);
    if let Ok(g) = decode_bounded::<pb::Gossip>(data, MAX_GOSSIP_FRAME) {
        match g.body {
            Some(pb::gossip::Body::SocialEvent(e)) => {
                let _ = validate::social_event(&e, now);
                let _ = signing::verify_social_event(id, &e);
            }
            Some(pb::gossip::Body::NodeAnnounce(a)) => {
                let _ = validate::node_announce(&a, now);
                let _ = signing::verify_node_announce(id, &a);
            }
            Some(pb::gossip::Body::Attestation(a)) => {
                let _ = validate::attestation(&a, now);
                let _ = signing::verify_attestation(id, &a);
            }
            _ => {}
        }
    }
    if let Ok(e) = pb::SocialEvent::decode(data) {
        let _ = validate::social_event(&e, now);
        let _ = signing::verify_social_event(id, &e);
    }
    if let Ok(m) = pb::BlobManifest::decode(data) {
        let _ = blob::validate_manifest(&m);
        let _ = blob::cid(&m);
        let _ = blob::chunk_matches(&m, 0, data);
    }
    if let Ok(e) = pb::Envelope::decode(data) {
        let _ = validate::envelope(&e, now);
        let _ = signing::envelope_id(&e);
    }
    if let Ok(f) = pb::MailboxFetch::decode(data) {
        let _ = validate::mailbox_fetch(&f, now);
        let _ = signing::verify_mailbox_fetch(id, &f);
    }
    if let Ok(r) = pb::ServiceReceipt::decode(data) {
        let _ = signing::verify_receipt(id, &r);
    }
    if let Ok(h) = pb::Handshake::decode(data) {
        let mut magic = [0u8; 4];
        if h.network_magic.len() == 4 {
            magic.copy_from_slice(&h.network_magic);
        }
        let _ = id.verify_handshake(&Handshake {
            network_magic: magic,
            network_id: h.network_id,
            chain_id: h.chain_id,
            genesis_hash: h.genesis_hash,
            protocol_major_version: h.protocol_major_version,
        });
    }
    if let Ok(m) = chat::ChatMessage::decode(data) {
        let _ = chat::ChatKind::try_from(m.kind);
    }
}

#[test]
fn random_and_mutated_inputs_never_panic() {
    let id = NetworkIdentity::devnet(GENESIS);
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);

    // Valid seeds, so mutations explore the interesting neighbourhood.
    let signer = hashgram_proto::Ed25519Signer::from_secret([3u8; 32]);
    let mut ev = pb::SocialEvent {
        network_id: id.network_id.clone(),
        version: 1,
        r#type: "POST_CREATE".into(),
        author: "hash1qpzry9x8gf2tvdw0s3jn54khce6mua7lqqqqqqqq".into(),
        timestamp: 1_700_000_000,
        payload: pb::PostCreate {
            text: "hi".into(),
            ..Default::default()
        }
        .encode_to_vec(),
        ..Default::default()
    };
    signing::sign_social_event(&id, &signer, &mut ev).expect("sign");
    let manifest = blob::manifest_for(b"hello", "text/plain", false).expect("manifest");
    let seeds: Vec<Vec<u8>> = vec![
        pb::Gossip {
            body: Some(pb::gossip::Body::SocialEvent(ev.clone())),
        }
        .encode_to_vec(),
        ev.encode_to_vec(),
        manifest.encode_to_vec(),
        hashgram_proto::frame::encode_frame(
            &pb::Request {
                body: Some(pb::request::Body::BlobGetManifest(pb::BlobGetManifest {
                    cid: vec![1; 32],
                })),
            },
            MAX_RPC_FRAME,
        )
        .expect("frame"),
        pb::Handshake {
            network_magic: b"HGD1".to_vec(),
            network_id: "hashgram-devnet".into(),
            chain_id: "hashgram-devnet-1".into(),
            genesis_hash: GENESIS.into(),
            protocol_major_version: 1,
            ..Default::default()
        }
        .encode_to_vec(),
    ];

    for _ in 0..4000 {
        let data = rng.bytes(512);
        exercise(&id, &data);
    }
    for seed in &seeds {
        for _ in 0..2000 {
            let data = rng.mutate(seed);
            exercise(&id, &data);
        }
    }
}
