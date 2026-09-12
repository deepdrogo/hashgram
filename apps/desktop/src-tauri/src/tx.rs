//! Transaction intents the UI can submit, and how they become messages.
//!
//! The frontend never builds protobuf. It sends one of these, the Rust side
//! validates it (addresses, amounts as integers — `uhash` reaches 1e15, so
//! `u128`, never a float), turns it into `Any` messages with
//! `hashgram_chain::msgs`, and produces the one-line summary the confirm
//! screen and the history show.

use hashgram_sdk::chain::msgs::{self, VoteOption};
use hashgram_sdk::chain::Any;
use serde::{Deserialize, Serialize};

/// Bech32 prefix of a Hashgram account address.
pub const ADDRESS_PREFIX: &str = "hash";
/// Bech32 prefix of a validator operator address.
pub const VALOPER_PREFIX: &str = "hashvaloper";

/// One intent.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MsgSpec {
    /// Send HASH. Transfers are untaxed.
    Send {
        /// Recipient `hash1…`.
        to: String,
        /// Amount in uhash as a decimal string.
        amount_uhash: String,
    },
    /// Delegate to a validator.
    Delegate {
        /// `hashvaloper1…`.
        validator: String,
        /// uhash.
        amount_uhash: String,
    },
    /// Undelegate (21-day unbonding).
    Undelegate {
        /// `hashvaloper1…`.
        validator: String,
        /// uhash.
        amount_uhash: String,
    },
    /// Redelegate.
    Redelegate {
        /// From.
        from_validator: String,
        /// To.
        to_validator: String,
        /// uhash.
        amount_uhash: String,
    },
    /// Withdraw staking rewards from one validator.
    WithdrawRewards {
        /// `hashvaloper1…`.
        validator: String,
    },
    /// Vote on a proposal.
    Vote {
        /// Proposal id.
        proposal_id: u64,
        /// `yes` | `no` | `abstain` | `no_with_veto`.
        option: String,
    },
    /// Register an `@username` (fee 1 HASH, ~1 year).
    RegisterUsername {
        /// Name without `@`.
        name: String,
    },
    /// Renew.
    RenewUsername {
        /// Name.
        name: String,
    },
    /// Transfer to another address.
    TransferUsername {
        /// Name.
        name: String,
        /// New owner.
        to: String,
    },
    /// Release.
    ReleaseUsername {
        /// Name.
        name: String,
    },
    /// Set social-recovery guardians.
    SetRecoveryConfig {
        /// Guardian addresses.
        guardians: Vec<String>,
        /// Approvals required (0 disables).
        threshold: u32,
        /// Mandatory delay in blocks.
        delay_blocks: i64,
    },
    /// Cancel a pending recovery of this identity.
    CancelRecovery,
    /// Revoke one of this identity's devices.
    RevokeDevice {
        /// Device id.
        device_id: String,
    },
    /// Begin unbonding the provider bond (21 days).
    BeginUnbonding,
    /// Withdraw a completed provider bond.
    WithdrawBond,
}

/// A validated, buildable transaction.
pub struct Built {
    /// Messages.
    pub msgs: Vec<Any>,
    /// Summary for the confirm screen and history.
    pub summary: String,
    /// Warnings the UI must show before confirm (21-day unbonding, fee
    /// destination, slashing).
    pub warnings: Vec<String>,
}

/// Parses an integer uhash amount.
pub fn parse_uhash(s: &str) -> Result<u128, String> {
    let t = s.trim().replace('_', "");
    if t.is_empty() {
        return Err("amount is empty".into());
    }
    t.parse::<u128>()
        .map_err(|_| format!("amount {s:?} is not a whole number of uhash"))
}

/// Parses a HASH amount typed by a person ("12.5", "0.000001") into uhash.
pub fn parse_hash_amount(s: &str) -> Result<u128, String> {
    let t = s.trim().replace([',', '_', ' '], "");
    if t.is_empty() {
        return Err("amount is empty".into());
    }
    let (whole, frac) = match t.split_once('.') {
        Some((w, f)) => (w, f),
        None => (t.as_str(), ""),
    };
    if frac.len() > 6 {
        return Err("HASH has 6 decimals".into());
    }
    if !whole.chars().all(|c| c.is_ascii_digit()) || !frac.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("{s:?} is not a number"));
    }
    let whole: u128 = if whole.is_empty() {
        0
    } else {
        whole.parse().map_err(|_| "amount too large".to_owned())?
    };
    let mut frac_s = frac.to_owned();
    while frac_s.len() < 6 {
        frac_s.push('0');
    }
    let frac: u128 = if frac_s.is_empty() {
        0
    } else {
        frac_s.parse().map_err(|_| "amount too large".to_owned())?
    };
    whole
        .checked_mul(1_000_000)
        .and_then(|w| w.checked_add(frac))
        .ok_or_else(|| "amount too large".to_owned())
}

/// Formats uhash as HASH with six decimals, trailing zeros trimmed to at
/// least two.
#[must_use]
pub fn format_hash(uhash: u128) -> String {
    let whole = uhash.div_euclid(1_000_000);
    let frac = uhash % 1_000_000;
    let mut f = format!("{frac:06}");
    while f.len() > 2 && f.ends_with('0') {
        f.pop();
    }
    format!("{whole}.{f}")
}

/// Validates a Bech32 address against a prefix. `cosmos1…` is not a
/// Hashgram address.
pub fn validate_address(addr: &str, prefix: &str) -> Result<(), String> {
    let (hrp, _) = bech32::decode(addr.trim())
        .map_err(|_| format!("{addr:?} is not a valid Bech32 address"))?;
    if hrp.as_str() != prefix {
        return Err(format!(
            "{addr:?} has prefix {:?}; a Hashgram address starts with {prefix}1",
            hrp.as_str()
        ));
    }
    Ok(())
}

/// Middle-truncates a hash/address for display: `hash1abc…wxyz`.
#[must_use]
pub fn truncate_middle(s: &str, head: usize, tail: usize) -> String {
    if s.chars().count() <= head + tail + 1 {
        return s.to_owned();
    }
    let h: String = s.chars().take(head).collect();
    let t: String = s
        .chars()
        .rev()
        .take(tail)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{h}…{t}")
}

fn valoper(v: &str) -> Result<(), String> {
    validate_address(v, VALOPER_PREFIX)
}

fn username_ok(name: &str) -> Result<String, String> {
    let n = name.trim().trim_start_matches('@').to_ascii_lowercase();
    if n.len() < 3 || n.len() > 32 {
        return Err("a username is 3–32 characters".into());
    }
    if !n
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err("a username is lowercase letters, digits and underscore".into());
    }
    Ok(n)
}

const UNBONDING_WARNING: &str = "Unbonding takes 21 days. During that time the tokens earn nothing and cannot be moved. Validators are slashed 5 % for double-signing and 0.01 % for downtime; delegators share those losses.";
const FEE_NOTE: &str = "Transfers are untaxed: 100 HASH sent is 100 HASH received. 1 % of the network fee (not of the amount) goes to the Founder.";

impl MsgSpec {
    /// Builds the messages for `signer`.
    pub fn build(&self, signer: &str) -> Result<Built, String> {
        let mut warnings = Vec::new();
        let (msgs, summary) = match self {
            Self::Send { to, amount_uhash } => {
                validate_address(to, ADDRESS_PREFIX)?;
                let amt = parse_uhash(amount_uhash)?;
                if amt == 0 {
                    return Err("amount must be more than 0".into());
                }
                warnings.push(FEE_NOTE.to_owned());
                (
                    vec![msgs::bank_send(signer, to.trim(), amt)],
                    format!(
                        "Send {} HASH to {}",
                        format_hash(amt),
                        truncate_middle(to.trim(), 10, 6)
                    ),
                )
            }
            Self::Delegate {
                validator,
                amount_uhash,
            } => {
                valoper(validator)?;
                let amt = parse_uhash(amount_uhash)?;
                warnings.push(UNBONDING_WARNING.to_owned());
                (
                    vec![msgs::delegate(signer, validator.trim(), amt)],
                    format!(
                        "Delegate {} HASH to {}",
                        format_hash(amt),
                        truncate_middle(validator.trim(), 14, 6)
                    ),
                )
            }
            Self::Undelegate {
                validator,
                amount_uhash,
            } => {
                valoper(validator)?;
                let amt = parse_uhash(amount_uhash)?;
                warnings.push(UNBONDING_WARNING.to_owned());
                (
                    vec![msgs::undelegate(signer, validator.trim(), amt)],
                    format!(
                        "Undelegate {} HASH from {} (21 days)",
                        format_hash(amt),
                        truncate_middle(validator.trim(), 14, 6)
                    ),
                )
            }
            Self::Redelegate {
                from_validator,
                to_validator,
                amount_uhash,
            } => {
                valoper(from_validator)?;
                valoper(to_validator)?;
                let amt = parse_uhash(amount_uhash)?;
                warnings.push("A redelegation cannot be redelegated again for 21 days.".to_owned());
                (
                    vec![msgs::redelegate(
                        signer,
                        from_validator.trim(),
                        to_validator.trim(),
                        amt,
                    )],
                    format!(
                        "Redelegate {} HASH to {}",
                        format_hash(amt),
                        truncate_middle(to_validator.trim(), 14, 6)
                    ),
                )
            }
            Self::WithdrawRewards { validator } => {
                valoper(validator)?;
                (
                    vec![msgs::withdraw_rewards(signer, validator.trim())],
                    format!(
                        "Withdraw rewards from {}",
                        truncate_middle(validator.trim(), 14, 6)
                    ),
                )
            }
            Self::Vote {
                proposal_id,
                option,
            } => {
                let opt = VoteOption::parse(option).ok_or_else(|| {
                    format!("vote option {option:?} is not yes/no/abstain/no_with_veto")
                })?;
                (
                    vec![msgs::vote(signer, *proposal_id, opt)],
                    format!(
                        "Vote {} on proposal #{proposal_id}",
                        option.to_ascii_lowercase()
                    ),
                )
            }
            Self::RegisterUsername { name } => {
                let n = username_ok(name)?;
                warnings.push("Registration costs 1 HASH (a protocol fee) and is valid for 7,884,000 blocks (about a year), with a 30-day grace period to renew.".to_owned());
                (
                    vec![msgs::register_username(
                        &hashgram_sdk::chain::pb::username::MsgRegister {
                            owner: signer.to_owned(),
                            name: n.clone(),
                        },
                    )],
                    format!("Register @{n}"),
                )
            }
            Self::RenewUsername { name } => {
                let n = username_ok(name)?;
                (
                    vec![msgs::renew_username(signer, &n)],
                    format!("Renew @{n}"),
                )
            }
            Self::TransferUsername { name, to } => {
                let n = username_ok(name)?;
                validate_address(to, ADDRESS_PREFIX)?;
                (
                    vec![msgs::transfer_username(signer, &n, to.trim())],
                    format!("Transfer @{n} to {}", truncate_middle(to.trim(), 10, 6)),
                )
            }
            Self::ReleaseUsername { name } => {
                let n = username_ok(name)?;
                warnings.push("Releasing a name lets anyone register it.".to_owned());
                (
                    vec![msgs::release_username(signer, &n)],
                    format!("Release @{n}"),
                )
            }
            Self::SetRecoveryConfig {
                guardians,
                threshold,
                delay_blocks,
            } => {
                for g in guardians {
                    validate_address(g, ADDRESS_PREFIX)?;
                }
                if *threshold as usize > guardians.len() {
                    return Err("threshold cannot exceed the number of guardians".into());
                }
                warnings.push("The recovery delay is your window to cancel a recovery you did not ask for. Keep it long enough to notice.".to_owned());
                let m = hashgram_sdk::chain::pb::identity::MsgSetRecoveryConfig {
                    address: signer.to_owned(),
                    recovery: Some(hashgram_sdk::chain::pb::identity::RecoveryConfig {
                        guardians: guardians.iter().map(|g| g.trim().to_owned()).collect(),
                        threshold: *threshold,
                        recovery_delay_blocks: *delay_blocks,
                        recovery_hash: Vec::new(),
                    }),
                };
                (
                    vec![msgs::set_recovery_config(&m)],
                    format!("Set {} guardian(s), threshold {threshold}", guardians.len()),
                )
            }
            Self::CancelRecovery => (
                vec![msgs::cancel_recovery(signer)],
                "Cancel pending recovery".to_owned(),
            ),
            Self::RevokeDevice { device_id } => (
                vec![msgs::revoke_device(
                    &hashgram_sdk::chain::pb::identity::MsgRevokeDevice {
                        address: signer.to_owned(),
                        device_id: device_id.clone(),
                    },
                )],
                format!("Revoke device {}", truncate_middle(device_id, 8, 4)),
            ),
            Self::BeginUnbonding => {
                warnings.push("The provider bond unbonds over 21 days. The node stops earning when unbonding begins.".to_owned());
                (
                    vec![msgs::begin_unbonding(signer)],
                    "Begin unbonding provider bond".to_owned(),
                )
            }
            Self::WithdrawBond => (
                vec![msgs::withdraw_bond(signer)],
                "Withdraw provider bond".to_owned(),
            ),
        };
        Ok(Built {
            msgs,
            summary,
            warnings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADDR: &str = "hash13t8v5nnghrvgcuuqcrt9k5wyhtqwq7fl3ynjpy";
    const VAL: &str = "hashvaloper127zemcfnxd3jrldpjzzgcckek4dswyw0l7rfcq";

    #[test]
    fn amounts_are_integers_never_floats() {
        assert_eq!(parse_hash_amount("1").unwrap(), 1_000_000);
        assert_eq!(parse_hash_amount("0.000001").unwrap(), 1);
        assert_eq!(parse_hash_amount("12.5").unwrap(), 12_500_000);
        assert_eq!(parse_hash_amount("1,000").unwrap(), 1_000_000_000);
        assert!(parse_hash_amount("1.0000001").is_err());
        assert!(parse_hash_amount("abc").is_err());
        // 1e15 uhash, the whole supply, fits and round-trips.
        assert_eq!(
            parse_hash_amount("1000000000").unwrap(),
            1_000_000_000_000_000
        );
        assert_eq!(format_hash(1_000_000_000_000_000), "1000000000.00");
        assert_eq!(format_hash(12_500_000), "12.50");
        assert_eq!(format_hash(1), "0.000001");
    }

    #[test]
    fn hash_prefix_is_required_and_cosmos_is_refused() {
        assert!(validate_address(ADDR, ADDRESS_PREFIX).is_ok());
        assert!(validate_address(
            "cosmos1qypqxpq9qcrsszg2pvxq6rs0zqg3yyc5lzv7xu",
            ADDRESS_PREFIX
        )
        .is_err());
        assert!(validate_address("hash1notvalid", ADDRESS_PREFIX).is_err());
        assert!(validate_address(VAL, VALOPER_PREFIX).is_ok());
        assert!(validate_address(VAL, ADDRESS_PREFIX).is_err());
    }

    #[test]
    fn send_builds_with_the_untaxed_note() {
        let b = MsgSpec::Send {
            to: ADDR.into(),
            amount_uhash: "100000000".into(),
        }
        .build("hash1sender")
        .unwrap();
        assert_eq!(b.msgs.len(), 1);
        assert_eq!(b.msgs[0].type_url, "/cosmos.bank.v1beta1.MsgSend");
        assert!(b.summary.starts_with("Send 100.00 HASH to hash13t8v5"));
        assert!(b.warnings.iter().any(|w| w.contains("untaxed")));
    }

    #[test]
    fn staking_warns_about_21_days_before_confirm() {
        for spec in [
            MsgSpec::Delegate {
                validator: VAL.into(),
                amount_uhash: "1".into(),
            },
            MsgSpec::Undelegate {
                validator: VAL.into(),
                amount_uhash: "1".into(),
            },
        ] {
            let b = spec.build(ADDR).unwrap();
            assert!(
                b.warnings.iter().any(|w| w.contains("21 days")),
                "{:?}",
                b.warnings
            );
            assert!(b
                .warnings
                .iter()
                .any(|w| w.contains("5 %") && w.contains("0.01 %")));
        }
    }

    #[test]
    fn usernames_are_normalised_and_bounded() {
        let b = MsgSpec::RegisterUsername {
            name: "@Alice_01".into(),
        }
        .build(ADDR)
        .unwrap();
        assert_eq!(b.summary, "Register @alice_01");
        assert!(MsgSpec::RegisterUsername { name: "ab".into() }
            .build(ADDR)
            .is_err());
        assert!(MsgSpec::RegisterUsername {
            name: "has space".into()
        }
        .build(ADDR)
        .is_err());
    }

    #[test]
    fn truncation_keeps_head_and_tail() {
        assert_eq!(truncate_middle(ADDR, 10, 6), "hash13t8v5…3ynjpy");
        assert_eq!(truncate_middle("short", 10, 6), "short");
    }

    #[test]
    fn spec_json_shape_is_tagged() {
        let s: MsgSpec =
            serde_json::from_str(r#"{"type":"vote","proposal_id":3,"option":"yes"}"#).unwrap();
        assert!(matches!(s, MsgSpec::Vote { proposal_id: 3, .. }));
        let b = s.build(ADDR).unwrap();
        assert_eq!(b.msgs[0].type_url, "/cosmos.gov.v1.MsgVote");
    }
}
