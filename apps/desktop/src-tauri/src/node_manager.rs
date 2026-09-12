//! Earn → Run a node on this PC.
//!
//! The app ships `hashgram-node.exe` as a sidecar and manages it: writes
//! its configuration under `%LOCALAPPDATA%\Hashgram\data\node`, keeps the
//! node's operator key in the vault (and in the node's key file, which the
//! node needs to sign receipts — the file is the working copy), registers a
//! background task so the node runs at logon, and reads its local API for
//! live status. The node reaches the chain through the app's loopback
//! gateway (`chain_proxy`), so a home PC needs no `hashgramd`.
//!
//! Elevation: a real Windows service needs administrator rights, which a
//! per-user install does not have. The manager registers a per-user
//! Scheduled Task ("at logon, hidden, restart on failure") — no UAC — and
//! upgrades to a Windows service when it is running elevated (`sc create`).
//! Both are "a service managed by the app" from the user's point of view;
//! the UI says which one is in effect.

use std::path::PathBuf;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::paths;

/// Task / service name.
pub const SERVICE_NAME: &str = "HashgramNode";
/// The node's local API.
pub const NODE_API: &str = "http://127.0.0.1:26672";
/// The node's P2P port.
pub const NODE_PORT: u16 = 26670;

/// What the user chose.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NodeSetup {
    /// Roles among store, media, relay.
    pub roles: Vec<String>,
    /// Disk quota in GiB.
    pub storage_gib: u32,
    /// Bandwidth cap in Mbit/s (0 = unlimited; advisory, announced).
    pub bandwidth_mbps: u32,
    /// Reward address (a separate cold address by default).
    pub reward_address: String,
    /// Moniker shown to peers.
    pub moniker: String,
    /// Register as a provider automatically once the bond is funded.
    pub auto_register: bool,
}

impl Default for NodeSetup {
    fn default() -> Self {
        Self {
            roles: vec!["store".into(), "media".into(), "relay".into()],
            storage_gib: 20,
            bandwidth_mbps: 0,
            reward_address: String::new(),
            moniker: std::env::var("COMPUTERNAME")
                .unwrap_or_else(|_| "home-pc".into())
                .to_ascii_lowercase(),
            auto_register: true,
        }
    }
}

/// Where the node lives.
#[must_use]
pub fn node_home() -> PathBuf {
    paths::data_dir().join("node")
}

/// The node's config file.
#[must_use]
pub fn config_path() -> PathBuf {
    node_home().join("node.toml")
}

/// Setup as last written.
#[must_use]
pub fn setup_path() -> PathBuf {
    node_home().join("setup.json")
}

/// Finds the bundled `hashgram-node.exe`: next to the app (installed), in
/// the Tauri sidecar layout, or in the workspace target dir (development).
#[must_use]
pub fn node_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.to_path_buf();
    let candidates = [
        dir.join("hashgram-node.exe"),
        dir.join("hashgram-node-x86_64-pc-windows-msvc.exe"),
        dir.join("binaries")
            .join("hashgram-node-x86_64-pc-windows-msvc.exe"),
        dir.join("hashgram-node"),
    ];
    let real = |p: &PathBuf| std::fs::metadata(p).map(|m| m.len() > 0).unwrap_or(false);
    for c in candidates {
        if real(&c) {
            return Some(c);
        }
    }
    // Development: the workspace target directory.
    let mut d = dir.clone();
    for _ in 0..4 {
        for prof in ["release", "debug"] {
            let p = d.join("target").join(prof).join("hashgram-node.exe");
            if p.exists() {
                return Some(p);
            }
            let p = d.join(prof).join("hashgram-node.exe");
            if p.exists() {
                return Some(p);
            }
        }
        d = d.parent()?.to_path_buf();
    }
    None
}

/// Writes node.toml, network.json (mainnet pin) and roles.json.
pub fn write_config(
    setup: &NodeSetup,
    network: &hashgram_sdk::NetworkIdentity,
    operator_secret_hex: &str,
) -> Result<(), String> {
    write_config_at(&node_home(), setup, network, operator_secret_hex)
}

/// Writes the node files under an explicit directory.
pub fn write_config_at(
    home: &std::path::Path,
    setup: &NodeSetup,
    network: &hashgram_sdk::NetworkIdentity,
    operator_secret_hex: &str,
) -> Result<(), String> {
    let home = home.to_path_buf();
    std::fs::create_dir_all(&home).map_err(|e| e.to_string())?;
    let roles: Vec<String> = setup
        .roles
        .iter()
        .filter(|r| ["store", "media", "relay"].contains(&r.as_str()))
        .cloned()
        .collect();
    if roles.is_empty() {
        return Err("choose at least one role".into());
    }
    if !setup.reward_address.trim().is_empty() {
        crate::tx::validate_address(&setup.reward_address, crate::tx::ADDRESS_PREFIX)?;
    }
    let quota: u64 = u64::from(setup.storage_gib.max(1)) * 1024 * 1024 * 1024;
    let home_s = home.display().to_string().replace('\\', "/");
    let toml = format!(
        r#"# Written by Hashgram for Windows (Earn -> Run a node). Edit in the app.
network = "{network}"
genesis_hash = "{genesis}"
roles = [{roles}]
moniker = "{moniker}"
listen_addr = "0.0.0.0"
listen_port = {port}
api_addr = "127.0.0.1:26672"
metrics_addr = "127.0.0.1:26671"
chain_api = "{chain_api}"
peerstore_path = "{home}/peerstore.json"
data_dir = "{home}"
storage_quota_bytes = {quota}
reward_address = "{reward}"
auto_register_provider = {auto}
operator_key_file = "{home}/operator.key"
"#,
        network = if network.is_mainnet() {
            "mainnet"
        } else {
            "devnet"
        },
        genesis = network.genesis_hash,
        roles = roles
            .iter()
            .map(|r| format!("\"{r}\""))
            .collect::<Vec<_>>()
            .join(", "),
        moniker = setup.moniker.replace('"', ""),
        port = NODE_PORT,
        chain_api = crate::chain_proxy::url(),
        home = home_s,
        quota = quota,
        reward = setup.reward_address.trim(),
        auto = setup.auto_register,
    );
    std::fs::write(home.join("node.toml"), toml).map_err(|e| e.to_string())?;
    // The operator key file the node reads (0600-equivalent: the user's
    // profile directory). The vault keeps the authoritative copy.
    std::fs::write(home.join("operator.key"), operator_secret_hex.trim())
        .map_err(|e| e.to_string())?;
    std::fs::write(
        home.join("setup.json"),
        serde_json::to_vec_pretty(setup).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Reads the saved setup.
#[must_use]
pub fn read_setup() -> Option<NodeSetup> {
    std::fs::read(setup_path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
}

/// How the node is registered to run.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Registration {
    /// Not registered.
    None,
    /// Per-user Scheduled Task at logon.
    ScheduledTask,
    /// Windows service (needed elevation).
    Service,
}

fn run(cmd: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(cmd)
        .args(args)
        .output()
        .map_err(|e| format!("{cmd}: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if out.status.success() {
        Ok(stdout)
    } else {
        Err(format!(
            "{cmd} {}: {}{}",
            args.join(" "),
            stdout.trim(),
            stderr.trim()
        ))
    }
}

/// Which registration exists.
#[must_use]
pub fn registration() -> Registration {
    if run("sc.exe", &["query", SERVICE_NAME]).is_ok() {
        return Registration::Service;
    }
    if run("schtasks.exe", &["/Query", "/TN", SERVICE_NAME]).is_ok() {
        return Registration::ScheduledTask;
    }
    Registration::None
}

/// Whether the current process is elevated (a service can be created).
#[must_use]
pub fn is_elevated() -> bool {
    // `net session` succeeds only when elevated. Cheap and dependency-free.
    run("net.exe", &["session"]).is_ok()
}

/// Finds the service wrapper (`hashgram-node-service.exe`) next to the node
/// binary or in the workspace target dir.
#[must_use]
pub fn wrapper_binary() -> Option<PathBuf> {
    let node = node_binary()?;
    let dir = node.parent()?;
    for name in [
        "hashgram-node-service.exe",
        "hashgram-node-service-x86_64-pc-windows-msvc.exe",
    ] {
        let p = dir.join(name);
        if std::fs::metadata(&p).map(|m| m.len() > 0).unwrap_or(false) {
            return Some(p);
        }
    }
    None
}

/// Registers the node to run at logon (task) or as a service (elevated).
/// Both go through the wrapper, which supervises the node, restarts it
/// with back-off and writes `node.log`.
pub fn install() -> Result<Registration, String> {
    let bin = node_binary()
        .ok_or_else(|| "hashgram-node.exe is not bundled with this build".to_owned())?;
    let wrapper = wrapper_binary()
        .ok_or_else(|| "hashgram-node-service.exe is not bundled with this build".to_owned())?;
    let home = node_home();
    let cfg = config_path();
    if !cfg.exists() {
        return Err("write the node configuration first".into());
    }
    let common = format!(
        "--node \"{}\" --home \"{}\" --config \"{}\"",
        bin.display(),
        home.display(),
        cfg.display()
    );
    if is_elevated() {
        let cmdline = format!("\"{}\" --service {common}", wrapper.display());
        let _ = run("sc.exe", &["stop", SERVICE_NAME]);
        let _ = run("sc.exe", &["delete", SERVICE_NAME]);
        run(
            "sc.exe",
            &[
                "create",
                SERVICE_NAME,
                "binPath=",
                &cmdline,
                "start=",
                "auto",
                "DisplayName=",
                "Hashgram Node",
            ],
        )?;
        let _ = run(
            "sc.exe",
            &[
                "description",
                SERVICE_NAME,
                "Hashgram peer-to-peer node managed by Hashgram for Windows",
            ],
        );
        let _ = run(
            "sc.exe",
            &[
                "failure",
                SERVICE_NAME,
                "reset=",
                "86400",
                "actions=",
                "restart/5000/restart/30000/restart/60000",
            ],
        );
        return Ok(Registration::Service);
    }
    let _ = run("schtasks.exe", &["/Delete", "/TN", SERVICE_NAME, "/F"]);
    // A logon task for this user running the wrapper in foreground mode:
    // the wrapper supervises and restarts the node. Runs whether or not the
    // app is open.
    let cmdline = format!("\"{}\" {common}", wrapper.display());
    run(
        "schtasks.exe",
        &[
            "/Create",
            "/TN",
            SERVICE_NAME,
            "/SC",
            "ONLOGON",
            "/RL",
            "LIMITED",
            "/F",
            "/TR",
            &cmdline,
        ],
    )?;
    Ok(Registration::ScheduledTask)
}

/// Starts the node now.
pub fn start() -> Result<(), String> {
    match registration() {
        Registration::Service => run("sc.exe", &["start", SERVICE_NAME]).map(|_| ()),
        Registration::ScheduledTask => {
            run("schtasks.exe", &["/Run", "/TN", SERVICE_NAME]).map(|_| ())
        }
        Registration::None => Err("the node is not installed".into()),
    }
}

/// Stops the node.
pub fn stop() -> Result<(), String> {
    match registration() {
        Registration::Service => run("sc.exe", &["stop", SERVICE_NAME]).map(|_| ()),
        Registration::ScheduledTask => {
            let _ = run("schtasks.exe", &["/End", "/TN", SERVICE_NAME]);
            let _ = run("taskkill.exe", &["/IM", "hashgram-node-service.exe", "/F"]);
            let _ = run("taskkill.exe", &["/IM", "hashgram-node.exe", "/F"]);
            Ok(())
        }
        Registration::None => Ok(()),
    }
}

/// Removes the registration (keeps data).
pub fn uninstall() -> Result<(), String> {
    let _ = stop();
    match registration() {
        Registration::Service => run("sc.exe", &["delete", SERVICE_NAME]).map(|_| ()),
        Registration::ScheduledTask => {
            run("schtasks.exe", &["/Delete", "/TN", SERVICE_NAME, "/F"]).map(|_| ())
        }
        Registration::None => Ok(()),
    }
}

/// Whether a node process is running (its local API answers).
pub async fn api_get(path: &str) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .map_err(|e| e.to_string())?;
    let r = client
        .get(format!("{NODE_API}/{}", path.trim_start_matches('/')))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("node API {}", r.status()));
    }
    r.json().await.map_err(|e| e.to_string())
}

/// Reads the node's peer id from its key (`hashgram-node node-id`).
pub fn node_id() -> Result<String, String> {
    let bin = node_binary().ok_or_else(|| "hashgram-node.exe is not bundled".to_owned())?;
    let out = run(
        &bin.display().to_string(),
        &["node-id", "--home", &node_home().display().to_string()],
    )?;
    Ok(out.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_is_written_with_the_loopback_gateway_and_pinned_genesis() {
        let d = std::env::temp_dir().join(format!("hg-node-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let setup = NodeSetup {
            roles: vec!["store".into(), "bogus".into()],
            reward_address: "hash13t8v5nnghrvgcuuqcrt9k5wyhtqwq7fl3ynjpy".into(),
            ..NodeSetup::default()
        };
        let net = hashgram_sdk::NetworkIdentity::mainnet(hashgram_sdk::net::MAINNET_GENESIS_HASH);
        write_config_at(&d, &setup, &net, "aa".repeat(32).as_str()).unwrap();
        let toml = std::fs::read_to_string(d.join("node.toml")).unwrap();
        assert!(
            toml.contains("roles = [\"store\"]"),
            "unknown roles dropped: {toml}"
        );
        assert!(toml.contains(&format!("chain_api = \"{}\"", crate::chain_proxy::url())));
        assert!(toml.contains(hashgram_sdk::net::MAINNET_GENESIS_HASH));
        assert!(toml.contains("reward_address = \"hash13t8v5"));
        assert!(
            !toml.contains("/ip4/") && !toml.contains("bootstrap_peers"),
            "no hardcoded server: discovery is the compiled-in list plus the peerstore"
        );
        assert_eq!(
            std::fs::read_to_string(d.join("operator.key")).unwrap(),
            "aa".repeat(32)
        );
        let saved: NodeSetup =
            serde_json::from_slice(&std::fs::read(d.join("setup.json")).unwrap()).unwrap();
        assert_eq!(saved.roles, vec!["store".to_owned(), "bogus".to_owned()]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_bad_reward_address_is_refused() {
        let setup = NodeSetup {
            reward_address: "cosmos1qypqxpq9qcrsszg2pvxq6rs0zqg3yyc5lzv7xu".into(),
            ..NodeSetup::default()
        };
        let net = hashgram_sdk::NetworkIdentity::mainnet(hashgram_sdk::net::MAINNET_GENESIS_HASH);
        assert!(write_config_at(
            &std::env::temp_dir().join("hg-node-bad"),
            &setup,
            &net,
            "00"
        )
        .is_err());
    }
}
