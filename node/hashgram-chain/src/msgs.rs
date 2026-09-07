//! Message constructors.
//!
//! Every Hashgram transaction a client sends, as a protobuf `Any` with the
//! type URL the chain registers. Type URLs are `"/" + full proto name`; a
//! typo here is a `tx parse error` at broadcast, which the tests below catch
//! against the proto package names.

use cosmrs::Any;
use prost::Message;

use crate::pb;

fn any<M: Message>(type_url: &str, m: &M) -> Any {
    Any {
        type_url: type_url.to_owned(),
        value: m.encode_to_vec(),
    }
}

/// `MsgSend` from `x/bank`.
#[must_use]
pub fn bank_send(from: &str, to: &str, amount_uhash: u128) -> Any {
    let msg = cosmrs::proto::cosmos::bank::v1beta1::MsgSend {
        from_address: from.to_owned(),
        to_address: to.to_owned(),
        amount: vec![cosmrs::proto::cosmos::base::v1beta1::Coin {
            denom: crate::wallet::DENOM.to_owned(),
            amount: amount_uhash.to_string(),
        }],
    };
    // cosmos-sdk-proto types implement the prost 0.13 trait; ours 0.14.
    Any {
        type_url: "/cosmos.bank.v1beta1.MsgSend".to_owned(),
        value: prost013::Message::encode_to_vec(&msg),
    }
}

/// `MsgCreateIdentity`.
#[must_use]
pub fn create_identity(m: &pb::identity::MsgCreateIdentity) -> Any {
    any("/hashgram.identity.v1.MsgCreateIdentity", m)
}

/// `MsgAddDevice`.
#[must_use]
pub fn add_device(m: &pb::identity::MsgAddDevice) -> Any {
    any("/hashgram.identity.v1.MsgAddDevice", m)
}

/// `MsgRevokeDevice`.
#[must_use]
pub fn revoke_device(m: &pb::identity::MsgRevokeDevice) -> Any {
    any("/hashgram.identity.v1.MsgRevokeDevice", m)
}

/// `MsgRotateRootKey`.
#[must_use]
pub fn rotate_root_key(m: &pb::identity::MsgRotateRootKey) -> Any {
    any("/hashgram.identity.v1.MsgRotateRootKey", m)
}

/// `MsgSetRecoveryConfig`.
#[must_use]
pub fn set_recovery_config(m: &pb::identity::MsgSetRecoveryConfig) -> Any {
    any("/hashgram.identity.v1.MsgSetRecoveryConfig", m)
}

/// `MsgRegister` from `x/username`.
#[must_use]
pub fn register_username(m: &pb::username::MsgRegister) -> Any {
    any("/hashgram.username.v1.MsgRegister", m)
}

/// `MsgClaimWelcome` from `x/welcome`.
#[must_use]
pub fn claim_welcome(m: &pb::welcome::MsgClaimWelcome) -> Any {
    any("/hashgram.welcome.v1.MsgClaimWelcome", m)
}

/// `MsgRegisterProvider` from `x/serviceproof`.
#[must_use]
pub fn register_provider(m: &pb::serviceproof::MsgRegisterProvider) -> Any {
    any("/hashgram.serviceproof.v1.MsgRegisterProvider", m)
}

/// `MsgUpdateProvider`.
#[must_use]
pub fn update_provider(m: &pb::serviceproof::MsgUpdateProvider) -> Any {
    any("/hashgram.serviceproof.v1.MsgUpdateProvider", m)
}

/// `MsgSubmitReceipts`.
#[must_use]
pub fn submit_receipts(m: &pb::serviceproof::MsgSubmitReceipts) -> Any {
    any("/hashgram.serviceproof.v1.MsgSubmitReceipts", m)
}

/// `MsgAnswerChallenge`.
#[must_use]
pub fn answer_challenge(m: &pb::serviceproof::MsgAnswerChallenge) -> Any {
    any("/hashgram.serviceproof.v1.MsgAnswerChallenge", m)
}

/// `MsgAssignStorage`.
#[must_use]
pub fn assign_storage(m: &pb::serviceproof::MsgAssignStorage) -> Any {
    any("/hashgram.serviceproof.v1.MsgAssignStorage", m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_urls_match_the_proto_packages() {
        // prost names the generated struct after the proto message and the
        // module after the package, so the URL must be derivable from both.
        let a = create_identity(&pb::identity::MsgCreateIdentity::default());
        assert_eq!(a.type_url, "/hashgram.identity.v1.MsgCreateIdentity");
        let b = submit_receipts(&pb::serviceproof::MsgSubmitReceipts::default());
        assert_eq!(b.type_url, "/hashgram.serviceproof.v1.MsgSubmitReceipts");
        let c = bank_send("hash1a", "hash1b", 100);
        assert_eq!(c.type_url, "/cosmos.bank.v1beta1.MsgSend");
        let decoded: cosmrs::proto::cosmos::bank::v1beta1::MsgSend =
            prost013::Message::decode(c.value.as_slice()).unwrap();
        assert_eq!(decoded.amount[0].amount, "100");
        assert_eq!(decoded.amount[0].denom, "uhash");
    }
}
