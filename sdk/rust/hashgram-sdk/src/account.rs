//! The local account: vault, wallet, root and device keys, and the chain
//! transactions that register them.

use std::path::Path;

use hashgram_chain::pb::identity as idpb;
use hashgram_chain::{msgs, Client, Wallet};
use hashgram_identity::vault::KdfCost;
use hashgram_identity::{Vault, VaultContents};
use hashgram_net::NetworkIdentity;
use hashgram_proto::Ed25519Signer;

use crate::SdkError;

/// A device's opened account material.
pub struct Account {
    /// The vault file.
    pub vault: Vault,
    /// Passphrase, kept to re-encrypt on save.
    passphrase: String,
    /// Decrypted contents.
    pub contents: VaultContents,
    /// KDF cost used when writing.
    pub kdf: KdfCost,
}

impl Account {
    /// Creates a new account: fresh wallet, root and device keys.
    /// Returns the account and the mnemonic, which the caller shows once.
    pub fn create(
        vault_path: &Path,
        passphrase: &str,
        device_id: &str,
        kdf: KdfCost,
    ) -> Result<(Self, String), SdkError> {
        let (mnemonic, wallet) = Wallet::generate()?;
        let root = Ed25519Signer::generate()?;
        let device = Ed25519Signer::generate()?;
        let contents = VaultContents {
            address: wallet.address().to_string(),
            wallet_secret: Some(wallet.secret_bytes().to_vec()),
            mnemonic: None,
            root_seed: Some(root.secret_bytes().to_vec()),
            device_seed: Some(device.secret_bytes().to_vec()),
            device_id: device_id.to_owned(),
            extra: Default::default(),
        };
        let vault = Vault::at(vault_path);
        vault.create(passphrase, &contents, kdf)?;
        Ok((
            Self {
                vault,
                passphrase: passphrase.to_owned(),
                contents,
                kdf,
            },
            mnemonic,
        ))
    }

    /// Creates an account from an existing mnemonic (restoring the wallet)
    /// with fresh root and device keys.
    pub fn import(
        vault_path: &Path,
        passphrase: &str,
        mnemonic: &str,
        device_id: &str,
        kdf: KdfCost,
    ) -> Result<Self, SdkError> {
        let wallet = Wallet::from_mnemonic(mnemonic, "", 0)?;
        Self::import_wallet(vault_path, passphrase, wallet, device_id, kdf)
    }

    /// Creates an account from a raw secp256k1 secret (for devnet keys
    /// exported from `hashgramd keys export --unarmored-hex`).
    pub fn import_secret(
        vault_path: &Path,
        passphrase: &str,
        secret: &[u8],
        device_id: &str,
        kdf: KdfCost,
    ) -> Result<Self, SdkError> {
        let wallet = Wallet::from_secret(secret)?;
        Self::import_wallet(vault_path, passphrase, wallet, device_id, kdf)
    }

    fn import_wallet(
        vault_path: &Path,
        passphrase: &str,
        wallet: Wallet,
        device_id: &str,
        kdf: KdfCost,
    ) -> Result<Self, SdkError> {
        let root = Ed25519Signer::generate()?;
        let device = Ed25519Signer::generate()?;
        let contents = VaultContents {
            address: wallet.address().to_string(),
            wallet_secret: Some(wallet.secret_bytes().to_vec()),
            mnemonic: None,
            root_seed: Some(root.secret_bytes().to_vec()),
            device_seed: Some(device.secret_bytes().to_vec()),
            device_id: device_id.to_owned(),
            extra: Default::default(),
        };
        let vault = Vault::at(vault_path);
        vault.create(passphrase, &contents, kdf)?;
        Ok(Self {
            vault,
            passphrase: passphrase.to_owned(),
            contents,
            kdf,
        })
    }

    /// Creates a second-device vault: a device key only, for an address
    /// whose root lives elsewhere. The root holder issues the certificate.
    pub fn create_device_only(
        vault_path: &Path,
        passphrase: &str,
        address: &str,
        device_id: &str,
        kdf: KdfCost,
    ) -> Result<Self, SdkError> {
        let device = Ed25519Signer::generate()?;
        let contents = VaultContents {
            address: address.to_owned(),
            wallet_secret: None,
            mnemonic: None,
            root_seed: None,
            device_seed: Some(device.secret_bytes().to_vec()),
            device_id: device_id.to_owned(),
            extra: Default::default(),
        };
        let vault = Vault::at(vault_path);
        vault.create(passphrase, &contents, kdf)?;
        Ok(Self {
            vault,
            passphrase: passphrase.to_owned(),
            contents,
            kdf,
        })
    }

    /// Opens an existing vault.
    pub fn open(vault_path: &Path, passphrase: &str, kdf: KdfCost) -> Result<Self, SdkError> {
        let vault = Vault::at(vault_path);
        let contents = vault.open(passphrase)?;
        Ok(Self {
            vault,
            passphrase: passphrase.to_owned(),
            contents,
            kdf,
        })
    }

    /// Re-encrypts the vault with the current contents.
    pub fn save(&self) -> Result<(), SdkError> {
        self.vault
            .write(&self.passphrase, &self.contents, self.kdf)?;
        Ok(())
    }

    /// The address.
    #[must_use]
    pub fn address(&self) -> &str {
        &self.contents.address
    }

    /// The wallet, if this device holds the account key.
    pub fn wallet(&self) -> Result<Wallet, SdkError> {
        let secret = self
            .contents
            .wallet_secret
            .as_ref()
            .ok_or(SdkError::NoWalletKey)?;
        Ok(Wallet::from_secret(secret)?)
    }

    /// The root signer, if this device holds it.
    pub fn root(&self) -> Result<Ed25519Signer, SdkError> {
        let seed = self
            .contents
            .root_seed
            .as_ref()
            .ok_or(SdkError::NoRootKey)?;
        let seed: [u8; 32] = seed
            .as_slice()
            .try_into()
            .map_err(|_| SdkError::NoRootKey)?;
        Ok(Ed25519Signer::from_secret(seed))
    }

    /// The device signer.
    pub fn device(&self) -> Result<Ed25519Signer, SdkError> {
        let seed = self
            .contents
            .device_seed
            .as_ref()
            .ok_or(SdkError::NoDeviceKey)?;
        let seed: [u8; 32] = seed
            .as_slice()
            .try_into()
            .map_err(|_| SdkError::NoDeviceKey)?;
        Ok(Ed25519Signer::from_secret(seed))
    }

    /// The device seed, for MLS.
    pub fn device_seed(&self) -> Result<[u8; 32], SdkError> {
        let seed = self
            .contents
            .device_seed
            .as_ref()
            .ok_or(SdkError::NoDeviceKey)?;
        seed.as_slice()
            .try_into()
            .map_err(|_| SdkError::NoDeviceKey)
    }
}

/// Registers the identity on chain: root key plus this device, in one
/// transaction. Requires the wallet and root keys.
pub async fn create_identity_on_chain(
    account: &Account,
    network: &NetworkIdentity,
    chain: &Client,
    label: &str,
    platform: &str,
) -> Result<hashgram_chain::TxResult, SdkError> {
    let wallet = account.wallet()?;
    let root = account.root()?;
    let device = account.device()?;
    let height = chain.height().await?;
    let cert = hashgram_chain::identity::issue_certificate(
        network,
        &root,
        account.address(),
        &account.contents.device_id,
        &device.public_key(),
        0,
        (height + 1000) as i64,
    )?;
    let msg = idpb::MsgCreateIdentity {
        address: account.address().to_owned(),
        root_pubkey: root.public_key().to_vec(),
        root_key_type: idpb::KeyType::Ed25519 as i32,
        recovery: Some(idpb::RecoveryConfig {
            guardians: vec![],
            threshold: 0,
            recovery_delay_blocks: 43_200,
            recovery_hash: vec![],
        }),
        initial_device: Some(cert),
        initial_device_label: label.to_owned(),
        initial_device_platform: platform.to_owned(),
    };
    Ok(chain
        .sign_and_broadcast(
            &wallet,
            vec![msgs::create_identity(&msg)],
            "hashgram identity",
        )
        .await?)
}

/// Authorises another device: the root holder signs a certificate for the
/// device's public key and submits it. `rotation_count` is the identity's
/// current value (query it first).
#[allow(clippy::too_many_arguments)] // one call site, every argument distinct
pub async fn add_device_on_chain(
    account: &Account,
    network: &NetworkIdentity,
    chain: &Client,
    device_id: &str,
    device_pubkey: &[u8; 32],
    rotation_count: u32,
    label: &str,
    platform: &str,
) -> Result<hashgram_chain::TxResult, SdkError> {
    let wallet = account.wallet()?;
    let root = account.root()?;
    let height = chain.height().await?;
    let cert = hashgram_chain::identity::issue_certificate(
        network,
        &root,
        account.address(),
        device_id,
        device_pubkey,
        rotation_count,
        (height + 1000) as i64,
    )?;
    let msg = idpb::MsgAddDevice {
        address: account.address().to_owned(),
        certificate: Some(cert),
        label: label.to_owned(),
        platform: platform.to_owned(),
    };
    Ok(chain
        .sign_and_broadcast(&wallet, vec![msgs::add_device(&msg)], "hashgram add device")
        .await?)
}

/// Revokes a device.
pub async fn revoke_device_on_chain(
    account: &Account,
    chain: &Client,
    device_id: &str,
) -> Result<hashgram_chain::TxResult, SdkError> {
    let wallet = account.wallet()?;
    let msg = idpb::MsgRevokeDevice {
        address: account.address().to_owned(),
        device_id: device_id.to_owned(),
    };
    Ok(chain
        .sign_and_broadcast(
            &wallet,
            vec![msgs::revoke_device(&msg)],
            "hashgram revoke device",
        )
        .await?)
}

/// A device as the chain reports it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DeviceView {
    /// Device id.
    pub device_id: String,
    /// Hex public key.
    pub device_pubkey: String,
    /// Label.
    #[serde(default)]
    pub label: String,
    /// Platform.
    #[serde(default)]
    pub platform: String,
    /// Revoked.
    #[serde(default)]
    pub revoked: bool,
}

/// Lists an address's devices from the chain.
pub async fn devices_on_chain(chain: &Client, address: &str) -> Result<Vec<DeviceView>, SdkError> {
    let v = chain
        .query(&format!("hashgram/identity/v1/devices/{address}"))
        .await?;
    let list = v
        .get("devices")
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for d in list {
        let pubkey_b64 = d
            .get("device_pubkey")
            .and_then(|x| x.as_str())
            .unwrap_or("");
        let pubkey = base64_decode(pubkey_b64).unwrap_or_default();
        out.push(DeviceView {
            device_id: d
                .get("device_id")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_owned(),
            device_pubkey: hex::encode(pubkey),
            label: d
                .get("label")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_owned(),
            platform: d
                .get("platform")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_owned(),
            revoked: d.get("revoked").and_then(|x| x.as_bool()).unwrap_or(false),
        });
    }
    Ok(out)
}

/// The identity's current rotation count, or `None` if no identity.
pub async fn rotation_count_on_chain(
    chain: &Client,
    address: &str,
) -> Result<Option<u32>, SdkError> {
    let v = chain
        .query(&format!("hashgram/identity/v1/identity/{address}"))
        .await?;
    if !v.get("found").and_then(|f| f.as_bool()).unwrap_or(false) {
        return Ok(None);
    }
    Ok(Some(
        v.get("identity")
            .and_then(|i| i.get("rotation_count"))
            .and_then(|r| {
                r.as_u64()
                    .or_else(|| r.as_str().and_then(|s| s.parse().ok()))
            })
            .unwrap_or(0) as u32,
    ))
}

/// Standard base64 decode (the gateway renders bytes as base64).
pub(crate) fn base64_decode(s: &str) -> Option<Vec<u8>> {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(s.len());
    let mut buf = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        if c == b'=' {
            break;
        }
        let v = T.iter().position(|t| *t == c)? as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}
