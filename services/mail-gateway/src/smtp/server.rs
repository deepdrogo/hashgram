//! The inbound SMTP server.
//!
//! [`Session`] is a pure state machine: feed it one line, get back what to
//! send and what to ask the application. It never touches a socket, so
//! every transition — including the ugly ones (commands out of order,
//! oversize `DATA`, a client that never sends the terminator) — is
//! unit-tested without networking. [`serve`] is the thin tokio driver
//! around it.
//!
//! What v1 deliberately does not do: `AUTH` (nobody submits through this
//! gateway; Hashgram users send outbound mail natively to the bridge
//! identity) and `STARTTLS` (TLS is terminated by the fronting MTA or
//! proxy; see `docs/MAIL_GATEWAY.md` § Limitations). Both answer `502`.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tracing::{debug, info, warn};

use super::{split_line, Reply, MAX_COMMAND_LINE};
use crate::ratelimit::IpLimiter;

/// The transaction envelope as the client stated it.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Envelope {
    /// `HELO`/`EHLO` argument.
    pub helo: String,
    /// Reverse path (empty for a null sender `<>`).
    pub mail_from: String,
    /// Forward paths, as given.
    pub rcpt_to: Vec<String>,
    /// Peer address (string so it serialises into the queue).
    pub peer: String,
}

/// What the application says about a recipient or a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Take it.
    Accept,
    /// Refuse permanently (5xx).
    Reject(u16, String),
    /// Refuse for now (4xx).
    TempFail(u16, String),
}

impl Verdict {
    /// A `550` with text.
    #[must_use]
    pub fn reject(text: impl Into<String>) -> Self {
        Self::Reject(550, text.into())
    }
    /// A `451` with text.
    #[must_use]
    pub fn temp_fail(text: impl Into<String>) -> Self {
        Self::TempFail(451, text.into())
    }
}

/// The application behind the listener.
pub trait InboundHandler: Send + Sync + 'static {
    /// Called for each `RCPT TO`. Return `Reject` for unknown local users
    /// (SMTP `550`) or relaying attempts.
    fn check_recipient(
        &self,
        rcpt: &str,
        envelope: &Envelope,
    ) -> impl Future<Output = Verdict> + Send;
    /// Called once the full message is in. `Accept` means we now own it
    /// (it is queued durably); the client receives `250`.
    fn accept_message(
        &self,
        envelope: Envelope,
        raw: Vec<u8>,
    ) -> impl Future<Output = Verdict> + Send;
}

/// Server bounds.
#[derive(Debug, Clone)]
pub struct Limits {
    /// Maximum `DATA` size.
    pub max_message_bytes: u64,
    /// Maximum `RCPT TO` per transaction.
    pub max_recipients: usize,
}

/// What the driver must do after feeding a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Send a reply and keep going.
    Reply(Reply),
    /// Send a reply and close the connection.
    Close(Reply),
    /// Ask the handler about a recipient, then call
    /// [`Session::recipient_verdict`].
    CheckRecipient(String),
    /// Hand the message to the handler, then call
    /// [`Session::message_verdict`].
    Message(Envelope, Vec<u8>),
    /// Nothing to send (a `DATA` line was absorbed).
    Continue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Before `HELO`/`EHLO`.
    Connected,
    /// Greeted; no transaction.
    Ready,
    /// After `MAIL FROM`.
    Mail,
    /// At least one `RCPT TO`.
    Rcpt,
    /// Collecting `DATA`.
    Data,
}

/// One SMTP connection's protocol state.
#[derive(Debug)]
pub struct Session {
    hostname: String,
    limits: Limits,
    phase: Phase,
    envelope: Envelope,
    data: Vec<u8>,
    /// Set once `DATA` exceeded the limit: the rest is consumed and dropped
    /// so the connection stays in sync, then `552` is returned.
    oversize: bool,
    errors: u32,
}

/// Consecutive protocol errors before we hang up (RFC 5321 §4.3.2 lets a
/// server drop clients that keep sending garbage).
const MAX_ERRORS: u32 = 10;

impl Session {
    /// A fresh session for a peer.
    #[must_use]
    pub fn new(hostname: &str, limits: Limits, peer: &str) -> Self {
        Self {
            hostname: hostname.to_owned(),
            limits,
            phase: Phase::Connected,
            envelope: Envelope {
                peer: peer.to_owned(),
                ..Envelope::default()
            },
            data: Vec::new(),
            oversize: false,
            errors: 0,
        }
    }

    /// The greeting.
    #[must_use]
    pub fn banner(&self) -> Reply {
        Reply::new(
            220,
            format!("{} ESMTP hashgram-mail-gateway", self.hostname),
        )
    }

    /// Whether the next line is message data rather than a command.
    #[must_use]
    pub fn in_data(&self) -> bool {
        self.phase == Phase::Data
    }

    /// The envelope so far.
    #[must_use]
    pub fn envelope(&self) -> &Envelope {
        &self.envelope
    }

    /// The bounds.
    #[must_use]
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// Marks the current `DATA` as over the limit (the driver calls this
    /// when a single unterminated line already exceeds it).
    pub fn mark_oversize(&mut self) {
        if self.phase == Phase::Data {
            self.oversize = true;
            self.data = Vec::new();
        }
    }

    fn reset_transaction(&mut self) {
        self.envelope.mail_from.clear();
        self.envelope.rcpt_to.clear();
        self.data = Vec::new();
        self.oversize = false;
        if self.phase != Phase::Connected {
            self.phase = Phase::Ready;
        }
    }

    fn error(&mut self, reply: Reply) -> Action {
        self.errors += 1;
        if self.errors >= MAX_ERRORS {
            return Action::Close(Reply::new(
                421,
                format!("{} too many errors, closing", self.hostname),
            ));
        }
        Action::Reply(reply)
    }

    /// Feeds one command line (no terminator).
    pub fn command(&mut self, line: &[u8]) -> Action {
        if line.len() > MAX_COMMAND_LINE {
            return self.error(Reply::new(500, "line too long"));
        }
        let Ok(text) = std::str::from_utf8(line) else {
            return self.error(Reply::new(500, "command is not valid UTF-8"));
        };
        let text = text.trim_end();
        let (verb, arg) = match text.find(' ') {
            Some(i) => (
                text.get(..i).unwrap_or(""),
                text.get(i + 1..).unwrap_or("").trim(),
            ),
            None => (text, ""),
        };
        let action = match verb.to_ascii_uppercase().as_str() {
            "EHLO" => self.helo(arg, true),
            "HELO" => self.helo(arg, false),
            "MAIL" => self.mail(arg),
            "RCPT" => self.rcpt(arg),
            "DATA" => self.data_cmd(arg),
            "RSET" => {
                self.reset_transaction();
                Action::Reply(Reply::new(250, "OK"))
            }
            "NOOP" => Action::Reply(Reply::new(250, "OK")),
            "QUIT" => Action::Close(Reply::new(
                221,
                format!("{} closing connection", self.hostname),
            )),
            "VRFY" | "EXPN" => Action::Reply(Reply::new(
                252,
                "Cannot VRFY user, but will accept message and attempt delivery",
            )),
            "HELP" => Action::Reply(Reply::new(
                214,
                "https://github.com/hashgram/hashgram docs/MAIL_GATEWAY.md",
            )),
            "STARTTLS" => self.error(Reply::new(
                502,
                "STARTTLS not offered; TLS is terminated upstream",
            )),
            "AUTH" => self.error(Reply::new(
                502,
                "AUTH not offered; this gateway does not relay for authenticated users",
            )),
            "" => self.error(Reply::new(500, "empty command")),
            _ => self.error(Reply::new(500, "command not recognized")),
        };
        // The error budget is for *consecutive* garbage; a client that
        // recovers is not the attacker this protects against.
        let recovered = matches!(&action, Action::Reply(r) if r.code < 400)
            || matches!(action, Action::CheckRecipient(_));
        if recovered {
            self.errors = 0;
        }
        action
    }

    fn helo(&mut self, arg: &str, extended: bool) -> Action {
        if arg.is_empty() || arg.contains(char::is_whitespace) {
            return self.error(Reply::new(501, "syntax: EHLO hostname"));
        }
        self.reset_transaction();
        self.envelope.helo = arg.chars().take(253).collect();
        self.phase = Phase::Ready;
        if extended {
            Action::Reply(Reply::multi(
                250,
                vec![
                    format!("{} greets {}", self.hostname, self.envelope.helo),
                    format!("SIZE {}", self.limits.max_message_bytes),
                    "8BITMIME".into(),
                    "PIPELINING".into(),
                    "ENHANCEDSTATUSCODES".into(),
                ],
            ))
        } else {
            Action::Reply(Reply::new(
                250,
                format!("{} greets {}", self.hostname, self.envelope.helo),
            ))
        }
    }

    fn mail(&mut self, arg: &str) -> Action {
        match self.phase {
            Phase::Connected => return self.error(Reply::new(503, "5.5.1 send HELO/EHLO first")),
            Phase::Mail | Phase::Rcpt => {
                return self.error(Reply::new(503, "5.5.1 nested MAIL command"))
            }
            Phase::Data => return self.error(Reply::new(503, "5.5.1 bad sequence")),
            Phase::Ready => {}
        }
        let Some((path, params)) = parse_path_arg(arg, "FROM:") else {
            return self.error(Reply::new(
                501,
                "5.5.4 syntax: MAIL FROM:<address> [SIZE=n]",
            ));
        };
        for p in params {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            match k.to_ascii_uppercase().as_str() {
                "SIZE" => match v.parse::<u64>() {
                    Ok(n) if n > self.limits.max_message_bytes => {
                        return self.error(Reply::new(
                            552,
                            "5.3.4 message exceeds fixed maximum message size",
                        ));
                    }
                    Ok(_) => {}
                    Err(_) => return self.error(Reply::new(501, "5.5.4 bad SIZE parameter")),
                },
                "BODY" => {
                    if !v.eq_ignore_ascii_case("7BIT") && !v.eq_ignore_ascii_case("8BITMIME") {
                        return self.error(Reply::new(501, "5.5.4 unsupported BODY value"));
                    }
                }
                _ => return self.error(Reply::new(555, "5.5.4 unsupported MAIL parameter")),
            }
        }
        if !path.is_empty() && (path.len() > 254 || !path.contains('@')) {
            return self.error(Reply::new(501, "5.1.7 bad sender address syntax"));
        }
        self.envelope.mail_from = path.to_owned();
        self.envelope.rcpt_to.clear();
        self.phase = Phase::Mail;
        Action::Reply(Reply::new(250, "2.1.0 OK"))
    }

    fn rcpt(&mut self, arg: &str) -> Action {
        match self.phase {
            Phase::Mail | Phase::Rcpt => {}
            _ => return self.error(Reply::new(503, "5.5.1 need MAIL command first")),
        }
        let Some((path, params)) = parse_path_arg(arg, "TO:") else {
            return self.error(Reply::new(501, "5.5.4 syntax: RCPT TO:<address>"));
        };
        if !params.is_empty() {
            return self.error(Reply::new(555, "5.5.4 unsupported RCPT parameter"));
        }
        if path.is_empty() || path.len() > 254 || !path.contains('@') {
            return self.error(Reply::new(501, "5.1.3 bad recipient address syntax"));
        }
        if self.envelope.rcpt_to.len() >= self.limits.max_recipients {
            return self.error(Reply::new(452, "4.5.3 too many recipients"));
        }
        Action::CheckRecipient(path.to_owned())
    }

    /// The handler's answer to [`Action::CheckRecipient`].
    pub fn recipient_verdict(&mut self, rcpt: String, verdict: Verdict) -> Reply {
        match verdict {
            Verdict::Accept => {
                self.envelope.rcpt_to.push(rcpt);
                self.phase = Phase::Rcpt;
                Reply::new(250, "2.1.5 OK")
            }
            Verdict::Reject(code, text) => Reply::new(code.max(500), text),
            Verdict::TempFail(code, text) => Reply::new(code.clamp(400, 499), text),
        }
    }

    fn data_cmd(&mut self, arg: &str) -> Action {
        if !arg.is_empty() {
            return self.error(Reply::new(501, "5.5.4 DATA takes no parameters"));
        }
        match self.phase {
            Phase::Rcpt => {}
            Phase::Mail => return self.error(Reply::new(503, "5.5.1 need RCPT command first")),
            _ => return self.error(Reply::new(503, "5.5.1 need MAIL and RCPT first")),
        }
        self.phase = Phase::Data;
        self.data = Vec::new();
        self.oversize = false;
        Action::Reply(Reply::new(354, "End data with <CR><LF>.<CR><LF>"))
    }

    /// Feeds one line of `DATA` (no terminator). Dot-unstuffs; the lone
    /// `.` ends the message.
    pub fn data_line(&mut self, line: &[u8]) -> Action {
        if self.phase != Phase::Data {
            return self.error(Reply::new(503, "5.5.1 not in DATA"));
        }
        if line == b"." {
            self.phase = Phase::Rcpt;
            if self.oversize {
                self.reset_transaction();
                return self.error(Reply::new(
                    552,
                    "5.3.4 message exceeds fixed maximum message size",
                ));
            }
            let raw = std::mem::take(&mut self.data);
            return Action::Message(self.envelope.clone(), raw);
        }
        if self.oversize {
            return Action::Continue;
        }
        let body = line.strip_prefix(b".").unwrap_or(line);
        let projected = self.data.len() as u64 + body.len() as u64 + 2;
        if projected > self.limits.max_message_bytes {
            self.oversize = true;
            self.data = Vec::new();
            return Action::Continue;
        }
        self.data.extend_from_slice(body);
        self.data.extend_from_slice(b"\r\n");
        Action::Continue
    }

    /// The handler's answer to [`Action::Message`]. The transaction is over
    /// either way.
    pub fn message_verdict(&mut self, verdict: Verdict) -> Reply {
        self.reset_transaction();
        match verdict {
            Verdict::Accept => Reply::new(250, "2.0.0 OK: queued"),
            Verdict::Reject(code, text) => Reply::new(code.max(500), text),
            Verdict::TempFail(code, text) => Reply::new(code.clamp(400, 499), text),
        }
    }
}

/// `FROM:<a@b> SIZE=1 BODY=8BITMIME` → (`a@b`, [`SIZE=1`, `BODY=8BITMIME`]).
/// Tolerates a space after the colon and a missing angle bracket pair.
fn parse_path_arg<'a>(arg: &'a str, keyword: &str) -> Option<(&'a str, Vec<&'a str>)> {
    let kw_len = keyword.len();
    let head = arg.get(..kw_len)?;
    if !head.eq_ignore_ascii_case(keyword) {
        return None;
    }
    let rest = arg.get(kw_len..)?.trim_start();
    let (path_part, params) = if let Some(after) = rest.strip_prefix('<') {
        let end = after.find('>')?;
        (after.get(..end)?, after.get(end + 1..)?)
    } else {
        match rest.find(' ') {
            Some(i) => (rest.get(..i)?, rest.get(i..)?),
            None => (rest, ""),
        }
    };
    // Drop an obsolete source route: `@a,@b:user@host`.
    let path = match path_part.rsplit_once(':') {
        Some((route, mailbox)) if route.starts_with('@') => mailbox,
        _ => path_part,
    };
    if path.chars().any(|c| c.is_control() || c == ' ') {
        return None;
    }
    let params: Vec<&str> = params.split_whitespace().collect();
    Some((path, params))
}

/// Drives one connection over any byte stream.
pub async fn serve_connection<S, H>(
    mut stream: S,
    peer: &str,
    hostname: &str,
    limits: Limits,
    idle: Duration,
    handler: Arc<H>,
) -> Result<(), std::io::Error>
where
    S: AsyncRead + AsyncWrite + Unpin,
    H: InboundHandler,
{
    let mut session = Session::new(hostname, limits, peer);
    stream
        .write_all(session.banner().to_wire().as_bytes())
        .await?;
    let mut buf: Vec<u8> = Vec::with_capacity(8192);
    let mut chunk = vec![0u8; 16 * 1024];
    loop {
        // Consume every complete line already buffered (PIPELINING).
        while let Some((line, used)) = split_line(&buf).map(|(l, n)| (l.to_vec(), n)) {
            buf.drain(..used);
            let action = if session.in_data() {
                session.data_line(&line)
            } else {
                session.command(&line)
            };
            let reply = match action {
                Action::Continue => continue,
                Action::Reply(r) => r,
                Action::Close(r) => {
                    stream.write_all(r.to_wire().as_bytes()).await?;
                    let _ = stream.shutdown().await;
                    return Ok(());
                }
                Action::CheckRecipient(rcpt) => {
                    let envelope_snapshot = session.envelope().clone();
                    let v = handler.check_recipient(&rcpt, &envelope_snapshot).await;
                    session.recipient_verdict(rcpt, v)
                }
                Action::Message(envelope, raw) => {
                    let size = raw.len();
                    let v = handler.accept_message(envelope, raw).await;
                    debug!(peer, size, verdict = ?v, "message verdict");
                    session.message_verdict(v)
                }
            };
            stream.write_all(reply.to_wire().as_bytes()).await?;
        }
        if buf.len() > MAX_COMMAND_LINE && !session.in_data() {
            // A command line with no terminator in sight: refuse rather
            // than buffer without bound.
            stream
                .write_all(Reply::new(500, "line too long").to_wire().as_bytes())
                .await?;
            buf.clear();
        }
        if session.in_data() && buf.len() as u64 > session.limits().max_message_bytes + 1024 {
            // A single DATA "line" larger than the whole size limit: drop
            // the bytes, remember the overflow, keep scanning for the
            // terminator so the reply is a clean 552.
            buf.clear();
            session.mark_oversize();
        }
        let n = match tokio::time::timeout(idle, stream.read(&mut chunk)).await {
            Ok(Ok(0)) => return Ok(()),
            Ok(Ok(n)) => n,
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                let _ = stream
                    .write_all(
                        Reply::new(421, "4.4.2 idle timeout, closing")
                            .to_wire()
                            .as_bytes(),
                    )
                    .await;
                return Ok(());
            }
        };
        buf.extend_from_slice(chunk.get(..n).unwrap_or(&[]));
    }
}

/// Listener settings.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Bind address.
    pub listen: SocketAddr,
    /// Banner hostname.
    pub hostname: String,
    /// Bounds.
    pub limits: Limits,
    /// Per-connection idle timeout.
    pub idle_timeout: Duration,
    /// Concurrent connection cap.
    pub max_connections: usize,
    /// Connections per source IP per minute (0 = unlimited).
    pub connections_per_ip_per_minute: u32,
}

/// Binds `cfg.listen` and accepts connections until `shutdown` resolves.
pub async fn serve<H: InboundHandler>(
    cfg: ServerConfig,
    handler: Arc<H>,
    shutdown: impl Future<Output = ()>,
) -> Result<(), std::io::Error> {
    let listener = TcpListener::bind(cfg.listen).await?;
    info!(listen = %cfg.listen, "smtp listening");
    serve_listener(listener, cfg, handler, shutdown).await
}

/// Accepts connections on an already-bound listener until `shutdown`
/// resolves (split from [`serve`] so tests can bind port 0).
pub async fn serve_listener<H: InboundHandler>(
    listener: TcpListener,
    cfg: ServerConfig,
    handler: Arc<H>,
    shutdown: impl Future<Output = ()>,
) -> Result<(), std::io::Error> {
    let sem = Arc::new(tokio::sync::Semaphore::new(cfg.max_connections));
    let limiter = Arc::new(IpLimiter::new(
        Duration::from_secs(60),
        cfg.connections_per_ip_per_minute,
    ));
    let mut shutdown = std::pin::pin!(shutdown);
    loop {
        let (stream, peer) = tokio::select! {
            r = listener.accept() => r?,
            () = &mut shutdown => {
                info!("smtp listener stopping");
                return Ok(());
            }
        };
        let Ok(permit) = sem.clone().try_acquire_owned() else {
            warn!(%peer, "connection cap reached; refusing");
            let mut s = stream;
            let _ = s
                .write_all(
                    Reply::new(421, "4.3.2 too many connections, try later")
                        .to_wire()
                        .as_bytes(),
                )
                .await;
            continue;
        };
        if !limiter.allow(peer.ip()) {
            debug!(%peer, "per-ip connection rate exceeded");
            let mut s = stream;
            let _ = s
                .write_all(
                    Reply::new(421, "4.7.0 too many connections from your address")
                        .to_wire()
                        .as_bytes(),
                )
                .await;
            continue;
        }
        let handler = handler.clone();
        let hostname = cfg.hostname.clone();
        let limits = cfg.limits.clone();
        let idle = cfg.idle_timeout;
        tokio::spawn(async move {
            let _permit = permit;
            let peer_s = peer.to_string();
            if let Err(e) =
                serve_connection(stream, &peer_s, &hostname, limits, idle, handler).await
            {
                debug!(peer = %peer_s, error = %e, "smtp connection ended with error");
            }
        });
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

    fn limits() -> Limits {
        Limits {
            max_message_bytes: 200,
            max_recipients: 2,
        }
    }

    fn session() -> Session {
        Session::new("mx.hashgram.io", limits(), "127.0.0.1:1")
    }

    fn reply_code(a: &Action) -> u16 {
        match a {
            Action::Reply(r) | Action::Close(r) => r.code,
            other => panic!("expected a reply, got {other:?}"),
        }
    }

    #[test]
    fn happy_path_with_dot_stuffing() {
        let mut s = session();
        assert_eq!(s.banner().code, 220);
        let a = s.command(b"EHLO client.example.com");
        match a {
            Action::Reply(r) => {
                assert_eq!(r.code, 250);
                assert!(r.lines.iter().any(|l| l == "SIZE 200"));
                assert!(r.lines.iter().any(|l| l == "8BITMIME"));
            }
            _ => panic!(),
        }
        assert_eq!(
            reply_code(&s.command(b"MAIL FROM:<bob@example.com> SIZE=100")),
            250
        );
        assert_eq!(
            s.command(b"RCPT TO:<alice@hashgram.io>"),
            Action::CheckRecipient("alice@hashgram.io".into())
        );
        assert_eq!(
            s.recipient_verdict("alice@hashgram.io".into(), Verdict::Accept)
                .code,
            250
        );
        assert_eq!(reply_code(&s.command(b"DATA")), 354);
        assert!(s.in_data());
        assert_eq!(s.data_line(b"Subject: hi"), Action::Continue);
        assert_eq!(s.data_line(b""), Action::Continue);
        assert_eq!(s.data_line(b"..leading dot"), Action::Continue);
        assert_eq!(s.data_line(b".not the end"), Action::Continue);
        match s.data_line(b".") {
            Action::Message(env, raw) => {
                assert_eq!(env.mail_from, "bob@example.com");
                assert_eq!(env.rcpt_to, vec!["alice@hashgram.io"]);
                assert_eq!(env.helo, "client.example.com");
                assert_eq!(raw, b"Subject: hi\r\n\r\n.leading dot\r\nnot the end\r\n");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(s.message_verdict(Verdict::Accept).code, 250);
        assert!(!s.in_data());
        // A second transaction on the same connection works.
        assert_eq!(
            reply_code(&s.command(b"mail from: <carol@example.com>")),
            250
        );
        assert_eq!(reply_code(&s.command(b"QUIT")), 221);
    }

    #[test]
    fn null_sender_and_source_routes() {
        let mut s = session();
        s.command(b"HELO x");
        assert_eq!(reply_code(&s.command(b"MAIL FROM:<>")), 250);
        assert_eq!(
            s.command(b"RCPT TO:<@relay.example.com:alice@hashgram.io>"),
            Action::CheckRecipient("alice@hashgram.io".into())
        );
    }

    #[test]
    fn bad_sequences() {
        let mut s = session();
        assert_eq!(reply_code(&s.command(b"MAIL FROM:<a@b.c>")), 503);
        assert_eq!(reply_code(&s.command(b"RCPT TO:<a@b.c>")), 503);
        assert_eq!(reply_code(&s.command(b"DATA")), 503);
        s.command(b"EHLO x");
        assert_eq!(reply_code(&s.command(b"DATA")), 503);
        assert_eq!(reply_code(&s.command(b"RCPT TO:<a@hashgram.io>")), 503);
        s.command(b"MAIL FROM:<a@b.c>");
        assert_eq!(reply_code(&s.command(b"MAIL FROM:<a@b.c>")), 503);
        assert_eq!(reply_code(&s.command(b"DATA")), 503); // no RCPT yet
        assert_eq!(reply_code(&s.command(b"RSET")), 250);
        assert_eq!(reply_code(&s.command(b"DATA")), 503);
        assert_eq!(reply_code(&s.command(b"BOGUS")), 500);
        assert_eq!(reply_code(&s.command(b"STARTTLS")), 502);
        assert_eq!(reply_code(&s.command(b"AUTH PLAIN")), 502);
        assert_eq!(reply_code(&s.command(b"MAIL FROM:junk")), 501);
        assert_eq!(reply_code(&s.command(b"MAIL FROM:<a@b.c> FOO=bar")), 555);
        assert_eq!(reply_code(&s.command(b"NOOP")), 250);
        assert_eq!(reply_code(&s.command(b"EHLO")), 501);
    }

    #[test]
    fn size_limits() {
        let mut s = session();
        s.command(b"EHLO x");
        assert_eq!(reply_code(&s.command(b"MAIL FROM:<a@b.c> SIZE=201")), 552);
        s.command(b"MAIL FROM:<a@b.c>");
        s.command(b"RCPT TO:<alice@hashgram.io>");
        s.recipient_verdict("alice@hashgram.io".into(), Verdict::Accept);
        s.command(b"DATA");
        let big = vec![b'x'; 150];
        assert_eq!(s.data_line(&big), Action::Continue);
        assert_eq!(s.data_line(&big), Action::Continue); // crosses 200
        assert_eq!(s.data_line(b"more"), Action::Continue);
        assert_eq!(reply_code(&s.data_line(b".")), 552);
        assert!(!s.in_data());
        // Connection remains usable.
        assert_eq!(reply_code(&s.command(b"MAIL FROM:<a@b.c>")), 250);
    }

    #[test]
    fn recipient_limits_and_rejections() {
        let mut s = session();
        s.command(b"EHLO x");
        s.command(b"MAIL FROM:<a@b.c>");
        s.command(b"RCPT TO:<nobody@hashgram.io>");
        let r = s.recipient_verdict(
            "nobody@hashgram.io".into(),
            Verdict::reject("5.1.1 no such user"),
        );
        assert_eq!(r.code, 550);
        assert_eq!(reply_code(&s.command(b"DATA")), 503); // rejected rcpt does not count
        for name in ["a1", "a2"] {
            s.command(format!("RCPT TO:<{name}@hashgram.io>").as_bytes());
            s.recipient_verdict(format!("{name}@hashgram.io"), Verdict::Accept);
        }
        assert_eq!(reply_code(&s.command(b"RCPT TO:<a3@hashgram.io>")), 452);
        s.command(b"DATA");
        s.data_line(b"x");
        match s.data_line(b".") {
            Action::Message(env, _) => assert_eq!(env.rcpt_to.len(), 2),
            _ => panic!(),
        }
        assert_eq!(
            s.message_verdict(Verdict::temp_fail("4.3.0 try later"))
                .code,
            451
        );
    }

    #[test]
    fn too_many_errors_closes() {
        let mut s = session();
        let mut last = Action::Continue;
        for _ in 0..MAX_ERRORS {
            last = s.command(b"WHAT");
        }
        assert!(matches!(last, Action::Close(r) if r.code == 421));
    }

    #[test]
    fn path_arg_parsing() {
        assert_eq!(
            parse_path_arg("FROM:<a@b.c>", "FROM:"),
            Some(("a@b.c", vec![]))
        );
        assert_eq!(
            parse_path_arg("from: <a@b.c> SIZE=5", "FROM:"),
            Some(("a@b.c", vec!["SIZE=5"]))
        );
        assert_eq!(parse_path_arg("TO:a@b.c", "TO:"), Some(("a@b.c", vec![])));
        assert_eq!(
            parse_path_arg("TO:<@r1,@r2:a@b.c>", "TO:"),
            Some(("a@b.c", vec![]))
        );
        assert_eq!(parse_path_arg("FROM:<>", "FROM:"), Some(("", vec![])));
        assert_eq!(parse_path_arg("FROM:<a@b.c", "FROM:"), None);
        assert_eq!(parse_path_arg("TO:<a@b.c>", "FROM:"), None);
    }

    struct EchoHandler;
    impl InboundHandler for EchoHandler {
        async fn check_recipient(&self, rcpt: &str, _e: &Envelope) -> Verdict {
            if rcpt.ends_with("@hashgram.io") {
                Verdict::Accept
            } else {
                Verdict::reject("5.7.1 relay denied")
            }
        }
        async fn accept_message(&self, _e: Envelope, raw: Vec<u8>) -> Verdict {
            if raw.windows(4).any(|w| w == b"FAIL") {
                Verdict::TempFail(451, "later".into())
            } else {
                Verdict::Accept
            }
        }
    }

    #[tokio::test]
    async fn driver_round_trip_over_duplex() {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let srv = tokio::spawn(serve_connection(
            server,
            "test",
            "mx.test",
            Limits {
                max_message_bytes: 10_000,
                max_recipients: 5,
            },
            Duration::from_secs(5),
            Arc::new(EchoHandler),
        ));
        let (mut rd, mut wr) = tokio::io::split(client);
        let mut out = Vec::new();
        let mut tmp = [0u8; 4096];
        // Pipelined burst: everything at once, including the message.
        wr.write_all(
            b"EHLO c\r\nMAIL FROM:<x@example.com>\r\nRCPT TO:<alice@hashgram.io>\r\nRCPT TO:<bob@other.org>\r\nDATA\r\nSubject: t\r\n\r\nbody\r\n.\r\nQUIT\r\n",
        )
        .await
        .unwrap();
        loop {
            let n = rd.read(&mut tmp).await.unwrap();
            if n == 0 {
                break;
            }
            out.extend_from_slice(&tmp[..n]);
        }
        let text = String::from_utf8(out).unwrap();
        let codes: Vec<&str> = text
            .lines()
            .filter(|l| l.len() >= 4 && &l[3..4] == " ")
            .map(|l| &l[..3])
            .collect();
        assert_eq!(
            codes,
            vec!["220", "250", "250", "250", "550", "354", "250", "221"]
        );
        srv.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn listener_accepts_limits_and_stops() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let cfg = ServerConfig {
            listen: addr,
            hostname: "mx.test".into(),
            limits: Limits {
                max_message_bytes: 10_000,
                max_recipients: 5,
            },
            idle_timeout: Duration::from_secs(5),
            max_connections: 10,
            connections_per_ip_per_minute: 2,
        };
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let srv = tokio::spawn(serve_listener(
            listener,
            cfg,
            Arc::new(EchoHandler),
            async move {
                let _ = rx.await;
            },
        ));
        async fn talk(addr: SocketAddr, script: &[u8]) -> String {
            let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
            s.write_all(script).await.unwrap();
            let mut out = Vec::new();
            let mut tmp = [0u8; 4096];
            loop {
                match tokio::time::timeout(Duration::from_secs(5), s.read(&mut tmp)).await {
                    Ok(Ok(0)) | Err(_) | Ok(Err(_)) => break,
                    Ok(Ok(n)) => out.extend_from_slice(&tmp[..n]),
                }
            }
            String::from_utf8_lossy(&out).into_owned()
        }
        let first = talk(addr, b"EHLO a\r\nQUIT\r\n").await;
        assert!(first.starts_with("220 mx.test"));
        assert!(first.contains("221 "));
        let second = talk(addr, b"QUIT\r\n").await;
        assert!(second.contains("221 "));
        // Third connection within the minute from the same IP is refused.
        let third = talk(addr, b"QUIT\r\n").await;
        assert!(third.starts_with("421 "), "{third}");
        let _ = tx.send(());
        srv.await.unwrap().unwrap();
    }
}
