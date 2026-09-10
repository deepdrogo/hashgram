//! Hostile bytes through OpenMLS entry points must fail, not panic.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use hashgram_mls::{GroupMeta, MlsClient};

#[test]
fn hostile_mls_bytes_do_not_panic() {
    let mut c = MlsClient::new("hash1fuzz", &[7u8; 32]).unwrap();
    let gid = c.create_group(GroupMeta::default()).unwrap();
    let mut x = 0x2545_f491_4f6c_dd1du64;
    for _ in 0..500 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let n = (x % 300) as usize;
        let data: Vec<u8> = (0..n)
            .map(|i| (x.rotate_left(i as u32 % 63) & 0xff) as u8)
            .collect();
        let _ = c.process(&data);
        let _ = c.join(&data, GroupMeta::default());
        let _ = c.add_members(&gid, std::slice::from_ref(&data));
    }
    // A real message from another device still works afterwards.
    let mut other = MlsClient::new("hash1other", &[8u8; 32]).unwrap();
    let kp = other.key_package().unwrap();
    let (_, welcome) = c.add_members(&gid, &[kp]).unwrap();
    other.join(&welcome, GroupMeta::default()).unwrap();
    let ct = c.encrypt(&gid, b"still fine").unwrap();
    assert!(matches!(
        other.process(&ct).unwrap(),
        hashgram_mls::Inbound::Application { .. }
    ));
}
