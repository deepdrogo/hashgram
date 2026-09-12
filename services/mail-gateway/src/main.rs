//! `hashgram-mail-gateway`: the binary.
//!
//! Subcommands are the operator's runbook in code: `init` creates the
//! bridge identity, `register` puts it on chain, `dns` prints the records
//! to publish, `check-config` validates without touching the network,
//! `run` serves. Secrets never appear on the command line: the vault
//! passphrase comes from `HASHGRAM_GATEWAY_PASSPHRASE`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use clap::{Parser, Subcommand};
use hashgram_mail_gateway::bridge::{Bridge, GatewayHandler, Shared};
use hashgram_mail_gateway::config::{GatewayConfig, PASSPHRASE_ENV};
use hashgram_mail_gateway::dns::SystemResolver;
use hashgram_mail_gateway::metrics::Metrics;
use hashgram_mail_gateway::smtp::server::{self, Limits, ServerConfig};
use hashgram_mail_gateway::{dkim, metrics};
use tracing::{error, info, warn};

#[derive(Parser, Debug)]
#[command(
    name = "hashgram-mail-gateway",
    about = "External SMTP gateway for HashMail",
    version
)]
struct Cli {
    /// Path to `gateway.toml`.
    #[arg(
        short,
        long,
        default_value = "gateway.toml",
        env = "HASHGRAM_GATEWAY_CONFIG"
    )]
    config: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Serve: SMTP listener, health/metrics, and the bridge worker.
    Run,
    /// Create the bridge identity's vault under `[hashgram] home`. Prints
    /// the address and, once, the recovery mnemonic.
    Init {
        /// Device label recorded in the vault.
        #[arg(long, default_value = "mail-gateway")]
        device: String,
    },
    /// Register the bridge identity on chain (the wallet must be funded).
    Register {
        /// Device label shown on chain.
        #[arg(long, default_value = "mail-gateway")]
        label: String,
    },
    /// Validate the configuration and print a summary (no network).
    CheckConfig,
    /// Print the DNS records to publish: MX, SPF, DKIM, DMARC.
    Dns {
        /// Public hostname of the machine receiving port 25 (the MX target).
        #[arg(long)]
        mx_host: Option<String>,
        /// Public IPv4 of the sending host for SPF (`ip4:` mechanism).
        #[arg(long)]
        ip4: Vec<String>,
        /// Public IPv6 of the sending host for SPF (`ip6:` mechanism).
        #[arg(long)]
        ip6: Vec<String>,
    },
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new("info,hickory_resolver=warn,hickory_proto=warn")
    });
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    let cli = Cli::parse();
    let cfg = GatewayConfig::load(&cli.config)
        .with_context(|| format!("loading {}", cli.config.display()))?;
    match cli.cmd {
        Cmd::CheckConfig => check_config(&cfg),
        Cmd::Dns { mx_host, ip4, ip6 } => dns(&cfg, mx_host.as_deref(), &ip4, &ip6),
        Cmd::Init { device } => init(&cfg, &device),
        Cmd::Register { label } => register(&cfg, &label).await,
        Cmd::Run => run(cfg).await,
    }
}

fn check_config(cfg: &GatewayConfig) -> anyhow::Result<()> {
    println!("config OK");
    println!("  domain:            {}", cfg.domain.name);
    println!(
        "  smtp listen:       {} (hostname {})",
        cfg.smtp.listen,
        cfg.smtp_hostname()
    );
    println!("  max message:       {} bytes", cfg.smtp.max_message_bytes);
    println!("  network:           {}", cfg.hashgram.network);
    println!("  home:              {}", cfg.hashgram.home.display());
    println!("  store:             {}", cfg.store.sqlite_path.display());
    println!(
        "  http:              {}",
        if cfg.http.listen.is_empty() {
            "disabled"
        } else {
            &cfg.http.listen
        }
    );
    match (&cfg.domain.dkim_selector, &cfg.domain.dkim_private_key_file) {
        (Some(s), Some(p)) => {
            let text =
                std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?;
            let key = dkim::SigningKey::parse(&text)?;
            println!("  dkim:              {} selector {s}", key.algorithm());
        }
        _ => println!(
            "  dkim:              DISABLED (outbound mail will be rejected by most receivers)"
        ),
    }
    println!(
        "  policy:            spf/dkim required={} spam>{} contacts-only={}",
        cfg.policy.require_spf_or_dkim,
        cfg.policy.reject_spam_score_over,
        cfg.policy.allow_outbound_from_contacts_only
    );
    println!(
        "  outbound:          port {} require_tls={} smarthost={:?}",
        cfg.outbound.smtp_port, cfg.outbound.require_tls, cfg.outbound.smarthost
    );
    let vault = hashgram_sdk::Paths::new(&cfg.hashgram.home).vault();
    println!("  vault present:     {}", vault.exists());
    println!(
        "  passphrase env:    {}",
        if std::env::var(PASSPHRASE_ENV).is_ok() {
            "set"
        } else {
            "NOT SET"
        }
    );
    Ok(())
}

fn dns(
    cfg: &GatewayConfig,
    mx_host: Option<&str>,
    ip4: &[String],
    ip6: &[String],
) -> anyhow::Result<()> {
    let d = &cfg.domain.name;
    let mx = mx_host.unwrap_or(cfg.smtp_hostname());
    println!("; Records for {d} — see docs/MAIL_GATEWAY.md § DNS");
    println!("{d}.\t\t3600\tIN\tMX\t10 {mx}.");
    let mut spf = String::from("v=spf1");
    for ip in ip4 {
        spf.push_str(&format!(" ip4:{ip}"));
    }
    for ip in ip6 {
        spf.push_str(&format!(" ip6:{ip}"));
    }
    if ip4.is_empty() && ip6.is_empty() {
        spf.push_str(" mx");
    }
    spf.push_str(" -all");
    println!("{d}.\t\t3600\tIN\tTXT\t\"{spf}\"");
    match (&cfg.domain.dkim_selector, &cfg.domain.dkim_private_key_file) {
        (Some(s), Some(p)) => {
            let text =
                std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?;
            let key = dkim::SigningKey::parse(&text)?;
            let record = key.dns_record()?;
            // TXT strings are limited to 255 bytes each; split the value.
            let chunks: Vec<String> = record
                .as_bytes()
                .chunks(250)
                .map(|c| format!("\"{}\"", String::from_utf8_lossy(c)))
                .collect();
            println!("{s}._domainkey.{d}.\t3600\tIN\tTXT\t{}", chunks.join(" "));
        }
        _ => println!(
            "; DKIM: configure domain.dkim_selector and domain.dkim_private_key_file first"
        ),
    }
    println!("_dmarc.{d}.\t3600\tIN\tTXT\t\"v=DMARC1; p=quarantine; rua=mailto:postmaster@{d}; adkim=s; aspf=s\"");
    Ok(())
}

fn init(cfg: &GatewayConfig, device: &str) -> anyhow::Result<()> {
    let paths = hashgram_sdk::Paths::new(&cfg.hashgram.home);
    if paths.vault().exists() {
        anyhow::bail!(
            "{} already exists; refusing to overwrite the bridge identity",
            paths.vault().display()
        );
    }
    std::fs::create_dir_all(&cfg.hashgram.home)?;
    let passphrase = GatewayConfig::passphrase()?;
    let kdf = cfg.sdk_config()?.kdf;
    let (account, mnemonic) =
        hashgram_sdk::account::Account::create(&paths.vault(), &passphrase, device, kdf)?;
    account.save()?;
    println!("bridge identity created");
    println!("  address:  {}", account.address());
    println!("  vault:    {}", paths.vault().display());
    println!();
    println!("RECOVERY MNEMONIC (shown once; store offline, it IS the identity):");
    println!("  {mnemonic}");
    println!();
    println!(
        "next: fund the address, then `hashgram-mail-gateway register`, then claim a username"
    );
    println!(
        "      (e.g. `gateway`) for it with hashgram-client so users can add it as a contact."
    );
    Ok(())
}

async fn register(cfg: &GatewayConfig, label: &str) -> anyhow::Result<()> {
    let passphrase = GatewayConfig::passphrase()?;
    let one = hashgram_sdk::HashgramOne::open(cfg.sdk_config()?, &passphrase).await?;
    if one.link.peers().await.is_empty() {
        anyhow::bail!(
            "no node completed the Hashgram handshake; check [hashgram] bootstrap / chain_api"
        );
    }
    let r = hashgram_sdk::account::create_identity_on_chain(
        &one.account,
        &one.network,
        &one.chain,
        label,
        "server",
    )
    .await?;
    println!("identity registered: {r:?}");
    Ok(())
}

async fn run(cfg: GatewayConfig) -> anyhow::Result<()> {
    let passphrase = GatewayConfig::passphrase()?;
    let metrics = Arc::new(Metrics::new());
    let shared = Shared::open(cfg.clone(), &passphrase, metrics.clone()).await?;
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);

    // Health/metrics.
    let mut tasks = tokio::task::JoinSet::new();
    if !cfg.http.listen.is_empty() {
        let listen = cfg.http.listen.parse()?;
        let m = metrics.clone();
        let mut rx = stop_rx.clone();
        tasks.spawn(async move {
            let shutdown = async move {
                let _ = rx.wait_for(|s| *s).await;
            };
            if let Err(e) = metrics::serve(listen, m, shutdown).await {
                error!(error = %e, "http server failed");
            }
        });
    }

    // SMTP.
    let server_cfg = ServerConfig {
        listen: cfg.smtp_listen()?,
        hostname: cfg.smtp_hostname().to_owned(),
        limits: Limits {
            max_message_bytes: cfg.smtp.max_message_bytes,
            max_recipients: cfg.smtp.max_recipients,
        },
        idle_timeout: Duration::from_secs(cfg.smtp.idle_timeout_secs),
        max_connections: cfg.smtp.max_connections,
        connections_per_ip_per_minute: cfg.smtp.connections_per_ip_per_minute,
    };
    let handler = Arc::new(GatewayHandler {
        shared: shared.clone(),
    });
    {
        let mut rx = stop_rx.clone();
        tasks.spawn(async move {
            let shutdown = async move {
                let _ = rx.wait_for(|s| *s).await;
            };
            if let Err(e) = server::serve(server_cfg, handler, shutdown).await {
                error!(error = %e, "smtp server failed");
            }
        });
    }

    // Bridge worker.
    {
        let resolver = SystemResolver::new()?;
        let bridge = Bridge::new(shared.clone(), resolver);
        let mut rx = stop_rx.clone();
        tasks.spawn(async move {
            let shutdown = async move {
                let _ = rx.wait_for(|s| *s).await;
            };
            bridge.run(shutdown).await;
        });
    }

    info!(domain = %cfg.domain.name, "gateway running");
    wait_for_signal().await;
    warn!("shutdown requested");
    let _ = stop_tx.send(true);
    let _ = tokio::time::timeout(Duration::from_secs(20), async {
        while tasks.join_next().await.is_some() {}
    })
    .await;
    info!("bye");
    Ok(())
}

async fn wait_for_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(s) => s,
                Err(_) => {
                    let _ = ctrl_c.await;
                    return;
                }
            };
        tokio::select! {
            _ = ctrl_c => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = ctrl_c.await;
    }
}
