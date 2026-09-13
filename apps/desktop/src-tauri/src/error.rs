//! The one error type a command returns to the webview.
//!
//! `SdkError` variants are mapped to a small set of stable codes the UI
//! switches on (offline banner, "recipient has no device" hint, retry
//! button, update prompt). The message is plain English for the toast;
//! never a `Debug` dump.

use hashgram_sdk::SdkError;
use serde::Serialize;

/// A user-facing error.
#[derive(Debug, Clone, Serialize, thiserror::Error)]
#[error("{message}")]
pub struct UiError {
    /// Stable code: `locked`, `offline`, `no_recipient_device`, `unsupported`,
    /// `no_funds`, `delivery`, `invalid`, `not_found`, `corrupt`, `store`,
    /// `no_wallet_key`, `no_root_key`, `chain`, `internal`.
    pub code: &'static str,
    /// What happened, in words a person can act on.
    pub message: String,
    /// Whether trying again may help (network, delivery).
    pub retryable: bool,
}

impl UiError {
    /// An error with a code.
    #[must_use]
    pub fn new(code: &'static str, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code,
            message: message.into(),
            retryable,
        }
    }
    /// The vault is locked.
    #[must_use]
    pub fn locked() -> Self {
        Self::new("locked", "the vault is locked; unlock to continue", false)
    }
    /// User input the command refused.
    #[must_use]
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new("invalid", message, false)
    }
    /// Something is missing.
    #[must_use]
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new("not_found", message, false)
    }
    /// A local failure that is not the user's doing.
    #[must_use]
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new("internal", message, false)
    }
    /// The network is not reachable right now.
    #[must_use]
    pub fn offline(message: impl Into<String>) -> Self {
        Self::new("offline", message, true)
    }
    /// This local device has not been authorised on chain yet.
    #[must_use]
    pub fn identity_not_registered() -> Self {
        Self::new(
            "identity_not_registered",
            "This PC is not registered for your identity yet. Receive at least 0.01 HASH, then open Wallet → Devices and register this device before posting or sending mail.",
            false,
        )
    }
}

impl From<SdkError> for UiError {
    fn from(e: SdkError) -> Self {
        match e {
            SdkError::Link(hashgram_sdk::link::LinkError::NoPeer(role)) => Self::offline(format!(
                "offline — no {role} node is reachable yet; showing what is on this device"
            )),
            SdkError::Link(other) => {
                let message = other.to_string();
                if message.to_ascii_lowercase().contains("not an active device") {
                    Self::identity_not_registered()
                } else {
                    Self::new("offline", format!("network: {message}"), true)
                }
            }
            SdkError::NoRecipients | SdkError::NoKeyPackage(_) => Self::new(
                "no_recipient_device",
                "the recipient has no device online yet — they must open Hashgram once while connected",
                true,
            ),
            SdkError::Unsupported(what) => Self::new(
                "unsupported",
                format!("this needs a newer Hashgram to read ({what}); update the app"),
                false,
            ),
            SdkError::Chain(hashgram_sdk::chain::ClientError::NoAccount(_)) => Self::new(
                "no_funds",
                "this account has no HASH yet; receive some to pay the network fee",
                false,
            ),
            SdkError::Chain(hashgram_sdk::chain::ClientError::NoNode(_)) => {
                Self::offline("offline — no node answers chain reads yet")
            }
            SdkError::Chain(hashgram_sdk::chain::ClientError::Disputed(d)) => Self::new(
                "disputed",
                format!("nodes disagree about the chain state; nothing was trusted ({d})"),
                true,
            ),
            SdkError::Chain(hashgram_sdk::chain::ClientError::Rejected { code, log }) => {
                let lower = log.to_ascii_lowercase();
                if lower.contains("insufficient funds") || lower.contains("insufficient fee") {
                    Self::new("no_funds", "not enough HASH for this transaction and its fee", false)
                } else {
                    Self::new("chain", format!("the chain rejected the transaction (code {code}): {log}"), false)
                }
            }
            SdkError::Chain(hashgram_sdk::chain::ClientError::Gateway { status: 404, .. }) => {
                Self::not_found("not found on chain")
            }
            SdkError::Chain(c) => {
                let s = c.to_string();
                let lower = s.to_ascii_lowercase();
                if lower.contains("insufficient funds") || lower.contains("insufficient fee") {
                    Self::new("no_funds", "not enough HASH for this transaction and its fee", false)
                } else if lower.contains("no verified peer") || lower.contains("unreachable") {
                    Self::offline("offline — no node answers chain reads yet")
                } else {
                    Self::new("chain", format!("chain: {s}"), true)
                }
            }
            SdkError::Delivery(d) => {
                if d.to_ascii_lowercase().contains("not an active device") {
                    Self::identity_not_registered()
                } else {
                    Self::new("delivery", format!("not delivered: {d}"), true)
                }
            }
            SdkError::Invalid(m) => Self::invalid(m),
            SdkError::NotFound(m) => Self::not_found(format!("not found: {m}")),
            SdkError::Corrupt(m) => Self::new("corrupt", format!("a node served corrupt data: {m}"), true),
            SdkError::Store(m) => Self::new("store", format!("local store: {m}"), false),
            SdkError::NoWalletKey => Self::new(
                "no_wallet_key",
                "this device does not hold the wallet key; restore from the 24 words to sign",
                false,
            ),
            SdkError::NoRootKey => Self::new(
                "no_root_key",
                "this device does not hold the identity root key; use the device that created the identity",
                false,
            ),
            SdkError::NoDeviceKey => Self::internal("the vault has no device key"),
            SdkError::Vault(v) => {
                let s = v.to_string();
                if s.contains("decrypt") || s.contains("passphrase") || s.contains("aead") || s.contains("tag") {
                    Self::new("wrong_passphrase", "wrong passphrase", false)
                } else {
                    Self::new("vault", format!("vault: {s}"), false)
                }
            }
            SdkError::Wallet(w) => Self::invalid(format!("wallet: {w}")),
            SdkError::Mls(m) => Self::new("mls", format!("encryption session: {m}"), true),
            SdkError::Sign(s) => Self::internal(format!("signing: {s}")),
            SdkError::Canonical(c) => Self::internal(format!("encoding: {c}")),
            SdkError::Key(k) => Self::internal(format!("key: {k}")),
        }
    }
}

impl From<hashgram_sdk::AppError> for UiError {
    fn from(e: hashgram_sdk::AppError) -> Self {
        SdkError::from(e).into()
    }
}

impl From<String> for UiError {
    fn from(s: String) -> Self {
        Self::internal(s)
    }
}

impl From<&str> for UiError {
    fn from(s: &str) -> Self {
        Self::internal(s.to_owned())
    }
}

impl From<std::io::Error> for UiError {
    fn from(e: std::io::Error) -> Self {
        Self::new("io", format!("file: {e}"), false)
    }
}

/// Shorthand for command results.
pub type CmdResult<T> = Result<T, UiError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_errors_map_to_stable_codes() {
        let e: UiError = SdkError::Link(hashgram_sdk::link::LinkError::NoPeer("store")).into();
        assert_eq!(e.code, "offline");
        assert!(e.retryable);
        let e: UiError = SdkError::NoRecipients.into();
        assert_eq!(e.code, "no_recipient_device");
        let e: UiError = SdkError::NoKeyPackage("abcd".into()).into();
        assert_eq!(e.code, "no_recipient_device");
        let e: UiError = SdkError::Link(hashgram_sdk::link::LinkError::Refused {
            code: "invalid".into(),
            message: "device abc is not an active device of hash1x".into(),
        })
        .into();
        assert_eq!(e.code, "identity_not_registered");
        assert!(!e.retryable);
        let e: UiError = SdkError::Unsupported("mail v9".into()).into();
        assert_eq!(e.code, "unsupported");
        assert!(e.message.contains("update"));
        let e: UiError = SdkError::Chain(hashgram_sdk::chain::ClientError::NoAccount("hash1x".into())).into();
        assert_eq!(e.code, "no_funds");
        let e: UiError = SdkError::Delivery("bcc x: timeout".into()).into();
        assert_eq!(e.code, "delivery");
        assert!(e.retryable);
        let e: UiError = SdkError::Invalid("space rule: actor may not post".into()).into();
        assert_eq!(e.code, "invalid");
        assert!(!e.retryable);
        let e: UiError = SdkError::NoWalletKey.into();
        assert_eq!(e.code, "no_wallet_key");
        // Never a Debug dump.
        for e in [
            UiError::from(SdkError::NoRecipients),
            UiError::from(SdkError::Store("x".into())),
        ] {
            assert!(!e.message.contains("SdkError"));
            assert!(!e.message.contains("{"));
        }
    }
}
