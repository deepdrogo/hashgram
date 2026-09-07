#![no_main]
use hashgram_net::{Handshake, NetworkIdentity};
use hashgram_proto::pb;
use libfuzzer_sys::fuzz_target;
use prost::Message;

fuzz_target!(|data: &[u8]| {
    let id = NetworkIdentity::mainnet("9348af00681eecefb8d6329d5ba101c13bc3c8943f2c610295026f6503654287");
    if let Ok(h) = pb::Handshake::decode(data) {
        let mut magic = [0u8; 4];
        if h.network_magic.len() == 4 { magic.copy_from_slice(&h.network_magic); }
        let hs = Handshake { network_magic: magic, network_id: h.network_id, chain_id: h.chain_id, genesis_hash: h.genesis_hash, protocol_major_version: h.protocol_major_version };
        let _ = id.verify_handshake(&hs);
    }
});
