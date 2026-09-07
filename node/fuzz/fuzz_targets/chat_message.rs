#![no_main]
use hashgram_proto::chat;
use libfuzzer_sys::fuzz_target;
use prost::Message;

fuzz_target!(|data: &[u8]| {
    if let Ok(m) = chat::ChatMessage::decode(data) {
        let _ = chat::ChatKind::try_from(m.kind);
        let _ = m.encode_to_vec();
    }
});
