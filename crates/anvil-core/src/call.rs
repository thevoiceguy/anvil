//! Per-call runtime state.
//!
//! A `CallEntry` tracks both outgoing and incoming calls. Outgoing calls
//! have an `outbound_handle` from `sip_uac::integrated::CallHandle`;
//! incoming calls start with `inbound = Some(PendingInbound)` while the
//! app is still deciding whether to answer, and transition to a confirmed
//! `dialog` once `Anvil::answer` sends the 2xx.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use sip_core::{Request, Response};
use sip_dialog::Dialog;
use sip_transaction::ServerTransactionHandle;
use sip_uac::integrated::CallHandle;
use tokio::net::UdpSocket;

use crate::config::Codec;
use crate::media::MediaPipeline;
use crate::CallId;

/// Process-local monotonically increasing source of `CallId`s.
static NEXT_CALL_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_call_id() -> CallId {
    CallId(NEXT_CALL_ID.fetch_add(1, Ordering::Relaxed))
}

/// State for an incoming INVITE that the app hasn't yet accepted or rejected.
pub(crate) struct PendingInbound {
    pub request: Request,
    pub answer_response: Response,
    pub server_handle: ServerTransactionHandle,
    pub dialog: Dialog,
    pub sdp_answer_sent: bool,
    pub rtp_socket: Arc<UdpSocket>,
}

/// Runtime state for one active call.
pub(crate) struct CallEntry {
    /// Outgoing calls only. The UAC handle drives provisional / final / term
    /// channels; we read the dialog through it.
    pub outbound_handle: Option<Arc<CallHandle>>,
    /// Inbound INVITE that hasn't been answered yet.
    pub inbound: Option<PendingInbound>,
    /// Confirmed dialog for in-progress inbound calls (post-answer). Used as
    /// the BYE target if the app hangs up first.
    pub dialog: Option<Dialog>,
    pub negotiated_codec: Option<Codec>,
    pub rtp_socket: Arc<UdpSocket>,
    /// Populated when the call reaches `CallEstablished` and the RTP
    /// pipeline starts. `None` until then. Dropping it aborts the
    /// send / receive tasks.
    pub pipeline: Option<MediaPipeline>,
    /// The call's SDES-SRTP keys when its media is encrypted: ours (sent in
    /// every offer and answer, re-INVITEs included) and theirs once known.
    pub srtp: Option<CallSrtp>,
}

/// A call's SRTP keys.
#[derive(Debug, Clone)]
pub(crate) struct CallSrtp {
    pub ours: crate::srtp::Crypto,
    pub theirs: Option<crate::srtp::Crypto>,
}

impl CallSrtp {
    /// The context once both keys are known.
    pub fn context(&self) -> Option<forge_rtp::srtp::SrtpContext> {
        crate::srtp::context(&self.ours, self.theirs.as_ref()?)
    }
}

/// Bind a UDP socket for RTP: at the media IP when it is this machine's,
/// else (a STUN-discovered public address) on every interface.
pub(crate) async fn bind_rtp_socket(media_ip: IpAddr) -> std::io::Result<Arc<UdpSocket>> {
    match UdpSocket::bind(SocketAddr::new(media_ip, 0)).await {
        Ok(sock) => Ok(Arc::new(sock)),
        Err(e) if e.kind() == std::io::ErrorKind::AddrNotAvailable => {
            let any = match media_ip {
                IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                IpAddr::V6(_) => IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED),
            };
            Ok(Arc::new(UdpSocket::bind(SocketAddr::new(any, 0)).await?))
        }
        Err(e) => Err(e),
    }
}

/// The media IP when the account pins none: this machine's address on the
/// route to `server` (a registrar or proxy URI), else on the default
/// route, else loopback. Connecting a UDP socket picks the route and
/// sends nothing. Loopback was the default once, and a phone that names
/// loopback is heard by nobody.
pub(crate) async fn default_media_ip(server: &str) -> IpAddr {
    let mut targets: Vec<SocketAddr> = match server_host_port(server) {
        Some(host_port) => tokio::net::lookup_host(host_port)
            .await
            .map(|addrs| addrs.collect())
            .unwrap_or_default(),
        None => Vec::new(),
    };
    targets.push(SocketAddr::from(([192, 0, 2, 1], 9)));
    for target in targets {
        if let Some(ip) = route_ip(target) {
            return ip;
        }
    }
    IpAddr::V4(Ipv4Addr::LOCALHOST)
}

/// This machine's address on the route to `target`.
fn route_ip(target: SocketAddr) -> Option<IpAddr> {
    let any: SocketAddr = match target {
        SocketAddr::V4(_) => SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)),
        SocketAddr::V6(_) => SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, 0)),
    };
    let sock = std::net::UdpSocket::bind(any).ok()?;
    sock.connect(target).ok()?;
    let ip = sock.local_addr().ok()?.ip();
    (!ip.is_unspecified()).then_some(ip)
}

/// `host:port` of a SIP URI (`sip:pbx.example.com:5080;transport=udp`),
/// 5060 when it names no port.
fn server_host_port(uri: &str) -> Option<String> {
    let uri = uri.trim().trim_start_matches('<');
    let rest = uri
        .strip_prefix("sips:")
        .or_else(|| uri.strip_prefix("sip:"))
        .unwrap_or(uri);
    let rest = rest.rsplit_once('@').map(|(_, h)| h).unwrap_or(rest);
    let host_port = rest.split([';', '?', '>']).next()?.trim();
    if host_port.is_empty() {
        return None;
    }
    let has_port = match host_port.rfind(']') {
        Some(close) => host_port[close..].contains(':'),
        None => host_port.contains(':'),
    };
    Some(if has_port {
        host_port.to_string()
    } else {
        format!("{host_port}:5060")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_servers_host_and_port_are_read_off_its_uri() {
        assert_eq!(
            server_host_port("sip:d8.feryx.io").as_deref(),
            Some("d8.feryx.io:5060")
        );
        assert_eq!(
            server_host_port("<sip:pbx.example.com:5080;transport=udp>").as_deref(),
            Some("pbx.example.com:5080")
        );
        assert_eq!(
            server_host_port("sips:bob@10.0.0.1:5061").as_deref(),
            Some("10.0.0.1:5061")
        );
        assert_eq!(
            server_host_port("sip:[2001:db8::1]").as_deref(),
            Some("[2001:db8::1]:5060")
        );
        assert_eq!(
            server_host_port("sip:[2001:db8::1]:5070").as_deref(),
            Some("[2001:db8::1]:5070")
        );
        assert_eq!(server_host_port("sip:"), None);
    }

    #[tokio::test]
    async fn the_media_ip_is_this_machines_on_the_route_to_the_server() {
        // A server on this machine is reached over loopback, and named so.
        assert_eq!(
            default_media_ip("sip:127.0.0.1:5060").await,
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        );
        // One elsewhere is not: never loopback while a route exists.
        let elsewhere = default_media_ip("sip:192.0.2.10").await;
        if route_ip(SocketAddr::from(([192, 0, 2, 10], 5060))).is_some() {
            assert!(!elsewhere.is_loopback(), "{elsewhere}");
        }
    }

    #[tokio::test]
    async fn rtp_binds_on_every_interface_when_the_media_ip_is_not_ours() {
        // TEST-NET-3: never this machine's.
        let sock = bind_rtp_socket(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7)))
            .await
            .unwrap();
        assert!(sock.local_addr().unwrap().ip().is_unspecified());
        let sock = bind_rtp_socket(IpAddr::V4(Ipv4Addr::LOCALHOST))
            .await
            .unwrap();
        assert!(sock.local_addr().unwrap().ip().is_loopback());
    }
}
