//! HTML → plain text, for Internet mail that ships only a `text/html` part.
//!
//! `MailMessage.body_text` is mandatory ("always present; HTML is optional
//! and derived") and readers without an HTML view show it, so the
//! conversion must be readable rather than faithful: block elements become
//! line breaks, `<script>`/`<style>` vanish, entities are decoded, link
//! targets are kept in angle brackets, runs of whitespace collapse. It is
//! not a sanitiser — the HTML part itself is kept in `body_html` and
//! readers MUST sanitise that (proto comment on `body_html`).

/// Converts an HTML fragment or document to plain text.
#[must_use]
pub fn html_to_text(html: &str) -> String {
    let mut c = Converter::default();
    c.run(html);
    collapse(&c.out)
}

#[derive(Default)]
struct Converter {
    out: String,
    pending_link: Option<String>,
}

const BLOCKS: &[&str] = &[
    "p",
    "div",
    "tr",
    "li",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "blockquote",
    "pre",
    "table",
    "ul",
    "ol",
    "hr",
    "section",
    "article",
    "header",
    "footer",
    "dd",
    "dt",
];

impl Converter {
    fn run(&mut self, html: &str) {
        let mut rest = html;
        while !rest.is_empty() {
            let Some(lt) = rest.find('<') else {
                self.text(rest);
                break;
            };
            self.text(rest.get(..lt).unwrap_or(""));
            let after_lt = rest.get(lt + 1..).unwrap_or("");
            if after_lt.starts_with("!--") {
                rest = match after_lt.find("-->") {
                    Some(i) => after_lt.get(i + 3..).unwrap_or(""),
                    None => "",
                };
                continue;
            }
            let Some(gt) = after_lt.find('>') else {
                // Unterminated tag: treat it and the remainder as text.
                self.text(rest.get(lt..).unwrap_or(""));
                break;
            };
            let tag = after_lt.get(..gt).unwrap_or("");
            rest = after_lt.get(gt + 1..).unwrap_or("");
            let closing = tag.starts_with('/');
            let name: String = tag
                .trim_start_matches('/')
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
                .to_ascii_lowercase();
            match name.as_str() {
                "script" | "style" | "head" | "title" if !closing => {
                    let close = format!("</{name}");
                    rest = match find_ci(rest, &close) {
                        Some(i) => {
                            let r = rest.get(i + close.len()..).unwrap_or("");
                            r.find('>').map_or("", |g| r.get(g + 1..).unwrap_or(""))
                        }
                        None => "",
                    };
                }
                // `<br>` is an explicit break: two of them are a blank
                // line, unlike adjacent block tags.
                "br" => self.out.push('\n'),
                "hr" => {
                    self.newline();
                    self.out.push_str("----");
                    self.newline();
                }
                "li" if !closing => {
                    self.newline();
                    self.out.push_str("- ");
                }
                "td" | "th" if closing => self.out.push(' '),
                "a" if !closing => {
                    if let Some(href) = attr(tag, "href") {
                        if !href.starts_with('#')
                            && !href.starts_with("mailto:")
                            && !href.starts_with("javascript:")
                        {
                            self.pending_link = Some(href);
                        }
                    }
                }
                "a" if closing => {
                    if let Some(h) = self.pending_link.take() {
                        self.out.push_str(" <");
                        self.out.push_str(&h);
                        self.out.push('>');
                    }
                }
                n if BLOCKS.contains(&n) => self.newline(),
                _ => {}
            }
        }
    }

    fn text(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        if s.chars().all(char::is_whitespace) {
            // Inter-tag whitespace: an inline separator at most, never a
            // paragraph break (those come from block tags).
            if !self.out.is_empty() && !self.out.ends_with(['\n', ' ']) {
                self.out.push(' ');
            }
            return;
        }
        self.out.push_str(&decode_entities(s));
    }

    fn newline(&mut self) {
        if !self.out.ends_with('\n') {
            self.out.push('\n');
        }
    }
}

/// Case-insensitive substring search on ASCII needles.
fn find_ci(hay: &str, needle: &str) -> Option<usize> {
    let n = needle.to_ascii_lowercase();
    let h = hay.to_ascii_lowercase();
    h.find(&n)
}

/// Extracts a (possibly quoted) attribute value from a tag body.
fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut search = 0;
    while let Some(i) = lower.get(search..)?.find(name) {
        let at = search + i;
        let after = lower.get(at + name.len()..)?.trim_start();
        let boundary_ok = at == 0
            || lower
                .as_bytes()
                .get(at - 1)
                .is_some_and(|b| b.is_ascii_whitespace());
        if boundary_ok && after.starts_with('=') {
            let value_start = tag.len() - after.len() + 1;
            let v = tag.get(value_start..)?.trim_start();
            let v = if let Some(q) = v.strip_prefix('"') {
                q.split('"').next().unwrap_or("")
            } else if let Some(q) = v.strip_prefix('\'') {
                q.split('\'').next().unwrap_or("")
            } else {
                v.split(|c: char| c.is_whitespace()).next().unwrap_or("")
            };
            return Some(v.trim().to_owned());
        }
        search = at + name.len();
    }
    None
}

/// Decodes the entities that appear in real mail; unknown ones are kept
/// verbatim.
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(rest.get(..amp).unwrap_or(""));
        let after = rest.get(amp + 1..).unwrap_or("");
        let Some(semi) = after.find(';').filter(|&i| i <= 10) else {
            out.push('&');
            rest = after;
            continue;
        };
        let ent = after.get(..semi).unwrap_or("");
        let decoded = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            "copy" => Some('©'),
            "reg" => Some('®'),
            "hellip" => Some('…'),
            "mdash" => Some('—'),
            "ndash" => Some('–'),
            "lsquo" => Some('‘'),
            "rsquo" => Some('’'),
            "ldquo" => Some('“'),
            "rdquo" => Some('”'),
            _ => ent
                .strip_prefix('#')
                .and_then(|n| {
                    n.strip_prefix(['x', 'X'])
                        .and_then(|h| u32::from_str_radix(h, 16).ok())
                        .or_else(|| n.parse::<u32>().ok())
                })
                .and_then(char::from_u32)
                .filter(|c| !c.is_control()),
        };
        match decoded {
            Some(c) => out.push(c),
            None => {
                out.push('&');
                out.push_str(ent);
                out.push(';');
            }
        }
        rest = after.get(semi + 1..).unwrap_or("");
    }
    out.push_str(rest);
    out
}

/// Collapses whitespace: runs of spaces/tabs → one space, more than two
/// consecutive newlines → two, trailing spaces trimmed.
fn collapse(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank_lines = 0;
    for line in s.lines() {
        let words: Vec<&str> = line
            .split([' ', '\t', '\r', '\u{a0}'])
            .filter(|w| !w.is_empty())
            .collect();
        if words.is_empty() {
            blank_lines += 1;
            if blank_lines <= 1 && !out.is_empty() {
                out.push('\n');
            }
            continue;
        }
        blank_lines = 0;
        out.push_str(&words.join(" "));
        out.push('\n');
    }
    out.trim_end().to_owned()
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
    fn basic_conversion() {
        let html = r#"<html><head><title>T</title><style>p{color:red}</style></head>
<body><p>Hello&nbsp;<b>World</b>&amp; friends</p><script>alert(1)</script>
<ul><li>one</li><li>two</li></ul><a href="https://example.com/x">link</a><br>bye &#x1F600; &#169;</body></html>"#;
        let t = html_to_text(html);
        assert_eq!(
            t,
            "Hello World& friends\n- one\n- two\nlink <https://example.com/x>\nbye 😀 ©"
        );
    }

    #[test]
    fn robust_against_garbage() {
        assert_eq!(html_to_text("plain text"), "plain text");
        assert_eq!(
            html_to_text("a < b and <unterminated"),
            "a < b and <unterminated"
        );
        assert_eq!(html_to_text("<!-- c --><p>x</p><!-- unterminated"), "x");
        assert_eq!(html_to_text("<script>never closed"), "");
        assert_eq!(html_to_text("&bogus; &#xZZ; &amp"), "&bogus; &#xZZ; &amp");
    }

    #[test]
    fn tables_and_whitespace() {
        let t = html_to_text("<table><tr><td>a</td><td>b</td></tr><tr><td>c</td></tr></table>\n\n\n<div>   d   e  </div>");
        // Whitespace between tags is not a paragraph break.
        assert_eq!(t, "a b\nc\nd e");
        // Paragraph breaks come from block tags; runs of blank lines collapse to one.
        assert_eq!(html_to_text("<p>a</p><br><br><br><p>b</p>"), "a\n\nb");
    }

    #[test]
    fn attributes() {
        assert_eq!(
            attr(r#"a class="x" href='https://e.com/?a=1&b=2'"#, "href").unwrap(),
            "https://e.com/?a=1&b=2"
        );
        assert_eq!(attr("a href=plain", "href").unwrap(), "plain");
        assert!(attr("a data-href=\"x\"", "href").is_none());
    }
}
