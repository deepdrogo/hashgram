//! The outbound SMTP client: one message to one MX.
//!
//! Internet mail transport is *opportunistic* TLS (RFC 3207): if the MX
//! advertises `STARTTLS` we upgrade and verify its certificate against the
//! Mozilla root set (`webpki-roots`); if it does not, or the handshake or
//! verification fails, we deliver in plaintext unless the operator set
//! `outbound.require_tls`. That matches what every MTA does and what
//! recipients expect — a hard TLS requirement bounces mail to the long
//! tail of legacy MXs. The choice is recorded in the returned
//! [`Delivered::tls`] so the log shows how each message travelled.
//!
//! The protocol is driven over any byte stream so the transaction can be
//! tested against an in-memory fake MX; the TCP + TLS plumbing sits in
//! [`deliver`].

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::debug;

use super::{dot_stuff, parse_reply_line, split_line, Reply};

/// Client settings.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Our name in `EHLO`.
    pub helo_hostname: String,
    /// Per-step timeout.
    pub timeout: Duration,
    /// Refuse plaintext delivery.
    pub require_tls: bool,
}

/// Why a delivery attempt did not succeed.
#[derive(Debug, Clone, thiserror::Error)]
pub enum DeliveryError {
    /// Connection-level failure: retry later.
    #[error("connection: {0}")]
    Io(String),
    /// Remote said 4xx: retry later.
    #[error("transient: {0}")]
    Transient(Reply),
    /// Remote said 5xx: never retry.
    #[error("permanent: {0}")]
    Permanent(Reply),
    /// TLS required but unavailable or failed.
    #[error("tls: {0}")]
    Tls(String),
    /// The remote spoke something that is not SMTP.
    #[error("protocol: {0}")]
    Protocol(String),
}

impl DeliveryError {
    /// Whether retrying can never help.
    #[must_use]
    pub fn is_permanent(&self) -> bool {
        matches!(self, Self::Permanent(_))
    }
}

/// A successful delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivered {
    /// The final `250` line from the MX (its queue id, usually).
    pub reply: Reply,
    /// Whether the message travelled under TLS.
    pub tls: bool,
    /// Recipients the MX refused permanently while accepting the rest.
    pub rejected: Vec<(String, Reply)>,
}

/// Extensions we care about from the `EHLO` reply.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Extensions {
    /// `STARTTLS` offered.
    pub starttls: bool,
    /// `SIZE` extension offered (we may send `SIZE=` on `MAIL FROM`).
    pub size_supported: bool,
    /// `SIZE n` limit, if announced with a non-zero value.
    pub size: Option<u64>,
    /// `8BITMIME` offered.
    pub eight_bit_mime: bool,
}

/// Parses `EHLO` reply lines into [`Extensions`].
#[must_use]
pub fn parse_extensions(reply: &Reply) -> Extensions {
    let mut ext = Extensions::default();
    for line in reply.lines.iter().skip(1) {
        let mut it = line.split_whitespace();
        match it.next().map(str::to_ascii_uppercase).as_deref() {
            Some("STARTTLS") => ext.starttls = true,
            Some("8BITMIME") => ext.eight_bit_mime = true,
            Some("SIZE") => {
                ext.size_supported = true;
                if let Some(n) = it
                    .next()
                    .and_then(|v| v.parse::<u64>().ok())
                    .filter(|n| *n > 0)
                {
                    ext.size = Some(n);
                }
            }
            _ => {}
        }
    }
    ext
}

/// A line-oriented SMTP connection over any stream.
struct Conn<S> {
    stream: S,
    buf: Vec<u8>,
    timeout: Duration,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Conn<S> {
    fn new(stream: S, timeout: Duration) -> Self {
        Self {
            stream,
            buf: Vec::with_capacity(4096),
            timeout,
        }
    }

    async fn send(&mut self, line: &str) -> Result<(), DeliveryError> {
        let mut bytes = line.as_bytes().to_vec();
        bytes.extend_from_slice(b"\r\n");
        tokio::time::timeout(self.timeout, self.stream.write_all(&bytes))
            .await
            .map_err(|_| DeliveryError::Io("write timeout".into()))?
            .map_err(|e| DeliveryError::Io(e.to_string()))
    }

    async fn send_raw(&mut self, bytes: &[u8]) -> Result<(), DeliveryError> {
        tokio::time::timeout(self.timeout, self.stream.write_all(bytes))
            .await
            .map_err(|_| DeliveryError::Io("write timeout".into()))?
            .map_err(|e| DeliveryError::Io(e.to_string()))
    }

    async fn read_line(&mut self) -> Result<String, DeliveryError> {
        let mut chunk = [0u8; 4096];
        loop {
            if let Some((line, used)) = split_line(&self.buf).map(|(l, n)| (l.to_vec(), n)) {
                self.buf.drain(..used);
                return String::from_utf8(line)
                    .map_err(|_| DeliveryError::Protocol("non-UTF-8 reply".into()));
            }
            if self.buf.len() > 64 * 1024 {
                return Err(DeliveryError::Protocol("reply line too long".into()));
            }
            let n = tokio::time::timeout(self.timeout, self.stream.read(&mut chunk))
                .await
                .map_err(|_| DeliveryError::Io("read timeout".into()))?
                .map_err(|e| DeliveryError::Io(e.to_string()))?;
            if n == 0 {
                return Err(DeliveryError::Io("connection closed by remote".into()));
            }
            self.buf.extend_from_slice(chunk.get(..n).unwrap_or(&[]));
        }
    }

    /// Reads one (possibly multi-line) reply.
    async fn read_reply(&mut self) -> Result<Reply, DeliveryError> {
        let mut lines = Vec::new();
        let mut code = 0u16;
        loop {
            let line = self.read_line().await?;
            let (c, last, text) = parse_reply_line(&line)
                .ok_or_else(|| DeliveryError::Protocol(format!("bad reply line {line:?}")))?;
            if code != 0 && c != code {
                return Err(DeliveryError::Protocol(
                    "inconsistent multi-line reply codes".into(),
                ));
            }
            code = c;
            lines.push(text.to_owned());
            if last {
                return Ok(Reply { code, lines });
            }
            if lines.len() > 100 {
                return Err(DeliveryError::Protocol("multi-line reply too long".into()));
            }
        }
    }

    /// Sends a command and classifies the reply.
    async fn cmd(&mut self, line: &str, expect: u16) -> Result<Reply, DeliveryError> {
        self.send(line).await?;
        let r = self.read_reply().await?;
        classify(r, expect)
    }

    fn into_inner(self) -> S {
        self.stream
    }
}

fn classify(r: Reply, expect: u16) -> Result<Reply, DeliveryError> {
    if r.code == expect || (expect == 250 && r.code == 251) {
        Ok(r)
    } else if r.is_transient() {
        Err(DeliveryError::Transient(r))
    } else if r.is_permanent() {
        Err(DeliveryError::Permanent(r))
    } else {
        Err(DeliveryError::Protocol(format!("unexpected reply {r}")))
    }
}

/// The outcome of the greeting + EHLO phase.
struct Greeted<S> {
    conn: Conn<S>,
    ext: Extensions,
}

async fn greet<S: AsyncRead + AsyncWrite + Unpin>(
    mut conn: Conn<S>,
    helo: &str,
) -> Result<Greeted<S>, DeliveryError> {
    let banner = conn.read_reply().await?;
    classify(banner, 220)?;
    let ehlo = match conn.cmd(&format!("EHLO {helo}"), 250).await {
        Ok(r) => r,
        Err(DeliveryError::Permanent(_)) => {
            // Ancient server without ESMTP.
            conn.cmd(&format!("HELO {helo}"), 250).await?
        }
        Err(e) => return Err(e),
    };
    let ext = parse_extensions(&ehlo);
    Ok(Greeted { conn, ext })
}

/// Runs `MAIL FROM` … `QUIT` on an already-greeted connection.
async fn transaction<S: AsyncRead + AsyncWrite + Unpin>(
    conn: &mut Conn<S>,
    ext: &Extensions,
    mail_from: &str,
    rcpts: &[String],
    raw: &[u8],
) -> Result<(Reply, Vec<(String, Reply)>), DeliveryError> {
    if let Some(limit) = ext.size {
        if raw.len() as u64 > limit {
            return Err(DeliveryError::Permanent(Reply::new(
                552,
                format!("message of {} bytes exceeds remote SIZE {limit}", raw.len()),
            )));
        }
    }
    let body_param = if ext.eight_bit_mime {
        " BODY=8BITMIME"
    } else {
        ""
    };
    let size_param = if ext.size_supported {
        format!(" SIZE={}", raw.len())
    } else {
        String::new()
    };
    conn.cmd(
        &format!("MAIL FROM:<{mail_from}>{size_param}{body_param}"),
        250,
    )
    .await?;
    let mut rejected = Vec::new();
    let mut accepted = 0usize;
    let mut last_transient: Option<Reply> = None;
    for r in rcpts {
        match conn.cmd(&format!("RCPT TO:<{r}>"), 250).await {
            Ok(_) => accepted += 1,
            Err(DeliveryError::Permanent(reply)) => rejected.push((r.clone(), reply)),
            Err(DeliveryError::Transient(reply)) => last_transient = Some(reply),
            Err(e) => return Err(e),
        }
    }
    if accepted == 0 {
        let _ = conn.send("QUIT").await;
        return match (rejected.pop(), last_transient) {
            (_, Some(t)) => Err(DeliveryError::Transient(t)),
            (Some((_, p)), None) => Err(DeliveryError::Permanent(p)),
            (None, None) => Err(DeliveryError::Protocol("no recipients".into())),
        };
    }
    conn.cmd("DATA", 354).await?;
    conn.send_raw(&dot_stuff(raw)).await?;
    let final_reply = conn.read_reply().await?;
    let final_reply = classify(final_reply, 250)?;
    let _ = conn.send("QUIT").await;
    let _ = tokio::time::timeout(Duration::from_secs(2), conn.read_reply()).await;
    Ok((final_reply, rejected))
}

/// Delivers over a caller-provided stream with no TLS upgrade (tests, or
/// a smarthost reached through a local tunnel).
pub async fn deliver_on<S: AsyncRead + AsyncWrite + Unpin>(
    cfg: &ClientConfig,
    stream: S,
    mail_from: &str,
    rcpts: &[String],
    raw: &[u8],
) -> Result<Delivered, DeliveryError> {
    let conn = Conn::new(stream, cfg.timeout);
    let Greeted { mut conn, ext } = greet(conn, &cfg.helo_hostname).await?;
    if cfg.require_tls {
        return Err(DeliveryError::Tls(
            "TLS required but this transport cannot upgrade".into(),
        ));
    }
    let (reply, rejected) = transaction(&mut conn, &ext, mail_from, rcpts, raw).await?;
    Ok(Delivered {
        reply,
        tls: false,
        rejected,
    })
}

fn tls_connector() -> Result<tokio_rustls::TlsConnector, DeliveryError> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| DeliveryError::Tls(e.to_string()))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(tokio_rustls::TlsConnector::from(Arc::new(config)))
}

async fn connect(host: &str, port: u16, timeout: Duration) -> Result<TcpStream, DeliveryError> {
    let stream = tokio::time::timeout(timeout, TcpStream::connect((host, port)))
        .await
        .map_err(|_| DeliveryError::Io(format!("connect {host}:{port} timed out")))?
        .map_err(|e| DeliveryError::Io(format!("connect {host}:{port}: {e}")))?;
    let _ = stream.set_nodelay(true);
    Ok(stream)
}

/// Connects to `host:port`, upgrades to TLS when offered (falling back to
/// plaintext unless `require_tls`), and runs the transaction.
pub async fn deliver(
    cfg: &ClientConfig,
    host: &str,
    port: u16,
    mail_from: &str,
    rcpts: &[String],
    raw: &[u8],
) -> Result<Delivered, DeliveryError> {
    let tcp = connect(host, port, cfg.timeout).await?;
    let Greeted { mut conn, ext } = greet(Conn::new(tcp, cfg.timeout), &cfg.helo_hostname).await?;

    if !ext.starttls {
        if cfg.require_tls {
            return Err(DeliveryError::Tls(format!(
                "{host} does not offer STARTTLS"
            )));
        }
        let (reply, rejected) = transaction(&mut conn, &ext, mail_from, rcpts, raw).await?;
        return Ok(Delivered {
            reply,
            tls: false,
            rejected,
        });
    }

    let tls_failure = match conn.cmd("STARTTLS", 220).await {
        Ok(_) => {
            let tcp = conn.into_inner();
            match upgrade(tcp, host, cfg.timeout).await {
                Ok(tls) => {
                    // RFC 3207 §4.2: the client MUST discard knowledge
                    // from before the upgrade and EHLO again.
                    let Greeted { mut conn, ext } =
                        greet_after_tls(Conn::new(tls, cfg.timeout), &cfg.helo_hostname).await?;
                    let (reply, rejected) =
                        transaction(&mut conn, &ext, mail_from, rcpts, raw).await?;
                    return Ok(Delivered {
                        reply,
                        tls: true,
                        rejected,
                    });
                }
                Err(e) => e.to_string(),
            }
        }
        Err(DeliveryError::Transient(r)) | Err(DeliveryError::Permanent(r)) => {
            format!("STARTTLS refused: {r}")
        }
        Err(e) => return Err(e),
    };

    if cfg.require_tls {
        return Err(DeliveryError::Tls(tls_failure));
    }
    debug!(
        host,
        reason = tls_failure,
        "TLS upgrade failed; falling back to plaintext"
    );
    // The old connection is in an undefined state after a failed upgrade:
    // reconnect and do not try STARTTLS again.
    let tcp = connect(host, port, cfg.timeout).await?;
    let Greeted { mut conn, ext } = greet(Conn::new(tcp, cfg.timeout), &cfg.helo_hostname).await?;
    let (reply, rejected) = transaction(&mut conn, &ext, mail_from, rcpts, raw).await?;
    Ok(Delivered {
        reply,
        tls: false,
        rejected,
    })
}

async fn upgrade(
    tcp: TcpStream,
    host: &str,
    timeout: Duration,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>, DeliveryError> {
    let connector = tls_connector()?;
    let name = rustls::pki_types::ServerName::try_from(host.to_owned())
        .map_err(|e| DeliveryError::Tls(format!("server name {host}: {e}")))?;
    tokio::time::timeout(timeout, connector.connect(name, tcp))
        .await
        .map_err(|_| DeliveryError::Tls("handshake timed out".into()))?
        .map_err(|e| DeliveryError::Tls(e.to_string()))
}

/// After STARTTLS there is no banner; only EHLO.
async fn greet_after_tls<S: AsyncRead + AsyncWrite + Unpin>(
    mut conn: Conn<S>,
    helo: &str,
) -> Result<Greeted<S>, DeliveryError> {
    let ehlo = conn.cmd(&format!("EHLO {helo}"), 250).await?;
    let ext = parse_extensions(&ehlo);
    Ok(Greeted { conn, ext })
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

    fn cfg() -> ClientConfig {
        ClientConfig {
            helo_hostname: "gw.hashgram.io".into(),
            timeout: Duration::from_secs(5),
            require_tls: false,
        }
    }

    /// A scripted MX: for each client line, the reply to send. Records
    /// everything it received.
    async fn fake_mx<S: AsyncRead + AsyncWrite + Unpin>(
        mut s: S,
        script: Vec<(&'static str, &'static str)>,
    ) -> Vec<String> {
        let mut seen = Vec::new();
        s.write_all(b"220 mx.example.com ESMTP\r\n").await.unwrap();
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        let mut in_data = false;
        let mut script = script.into_iter();
        loop {
            let n = match s.read(&mut tmp).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            buf.extend_from_slice(&tmp[..n]);
            while let Some((line, used)) = split_line(&buf).map(|(l, n)| (l.to_vec(), n)) {
                buf.drain(..used);
                let line = String::from_utf8(line).unwrap();
                if in_data {
                    if line == "." {
                        in_data = false;
                        seen.push("<DATA END>".into());
                        s.write_all(b"250 2.0.0 queued as ABC\r\n").await.unwrap();
                    } else {
                        seen.push(format!("D:{line}"));
                    }
                    continue;
                }
                seen.push(line.clone());
                let verb = line.split(' ').next().unwrap_or("").to_ascii_uppercase();
                if verb == "QUIT" {
                    let _ = s.write_all(b"221 bye\r\n").await;
                    return seen;
                }
                let reply = script
                    .next()
                    .map(|(expect, reply)| {
                        assert!(
                            line.to_ascii_uppercase()
                                .starts_with(&expect.to_ascii_uppercase()),
                            "expected {expect}, got {line}"
                        );
                        reply
                    })
                    .unwrap_or("250 OK");
                if verb == "DATA" && reply.starts_with("354") {
                    in_data = true;
                }
                s.write_all(reply.as_bytes()).await.unwrap();
                s.write_all(b"\r\n").await.unwrap();
            }
        }
        seen
    }

    #[tokio::test]
    async fn full_transaction_with_dot_stuffing_and_size() {
        let (a, b) = tokio::io::duplex(65536);
        let mx = tokio::spawn(fake_mx(
            b,
            vec![
                (
                    "EHLO",
                    "250-mx.example.com\r\n250-SIZE 1000\r\n250 8BITMIME",
                ),
                ("MAIL FROM:<alice@hashgram.io> SIZE=", "250 ok"),
                ("RCPT TO:<bob@example.com>", "250 ok"),
                ("RCPT TO:<nobody@example.com>", "550 no such user"),
                ("DATA", "354 go"),
            ],
        ));
        let d = deliver_on(
            &cfg(),
            a,
            "alice@hashgram.io",
            &["bob@example.com".into(), "nobody@example.com".into()],
            b"Subject: t\r\n\r\n.dot\r\nend",
        )
        .await
        .unwrap();
        assert_eq!(d.reply.code, 250);
        assert!(!d.tls);
        assert_eq!(d.rejected.len(), 1);
        assert_eq!(d.rejected[0].0, "nobody@example.com");
        let seen = mx.await.unwrap();
        assert_eq!(seen[0], "EHLO gw.hashgram.io");
        assert!(
            seen[1].starts_with("MAIL FROM:<alice@hashgram.io> SIZE=")
                && seen[1].ends_with(" BODY=8BITMIME")
        );
        assert!(seen.contains(&"D:..dot".to_owned()));
        assert!(seen.contains(&"D:end".to_owned()));
        assert_eq!(seen.last().unwrap(), "QUIT");
    }

    #[tokio::test]
    async fn permanent_and_transient_errors_are_classified() {
        let (a, b) = tokio::io::duplex(65536);
        tokio::spawn(fake_mx(
            b,
            vec![("EHLO", "250 mx"), ("MAIL", "550 5.1.8 sender rejected")],
        ));
        let e = deliver_on(
            &cfg(),
            a,
            "alice@hashgram.io",
            &["bob@example.com".into()],
            b"x",
        )
        .await
        .err()
        .unwrap();
        assert!(e.is_permanent());

        let (a, b) = tokio::io::duplex(65536);
        tokio::spawn(fake_mx(
            b,
            vec![
                ("EHLO", "250 mx"),
                ("MAIL", "250 ok"),
                ("RCPT", "451 greylisted"),
            ],
        ));
        let e = deliver_on(
            &cfg(),
            a,
            "alice@hashgram.io",
            &["bob@example.com".into()],
            b"x",
        )
        .await
        .err()
        .unwrap();
        assert!(matches!(e, DeliveryError::Transient(r) if r.code == 451));

        let (a, b) = tokio::io::duplex(65536);
        tokio::spawn(fake_mx(b, vec![("EHLO", "250-mx\r\n250 SIZE 5")]));
        let e = deliver_on(
            &cfg(),
            a,
            "alice@hashgram.io",
            &["bob@example.com".into()],
            b"more than five",
        )
        .await
        .err()
        .unwrap();
        assert!(matches!(e, DeliveryError::Permanent(r) if r.code == 552));
    }

    #[tokio::test]
    async fn require_tls_refuses_plain_transport() {
        let (a, b) = tokio::io::duplex(65536);
        tokio::spawn(fake_mx(b, vec![("EHLO", "250 mx")]));
        let mut c = cfg();
        c.require_tls = true;
        let e = deliver_on(
            &c,
            a,
            "alice@hashgram.io",
            &["bob@example.com".into()],
            b"x",
        )
        .await
        .err()
        .unwrap();
        assert!(matches!(e, DeliveryError::Tls(_)));
    }

    #[test]
    fn extension_parsing() {
        let r = Reply::multi(
            250,
            vec![
                "mx".into(),
                "SIZE 52428800".into(),
                "starttls".into(),
                "8BITMIME".into(),
                "SIZE".into(),
            ],
        );
        let e = parse_extensions(&r);
        assert!(e.starttls && e.eight_bit_mime && e.size_supported);
        assert_eq!(e.size, Some(52_428_800));
        let r = Reply::multi(250, vec!["mx".into(), "SIZE".into()]);
        let e = parse_extensions(&r);
        assert!(e.size_supported);
        assert_eq!(e.size, None);
        assert!(!parse_extensions(&Reply::new(250, "mx")).size_supported);
    }

    #[test]
    fn tls_connector_builds() {
        assert!(tls_connector().is_ok());
    }
}
