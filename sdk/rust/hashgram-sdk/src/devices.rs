//! Devices: keep the MLS groups and the user's other devices in step with
//! the chain's device registry.
//!
//! * [`Devices::reconcile`] compares every group's roster with the chain:
//!   devices the chain marks revoked are removed with an MLS commit (from
//!   the next epoch they decrypt nothing); active devices of members that
//!   are missing from a group are added (so a member's new phone starts
//!   receiving). See `docs/MULTI_DEVICE_SECURITY.md` for the window this
//!   leaves open.
//! * [`Devices::bootstrap_new_device`] sends what a new device of *ours*
//!   needs but cannot derive: the Drive keyring, the contact book and the
//!   current mail flags, through the self group.
//! * On-chain add/revoke/rotation are thin wrappers over `account`.

use std::collections::{BTreeMap, BTreeSet};

use hashgram_app::pb as app;
use tracing::{debug, info, warn};

use crate::account::{devices_on_chain, DeviceView};
use crate::app::HashgramOne;
use crate::SdkError;

/// What a reconcile pass did.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ReconcileReport {
    /// Groups examined.
    pub groups: usize,
    /// Revoked devices removed (group, device hex).
    pub removed: Vec<(String, String)>,
    /// New devices added (group, address).
    pub added: Vec<(String, String)>,
    /// Errors (group, message).
    pub errors: Vec<(String, String)>,
}

/// The Devices API.
pub struct Devices<'a> {
    pub(crate) one: &'a mut HashgramOne,
}

impl<'a> Devices<'a> {
    /// Our devices as the chain reports them.
    pub async fn list(&mut self) -> Result<Vec<DeviceView>, SdkError> {
        let me = self.one.account.address().to_owned();
        devices_on_chain(&self.one.chain, &me).await
    }

    /// This device's id and hex public key.
    pub fn this_device(&self) -> Result<(String, String), SdkError> {
        Ok((
            self.one.account.contents.device_id.clone(),
            hex::encode(self.one.account.device()?.public_key()),
        ))
    }

    /// Adds a device to our identity on chain (requires the root key on
    /// this device). `pubkey_hex` is the new device's ed25519 public key.
    pub async fn add_on_chain(
        &mut self,
        device_id: &str,
        pubkey_hex: &str,
        label: &str,
        platform: &str,
    ) -> Result<String, SdkError> {
        let pk = hex::decode(pubkey_hex).map_err(|e| SdkError::Invalid(e.to_string()))?;
        let pk: [u8; 32] = pk
            .as_slice()
            .try_into()
            .map_err(|_| SdkError::Invalid("device key must be 32 bytes".into()))?;
        let me = self.one.account.address().to_owned();
        let rotation = crate::account::rotation_count_on_chain(&self.one.chain, &me)
            .await?
            .ok_or_else(|| SdkError::NotFound("identity is not registered on chain".into()))?;
        let r = crate::account::add_device_on_chain(
            &self.one.account,
            &self.one.network,
            &self.one.chain,
            device_id,
            &pk,
            rotation,
            label,
            platform,
        )
        .await?;
        Ok(r.txhash)
    }

    /// Revokes a device on chain (requires the wallet key). Then removes it
    /// from every group we are in.
    pub async fn revoke_on_chain(&mut self, device_id: &str) -> Result<String, SdkError> {
        let r =
            crate::account::revoke_device_on_chain(&self.one.account, &self.one.chain, device_id)
                .await?;
        let _ = self.reconcile().await;
        Ok(r.txhash)
    }

    /// Reconciles MLS rosters with the chain for every group.
    pub async fn reconcile(&mut self) -> Result<ReconcileReport, SdkError> {
        let mut report = ReconcileReport::default();
        let conversations = self.one.messaging.conversations();
        report.groups = conversations.len();
        let mut chain_cache: BTreeMap<String, Vec<DeviceView>> = BTreeMap::new();
        for (gid_hex, _meta, addrs) in conversations {
            let Ok(gid) = hex::decode(&gid_hex) else {
                continue;
            };
            let members = match self.one.messaging.mls().members(&gid) {
                Ok(m) => m,
                Err(e) => {
                    report.errors.push((gid_hex.clone(), e.to_string()));
                    continue;
                }
            };
            // Chain view for every address in the group.
            let mut active: BTreeMap<String, BTreeSet<Vec<u8>>> = BTreeMap::new();
            let mut revoked: BTreeSet<Vec<u8>> = BTreeSet::new();
            for a in &addrs {
                let devs = match chain_cache.get(a) {
                    Some(d) => d.clone(),
                    None => match devices_on_chain(&self.one.chain, a).await {
                        Ok(d) => {
                            chain_cache.insert(a.clone(), d.clone());
                            d
                        }
                        Err(e) => {
                            debug!(address = %a, error = %e, "device lookup failed; skipping group");
                            report.errors.push((gid_hex.clone(), format!("{a}: {e}")));
                            continue;
                        }
                    },
                };
                for d in devs {
                    let Ok(k) = hex::decode(&d.device_pubkey) else {
                        continue;
                    };
                    if d.revoked {
                        revoked.insert(k);
                    } else {
                        active.entry(a.clone()).or_default().insert(k);
                    }
                }
            }
            // Remove revoked members.
            let mut to_remove: Vec<u32> = Vec::new();
            for m in &members {
                let (_addr, key) = hashgram_mls::parse_identity(&m.identity).unwrap_or_default();
                if revoked.contains(&key) {
                    to_remove.push(m.index);
                    report.removed.push((gid_hex.clone(), hex::encode(&key)));
                }
            }
            if !to_remove.is_empty() {
                match self
                    .one
                    .messaging
                    .mls_mut()
                    .remove_members(&gid, &to_remove)
                {
                    Ok(commit) => {
                        for key in self
                            .one
                            .messaging
                            .mls()
                            .recipient_devices(&gid)
                            .unwrap_or_default()
                        {
                            if let Err(e) = self
                                .one
                                .messaging
                                .deliver_raw(
                                    &self.one.link,
                                    &self.one.network,
                                    &key,
                                    hashgram_proto::pb::EnvelopeKind::MlsMessage,
                                    &commit,
                                )
                                .await
                            {
                                debug!(error = %e, "removal commit delivery failed for one device");
                            }
                        }
                        info!(group = %gid_hex, n = to_remove.len(), "revoked devices removed from group");
                    }
                    Err(e) => report.errors.push((gid_hex.clone(), e.to_string())),
                }
            }
            // Add missing active devices.
            let present: BTreeSet<Vec<u8>> = members
                .iter()
                .map(|m| {
                    hashgram_mls::parse_identity(&m.identity)
                        .map(|(_, k)| k)
                        .unwrap_or_default()
                })
                .collect();
            for (addr, keys) in &active {
                if keys.iter().any(|k| !present.contains(k)) {
                    match self
                        .one
                        .messaging
                        .add_participant(
                            &self.one.link,
                            &self.one.chain,
                            &self.one.network,
                            &gid,
                            addr,
                        )
                        .await
                    {
                        Ok(()) => report.added.push((gid_hex.clone(), addr.clone())),
                        Err(SdkError::NoRecipients) => {} // no key package yet
                        Err(e) => {
                            warn!(group = %gid_hex, address = %addr, error = %e, "adding new device failed");
                            report
                                .errors
                                .push((gid_hex.clone(), format!("{addr}: {e}")));
                        }
                    }
                }
            }
        }
        Ok(report)
    }

    /// Sends the Drive keyring, contacts and mail flags to our other
    /// devices. Call after adding a device (and it is also done by the sync
    /// engine whenever state changed).
    pub async fn bootstrap_new_device(&mut self) -> Result<bool, SdkError> {
        let Some(gid) = self.one.self_group().await? else {
            return Ok(false);
        };
        self.one.drive().announce_keyring().await;
        let snapshot = self.one.people_state.contacts.snapshot();
        self.one
            .send_app(
                &gid,
                app::app_message::Body::DeviceSync(app::DeviceSync {
                    version: hashgram_app::version::DEVICE_SYNC_VERSION,
                    body: Some(app::device_sync::Body::Contacts(snapshot)),
                }),
            )
            .await?;
        Ok(true)
    }

    /// Handles a `DeviceSync` from one of our own devices. Refuses anything
    /// whose MLS sender is not our own address.
    pub(crate) async fn handle_incoming(
        &mut self,
        sender: &str,
        sync: &app::DeviceSync,
    ) -> Result<bool, SdkError> {
        if sender != self.one.account.address() {
            warn!("device sync from another address ignored");
            return Ok(false);
        }
        match &sync.body {
            Some(app::device_sync::Body::DriveKeyring(k)) => {
                self.one.drive().apply_keyring(k).await
            }
            Some(app::device_sync::Body::MailState(h)) => {
                self.one.mail().apply_state_hint(h)?;
                Ok(true)
            }
            Some(app::device_sync::Body::Contacts(c)) => {
                self.one.people().apply_snapshot(c)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }
}
