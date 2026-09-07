#![no_main]
//! Every byte string a peer could send on /hashgram/rpc/1 must be refused or
//! decoded, never panic. Decoding is followed by the classification and
//! validation paths the node runs on a real request.
use hashgram_proto::frame::{decode_frame, decode_bounded};
use hashgram_proto::limits::MAX_RPC_FRAME;
use hashgram_proto::pb;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = decode_frame::<pb::Request>(data, MAX_RPC_FRAME);
    let _ = decode_frame::<pb::Response>(data, MAX_RPC_FRAME);
    if let Ok(req) = decode_bounded::<pb::Request>(data, MAX_RPC_FRAME) {
        let now = 1_700_000_000;
        match req.body {
            Some(pb::request::Body::MailboxFetch(f)) => { let _ = hashgram_proto::validate::mailbox_fetch(&f, now); }
            Some(pb::request::Body::MailboxAck(a)) => { let _ = hashgram_proto::validate::mailbox_ack(&a, now); }
            Some(pb::request::Body::MailboxPut(p)) => { if let Some(e) = p.envelope { let _ = hashgram_proto::validate::envelope(&e, now); let _ = hashgram_proto::signing::envelope_id(&e); } }
            Some(pb::request::Body::KeyPackagePublish(k)) => { let _ = hashgram_proto::validate::key_package(&k, now); }
            Some(pb::request::Body::BlobPutManifest(p)) => { let _ = hashgram_proto::validate::blob_put_manifest(&p, now); if let Some(m) = p.manifest { let _ = hashgram_proto::blob::validate_manifest(&m); let _ = hashgram_proto::blob::cid(&m); } }
            Some(pb::request::Body::EventPublish(p)) => { if let Some(e) = p.event { let _ = hashgram_proto::validate::social_event(&e, now); } }
            _ => {}
        }
    }
});
