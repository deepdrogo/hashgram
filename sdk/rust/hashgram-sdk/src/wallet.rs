//! Wallet: balances, transfers, staking, username registration — thin,
//! typed wrappers over `hashgram-chain` so a client never builds a
//! protobuf `Any` itself.

use hashgram_chain::msgs;
use hashgram_chain::pb::username as up;
use serde_json::Value;

use crate::app::HashgramOne;
use crate::SdkError;

/// One HASH in uhash.
pub const UHASH_PER_HASH: u128 = 1_000_000;

/// Formats uhash as `x.yyyyyy HASH`.
#[must_use]
#[allow(clippy::integer_division)] // exact: display units are a fixed 10^6 scale
pub fn format_hash(uhash: u128) -> String {
    format!(
        "{}.{:06} HASH",
        uhash / UHASH_PER_HASH,
        uhash % UHASH_PER_HASH
    )
}

/// Parses `"1.5"` / `"1.5 HASH"` / `"1500000uhash"` into uhash. Never
/// uses floating point.
pub fn parse_amount(s: &str) -> Result<u128, SdkError> {
    let t = s.trim().to_ascii_lowercase();
    if let Some(u) = t.strip_suffix("uhash") {
        return u
            .trim()
            .parse()
            .map_err(|_| SdkError::Invalid(format!("bad amount {s}")));
    }
    let t = t.strip_suffix("hash").map(str::trim).unwrap_or(&t);
    let (whole, frac) = t.split_once('.').unwrap_or((t, ""));
    if frac.len() > 6 || whole.is_empty() && frac.is_empty() {
        return Err(SdkError::Invalid(format!("bad amount {s}")));
    }
    let whole: u128 = if whole.is_empty() {
        0
    } else {
        whole
            .parse()
            .map_err(|_| SdkError::Invalid(format!("bad amount {s}")))?
    };
    let mut f = frac.to_owned();
    while f.len() < 6 {
        f.push('0');
    }
    let frac: u128 = if f.is_empty() {
        0
    } else {
        f.parse()
            .map_err(|_| SdkError::Invalid(format!("bad amount {s}")))?
    };
    whole
        .checked_mul(UHASH_PER_HASH)
        .and_then(|w| w.checked_add(frac))
        .ok_or_else(|| SdkError::Invalid("amount overflow".into()))
}

/// Balance view.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Balance {
    /// Address.
    pub address: String,
    /// uhash as a string (never a float).
    pub uhash: String,
    /// Formatted.
    pub display: String,
    /// How many nodes/operators agreed (relay reads).
    pub verification: Option<String>,
}

/// The Wallet API.
pub struct WalletApi<'a> {
    pub(crate) one: &'a mut HashgramOne,
}

impl<'a> WalletApi<'a> {
    /// Balance of ours (or of `address`).
    pub async fn balance(&mut self, address: Option<&str>) -> Result<Balance, SdkError> {
        let a = address
            .map(str::to_owned)
            .unwrap_or_else(|| self.one.account.address().to_owned());
        let uhash = match self.one.chain.balance(&a).await {
            Ok(b) => b,
            Err(crate::chain::ClientError::NoAccount(_)) => 0,
            Err(e) => return Err(e.into()),
        };
        let verification = self.one.chain.verification().map(|v| {
            if v.single_operator {
                "verified by 1 node — only one operator reachable".to_owned()
            } else if v.agreed {
                format!(
                    "verified by {} nodes ({} operators)",
                    v.peers.len(),
                    v.operators.len()
                )
            } else {
                "unverified".to_owned()
            }
        });
        Ok(Balance {
            address: a,
            uhash: uhash.to_string(),
            display: format_hash(uhash),
            verification,
        })
    }

    /// Sends HASH.
    pub async fn send(
        &mut self,
        to: &str,
        amount_uhash: u128,
        memo: &str,
    ) -> Result<String, SdkError> {
        hashgram_app::ids::require_address("to", to)?;
        let wallet = self.one.account.wallet()?;
        let r = self
            .one
            .chain
            .sign_and_broadcast(
                &wallet,
                vec![msgs::bank_send(
                    self.one.account.address(),
                    to,
                    amount_uhash,
                )],
                memo,
            )
            .await?;
        Ok(r.txhash)
    }

    /// Fee preview for a send.
    pub async fn preview_send(
        &mut self,
        to: &str,
        amount_uhash: u128,
    ) -> Result<hashgram_chain::FeePreview, SdkError> {
        let wallet = self.one.account.wallet()?;
        Ok(self
            .one
            .chain
            .fee_preview(
                &wallet,
                vec![msgs::bank_send(
                    self.one.account.address(),
                    to,
                    amount_uhash,
                )],
                "",
            )
            .await?)
    }

    /// Delegates to a validator.
    pub async fn stake(&mut self, validator: &str, amount_uhash: u128) -> Result<String, SdkError> {
        let wallet = self.one.account.wallet()?;
        let r = self
            .one
            .chain
            .sign_and_broadcast(
                &wallet,
                vec![msgs::delegate(
                    self.one.account.address(),
                    validator,
                    amount_uhash,
                )],
                "hashgram one: stake",
            )
            .await?;
        Ok(r.txhash)
    }

    /// Undelegates.
    pub async fn unstake(
        &mut self,
        validator: &str,
        amount_uhash: u128,
    ) -> Result<String, SdkError> {
        let wallet = self.one.account.wallet()?;
        let r = self
            .one
            .chain
            .sign_and_broadcast(
                &wallet,
                vec![msgs::undelegate(
                    self.one.account.address(),
                    validator,
                    amount_uhash,
                )],
                "hashgram one: unstake",
            )
            .await?;
        Ok(r.txhash)
    }

    /// Withdraws staking rewards from a validator.
    pub async fn withdraw_rewards(&mut self, validator: &str) -> Result<String, SdkError> {
        let wallet = self.one.account.wallet()?;
        let r = self
            .one
            .chain
            .sign_and_broadcast(
                &wallet,
                vec![msgs::withdraw_rewards(
                    self.one.account.address(),
                    validator,
                )],
                "hashgram one: rewards",
            )
            .await?;
        Ok(r.txhash)
    }

    /// Registers a username (1 HASH fee on Mainnet).
    pub async fn register_username(&mut self, name: &str) -> Result<String, SdkError> {
        let wallet = self.one.account.wallet()?;
        let msg = up::MsgRegister {
            owner: self.one.account.address().to_owned(),
            name: name.to_owned(),
        };
        let r = self
            .one
            .chain
            .sign_and_broadcast(
                &wallet,
                vec![msgs::register_username(&msg)],
                "hashgram one: username",
            )
            .await?;
        Ok(r.txhash)
    }

    /// Renews a username.
    pub async fn renew_username(&mut self, name: &str) -> Result<String, SdkError> {
        let wallet = self.one.account.wallet()?;
        let r = self
            .one
            .chain
            .sign_and_broadcast(
                &wallet,
                vec![msgs::renew_username(self.one.account.address(), name)],
                "hashgram one: renew",
            )
            .await?;
        Ok(r.txhash)
    }

    /// Username availability with the chain's reason code.
    pub async fn username_availability(&mut self, name: &str) -> Result<Value, SdkError> {
        Ok(self
            .one
            .chain
            .query(&format!("hashgram/username/v1/availability/{name}"))
            .await?)
    }

    /// Delegations of ours.
    pub async fn delegations(&mut self) -> Result<Value, SdkError> {
        let me = self.one.account.address();
        Ok(self
            .one
            .chain
            .query(&format!("cosmos/staking/v1beta1/delegations/{me}"))
            .await?)
    }

    /// Recent transactions involving us (through the gateway's tx search).
    pub async fn history(&mut self, limit: u32) -> Result<Value, SdkError> {
        let me = self.one.account.address();
        Ok(self
            .one
            .chain
            .query(&format!(
                "cosmos/tx/v1beta1/txs?events=transfer.recipient%3D%27{me}%27&pagination.limit={limit}&order_by=ORDER_BY_DESC"
            ))
            .await?)
    }

    /// Transaction status by hash.
    pub async fn tx(&mut self, hash: &str) -> Result<Option<hashgram_chain::TxResult>, SdkError> {
        Ok(self.one.chain.tx(hash).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts() {
        assert_eq!(parse_amount("1").unwrap(), 1_000_000);
        assert_eq!(parse_amount("1.5 HASH").unwrap(), 1_500_000);
        assert_eq!(parse_amount("0.000001").unwrap(), 1);
        assert_eq!(parse_amount("2500000uhash").unwrap(), 2_500_000);
        assert_eq!(parse_amount(".5").unwrap(), 500_000);
        assert!(parse_amount("1.1234567").is_err());
        assert!(parse_amount("abc").is_err());
        assert_eq!(format_hash(1_500_000), "1.500000 HASH");
    }
}
