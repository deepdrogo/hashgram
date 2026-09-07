#![no_main]
use hashgram_proto::pb;
use libfuzzer_sys::fuzz_target;
use prost::Message;

fuzz_target!(|data: &[u8]| {
    if let Ok(m) = pb::BlobManifest::decode(data) {
        let _ = hashgram_proto::blob::validate_manifest(&m);
        let _ = hashgram_proto::blob::cid(&m);
        let _ = hashgram_proto::blob::chunk_range(m.size, 0);
        let _ = hashgram_proto::blob::chunk_matches(&m, 0, data);
    }
});
