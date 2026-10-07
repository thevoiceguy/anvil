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

/// Bind a UDP socket for RTP.
pub(crate) async fn bind_rtp_socket(media_ip: IpAddr) -> std::io::Result<Arc<UdpSocket>> {
    let addr = SocketAddr::new(media_ip, 0);
    let sock = UdpSocket::bind(addr).await?;
    Ok(Arc::new(sock))
}

/// Default media IP when the caller doesn't pin one.
pub(crate) fn default_media_ip() -> IpAddr {
    IpAddr::V4(Ipv4Addr::LOCALHOST)
}
