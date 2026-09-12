//! `gateway.toml`: everything an operator decides, validated once at start.
//!
//! The file is split into sections that mirror the trust boundaries of the
//! gateway: `[hashgram]` is how the bridge identity reaches the network,
//! `[smtp]` is the Internet-facing listener, `[domain]` is what we claim to
//! be when we speak outward, `[policy]` is what we refuse, `[store]` and
//! `[http]` are local. Secrets (the vault passphrase) never live in the
//! file: they come from the environment so the file can be world-readable
//! in a container image without leaking the identity.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use hashgram_sdk::NetworkIdentity;
use serde::Deserialize;

use crate::GatewayError;

/// Environment variable carrying the vault passphrase.
pub const PASSPHRASE_ENV: &str = "HASHGRAM_GATEWAY_PASSPHRASE";

/// Default SMTP listener: loopback, unprivileged. A fronting MTA or proxy
/// terminates TLS and forwards here; see `docs/MAIL_GATEWAY.md`.
pub const DEFAULT_SMTP_LISTEN: &str = "127.0.0.1:2525";
/// Default maximum message size accepted by `DATA` (25 MiB, what the big
/// providers accept).
pub const DEFAULT_MAX_MESSAGE_BYTES: u64 = 25 * 1024 * 1024;

/// The whole file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayConfig {
    /// Bridge identity and network.
    pub hashgram: HashgramSection,
    /// Inbound SMTP listener.
    #[serde(default)]
    pub smtp: SmtpSection,
    /// The mail domain we serve.
    pub domain: DomainSection,
    /// Acceptance policy.
    #[serde(default)]
    pub policy: PolicySection,
    /// Local state.
    pub store: StoreSection,
    /// Health and metrics.
    #[serde(default)]
    pub http: HttpSection,
    /// Outbound relay settings.
    #[serde(default)]
    pub outbound: OutboundSection,
}

/// `[hashgram]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HashgramSection {
    /// Directory holding the bridge identity's vault, local store and
    /// peerstore (`hashgram_sdk::Paths` layout).
    pub home: PathBuf,
    /// `"mainnet"` or `"devnet"`.
    pub network: String,
    /// Genesis hash pin. Optional on mainnet (the compiled-in value is
    /// used), required on devnet because there is no canonical devnet.
    #[serde(default)]
    pub genesis_hash: Option<String>,
    /// Optional REST chain API; `None` reads the chain through the P2P
    /// relay.
    #[serde(default)]
    pub chain_api: Option<String>,
    /// Bootstrap multiaddrs; empty means the compiled-in mainnet list.
    #[serde(default)]
    pub bootstrap: Vec<String>,
    /// Seconds to wait for a verified peer at start (default 10).
    #[serde(default = "default_connect_wait_secs")]
    pub connect_wait_secs: u64,
    /// Seconds between sync rounds (default 5).
    #[serde(default = "default_sync_interval_secs")]
    pub sync_interval_secs: u64,
    /// Use the light Argon2 cost for the vault. DEVNET ONLY.
    #[serde(default)]
    pub light_kdf: bool,
}

fn default_connect_wait_secs() -> u64 {
    10
}
fn default_sync_interval_secs() -> u64 {
    5
}

/// `[smtp]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SmtpSection {
    /// Listen address.
    #[serde(default = "default_listen")]
    pub listen: String,
    /// Allow a port below 1024. Off by default: this process should not run
    /// as root, and port 25 belongs to the fronting MTA.
    #[serde(default)]
    pub allow_privileged: bool,
    /// Hostname announced in the banner and `EHLO` reply; defaults to the
    /// domain name.
    #[serde(default)]
    pub hostname: Option<String>,
    /// Maximum `DATA` size.
    #[serde(default = "default_max_message_bytes")]
    pub max_message_bytes: u64,
    /// Connections accepted per source IP per minute (0 = unlimited).
    #[serde(default = "default_conn_per_min")]
    pub connections_per_ip_per_minute: u32,
    /// Messages accepted per source IP per hour (0 = unlimited).
    #[serde(default = "default_msgs_per_hour")]
    pub messages_per_ip_per_hour: u32,
    /// Recipients per message.
    #[serde(default = "default_max_rcpt")]
    pub max_recipients: usize,
    /// Idle timeout per connection, seconds.
    #[serde(default = "default_idle_secs")]
    pub idle_timeout_secs: u64,
    /// Concurrent connections.
    #[serde(default = "default_max_conns")]
    pub max_connections: usize,
}

fn default_listen() -> String {
    DEFAULT_SMTP_LISTEN.to_owned()
}
fn default_max_message_bytes() -> u64 {
    DEFAULT_MAX_MESSAGE_BYTES
}
fn default_conn_per_min() -> u32 {
    60
}
fn default_msgs_per_hour() -> u32 {
    300
}
fn default_max_rcpt() -> usize {
    50
}
fn default_idle_secs() -> u64 {
    300
}
fn default_max_conns() -> usize {
    200
}

impl Default for SmtpSection {
    fn default() -> Self {
        Self {
            listen: default_listen(),
            allow_privileged: false,
            hostname: None,
            max_message_bytes: default_max_message_bytes(),
            connections_per_ip_per_minute: default_conn_per_min(),
            messages_per_ip_per_hour: default_msgs_per_hour(),
            max_recipients: default_max_rcpt(),
            idle_timeout_secs: default_idle_secs(),
            max_connections: default_max_conns(),
        }
    }
}

/// `[domain]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainSection {
    /// The domain whose local parts are Hashgram usernames.
    pub name: String,
    /// DKIM selector (`<selector>._domainkey.<name>`). Signing is enabled
    /// when both selector and key file are set.
    #[serde(default)]
    pub dkim_selector: Option<String>,
    /// DKIM private key: PKCS#8/PKCS#1 PEM for rsa-sha256, or a 32-byte
    /// ed25519 seed (PEM `PRIVATE KEY`, raw base64 or hex) for
    /// ed25519-sha256.
    #[serde(default)]
    pub dkim_private_key_file: Option<PathBuf>,
}

/// `[policy]`. Every default is "accept": the gateway is a bridge, not a
/// filter, until the operator says otherwise.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicySection {
    /// Refuse inbound mail whose `Authentication-Results` (added by the
    /// fronting MTA) shows neither `spf=pass` nor `dkim=pass`.
    #[serde(default)]
    pub require_spf_or_dkim: bool,
    /// Refuse inbound mail whose spam score (0..1000) exceeds this; 0
    /// disables the check.
    #[serde(default)]
    pub reject_spam_score_over: u32,
    /// Relay outbound only for Hashgram users who are contacts of the
    /// bridge identity.
    #[serde(default)]
    pub allow_outbound_from_contacts_only: bool,
}

/// `[store]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreSection {
    /// SQLite file for the inbound log, retry queue, thread map, cursors.
    pub sqlite_path: PathBuf,
}

/// `[http]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpSection {
    /// Loopback address for `/healthz` and `/metrics`; empty disables.
    #[serde(default = "default_http_listen")]
    pub listen: String,
}

fn default_http_listen() -> String {
    "127.0.0.1:9725".to_owned()
}

impl Default for HttpSection {
    fn default() -> Self {
        Self {
            listen: default_http_listen(),
        }
    }
}

/// `[outbound]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboundSection {
    /// Messages one Hashgram user may relay per hour.
    #[serde(default = "default_per_user_per_hour")]
    pub per_user_per_hour: u32,
    /// Refuse to deliver in plaintext when the remote MX does not offer
    /// STARTTLS or its certificate fails verification. Off by default:
    /// Internet mail is opportunistic-TLS, and a hard requirement bounces
    /// mail to every legacy MX.
    #[serde(default)]
    pub require_tls: bool,
    /// Connect / command timeout per remote MX, seconds.
    #[serde(default = "default_smtp_timeout_secs")]
    pub smtp_timeout_secs: u64,
    /// Remote SMTP port. 25 in production; tests and lab setups override.
    #[serde(default = "default_smtp_port")]
    pub smtp_port: u16,
    /// Give up (bounce) after this many delivery attempts.
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
    /// Optional smarthost (`host:port`) used instead of MX lookup — for
    /// operators whose provider forbids direct port-25 egress.
    #[serde(default)]
    pub smarthost: Option<String>,
}

fn default_per_user_per_hour() -> u32 {
    100
}
fn default_smtp_timeout_secs() -> u64 {
    60
}
fn default_smtp_port() -> u16 {
    25
}
fn default_max_attempts() -> u32 {
    12
}

impl Default for OutboundSection {
    fn default() -> Self {
        Self {
            per_user_per_hour: default_per_user_per_hour(),
            require_tls: false,
            smtp_timeout_secs: default_smtp_timeout_secs(),
            smtp_port: default_smtp_port(),
            max_attempts: default_max_attempts(),
            smarthost: None,
        }
    }
}

impl GatewayConfig {
    /// Parses and validates a TOML document.
    pub fn from_toml(text: &str) -> Result<Self, GatewayError> {
        let cfg: Self = toml::from_str(text).map_err(|e| GatewayError::Config(e.to_string()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Reads and validates a file.
    pub fn load(path: &Path) -> Result<Self, GatewayError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| GatewayError::Config(format!("{}: {e}", path.display())))?;
        Self::from_toml(&text)
    }

    /// The invariants a running gateway relies on. Checked once so the rest
    /// of the code can treat the config as trusted.
    pub fn validate(&self) -> Result<(), GatewayError> {
        let listen = self.smtp_listen()?;
        if listen.port() < 1024 && !self.smtp.allow_privileged {
            return Err(GatewayError::Config(format!(
                "smtp.listen port {} is privileged; set smtp.allow_privileged = true if you really mean it",
                listen.port()
            )));
        }
        if self.smtp.max_message_bytes < 1024 {
            return Err(GatewayError::Config(
                "smtp.max_message_bytes is too small".into(),
            ));
        }
        if self.smtp.max_recipients == 0
            || self.smtp.max_recipients > hashgram_app::mail::MAX_RECIPIENTS
        {
            return Err(GatewayError::Config(format!(
                "smtp.max_recipients must be 1..={}",
                hashgram_app::mail::MAX_RECIPIENTS
            )));
        }
        let d = self.domain.name.trim();
        if d.is_empty() || !d.contains('.') || d.contains('@') || d.chars().any(char::is_whitespace)
        {
            return Err(GatewayError::Config(format!(
                "domain.name {d:?} is not a hostname"
            )));
        }
        if self.domain.dkim_selector.is_some() != self.domain.dkim_private_key_file.is_some() {
            return Err(GatewayError::Config(
                "domain.dkim_selector and domain.dkim_private_key_file must be set together".into(),
            ));
        }
        if let Some(sel) = &self.domain.dkim_selector {
            if sel.is_empty()
                || !sel
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
            {
                return Err(GatewayError::Config(format!(
                    "domain.dkim_selector {sel:?} is not a DNS label"
                )));
            }
        }
        if self.policy.reject_spam_score_over > 1000 {
            return Err(GatewayError::Config(
                "policy.reject_spam_score_over must be 0..=1000".into(),
            ));
        }
        match self.hashgram.network.as_str() {
            "mainnet" => {}
            "devnet" => {
                if self
                    .hashgram
                    .genesis_hash
                    .as_deref()
                    .unwrap_or("")
                    .is_empty()
                {
                    return Err(GatewayError::Config(
                        "hashgram.genesis_hash is required on devnet".into(),
                    ));
                }
            }
            other => {
                return Err(GatewayError::Config(format!(
                    "hashgram.network must be \"mainnet\" or \"devnet\", got {other:?}"
                )))
            }
        }
        if let Some(h) = &self.hashgram.genesis_hash {
            if h.len() != 64 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(GatewayError::Config(
                    "hashgram.genesis_hash must be 64 hex characters".into(),
                ));
            }
        }
        if !self.http.listen.is_empty() {
            let a: SocketAddr = self
                .http
                .listen
                .parse()
                .map_err(|e| GatewayError::Config(format!("http.listen: {e}")))?;
            if !a.ip().is_loopback() {
                return Err(GatewayError::Config(
                    "http.listen must be a loopback address; expose metrics through your own proxy"
                        .into(),
                ));
            }
        }
        if let Some(s) = &self.outbound.smarthost {
            if s.rsplit_once(':')
                .and_then(|(_, p)| p.parse::<u16>().ok())
                .is_none()
            {
                return Err(GatewayError::Config(
                    "outbound.smarthost must be host:port".into(),
                ));
            }
        }
        if self.outbound.max_attempts == 0 {
            return Err(GatewayError::Config(
                "outbound.max_attempts must be at least 1".into(),
            ));
        }
        Ok(())
    }

    /// Parsed SMTP listen address.
    pub fn smtp_listen(&self) -> Result<SocketAddr, GatewayError> {
        self.smtp
            .listen
            .parse()
            .map_err(|e| GatewayError::Config(format!("smtp.listen: {e}")))
    }

    /// Hostname for the SMTP banner.
    #[must_use]
    pub fn smtp_hostname(&self) -> &str {
        self.smtp.hostname.as_deref().unwrap_or(&self.domain.name)
    }

    /// The network pin the bridge identity connects to.
    pub fn network_identity(&self) -> Result<NetworkIdentity, GatewayError> {
        match self.hashgram.network.as_str() {
            "mainnet" => Ok(NetworkIdentity::mainnet(
                self.hashgram
                    .genesis_hash
                    .as_deref()
                    .unwrap_or(hashgram_sdk::net::MAINNET_GENESIS_HASH),
            )),
            "devnet" => Ok(NetworkIdentity::devnet(
                self.hashgram.genesis_hash.as_deref().unwrap_or(""),
            )),
            other => Err(GatewayError::Config(format!("unknown network {other:?}"))),
        }
    }

    /// The SDK `Config` for the bridge identity.
    pub fn sdk_config(&self) -> Result<hashgram_sdk::Config, GatewayError> {
        let bootstrap = self
            .hashgram
            .bootstrap
            .iter()
            .map(|s| {
                s.parse::<hashgram_sdk::Multiaddr>()
                    .map_err(|e| GatewayError::Config(format!("bootstrap {s}: {e}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(hashgram_sdk::Config {
            paths: hashgram_sdk::Paths::new(&self.hashgram.home),
            network: self.network_identity()?,
            bootstrap,
            chain_api: self
                .hashgram
                .chain_api
                .clone()
                .filter(|s| !s.trim().is_empty()),
            kdf: if self.hashgram.light_kdf {
                hashgram_sdk::KdfCost::light()
            } else {
                hashgram_sdk::KdfCost::default()
            },
            connect_wait: Duration::from_secs(self.hashgram.connect_wait_secs),
        })
    }

    /// Reads the passphrase from the environment.
    pub fn passphrase() -> Result<String, GatewayError> {
        std::env::var(PASSPHRASE_ENV)
            .ok()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| GatewayError::Config(format!("{PASSPHRASE_ENV} is not set")))
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
[hashgram]
home = "/var/lib/hashgram-mail-gateway"
network = "mainnet"

[domain]
name = "hashgram.io"

[store]
sqlite_path = "/var/lib/hashgram-mail-gateway/gateway.sqlite"
"#;

    #[test]
    fn minimal_parses_with_defaults() {
        let c = GatewayConfig::from_toml(MINIMAL).expect("parses");
        assert_eq!(c.smtp.listen, DEFAULT_SMTP_LISTEN);
        assert_eq!(c.smtp.max_message_bytes, DEFAULT_MAX_MESSAGE_BYTES);
        assert_eq!(c.smtp_hostname(), "hashgram.io");
        assert!(!c.policy.require_spf_or_dkim);
        assert_eq!(c.outbound.smtp_port, 25);
        assert!(c.network_identity().expect("mainnet").is_mainnet());
        assert!(c.sdk_config().is_ok());
    }

    #[test]
    fn privileged_port_needs_opt_in() {
        let text = MINIMAL.replace("[domain]", "[smtp]\nlisten = \"0.0.0.0:25\"\n\n[domain]");
        let err = GatewayConfig::from_toml(&text).expect_err("refused");
        assert!(err.to_string().contains("privileged"));
        let text = text.replace(
            "listen = \"0.0.0.0:25\"",
            "listen = \"0.0.0.0:25\"\nallow_privileged = true",
        );
        assert!(GatewayConfig::from_toml(&text).is_ok());
    }

    #[test]
    fn devnet_requires_genesis_and_dkim_pair() {
        let text = MINIMAL.replace("\"mainnet\"", "\"devnet\"");
        assert!(GatewayConfig::from_toml(&text).is_err());
        let text = text.replace(
            "network = \"devnet\"",
            "network = \"devnet\"\ngenesis_hash = \"0000000000000000000000000000000000000000000000000000000000000000\"",
        );
        assert!(GatewayConfig::from_toml(&text).is_ok());
        let text = MINIMAL.replace(
            "name = \"hashgram.io\"",
            "name = \"hashgram.io\"\ndkim_selector = \"s1\"",
        );
        assert!(GatewayConfig::from_toml(&text).is_err());
    }

    #[test]
    fn unknown_keys_and_bad_values_are_refused() {
        assert!(GatewayConfig::from_toml(&format!("{MINIMAL}\n[policy]\nbogus = 1\n")).is_err());
        assert!(GatewayConfig::from_toml(&format!(
            "{MINIMAL}\n[policy]\nreject_spam_score_over = 2000\n"
        ))
        .is_err());
        assert!(GatewayConfig::from_toml(&format!(
            "{MINIMAL}\n[http]\nlisten = \"0.0.0.0:9725\"\n"
        ))
        .is_err());
        assert!(GatewayConfig::from_toml(&format!(
            "{MINIMAL}\n[outbound]\nsmarthost = \"nohost\"\n"
        ))
        .is_err());
    }
}
