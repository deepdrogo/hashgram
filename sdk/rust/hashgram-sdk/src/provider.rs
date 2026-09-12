//! Earn / providers: the `x/serviceproof` lifecycle as an application API.
//!
//! Nothing here changes the economics: rewards come from chain-issued
//! storage challenges and client-signed receipts, never from declared
//! capacity. This module reads a provider's on-chain record and derives a
//! human lifecycle state from it, and wraps the registration, update,
//! unbonding and withdrawal transactions.

use hashgram_chain::msgs;
use hashgram_chain::pb::serviceproof as sp;
use serde_json::Value;

use crate::app::HashgramOne;
use crate::SdkError;

/// Derived lifecycle state (see `docs/HASHGRAM_ONE_ARCHITECTURE.md` §10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, Default)]
pub enum Lifecycle {
    /// No registration.
    #[default]
    Unregistered,
    /// Registered, bond posted, nothing assigned and no receipts yet.
    Registered,
    /// Storage role offered but no assignment yet (assigners may be empty
    /// on this network).
    WaitingForAssignment,
    /// Earning credit in the current epoch.
    Active,
    /// Fraud score above zero but not jailed.
    Degraded,
    /// Jailed until a height.
    Jailed,
    /// Unbonding (21 days).
    Unbonding,
    /// Bond withdrawn / registration deleted.
    Withdrawn,
}

/// A provider as the application sees it.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ProviderStatus {
    /// Operator.
    pub operator: String,
    /// Reward address.
    pub reward_address: String,
    /// Roles as strings.
    pub roles: Vec<String>,
    /// Bond (uhash).
    pub bond_uhash: String,
    /// Declared storage.
    pub declared_storage_bytes: u64,
    /// Fraud score.
    pub fraud_score: u32,
    /// Jailed.
    pub jailed: bool,
    /// Jailed until height.
    pub jailed_until_height: i64,
    /// Unbonding start height (0 = not unbonding).
    pub unbonding_height: i64,
    /// Registered at.
    pub registered_height: i64,
    /// Moniker.
    pub moniker: String,
    /// Derived lifecycle.
    pub lifecycle: Lifecycle,
    /// Raw record for the UI's "details".
    pub raw: Value,
}

/// Earnings view.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Earnings {
    /// Total paid (uhash).
    pub total_paid_uhash: String,
    /// Credit accrued in the open epoch.
    pub pending_credit: String,
    /// Current epoch number.
    pub epoch: u64,
    /// Reserve remaining (uhash).
    pub reserve_remaining_uhash: String,
    /// Raw.
    pub raw: Value,
}

/// Role names ↔ enum.
pub fn role_from_str(s: &str) -> Option<sp::ServiceRole> {
    match s.to_ascii_lowercase().as_str() {
        "storage" | "store" => Some(sp::ServiceRole::Storage),
        "relay" | "bootstrap" => Some(sp::ServiceRole::Relay),
        "call" => Some(sp::ServiceRole::Call),
        "media" => Some(sp::ServiceRole::Media),
        _ => None,
    }
}

fn role_name(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim_start_matches("SERVICE_ROLE_").to_ascii_lowercase(),
        Value::Number(n) => match n.as_i64() {
            Some(1) => "storage".into(),
            Some(2) => "relay".into(),
            Some(3) => "call".into(),
            Some(4) => "media".into(),
            _ => "unknown".into(),
        },
        _ => "unknown".into(),
    }
}

fn str_of(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_owned()
}
fn i64_of(v: &Value, k: &str) -> i64 {
    v.get(k)
        .and_then(|x| {
            x.as_i64()
                .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
        })
        .unwrap_or(0)
}
fn u64_of(v: &Value, k: &str) -> u64 {
    v.get(k)
        .and_then(|x| {
            x.as_u64()
                .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
        })
        .unwrap_or(0)
}

/// Derives a status from a provider JSON record and a few extra facts.
#[must_use]
pub fn status_from_json(
    p: &Value,
    has_assignments: bool,
    has_pending_credit: bool,
) -> ProviderStatus {
    let roles: Vec<String> = p
        .get("roles")
        .and_then(|r| r.as_array())
        .map(|a| a.iter().map(role_name).collect())
        .unwrap_or_default();
    let bond = p
        .get("bond")
        .and_then(|b| b.as_array())
        .and_then(|a| {
            a.iter()
                .find(|c| c.get("denom").and_then(|d| d.as_str()) == Some("uhash"))
        })
        .map(|c| str_of(c, "amount"))
        .unwrap_or_else(|| "0".into());
    let jailed = p.get("jailed").and_then(|x| x.as_bool()).unwrap_or(false);
    let unbonding = i64_of(p, "unbonding_height");
    let fraud = u64_of(p, "fraud_score") as u32;
    let lifecycle = if unbonding > 0 {
        Lifecycle::Unbonding
    } else if jailed {
        Lifecycle::Jailed
    } else if fraud > 0 {
        Lifecycle::Degraded
    } else if has_pending_credit || has_assignments {
        Lifecycle::Active
    } else if roles.iter().any(|r| r == "storage") {
        Lifecycle::WaitingForAssignment
    } else {
        Lifecycle::Registered
    };
    ProviderStatus {
        operator: str_of(p, "operator"),
        reward_address: str_of(p, "reward_address"),
        roles,
        bond_uhash: bond,
        declared_storage_bytes: u64_of(p, "declared_storage_bytes"),
        fraud_score: fraud,
        jailed,
        jailed_until_height: i64_of(p, "jailed_until_height"),
        unbonding_height: unbonding,
        registered_height: i64_of(p, "registered_height"),
        moniker: str_of(p, "moniker"),
        lifecycle,
        raw: p.clone(),
    }
}

/// The Provider API.
pub struct Provider<'a> {
    pub(crate) one: &'a mut HashgramOne,
}

impl<'a> Provider<'a> {
    /// Status of an operator (ours by default).
    pub async fn status(&mut self, operator: Option<&str>) -> Result<ProviderStatus, SdkError> {
        let op = operator
            .map(str::to_owned)
            .unwrap_or_else(|| self.one.account.address().to_owned());
        let v = match self
            .one
            .chain
            .query(&format!("hashgram/serviceproof/v1/provider/{op}"))
            .await
        {
            Ok(v) => v,
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("not found") || msg.contains("404") {
                    return Ok(ProviderStatus {
                        operator: op,
                        lifecycle: Lifecycle::Unregistered,
                        ..Default::default()
                    });
                }
                return Err(e.into());
            }
        };
        let p = v.get("provider").cloned().unwrap_or(v);
        let assignments = self
            .one
            .chain
            .query(&format!("hashgram/serviceproof/v1/assignments/{op}"))
            .await
            .ok()
            .and_then(|a| {
                a.get("assignments")
                    .and_then(|x| x.as_array())
                    .map(|x| !x.is_empty())
            })
            .unwrap_or(false);
        let pending = self
            .one
            .chain
            .query(&format!("hashgram/serviceproof/v1/rewards/{op}"))
            .await
            .ok()
            .map(|r| {
                ["pending_credit", "credit", "accrued_credit"]
                    .iter()
                    .any(|k| {
                        u64_of(&r, k) > 0
                            || r.get(k)
                                .and_then(|x| x.as_str())
                                .map(|s| s != "0" && !s.is_empty())
                                .unwrap_or(false)
                    })
            })
            .unwrap_or(false);
        Ok(status_from_json(&p, assignments, pending))
    }

    /// Earnings of an operator (ours by default).
    pub async fn earnings(&mut self, operator: Option<&str>) -> Result<Earnings, SdkError> {
        let op = operator
            .map(str::to_owned)
            .unwrap_or_else(|| self.one.account.address().to_owned());
        let r = self
            .one
            .chain
            .query(&format!("hashgram/serviceproof/v1/rewards/{op}"))
            .await
            .unwrap_or(Value::Null);
        let epoch = self
            .one
            .chain
            .query("hashgram/serviceproof/v1/epoch/current")
            .await
            .unwrap_or(Value::Null);
        let reserve = self
            .one
            .chain
            .query("hashgram/serviceproof/v1/reserve")
            .await
            .unwrap_or(Value::Null);
        let find = |v: &Value, keys: &[&str]| -> String {
            for k in keys {
                if let Some(x) = v.get(k) {
                    if let Some(s) = x.as_str() {
                        return s.to_owned();
                    }
                    if let Some(n) = x.as_u64() {
                        return n.to_string();
                    }
                    if let Some(a) = x.as_array() {
                        if let Some(c) = a
                            .iter()
                            .find(|c| c.get("denom").and_then(|d| d.as_str()) == Some("uhash"))
                        {
                            return str_of(c, "amount");
                        }
                    }
                }
            }
            "0".into()
        };
        Ok(Earnings {
            total_paid_uhash: find(&r, &["total_paid", "paid", "total_rewards"]),
            pending_credit: find(&r, &["pending_credit", "credit", "accrued_credit"]),
            epoch: u64_of(epoch.get("epoch").unwrap_or(&epoch), "number")
                .max(u64_of(&epoch, "epoch_number")),
            reserve_remaining_uhash: find(
                reserve.get("reserve").unwrap_or(&reserve),
                &["remaining", "balance", "amount"],
            ),
            raw: r,
        })
    }

    /// Network-wide provider list (paginated by the gateway).
    pub async fn list(&mut self) -> Result<Vec<ProviderStatus>, SdkError> {
        let v = self
            .one
            .chain
            .query("hashgram/serviceproof/v1/providers?pagination.limit=200")
            .await?;
        Ok(v.get("providers")
            .and_then(|p| p.as_array())
            .map(|a| {
                a.iter()
                    .map(|p| status_from_json(p, false, false))
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Registers this account as a provider. `node_pubkey_hex` is the P2P
    /// node's ed25519 key (from `hashgram-node node-id` / `/v1/status`).
    pub async fn register(
        &mut self,
        reward_address: &str,
        node_pubkey_hex: &str,
        roles: &[&str],
        bond_uhash: u128,
        declared_storage_bytes: u64,
        moniker: &str,
    ) -> Result<String, SdkError> {
        let wallet = self.one.account.wallet()?;
        let node_pubkey =
            hex::decode(node_pubkey_hex).map_err(|e| SdkError::Invalid(e.to_string()))?;
        let roles: Vec<i32> = roles
            .iter()
            .map(|r| {
                role_from_str(r)
                    .map(|x| x as i32)
                    .ok_or_else(|| SdkError::Invalid(format!("unknown role {r}")))
            })
            .collect::<Result<_, _>>()?;
        let msg = sp::MsgRegisterProvider {
            operator: self.one.account.address().to_owned(),
            reward_address: reward_address.to_owned(),
            node_pubkey,
            node_key_type: sp::KeyType::Ed25519 as i32,
            roles,
            bond: vec![hashgram_chain::pb::cosmos::base::v1beta1::Coin {
                denom: "uhash".into(),
                amount: bond_uhash.to_string(),
            }],
            declared_storage_bytes,
            declared_bandwidth_bps: 0,
            moniker: moniker.to_owned(),
        };
        let r = self
            .one
            .chain
            .sign_and_broadcast(
                &wallet,
                vec![msgs::register_provider(&msg)],
                "hashgram one: register provider",
            )
            .await?;
        Ok(r.txhash)
    }

    /// Updates mutable fields.
    pub async fn update(
        &mut self,
        reward_address: &str,
        roles: &[&str],
        declared_storage_bytes: u64,
        moniker: &str,
        additional_bond_uhash: u128,
    ) -> Result<String, SdkError> {
        let wallet = self.one.account.wallet()?;
        let roles: Vec<i32> = roles
            .iter()
            .map(|r| {
                role_from_str(r)
                    .map(|x| x as i32)
                    .ok_or_else(|| SdkError::Invalid(format!("unknown role {r}")))
            })
            .collect::<Result<_, _>>()?;
        let msg = sp::MsgUpdateProvider {
            operator: self.one.account.address().to_owned(),
            reward_address: reward_address.to_owned(),
            roles,
            declared_storage_bytes,
            declared_bandwidth_bps: 0,
            moniker: moniker.to_owned(),
            additional_bond: if additional_bond_uhash > 0 {
                vec![hashgram_chain::pb::cosmos::base::v1beta1::Coin {
                    denom: "uhash".into(),
                    amount: additional_bond_uhash.to_string(),
                }]
            } else {
                vec![]
            },
        };
        let r = self
            .one
            .chain
            .sign_and_broadcast(
                &wallet,
                vec![msgs::update_provider(&msg)],
                "hashgram one: update provider",
            )
            .await?;
        Ok(r.txhash)
    }

    /// Begins unbonding.
    pub async fn unbond(&mut self) -> Result<String, SdkError> {
        let wallet = self.one.account.wallet()?;
        let r = self
            .one
            .chain
            .sign_and_broadcast(
                &wallet,
                vec![msgs::begin_unbonding(self.one.account.address())],
                "hashgram one: unbond",
            )
            .await?;
        Ok(r.txhash)
    }

    /// Withdraws the bond after the unbonding period.
    pub async fn withdraw(&mut self) -> Result<String, SdkError> {
        let wallet = self.one.account.wallet()?;
        let r = self
            .one
            .chain
            .sign_and_broadcast(
                &wallet,
                vec![msgs::withdraw_bond(self.one.account.address())],
                "hashgram one: withdraw bond",
            )
            .await?;
        Ok(r.txhash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_derivation() {
        let p = serde_json::json!({
            "operator": "hash1op", "roles": ["SERVICE_ROLE_STORAGE"], "bond": [{"denom":"uhash","amount":"1000000000"}],
            "fraud_score": "0", "jailed": false, "unbonding_height": "0"
        });
        assert_eq!(
            status_from_json(&p, false, false).lifecycle,
            Lifecycle::WaitingForAssignment
        );
        assert_eq!(
            status_from_json(&p, true, false).lifecycle,
            Lifecycle::Active
        );
        let mut d = p.clone();
        d["fraud_score"] = serde_json::json!("25");
        assert_eq!(
            status_from_json(&d, true, true).lifecycle,
            Lifecycle::Degraded
        );
        d["jailed"] = serde_json::json!(true);
        assert_eq!(
            status_from_json(&d, true, true).lifecycle,
            Lifecycle::Jailed
        );
        d["unbonding_height"] = serde_json::json!("500");
        assert_eq!(
            status_from_json(&d, true, true).lifecycle,
            Lifecycle::Unbonding
        );
        let relay = serde_json::json!({"roles": [2], "bond": []});
        let s = status_from_json(&relay, false, false);
        assert_eq!(s.lifecycle, Lifecycle::Registered);
        assert_eq!(s.roles, vec!["relay"]);
        assert_eq!(s.bond_uhash, "0");
    }
}
