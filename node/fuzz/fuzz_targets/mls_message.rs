#![no_main]
//! A store node hands a device whatever bytes are in its mailbox. Processing
//! hostile bytes through OpenMLS must fail cleanly.
use hashgram_mls::{GroupMeta, MlsClient};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut c = MlsClient::new("hash1fuzz", &[7u8; 32]).expect("client");
    let _ = c.create_group(GroupMeta::default());
    let _ = c.process(data);
    let _ = c.join(data, GroupMeta::default());
    let _ = c.add_members(&[0u8; 8], &[data.to_vec()]);
});
