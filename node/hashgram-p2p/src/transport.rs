//! Transport construction, and the one Windows-specific rule it carries.
//!
//! # Why TCP dials must not reuse the listening port on Windows
//!
//! libp2p dials with [`PortUse::Reuse`] by default: the outgoing TCP socket
//! is bound to the port the node listens on, so a peer that sees the
//! connection arrive learns a dialable address (this is what makes AutoNAT
//! and hole punching work on Linux, where `SO_REUSEPORT` permits it). Windows
//! has no `SO_REUSEPORT`; `SO_REUSEADDR` lets the second socket *bind*, but
//! `connect()` from a socket sharing a listening socket's port fails with
//! `WSAEADDRINUSE` (10048). The result was that every TCP dial from a
//! Windows client failed and only QUIC ever connected, so on a network where
//! UDP is filtered (many offices, some ISPs) or fragile (VPN MTUs) the
//! client never reached a node at all.
//!
//! [`FreshPortDial`] wraps the TCP transport and, on Windows only, asks for a
//! new ephemeral port on every dial. Linux nodes keep the reuse behaviour
//! they rely on.

use std::pin::Pin;
use std::task::{Context, Poll};

use libp2p::core::muxing::StreamMuxerBox;
use libp2p::core::transport::{
    Boxed, DialOpts, ListenerId, PortUse, TransportError, TransportEvent,
};
use libp2p::core::upgrade::Version;
use libp2p::identity::Keypair;
use libp2p::{noise, yamux, Multiaddr, PeerId, Transport};

/// A transport whose dials never reuse the listening port on Windows.
pub struct FreshPortDial<T>(pub T);

impl<T: Transport + Unpin> Transport for FreshPortDial<T> {
    type Output = T::Output;
    type Error = T::Error;
    type ListenerUpgrade = T::ListenerUpgrade;
    type Dial = T::Dial;

    fn listen_on(
        &mut self,
        id: ListenerId,
        addr: Multiaddr,
    ) -> Result<(), TransportError<Self::Error>> {
        self.0.listen_on(id, addr)
    }

    fn remove_listener(&mut self, id: ListenerId) -> bool {
        self.0.remove_listener(id)
    }

    fn dial(
        &mut self,
        addr: Multiaddr,
        opts: DialOpts,
    ) -> Result<Self::Dial, TransportError<Self::Error>> {
        self.0.dial(addr, dial_opts_for_platform(opts))
    }

    fn poll(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<TransportEvent<Self::ListenerUpgrade, Self::Error>> {
        Pin::new(&mut self.get_mut().0).poll(cx)
    }
}

/// The dial options a TCP dial actually uses on this platform.
#[must_use]
pub fn dial_opts_for_platform(opts: DialOpts) -> DialOpts {
    if cfg!(windows) {
        DialOpts {
            role: opts.role,
            port_use: PortUse::New,
        }
    } else {
        opts
    }
}

/// The TCP transport with Noise and Yamux, wrapped so Windows dials work.
pub(crate) fn tcp_transport(
    key: &Keypair,
) -> Result<Boxed<(PeerId, StreamMuxerBox)>, Box<dyn std::error::Error + Send + Sync>> {
    let tcp = libp2p::tcp::tokio::Transport::new(libp2p::tcp::Config::default().nodelay(true));
    let noise =
        noise::Config::new(key).map_err(Box::<dyn std::error::Error + Send + Sync>::from)?;
    Ok(FreshPortDial(tcp)
        .upgrade(Version::V1Lazy)
        .authenticate(noise)
        .multiplex(yamux::Config::default())
        .map(|(peer, muxer), _| (peer, StreamMuxerBox::new(muxer)))
        .boxed())
}

#[cfg(test)]
mod tests {
    use super::*;
    use libp2p::core::Endpoint;

    #[test]
    fn windows_never_reuses_the_listen_port_for_a_dial() {
        let out = dial_opts_for_platform(DialOpts {
            role: Endpoint::Dialer,
            port_use: PortUse::Reuse,
        });
        if cfg!(windows) {
            assert_eq!(out.port_use, PortUse::New);
        } else {
            assert_eq!(out.port_use, PortUse::Reuse);
        }
        assert_eq!(out.role, Endpoint::Dialer);
    }
}
