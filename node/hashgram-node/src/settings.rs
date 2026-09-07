//! Configuration assembly.
//!
//! Three files, one owner each:
//!
//! - `/etc/hashgram/node.toml` — P2P tuning, written by the installer and
//!   edited by the operator.
//! - `/etc/hashgram/network.json` — the pinned network identity, written by
//!   `hashgramctl join-mainnet` or `init-mainnet-genesis` after verifying the
//!   genesis hash. This is the file that decides which network the node is
//!   on, and the daemon refuses to start without it.
//! - `/etc/hashgram/roles.json` — which roles this machine serves, written by
//!   `hashgramctl configure-role`.
//!
//! `node.toml` may repeat the network and genesis hash. If it does and they
//! disagree with the pin file, that is a startup failure: two files claiming
//! different networks is exactly the situation where guessing is wrong.

use std::path::Path;

use anyhow::{bail, Context};
use hashgram_net::NetworkIdentity;
use hashgram_p2p::NodeConfig;
use serde::Deserialize;

/// The pin file, as `hashgramctl` writes it.
#[derive(Debug, Deserialize)]
pub struct NetworkFile {
    /// `hashgram-mainnet` or `hashgram-devnet`.
    pub network_id: String,
    /// The pin.
    pub genesis_hash: String,
}

/// The roles file, as `hashgramctl` writes it.
#[derive(Debug, Default, Deserialize)]
pub struct RolesFile {
    /// Roles.
    #[serde(default)]
    pub roles: Vec<String>,
    /// Reward address.
    #[serde(default)]
    pub reward_address: String,
    /// Declared storage.
    #[serde(default)]
    pub declared_storage_bytes: u64,
}

/// The assembled configuration.
#[derive(Debug)]
pub struct Settings {
    /// P2P configuration with the pin and roles filled in.
    pub config: NodeConfig,
    /// The resolved network identity.
    pub identity: NetworkIdentity,
}

/// Loads and cross-checks the three files.
pub fn load(config_path: &Path, home: Option<&Path>) -> anyhow::Result<Settings> {
    let raw = std::fs::read_to_string(config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;
    let mut config: NodeConfig =
        toml::from_str(&raw).with_context(|| format!("parsing {}", config_path.display()))?;

    let etc = config_path
        .parent()
        .unwrap_or_else(|| Path::new("/etc/hashgram"));

    // The pin.
    let pin_path = etc.join("network.json");
    match std::fs::read(&pin_path) {
        Ok(bytes) => {
            let pin: NetworkFile = serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing {}", pin_path.display()))?;
            let network = match pin.network_id.as_str() {
                "hashgram-mainnet" => "mainnet",
                "hashgram-devnet" => "devnet",
                other => bail!("{} pins unknown network id {other:?}", pin_path.display()),
            };
            if !config.network.is_empty() && config.network != network {
                bail!(
                    "{} says network = {:?} but {} pins {:?}; refusing to guess which is right",
                    config_path.display(),
                    config.network,
                    pin_path.display(),
                    network
                );
            }
            let pinned = pin.genesis_hash.to_ascii_lowercase();
            if !config.genesis_hash.is_empty() && config.genesis_hash.to_ascii_lowercase() != pinned
            {
                bail!(
                    "{} and {} disagree on the genesis hash; refusing to start on an ambiguous pin",
                    config_path.display(),
                    pin_path.display()
                );
            }
            config.network = network.to_owned();
            config.genesis_hash = pinned;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if config.genesis_hash.is_empty() {
                bail!(
                    "{} does not exist and {} has no genesis_hash. This node has no pinned network.\n\
                     Run `hashgramctl join-mainnet --genesis-hash <sha256> ...` first.",
                    pin_path.display(),
                    config_path.display()
                );
            }
        }
        Err(e) => return Err(e).with_context(|| format!("reading {}", pin_path.display())),
    }

    // Roles.
    let roles_path = etc.join("roles.json");
    if let Ok(bytes) = std::fs::read(&roles_path) {
        let roles: RolesFile = serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing {}", roles_path.display()))?;
        if config.roles.is_empty() {
            // The chain-only role is not a P2P role; everything else is.
            config.roles = roles
                .roles
                .into_iter()
                .filter(|r| r != "validator")
                .collect();
        }
        if config.reward_address.is_empty() {
            config.reward_address = roles.reward_address;
        }
        if config.storage_quota_bytes == 0 {
            config.storage_quota_bytes = roles.declared_storage_bytes;
        }
    }

    if let Some(home) = home {
        config.data_dir = home.display().to_string();
        if config.peerstore_path == "/var/lib/hashgram/node/peerstore.json" {
            config.peerstore_path = home.join("peerstore.json").display().to_string();
        }
    }

    let identity = config.validate().context("node configuration is invalid")?;
    Ok(Settings { config, identity })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const GENESIS: &str = "9348af00681eecefb8d6329d5ba101c13bc3c8943f2c610295026f6503654287";

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hg-settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn pin(d: &Path, genesis: &str) {
        std::fs::write(
            d.join("network.json"),
            format!(
                r#"{{"network_name":"Hashgram Devnet","network_id":"hashgram-devnet","chain_id":"hashgram-devnet-1","network_magic":"HGD1","protocol_major_version":1,"genesis_hash":"{genesis}"}}"#
            ),
        )
        .unwrap();
    }

    #[test]
    fn the_pin_file_supplies_network_and_genesis() {
        let d = dir("pin");
        pin(&d, GENESIS);
        std::fs::write(d.join("node.toml"), "listen_port = 26670\n").unwrap();
        std::fs::write(
            d.join("roles.json"),
            r#"{"roles":["validator","relay","store"]}"#,
        )
        .unwrap();
        let s = load(&d.join("node.toml"), Some(&d)).unwrap();
        assert_eq!(s.config.network, "devnet");
        assert_eq!(s.config.genesis_hash, GENESIS);
        assert_eq!(s.config.roles, vec!["relay", "store"]);
        assert!(!s.identity.is_mainnet());
    }

    #[test]
    fn a_disagreeing_pin_is_a_startup_failure() {
        let d = dir("disagree");
        pin(&d, GENESIS);
        std::fs::write(
            d.join("node.toml"),
            "genesis_hash = \"eed34034d774c8a2f0b1e5c6d7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d3e4f5a6\"\n",
        )
        .unwrap();
        let err = load(&d.join("node.toml"), None).unwrap_err().to_string();
        assert!(err.contains("disagree"), "{err}");
    }

    #[test]
    fn no_pin_and_no_hash_refuses_to_start() {
        let d = dir("nopin");
        std::fs::write(d.join("node.toml"), "network = \"devnet\"\n").unwrap();
        let err = load(&d.join("node.toml"), None).unwrap_err().to_string();
        assert!(err.contains("join-mainnet"), "{err}");
    }
}
