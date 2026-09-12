//! `hashgram-client one …`: the Hashgram One application commands.
//!
//! A developer harness over `hashgram_sdk::HashgramOne` — the same facade
//! the desktop application uses — so every Mail, Drive, People, Feed,
//! Circle, Space, Earn and Network flow can be exercised end to end
//! against a live network from a terminal. Output is text, or JSON with
//! `--json`.

use std::path::Path;
use std::time::Duration;

use anyhow::Context as _;
use clap::Subcommand;
use hashgram_sdk::mail::folder;
use hashgram_sdk::protocol::mail::Draft;
use hashgram_sdk::protocol::pb as app;
use hashgram_sdk::{Config, HashgramOne, KdfCost, Multiaddr, NetworkIdentity, Paths};
use serde::Serialize;

/// Top-level `one` commands.
#[derive(Subcommand)]
pub enum OneCmd {
    /// HashMail.
    Mail {
        #[command(subcommand)]
        cmd: MailCmd,
    },
    /// HashDrive.
    Drive {
        #[command(subcommand)]
        cmd: DriveCmd,
    },
    /// People.
    People {
        #[command(subcommand)]
        cmd: PeopleCmd,
    },
    /// Feed.
    Feed {
        #[command(subcommand)]
        cmd: FeedCmd,
    },
    /// Circles.
    Circle {
        #[command(subcommand)]
        cmd: CircleCmd,
    },
    /// Spaces.
    Space {
        #[command(subcommand)]
        cmd: SpaceCmd,
    },
    /// Devices.
    Devices {
        #[command(subcommand)]
        cmd: DevicesCmd,
    },
    /// Earn / provider.
    Provider {
        #[command(subcommand)]
        cmd: ProviderCmd,
    },
    /// Network view.
    Network {
        #[command(subcommand)]
        cmd: NetworkCmd,
    },
    /// Encrypted vault backup (docs/MULTI_DEVICE_SECURITY.md §7.3).
    Backup {
        #[command(subcommand)]
        cmd: BackupCmd,
    },
    /// Run one sync round (mailbox, outbox, feed, wallet, devices).
    Sync {
        /// Repeat every N seconds until interrupted.
        #[arg(long)]
        watch: Option<u64>,
    },
    /// Wallet balance through the facade.
    Balance,
}

/// Backup commands.
#[derive(Subcommand)]
pub enum BackupCmd {
    /// Export an encrypted backup; passphrase from HASHGRAM_BACKUP_PASSPHRASE.
    Export {
        out: String,
        /// DEVNET ONLY: light KDF for tests.
        #[arg(long)]
        light: bool,
    },
    /// Show a backup's KDF parameters (no passphrase needed).
    Inspect { file: String },
}

/// Mail commands.
#[derive(Subcommand)]
pub enum MailCmd {
    /// Send a message.
    Send {
        /// Recipients: hash1…, @name, name@hashgram.io (repeat). Optional
        /// when replying (the reply target's sender is used).
        #[arg(long = "to")]
        to: Vec<String>,
        /// CC recipients.
        #[arg(long = "cc")]
        cc: Vec<String>,
        /// BCC recipients.
        #[arg(long = "bcc")]
        bcc: Vec<String>,
        /// Subject.
        #[arg(long, default_value = "")]
        subject: String,
        /// Body text (or `-` to read stdin).
        #[arg(long, default_value = "")]
        body: String,
        /// Attach a file (repeat).
        #[arg(long = "attach")]
        attach: Vec<String>,
        /// Attach a Drive entry by hex id as a live shared file (repeat).
        #[arg(long = "attach-drive-live")]
        attach_drive_live: Vec<String>,
        /// Attach a Drive entry by hex id as a snapshot (repeat).
        #[arg(long = "attach-drive")]
        attach_drive: Vec<String>,
        /// Reply to a message id (hex).
        #[arg(long)]
        reply_to: Option<String>,
        /// Reply-all when replying.
        #[arg(long)]
        all: bool,
        /// Ask for a read receipt.
        #[arg(long)]
        read_receipt: bool,
    },
    /// List a folder.
    List {
        /// inbox|sent|drafts|archive|trash|spam|requests
        #[arg(default_value = "inbox")]
        folder: String,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// Group by thread.
        #[arg(long)]
        threads: bool,
    },
    /// Show a message.
    Show { id: String },
    /// Show a thread.
    Thread { thread_id: String },
    /// Mark read/unread.
    Read {
        id: String,
        #[arg(long)]
        unread: bool,
    },
    /// Star / unstar.
    Star {
        id: String,
        #[arg(long)]
        off: bool,
    },
    /// Move to a folder.
    Move { id: String, folder: String },
    /// Archive.
    Archive { id: String },
    /// Trash (or permanently delete when already trashed).
    Trash { id: String },
    /// Accept a Requests message into the Inbox.
    Accept { id: String },
    /// Add or remove a label.
    Label {
        id: String,
        label: String,
        #[arg(long)]
        remove: bool,
    },
    /// Local search.
    Search {
        query: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Folder counters.
    Counts,
    /// Save an attachment to a file.
    Attachment {
        id: String,
        /// Attachment index.
        index: usize,
        out: String,
    },
    /// Toggle read receipts.
    Settings {
        #[arg(long)]
        read_receipts: Option<bool>,
    },
}

/// Drive commands.
#[derive(Subcommand)]
pub enum DriveCmd {
    /// List a folder ("" = root) by hex id or path.
    Ls {
        #[arg(default_value = "/")]
        path: String,
    },
    /// Create a folder under a path.
    Mkdir { path: String },
    /// Upload a file into a folder path.
    Put {
        file: String,
        #[arg(default_value = "/")]
        folder: String,
        #[arg(long)]
        mime: Option<String>,
        #[arg(long)]
        name: Option<String>,
    },
    /// Download a file (by hex id or path) to `out`.
    Get {
        entry: String,
        out: String,
        #[arg(long)]
        version: Option<u64>,
    },
    /// Replace content of a file with a new version.
    Update { entry: String, file: String },
    /// Rename.
    Rename { entry: String, name: String },
    /// Move.
    Mv { entry: String, folder: String },
    /// Trash.
    Rm { entry: String },
    /// Restore from trash.
    Restore { entry: String },
    /// Permanently delete (must be trashed).
    Purge { entry: String },
    /// Show trash.
    Trash,
    /// Empty trash.
    EmptyTrash,
    /// Versions of a file.
    Versions { entry: String },
    /// Restore a version.
    RestoreVersion { entry: String, version: u64 },
    /// Re-encrypt a file under a fresh key (revokes its live shares).
    Rekey { entry: String },
    /// Star.
    Star {
        entry: String,
        #[arg(long)]
        off: bool,
    },
    /// Share with addresses (comma separated).
    Share {
        entry: String,
        grantees: String,
        #[arg(long)]
        live: bool,
        #[arg(long, default_value = "")]
        note: String,
    },
    /// Revoke a share.
    Revoke { share_id: String },
    /// Shares granted.
    Shares,
    /// Shared with me.
    SharedWithMe,
    /// Download a capability shared with me (by share id) to `out`.
    GetShared { share_id: String, out: String },
    /// Save a shared file into my Drive.
    SaveShared {
        share_id: String,
        #[arg(default_value = "/")]
        folder: String,
    },
    /// Commit the manifest (also done by sync).
    Commit,
    /// Usage.
    Usage,
    /// Search by name.
    Search { query: String },
}

/// People commands.
#[derive(Subcommand)]
pub enum PeopleCmd {
    /// Resolve an address/username/mail address.
    Resolve { input: String },
    /// Public profile.
    Profile { address: String },
    /// Send a contact request.
    Request {
        input: String,
        #[arg(long, default_value = "")]
        message: String,
    },
    /// Accept an incoming request.
    Accept { address: String },
    /// Reject an incoming request.
    Reject { address: String },
    /// Remove a friend.
    Remove { address: String },
    /// Block / unblock.
    Block {
        address: String,
        #[arg(long)]
        off: bool,
    },
    /// Mute / unmute.
    Mute {
        address: String,
        #[arg(long)]
        off: bool,
    },
    /// Trust / untrust.
    Trust {
        address: String,
        #[arg(long)]
        off: bool,
    },
    /// Follow / unfollow (public).
    Follow {
        address: String,
        #[arg(long)]
        off: bool,
    },
    /// List contacts / requests.
    List {
        /// friends|incoming|outgoing|blocked|all
        #[arg(default_value = "all")]
        which: String,
    },
    /// Set my display name.
    Name { name: String },
    /// Send my profile card to a contact.
    Card {
        address: String,
        #[arg(long, default_value = "")]
        bio: String,
        #[arg(long)]
        disclose_wallet: bool,
    },
}

/// Feed commands.
#[derive(Subcommand)]
pub enum FeedCmd {
    /// Post publicly.
    Post {
        text: String,
        #[arg(long = "tag")]
        tags: Vec<String>,
    },
    /// Comment.
    Comment { post: String, text: String },
    /// React.
    React { target: String, reaction: String },
    /// Repost.
    Repost {
        post: String,
        #[arg(long, default_value = "")]
        comment: String,
    },
    /// Following feed.
    Following {
        #[arg(long, default_value_t = 30)]
        limit: usize,
    },
    /// Friends feed.
    Friends {
        #[arg(long, default_value_t = 30)]
        limit: usize,
    },
    /// One author.
    Author {
        address: String,
        #[arg(long, default_value_t = 30)]
        limit: usize,
    },
    /// Post thread.
    Thread { post: String },
    /// Refresh followed authors.
    Refresh,
    /// Update public profile.
    Profile {
        #[arg(long, default_value = "")]
        name: String,
        #[arg(long, default_value = "")]
        bio: String,
    },
}

/// Circle commands.
#[derive(Subcommand)]
pub enum CircleCmd {
    /// Create.
    Create {
        name: String,
        #[arg(long, default_value = "")]
        description: String,
        /// Initial members.
        #[arg(long = "member")]
        members: Vec<String>,
    },
    /// List.
    List,
    /// Add a member.
    Add { circle: String, address: String },
    /// Remove a member.
    Remove { circle: String, address: String },
    /// Leave.
    Leave { circle: String },
    /// Post.
    Post { circle: String, text: String },
    /// Poll.
    Poll {
        circle: String,
        question: String,
        #[arg(long = "option", required = true)]
        options: Vec<String>,
    },
    /// Comment.
    Comment {
        circle: String,
        post: String,
        text: String,
    },
    /// React.
    React {
        circle: String,
        target: String,
        reaction: String,
    },
    /// Vote.
    Vote {
        circle: String,
        post: String,
        #[arg(long = "choice", required = true)]
        choices: Vec<u32>,
    },
    /// Timeline.
    Posts {
        circle: String,
        #[arg(long, default_value_t = 30)]
        limit: usize,
    },
    /// Comments of a post.
    Comments { circle: String, post: String },
}

/// Space commands.
#[derive(Subcommand)]
pub enum SpaceCmd {
    /// Create.
    Create {
        name: String,
        #[arg(long, default_value = "")]
        description: String,
    },
    /// List.
    List,
    /// Show state.
    Show { space: String },
    /// Invite with a role: guest|member|admin.
    Invite {
        space: String,
        address: String,
        #[arg(long, default_value = "member")]
        role: String,
    },
    /// Remove (or leave when the address is you).
    Remove {
        space: String,
        address: String,
        #[arg(long, default_value = "")]
        reason: String,
    },
    /// Change role: guest|member|admin|owner.
    Role {
        space: String,
        address: String,
        role: String,
    },
    /// Announcement (admin+).
    Announce {
        space: String,
        title: String,
        text: String,
    },
    /// Post.
    Post { space: String, text: String },
    /// Comment.
    Comment {
        space: String,
        post: String,
        text: String,
    },
    /// Content page.
    Content {
        space: String,
        #[arg(long, default_value_t = 30)]
        limit: usize,
    },
    /// Members.
    Members { space: String },
    /// Share a Drive entry into the Space drive.
    ShareDrive {
        space: String,
        entry: String,
        #[arg(long, default_value = "")]
        path: String,
        #[arg(long)]
        snapshot: bool,
    },
    /// Space drive listing.
    Drive { space: String },
    /// Mail every member.
    Mail {
        space: String,
        subject: String,
        body: String,
    },
    /// Update info.
    Info {
        space: String,
        name: String,
        #[arg(long, default_value = "")]
        description: String,
    },
}

/// Devices commands.
#[derive(Subcommand)]
pub enum DevicesCmd {
    /// List our devices from the chain.
    List,
    /// This device.
    This,
    /// Reconcile MLS rosters with the chain.
    Reconcile,
    /// Send keyring/contacts to our other devices.
    Bootstrap,
}

/// Provider commands.
#[derive(Subcommand)]
pub enum ProviderCmd {
    /// Status (ours or an operator's).
    Status { operator: Option<String> },
    /// Earnings.
    Earnings { operator: Option<String> },
    /// List providers.
    List,
    /// Register.
    Register {
        #[arg(long)]
        reward_address: String,
        #[arg(long)]
        node_pubkey: String,
        #[arg(long = "role", required = true)]
        roles: Vec<String>,
        #[arg(long, default_value_t = 1_000_000_000)]
        bond_uhash: u128,
        #[arg(long, default_value_t = 0)]
        storage_bytes: u64,
        #[arg(long, default_value = "")]
        moniker: String,
    },
    /// Begin unbonding.
    Unbond,
    /// Withdraw bond.
    Withdraw,
}

/// Network commands.
#[derive(Subcommand)]
pub enum NetworkCmd {
    /// Overview.
    Overview,
    /// Validators (chain).
    Validators,
    /// Supply figures.
    Supply,
    /// Indexer leaderboards/stats (needs --indexer URL).
    Top {
        /// holders|validators|providers|stats
        what: String,
        #[arg(long)]
        indexer: String,
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
}

/// What the harness needs from the CLI context.
pub struct OneCtx<'a> {
    /// Home dir.
    pub home: &'a Path,
    /// Network.
    pub network: NetworkIdentity,
    /// Passphrase.
    pub passphrase: &'a str,
    /// KDF.
    pub kdf: KdfCost,
    /// REST gateway or empty for the relay.
    pub chain_api: String,
    /// Bootstrap.
    pub bootstrap: Vec<String>,
    /// JSON output.
    pub json: bool,
}

fn out<T: Serialize>(json: bool, v: &T, text: impl FnOnce() -> String) {
    if json {
        println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
    } else {
        println!("{}", text());
    }
}

async fn open(ctx: &OneCtx<'_>, require_peer: bool) -> anyhow::Result<HashgramOne> {
    let bootstrap: Vec<Multiaddr> = ctx
        .bootstrap
        .iter()
        .map(|a| a.parse().with_context(|| format!("bootstrap {a}")))
        .collect::<anyhow::Result<_>>()?;
    let cfg = Config {
        paths: Paths::new(ctx.home),
        network: ctx.network.clone(),
        bootstrap,
        chain_api: if ctx.chain_api.trim().is_empty() {
            None
        } else {
            Some(ctx.chain_api.clone())
        },
        kdf: ctx.kdf,
        connect_wait: Duration::from_secs(if require_peer { 10 } else { 1 }),
    };
    let one = HashgramOne::open(cfg, ctx.passphrase).await?;
    if require_peer && one.link.peers().await.is_empty() {
        anyhow::bail!("no node completed the Hashgram handshake within 10s");
    }
    Ok(one)
}

fn role(s: &str) -> anyhow::Result<app::SpaceRole> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "guest" => app::SpaceRole::Guest,
        "member" => app::SpaceRole::Member,
        "admin" => app::SpaceRole::Admin,
        "owner" => app::SpaceRole::Owner,
        other => anyhow::bail!("unknown role {other}"),
    })
}

fn resolve_entry(one: &mut HashgramOne, entry: &str) -> anyhow::Result<String> {
    if entry.starts_with('/') {
        one.drive()
            .resolve_path(entry)
            .with_context(|| format!("no Drive entry at {entry}"))
    } else if entry.is_empty() {
        Ok(String::new())
    } else {
        Ok(entry.to_owned())
    }
}

fn resolve_folder(one: &mut HashgramOne, folder_path: &str) -> anyhow::Result<String> {
    if folder_path == "/" || folder_path.is_empty() {
        return Ok(String::new());
    }
    resolve_entry(one, folder_path)
}

fn guess_mime(name: &str) -> &'static str {
    match name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "txt" | "md" => "text/plain",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "json" => "application/json",
        "mp4" => "video/mp4",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

/// Runs a `one` command.
pub async fn run(ctx: OneCtx<'_>, cmd: OneCmd) -> anyhow::Result<()> {
    let json = ctx.json;
    // Local-only commands (backup, folder listings) must work offline.
    let needs_peer = !matches!(cmd, OneCmd::Backup { .. });
    let mut one = open(&ctx, needs_peer).await?;
    let r = dispatch(&mut one, json, cmd).await;
    one.save()?;
    r
}

async fn dispatch(one: &mut HashgramOne, json: bool, cmd: OneCmd) -> anyhow::Result<()> {
    match cmd {
        OneCmd::Sync { watch } => loop {
            let rep = one.sync().round().await;
            match rep {
                Ok(r) => out(json, &r, || {
                    format!(
                        "sync ok: {} envelopes, {} mail, {} drive, {} people, {} circle, {} space, {} device-sync, {} unsupported, {} feed events{}",
                        r.envelopes, r.mail, r.drive, r.people, r.circles, r.spaces, r.device_sync, r.unsupported, r.feed,
                        r.balance_uhash.map(|b| format!(", balance {}", hashgram_sdk::wallet::format_hash(b))).unwrap_or_default()
                    )
                }),
                Err(e) => eprintln!("sync: {e}"),
            }
            one.save()?;
            match watch {
                Some(secs) => tokio::time::sleep(Duration::from_secs(secs.max(1))).await,
                None => return Ok(()),
            }
        },
        OneCmd::Backup { cmd } => match cmd {
            BackupCmd::Export { out: path, light } => {
                let pass = std::env::var("HASHGRAM_BACKUP_PASSPHRASE")
                    .context("set HASHGRAM_BACKUP_PASSPHRASE")?;
                let cost = if light { Some((8 * 1024, 1, 1)) } else { None };
                let meta = hashgram_sdk::backup::export_backup(
                    &one.account,
                    Path::new(&path),
                    &pass,
                    cost,
                )?;
                out(json, &meta, || {
                    format!("backup written to {path} (partial={})", meta.partial)
                });
                Ok(())
            }
            BackupCmd::Inspect { file } => {
                let bytes = std::fs::read(&file)?;
                let (m, t, p) = hashgram_sdk::backup::inspect_backup(&bytes)?;
                println!("argon2id m={m} KiB t={t} p={p}");
                Ok(())
            }
        },
        OneCmd::Balance => {
            let b = one.wallet().balance(None).await?;
            out(json, &b, || {
                format!(
                    "{}\n{}{}",
                    b.address,
                    b.display,
                    b.verification
                        .as_ref()
                        .map(|v| format!("\n{v}"))
                        .unwrap_or_default()
                )
            });
            Ok(())
        }
        OneCmd::Mail { cmd } => mail(one, json, cmd).await,
        OneCmd::Drive { cmd } => drive(one, json, cmd).await,
        OneCmd::People { cmd } => people(one, json, cmd).await,
        OneCmd::Feed { cmd } => feed(one, json, cmd).await,
        OneCmd::Circle { cmd } => circle(one, json, cmd).await,
        OneCmd::Space { cmd } => space(one, json, cmd).await,
        OneCmd::Devices { cmd } => devices(one, json, cmd).await,
        OneCmd::Provider { cmd } => provider(one, json, cmd).await,
        OneCmd::Network { cmd } => network(one, json, cmd).await,
    }
}

async fn mail(one: &mut HashgramOne, json: bool, cmd: MailCmd) -> anyhow::Result<()> {
    match cmd {
        MailCmd::Send {
            to,
            cc,
            bcc,
            subject,
            body,
            attach,
            attach_drive_live,
            attach_drive,
            reply_to,
            all,
            read_receipt,
        } => {
            let body = if body == "-" {
                let mut s = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
                s
            } else {
                body
            };
            let mut draft = match &reply_to {
                Some(id) => one.mail().reply_draft(id, all)?,
                None => Draft::default(),
            };
            if to.is_empty() && reply_to.is_none() && bcc.is_empty() {
                anyhow::bail!("--to is required unless replying");
            }
            if !to.is_empty() {
                draft.to = one.mail().resolve_recipients(&to).await?;
            }
            draft.cc = one.mail().resolve_recipients(&cc).await?;
            draft.bcc = one.mail().resolve_recipients(&bcc).await?;
            if !subject.is_empty() {
                draft.subject = subject;
            }
            draft.body_text = body;
            draft.request_read_receipt = read_receipt;
            for f in &attach {
                let bytes = std::fs::read(f).with_context(|| format!("reading {f}"))?;
                let name = Path::new(f)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| f.clone());
                draft.attachments.push(
                    one.mail()
                        .make_attachment(&name, guess_mime(&name), &bytes)
                        .await?,
                );
            }
            let recipients: Vec<String> = draft
                .to
                .iter()
                .chain(draft.cc.iter())
                .map(|a| a.address.clone())
                .collect();
            for e in &attach_drive {
                let id = resolve_entry(one, e)?;
                draft.attachments.push(
                    one.mail()
                        .attach_from_drive(&id, &recipients, false)
                        .await?,
                );
            }
            for e in &attach_drive_live {
                let id = resolve_entry(one, e)?;
                draft
                    .attachments
                    .push(one.mail().attach_from_drive(&id, &recipients, true).await?);
            }
            let id = one.mail().send(draft).await?;
            out(json, &serde_json::json!({ "message_id": id }), || {
                format!("sent {id}")
            });
        }
        MailCmd::List {
            folder: f,
            limit,
            threads,
        } => {
            let rows = if threads {
                one.mail().threads(&f, 0, limit)?
            } else {
                one.mail().list(&f, 0, limit)?
            };
            out(json, &rows, || {
                if rows.is_empty() {
                    return format!("{f}: empty");
                }
                rows.iter()
                    .map(|r| {
                        format!(
                            "{} {} {:<44} {:<40} {}{}",
                            if r.read { " " } else { "*" },
                            if r.starred { "★" } else { " " },
                            if r.from_username.is_empty() {
                                r.from.clone()
                            } else {
                                format!("@{}", r.from_username)
                            },
                            r.subject,
                            r.id,
                            if r.attachments > 0 {
                                format!(" [{} att]", r.attachments)
                            } else {
                                String::new()
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        MailCmd::Show { id } => {
            let rec = one.mail().get(&id)?.context("no such message")?;
            out(json, &rec, || {
                let m = &rec.message;
                let fmt = |a: &app::MailAddress| {
                    if a.username.is_empty() {
                        a.address.clone()
                    } else {
                        format!("@{} <{}>", a.username, a.address)
                    }
                };
                format!(
                    "From: {}\nTo: {}\nCc: {}\nSubject: {}\nDate: {}\nFolder: {}{}{}\nAttachments: {}\n\n{}",
                    m.from.as_ref().map(fmt).unwrap_or_default(),
                    m.to.iter().map(fmt).collect::<Vec<_>>().join(", "),
                    m.cc.iter().map(fmt).collect::<Vec<_>>().join(", "),
                    m.subject,
                    m.created_at_ms,
                    rec.folder,
                    if m.bcc_copy { " (bcc copy)" } else { "" },
                    if m.origin == app::MailOrigin::ExternalGateway as i32 { " [EXTERNAL — bridged by a gateway, not end-to-end encrypted]" } else { "" },
                    m.attachments.iter().enumerate().map(|(i, a)| format!("[{i}] {} ({}, {} bytes)", a.name, a.mime, a.size)).collect::<Vec<_>>().join(", "),
                    m.body_text
                )
            });
        }
        MailCmd::Thread { thread_id } => {
            let t = one.mail().thread(&thread_id)?.context("no such thread")?;
            out(json, &t, || {
                format!(
                    "{} ({} messages, {} unread)\n{}",
                    t.subject,
                    t.messages.len(),
                    t.unread,
                    t.messages
                        .iter()
                        .map(|m| format!(
                            "  {} {} — {}",
                            m.message.created_at_ms,
                            m.authenticated_sender,
                            m.message.body_text.chars().take(80).collect::<String>()
                        ))
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            });
        }
        MailCmd::Read { id, unread } => {
            one.mail().mark_read(&id, !unread).await?;
            println!("ok");
        }
        MailCmd::Star { id, off } => {
            one.mail().star(&id, !off)?;
            println!("ok");
        }
        MailCmd::Move { id, folder: f } => {
            one.mail().move_to(&id, &f)?;
            println!("ok");
        }
        MailCmd::Archive { id } => {
            one.mail().archive(&id)?;
            println!("ok");
        }
        MailCmd::Trash { id } => {
            one.mail().trash(&id)?;
            println!("ok");
        }
        MailCmd::Accept { id } => {
            one.mail().accept_request(&id)?;
            println!("moved to {}", folder::INBOX);
        }
        MailCmd::Label { id, label, remove } => {
            one.mail().set_label(&id, &label, !remove)?;
            println!("ok");
        }
        MailCmd::Search { query, limit } => {
            let rows = one.mail().search(&query, limit)?;
            out(json, &rows, || {
                rows.iter()
                    .map(|r| format!("{} {} {}", r.folder, r.subject, r.id))
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        MailCmd::Counts => {
            let c = one.mail().counts();
            out(json, &c, || {
                c.iter()
                    .map(|(f, n)| format!("{f:<9} {:>5} total {:>5} unread", n.total, n.unread))
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        MailCmd::Attachment {
            id,
            index,
            out: path,
        } => {
            let rec = one.mail().get(&id)?.context("no such message")?;
            let a = rec
                .message
                .attachments
                .get(index)
                .context("no such attachment")?
                .clone();
            let bytes = one.mail().attachment_bytes(&a).await?;
            std::fs::write(&path, &bytes)?;
            println!("wrote {} bytes to {path}", bytes.len());
        }
        MailCmd::Settings { read_receipts } => {
            let mut s = one.mail().settings().clone();
            if let Some(rr) = read_receipts {
                s.send_read_receipts = rr;
            }
            one.mail().set_settings(s.clone())?;
            out(json, &s, || format!("{s:?}"));
        }
    }
    Ok(())
}

async fn drive(one: &mut HashgramOne, json: bool, cmd: DriveCmd) -> anyhow::Result<()> {
    match cmd {
        DriveCmd::Ls { path } => {
            let parent = resolve_folder(one, &path)?;
            let rows = one.drive().list(&parent)?;
            out(json, &rows, || {
                if rows.is_empty() {
                    return "(empty)".into();
                }
                rows.iter()
                    .map(|e| {
                        format!(
                            "{:<6} {:>10} {} {}{}",
                            e.kind,
                            e.size,
                            e.id,
                            e.name,
                            if e.starred { " ★" } else { "" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        DriveCmd::Mkdir { path } => {
            let (parent, name) = path
                .trim_end_matches('/')
                .rsplit_once('/')
                .map(|(p, n)| (p.to_owned(), n.to_owned()))
                .unwrap_or((String::new(), path.clone()));
            let parent_id = resolve_folder(one, if parent.is_empty() { "/" } else { &parent })?;
            let id = one.drive().mkdir(&parent_id, &name)?;
            println!("{id}");
        }
        DriveCmd::Put {
            file,
            folder: f,
            mime,
            name,
        } => {
            let bytes = std::fs::read(&file).with_context(|| format!("reading {file}"))?;
            let name = name.unwrap_or_else(|| {
                Path::new(&file)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or(file.clone())
            });
            let mime = mime.unwrap_or_else(|| guess_mime(&name).to_owned());
            let parent = resolve_folder(one, &f)?;
            let id = one.drive().upload(&parent, &name, &mime, &bytes).await?;
            let rev = one.drive().commit().await?;
            out(
                json,
                &serde_json::json!({"entry_id": id, "revision": rev}),
                || format!("{id} (manifest revision {rev})"),
            );
        }
        DriveCmd::Get {
            entry,
            out: path,
            version,
        } => {
            let id = resolve_entry(one, &entry)?;
            let bytes = match version {
                Some(v) => one.drive().download_version(&id, v).await?,
                None => one.drive().download(&id).await?,
            };
            std::fs::write(&path, &bytes)?;
            println!("wrote {} bytes to {path}", bytes.len());
        }
        DriveCmd::Update { entry, file } => {
            let id = resolve_entry(one, &entry)?;
            let bytes = std::fs::read(&file)?;
            let v = one.drive().update(&id, &bytes, "").await?;
            one.drive().commit().await?;
            println!("version {v}");
        }
        DriveCmd::Rename { entry, name } => {
            let id = resolve_entry(one, &entry)?;
            one.drive().rename(&id, &name)?;
            one.drive().commit().await?;
            println!("ok");
        }
        DriveCmd::Mv { entry, folder: f } => {
            let id = resolve_entry(one, &entry)?;
            let p = resolve_folder(one, &f)?;
            one.drive().mv(&id, &p)?;
            one.drive().commit().await?;
            println!("ok");
        }
        DriveCmd::Rm { entry } => {
            let id = resolve_entry(one, &entry)?;
            one.drive().trash(&id)?;
            one.drive().commit().await?;
            println!("trashed");
        }
        DriveCmd::Restore { entry } => {
            one.drive().restore(&entry)?;
            one.drive().commit().await?;
            println!("ok");
        }
        DriveCmd::Purge { entry } => {
            let n = one.drive().delete(&entry)?;
            one.drive().commit().await?;
            println!("deleted {n} entries");
        }
        DriveCmd::Trash => {
            let rows = one.drive().trash_list();
            out(json, &rows, || {
                rows.iter()
                    .map(|e| format!("{:<6} {} {}", e.kind, e.id, e.path))
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        DriveCmd::EmptyTrash => {
            let n = one.drive().empty_trash()?;
            one.drive().commit().await?;
            println!("deleted {n} entries");
        }
        DriveCmd::Versions { entry } => {
            let id = resolve_entry(one, &entry)?;
            let v = one.drive().versions(&id)?;
            out(json, &v, || {
                v.iter()
                    .map(|x| {
                        format!(
                            "v{} {} bytes {} {}",
                            x.version_no,
                            x.object.as_ref().map(|o| o.size).unwrap_or(0),
                            x.created_at_ms,
                            x.note
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        DriveCmd::RestoreVersion { entry, version } => {
            let id = resolve_entry(one, &entry)?;
            one.drive().restore_version(&id, version).await?;
            one.drive().commit().await?;
            println!("ok");
        }
        DriveCmd::Rekey { entry } => {
            let id = resolve_entry(one, &entry)?;
            let v = one.drive().rekey(&id).await?;
            one.drive().commit().await?;
            println!("rekeyed as version {v}");
        }
        DriveCmd::Star { entry, off } => {
            let id = resolve_entry(one, &entry)?;
            one.drive().star(&id, !off)?;
            println!("ok");
        }
        DriveCmd::Share {
            entry,
            grantees,
            live,
            note,
        } => {
            let id = resolve_entry(one, &entry)?;
            let mode = if live {
                app::DriveShareMode::Live
            } else {
                app::DriveShareMode::Snapshot
            };
            let cap = one
                .drive()
                .share(&id, &grantees, mode, app::DrivePermission::Read, &note)
                .await?;
            one.drive().commit().await?;
            out(json, &cap, || {
                format!("share {} ({:?}) sent", hex::encode(&cap.share_id), mode)
            });
        }
        DriveCmd::Revoke { share_id } => {
            one.drive().revoke(&share_id).await?;
            one.drive().commit().await?;
            println!("revoked");
        }
        DriveCmd::Shares => {
            let s = one.drive().shares();
            out(json, &s, || {
                s.iter()
                    .map(|r| {
                        format!(
                            "{} entry {} → {} {}{}",
                            hex::encode(&r.share_id),
                            hex::encode(&r.entry_id),
                            r.grantee,
                            if r.mode == app::DriveShareMode::Live as i32 {
                                "live"
                            } else {
                                "snapshot"
                            },
                            if r.revoked { " (revoked)" } else { "" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        DriveCmd::SharedWithMe => {
            let s = one.drive().shared_with_me()?;
            out(json, &s, || {
                s.iter()
                    .map(|r| {
                        format!(
                            "{} from {} {} ({} bytes, v{}){}{}",
                            hex::encode(&r.capability.share_id),
                            r.from,
                            r.capability.name,
                            r.capability.size,
                            r.capability.version_no,
                            if r.capability.folder { " [folder]" } else { "" },
                            if r.revoked { " (revoked)" } else { "" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        DriveCmd::GetShared {
            share_id,
            out: path,
        } => {
            let s = one.drive().shared_with_me()?;
            let rec = s
                .into_iter()
                .find(|r| hex::encode(&r.capability.share_id) == share_id)
                .context("no such share")?;
            let bytes = one.drive().download_capability(&rec.capability).await?;
            std::fs::write(&path, &bytes)?;
            println!("wrote {} bytes to {path}", bytes.len());
        }
        DriveCmd::SaveShared {
            share_id,
            folder: f,
        } => {
            let s = one.drive().shared_with_me()?;
            let rec = s
                .into_iter()
                .find(|r| hex::encode(&r.capability.share_id) == share_id)
                .context("no such share")?;
            let parent = resolve_folder(one, &f)?;
            let id = one.drive().save_capability(&rec.capability, &parent)?;
            one.drive().commit().await?;
            println!("{id}");
        }
        DriveCmd::Commit => {
            let rev = one.drive().commit().await?;
            println!("revision {rev}");
        }
        DriveCmd::Usage => {
            let u = one.drive().usage();
            out(json, &u, || format!("{u:?}"));
        }
        DriveCmd::Search { query } => {
            let rows = one.drive().search(&query, 50);
            out(json, &rows, || {
                rows.iter()
                    .map(|e| format!("{:<6} {} {}", e.kind, e.id, e.path))
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
    }
    Ok(())
}

async fn people(one: &mut HashgramOne, json: bool, cmd: PeopleCmd) -> anyhow::Result<()> {
    match cmd {
        PeopleCmd::Resolve { input } => {
            let r = one.people().resolve(&input).await?;
            out(json, &r, || format!("{r:#?}"));
        }
        PeopleCmd::Profile { address } => {
            let p = one.people().profile(&address).await?;
            out(json, &p, || format!("{p:#?}"));
        }
        PeopleCmd::Request { input, message } => {
            let r = one.people().request(&input, &message).await?;
            println!("request sent to {}", r.address);
        }
        PeopleCmd::Accept { address } => {
            one.people().respond(&address, true).await?;
            println!("accepted");
        }
        PeopleCmd::Reject { address } => {
            one.people().respond(&address, false).await?;
            println!("rejected");
        }
        PeopleCmd::Remove { address } => {
            one.people().remove(&address)?;
            println!("removed");
        }
        PeopleCmd::Block { address, off } => {
            if off {
                one.people().unblock(&address)?;
            } else {
                one.people().block(&address)?;
            }
            println!("ok");
        }
        PeopleCmd::Mute { address, off } => {
            one.people().mute(&address, !off)?;
            println!("ok");
        }
        PeopleCmd::Trust { address, off } => {
            one.people().trust(&address, !off)?;
            println!("ok");
        }
        PeopleCmd::Follow { address, off } => {
            one.people().follow(&address, !off).await?;
            println!("ok");
        }
        PeopleCmd::List { which } => {
            let rows = match which.as_str() {
                "friends" => one.people().friends(),
                "incoming" => one.people().incoming_requests(),
                "outgoing" => one.people().outgoing_requests(),
                "blocked" => one.people().blocked(),
                _ => one.people().all(),
            };
            out(json, &rows, || {
                rows.iter()
                    .map(|r| {
                        format!(
                            "{} {} {} [{}]",
                            r.address,
                            r.username,
                            r.display_name,
                            r.states.join(",")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        PeopleCmd::Name { name } => {
            one.people().set_my_display_name(&name)?;
            println!("ok");
        }
        PeopleCmd::Card {
            address,
            bio,
            disclose_wallet,
        } => {
            one.people()
                .send_card(&address, &bio, disclose_wallet)
                .await?;
            println!("card sent");
        }
    }
    Ok(())
}

async fn feed(one: &mut HashgramOne, json: bool, cmd: FeedCmd) -> anyhow::Result<()> {
    match cmd {
        FeedCmd::Post { text, tags } => {
            let id = one.feed().post(&text, tags, vec![], false).await?;
            println!("{id}");
        }
        FeedCmd::Comment { post, text } => println!("{}", one.feed().comment(&post, &text).await?),
        FeedCmd::React { target, reaction } => {
            println!("{}", one.feed().react(&target, &reaction).await?)
        }
        FeedCmd::Repost { post, comment } => {
            println!("{}", one.feed().repost(&post, &comment).await?)
        }
        FeedCmd::Following { limit } => {
            one.feed().refresh().await?;
            let rows = one.feed().following(0, limit)?;
            out(json, &rows, || {
                rows.iter()
                    .map(|i| {
                        format!(
                            "{} {} {} {}",
                            i.timestamp,
                            i.author,
                            i.kind,
                            i.payload.get("text").and_then(|t| t.as_str()).unwrap_or("")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        FeedCmd::Friends { limit } => {
            one.feed().refresh().await?;
            let rows = one.feed().friends(0, limit)?;
            out(json, &rows, || {
                rows.iter()
                    .map(|i| {
                        format!(
                            "{} {} {}",
                            i.timestamp,
                            i.author,
                            i.payload.get("text").and_then(|t| t.as_str()).unwrap_or("")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        FeedCmd::Author { address, limit } => {
            one.feed().refresh_author(&address, 100).await?;
            let rows = one.feed().author(&address, 0, limit)?;
            out(json, &rows, || {
                rows.iter()
                    .map(|i| {
                        format!(
                            "{} {} {} {}",
                            i.timestamp,
                            i.kind,
                            i.id,
                            i.payload.get("text").and_then(|t| t.as_str()).unwrap_or("")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        FeedCmd::Thread { post } => {
            let t = one
                .feed()
                .thread(&post)?
                .context("post not cached; refresh its author first")?;
            out(json, &t, || format!("{t:#?}"));
        }
        FeedCmd::Refresh => println!("{} new events", one.feed().refresh().await?),
        FeedCmd::Profile { name, bio } => {
            println!("{}", one.feed().update_profile(&name, &bio, "").await?)
        }
    }
    Ok(())
}

async fn circle(one: &mut HashgramOne, json: bool, cmd: CircleCmd) -> anyhow::Result<()> {
    match cmd {
        CircleCmd::Create {
            name,
            description,
            members,
        } => println!(
            "{}",
            one.circles().create(&name, &description, &members).await?
        ),
        CircleCmd::List => {
            let rows = one.circles().list()?;
            out(json, &rows, || {
                rows.iter()
                    .map(|c| format!("{} {} ({} members)", c.id, c.name, c.members.len()))
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        CircleCmd::Add { circle, address } => {
            one.circles().add_member(&circle, &address).await?;
            println!("ok");
        }
        CircleCmd::Remove { circle, address } => println!(
            "removed {} devices",
            one.circles().remove_member(&circle, &address).await?
        ),
        CircleCmd::Leave { circle } => {
            one.circles().leave(&circle).await?;
            println!("left");
        }
        CircleCmd::Post { circle, text } => println!(
            "{}",
            one.circles()
                .post(&circle, &text, vec![], vec![], None)
                .await?
        ),
        CircleCmd::Poll {
            circle,
            question,
            options,
        } => {
            let poll = app::Poll {
                question,
                options,
                multiple_choice: false,
                closes_at_ms: 0,
            };
            println!(
                "{}",
                one.circles()
                    .post(&circle, "", vec![], vec![], Some(poll))
                    .await?
            )
        }
        CircleCmd::Comment { circle, post, text } => {
            println!("{}", one.circles().comment(&circle, &post, &text).await?)
        }
        CircleCmd::React {
            circle,
            target,
            reaction,
        } => println!(
            "{}",
            one.circles().react(&circle, &target, &reaction).await?
        ),
        CircleCmd::Vote {
            circle,
            post,
            choices,
        } => println!("{}", one.circles().vote(&circle, &post, choices).await?),
        CircleCmd::Posts { circle, limit } => {
            let rows = one.circles().posts(&circle, 0, limit)?;
            out(json, &rows, || {
                rows.iter()
                    .map(|i| {
                        format!(
                            "{} {} {} {}{}",
                            i.at_ms,
                            i.author,
                            i.id,
                            i.text,
                            i.poll
                                .as_ref()
                                .map(|p| format!(" [poll: {} votes {:?}]", p.question, i.votes))
                                .unwrap_or_default()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        CircleCmd::Comments { circle, post } => {
            let rows = one.circles().comments(&circle, &post)?;
            out(json, &rows, || {
                rows.iter()
                    .map(|i| format!("{} {} {}", i.at_ms, i.author, i.text))
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
    }
    Ok(())
}

async fn space(one: &mut HashgramOne, json: bool, cmd: SpaceCmd) -> anyhow::Result<()> {
    match cmd {
        SpaceCmd::Create { name, description } => {
            println!("{}", one.spaces().create(&name, &description).await?)
        }
        SpaceCmd::List => {
            let rows = one.spaces().list()?;
            out(json, &rows, || {
                rows.iter()
                    .map(|s| {
                        format!(
                            "{} {} role={} members={}",
                            s.id, s.name, s.my_role, s.members
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        SpaceCmd::Show { space: s } => {
            let st = one.spaces().state(&s)?;
            out(json, &st, || {
                format!(
                    "{} — {}\nmembers: {:?}\ncontent: {} items\ndrive: {} entries",
                    st.name,
                    st.description,
                    st.members_sorted()
                        .iter()
                        .map(|m| format!("{}:{}", m.address, m.role))
                        .collect::<Vec<_>>(),
                    st.content.len(),
                    st.drive.len()
                )
            });
        }
        SpaceCmd::Invite {
            space: s,
            address,
            role: r,
        } => {
            one.spaces().invite(&s, &address, role(&r)?).await?;
            println!("invited");
        }
        SpaceCmd::Remove {
            space: s,
            address,
            reason,
        } => {
            one.spaces().remove(&s, &address, &reason).await?;
            println!("removed");
        }
        SpaceCmd::Role {
            space: s,
            address,
            role: r,
        } => println!("{}", one.spaces().set_role(&s, &address, role(&r)?).await?),
        SpaceCmd::Announce {
            space: s,
            title,
            text,
        } => println!(
            "{}",
            one.spaces().announce(&s, &title, &text, vec![]).await?
        ),
        SpaceCmd::Post { space: s, text } => {
            println!("{}", one.spaces().post(&s, &text, vec![], vec![]).await?)
        }
        SpaceCmd::Comment {
            space: s,
            post,
            text,
        } => println!("{}", one.spaces().comment(&s, &post, &text).await?),
        SpaceCmd::Content { space: s, limit } => {
            let rows = one.spaces().content(&s, 0, limit)?;
            out(json, &rows, || {
                rows.iter()
                    .map(|c| format!("{} {} {} {}{}", c.at_ms, c.kind, c.actor, c.title, c.text))
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        SpaceCmd::Members { space: s } => {
            let rows = one.spaces().members(&s)?;
            out(json, &rows, || {
                rows.iter()
                    .map(|m| format!("{} role={}", m.address, m.role))
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        SpaceCmd::ShareDrive {
            space: s,
            entry,
            path,
            snapshot,
        } => {
            let id = resolve_entry(one, &entry)?;
            println!(
                "{}",
                one.spaces().share_drive(&s, &id, &path, !snapshot).await?
            );
        }
        SpaceCmd::Drive { space: s } => {
            let rows = one.spaces().drive_entries(&s)?;
            out(json, &rows, || {
                rows.iter()
                    .map(|e| {
                        format!(
                            "{} {}/{} ({} bytes) by {}",
                            hex::encode(&e.capability.share_id),
                            e.path,
                            e.capability.name,
                            e.capability.size,
                            e.by
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        SpaceCmd::Mail {
            space: s,
            subject,
            body,
        } => println!("{}", one.spaces().mail(&s, &subject, &body).await?),
        SpaceCmd::Info {
            space: s,
            name,
            description,
        } => println!("{}", one.spaces().set_info(&s, &name, &description).await?),
    }
    Ok(())
}

async fn devices(one: &mut HashgramOne, json: bool, cmd: DevicesCmd) -> anyhow::Result<()> {
    match cmd {
        DevicesCmd::List => {
            let d = one.devices().list().await?;
            out(json, &d, || {
                d.iter()
                    .map(|x| {
                        format!(
                            "{} {} {} {}{}",
                            x.device_id,
                            x.device_pubkey,
                            x.label,
                            x.platform,
                            if x.revoked { " (revoked)" } else { "" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        DevicesCmd::This => {
            let (id, pk) = one.devices().this_device()?;
            println!("{id} {pk}");
        }
        DevicesCmd::Reconcile => {
            let r = one.devices().reconcile().await?;
            out(json, &r, || format!("{r:#?}"));
        }
        DevicesCmd::Bootstrap => println!(
            "{}",
            if one.devices().bootstrap_new_device().await? {
                "sent to self group"
            } else {
                "single device; nothing to sync"
            }
        ),
    }
    Ok(())
}

async fn provider(one: &mut HashgramOne, json: bool, cmd: ProviderCmd) -> anyhow::Result<()> {
    match cmd {
        ProviderCmd::Status { operator } => {
            let s = one.provider().status(operator.as_deref()).await?;
            out(json, &s, || {
                format!(
                    "{} {:?} roles={:?} bond={} uhash fraud={}",
                    s.operator, s.lifecycle, s.roles, s.bond_uhash, s.fraud_score
                )
            });
        }
        ProviderCmd::Earnings { operator } => {
            let e = one.provider().earnings(operator.as_deref()).await?;
            out(json, &e, || {
                format!(
                    "paid {} uhash, pending credit {}, epoch {}, reserve {}",
                    e.total_paid_uhash, e.pending_credit, e.epoch, e.reserve_remaining_uhash
                )
            });
        }
        ProviderCmd::List => {
            let l = one.provider().list().await?;
            out(json, &l, || {
                l.iter()
                    .map(|s| format!("{} {:?} {:?}", s.operator, s.lifecycle, s.roles))
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        ProviderCmd::Register {
            reward_address,
            node_pubkey,
            roles,
            bond_uhash,
            storage_bytes,
            moniker,
        } => {
            let r: Vec<&str> = roles.iter().map(String::as_str).collect();
            println!(
                "{}",
                one.provider()
                    .register(
                        &reward_address,
                        &node_pubkey,
                        &r,
                        bond_uhash,
                        storage_bytes,
                        &moniker
                    )
                    .await?
            );
        }
        ProviderCmd::Unbond => println!("{}", one.provider().unbond().await?),
        ProviderCmd::Withdraw => println!("{}", one.provider().withdraw().await?),
    }
    Ok(())
}

async fn network(one: &mut HashgramOne, json: bool, cmd: NetworkCmd) -> anyhow::Result<()> {
    match cmd {
        NetworkCmd::Overview => {
            let o = one.network_api().overview().await?;
            out(json, &o, || {
                format!(
                    "{} / {} genesis {}\nheight {:?} {}\npeers:\n{}",
                    o.network_id,
                    o.chain_id,
                    o.genesis_hash,
                    o.height,
                    o.verification.clone().unwrap_or_default(),
                    o.peers
                        .iter()
                        .map(|p| format!("  {} {:?} {}", p.peer, p.roles, p.operator))
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            });
        }
        NetworkCmd::Validators => {
            let v = one.network_api().validators().await?;
            out(json, &v, || {
                v.iter()
                    .map(|x| {
                        format!(
                            "{} {} {}",
                            x.get("operator_address")
                                .and_then(|s| s.as_str())
                                .unwrap_or(""),
                            x.get("description")
                                .and_then(|d| d.get("moniker"))
                                .and_then(|s| s.as_str())
                                .unwrap_or(""),
                            x.get("tokens").and_then(|s| s.as_str()).unwrap_or("")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        NetworkCmd::Supply => {
            let s = one.network_api().supply().await?;
            println!("{}", serde_json::to_string_pretty(&s)?);
        }
        NetworkCmd::Top {
            what,
            indexer,
            limit,
        } => {
            let v = match what.as_str() {
                "holders" => one.network_api().top_holders(Some(&indexer), limit).await?,
                "validators" => {
                    one.network_api()
                        .top_validators(Some(&indexer), limit)
                        .await?
                }
                "providers" => {
                    one.network_api()
                        .top_providers(Some(&indexer), limit)
                        .await?
                }
                "stats" => one.network_api().stats(Some(&indexer)).await?,
                other => anyhow::bail!("unknown leaderboard {other}"),
            };
            println!("{}", serde_json::to_string_pretty(&v)?);
        }
    }
    Ok(())
}
