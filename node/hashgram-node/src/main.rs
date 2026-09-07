//! The Hashgram peer-to-peer node.
//!
//! Runs the libp2p swarm with the Hashgram handshake, and on top of it the
//! services this machine's roles call for: store-and-forward mailboxes, blob
//! storage with replication, the social event log, the safety attestation
//! table and call-node announcements. A local JSON API on loopback serves
//! `hashgramctl`, the indexer, the safety engine and same-host clients.
//!
//! Configuration comes from `/etc/hashgram/node.toml`, the network pin from
//! `/etc/hashgram/network.json` and the roles from `/etc/hashgram/roles.json`.
//! A node with no pinned genesis refuses to start.

#![forbid(unsafe_code)]
// A binary crate: `pub` within it is module organisation, not an API.
#![allow(unreachable_pub)]
#![cfg_attr(
    test,
    allow(
        clippy::panic,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
    )
)]

mod announce;
mod api;
mod app;
mod blob;
mod calls;
mod chain;
mod keys;
mod mailbox;
mod rewards;
mod safety;
mod settings;
mod social;
mod store;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use clap::{Parser, Subcommand};
use prometheus_client::registry::Registry;
use tracing::{error, info, warn};

#[derive(Parser)]
#[command(
    name = "hashgram-node",
    version,
    about = "The Hashgram peer-to-peer node"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the node.
    Run {
        /// Data directory.
        #[arg(
            long,
            env = "HASHGRAM_NODE_HOME",
            default_value = "/var/lib/hashgram/node"
        )]
        home: PathBuf,
        /// Configuration file.
        #[arg(long, default_value = "/etc/hashgram/node.toml")]
        config: PathBuf,
        /// DEVNET ONLY: skip chain lookups for device authorisation. Refused
        /// on mainnet.
        #[arg(long)]
        insecure_no_chain: bool,
    },
    /// Print this node's peer id, creating the key if absent.
    NodeId {
        /// Data directory.
        #[arg(
            long,
            env = "HASHGRAM_NODE_HOME",
            default_value = "/var/lib/hashgram/node"
        )]
        home: PathBuf,
    },
    /// Validate the configuration and exit.
    CheckConfig {
        /// Configuration file.
        #[arg(long, default_value = "/etc/hashgram/node.toml")]
        config: PathBuf,
        /// Data directory.
        #[arg(
            long,
            env = "HASHGRAM_NODE_HOME",
            default_value = "/var/lib/hashgram/node"
        )]
        home: PathBuf,
    },
    /// Generate the provider operator key (hex secp256k1 secret, 0600) and
    /// print its address. Never prints the secret.
    OperatorKey {
        /// Data directory.
        #[arg(
            long,
            env = "HASHGRAM_NODE_HOME",
            default_value = "/var/lib/hashgram/node"
        )]
        home: PathBuf,
        /// Only print the address of an existing key.
        #[arg(long)]
        show: bool,
    },
    /// Sign a bootstrap record file from a list of multiaddrs. The signing
    /// key is read from a file (32-byte hex seed) and never printed.
    SignBootstrap {
        /// Path to a hex ed25519 seed.
        #[arg(long)]
        key_file: PathBuf,
        /// Configuration file, for the network identity.
        #[arg(long, default_value = "/etc/hashgram/node.toml")]
        config: PathBuf,
        /// Multiaddrs with /p2p/.
        #[arg(long = "addr", required = true)]
        addrs: Vec<String>,
        /// Validity in days.
        #[arg(long, default_value_t = 180)]
        days: u64,
        /// Output file.
        #[arg(long)]
        out: PathBuf,
    },
}

fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("info,libp2p_gossipsub=warn,libp2p_kad=warn")
            }),
        )
        .init();

    let cli = Cli::parse();
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("hashgram-node: cannot start the runtime: {e}");
            std::process::exit(1);
        }
    };
    let result = match cli.cmd {
        Cmd::Run {
            home,
            config,
            insecure_no_chain,
        } => rt.block_on(run(home, config, insecure_no_chain)),
        Cmd::NodeId { home } => keys::load_or_create(&home).map(|k| {
            println!("{}", hashgram_p2p::PeerId::from(k.public()));
        }),
        Cmd::CheckConfig { config, home } => settings::load(&config, Some(&home)).map(|s| {
            println!("configuration valid");
            println!("  network      {}", s.identity.network_name);
            println!("  network id   {}", s.identity.network_id);
            println!("  chain id     {}", s.identity.chain_id);
            println!("  genesis hash {}", s.identity.genesis_hash);
            println!(
                "  roles        {}",
                if s.config.roles.is_empty() {
                    "(none)".to_owned()
                } else {
                    s.config.roles.join(",")
                }
            );
            println!(
                "  listen       {}:{} ({:?})",
                s.config.listen_addr, s.config.listen_port, s.config.transport
            );
            println!("  api          {}", s.config.api_addr);
        }),
        Cmd::SignBootstrap {
            key_file,
            config,
            addrs,
            days,
            out,
        } => sign_bootstrap(&key_file, &config, addrs, days, &out),
        Cmd::OperatorKey { home, show } => operator_key(&home, show),
    };
    if let Err(e) = result {
        error!("{e:#}");
        eprintln!("hashgram-node: {e:#}");
        std::process::exit(1);
    }
}

fn sign_bootstrap(
    key_file: &std::path::Path,
    config: &std::path::Path,
    addrs: Vec<String>,
    days: u64,
    out: &std::path::Path,
) -> anyhow::Result<()> {
    use prost::Message;
    let s = settings::load(config, None)?;
    let raw = std::fs::read_to_string(key_file)
        .with_context(|| format!("reading {}", key_file.display()))?;
    let seed: [u8; 32] = hex::decode(raw.trim())
        .context("key file is not hex")?
        .try_into()
        .map_err(|_| anyhow::anyhow!("key file is not a 32-byte seed"))?;
    let signer = hashgram_proto::Ed25519Signer::from_secret(seed);
    let now = store::now();
    let mut rec = hashgram_proto::pb::BootstrapRecord {
        addrs,
        issued_at: now,
        expires_at: now + days * 86_400,
        ..Default::default()
    };
    hashgram_proto::signing::sign_bootstrap_record(&s.identity, &signer, &mut rec)?;
    hashgram_proto::validate::bootstrap_record(&rec, now)?;
    std::fs::write(out, rec.encode_to_vec())
        .with_context(|| format!("writing {}", out.display()))?;
    println!("bootstrap record written to {}", out.display());
    println!("signer public key: {}", hex::encode(signer.public_key()));
    println!(
        "add that key to trusted_bootstrap_signers in node.toml on nodes that should trust it"
    );
    Ok(())
}

fn operator_key_path(home: &std::path::Path, cfg: &hashgram_p2p::NodeConfig) -> PathBuf {
    if cfg.operator_key_file.is_empty() {
        home.join("operator.key")
    } else {
        PathBuf::from(&cfg.operator_key_file)
    }
}

fn load_operator_secret(path: &std::path::Path) -> anyhow::Result<Option<Vec<u8>>> {
    match std::fs::read_to_string(path) {
        Ok(raw) => Ok(Some(
            hex::decode(raw.trim()).context("operator key is not hex")?,
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

fn operator_key(home: &std::path::Path, show: bool) -> anyhow::Result<()> {
    let path = home.join("operator.key");
    let secret = match load_operator_secret(&path)? {
        Some(s) => s,
        None if show => anyhow::bail!("no operator key at {}", path.display()),
        None => {
            let (_, w) = hashgram_chain::Wallet::generate()?;
            let secret = w.secret_bytes().to_vec();
            std::fs::create_dir_all(home)?;
            keys::write_secret(&path, hex::encode(&secret).as_bytes())?;
            eprintln!("operator key written to {} (0600)", path.display());
            secret
        }
    };
    let w = hashgram_chain::Wallet::from_secret(&secret)?;
    println!("{}", w.address());
    Ok(())
}

async fn run(home: PathBuf, config: PathBuf, insecure_no_chain: bool) -> anyhow::Result<()> {
    let s = settings::load(&config, Some(&home))?;
    let mut cfg = s.config;
    let identity = s.identity;

    // The provider operator key, if present. Its address goes into the
    // handshake so clients know whom to sign receipts for.
    let operator_secret = load_operator_secret(&operator_key_path(&home, &cfg))?;
    if let Some(sec) = &operator_secret {
        cfg.operator_address = hashgram_chain::Wallet::from_secret(sec)?
            .address()
            .to_string();
    }

    if insecure_no_chain && identity.is_mainnet() {
        anyhow::bail!("--insecure-no-chain is DEVNET ONLY and is refused on mainnet");
    }

    info!(
        network = %identity.network_name,
        chain_id = %identity.chain_id,
        genesis = %identity.genesis_hash,
        roles = ?cfg.roles,
        "starting hashgram-node {}",
        env!("CARGO_PKG_VERSION")
    );
    if !cfg.metrics_is_loopback() {
        warn!(addr = %cfg.metrics_addr, "metrics endpoint is not on loopback");
    }

    let keypair = keys::load_or_create(&home)?;
    let announce_signer = keys::announce_signer(&keypair)?;
    info!(peer_id = %hashgram_p2p::PeerId::from(keypair.public()), "node identity");

    // Chain cross-check: the co-located hashgramd must be on the same chain.
    let chain = if insecure_no_chain {
        warn!("DEVNET ONLY: device authorisation is not checked against the chain");
        None
    } else {
        let c = chain::ChainClient::new(&cfg.chain_api)?;
        match c.chain_id().await {
            Ok(id) if id == identity.chain_id => info!(chain_id = id, "chain node reachable and on the pinned chain"),
            Ok(id) => anyhow::bail!(
                "the chain node at {} is on chain {id:?} but this node is pinned to {:?}; refusing to start",
                cfg.chain_api,
                identity.chain_id
            ),
            Err(e) => warn!(error = %e, "chain node not reachable yet; device lookups will fail until it is"),
        }
        Some(c)
    };

    let mut registry = Registry::default();
    let peerstore =
        hashgram_p2p::peerstore::Peerstore::load(std::path::Path::new(&cfg.peerstore_path))
            .context("loading the peerstore")?;
    let (handle, events, swarm_task) = hashgram_p2p::start(
        cfg.clone(),
        identity.clone(),
        keypair,
        peerstore,
        &mut registry,
    )?;

    // Services by role.
    let mut services = app::Services::default();
    if cfg.stores() {
        let db = store::open(&home, "mailbox")?;
        services.mailbox = Some(mailbox::MailboxService::open(db, &identity.network_id)?);
        let db = store::open(&home, "blobs")?;
        services.blob = Some(blob::BlobService::open(db, cfg.storage_quota_bytes)?);
        info!(quota = cfg.storage_quota_bytes, "store services enabled");
    }
    {
        let authority: Arc<dyn social::DeviceAuthority> = match &chain {
            Some(c) => Arc::new(social::CachedAuthority::new(
                Arc::new(c.clone()),
                Duration::from_secs(600),
            )),
            None => Arc::new(social::PermissiveAuthority),
        };
        let db = store::open(&home, "social")?;
        services.social = Some(social::SocialService::open(
            db,
            &identity.network_id,
            authority,
            cfg.event_retention_days,
            cfg.max_events,
        )?);
    }
    {
        let db = store::open(&home, "safety")?;
        services.safety = Some(safety::SafetyService::open(db, &cfg.trusted_attestors)?);
    }

    let turn_secret = if cfg.has_role("call") && !cfg.turn_secret_file.is_empty() {
        match calls::load_secret(&cfg.turn_secret_file) {
            Ok(s) => {
                info!("TURN credential issuance enabled");
                Some(s)
            }
            Err(e) => {
                warn!(error = %e, "TURN secret not readable; credentials will not be issued");
                None
            }
        }
    } else {
        None
    };

    let rewards = match (&operator_secret, chain.is_some()) {
        (Some(sec), true) => {
            let db = store::open(&home, "rewards")?;
            let agent = rewards::RewardsAgent::open(
                db,
                cfg.clone(),
                identity.clone(),
                sec,
                announce_signer.clone(),
            )?;
            info!(operator = %agent.operator(), reward = %cfg.reward_address, "useful-service agent enabled");
            Some(agent)
        }
        (Some(_), false) => {
            warn!("operator key present but chain lookups are disabled; useful-service agent off");
            None
        }
        (None, _) => {
            info!("no operator key; this node will not earn useful-service rewards (run `hashgram-node operator-key`)");
            None
        }
    };

    let shared = Arc::new(app::Shared {
        handle: handle.clone(),
        config: cfg.clone(),
        identity: identity.clone(),
        announces: announce::AnnounceTable::default(),
        announce_signer,
        started: std::time::Instant::now(),
        services,
        rewards,
        turn_secret: turn_secret.clone(),
    });

    // Maintenance: sweeps and replication.
    let maintenance = {
        let shared = shared.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(60));
            loop {
                tick.tick().await;
                if let Some(m) = &shared.services.mailbox {
                    if let Err(e) = m.sweep() {
                        warn!(error = %e, "mailbox sweep failed");
                    }
                }
                if let Some(s) = &shared.services.social {
                    if let Err(e) = s.sweep() {
                        warn!(error = %e, "social sweep failed");
                    }
                }
                if let Some(b) = &shared.services.blob {
                    b.repair_pass(&shared, 8).await;
                }
                if let Some(agent) = &shared.rewards {
                    if let Some(b) = &shared.services.blob {
                        agent.answer_challenges(b).await;
                        agent.assignment_pass(&shared, b, 8).await;
                    }
                    agent.submit_receipts().await;
                }
            }
        })
    };
    if let Some(agent) = &shared.rewards {
        let agent = agent.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(10)).await;
            agent.ensure_registered().await;
        });
    }

    let api_state = Arc::new(api::ApiState {
        shared: shared.clone(),
        registry: Arc::new(tokio::sync::Mutex::new(registry)),
        chain,
        turn_secret,
    });
    let api_addr = cfg.api_addr.clone();
    let api_task = tokio::spawn(async move {
        if let Err(e) = api::serve(&api_addr, api_state).await {
            error!(error = %e, "local API stopped");
        }
    });

    let app_task = tokio::spawn(app::run(shared.clone(), events));

    tokio::select! {
        _ = tokio::signal::ctrl_c() => info!("interrupt received"),
        _ = terminate() => info!("termination requested"),
        _ = swarm_task => warn!("swarm task ended"),
        _ = app_task => warn!("application loop ended"),
    }
    maintenance.abort();
    api_task.abort();
    handle.shutdown().await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    info!("hashgram-node stopped");
    Ok(())
}

async fn terminate() {
    #[cfg(unix)]
    {
        if let Ok(mut sig) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            sig.recv().await;
        } else {
            std::future::pending::<()>().await;
        }
    }
    #[cfg(not(unix))]
    std::future::pending::<()>().await;
}
