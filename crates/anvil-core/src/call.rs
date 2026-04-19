//! Per-call runtime state.
//!
//! Phase 1 M2 scope: outgoing call only. The runtime tracks a `CallHandle`
//! from `sip_uac::integrated` and the UDP socket we advertised in the SDP
//! offer so later milestones can bind an RTP engine to it without churning
//! the SDP.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

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

/// Runtime state for one active call.
///
/// The `rtp_socket` is bound eagerly in `place_call` so the port we advertised
/// in the SDP offer is actually reserved. Phase 1 M2 does not send or receive
/// RTP; the audio milestone will replace this with a real engine.
pub(crate) struct CallEntry {
    pub handle: Arc<CallHandle>,
    pub negotiated_codec: Option<Codec>,
    pub rtp_socket: Arc<UdpSocket>,
    /// Populated when the call reaches `CallEstablished` and the RTP pipeline
    /// starts. `None` until then. Dropping it aborts the send/receive tasks.
    pub pipeline: Option<MediaPipeline>,
}

/// Bind a UDP socket for RTP. Caller supplies a media IP (usually the
/// signaling IP); the port is ephemeral.
pub(crate) async fn bind_rtp_socket(media_ip: IpAddr) -> std::io::Result<Arc<UdpSocket>> {
    // Bind to (media_ip, 0) so the OS picks an ephemeral port in the usual range.
    // Respecting a configured MediaConfig::rtp_port_range is a Phase 2 concern.
    let addr = SocketAddr::new(media_ip, 0);
    let sock = UdpSocket::bind(addr).await?;
    Ok(Arc::new(sock))
}

/// Default media IP when the caller doesn't pin one. 0.0.0.0 is **not** valid in
/// SDP — we need a concrete address — so we fall back to loopback for dev and
/// expect the caller to set a real IP via `AccountConfig::bind_addr` in
/// production.
pub(crate) fn default_media_ip() -> IpAddr {
    IpAddr::V4(Ipv4Addr::LOCALHOST)
}
