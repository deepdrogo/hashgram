#![no_main]
use hashgram_net::NetworkIdentity;
use hashgram_proto::frame::decode_bounded;
use hashgram_proto::limits::MAX_GOSSIP_FRAME;
use hashgram_proto::pb;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let id = NetworkIdentity::devnet("9348af00681eecefb8d6329d5ba101c13bc3c8943f2c610295026f6503654287");
    if let Ok(g) = decode_bounded::<pb::Gossip>(data, MAX_GOSSIP_FRAME) {
        let now = 1_700_000_000;
        match g.body {
            Some(pb::gossip::Body::SocialEvent(e)) => { let _ = hashgram_proto::validate::social_event(&e, now); let _ = hashgram_proto::signing::verify_social_event(&id, &e); }
            Some(pb::gossip::Body::NodeAnnounce(a)) => { let _ = hashgram_proto::validate::node_announce(&a, now); let _ = hashgram_proto::signing::verify_node_announce(&id, &a); }
            Some(pb::gossip::Body::Attestation(a)) => { let _ = hashgram_proto::validate::attestation(&a, now); let _ = hashgram_proto::signing::verify_attestation(&id, &a); }
            _ => {}
        }
    }
});
