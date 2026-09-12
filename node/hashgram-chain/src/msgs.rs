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

fn any013<M: prost013::Message>(type_url: &str, m: &M) -> Any {
    Any {
        type_url: type_url.to_owned(),
        value: prost013::Message::encode_to_vec(m),
    }
}

fn coin013(amount_uhash: u128) -> cosmrs::proto::cosmos::base::v1beta1::Coin {
    cosmrs::proto::cosmos::base::v1beta1::Coin {
        denom: crate::wallet::DENOM.to_owned(),
        amount: amount_uhash.to_string(),
    }
}

/// `MsgDelegate` from `x/staking`. Unbonding takes 21 days; the UI says so
/// before this is built.
#[must_use]
pub fn delegate(delegator: &str, validator: &str, amount_uhash: u128) -> Any {
    any013(
        "/cosmos.staking.v1beta1.MsgDelegate",
        &cosmrs::proto::cosmos::staking::v1beta1::MsgDelegate {
            delegator_address: delegator.to_owned(),
            validator_address: validator.to_owned(),
            amount: Some(coin013(amount_uhash)),
        },
    )
}

/// `MsgUndelegate` from `x/staking` (21-day unbonding).
#[must_use]
pub fn undelegate(delegator: &str, validator: &str, amount_uhash: u128) -> Any {
    any013(
        "/cosmos.staking.v1beta1.MsgUndelegate",
        &cosmrs::proto::cosmos::staking::v1beta1::MsgUndelegate {
            delegator_address: delegator.to_owned(),
            validator_address: validator.to_owned(),
            amount: Some(coin013(amount_uhash)),
        },
    )
}

/// `MsgBeginRedelegate` from `x/staking`.
#[must_use]
pub fn redelegate(
    delegator: &str,
    from_validator: &str,
    to_validator: &str,
    amount_uhash: u128,
) -> Any {
    any013(
        "/cosmos.staking.v1beta1.MsgBeginRedelegate",
        &cosmrs::proto::cosmos::staking::v1beta1::MsgBeginRedelegate {
            delegator_address: delegator.to_owned(),
            validator_src_address: from_validator.to_owned(),
            validator_dst_address: to_validator.to_owned(),
            amount: Some(coin013(amount_uhash)),
        },
    )
}

/// `MsgWithdrawDelegatorReward` from `x/distribution`.
#[must_use]
pub fn withdraw_rewards(delegator: &str, validator: &str) -> Any {
    any013(
        "/cosmos.distribution.v1beta1.MsgWithdrawDelegatorReward",
        &cosmrs::proto::cosmos::distribution::v1beta1::MsgWithdrawDelegatorReward {
            delegator_address: delegator.to_owned(),
            validator_address: validator.to_owned(),
        },
    )
}

/// A governance vote option, as `x/gov` v1 numbers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoteOption {
    /// Yes.
    Yes,
    /// Abstain.
    Abstain,
    /// No.
    No,
    /// No with veto (33.4 % of these rejects and burns the deposit).
    NoWithVeto,
}

impl VoteOption {
    /// Parses `yes`, `abstain`, `no`, `no_with_veto` / `veto`.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "yes" => Some(Self::Yes),
            "abstain" => Some(Self::Abstain),
            "no" => Some(Self::No),
            "no_with_veto" | "nowithveto" | "veto" => Some(Self::NoWithVeto),
            _ => None,
        }
    }

    fn code(self) -> i32 {
        match self {
            Self::Yes => 1,
            Self::Abstain => 2,
            Self::No => 3,
            Self::NoWithVeto => 4,
        }
    }
}

/// `MsgVote` from `x/gov` v1.
#[must_use]
pub fn vote(voter: &str, proposal_id: u64, option: VoteOption) -> Any {
    any013(
        "/cosmos.gov.v1.MsgVote",
        &cosmrs::proto::cosmos::gov::v1::MsgVote {
            proposal_id,
            voter: voter.to_owned(),
            option: option.code(),
            metadata: String::new(),
        },
    )
}

/// `MsgRenew` from `x/username`.
#[must_use]
pub fn renew_username(owner: &str, name: &str) -> Any {
    any(
        "/hashgram.username.v1.MsgRenew",
        &pb::username::MsgRenew {
            owner: owner.to_owned(),
            name: name.to_owned(),
        },
    )
}

/// `MsgTransfer` from `x/username`.
#[must_use]
pub fn transfer_username(owner: &str, name: &str, new_owner: &str) -> Any {
    any(
        "/hashgram.username.v1.MsgTransfer",
        &pb::username::MsgTransfer {
            owner: owner.to_owned(),
            name: name.to_owned(),
            new_owner: new_owner.to_owned(),
        },
    )
}

/// `MsgRelease` from `x/username`.
#[must_use]
pub fn release_username(owner: &str, name: &str) -> Any {
    any(
        "/hashgram.username.v1.MsgRelease",
        &pb::username::MsgRelease {
            owner: owner.to_owned(),
            name: name.to_owned(),
        },
    )
}

/// `MsgInitiateRecovery` from `x/identity` (a guardian opens recovery).
#[must_use]
pub fn initiate_recovery(m: &pb::identity::MsgInitiateRecovery) -> Any {
    any("/hashgram.identity.v1.MsgInitiateRecovery", m)
}

/// `MsgCancelRecovery` from `x/identity` (the identity's own account
/// rejects a pending recovery during the delay).
#[must_use]
pub fn cancel_recovery(address: &str) -> Any {
    any(
        "/hashgram.identity.v1.MsgCancelRecovery",
        &pb::identity::MsgCancelRecovery {
            address: address.to_owned(),
        },
    )
}

/// `MsgBeginUnbonding` from `x/serviceproof` (21 days before the bond can
/// be withdrawn).
#[must_use]
pub fn begin_unbonding(operator: &str) -> Any {
    any(
        "/hashgram.serviceproof.v1.MsgBeginUnbonding",
        &pb::serviceproof::MsgBeginUnbonding {
            operator: operator.to_owned(),
        },
    )
}

/// `MsgWithdrawBond` from `x/serviceproof`.
#[must_use]
pub fn withdraw_bond(operator: &str) -> Any {
    any(
        "/hashgram.serviceproof.v1.MsgWithdrawBond",
        &pb::serviceproof::MsgWithdrawBond {
            operator: operator.to_owned(),
        },
    )
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

    #[test]
    fn staking_gov_and_username_messages_round_trip() {
        let d = delegate("hash1d", "hashvaloper1v", 5_000_000);
        assert_eq!(d.type_url, "/cosmos.staking.v1beta1.MsgDelegate");
        let dd: cosmrs::proto::cosmos::staking::v1beta1::MsgDelegate =
            prost013::Message::decode(d.value.as_slice()).unwrap();
        assert_eq!(dd.amount.unwrap().amount, "5000000");

        let r = redelegate("hash1d", "hashvaloper1a", "hashvaloper1b", 1);
        assert_eq!(r.type_url, "/cosmos.staking.v1beta1.MsgBeginRedelegate");
        let u = undelegate("hash1d", "hashvaloper1a", 1);
        assert_eq!(u.type_url, "/cosmos.staking.v1beta1.MsgUndelegate");
        let w = withdraw_rewards("hash1d", "hashvaloper1a");
        assert_eq!(
            w.type_url,
            "/cosmos.distribution.v1beta1.MsgWithdrawDelegatorReward"
        );

        let v = vote("hash1d", 7, VoteOption::NoWithVeto);
        assert_eq!(v.type_url, "/cosmos.gov.v1.MsgVote");
        let vv: cosmrs::proto::cosmos::gov::v1::MsgVote =
            prost013::Message::decode(v.value.as_slice()).unwrap();
        assert_eq!((vv.proposal_id, vv.option), (7, 4));
        assert_eq!(VoteOption::parse("veto"), Some(VoteOption::NoWithVeto));
        assert_eq!(VoteOption::parse("maybe"), None);

        let t = transfer_username("hash1a", "alice", "hash1b");
        assert_eq!(t.type_url, "/hashgram.username.v1.MsgTransfer");
        let tt = pb::username::MsgTransfer::decode(t.value.as_slice()).unwrap();
        assert_eq!(tt.new_owner, "hash1b");
        assert_eq!(
            renew_username("hash1a", "alice").type_url,
            "/hashgram.username.v1.MsgRenew"
        );
        assert_eq!(
            release_username("hash1a", "alice").type_url,
            "/hashgram.username.v1.MsgRelease"
        );
        assert_eq!(
            cancel_recovery("hash1a").type_url,
            "/hashgram.identity.v1.MsgCancelRecovery"
        );
        assert_eq!(
            begin_unbonding("hash1a").type_url,
            "/hashgram.serviceproof.v1.MsgBeginUnbonding"
        );
        assert_eq!(
            withdraw_bond("hash1a").type_url,
            "/hashgram.serviceproof.v1.MsgWithdrawBond"
        );
    }

    #[test]
    fn gas_estimates_treat_staking_as_expensive() {
        let cheap = crate::client::estimate_gas(&[bank_send("a", "b", 1)]);
        let dear = crate::client::estimate_gas(&[delegate("a", "v", 1)]);
        assert!(dear > cheap);
        // A vesting-account delegation costs ~520k; the estimate must cover it.
        assert!(dear >= 520_000);
    }
}
