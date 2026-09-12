//! SMTP (RFC 5321) on both sides of the bridge.
//!
//! * [`server`]: the inbound listener the Internet (or the fronting MTA)
//!   talks to. A hand-written command state machine, because the surface
//!   we need is tiny (no AUTH, no STARTTLS, no extensions beyond SIZE and
//!   8BITMIME) and every line is attacker-controlled: the machine is pure
//!   and unit-tested line by line.
//! * [`client`]: the outbound sender that talks to a recipient's MX with
//!   opportunistic STARTTLS.
//!
//! Both share the reply type and the line/dot-stuffing helpers here.

pub mod client;
pub mod server;

use std::fmt;

/// Longest command line we accept (RFC 5321 §4.5.3.1.4 says 512 for
/// commands; 1000 gives room for long paths with parameters).
pub const MAX_COMMAND_LINE: usize = 1000;

/// An SMTP reply: one code, one or more text lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    /// Three-digit code.
    pub code: u16,
    /// Text lines (without the code); at least one.
    pub lines: Vec<String>,
}

impl Reply {
    /// Single-line reply.
    #[must_use]
    pub fn new(code: u16, text: impl Into<String>) -> Self {
        Self {
            code,
            lines: vec![text.into()],
        }
    }

    /// Multi-line reply.
    #[must_use]
    pub fn multi(code: u16, lines: Vec<String>) -> Self {
        let lines = if lines.is_empty() {
            vec![String::new()]
        } else {
            lines
        };
        Self { code, lines }
    }

    /// 2xx or 3xx.
    #[must_use]
    pub fn is_positive(&self) -> bool {
        (200..400).contains(&self.code)
    }

    /// 4xx: try again later.
    #[must_use]
    pub fn is_transient(&self) -> bool {
        (400..500).contains(&self.code)
    }

    /// 5xx: never.
    #[must_use]
    pub fn is_permanent(&self) -> bool {
        self.code >= 500
    }

    /// Wire form: `250-first\r\n250 last\r\n`.
    #[must_use]
    pub fn to_wire(&self) -> String {
        let mut out = String::new();
        let last = self.lines.len().saturating_sub(1);
        for (i, l) in self.lines.iter().enumerate() {
            let sep = if i == last { ' ' } else { '-' };
            out.push_str(&format!(
                "{}{}{}\r\n",
                self.code,
                sep,
                sanitise_reply_text(l)
            ));
        }
        out
    }
}

impl fmt::Display for Reply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.code, self.lines.join(" / "))
    }
}

/// Reply text must not contain CR/LF (it would forge extra replies) or
/// control characters.
fn sanitise_reply_text(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// Splits a buffer at the first line terminator (`\r\n`, or a bare `\n`
/// which real-world clients send and every MTA tolerates). Returns the
/// line without its terminator and the number of bytes consumed.
#[must_use]
pub fn split_line(buf: &[u8]) -> Option<(&[u8], usize)> {
    let nl = buf.iter().position(|&b| b == b'\n')?;
    let line = buf.get(..nl)?;
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    Some((line, nl + 1))
}

/// Applies RFC 5321 §4.5.2 dot-stuffing to a message for `DATA`: every
/// line that begins with `.` gets a second one, line endings are
/// normalised to CRLF, and the result ends with `\r\n.\r\n`.
#[must_use]
pub fn dot_stuff(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 64);
    let mut rest = body;
    loop {
        let (line, consumed, done) = match rest.iter().position(|&b| b == b'\n') {
            Some(i) => {
                let l = rest.get(..i).unwrap_or(&[]);
                (l.strip_suffix(b"\r").unwrap_or(l), i + 1, false)
            }
            None => (rest, rest.len(), true),
        };
        if !(done && line.is_empty()) {
            if line.first() == Some(&b'.') {
                out.push(b'.');
            }
            out.extend_from_slice(line);
            out.extend_from_slice(b"\r\n");
        }
        if done {
            break;
        }
        rest = rest.get(consumed..).unwrap_or(&[]);
    }
    out.extend_from_slice(b".\r\n");
    out
}

/// Parses one reply line: `250-text`, `250 text`, or a bare `250`.
/// Returns (code, is_last, text).
#[must_use]
pub fn parse_reply_line(line: &str) -> Option<(u16, bool, &str)> {
    let code_str = line.get(..3)?;
    if !code_str.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let code: u16 = code_str.parse().ok()?;
    match line.as_bytes().get(3) {
        None => Some((code, true, "")),
        Some(b' ') => Some((code, true, line.get(4..).unwrap_or(""))),
        Some(b'-') => Some((code, false, line.get(4..).unwrap_or(""))),
        _ => None,
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

    #[test]
    fn reply_wire_form() {
        let r = Reply::multi(250, vec!["a".into(), "b\r\nc".into()]);
        assert_eq!(r.to_wire(), "250-a\r\n250 b  c\r\n");
        assert_eq!(Reply::new(221, "bye").to_wire(), "221 bye\r\n");
        assert!(Reply::new(354, "").is_positive());
        assert!(Reply::new(451, "").is_transient());
        assert!(Reply::new(550, "").is_permanent());
    }

    #[test]
    fn line_splitting_tolerates_bare_lf() {
        assert_eq!(split_line(b"HELO x\r\nMAIL"), Some((&b"HELO x"[..], 8)));
        assert_eq!(split_line(b"HELO x\nMAIL"), Some((&b"HELO x"[..], 7)));
        assert_eq!(split_line(b"partial"), None);
    }

    #[test]
    fn dot_stuffing() {
        assert_eq!(dot_stuff(b"a\n.b\r\n..c\n"), b"a\r\n..b\r\n...c\r\n.\r\n");
        assert_eq!(dot_stuff(b"no newline"), b"no newline\r\n.\r\n");
        assert_eq!(dot_stuff(b""), b".\r\n");
        assert_eq!(dot_stuff(b"."), b"..\r\n.\r\n");
    }

    #[test]
    fn reply_line_parsing() {
        assert_eq!(
            parse_reply_line("250-mx.example.com"),
            Some((250, false, "mx.example.com"))
        );
        assert_eq!(parse_reply_line("250 OK"), Some((250, true, "OK")));
        assert_eq!(parse_reply_line("220"), Some((220, true, "")));
        assert_eq!(parse_reply_line("25x OK"), None);
        assert_eq!(parse_reply_line("250xOK"), None);
    }
}
