#![no_main]
use hashgram_net::NetworkIdentity;
use hashgram_proto::pb;
use libfuzzer_sys::fuzz_target;
use prost::Message;

fuzz_target!(|data: &[u8]| {
    let id = NetworkIdentity::devnet("9348af00681eecefb8d6329d5ba101c13bc3c8943f2c610295026f6503654287");
    if let Ok(e) = pb::SocialEvent::decode(data) {
        let _ = hashgram_proto::validate::social_event(&e, 1_700_000_000);
        let _ = hashgram_proto::signing::verify_social_event(&id, &e);
        let _ = hashgram_proto::signing::social_event_id(&id, &e);
    }
});
