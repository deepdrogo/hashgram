//! Account keys.
//!
//! A wallet is a secp256k1 key derived from a BIP-39 mnemonic on the Cosmos
//! path, exactly as `hashgram-keygen` and `hashgramd keys` derive it, so a
//! mnemonic written down from one tool restores the same address in every
//! other. The mnemonic is the backup; the wallet never writes it anywhere.

use cosmrs::bip32;
use cosmrs::crypto::secp256k1::SigningKey;
use cosmrs::crypto::PublicKey;
use cosmrs::AccountId;

/// The Hashgram bech32 prefix.
pub const BECH32_PREFIX: &str = "hash";

/// The base denomination.
pub const DENOM: &str = "uhash";

/// SLIP-0044 coin type, shared with the Go tooling.
pub const COIN_TYPE: u32 = 118;

/// Why a wallet could not be built.
#[derive(Debug, thiserror::Error)]
pub enum WalletError {
    /// Not a valid BIP-39 mnemonic.
    #[error("mnemonic is not valid BIP-39: {0}")]
    Mnemonic(String),
    /// Key derivation failed.
    #[error("key derivation failed: {0}")]
    Derivation(String),
    /// Raw key bytes not a valid secp256k1 scalar.
    #[error("invalid secp256k1 secret")]
    InvalidSecret,
    /// Randomness unavailable.
    #[error("the OS random source failed")]
    NoRandomness,
}

/// An account key.
pub struct Wallet {
    key: SigningKey,
    secret: [u8; 32],
    address: AccountId,
}

impl std::fmt::Debug for Wallet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Wallet({})", self.address)
    }
}

impl Wallet {
    /// Generates a new 24-word mnemonic and the wallet for account index 0.
    pub fn generate() -> Result<(String, Self), WalletError> {
        let mut entropy = [0u8; 32];
        getrandom::fill(&mut entropy).map_err(|_| WalletError::NoRandomness)?;
        let m = bip39::Mnemonic::from_entropy(&entropy)
            .map_err(|e| WalletError::Mnemonic(e.to_string()))?;
        let phrase = m.to_string();
        let wallet = Self::from_mnemonic(&phrase, "", 0)?;
        Ok((phrase, wallet))
    }

    /// Restores a wallet from a mnemonic, optional BIP-39 passphrase and
    /// account index (`m/44'/118'/0'/0/index`).
    pub fn from_mnemonic(phrase: &str, passphrase: &str, index: u32) -> Result<Self, WalletError> {
        let m = bip39::Mnemonic::parse_normalized(phrase)
            .map_err(|e| WalletError::Mnemonic(e.to_string()))?;
        let seed = m.to_seed_normalized(passphrase);
        let path: bip32::DerivationPath = format!("m/44'/{COIN_TYPE}'/0'/0/{index}")
            .parse()
            .map_err(|e: bip32::Error| WalletError::Derivation(e.to_string()))?;
        let xprv = bip32::XPrv::derive_from_path(seed, &path)
            .map_err(|e| WalletError::Derivation(e.to_string()))?;
        let secret: [u8; 32] = xprv.private_key().to_bytes().into();
        Self::from_secret(&secret)
    }

    /// Wraps a raw 32-byte secret.
    pub fn from_secret(secret: &[u8]) -> Result<Self, WalletError> {
        let secret: [u8; 32] = secret.try_into().map_err(|_| WalletError::InvalidSecret)?;
        let key = SigningKey::from_slice(&secret).map_err(|_| WalletError::InvalidSecret)?;
        let address = key
            .public_key()
            .account_id(BECH32_PREFIX)
            .map_err(|e| WalletError::Derivation(e.to_string()))?;
        Ok(Self {
            key,
            secret,
            address,
        })
    }

    /// The `hash1…` address.
    #[must_use]
    pub fn address(&self) -> &AccountId {
        &self.address
    }

    /// The compressed public key.
    #[must_use]
    pub fn public_key(&self) -> PublicKey {
        self.key.public_key()
    }

    /// The 32-byte secret, for the caller's encrypted keystore.
    #[must_use]
    pub fn secret_bytes(&self) -> [u8; 32] {
        self.secret
    }

    pub(crate) fn signing_key(&self) -> &SigningKey {
        &self.key
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_mnemonic_derives_a_stable_hash_address() {
        // The BIP-39 test vector "abandon ... about" on the Cosmos path gives
        // cosmos1... at index 0 in every Cosmos wallet; with the hash prefix
        // the same key bytes render as hash1... . Only the HRP differs.
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let w = Wallet::from_mnemonic(phrase, "", 0).unwrap();
        let addr = w.address().to_string();
        assert!(addr.starts_with("hash1"), "{addr}");
        // Deterministic.
        let again = Wallet::from_mnemonic(phrase, "", 0).unwrap();
        assert_eq!(again.address(), w.address());
        // Different index, different address.
        let other = Wallet::from_mnemonic(phrase, "", 1).unwrap();
        assert_ne!(other.address(), w.address());
        // Round trip through the raw secret.
        let back = Wallet::from_secret(&w.secret_bytes()).unwrap();
        assert_eq!(back.address(), w.address());
    }

    #[test]
    fn generate_produces_24_words() {
        let (phrase, w) = Wallet::generate().unwrap();
        assert_eq!(phrase.split_whitespace().count(), 24);
        assert!(w.address().to_string().starts_with("hash1"));
        let restored = Wallet::from_mnemonic(&phrase, "", 0).unwrap();
        assert_eq!(restored.address(), w.address());
    }

    #[test]
    fn a_bad_mnemonic_is_refused() {
        assert!(Wallet::from_mnemonic("not a mnemonic", "", 0).is_err());
    }
}
