//! Hashgram external mail gateway.
//!
//! An SMTP compatibility bridge between the public Internet and HashMail.
//! It owns an ordinary Hashgram identity (the *bridge identity*) and no
//! user keys:
//!
//! * **Inbound** (Internet → HashMail): a minimal SMTP server accepts
//!   mail for `<username>@<domain>`, parses the MIME, and sends a
//!   `MailMessage{origin: EXTERNAL_GATEWAY, external: …}` through MLS to
//!   the on-chain owner of `username`.
//! * **Outbound** (HashMail → Internet): a Hashgram user sends native mail
//!   *to the bridge identity* carrying an `ext-to:<address>` label; the
//!   gateway renders RFC 5322 MIME, DKIM-signs it and relays it to the
//!   recipient's MX.
//!
//! Everything the gateway sees in plaintext is, by construction, mail
//! that was (or is about to be) plaintext on the Internet. Native HashMail
//! between Hashgram identities never touches it. Architecture, trust
//! model, DNS, deployment and runbook: `docs/MAIL_GATEWAY.md`.
//!
//! Module map:
//!
//! | module | purpose |
//! | --- | --- |
//! | [`config`] | `gateway.toml` schema and validation |
//! | [`address`] | `alice@domain` ↔ username mapping |
//! | [`smtp`] | RFC 5321 server (inbound) and client (outbound, STARTTLS) |
//! | [`mime`] | RFC 5322/MIME parse (inbound) and render (outbound) |
//! | [`thread`] | `Message-ID` ↔ 16-byte id derivation |
//! | [`dkim`] | RFC 6376 relaxed/relaxed signing |
//! | [`dns`] | MX lookup |
//! | [`policy`] | acceptance policy |
//! | [`ratelimit`] | per-IP limits |
//! | [`store`] | SQLite: inbound log, queue, thread map, cursors |
//! | [`metrics`] | `/healthz`, `/metrics` |
//! | [`bridge`] | the glue to `hashgram_sdk::HashgramOne` |

pub mod address;
pub mod bridge;
pub mod config;
pub mod dkim;
pub mod dns;
pub mod metrics;
pub mod mime;
pub mod policy;
pub mod ratelimit;
pub mod smtp;
pub mod store;
pub mod thread;

/// Everything that can go wrong inside the gateway.
#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    /// `gateway.toml`.
    #[error("config: {0}")]
    Config(String),
    /// An address that is not a mailbox we understand.
    #[error("address: {0}")]
    Address(String),
    /// SQLite.
    #[error("store: {0}")]
    Store(String),
    /// MIME parsing.
    #[error("mime: {0}")]
    Mime(String),
    /// DKIM key or signing.
    #[error("dkim: {0}")]
    Dkim(String),
    /// DNS.
    #[error("dns: {0}")]
    Dns(String),
    /// The Hashgram SDK.
    #[error(transparent)]
    Sdk(#[from] hashgram_sdk::SdkError),
    /// The application layer.
    #[error(transparent)]
    App(#[from] hashgram_app::AppError),
    /// I/O.
    #[error("io: {0}")]
    Io(String),
}

impl From<std::io::Error> for GatewayError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}
