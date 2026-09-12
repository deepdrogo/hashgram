//! RFC 5322 / MIME on both directions of the bridge.
//!
//! * [`inbound`]: Internet message bytes → a structured [`inbound::InboundMail`]
//!   the bridge turns into a `hashgram_app::mail::Draft`. Uses `mailparse`.
//! * [`outbound`]: a native `MailMessage` → Internet message bytes. Written
//!   here rather than pulled from a crate because the surface is small,
//!   deterministic output matters for DKIM, and every byte we emit ought to
//!   be understood by whoever reviews this file.
//! * [`html`]: HTML → plain text for messages without a `text/plain` part.
//!
//! Shared encoders (RFC 2047 encoded-words, quoted-printable, base64 line
//! folding, RFC 5322 dates) live here with known-answer tests.

pub mod html;
pub mod inbound;
pub mod outbound;

use base64::Engine;

/// RFC 5322 §2.1.1: lines SHOULD be ≤ 78, MUST be ≤ 998 characters.
pub const SOFT_LINE: usize = 76;

/// Whether a string is safe to emit as a bare header value (printable
/// ASCII, no CR/LF).
#[must_use]
pub fn is_ascii_header_safe(s: &str) -> bool {
    s.bytes().all(|b| (0x20..0x7f).contains(&b) || b == b'\t')
}

/// RFC 2047 `B` encoding of a UTF-8 string, split into ≤ 75-byte
/// encoded-words joined by folding whitespace, so long subjects in any
/// script survive every MTA. ASCII-safe input is returned unchanged.
#[must_use]
pub fn encode_header_value(s: &str) -> String {
    if is_ascii_header_safe(s) {
        return s.to_owned();
    }
    // 75 total = 12 for "=?UTF-8?B?" + "?=" leaves 63 base64 chars = 45 bytes.
    const CHUNK: usize = 45;
    let mut words = Vec::new();
    let mut cur = String::new();
    for ch in s.chars() {
        if cur.len() + ch.len_utf8() > CHUNK {
            words.push(std::mem::take(&mut cur));
        }
        cur.push(ch);
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
        .iter()
        .map(|w| {
            format!(
                "=?UTF-8?B?{}?=",
                base64::engine::general_purpose::STANDARD.encode(w.as_bytes())
            )
        })
        .collect::<Vec<_>>()
        .join("\r\n ")
}

/// A display name for an address header: quoted-string when ASCII (with
/// `"` and `\` escaped), encoded-word otherwise. Empty → empty.
#[must_use]
pub fn encode_display_name(name: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        return String::new();
    }
    if is_ascii_header_safe(name) {
        let escaped: String = name
            .chars()
            .flat_map(|c| match c {
                '"' | '\\' => vec!['\\', c],
                _ => vec![c],
            })
            .collect();
        format!("\"{escaped}\"")
    } else {
        encode_header_value(name)
    }
}

/// `"Name" <addr>` or `<addr>`.
#[must_use]
pub fn format_mailbox(name: &str, addr: &str) -> String {
    let n = encode_display_name(name);
    if n.is_empty() {
        format!("<{addr}>")
    } else {
        format!("{n} <{addr}>")
    }
}

/// Quoted-printable (RFC 2045 §6.7) with soft line breaks at 76 columns.
/// Input line endings (`\n` or `\r\n`) become CRLF; trailing whitespace on
/// a line is encoded so it survives transport.
#[must_use]
pub fn quoted_printable(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len() + input.len().div_euclid(8));
    let text = input.strip_suffix(b"\n").unwrap_or(input);
    let lines: Vec<&[u8]> = text.split(|&b| b == b'\n').collect();
    let last = lines.len().saturating_sub(1);
    for (i, line) in lines.iter().enumerate() {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let mut col = 0usize;
        let n = line.len();
        for (j, &b) in line.iter().enumerate() {
            let is_last_in_line = j + 1 == n;
            let literal = ((b == b' ' || b == b'\t') && !is_last_in_line)
                || ((0x21..=0x7e).contains(&b) && b != b'=');
            let piece = if literal {
                String::from(b as char)
            } else {
                format!("={b:02X}")
            };
            if col + piece.len() > SOFT_LINE - 1 {
                out.push_str("=\r\n");
                col = 0;
            }
            out.push_str(&piece);
            col += piece.len();
        }
        if i != last || input.ends_with(b"\n") {
            out.push_str("\r\n");
        }
    }
    out
}

/// Base64 folded at 76 columns with CRLF, as MIME bodies want.
#[must_use]
pub fn base64_folded(data: &[u8]) -> String {
    let enc = base64::engine::general_purpose::STANDARD.encode(data);
    let mut out = String::with_capacity(enc.len() + enc.len().div_euclid(38) + 2);
    let bytes = enc.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let end = (i + SOFT_LINE).min(bytes.len());
        if let Ok(s) = std::str::from_utf8(bytes.get(i..end).unwrap_or(&[])) {
            out.push_str(s);
        }
        out.push_str("\r\n");
        i = end;
    }
    out
}

/// Days since 1970-01-01 → (year, month, day). Howard Hinnant's
/// `civil_from_days`; exact for the whole proleptic Gregorian range we
/// care about. Calendar arithmetic is integer division by definition.
#[must_use]
#[allow(clippy::integer_division)]
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// RFC 5322 §3.3 date-time in UTC, e.g. `Sat, 12 Sep 2026 18:25:00 +0000`.
#[must_use]
#[allow(clippy::integer_division)] // seconds → h:m:s is integer arithmetic
pub fn rfc5322_date(unix_ms: u64) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let secs = (unix_ms / 1000) as i64;
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let dow = DAYS
        .get(days.rem_euclid(7) as usize)
        .copied()
        .unwrap_or("Thu");
    let mon = MONTHS
        .get((m as usize).saturating_sub(1))
        .copied()
        .unwrap_or("Jan");
    format!(
        "{dow}, {d:02} {mon} {y} {:02}:{:02}:{:02} +0000",
        sod / 3600,
        (sod % 3600) / 60,
        sod % 60
    )
}

/// A file name safe for `MailAttachment.name`: no path separators, no
/// control characters, bounded, never empty.
#[must_use]
pub fn safe_filename(name: &str, fallback: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| if c == '/' || c == '\\' { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim();
    let cleaned = if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        fallback
    } else {
        cleaned
    };
    let mut out = String::new();
    for ch in cleaned.chars() {
        if out.len() + ch.len_utf8() > hashgram_app::mail::MAX_FILENAME {
            break;
        }
        out.push(ch);
    }
    out
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
    fn encoded_words() {
        assert_eq!(encode_header_value("Hello"), "Hello");
        let e = encode_header_value("გამარჯობა მსოფლიო");
        assert!(e.starts_with("=?UTF-8?B?") && e.ends_with("?="));
        for w in e.split("\r\n ") {
            assert!(w.len() <= 75, "{w}");
        }
        // mailparse decodes it back.
        let raw = format!("Subject: {e}");
        let (h, _) = mailparse::parse_header(raw.as_bytes()).unwrap();
        assert_eq!(h.get_value(), "გამარჯობა მსოფლიო");
        let long = "ა".repeat(100);
        let raw = format!("Subject: {}", encode_header_value(&long));
        let (h, _) = mailparse::parse_header(raw.as_bytes()).unwrap();
        assert_eq!(h.get_value(), long);
    }

    #[test]
    fn display_names_and_mailboxes() {
        assert_eq!(
            encode_display_name(r#"Al "The" Ice\"#),
            r#""Al \"The\" Ice\\""#
        );
        assert_eq!(format_mailbox("", "a@b.c"), "<a@b.c>");
        assert_eq!(format_mailbox("Alice", "a@b.c"), "\"Alice\" <a@b.c>");
        // Encoded-word display names are decoded by the header-aware
        // parser (a receiver reads them from a header, never bare).
        let raw = format!("To: {}", format_mailbox("ალისა", "a@b.c"));
        let (h, _) = mailparse::parse_header(raw.as_bytes()).unwrap();
        let list = mailparse::addrparse_header(&h).unwrap();
        match &list[0] {
            mailparse::MailAddr::Single(s) => {
                assert_eq!(s.addr, "a@b.c");
                assert_eq!(s.display_name.as_deref(), Some("ალისა"));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn quoted_printable_known_answers() {
        assert_eq!(quoted_printable(b"hello\n"), "hello\r\n");
        assert_eq!(quoted_printable(b"a=b"), "a=3Db");
        assert_eq!(quoted_printable(b"trailing \nx"), "trailing=20\r\nx");
        assert_eq!(quoted_printable("é".as_bytes()), "=C3=A9");
        let long = "x".repeat(200);
        let qp = quoted_printable(long.as_bytes());
        for l in qp.split("\r\n") {
            assert!(l.len() <= 76, "{}", l.len());
        }
        // Decodes back through mailparse.
        let msg = format!(
            "Content-Type: text/plain\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n{qp}"
        );
        assert_eq!(
            mailparse::parse_mail(msg.as_bytes())
                .unwrap()
                .get_body()
                .unwrap(),
            long
        );
    }

    #[test]
    fn base64_folding_round_trip() {
        let data: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        let b = base64_folded(&data);
        for l in b.trim_end().split("\r\n") {
            assert!(l.len() <= 76);
        }
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(b.replace("\r\n", ""))
            .unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn dates() {
        assert_eq!(rfc5322_date(0), "Thu, 01 Jan 1970 00:00:00 +0000");
        assert_eq!(
            rfc5322_date(951_782_400_000),
            "Tue, 29 Feb 2000 00:00:00 +0000"
        );
        assert_eq!(
            rfc5322_date(1_789_237_500_000),
            "Sat, 12 Sep 2026 18:25:00 +0000"
        );
        assert_eq!(
            mailparse::dateparse(&rfc5322_date(1_789_237_500_000)).unwrap(),
            1_789_237_500
        );
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }

    #[test]
    fn filenames() {
        assert_eq!(safe_filename("../evil\\x.pdf", "f"), ".._evil_x.pdf");
        assert_eq!(safe_filename("  ", "part-1.bin"), "part-1.bin");
        assert_eq!(safe_filename("..", "f"), "f");
        assert!(safe_filename(&"ა".repeat(300), "f").len() <= hashgram_app::mail::MAX_FILENAME);
    }
}
