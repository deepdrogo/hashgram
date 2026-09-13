//! `hashgram-client`: the developer client for everything off-chain.
//!
//! Proves the protocol works before any GUI exists: identities and devices,
//! the wallet, E2EE messaging and groups, social events including reels,
//! and media. Every command goes through `hashgram-sdk`, so what this binary
//! can do is exactly what a native application can do.
//!
//! Configuration is a small TOML file (`--profile`), or flags. The vault
//! passphrase comes from `HASHGRAM_PASSPHRASE` or `--passphrase-file`; it is
//! never taken as a flag, because flags end up in shell history.

#![forbid(unsafe_code)]
#![allow(unreachable_pub)]
// A developer CLI formatting its own values: integer division for HASH
// units is exact by construction, and indexing JSON this file just built
// cannot fail. The node and SDK keep the strict lints.
#![allow(clippy::integer_division, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context};
use clap::{Parser, Subcommand};
use hashgram_sdk::account::{self, Account};

mod one;
use hashgram_sdk::link::Link;
use hashgram_sdk::messaging::Messaging;
use hashgram_sdk::social::{payload_json, Social};
use hashgram_sdk::{blob, pb, ChainClient, KdfCost, Multiaddr, NetworkIdentity};
use serde::{Deserialize, Serialize};

#[derive(Parser)]
#[command(name = "hashgram-client", version, about = "Hashgram developer client")]
struct Cli {
    /// Profile directory holding keystore.json and profile.toml.
    #[arg(
        long,
        env = "HASHGRAM_CLIENT_HOME",
        default_value = "~/.hashgram-client"
    )]
    home: String,
    /// Read the vault passphrase from this file instead of HASHGRAM_PASSPHRASE.
    #[arg(long)]
    passphrase_file: Option<PathBuf>,
    /// DEVNET ONLY: light key derivation for fast tests.
    #[arg(long, env = "HASHGRAM_LIGHT_KDF")]
    light_kdf: bool,
    /// Emit JSON instead of text.
    #[arg(long)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct Profile {
    /// `mainnet` or `devnet`.
    network: String,
    /// Pinned genesis hash.
    genesis_hash: String,
    /// REST gateway.
    chain_api: String,
    /// Bootstrap multiaddrs with /p2p/.
    bootstrap: Vec<String>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Write profile.toml for this client.
    Configure {
        #[arg(long)]
        network: String,
        #[arg(long)]
        genesis_hash: String,
        /// REST gateway for chain reads and broadcast. Leave empty (the
        /// default) to read the chain through the P2P relay of the nodes
        /// this client connects to, cross-checked across two operators.
        #[arg(long, default_value = "")]
        chain_api: String,
        #[arg(long = "bootstrap")]
        bootstrap: Vec<String>,
    },
    /// Identity: account, root and devices.
    Identity {
        #[command(subcommand)]
        cmd: IdentityCmd,
    },
    /// Wallet: balance, send, history.
    Wallet {
        #[command(subcommand)]
        cmd: WalletCmd,
    },
    /// Private messaging.
    Message {
        #[command(subcommand)]
        cmd: MessageCmd,
    },
    /// Groups.
    Group {
        #[command(subcommand)]
        cmd: GroupCmd,
    },
    /// Profile events.
    Profile {
        #[command(subcommand)]
        cmd: ProfileCmd,
    },
    /// Follow an address.
    Follow { target: String },
    /// Unfollow an address.
    Unfollow { target: String },
    /// Publish a post.
    Post {
        text: String,
        #[arg(long = "tag")]
        hashtags: Vec<String>,
        #[arg(long)]
        channel: Option<String>,
        /// Attach an already-uploaded public blob (hex cid), mime, size.
        #[arg(long)]
        media_cid: Option<String>,
        #[arg(long, default_value = "image/jpeg")]
        media_mime: String,
        #[arg(long, default_value_t = 0)]
        media_size: u64,
    },
    /// Comment on a post (hex id).
    Comment { post: String, text: String },
    /// React to an event (hex id).
    React { target: String, reaction: String },
    /// Repost.
    Repost { post: String },
    /// Reels.
    Reel {
        #[command(subcommand)]
        cmd: ReelCmd,
    },
    /// Channels.
    Channel {
        #[command(subcommand)]
        cmd: ChannelCmd,
    },
    /// Fetch an author's events.
    Feed {
        author: String,
        #[arg(long, default_value_t = 0)]
        from: u64,
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// Media blobs.
    Blob {
        #[command(subcommand)]
        cmd: BlobCmd,
    },
    /// Network: peers and announcements.
    Net {
        #[command(subcommand)]
        cmd: NetCmd,
    },
    /// Calls: discovery, TURN credentials, signalling.
    Call {
        #[command(subcommand)]
        cmd: CallCmd,
    },
    /// Hashgram One: Mail, Drive, People, Feed, Circles, Spaces, Earn, Network.
    One {
        #[command(subcommand)]
        cmd: one::OneCmd,
    },
}

#[derive(Subcommand)]
enum CallCmd {
    /// List call nodes (TURN/SFU) announced on the network.
    Discover,
    /// Obtain time-limited TURN credentials from a call node.
    Turn,
    /// Send a call signal into a conversation (hex group id).
    Signal {
        group: String,
        /// offer|answer|ice|hangup|ring|busy
        kind: String,
        #[arg(long, default_value = "")]
        sdp: String,
        #[arg(long, default_value = "")]
        candidate: String,
        #[arg(long)]
        call_id: Option<String>,
        #[arg(long)]
        video: bool,
    },
}

#[derive(Subcommand)]
enum IdentityCmd {
    /// Create a new account, root and device, and register on chain.
    Create {
        #[arg(long, default_value = "device-1")]
        device_id: String,
        #[arg(long, default_value = "hashgram-client")]
        label: String,
        #[arg(long, default_value = "linux")]
        platform: String,
        /// Skip the on-chain registration (offline vault creation only).
        #[arg(long)]
        offline: bool,
    },
    /// Import an account from a mnemonic (fresh root and device), then register.
    Import {
        #[arg(long)]
        mnemonic_file: PathBuf,
        #[arg(long, default_value = "device-1")]
        device_id: String,
        #[arg(long)]
        offline: bool,
    },
    /// DEVNET ONLY: import a raw secp256k1 hex secret (from `hashgramd keys export --unarmored-hex --unsafe`).
    ImportSecret {
        #[arg(long)]
        secret_file: PathBuf,
        #[arg(long, default_value = "device-1")]
        device_id: String,
        #[arg(long)]
        offline: bool,
    },
    /// Register the identity in an existing vault on chain (after funding).
    Register {
        #[arg(long, default_value = "hashgram-client")]
        label: String,
        #[arg(long, default_value = "linux")]
        platform: String,
    },
    /// Show this device's identity.
    Show,
    /// Create a device-only vault for an address whose root is elsewhere.
    /// Prints the device public key to hand to the root holder.
    NewDevice {
        #[arg(long)]
        address: String,
        #[arg(long)]
        device_id: String,
    },
    /// Root holder: authorise a device by its hex public key.
    AddDevice {
        #[arg(long)]
        device_id: String,
        #[arg(long)]
        pubkey: String,
        #[arg(long, default_value = "")]
        label: String,
        #[arg(long, default_value = "")]
        platform: String,
    },
    /// Root holder: revoke a device.
    RemoveDevice {
        #[arg(long)]
        device_id: String,
    },
    /// List devices on chain for an address (default: own).
    Devices { address: Option<String> },
    /// Publish this device's MLS key package to store nodes.
    PublishKeys,
}

#[derive(Subcommand)]
enum WalletCmd {
    /// Show address and balance.
    Balance { address: Option<String> },
    /// Send HASH (amount in HASH, decimals allowed).
    Send { to: String, amount: String },
    /// Recent transactions involving this address.
    History {
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// Register a @username.
    RegisterName { name: String },
}

#[derive(Subcommand)]
enum MessageCmd {
    /// Start a direct conversation with an address (or reuse the existing one) and send text.
    Send { to: String, text: String },
    /// Fetch and decrypt everything waiting for this device.
    Receive {
        /// Keep polling every N seconds.
        #[arg(long)]
        watch: Option<u64>,
    },
    /// List conversations.
    List,
    /// Send text to a group by hex id.
    SendGroup { group: String, text: String },
}

#[derive(Subcommand)]
enum GroupCmd {
    /// Create a named group with participants.
    Create {
        name: String,
        #[arg(required = true)]
        members: Vec<String>,
    },
    /// Add a participant.
    Add { group: String, address: String },
    /// Remove a participant.
    Remove { group: String, address: String },
    /// Show members.
    Members { group: String },
}

#[derive(Subcommand)]
enum ProfileCmd {
    /// Publish a profile update.
    Create {
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "")]
        bio: String,
        #[arg(long)]
        avatar_cid: Option<String>,
    },
}

#[derive(Subcommand)]
enum ReelCmd {
    /// Publish a reel from a video file: uploads the video, then the event.
    Publish {
        video: PathBuf,
        #[arg(long, default_value = "")]
        caption: String,
        #[arg(long = "tag")]
        hashtags: Vec<String>,
        #[arg(long, default_value = "video/mp4")]
        mime: String,
        #[arg(long, default_value_t = 0)]
        duration_ms: u32,
    },
    /// Publish a test reel with a generated video blob.
    PublishTest,
}

#[derive(Subcommand)]
enum ChannelCmd {
    /// Create a channel.
    Create {
        name: String,
        #[arg(long, default_value = "")]
        description: String,
    },
}

#[derive(Subcommand)]
enum BlobCmd {
    /// Upload a file. --private encrypts first and prints the key.
    Upload {
        file: PathBuf,
        #[arg(long, default_value = "application/octet-stream")]
        mime: String,
        #[arg(long)]
        private: bool,
        #[arg(long, default_value_t = 2)]
        replicas: usize,
    },
    /// Download by hex cid. With --key and --nonce, decrypt.
    Download {
        cid: String,
        out: PathBuf,
        #[arg(long)]
        key: Option<String>,
        #[arg(long)]
        nonce: Option<String>,
    },
    /// List providers of a cid.
    Providers { cid: String },
    /// Verify a local file against a cid.
    Verify {
        file: PathBuf,
        cid: String,
        #[arg(long, default_value = "application/octet-stream")]
        mime: String,
        #[arg(long)]
        encrypted: bool,
    },
}

#[derive(Subcommand)]
enum NetCmd {
    /// Verified peers and their roles.
    Peers,
    /// Node announcements, optionally by role.
    Announcements {
        #[arg(long)]
        role: Option<String>,
    },
    /// TURN/SFU call infrastructure announced on the network.
    Calls,
}

fn expand_home(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Ok(h) = std::env::var("HOME") {
            return Path::new(&h).join(rest);
        }
    }
    PathBuf::from(p)
}

struct Ctx {
    home: PathBuf,
    profile: Profile,
    network: NetworkIdentity,
    passphrase: String,
    kdf: KdfCost,
    json: bool,
    /// One swarm per process, shared by the chain relay and every other
    /// network operation.
    link: tokio::sync::OnceCell<Arc<Link>>,
}

impl Ctx {
    fn vault_path(&self) -> PathBuf {
        self.home.join("keystore.json")
    }

    fn account(&self) -> anyhow::Result<Account> {
        Account::open(&self.vault_path(), &self.passphrase, self.kdf)
            .context("opening the keystore")
    }

    /// The chain client: HTTP when `chain_api` is set, otherwise the P2P
    /// relay through the connected nodes with cross-checking (Stage 0 of
    /// the desktop build: a wallet with no server address at all).
    async fn chain(&self) -> anyhow::Result<ChainClient> {
        if self.profile.chain_api.trim().is_empty() {
            let link = self.link().await?;
            if link.chain_relays().await.is_empty() {
                bail!("no connected node relays chain queries yet (needs a node with the relay or bootstrap role running the chain relay); set --chain-api to use a REST gateway instead");
            }
            return Ok(hashgram_sdk::chain_client_over_link(
                link,
                &self.network.chain_id,
            ));
        }
        Ok(ChainClient::new(
            &self.profile.chain_api,
            &self.network.chain_id,
        )?)
    }

    /// Prints how the last chain read was verified (P2P relay only).
    fn note_verification(&self, chain: &ChainClient) {
        let Some(v) = chain.verification() else {
            return;
        };
        if self.json {
            return;
        }
        if v.agreed {
            eprintln!(
                "verified by {} nodes ({} operators)",
                v.verified_by(),
                v.operators.len()
            );
        } else if v.single_operator {
            eprintln!("verified by 1 node â€” only one operator is reachable on this network; agreement could not be independent");
        } else {
            eprintln!(
                "verified by {} node(s); no second operator answered",
                v.verified_by()
            );
        }
    }

    async fn link(&self) -> anyhow::Result<Arc<Link>> {
        self.link
            .get_or_try_init(|| async { self.connect_link().await.map(Arc::new) })
            .await
            .cloned()
    }

    async fn connect_link(&self) -> anyhow::Result<Link> {
        // Operator-supplied bootstrap addresses win. With none configured, a
        // Mainnet profile falls back to the list compiled into the binary,
        // the same file the nodes use (app/params/mainnet/bootstrap_peers.txt),
        // so a fresh client needs no address typed by anyone. Devnets have
        // nothing built in and must be told.
        let configured: Vec<String> =
            if self.profile.bootstrap.is_empty() && self.network.is_mainnet() {
                hashgram_sdk::net::mainnet_bootstrap_peers()
            } else {
                self.profile.bootstrap.clone()
            };
        let addrs: Vec<Multiaddr> = configured
            .iter()
            .map(|a| a.parse().with_context(|| format!("bootstrap {a}")))
            .collect::<anyhow::Result<_>>()?;
        if addrs.is_empty() {
            bail!("no bootstrap peers configured; run `hashgram-client configure --bootstrap /ip4/.../p2p/...`");
        }
        let link = Link::connect(
            &self.network,
            &addrs,
            Some(&self.home.join("peerstore.json")),
            Duration::from_secs(10),
        )
        .await?;
        if link.peers().await.is_empty() {
            bail!("no node completed the Hashgram handshake within 10s; check the bootstrap addresses and the genesis hash");
        }
        Ok(link)
    }

    fn out<T: Serialize>(&self, v: &T, text: impl FnOnce() -> String) {
        if self.json {
            println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
        } else {
            println!("{}", text());
        }
    }
}

fn load_passphrase(file: Option<&Path>) -> anyhow::Result<String> {
    if let Some(f) = file {
        return Ok(std::fs::read_to_string(f)?.trim().to_owned());
    }
    if let Ok(p) = std::env::var("HASHGRAM_PASSPHRASE") {
        return Ok(p);
    }
    bail!("set HASHGRAM_PASSPHRASE or pass --passphrase-file")
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();
    if let Err(e) = run().await {
        eprintln!("hashgram-client: {e:#}");
        std::process::exit(1);
    }
}

async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let home = expand_home(&cli.home);
    std::fs::create_dir_all(&home)?;
    let profile_path = home.join("profile.toml");

    if let Cmd::Configure {
        network,
        genesis_hash,
        chain_api,
        bootstrap,
    } = &cli.cmd
    {
        let p = Profile {
            network: network.clone(),
            genesis_hash: genesis_hash.to_ascii_lowercase(),
            chain_api: chain_api.clone(),
            bootstrap: bootstrap.clone(),
        };
        std::fs::write(&profile_path, toml::to_string_pretty(&p)?)?;
        println!("profile written to {}", profile_path.display());
        return Ok(());
    }

    let profile: Profile =
        toml::from_str(&std::fs::read_to_string(&profile_path).with_context(|| {
            format!(
                "{} not found; run `hashgram-client configure` first",
                profile_path.display()
            )
        })?)?;
    let network = match profile.network.as_str() {
        "mainnet" => NetworkIdentity::mainnet(&profile.genesis_hash),
        "devnet" => NetworkIdentity::devnet(&profile.genesis_hash),
        other => bail!("profile network {other:?} is not mainnet or devnet"),
    };
    if cli.light_kdf && network.is_mainnet() {
        bail!("--light-kdf is DEVNET ONLY");
    }
    let ctx = Ctx {
        home,
        profile,
        network,
        passphrase: load_passphrase(cli.passphrase_file.as_deref())?,
        kdf: if cli.light_kdf {
            KdfCost::light()
        } else {
            KdfCost::default()
        },
        json: cli.json,
        link: tokio::sync::OnceCell::new(),
    };

    match cli.cmd {
        Cmd::Configure { .. } => unreachable!(),
        Cmd::One { cmd } => {
            one::run(
                one::OneCtx {
                    home: &ctx.home,
                    network: ctx.network.clone(),
                    passphrase: &ctx.passphrase,
                    kdf: ctx.kdf,
                    chain_api: ctx.profile.chain_api.clone(),
                    bootstrap: ctx.profile.bootstrap.clone(),
                    json: ctx.json,
                },
                cmd,
            )
            .await
        }
        Cmd::Identity { cmd } => identity(&ctx, cmd).await,
        Cmd::Wallet { cmd } => wallet(&ctx, cmd).await,
        Cmd::Message { cmd } => message(&ctx, cmd).await,
        Cmd::Group { cmd } => group(&ctx, cmd).await,
        Cmd::Profile {
            cmd:
                ProfileCmd::Create {
                    name,
                    bio,
                    avatar_cid,
                },
        } => {
            let p = pb::ProfileUpdate {
                display_name: name,
                bio,
                avatar_cid: avatar_cid.map(hex::decode).transpose()?.unwrap_or_default(),
                ..Default::default()
            };
            publish_event(&ctx, "PROFILE_UPDATE", &p, vec![]).await
        }
        Cmd::Follow { target } => {
            publish_event(&ctx, "FOLLOW", &pb::Follow { target }, vec![]).await
        }
        Cmd::Unfollow { target } => {
            publish_event(&ctx, "UNFOLLOW", &pb::Unfollow { target }, vec![]).await
        }
        Cmd::Post {
            text,
            hashtags,
            channel,
            media_cid,
            media_mime,
            media_size,
        } => {
            let media = match media_cid {
                Some(c) => vec![pb::MediaReference {
                    cid: hex::decode(c)?,
                    mime: media_mime,
                    size: media_size,
                    kind: "image".into(),
                    ..Default::default()
                }],
                None => vec![],
            };
            let p = pb::PostCreate {
                text,
                hashtags,
                channel: channel.map(hex::decode).transpose()?.unwrap_or_default(),
                ..Default::default()
            };
            publish_event(&ctx, "POST_CREATE", &p, media).await
        }
        Cmd::Comment { post, text } => {
            publish_event(
                &ctx,
                "COMMENT_CREATE",
                &pb::CommentCreate {
                    post: hex::decode(post)?,
                    text,
                    ..Default::default()
                },
                vec![],
            )
            .await
        }
        Cmd::React { target, reaction } => {
            publish_event(
                &ctx,
                "REACTION",
                &pb::Reaction {
                    target: hex::decode(target)?,
                    reaction,
                },
                vec![],
            )
            .await
        }
        Cmd::Repost { post } => {
            publish_event(
                &ctx,
                "REPOST",
                &pb::Repost {
                    post: hex::decode(post)?,
                    ..Default::default()
                },
                vec![],
            )
            .await
        }
        Cmd::Reel { cmd } => reel(&ctx, cmd).await,
        Cmd::Channel {
            cmd: ChannelCmd::Create { name, description },
        } => {
            publish_event(
                &ctx,
                "CHANNEL_CREATE",
                &pb::ChannelCreate {
                    name,
                    description,
                    ..Default::default()
                },
                vec![],
            )
            .await
        }
        Cmd::Feed {
            author,
            from,
            limit,
        } => {
            let acct = ctx.account()?;
            let social = Social::open(&acct)?;
            let link = ctx.link().await?;
            let events = social
                .fetch_author(&link, &ctx.network, &author, from, limit)
                .await?;
            let view: Vec<serde_json::Value> = events
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "id": hex::encode(&e.id),
                        "type": e.r#type,
                        "author": e.author,
                        "sequence": e.sequence,
                        "timestamp": e.timestamp,
                        "payload": payload_json(e),
                        "media": e.media.iter().map(|m| hex::encode(&m.cid)).collect::<Vec<_>>(),
                    })
                })
                .collect();
            ctx.out(&view, || {
                view.iter()
                    .map(|v| {
                        format!(
                            "{} #{} {} {}",
                            v["type"], v["sequence"], v["id"], v["payload"]
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
            link.shutdown().await;
            Ok(())
        }
        Cmd::Blob { cmd } => blob_cmd(&ctx, cmd).await,
        Cmd::Net { cmd } => net(&ctx, cmd).await,
        Cmd::Call { cmd } => call(&ctx, cmd).await,
    }
}

async fn call(ctx: &Ctx, cmd: CallCmd) -> anyhow::Result<()> {
    let link = ctx.link().await?;
    let result: anyhow::Result<()> = async {
        match cmd {
            CallCmd::Discover => {
                let nodes = hashgram_sdk::calls::discover(&link).await?;
                ctx.out(&nodes, || {
                    if nodes.is_empty() {
                        return "no call nodes announced".to_owned();
                    }
                    nodes
                        .iter()
                        .map(|n| {
                            format!(
                                "TURN {:?} realm={} creds={} sfu={:?} operator={}",
                                n.turn_uris, n.realm, n.issues_credentials, n.sfu_url, n.operator
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                });
                Ok(())
            }
            CallCmd::Turn => {
                let acct = ctx.account()?;
                let ice = hashgram_sdk::calls::turn_credentials(
                    &link,
                    &ctx.network,
                    &acct.device()?,
                    None,
                )
                .await?;
                ctx.out(&ice, || {
                    format!(
                        "urls       {:?}\nusername   {}\ncredential {}\nexpires_at {}",
                        ice.urls, ice.username, ice.credential, ice.expires_at
                    )
                });
                Ok(())
            }
            CallCmd::Signal {
                group,
                kind,
                sdp,
                candidate,
                call_id,
                video,
            } => {
                let mut acct = ctx.account()?;
                let mut messaging = Messaging::open(&acct)?;
                let call_id = match call_id {
                    Some(c) => hex::decode(c)?,
                    None => {
                        let s = hashgram_sdk::proto::Ed25519Signer::generate()?;
                        s.public_key()[..16].to_vec()
                    }
                };
                let sig = hashgram_sdk::chat::CallSignal {
                    kind: kind.clone(),
                    call_id: call_id.clone(),
                    sdp,
                    candidate,
                    video,
                    ..Default::default()
                };
                messaging
                    .send_call_signal(&link, &ctx.network, &hex::decode(group)?, sig)
                    .await?;
                messaging.persist(&mut acct)?;
                acct.save()?;
                println!("call signal {kind} sent (call {})", hex::encode(call_id));
                Ok(())
            }
        }
    }
    .await;
    link.shutdown().await;
    result
}

fn parse_hash(amount: &str) -> anyhow::Result<u128> {
    // "1.5" HASH -> 1_500_000 uhash, exact decimal arithmetic.
    let (whole, frac) = amount.split_once('.').unwrap_or((amount, ""));
    let whole: u128 = whole.parse().context("amount")?;
    let mut frac = frac.to_owned();
    if frac.len() > 6 {
        bail!("HASH has 6 decimal places");
    }
    while frac.len() < 6 {
        frac.push('0');
    }
    let frac: u128 = if frac.is_empty() {
        0
    } else {
        frac.parse().context("amount")?
    };
    Ok(whole * 1_000_000 + frac)
}

fn fmt_hash(uhash: u128) -> String {
    format!(
        "{}.{:06} HASH ({uhash} uhash)",
        uhash / 1_000_000,
        uhash % 1_000_000
    )
}

async fn identity(ctx: &Ctx, cmd: IdentityCmd) -> anyhow::Result<()> {
    match cmd {
        IdentityCmd::Create {
            device_id,
            label,
            platform,
            offline,
        } => {
            let (acct, mnemonic) =
                Account::create(&ctx.vault_path(), &ctx.passphrase, &device_id, ctx.kdf)?;
            println!("address:  {}", acct.address());
            println!(
                "device:   {} ({})",
                device_id,
                hex::encode(acct.device().map(|d| d.public_key()).unwrap_or_default())
            );
            println!();
            println!("RECOVERY PHRASE â€” write it down now, it is shown once and never stored:");
            println!("  {mnemonic}");
            println!();
            if offline {
                println!(
                    "vault written to {}; register later with funds in the account",
                    ctx.vault_path().display()
                );
                return Ok(());
            }
            register(ctx, &acct, &label, &platform).await
        }
        IdentityCmd::Import {
            mnemonic_file,
            device_id,
            offline,
        } => {
            let m = std::fs::read_to_string(&mnemonic_file)?;
            let acct = Account::import(
                &ctx.vault_path(),
                &ctx.passphrase,
                m.trim(),
                &device_id,
                ctx.kdf,
            )?;
            println!("address: {}", acct.address());
            if offline {
                return Ok(());
            }
            register(ctx, &acct, "hashgram-client", "linux").await
        }
        IdentityCmd::ImportSecret {
            secret_file,
            device_id,
            offline,
        } => {
            if ctx.network.is_mainnet() {
                bail!("import-secret is DEVNET ONLY");
            }
            let raw = std::fs::read_to_string(&secret_file)?;
            let secret = hex::decode(raw.trim()).context("secret file is not hex")?;
            let acct = Account::import_secret(
                &ctx.vault_path(),
                &ctx.passphrase,
                &secret,
                &device_id,
                ctx.kdf,
            )?;
            println!("address: {}", acct.address());
            if offline {
                return Ok(());
            }
            register(ctx, &acct, "hashgram-client", "linux").await
        }
        IdentityCmd::Register { label, platform } => {
            let acct = ctx.account()?;
            register(ctx, &acct, &label, &platform).await
        }
        IdentityCmd::Show => {
            let acct = ctx.account()?;
            let v = serde_json::json!({
                "address": acct.address(),
                "device_id": acct.contents.device_id,
                "device_pubkey": hex::encode(acct.device()?.public_key()),
                "holds_wallet_key": acct.contents.wallet_secret.is_some(),
                "holds_root_key": acct.contents.root_seed.is_some(),
                "root_pubkey": acct.root().ok().map(|r| hex::encode(r.public_key())),
                "mls_state": acct.contents.extra.contains_key(hashgram_sdk::messaging::VAULT_MLS_KEY),
            });
            ctx.out(&v, || serde_json::to_string_pretty(&v).unwrap_or_default());
            Ok(())
        }
        IdentityCmd::NewDevice { address, device_id } => {
            let acct = Account::create_device_only(
                &ctx.vault_path(),
                &ctx.passphrase,
                &address,
                &device_id,
                ctx.kdf,
            )?;
            println!(
                "device-only vault written to {}",
                ctx.vault_path().display()
            );
            println!("device id:      {device_id}");
            println!(
                "device pubkey:  {}",
                hex::encode(acct.device()?.public_key())
            );
            println!("Hand the pubkey to the root holder: hashgram-client identity add-device --device-id {device_id} --pubkey <hex>");
            Ok(())
        }
        IdentityCmd::AddDevice {
            device_id,
            pubkey,
            label,
            platform,
        } => {
            let acct = ctx.account()?;
            let chain = ctx.chain().await?;
            let key: [u8; 32] = hex::decode(pubkey)?
                .try_into()
                .map_err(|_| anyhow::anyhow!("pubkey must be 32 bytes"))?;
            let rot = account::rotation_count_on_chain(&chain, acct.address())
                .await?
                .ok_or_else(|| anyhow::anyhow!("no identity on chain for {}", acct.address()))?;
            let r = account::add_device_on_chain(
                &acct,
                &ctx.network,
                &chain,
                &device_id,
                &key,
                rot,
                &label,
                &platform,
            )
            .await?;
            println!(
                "device {device_id} authorised in tx {} at height {}",
                r.txhash, r.height
            );
            Ok(())
        }
        IdentityCmd::RemoveDevice { device_id } => {
            let acct = ctx.account()?;
            let chain = ctx.chain().await?;
            let r = account::revoke_device_on_chain(&acct, &chain, &device_id).await?;
            println!(
                "device {device_id} revoked in tx {} at height {}",
                r.txhash, r.height
            );
            Ok(())
        }
        IdentityCmd::Devices { address } => {
            let chain = ctx.chain().await?;
            let addr = match address {
                Some(a) => a,
                None => ctx.account()?.address().to_owned(),
            };
            let devices = account::devices_on_chain(&chain, &addr).await?;
            ctx.out(&devices, || {
                devices
                    .iter()
                    .map(|d| {
                        format!(
                            "{:<16} {} {}{}",
                            d.device_id,
                            d.device_pubkey,
                            d.label,
                            if d.revoked { " (REVOKED)" } else { "" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
            Ok(())
        }
        IdentityCmd::PublishKeys => {
            let mut acct = ctx.account()?;
            let mut messaging = Messaging::open(&acct)?;
            let link = ctx.link().await?;
            let n = messaging.publish_key_package(&link, &ctx.network).await?;
            messaging.persist(&mut acct)?;
            acct.save()?;
            println!("key package published to {n} store node(s)");
            link.shutdown().await;
            Ok(())
        }
    }
}

async fn register(ctx: &Ctx, acct: &Account, label: &str, platform: &str) -> anyhow::Result<()> {
    let chain = ctx.chain().await?;
    let bal = chain.balance(acct.address()).await?;
    if bal == 0 {
        println!("the account has no HASH yet; fund {} and run `identity create` again with --offline omitted,", acct.address());
        println!("or register later. The vault is saved.");
        return Ok(());
    }
    let r = account::create_identity_on_chain(acct, &ctx.network, &chain, label, platform).await?;
    println!(
        "identity registered in tx {} at height {}",
        r.txhash, r.height
    );
    Ok(())
}

async fn wallet(ctx: &Ctx, cmd: WalletCmd) -> anyhow::Result<()> {
    let chain = ctx.chain().await?;
    match cmd {
        WalletCmd::Balance { address } => {
            let addr = match address {
                Some(a) => a,
                None => ctx.account()?.address().to_owned(),
            };
            let bal = chain.balance(&addr).await?;
            let verification = chain.verification();
            let v = serde_json::json!({
                "address": addr,
                "uhash": bal.to_string(),
                "hash": format!("{}.{:06}", bal / 1_000_000, bal % 1_000_000),
                "transport": chain.transport().describe(),
                "verified_by": verification.as_ref().map(|v| v.verified_by()),
                "agreed": verification.as_ref().map(|v| v.agreed),
                "single_operator": verification.as_ref().map(|v| v.single_operator),
                "peers": verification.as_ref().map(|v| v.peers.clone()),
            });
            ctx.out(&v, || format!("{addr}\n{}", fmt_hash(bal)));
            ctx.note_verification(&chain);
            Ok(())
        }
        WalletCmd::Send { to, amount } => {
            let acct = ctx.account()?;
            let w = acct.wallet()?;
            let uhash = parse_hash(&amount)?;
            let r = chain
                .sign_and_broadcast(
                    &w,
                    vec![hashgram_sdk::chain::msgs::bank_send(
                        acct.address(),
                        &to,
                        uhash,
                    )],
                    "",
                )
                .await?;
            let v = serde_json::json!({ "txhash": r.txhash, "height": r.height, "gas_used": r.gas_used });
            ctx.out(&v, || {
                format!(
                    "sent {} to {to}\ntx {} height {} gas {}",
                    fmt_hash(uhash),
                    v["txhash"],
                    r.height,
                    r.gas_used
                )
            });
            Ok(())
        }
        WalletCmd::History { limit } => {
            let acct = ctx.account()?;
            let addr = acct.address();
            let mut all = Vec::new();
            for q in [
                format!("transfer.recipient='{addr}'"),
                format!("message.sender='{addr}'"),
            ] {
                let path = format!(
                    "cosmos/tx/v1beta1/txs?query={}&limit={limit}&order_by=ORDER_BY_DESC",
                    urlenc(&q)
                );
                if let Ok(v) = chain.query(&path).await {
                    if let Some(list) = v.get("tx_responses").and_then(|x| x.as_array()) {
                        for t in list {
                            all.push(serde_json::json!({
                                "height": t.get("height"),
                                "txhash": t.get("txhash"),
                                "code": t.get("code"),
                                "memo": t.get("tx").and_then(|x| x.get("body")).and_then(|b| b.get("memo")),
                                "msgs": t.get("tx").and_then(|x| x.get("body")).and_then(|b| b.get("messages")).and_then(|m| m.as_array()).map(|m| m.iter().filter_map(|x| x.get("@type").cloned()).collect::<Vec<_>>()),
                            }));
                        }
                    }
                }
            }
            ctx.out(&all, || {
                all.iter()
                    .map(|t| {
                        format!(
                            "h{} {} code={} {:?}",
                            t["height"], t["txhash"], t["code"], t["msgs"]
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
            Ok(())
        }
        WalletCmd::RegisterName { name } => {
            let acct = ctx.account()?;
            let w = acct.wallet()?;
            let msg = hashgram_sdk::chain::pb::username::MsgRegister {
                owner: acct.address().to_owned(),
                name: name.clone(),
            };
            let r = chain
                .sign_and_broadcast(
                    &w,
                    vec![hashgram_sdk::chain::msgs::register_username(&msg)],
                    "",
                )
                .await?;
            println!(
                "registered @{} in tx {} at height {}",
                name.trim_start_matches('@'),
                r.txhash,
                r.height
            );
            Ok(())
        }
    }
}

fn urlenc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

async fn message(ctx: &Ctx, cmd: MessageCmd) -> anyhow::Result<()> {
    let mut acct = ctx.account()?;
    let mut messaging = Messaging::open(&acct)?;
    let link = ctx.link().await?;
    let chain = ctx.chain().await?;
    let result: anyhow::Result<()> = async {
        match cmd {
            MessageCmd::Send { to, text } => {
                // Reuse an existing direct conversation with exactly this peer.
                let existing = messaging
                    .conversations()
                    .into_iter()
                    .find(|(_, meta, addrs)| {
                        meta.direct && addrs.iter().any(|a| a == &to) && addrs.len() <= 2
                    })
                    .map(|(gid, _, _)| hex::decode(gid).unwrap_or_default());
                let gid = match existing {
                    Some(g) => g,
                    None => {
                        messaging
                            .create_conversation(
                                &link,
                                &chain,
                                &ctx.network,
                                "",
                                std::slice::from_ref(&to),
                            )
                            .await?
                    }
                };
                let id = messaging
                    .send_text(&link, &ctx.network, &gid, &text)
                    .await?;
                println!(
                    "sent {} in conversation {}",
                    hex::encode(id),
                    hex::encode(gid)
                );
                Ok(())
            }
            MessageCmd::SendGroup { group, text } => {
                let gid = hex::decode(group)?;
                let id = messaging
                    .send_text(&link, &ctx.network, &gid, &text)
                    .await?;
                println!("sent {} in group {}", hex::encode(id), hex::encode(gid));
                Ok(())
            }
            MessageCmd::Receive { watch } => loop {
                let got = messaging.sync(&link, &ctx.network, Some(&chain)).await?;
                for r in &got {
                    if ctx.json {
                        println!("{}", serde_json::to_string(r).unwrap_or_default());
                    } else {
                        println!(
                            "[{}] {} ({}): {} {}{}",
                            &r.group_id[..12],
                            r.sender,
                            r.message.kind,
                            r.message.text,
                            if r.message.attachments.is_empty() {
                                String::new()
                            } else {
                                format!("{:?}", r.message.attachments)
                            },
                            r.message
                                .call
                                .as_ref()
                                .map(|c| format!(
                                    "call {} {} sdp={}b",
                                    c.kind,
                                    c.call_id,
                                    c.sdp.len()
                                ))
                                .unwrap_or_default()
                        );
                    }
                }
                messaging.persist(&mut acct)?;
                acct.save()?;
                match watch {
                    Some(secs) => tokio::time::sleep(Duration::from_secs(secs)).await,
                    None => {
                        if got.is_empty() {
                            println!("no new messages");
                        }
                        break Ok(());
                    }
                }
            },
            MessageCmd::List => {
                for (gid, meta, addrs) in messaging.conversations() {
                    println!(
                        "{gid}  {}  members={}",
                        if meta.direct {
                            "direct".to_owned()
                        } else {
                            format!("group:{}", meta.name)
                        },
                        addrs.join(",")
                    );
                }
                Ok(())
            }
        }
    }
    .await;
    messaging.persist(&mut acct)?;
    acct.save()?;
    link.shutdown().await;
    result
}

async fn group(ctx: &Ctx, cmd: GroupCmd) -> anyhow::Result<()> {
    let mut acct = ctx.account()?;
    let mut messaging = Messaging::open(&acct)?;
    let link = ctx.link().await?;
    let chain = ctx.chain().await?;
    let result: anyhow::Result<()> = async {
        match cmd {
            GroupCmd::Create { name, members } => {
                let gid = messaging
                    .create_conversation(&link, &chain, &ctx.network, &name, &members)
                    .await?;
                println!("group {} created: {}", name, hex::encode(gid));
                Ok(())
            }
            GroupCmd::Add { group, address } => {
                messaging
                    .add_participant(&link, &chain, &ctx.network, &hex::decode(group)?, &address)
                    .await?;
                println!("added {address}");
                Ok(())
            }
            GroupCmd::Remove { group, address } => {
                let n = messaging
                    .remove_participant(&link, &ctx.network, &hex::decode(group)?, &address)
                    .await?;
                println!("removed {n} device(s) of {address}");
                Ok(())
            }
            GroupCmd::Members { group } => {
                for m in messaging.mls().members(&hex::decode(group)?)? {
                    println!("{:>3}  {}", m.index, m.identity);
                }
                Ok(())
            }
        }
    }
    .await;
    messaging.persist(&mut acct)?;
    acct.save()?;
    link.shutdown().await;
    result
}

async fn publish_event<M: prost::Message>(
    ctx: &Ctx,
    kind: &str,
    payload: &M,
    media: Vec<pb::MediaReference>,
) -> anyhow::Result<()> {
    let mut acct = ctx.account()?;
    let mut social = Social::open(&acct)?;
    let ev = social.build(&ctx.network, kind, payload, media)?;
    let link = ctx.link().await?;
    let id = social.publish(&link, ev.clone()).await?;
    social.persist(&mut acct);
    acct.save()?;
    let v = serde_json::json!({ "id": hex::encode(&id), "type": kind, "sequence": ev.sequence });
    ctx.out(&v, || {
        format!(
            "{kind} published: {} (sequence {})",
            hex::encode(&id),
            ev.sequence
        )
    });
    link.shutdown().await;
    Ok(())
}

async fn reel(ctx: &Ctx, cmd: ReelCmd) -> anyhow::Result<()> {
    let (data, caption, hashtags, mime, duration_ms) = match cmd {
        ReelCmd::Publish {
            video,
            caption,
            hashtags,
            mime,
            duration_ms,
        } => (std::fs::read(&video)?, caption, hashtags, mime, duration_ms),
        ReelCmd::PublishTest => {
            let mut d = vec![0u8; 300_000];
            let _ = getrandom_fill(&mut d);
            (
                d,
                "test reel".into(),
                vec!["test".into()],
                "video/mp4".into(),
                5000,
            )
        }
    };
    let mut acct = ctx.account()?;
    let device = acct.device()?;
    let link = ctx.link().await?;
    let up = blob::upload(&link, &ctx.network, &device, &data, &mime, false, 2).await?;
    let media = vec![pb::MediaReference {
        cid: hex::decode(&up.cid)?,
        mime: mime.clone(),
        size: up.size,
        kind: "video".into(),
        duration_ms,
        content_hash: hex::decode(&up.plaintext_hash)?,
        ..Default::default()
    }];
    let mut social = Social::open(&acct)?;
    let ev = social.build(
        &ctx.network,
        "REEL_CREATE",
        &pb::ReelCreate {
            caption,
            hashtags,
            video_index: 0,
            allow_comments: true,
            ..Default::default()
        },
        media,
    )?;
    let id = social.publish(&link, ev).await?;
    social.persist(&mut acct);
    acct.save()?;
    let v = serde_json::json!({ "event": hex::encode(&id), "video_cid": up.cid, "providers": up.providers });
    ctx.out(&v, || {
        format!(
            "reel published: event {} video {} on {} node(s)",
            hex::encode(&id),
            up.cid,
            up.providers.len()
        )
    });
    link.shutdown().await;
    Ok(())
}

fn getrandom_fill(buf: &mut [u8]) -> Result<(), ()> {
    // A test reel is random bytes; use the device key generator's source.
    let s = hashgram_sdk::proto::Ed25519Signer::generate().map_err(|_| ())?;
    let mut seed = s.secret_bytes().to_vec();
    for chunk in buf.chunks_mut(32) {
        let h = blake3::hash(&seed);
        seed = h.as_bytes().to_vec();
        let n = chunk.len();
        chunk.copy_from_slice(&h.as_bytes()[..n]);
    }
    Ok(())
}

async fn blob_cmd(ctx: &Ctx, cmd: BlobCmd) -> anyhow::Result<()> {
    match cmd {
        BlobCmd::Upload {
            file,
            mime,
            private,
            replicas,
        } => {
            let acct = ctx.account()?;
            let device = acct.device()?;
            let data = std::fs::read(&file)?;
            let link = ctx.link().await?;
            let up = blob::upload(
                &link,
                &ctx.network,
                &device,
                &data,
                &mime,
                private,
                replicas,
            )
            .await?;
            ctx.out(&up, || {
                let mut s = format!("cid       {}\nsize      {}\nchunks    {}\nproviders {}", up.cid, up.size, up.chunks, up.providers.join(","));
                if let (Some(k), Some(n)) = (&up.key, &up.nonce) {
                    s.push_str(&format!("\nkey       {k}\nnonce     {n}\n(private: share key and nonce only inside an E2EE message)"));
                }
                s
            });
            link.shutdown().await;
            Ok(())
        }
        BlobCmd::Download {
            cid,
            out,
            key,
            nonce,
        } => {
            let link = ctx.link().await?;
            let c = hex::decode(&cid)?;
            let acct = ctx.account()?;
            let device = acct.device()?;
            let chain = ctx.chain().await?;
            let (bytes, m, from) =
                blob::download(&link, &c, Some((&chain, &ctx.network, &device))).await?;
            let bytes = match (key, nonce) {
                (Some(k), Some(n)) => {
                    let fk = blob::FileKey {
                        key: hex::decode(k)?
                            .try_into()
                            .map_err(|_| anyhow::anyhow!("key must be 32 bytes"))?,
                        nonce: hex::decode(n)?
                            .try_into()
                            .map_err(|_| anyhow::anyhow!("nonce must be 24 bytes"))?,
                    };
                    blob::decrypt_private(&bytes, &fk)?
                }
                _ => bytes,
            };
            std::fs::write(&out, &bytes)?;
            println!(
                "downloaded {} bytes ({}) from {from} to {}",
                bytes.len(),
                m.mime,
                out.display()
            );
            link.shutdown().await;
            Ok(())
        }
        BlobCmd::Providers { cid } => {
            let link = ctx.link().await?;
            let ps = blob::providers(&link, &hex::decode(cid)?).await;
            for p in &ps {
                println!("{p}");
            }
            if ps.is_empty() {
                println!("no providers");
            }
            link.shutdown().await;
            Ok(())
        }
        BlobCmd::Verify {
            file,
            cid,
            mime,
            encrypted,
        } => {
            let data = std::fs::read(&file)?;
            if blob::verify(&data, &mime, encrypted, &cid) {
                println!("OK: {} is the content of {cid}", file.display());
                Ok(())
            } else {
                bail!(
                    "MISMATCH: {} is not the content of {cid} (mime {mime}, encrypted {encrypted})",
                    file.display()
                )
            }
        }
    }
}

async fn net(ctx: &Ctx, cmd: NetCmd) -> anyhow::Result<()> {
    let link = ctx.link().await?;
    match cmd {
        NetCmd::Peers => {
            for p in link.peers().await {
                println!("{}  {}", p.peer, p.roles.join(","));
            }
        }
        NetCmd::Announcements { role } => {
            let roles: Vec<&str> = role.iter().map(String::as_str).collect();
            for a in link.announcements(&roles).await? {
                println!(
                    "{:<40} {} storage={} addrs={:?}",
                    a.roles.join(","),
                    a.operator_address,
                    a.declared_storage_bytes,
                    a.addrs
                );
            }
        }
        NetCmd::Calls => {
            for a in link.announcements(&["call"]).await? {
                if let Some(t) = &a.turn {
                    println!(
                        "TURN {:?} realm={} credentials={}",
                        t.uris, t.realm, t.issues_credentials
                    );
                }
                if let Some(s) = &a.sfu {
                    println!("SFU  {} ({})", s.url, s.kind);
                }
            }
        }
    }
    link.shutdown().await;
    Ok(())
}
