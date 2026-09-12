//! The bridge: where Internet mail becomes HashMail and back.
//!
//! This module is split into a *pure* half and a *runtime* half so the
//! mapping logic is tested without a network:
//!
//! * pure: [`ThreadIds`] resolution against the `thread_map`,
//!   [`build_inbound_message`] (parsed MIME + resolved recipients →
//!   `MailMessage` with `origin = EXTERNAL_GATEWAY`),
//!   [`external_recipients`] (the `ext-to:` / `ext-cc:` label convention),
//!   [`build_outbound`] (native record → [`OutboundMail`]), backoff.
//! * runtime: [`GatewayHandler`] (the SMTP server's application),
//!   [`Bridge::run`] (sync rounds, both retry queues, the inbox scan).
//!
//! # The `ext-to:` convention (v1)
//!
//! A Hashgram user has no way to put `bob@example.com` into a native
//! `MailAddress` — only chain identities fit there. To reach an Internet
//! address the client sends an ordinary native message *to the bridge
//! identity* and adds one label per external recipient:
//! `ext-to:bob@example.com` (To) or `ext-cc:carol@example.org` (Cc).
//! The gateway relays it as `From: <username>@<domain>`. Nothing else in
//! the message is special; the desktop composer applies this rule when the
//! typed recipient parses as `AddressForm::External`.
//!
//! # What the gateway holds in plaintext
//!
//! Exactly the messages that cross it: inbound Internet mail (plaintext
//! on the wire or TLS-terminated upstream) and outbound mail the user
//! deliberately addressed to the bridge. Native mail between Hashgram
//! identities is end-to-end encrypted in MLS groups the bridge is not a
//! member of; it never sees it.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use hashgram_app::mail::{self as m, MAX_NAME, MAX_REFERENCES};
use hashgram_app::pb as app;
use hashgram_sdk::mail::MailRecord;
use hashgram_sdk::HashgramOne;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, warn};

use crate::address::{classify, Mailbox as Mbx};
use crate::config::GatewayConfig;
use crate::dkim;
use crate::dns::{MxAnswer, MxResolver};
use crate::metrics::{Metrics, QueueLabels};
use crate::mime::inbound::{self, InboundMail};
use crate::mime::outbound::{self as out, OutboundMail};
use crate::policy::{self, Decision};
use crate::ratelimit::IpLimiter;
use crate::smtp::client::{self, ClientConfig, DeliveryError};
use crate::smtp::server::{Envelope, InboundHandler, Verdict};
use crate::store::{self as st, QueueItem, Store, ThreadRow};
use crate::thread;
use crate::GatewayError;

/// Label prefix for an external To recipient.
pub const EXT_TO: &str = "ext-to:";
/// Label prefix for an external Cc recipient.
pub const EXT_CC: &str = "ext-cc:";
/// Label the gateway adds to inbound messages so clients can filter.
pub const LABEL_EXTERNAL: &str = "external";

// ---------------------------------------------------------------------------
// Pure half
// ---------------------------------------------------------------------------

/// Threading ids for one inbound message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadIds {
    /// 16 bytes.
    pub message_id: Vec<u8>,
    /// 16 bytes.
    pub thread_id: Vec<u8>,
    /// 16 bytes or empty.
    pub in_reply_to: Vec<u8>,
    /// Ancestors, oldest first.
    pub references: Vec<Vec<u8>>,
    /// The `Message-ID` header value recorded (synthesised when absent).
    pub message_id_header: String,
}

/// The id a header value maps to: a remembered pairing first (ids we
/// generated for outbound mail cannot be re-derived), then our own
/// `<hex@domain>` form, then the deterministic derivation.
pub fn id_for_header(
    store: &Store,
    header: &str,
    domain: &str,
) -> Result<Option<Vec<u8>>, GatewayError> {
    let n = thread::normalise_message_id(header);
    if n.is_empty() {
        return Ok(None);
    }
    if let Some(row) = store.thread_by_header(&n)? {
        return Ok(hex::decode(row.hashgram_id).ok());
    }
    if let Some(id) = thread::parse_outbound_message_id(&n, domain) {
        return Ok(Some(id));
    }
    Ok(thread::derive_id(&n))
}

/// Resolves message/thread ids for a parsed inbound message and records
/// the pairing.
pub fn resolve_thread_ids(
    store: &Store,
    parsed: &InboundMail,
    domain: &str,
    now_secs: u64,
) -> Result<ThreadIds, GatewayError> {
    let (message_id, header) = match &parsed.message_id {
        Some(h) => (
            id_for_header(store, h, domain)?
                .ok_or_else(|| GatewayError::Mime("empty Message-ID".into()))?,
            h.clone(),
        ),
        None => {
            // No Message-ID: mint one so the pairing exists for replies.
            let id = hashgram_app::ids::random_id()?;
            let h = thread::normalise_message_id(&thread::outbound_message_id(&id, domain));
            (id, h)
        }
    };
    let in_reply_to = match &parsed.in_reply_to {
        Some(h) => id_for_header(store, h, domain)?.unwrap_or_default(),
        None => Vec::new(),
    };
    let mut references = Vec::new();
    for r in parsed.references.iter().take(MAX_REFERENCES) {
        if let Some(id) = id_for_header(store, r, domain)? {
            if !references.contains(&id) {
                references.push(id);
            }
        }
    }
    // RFC 5322 §3.6.4: the parent is the last reference; make sure it is
    // there so readers walking `references` find it.
    if !in_reply_to.is_empty() && !references.contains(&in_reply_to) {
        references.push(in_reply_to.clone());
        while references.len() > MAX_REFERENCES {
            references.remove(1);
        }
    }
    // Thread: the parent's thread if we know it, else the root reference,
    // else this message starts one.
    let mut thread_id: Option<Vec<u8>> = None;
    if !in_reply_to.is_empty() {
        if let Some(row) = store.thread_by_hashgram_id(&hex::encode(&in_reply_to))? {
            thread_id = hex::decode(row.thread_id).ok();
        }
    }
    if thread_id.is_none() {
        if let Some(root) = references.first() {
            if let Some(row) = store.thread_by_hashgram_id(&hex::encode(root))? {
                thread_id = hex::decode(row.thread_id).ok();
            } else {
                thread_id = Some(root.clone());
            }
        }
    }
    let thread_id = thread_id.unwrap_or_else(|| message_id.clone());
    store.map_thread(
        &ThreadRow {
            hashgram_id: hex::encode(&message_id),
            message_id_header: header.clone(),
            thread_id: hex::encode(&thread_id),
        },
        now_secs,
    )?;
    Ok(ThreadIds {
        message_id,
        thread_id,
        in_reply_to,
        references,
        message_id_header: header,
    })
}

/// Resolved recipients for one inbound message.
#[derive(Debug, Clone, Default)]
pub struct InboundRecipients {
    /// Envelope recipients that appear in the `To` header (or nowhere).
    pub to: Vec<app::MailAddress>,
    /// Envelope recipients that appear only in `Cc`.
    pub cc: Vec<app::MailAddress>,
}

/// Places each resolved envelope recipient on the To or Cc line based on
/// where the sender listed it. Recipients absent from both headers were
/// BCC'd by the sender; they go on To so the message validates, and the
/// reader can see they were not in the visible list because the original
/// header addresses are not theirs.
#[must_use]
pub fn place_recipients(
    parsed: &InboundMail,
    resolved: Vec<(String, app::MailAddress)>,
) -> InboundRecipients {
    let mut out = InboundRecipients::default();
    for (rcpt, addr) in resolved {
        let in_to = parsed.to.iter().any(|m| m.addr.eq_ignore_ascii_case(&rcpt));
        let in_cc = parsed.cc.iter().any(|m| m.addr.eq_ignore_ascii_case(&rcpt));
        if in_cc && !in_to {
            out.cc.push(addr);
        } else {
            out.to.push(addr);
        }
    }
    if out.to.is_empty() && !out.cc.is_empty() {
        out.to = std::mem::take(&mut out.cc);
    }
    out
}

fn bound_str(s: &str, max: usize) -> String {
    let mut o = String::new();
    for ch in s.chars() {
        if o.len() + ch.len_utf8() > max {
            break;
        }
        o.push(ch);
    }
    o
}

/// Builds the `MailMessage` for an inbound message. `from` is the bridge
/// identity's own address (MLS authenticates it as the sender); its
/// `display_name` is overwritten with the external sender so readers see
/// who really wrote it, alongside the mandatory `EXTERNAL_GATEWAY` origin.
pub fn build_inbound_message(
    parsed: &InboundMail,
    mut from: app::MailAddress,
    recipients: InboundRecipients,
    attachments: Vec<app::MailAttachment>,
    ids: &ThreadIds,
    received_at_ms: u64,
) -> Result<app::MailMessage, GatewayError> {
    let gateway = from.address.clone();
    from.display_name = bound_str(
        &match &parsed.from {
            Some(mb) if !mb.name.is_empty() => format!("{} <{}>", mb.name, mb.addr),
            Some(mb) => mb.addr.clone(),
            None => parsed.from_header.clone(),
        },
        MAX_NAME,
    );
    let draft = m::Draft {
        from,
        to: recipients.to,
        cc: recipients.cc,
        subject: parsed.subject.clone(),
        body_text: parsed.body_text.clone(),
        body_html: parsed.body_html.clone(),
        attachments,
        importance: parsed.importance,
        labels: vec![LABEL_EXTERNAL.to_owned()],
        ..m::Draft::default()
    };
    let built = draft.build_all()?;
    let mut msg = built
        .main
        .ok_or_else(|| GatewayError::Mime("no recipients after resolution".into()))?;
    msg.message_id = ids.message_id.clone();
    msg.thread_id = ids.thread_id.clone();
    msg.in_reply_to = ids.in_reply_to.clone();
    msg.references = ids.references.clone();
    msg.created_at_ms = parsed
        .date_ms
        .filter(|d| *d <= received_at_ms + 300_000)
        .unwrap_or(received_at_ms);
    msg.origin = app::MailOrigin::ExternalGateway as i32;
    msg.external = Some(app::ExternalMailMeta {
        gateway,
        from_header: parsed.from_header.clone(),
        message_id_header: bound_str(&ids.message_id_header, m::MAX_HEADER),
        auth_results: parsed.auth.as_strings(),
        spam_score: parsed.spam_score.min(1000),
    });
    m::validate(&msg)?;
    Ok(msg)
}

/// External recipients from a native message's labels, (To, Cc), each
/// validated as a remote mailbox under some other domain. Local addresses
/// (`x@<our domain>`) are refused: native mail is the right path.
pub fn external_recipients(
    labels: &[String],
    our_domain: &str,
) -> Result<(Vec<String>, Vec<String>), GatewayError> {
    let mut to = Vec::new();
    let mut cc = Vec::new();
    for l in labels {
        let (target, addr) = if let Some(a) = l.strip_prefix(EXT_TO) {
            (&mut to, a)
        } else if let Some(a) = l.strip_prefix(EXT_CC) {
            (&mut cc, a)
        } else {
            continue;
        };
        match classify(addr, our_domain)? {
            Mbx::Remote { local, domain } => {
                let full = format!("{local}@{domain}");
                if !target.iter().any(|t: &String| t.eq_ignore_ascii_case(&full)) {
                    target.push(full);
                }
            }
            Mbx::Local(u) => {
                return Err(GatewayError::Address(format!(
                    "{u}@{our_domain} is a Hashgram user; send native mail instead of relaying through the gateway"
                )))
            }
        }
    }
    Ok((to, cc))
}

/// The `Message-ID` header value (with brackets) for a native id: the
/// remembered pairing if the id came from the Internet, else our form.
pub fn header_for_id(store: &Store, id: &[u8], domain: &str) -> Result<String, GatewayError> {
    if id.is_empty() {
        return Ok(String::new());
    }
    if let Some(row) = store.thread_by_hashgram_id(&hex::encode(id))? {
        return Ok(format!("<{}>", row.message_id_header));
    }
    Ok(thread::outbound_message_id(id, domain))
}

/// Builds the Internet message for a native record addressed to the
/// bridge with `ext-to:` labels. `attachments` are the decrypted bytes in
/// the record's attachment order.
pub fn build_outbound(
    store: &Store,
    record: &MailRecord,
    sender_username: &str,
    domain: &str,
    attachments: Vec<out::Attachment>,
    now_secs: u64,
) -> Result<(OutboundMail, Vec<String>), GatewayError> {
    let msg = &record.message;
    let (to, cc) = external_recipients(&msg.labels, domain)?;
    if to.is_empty() && cc.is_empty() {
        return Err(GatewayError::Address("no ext-to: / ext-cc: labels".into()));
    }
    let message_id = thread::outbound_message_id(&msg.message_id, domain);
    store.map_thread(
        &ThreadRow {
            hashgram_id: hex::encode(&msg.message_id),
            message_id_header: thread::normalise_message_id(&message_id),
            thread_id: hex::encode(&msg.thread_id),
        },
        now_secs,
    )?;
    let in_reply_to = if msg.in_reply_to.is_empty() {
        None
    } else {
        Some(header_for_id(store, &msg.in_reply_to, domain)?)
    };
    let mut references = Vec::new();
    for r in &msg.references {
        let h = header_for_id(store, r, domain)?;
        if !h.is_empty() && !references.contains(&h) {
            references.push(h);
        }
    }
    let display_name = msg
        .from
        .as_ref()
        .map(|a| a.display_name.clone())
        .unwrap_or_default();
    let mailbox = |addr: &str| out::Mailbox {
        name: String::new(),
        addr: addr.to_owned(),
    };
    let all_rcpts: Vec<String> = to.iter().chain(cc.iter()).cloned().collect();
    let mail = OutboundMail {
        from: out::Mailbox {
            name: display_name,
            addr: crate::address::mail_address(sender_username, domain),
        },
        to: to.iter().map(|a| mailbox(a)).collect(),
        cc: cc.iter().map(|a| mailbox(a)).collect(),
        reply_to: None,
        subject: msg.subject.clone(),
        date_ms: if msg.created_at_ms == 0 {
            now_secs * 1000
        } else {
            msg.created_at_ms
        },
        message_id,
        in_reply_to,
        references,
        body_text: msg.body_text.clone(),
        body_html: msg.body_html.clone(),
        attachments,
        importance: app::MailImportance::try_from(msg.importance)
            .unwrap_or(app::MailImportance::Normal),
        extra_headers: vec![("X-Hashgram-Origin".into(), "native".into())],
    };
    Ok((mail, all_rcpts))
}

/// Retry delay after `attempts` failures: 30 s, 1 m, 2 m, … capped at 6 h.
#[must_use]
pub fn backoff_secs(attempts: u32) -> u64 {
    let base: u64 = 30;
    base.saturating_mul(1u64 << attempts.min(12)).min(6 * 3600)
}

/// A queued inbound message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundJob {
    /// SMTP envelope.
    pub envelope: Envelope,
    /// Raw message bytes.
    pub raw: Vec<u8>,
    /// Unix ms when accepted.
    pub received_at_ms: u64,
    /// Hex message id assigned at accept time.
    pub hashgram_message_id: String,
}

/// A queued outbound message for one recipient domain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboundJob {
    /// Hex native message id.
    pub hashgram_message_id: String,
    /// Authenticated Hashgram sender (for bounces).
    pub sender_address: String,
    /// `MAIL FROM`.
    pub mail_from: String,
    /// Recipient domain.
    pub domain: String,
    /// `RCPT TO` list under that domain.
    pub rcpts: Vec<String>,
    /// Signed RFC 5322 bytes.
    pub message: Vec<u8>,
    /// Subject (for the bounce).
    pub subject: String,
}

// ---------------------------------------------------------------------------
// Runtime half
// ---------------------------------------------------------------------------

/// A cached username → identity lookup.
#[derive(Debug, Clone)]
struct CachedResolve {
    result: Option<app::MailAddress>,
    at: Instant,
}

/// Shared runtime state.
pub struct Shared {
    /// Config.
    pub cfg: GatewayConfig,
    /// Store.
    pub store: Arc<Store>,
    /// Metrics.
    pub metrics: Arc<Metrics>,
    /// The bridge identity. One lock: the SDK facade is `&mut self`
    /// throughout, and gateway volumes do not need more.
    pub one: tokio::sync::Mutex<HashgramOne>,
    /// Our own `MailAddress`, fixed at start.
    pub me: app::MailAddress,
    cache: std::sync::Mutex<HashMap<String, CachedResolve>>,
    msgs_per_ip: IpLimiter,
    signer: Option<dkim::Signer>,
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shared")
            .field("me", &self.me.address)
            .finish_non_exhaustive()
    }
}

const RESOLVE_TTL_OK: Duration = Duration::from_secs(600);
const RESOLVE_TTL_MISS: Duration = Duration::from_secs(60);

impl Shared {
    /// Opens everything: store, DKIM key, the bridge identity.
    pub async fn open(
        cfg: GatewayConfig,
        passphrase: &str,
        metrics: Arc<Metrics>,
    ) -> Result<Arc<Self>, GatewayError> {
        let store = Arc::new(Store::open(&cfg.store.sqlite_path)?);
        let signer = match (&cfg.domain.dkim_selector, &cfg.domain.dkim_private_key_file) {
            (Some(sel), Some(path)) => {
                let text = std::fs::read_to_string(path)
                    .map_err(|e| GatewayError::Dkim(format!("{}: {e}", path.display())))?;
                let key = dkim::SigningKey::parse(&text)?;
                info!(algorithm = key.algorithm(), selector = %sel, "dkim signing enabled");
                Some(dkim::Signer {
                    domain: cfg.domain.name.clone(),
                    selector: sel.clone(),
                    key,
                })
            }
            _ => {
                warn!("dkim signing disabled: outbound mail will be unsigned and widely rejected");
                None
            }
        };
        let mut one = HashgramOne::open(cfg.sdk_config()?, passphrase).await?;
        let me = one.mail().my_address().await;
        info!(address = %me.address, username = %me.username, "bridge identity opened");
        let msgs_per_ip =
            IpLimiter::new(Duration::from_secs(3600), cfg.smtp.messages_per_ip_per_hour);
        Ok(Arc::new(Self {
            cfg,
            store,
            metrics,
            one: tokio::sync::Mutex::new(one),
            me,
            cache: std::sync::Mutex::new(HashMap::new()),
            msgs_per_ip,
            signer,
        }))
    }

    /// Username → `MailAddress`, cached. `Ok(None)` = no such username.
    pub async fn resolve_username(
        &self,
        username: &str,
    ) -> Result<Option<app::MailAddress>, GatewayError> {
        if let Ok(c) = self.cache.lock() {
            if let Some(hit) = c.get(username) {
                let ttl = if hit.result.is_some() {
                    RESOLVE_TTL_OK
                } else {
                    RESOLVE_TTL_MISS
                };
                if hit.at.elapsed() < ttl {
                    return Ok(hit.result.clone());
                }
            }
        }
        let mut one = self.one.lock().await;
        let result = match one.people().resolve(username).await {
            Ok(r) if r.has_identity => Some(app::MailAddress {
                address: r.address,
                username: r.username,
                display_name: r.display_name,
            }),
            Ok(_) => None, // username exists but no identity/devices: undeliverable
            Err(hashgram_sdk::SdkError::NotFound(_)) => None,
            Err(e) => return Err(e.into()),
        };
        drop(one);
        if let Ok(mut c) = self.cache.lock() {
            if c.len() > 50_000 {
                c.clear();
            }
            c.insert(
                username.to_owned(),
                CachedResolve {
                    result: result.clone(),
                    at: Instant::now(),
                },
            );
        }
        Ok(result)
    }
}

/// The SMTP server's application: recipient checks and message intake.
#[derive(Debug, Clone)]
pub struct GatewayHandler {
    /// Shared state.
    pub shared: Arc<Shared>,
}

impl InboundHandler for GatewayHandler {
    async fn check_recipient(&self, rcpt: &str, _envelope: &Envelope) -> Verdict {
        match classify(rcpt, &self.shared.cfg.domain.name) {
            Ok(Mbx::Local(username)) => match self.shared.resolve_username(&username).await {
                Ok(Some(_)) => Verdict::Accept,
                Ok(None) => Verdict::Reject(550, "5.1.1 no such user".into()),
                Err(e) => {
                    warn!(error = %e, "recipient lookup failed");
                    Verdict::TempFail(451, "4.4.3 directory lookup failed, try again later".into())
                }
            },
            Ok(Mbx::Remote { .. }) => Verdict::Reject(550, "5.7.1 relaying denied".into()),
            Err(_) => Verdict::Reject(550, "5.1.3 bad recipient address".into()),
        }
    }

    async fn accept_message(&self, envelope: Envelope, raw: Vec<u8>) -> Verdict {
        let s = &self.shared;
        s.metrics.smtp_messages_offered.inc();
        if let Ok(ip) = envelope
            .peer
            .rsplit_once(':')
            .map_or(envelope.peer.as_str(), |(h, _)| h)
            .trim_matches(['[', ']'])
            .parse()
        {
            if !s.msgs_per_ip.allow(ip) {
                s.metrics.message("inbound", "rejected");
                return Verdict::TempFail(
                    451,
                    "4.7.1 too many messages from your address, try later".into(),
                );
            }
        }
        let parsed = match inbound::parse(&raw) {
            Ok(p) => p,
            Err(e) => {
                debug!(error = %e, "unparseable message");
                s.metrics.message("inbound", "rejected");
                return Verdict::Reject(550, "5.6.0 message could not be parsed".into());
            }
        };
        let now_ms = hashgram_app::ids::now_ms();
        let now_secs = now_ms.div_euclid(1000);
        if let Decision::Reject(text) =
            policy::decide_inbound(&s.cfg.policy, &parsed.auth, parsed.spam_score)
        {
            let _ = s.store.log_inbound(
                now_secs,
                &parsed.from_header,
                &envelope.rcpt_to.join(","),
                parsed.message_id.as_deref().unwrap_or(""),
                "",
                st::status::REJECTED,
            );
            s.metrics.message("inbound", "rejected");
            return Verdict::Reject(550, text);
        }
        let ids = match resolve_thread_ids(&s.store, &parsed, &s.cfg.domain.name, now_secs) {
            Ok(i) => i,
            Err(e) => {
                error!(error = %e, "thread id resolution failed");
                return Verdict::TempFail(451, "4.3.0 temporary internal error".into());
            }
        };
        let id_hex = hex::encode(&ids.message_id);
        match s.store.inbound_seen(&id_hex) {
            Ok(true) => {
                // Same Message-ID delivered again (another MX attempt or a
                // second recipient batch): already ours, say so.
                debug!("duplicate inbound message accepted idempotently");
                return Verdict::Accept;
            }
            Ok(false) => {}
            Err(e) => {
                error!(error = %e, "store lookup failed");
                return Verdict::TempFail(451, "4.3.0 temporary internal error".into());
            }
        }
        let job = InboundJob {
            envelope: envelope.clone(),
            raw,
            received_at_ms: now_ms,
            hashgram_message_id: id_hex.clone(),
        };
        let queued = s
            .store
            .enqueue(st::kind::INBOUND, &job, now_secs, now_secs)
            .and_then(|_| {
                for r in &envelope.rcpt_to {
                    s.store.log_inbound(
                        now_secs,
                        &parsed.from_header,
                        r,
                        &ids.message_id_header,
                        &id_hex,
                        st::status::QUEUED,
                    )?;
                }
                Ok(())
            });
        match queued {
            Ok(()) => {
                info!(
                    size = job.raw.len(),
                    rcpts = envelope.rcpt_to.len(),
                    auth = ?parsed.auth.as_strings(),
                    spam_score = parsed.spam_score,
                    "inbound message queued"
                );
                s.metrics.message("inbound", "accepted");
                Verdict::Accept
            }
            Err(e) => {
                error!(error = %e, "could not queue inbound message");
                Verdict::TempFail(451, "4.3.0 temporary storage error".into())
            }
        }
    }
}

/// The worker.
pub struct Bridge<R: MxResolver> {
    /// Shared state.
    pub shared: Arc<Shared>,
    resolver: R,
    /// Per-message render attempts (attachment fetches can fail
    /// transiently); bounced after `MAX_RENDER_ATTEMPTS`.
    render_attempts: HashMap<String, u32>,
}

const MAX_RENDER_ATTEMPTS: u32 = 20;
const CURSOR_PREFIX: &str = "inbox_scan:";

impl<R: MxResolver> Bridge<R> {
    /// Builds the worker.
    #[must_use]
    pub fn new(shared: Arc<Shared>, resolver: R) -> Self {
        Self {
            shared,
            resolver,
            render_attempts: HashMap::new(),
        }
    }

    /// Runs rounds until `shutdown` resolves.
    pub async fn run(mut self, shutdown: impl std::future::Future<Output = ()>) {
        let mut shutdown = std::pin::pin!(shutdown);
        let interval = Duration::from_secs(self.shared.cfg.hashgram.sync_interval_secs.max(1));
        loop {
            let started = Instant::now();
            self.round().await;
            let elapsed = started.elapsed();
            let wait = interval.saturating_sub(elapsed);
            tokio::select! {
                () = &mut shutdown => {
                    info!("bridge worker stopping");
                    if let Err(e) = self.shared.one.lock().await.save() {
                        error!(error = %e, "final save failed");
                    }
                    return;
                }
                () = tokio::time::sleep(wait) => {}
            }
        }
    }

    /// One round: sync, inbound queue, inbox scan, outbound queue, save.
    pub async fn round(&mut self) {
        let s = self.shared.clone();
        let now_secs = hashgram_app::ids::now_secs();
        // 1. Sync with the network.
        {
            let mut one = s.one.lock().await;
            match one.sync().round().await {
                Ok(report) => {
                    s.metrics.sync_rounds.inc();
                    s.metrics.connected.set(1);
                    s.metrics.last_sync_ok.set(now_secs as i64);
                    s.metrics.set_healthy(true);
                    if report.mail > 0 {
                        debug!(
                            mail = report.mail,
                            envelopes = report.envelopes,
                            "sync round filed mail"
                        );
                    }
                }
                Err(e) => {
                    s.metrics.sync_failures.inc();
                    s.metrics.connected.set(0);
                    debug!(error = %e, "sync round failed");
                }
            }
        }
        // 2. Inbound queue → HashMail.
        match s.store.due::<InboundJob>(st::kind::INBOUND, now_secs, 20) {
            Ok(items) => {
                for item in items {
                    self.deliver_inbound(item, now_secs).await;
                }
            }
            Err(e) => error!(error = %e, "inbound queue read failed"),
        }
        // 3. New native mail addressed to us → outbound queue.
        if let Err(e) = self.scan_inbox(now_secs).await {
            error!(error = %e, "inbox scan failed");
        }
        // 4. Outbound queue → MX.
        match s.store.due::<OutboundJob>(st::kind::OUTBOUND, now_secs, 10) {
            Ok(items) => {
                for item in items {
                    self.deliver_outbound(item, now_secs).await;
                }
            }
            Err(e) => error!(error = %e, "outbound queue read failed"),
        }
        // 5. Persist MLS state; the SDK requires this after mutations.
        if let Err(e) = s.one.lock().await.save() {
            error!(error = %e, "save failed");
        }
        for k in [st::kind::INBOUND, st::kind::OUTBOUND] {
            if let Ok(d) = s.store.queue_depth(k) {
                s.metrics
                    .queue_depth
                    .get_or_create(&QueueLabels { kind: k })
                    .set(d);
            }
        }
    }

    async fn deliver_inbound(&self, item: QueueItem<InboundJob>, now_secs: u64) {
        let s = &self.shared;
        let job = &item.payload;
        let result = self.try_deliver_inbound(job).await;
        match result {
            Ok(()) => {
                let _ = s.store.dequeue(item.id);
                let _ = s
                    .store
                    .set_inbound_status(&job.hashgram_message_id, st::status::DELIVERED);
                s.metrics.message("inbound", "delivered");
                info!(
                    attempts = item.attempts,
                    "inbound message delivered into HashMail"
                );
            }
            Err(InboundFailure::Permanent(reason)) => {
                let _ = s.store.dequeue(item.id);
                let _ = s
                    .store
                    .set_inbound_status(&job.hashgram_message_id, st::status::FAILED);
                s.metrics.message("inbound", "failed");
                warn!(reason, "inbound message dropped permanently");
            }
            Err(InboundFailure::Transient(reason)) => {
                let attempts = item.attempts + 1;
                if attempts >= s.cfg.outbound.max_attempts {
                    let _ = s.store.dequeue(item.id);
                    let _ = s
                        .store
                        .set_inbound_status(&job.hashgram_message_id, st::status::FAILED);
                    s.metrics.message("inbound", "failed");
                    warn!(reason, attempts, "inbound message gave up after retries");
                } else {
                    let next = now_secs + backoff_secs(attempts);
                    let _ = s.store.reschedule(item.id, next, &reason);
                    s.metrics.message("inbound", "deferred");
                    debug!(reason, attempts, next, "inbound message deferred");
                }
            }
        }
    }

    async fn try_deliver_inbound(&self, job: &InboundJob) -> Result<(), InboundFailure> {
        let s = &self.shared;
        let parsed =
            inbound::parse(&job.raw).map_err(|e| InboundFailure::Permanent(e.to_string()))?;
        // Resolve every envelope recipient again: the cache may have
        // expired, and the identity may have gained devices since.
        let mut resolved = Vec::new();
        for r in &job.envelope.rcpt_to {
            let Ok(Mbx::Local(username)) = classify(r, &s.cfg.domain.name) else {
                continue;
            };
            match s.resolve_username(&username).await {
                Ok(Some(a)) => resolved.push((r.clone(), a)),
                Ok(None) => debug!("recipient vanished between RCPT and delivery"),
                Err(e) => return Err(InboundFailure::Transient(format!("resolve: {e}"))),
            }
        }
        if resolved.is_empty() {
            return Err(InboundFailure::Permanent(
                "no deliverable recipients".into(),
            ));
        }
        let ids = resolve_thread_ids(
            &s.store,
            &parsed,
            &s.cfg.domain.name,
            job.received_at_ms.div_euclid(1000),
        )
        .map_err(|e| InboundFailure::Permanent(e.to_string()))?;
        let recipients = place_recipients(&parsed, resolved);
        let mut one = s.one.lock().await;
        let mut attachments = Vec::with_capacity(parsed.attachments.len());
        for a in &parsed.attachments {
            match one.mail().make_attachment(&a.name, &a.mime, &a.data).await {
                Ok(mut att) => {
                    att.content_id = a.content_id.clone();
                    attachments.push(att);
                }
                Err(e) => return Err(InboundFailure::Transient(format!("attachment upload: {e}"))),
            }
        }
        let msg = build_inbound_message(
            &parsed,
            s.me.clone(),
            recipients,
            attachments,
            &ids,
            job.received_at_ms,
        )
        .map_err(|e| InboundFailure::Permanent(e.to_string()))?;
        match one.mail().send_built(Some(msg), Vec::new()).await {
            Ok(_) => Ok(()),
            Err(e @ hashgram_sdk::SdkError::Invalid(_)) => {
                Err(InboundFailure::Permanent(e.to_string()))
            }
            Err(e) => Err(InboundFailure::Transient(e.to_string())),
        }
    }

    /// Reads new native messages addressed to the bridge and turns those
    /// with `ext-to:` labels into outbound jobs.
    async fn scan_inbox(&mut self, now_secs: u64) -> Result<(), GatewayError> {
        let s = self.shared.clone();
        for folder in [
            hashgram_sdk::mail::folder::INBOX,
            hashgram_sdk::mail::folder::REQUESTS,
        ] {
            let cursor_name = format!("{CURSOR_PREFIX}{folder}");
            let cursor = s.store.cursor(&cursor_name)?;
            // Collect everything newer than the cursor, oldest first.
            let mut fresh: Vec<hashgram_sdk::mail::MailSummary> = Vec::new();
            let mut before = 0u64;
            loop {
                let page = s.one.lock().await.mail().list(folder, before, 100)?;
                if page.is_empty() {
                    break;
                }
                let oldest = page.last().map(|p| p.received_at_ms).unwrap_or(0);
                let full = page.len() == 100;
                fresh.extend(page.into_iter().filter(|p| p.received_at_ms > cursor));
                if !full || oldest <= cursor {
                    break;
                }
                before = oldest;
            }
            fresh.sort_by_key(|p| (p.received_at_ms, p.id.clone()));
            let mut new_cursor = cursor;
            for summary in fresh {
                if summary.outgoing {
                    new_cursor = summary.received_at_ms;
                    continue;
                }
                match self.process_native(&summary.id, now_secs).await {
                    Ok(()) => new_cursor = summary.received_at_ms,
                    Err(GatewayError::Sdk(e)) => {
                        // Network trouble: stop here and retry from this
                        // point next round.
                        let n = self.render_attempts.entry(summary.id.clone()).or_insert(0);
                        *n += 1;
                        if *n >= MAX_RENDER_ATTEMPTS {
                            warn!(error = %e, "giving up rendering an outbound message");
                            self.bounce(&summary.id, &format!("The gateway could not read this message's attachments after {MAX_RENDER_ATTEMPTS} attempts: {e}"))
                                .await;
                            let _ = s.store.log_outbound(
                                now_secs,
                                &summary.id,
                                "",
                                "",
                                "",
                                st::status::FAILED,
                            );
                            new_cursor = summary.received_at_ms;
                            continue;
                        }
                        debug!(error = %e, "deferring outbound render");
                        break;
                    }
                    Err(e) => {
                        warn!(error = %e, "outbound message refused");
                        new_cursor = summary.received_at_ms;
                    }
                }
            }
            if new_cursor != cursor {
                s.store.set_cursor(&cursor_name, new_cursor)?;
            }
        }
        Ok(())
    }

    /// Handles one native message: policy, rate limit, render, sign, queue.
    /// `Err(Sdk)` means "try again later"; other errors are final and have
    /// already produced a bounce.
    async fn process_native(&mut self, id: &str, now_secs: u64) -> Result<(), GatewayError> {
        let s = self.shared.clone();
        if s.store.outbound_seen(id)? {
            return Ok(());
        }
        let record = s.one.lock().await.mail().get(id)?;
        let Some(record) = record else { return Ok(()) };
        let labels = &record.message.labels;
        if !labels
            .iter()
            .any(|l| l.starts_with(EXT_TO) || l.starts_with(EXT_CC))
        {
            // Ordinary mail to the bridge identity (someone said hello).
            // Not ours to relay; leave it in the folder.
            return Ok(());
        }
        let sender = record.authenticated_sender.clone();
        let subject = record.message.subject.clone();
        let mut one = s.one.lock().await;
        let is_contact = one.people().friends().iter().any(|c| c.address == sender);
        if !policy::allow_outbound(&s.cfg.policy, is_contact) {
            drop(one);
            self.bounce(id, "This gateway relays only for its contacts.")
                .await;
            s.store
                .log_outbound(now_secs, id, &sender, "", "", st::status::REJECTED)?;
            s.metrics.message("outbound", "rejected");
            return Err(GatewayError::Address("sender is not a contact".into()));
        }
        let resolved = one.people().resolve(&sender).await?;
        if resolved.username.is_empty() {
            drop(one);
            self.bounce(id, "Your identity has no username; an Internet address needs one (`<username>@<domain>`).")
                .await;
            s.store
                .log_outbound(now_secs, id, &sender, "", "", st::status::REJECTED)?;
            s.metrics.message("outbound", "rejected");
            return Err(GatewayError::Address("sender has no username".into()));
        }
        let (to, cc) = match external_recipients(labels, &s.cfg.domain.name) {
            Ok(x) => x,
            Err(e) => {
                drop(one);
                self.bounce(id, &format!("Recipient labels were invalid: {e}"))
                    .await;
                s.store
                    .log_outbound(now_secs, id, &sender, "", "", st::status::REJECTED)?;
                s.metrics.message("outbound", "rejected");
                return Err(e);
            }
        };
        if to.is_empty() && cc.is_empty() {
            drop(one);
            self.bounce(id, "No valid ext-to: / ext-cc: recipient labels.")
                .await;
            s.store
                .log_outbound(now_secs, id, &sender, "", "", st::status::REJECTED)?;
            return Err(GatewayError::Address("no recipients".into()));
        }
        let count = s.store.rate_hit(&format!("out:{sender}"), now_secs, 3600)?;
        if s.cfg.outbound.per_user_per_hour > 0 && count > s.cfg.outbound.per_user_per_hour {
            drop(one);
            self.bounce(
                id,
                &format!(
                    "Rate limit: at most {} Internet messages per hour.",
                    s.cfg.outbound.per_user_per_hour
                ),
            )
            .await;
            s.store
                .log_outbound(now_secs, id, &sender, "", "", st::status::REJECTED)?;
            s.metrics.message("outbound", "rejected");
            return Err(GatewayError::Address("rate limited".into()));
        }
        // Attachments: decrypt (network for blobs / Drive).
        let mut attachments = Vec::with_capacity(record.message.attachments.len());
        for a in &record.message.attachments {
            let data = one.mail().attachment_bytes(a).await?;
            attachments.push(out::Attachment {
                name: a.name.clone(),
                mime: a.mime.clone(),
                content_id: a.content_id.clone(),
                data,
            });
        }
        drop(one);
        let (mail, _all) = build_outbound(
            &s.store,
            &record,
            &resolved.username,
            &s.cfg.domain.name,
            attachments,
            now_secs,
        )?;
        let mut seed = [0u8; 16];
        let _ = getrandom::fill(&mut seed);
        let rendered = mail.render(&seed);
        let bytes = match &s.signer {
            Some(signer) => signer.sign_message(&rendered.bytes, now_secs)?,
            None => rendered.bytes,
        };
        if bytes.len() as u64 > s.cfg.smtp.max_message_bytes {
            self.bounce(
                id,
                &format!(
                    "The rendered message is {} bytes; the limit is {}.",
                    bytes.len(),
                    s.cfg.smtp.max_message_bytes
                ),
            )
            .await;
            s.store
                .log_outbound(now_secs, id, &sender, "", "", st::status::REJECTED)?;
            return Err(GatewayError::Address("too large".into()));
        }
        // One job per recipient domain.
        let mut by_domain: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for r in to.iter().chain(cc.iter()) {
            if let Some((_, d)) = r.rsplit_once('@') {
                by_domain
                    .entry(d.to_ascii_lowercase())
                    .or_default()
                    .push(r.clone());
            }
        }
        let all_rcpts: Vec<String> = by_domain.values().flatten().cloned().collect();
        s.store.log_outbound(
            now_secs,
            id,
            &sender,
            &all_rcpts.join(","),
            &mail.message_id,
            st::status::QUEUED,
        )?;
        for (domain, rcpts) in by_domain {
            let job = OutboundJob {
                hashgram_message_id: id.to_owned(),
                sender_address: sender.clone(),
                mail_from: mail.from.addr.clone(),
                domain,
                rcpts,
                message: bytes.clone(),
                subject: subject.clone(),
            };
            s.store
                .enqueue(st::kind::OUTBOUND, &job, now_secs, now_secs)?;
        }
        s.metrics.message("outbound", "accepted");
        info!(
            rcpts = all_rcpts.len(),
            size = bytes.len(),
            signed = s.signer.is_some(),
            "outbound message queued"
        );
        // Keep the bridge mailbox small.
        let _ = s.one.lock().await.mail().archive(id);
        self.render_attempts.remove(id);
        Ok(())
    }

    async fn deliver_outbound(&self, item: QueueItem<OutboundJob>, now_secs: u64) {
        let s = &self.shared;
        let job = &item.payload;
        let cfg = ClientConfig {
            helo_hostname: s.cfg.smtp_hostname().to_owned(),
            timeout: Duration::from_secs(s.cfg.outbound.smtp_timeout_secs),
            require_tls: s.cfg.outbound.require_tls,
        };
        let hosts: Result<Vec<(String, u16)>, DeliveryError> = match &s.cfg.outbound.smarthost {
            Some(sh) => match sh
                .rsplit_once(':')
                .and_then(|(h, p)| p.parse::<u16>().ok().map(|p| (h.to_owned(), p)))
            {
                Some(hp) => Ok(vec![hp]),
                None => Err(DeliveryError::Protocol("bad smarthost".into())),
            },
            None => match self.resolver.resolve(&job.domain).await {
                Ok(MxAnswer::Hosts(h)) => Ok(h
                    .into_iter()
                    .map(|h| (h.host, s.cfg.outbound.smtp_port))
                    .collect()),
                Ok(MxAnswer::NullMx) => Err(DeliveryError::Permanent(crate::smtp::Reply::new(
                    556,
                    format!("{} does not accept mail (null MX)", job.domain),
                ))),
                Err(e) => Err(DeliveryError::Io(e.to_string())),
            },
        };
        let mut last: Option<DeliveryError> = None;
        let mut outcome: Option<client::Delivered> = None;
        match hosts {
            Ok(hosts) => {
                for (host, port) in hosts.iter().take(5) {
                    match client::deliver(
                        &cfg,
                        host,
                        *port,
                        &job.mail_from,
                        &job.rcpts,
                        &job.message,
                    )
                    .await
                    {
                        Ok(d) => {
                            outcome = Some(d);
                            break;
                        }
                        Err(e @ DeliveryError::Permanent(_)) => {
                            last = Some(e);
                            break;
                        }
                        Err(e) => {
                            debug!(host = %host, error = %e, "mx attempt failed");
                            last = Some(e);
                        }
                    }
                }
            }
            Err(e) => last = Some(e),
        }
        match (outcome, last) {
            (Some(d), _) => {
                let _ = s.store.dequeue(item.id);
                let _ = s.store.set_outbound_status(
                    &job.hashgram_message_id,
                    st::status::DELIVERED,
                    "",
                );
                s.metrics.message("outbound", "delivered");
                info!(domain = %job.domain, tls = d.tls, rcpts = job.rcpts.len(), rejected = d.rejected.len(), "outbound message delivered");
                if !d.rejected.is_empty() {
                    let list = d
                        .rejected
                        .iter()
                        .map(|(r, reply)| format!("{r}: {reply}"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    self.bounce(
                        &job.hashgram_message_id,
                        &format!("Some recipients were refused by {}:\n{list}", job.domain),
                    )
                    .await;
                }
            }
            (None, Some(e)) if e.is_permanent() => {
                let _ = s.store.dequeue(item.id);
                let _ = s.store.set_outbound_status(
                    &job.hashgram_message_id,
                    st::status::FAILED,
                    &e.to_string(),
                );
                s.metrics.message("outbound", "failed");
                warn!(domain = %job.domain, error = %e, "outbound message failed permanently");
                self.bounce(
                    &job.hashgram_message_id,
                    &format!("{} refused the message: {e}", job.domain),
                )
                .await;
            }
            (None, last) => {
                let reason = last
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "no mail hosts".into());
                let attempts = item.attempts + 1;
                if attempts >= s.cfg.outbound.max_attempts {
                    let _ = s.store.dequeue(item.id);
                    let _ = s.store.set_outbound_status(
                        &job.hashgram_message_id,
                        st::status::FAILED,
                        &reason,
                    );
                    s.metrics.message("outbound", "failed");
                    warn!(domain = %job.domain, attempts, reason, "outbound message gave up");
                    self.bounce(
                        &job.hashgram_message_id,
                        &format!("Could not deliver to {} after {attempts} attempts. Last error: {reason}", job.domain),
                    )
                    .await;
                } else {
                    let next = now_secs + backoff_secs(attempts);
                    let _ = s.store.reschedule(item.id, next, &reason);
                    let _ = s.store.set_outbound_status(
                        &job.hashgram_message_id,
                        st::status::QUEUED,
                        &reason,
                    );
                    s.metrics.message("outbound", "deferred");
                    debug!(domain = %job.domain, attempts, next, reason, "outbound message deferred");
                }
            }
        }
    }

    /// Sends a native "Undeliverable" notice back to the sender of a
    /// message we could not relay, threaded onto their message.
    async fn bounce(&self, id: &str, reason: &str) {
        let s = &self.shared;
        let mut one = s.one.lock().await;
        let Ok(Some(record)) = one.mail().get(id) else {
            return;
        };
        let sender = record.authenticated_sender.clone();
        let draft = m::Draft {
            to: vec![app::MailAddress {
                address: sender.clone(),
                username: record.message.from.as_ref().map(|a| a.username.clone()).unwrap_or_default(),
                display_name: String::new(),
            }],
            subject: format!("Undeliverable: {}", record.message.subject),
            body_text: format!(
                "Your message could not be delivered to the Internet by the {} gateway.\n\nReason:\n{reason}\n\nThis notice was generated by the gateway; the original message stays in your Sent folder.",
                s.cfg.domain.name
            ),
            in_reply_to: Some(m::ReplyTarget {
                message_id: record.message.message_id.clone(),
                thread_id: record.message.thread_id.clone(),
                references: record.message.references.clone(),
            }),
            ..m::Draft::default()
        };
        match one.mail().send(draft).await {
            Ok(_) => {
                s.metrics.message("outbound", "bounced");
                info!("bounce sent to the sender");
            }
            Err(e) => warn!(error = %e, "could not send bounce"),
        }
    }
}

#[derive(Debug)]
enum InboundFailure {
    Permanent(String),
    Transient(String),
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
    use crate::mime::inbound::Mailbox as InMbx;

    const GW: &str = "hash1e0tl2hff03hu4g3sawcjqa2p9tc4uh24e4vfl5";
    const ALICE: &str = "hash178njz3gft77798ssh6lrqxh2hskw5dm68f2uh5";
    const CAROL: &str = "hash1znsxwr000000000000000000000000000000000";
    const D: &str = "hashgram.io";

    fn addr(a: &str, u: &str) -> app::MailAddress {
        app::MailAddress {
            address: a.into(),
            username: u.into(),
            display_name: String::new(),
        }
    }

    fn parsed(msgid: Option<&str>, irt: Option<&str>, refs: &[&str]) -> InboundMail {
        InboundMail {
            from: Some(InMbx {
                name: "Bob".into(),
                addr: "bob@example.com".into(),
            }),
            from_header: "Bob <bob@example.com>".into(),
            to: vec![InMbx {
                name: String::new(),
                addr: "alice@hashgram.io".into(),
            }],
            cc: vec![InMbx {
                name: String::new(),
                addr: "carol@hashgram.io".into(),
            }],
            subject: "Hi".into(),
            message_id: msgid.map(str::to_owned),
            in_reply_to: irt.map(str::to_owned),
            references: refs.iter().map(|s| (*s).to_owned()).collect(),
            body_text: "hello".into(),
            spam_score: 120,
            ..InboundMail::default()
        }
    }

    #[test]
    fn thread_ids_derive_and_remember() {
        let store = Store::open_in_memory().unwrap();
        // First message in a thread from the Internet.
        let p1 = parsed(Some("m1@example.com"), None, &[]);
        let t1 = resolve_thread_ids(&store, &p1, D, 1).unwrap();
        assert_eq!(t1.message_id, thread::derive_id("m1@example.com").unwrap());
        assert_eq!(t1.thread_id, t1.message_id);
        assert!(t1.in_reply_to.is_empty());
        // Reply from the Internet quoting it.
        let p2 = parsed(
            Some("m2@example.com"),
            Some("m1@example.com"),
            &["m1@example.com"],
        );
        let t2 = resolve_thread_ids(&store, &p2, D, 2).unwrap();
        assert_eq!(t2.in_reply_to, t1.message_id);
        assert_eq!(t2.thread_id, t1.thread_id);
        assert_eq!(t2.references, vec![t1.message_id.clone()]);
        // Delivered twice: same id (dedup upstream), same mapping.
        let t2b = resolve_thread_ids(&store, &p2, D, 3).unwrap();
        assert_eq!(t2b, t2);
        // A reply to one of OUR outbound messages (our <hex@domain> id).
        let ours = vec![9u8; 16];
        let our_header = thread::outbound_message_id(&ours, D);
        let p3 = parsed(Some("m3@example.com"), Some(&our_header), &[]);
        let t3 = resolve_thread_ids(&store, &p3, D, 4).unwrap();
        assert_eq!(t3.in_reply_to, ours);
        assert_eq!(t3.references, vec![ours.clone()]);
        // Nothing known about `ours`' thread: the root reference (= parent) starts it.
        assert_eq!(t3.thread_id, ours);
        // No Message-ID at all: minted, still mapped.
        let p4 = parsed(None, None, &[]);
        let t4 = resolve_thread_ids(&store, &p4, D, 5).unwrap();
        assert_eq!(t4.message_id.len(), 16);
        assert!(t4.message_id_header.ends_with("@hashgram.io"));
        assert!(store
            .thread_by_hashgram_id(&hex::encode(&t4.message_id))
            .unwrap()
            .is_some());
    }

    #[test]
    fn outbound_thread_headers_map_back() {
        let store = Store::open_in_memory().unwrap();
        let p1 = parsed(Some("m1@example.com"), None, &[]);
        let t1 = resolve_thread_ids(&store, &p1, D, 1).unwrap();
        // A native reply references the derived id; the header we emit is
        // the *original* Internet Message-ID, not our hex form.
        assert_eq!(
            header_for_id(&store, &t1.message_id, D).unwrap(),
            "<m1@example.com>"
        );
        let fresh = vec![5u8; 16];
        assert_eq!(
            header_for_id(&store, &fresh, D).unwrap(),
            thread::outbound_message_id(&fresh, D)
        );
        assert_eq!(header_for_id(&store, &[], D).unwrap(), "");
    }

    #[test]
    fn inbound_message_is_external_and_valid() {
        let store = Store::open_in_memory().unwrap();
        let p = parsed(Some("m1@example.com"), None, &[]);
        let ids = resolve_thread_ids(&store, &p, D, 1).unwrap();
        let recipients = place_recipients(
            &p,
            vec![
                ("alice@hashgram.io".into(), addr(ALICE, "alice")),
                ("carol@hashgram.io".into(), addr(CAROL, "carol")),
            ],
        );
        assert_eq!(recipients.to.len(), 1);
        assert_eq!(recipients.cc.len(), 1);
        assert_eq!(recipients.cc[0].address, CAROL);
        let att = app::MailAttachment {
            name: "a.txt".into(),
            mime: "text/plain".into(),
            size: 2,
            source: Some(app::mail_attachment::Source::InlineData(vec![1, 2])),
            ..Default::default()
        };
        let msg = build_inbound_message(
            &p,
            addr(GW, "gateway"),
            recipients,
            vec![att],
            &ids,
            1_000_000,
        )
        .unwrap();
        assert_eq!(msg.origin, app::MailOrigin::ExternalGateway as i32);
        let ext = msg.external.as_ref().unwrap();
        assert_eq!(ext.gateway, GW);
        assert_eq!(ext.from_header, "Bob <bob@example.com>");
        assert_eq!(ext.message_id_header, "m1@example.com");
        assert_eq!(
            ext.auth_results,
            vec!["spf=none", "dkim=none", "dmarc=none"]
        );
        assert_eq!(ext.spam_score, 120);
        assert_eq!(msg.message_id, ids.message_id);
        assert_eq!(
            msg.from.as_ref().unwrap().display_name,
            "Bob <bob@example.com>"
        );
        assert_eq!(msg.from.as_ref().unwrap().address, GW);
        assert_eq!(msg.labels, vec![LABEL_EXTERNAL]);
        assert_eq!(msg.created_at_ms, 1_000_000); // no Date header → received time
        assert!(m::validate(&msg).is_ok());
        // A Date far in the future is not trusted.
        let mut p2 = p.clone();
        p2.date_ms = Some(9_000_000_000_000);
        let msg2 = build_inbound_message(
            &p2,
            addr(GW, "gateway"),
            place_recipients(
                &p2,
                vec![("alice@hashgram.io".into(), addr(ALICE, "alice"))],
            ),
            vec![],
            &ids,
            1_000_000,
        )
        .unwrap();
        assert_eq!(msg2.created_at_ms, 1_000_000);
    }

    #[test]
    fn bcc_style_recipient_goes_to_to_line() {
        let p = parsed(Some("x@example.com"), None, &[]);
        let r = place_recipients(&p, vec![("dave@hashgram.io".into(), addr(CAROL, "dave"))]);
        assert_eq!(r.to.len(), 1);
        assert!(r.cc.is_empty());
    }

    #[test]
    fn ext_labels() {
        let labels: Vec<String> = vec![
            "invoice".into(),
            "ext-to:Bob@Example.com".into(),
            "ext-to:bob@example.com".into(),
            "ext-cc:carol@example.org".into(),
        ];
        let (to, cc) = external_recipients(&labels, D).unwrap();
        assert_eq!(to, vec!["Bob@example.com"]);
        assert_eq!(cc, vec!["carol@example.org"]);
        assert!(external_recipients(&["ext-to:alice@hashgram.io".to_owned()], D).is_err());
        assert!(external_recipients(&["ext-to:garbage".to_owned()], D).is_err());
        assert_eq!(
            external_recipients(&["plain".to_owned()], D).unwrap(),
            (vec![], vec![])
        );
    }

    #[test]
    fn outbound_build_uses_thread_map() {
        let store = Store::open_in_memory().unwrap();
        let p1 = parsed(Some("m1@example.com"), None, &[]);
        let t1 = resolve_thread_ids(&store, &p1, D, 1).unwrap();
        let native_id = vec![7u8; 16];
        let record = MailRecord {
            message: app::MailMessage {
                version: 1,
                message_id: native_id.clone(),
                thread_id: t1.thread_id.clone(),
                from: Some(app::MailAddress {
                    address: ALICE.into(),
                    username: "alice".into(),
                    display_name: "Alice".into(),
                }),
                to: vec![addr(GW, "gateway")],
                created_at_ms: 1_789_237_500_000,
                subject: "Re: Hi".into(),
                in_reply_to: t1.message_id.clone(),
                references: vec![t1.message_id.clone()],
                body_text: "thanks".into(),
                labels: vec!["ext-to:bob@example.com".into()],
                importance: app::MailImportance::High as i32,
                ..Default::default()
            },
            folder: "inbox".into(),
            read: false,
            starred: false,
            labels: vec![],
            received_at_ms: 1,
            authenticated_sender: ALICE.into(),
            group_id: String::new(),
            delivered_to: BTreeMap::new(),
            read_by: BTreeMap::new(),
            outgoing: false,
            trust_score: 0,
        };
        let (mail, rcpts) = build_outbound(&store, &record, "alice", D, vec![], 10).unwrap();
        assert_eq!(rcpts, vec!["bob@example.com"]);
        assert_eq!(mail.from.addr, "alice@hashgram.io");
        assert_eq!(mail.from.name, "Alice");
        assert_eq!(
            mail.message_id,
            format!("<{}@hashgram.io>", "07".repeat(16))
        );
        assert_eq!(mail.in_reply_to.as_deref(), Some("<m1@example.com>"));
        assert_eq!(mail.references, vec!["<m1@example.com>"]);
        assert_eq!(mail.importance, app::MailImportance::High);
        // The outbound id is now remembered, so an Internet reply maps back.
        let row = store
            .thread_by_header(&format!("{}@hashgram.io", "07".repeat(16)))
            .unwrap()
            .unwrap();
        assert_eq!(row.hashgram_id, hex::encode(&native_id));
        assert_eq!(row.thread_id, hex::encode(&t1.thread_id));
        let reply = parsed(Some("m9@example.com"), Some(&mail.message_id), &[]);
        let t9 = resolve_thread_ids(&store, &reply, D, 11).unwrap();
        assert_eq!(t9.in_reply_to, native_id);
        assert_eq!(t9.thread_id, t1.thread_id);
        // Renders and parses.
        let r = mail.render(&[1u8; 16]);
        assert!(out::well_formed(&r.bytes));
        let back = inbound::parse(&r.bytes).unwrap();
        assert_eq!(back.in_reply_to.as_deref(), Some("m1@example.com"));
    }

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff_secs(0), 30);
        assert_eq!(backoff_secs(1), 60);
        assert_eq!(backoff_secs(3), 240);
        assert_eq!(backoff_secs(20), 6 * 3600);
    }
}
