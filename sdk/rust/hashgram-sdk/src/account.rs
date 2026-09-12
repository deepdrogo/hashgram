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

/// Words in a Hashgram account mnemonic. There is no other length.
pub const MNEMONIC_WORDS: usize = 24;

/// Derives the identity root seed from the wallet secret.
///
/// The 24 words are the account. Restoring them on a second machine must
/// yield not only the same `hash1…` address but the same identity root key,
/// or that machine could never sign a `MsgAddDevice` for itself. So the
/// root seed is a fixed derivation of the wallet secret (BLAKE3 in
/// derive-key mode with a Hashgram-specific context), never a fresh random
/// key. Only the public key goes on chain.
#[must_use]
pub fn derive_root_seed(wallet_secret: &[u8]) -> [u8; 32] {
    blake3::derive_key("hashgram identity root key v1", wallet_secret)
}

/// A device's opened account material.
pub struct Account {
    /// The vault file.
    pub vault: Vault,
    /// Passphrase, kept to re-encrypt on save when no session key is held.
    passphrase: String,
    /// Decrypted contents.
    pub contents: VaultContents,
    /// KDF cost used when writing.
    pub kdf: KdfCost,
    /// Derived vault key for this session: saves skip the KDF.
    key: Option<hashgram_identity::vault::VaultKey>,
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
        let root = Ed25519Signer::from_secret(derive_root_seed(&wallet.secret_bytes()));
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
        // One more KDF now buys KDF-free saves for the rest of the session.
        let key = vault.open_with_key(passphrase).ok().map(|(_, k)| k);
        Ok((
            Self {
                vault,
                passphrase: passphrase.to_owned(),
                contents,
                kdf,
                key,
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
        // A Hashgram account is 24 words (256-bit entropy), the only length
        // `Wallet::generate` produces. Shorter BIP-39 phrases are valid
        // wallets elsewhere but are not Hashgram accounts; refusing them
        // here keeps every client on one rule.
        let words = mnemonic.split_whitespace().count();
        if words != MNEMONIC_WORDS {
            return Err(SdkError::Invalid(format!(
                "a Hashgram account is {MNEMONIC_WORDS} words; got {words}"
            )));
        }
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
        let root = Ed25519Signer::from_secret(derive_root_seed(&wallet.secret_bytes()));
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
        // One more KDF now buys KDF-free saves for the rest of the session.
        let key = vault.open_with_key(passphrase).ok().map(|(_, k)| k);
        Ok(Self {
            vault,
            passphrase: passphrase.to_owned(),
            contents,
            kdf,
            key,
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
        // One more KDF now buys KDF-free saves for the rest of the session.
        let key = vault.open_with_key(passphrase).ok().map(|(_, k)| k);
        Ok(Self {
            vault,
            passphrase: passphrase.to_owned(),
            contents,
            kdf,
            key,
        })
    }

    /// Writes a new vault from given contents (used by backup import).
    pub fn from_contents(
        vault_path: &Path,
        passphrase: &str,
        contents: VaultContents,
        kdf: KdfCost,
    ) -> Result<Self, SdkError> {
        let vault = Vault::at(vault_path);
        vault.create(passphrase, &contents, kdf)?;
        let key = vault.open_with_key(passphrase).ok().map(|(_, k)| k);
        Ok(Self {
            vault,
            passphrase: passphrase.to_owned(),
            contents,
            kdf,
            key,
        })
    }

    /// Opens an existing vault. The derived key is kept for the session so
    /// [`Self::save`] does not repeat the KDF.
    pub fn open(vault_path: &Path, passphrase: &str, kdf: KdfCost) -> Result<Self, SdkError> {
        let vault = Vault::at(vault_path);
        let (contents, key) = vault.open_with_key(passphrase)?;
        Ok(Self {
            vault,
            passphrase: passphrase.to_owned(),
            contents,
            kdf,
            key: Some(key),
        })
    }

    /// Re-encrypts the vault with the current contents. Uses the session
    /// key when one is held (an opened vault), the passphrase otherwise (a
    /// vault just created: one more KDF, once).
    pub fn save(&self) -> Result<(), SdkError> {
        match &self.key {
            Some(k) => self.vault.write_with_key(k, &self.contents)?,
            None => self
                .vault
                .write(&self.passphrase, &self.contents, self.kdf)?,
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    const WORDS: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("hg-account-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("vault.json")
    }

    #[test]
    fn restoring_the_same_words_yields_the_same_address_and_root_key() {
        // Two machines, the same 24 words: identical address and identical
        // identity root key, so the second can sign its own MsgAddDevice.
        // Device keys differ: each machine is its own device.
        let a = Account::import(&tmp("a"), "pass-a", WORDS, "pc-1", KdfCost::light()).unwrap();
        let b = Account::import(&tmp("b"), "pass-b", WORDS, "pc-2", KdfCost::light()).unwrap();
        assert_eq!(a.address(), b.address());
        assert!(a.address().starts_with("hash1"));
        assert_eq!(
            a.root().unwrap().public_key(),
            b.root().unwrap().public_key()
        );
        assert_ne!(
            a.device().unwrap().public_key(),
            b.device().unwrap().public_key()
        );
    }

    #[test]
    fn a_created_account_restores_to_itself() {
        let (created, mnemonic) =
            Account::create(&tmp("c"), "pass", "pc-1", KdfCost::light()).unwrap();
        assert_eq!(
            mnemonic.split_whitespace().count(),
            24,
            "accounts are 24 words only"
        );
        let restored =
            Account::import(&tmp("d"), "other", &mnemonic, "pc-2", KdfCost::light()).unwrap();
        assert_eq!(created.address(), restored.address());
        assert_eq!(
            created.root().unwrap().public_key(),
            restored.root().unwrap().public_key()
        );
    }

    #[test]
    fn twelve_words_are_refused() {
        // The BIP-39 parser would accept a 12-word phrase; the account layer
        // is where Hashgram's rule lives: 24 words, nothing else.
        let twelve = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let path = tmp("e");
        let err = match Account::import(&path, "p", twelve, "pc", KdfCost::light()) {
            Ok(_) => panic!("a 12-word phrase was accepted"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("24 words"), "{err}");
        assert!(!path.exists(), "no vault is written for a refused phrase");
    }
}
