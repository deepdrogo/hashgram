//! MX resolution for outbound delivery.
//!
//! RFC 5321 §5.1: try MX records in preference order; a domain without
//! MX records but with an address record is its own mail host (the
//! "implicit MX"); a single "null MX" (`0 .`, RFC 7505) means the domain
//! accepts no mail at all and the message bounces immediately. The
//! resolver is behind a trait so the queue logic is testable with a
//! canned answer.

use std::future::Future;

use hickory_resolver::TokioResolver;

use crate::GatewayError;

/// One mail host to try, best first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailHost {
    /// Hostname (no trailing dot).
    pub host: String,
    /// MX preference (lower first); `u16::MAX` for the implicit MX.
    pub preference: u16,
}

/// What a lookup says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MxAnswer {
    /// Hosts to try in order.
    Hosts(Vec<MailHost>),
    /// RFC 7505 null MX: the domain refuses mail. Permanent.
    NullMx,
}

/// Resolves the mail hosts for a domain.
pub trait MxResolver: Send + Sync {
    /// Looks up `domain`.
    fn resolve(&self, domain: &str) -> impl Future<Output = Result<MxAnswer, GatewayError>> + Send;
}

/// The system resolver (`/etc/resolv.conf`).
pub struct SystemResolver {
    inner: TokioResolver,
}

impl std::fmt::Debug for SystemResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemResolver").finish_non_exhaustive()
    }
}

impl SystemResolver {
    /// Builds from the OS configuration.
    pub fn new() -> Result<Self, GatewayError> {
        let inner = TokioResolver::builder_tokio()
            .map_err(|e| GatewayError::Dns(e.to_string()))?
            .build();
        Ok(Self { inner })
    }
}

impl MxResolver for SystemResolver {
    async fn resolve(&self, domain: &str) -> Result<MxAnswer, GatewayError> {
        let fqdn = format!("{}.", domain.trim_end_matches('.'));
        match self.inner.mx_lookup(fqdn.as_str()).await {
            Ok(mx) => {
                let hosts: Vec<MailHost> = mx
                    .iter()
                    .map(|r| MailHost {
                        host: r.exchange().to_utf8().trim_end_matches('.').to_owned(),
                        preference: r.preference(),
                    })
                    .collect();
                Ok(order_hosts(hosts, domain))
            }
            Err(e) => {
                // NXDOMAIN / no records: fall back to the implicit MX only
                // when the name itself resolves; otherwise report.
                let msg = e.to_string();
                if e.is_no_records_found() {
                    match self.inner.lookup_ip(fqdn.as_str()).await {
                        Ok(ips) if ips.iter().next().is_some() => {
                            Ok(MxAnswer::Hosts(vec![MailHost {
                                host: domain.to_owned(),
                                preference: u16::MAX,
                            }]))
                        }
                        _ => Err(GatewayError::Dns(format!(
                            "{domain}: no MX and no address records"
                        ))),
                    }
                } else {
                    Err(GatewayError::Dns(format!("{domain}: {msg}")))
                }
            }
        }
    }
}

/// Sorts by preference, detects the null MX, falls back to the implicit
/// MX when the record set is empty.
#[must_use]
pub fn order_hosts(mut hosts: Vec<MailHost>, domain: &str) -> MxAnswer {
    if hosts.len() == 1
        && hosts
            .first()
            .is_some_and(|h| h.host.is_empty() && h.preference == 0)
    {
        return MxAnswer::NullMx;
    }
    hosts.retain(|h| !h.host.is_empty());
    if hosts.is_empty() {
        return MxAnswer::Hosts(vec![MailHost {
            host: domain.to_owned(),
            preference: u16::MAX,
        }]);
    }
    hosts.sort_by(|a, b| {
        a.preference
            .cmp(&b.preference)
            .then_with(|| a.host.cmp(&b.host))
    });
    hosts.dedup_by(|a, b| a.host.eq_ignore_ascii_case(&b.host));
    MxAnswer::Hosts(hosts)
}

/// A resolver answering from a fixed table (tests, lab setups).
#[derive(Debug, Default, Clone)]
pub struct StaticResolver {
    /// domain → answer.
    pub table: std::collections::HashMap<String, MxAnswer>,
}

impl MxResolver for StaticResolver {
    async fn resolve(&self, domain: &str) -> Result<MxAnswer, GatewayError> {
        self.table
            .get(&domain.to_ascii_lowercase())
            .cloned()
            .ok_or_else(|| GatewayError::Dns(format!("{domain}: not in static table")))
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

    fn h(host: &str, p: u16) -> MailHost {
        MailHost {
            host: host.into(),
            preference: p,
        }
    }

    #[test]
    fn ordering_and_null_mx() {
        assert_eq!(order_hosts(vec![h("", 0)], "example.com"), MxAnswer::NullMx);
        assert_eq!(
            order_hosts(
                vec![
                    h("b.example.com", 20),
                    h("a.example.com", 10),
                    h("A.example.com", 10)
                ],
                "example.com"
            ),
            MxAnswer::Hosts(vec![h("A.example.com", 10), h("b.example.com", 20)])
        );
        assert_eq!(
            order_hosts(vec![], "example.com"),
            MxAnswer::Hosts(vec![h("example.com", u16::MAX)])
        );
    }

    #[tokio::test]
    async fn static_resolver() {
        let mut r = StaticResolver::default();
        r.table.insert(
            "example.com".into(),
            MxAnswer::Hosts(vec![h("mx.example.com", 10)]),
        );
        assert!(
            matches!(r.resolve("Example.COM").await.unwrap(), MxAnswer::Hosts(v) if v.len() == 1)
        );
        assert!(r.resolve("nope.org").await.is_err());
    }
}
