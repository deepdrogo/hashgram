#![no_main]
use hashgram_proto::pb;
use libfuzzer_sys::fuzz_target;
use prost::Message;

fuzz_target!(|data: &[u8]| {
    if let Ok(e) = pb::Envelope::decode(data) {
        let _ = hashgram_proto::validate::envelope(&e, 1_700_000_000);
        let _ = hashgram_proto::signing::envelope_id(&e);
    }
});
