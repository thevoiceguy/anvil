//! Beneath the streams: what carries a call's video packets (§2.4).
//!
//! A stream hands its transport plain RTP and RTCP and is handed plain
//! packets back; how they travel is the transport's alone. SIP's is
//! [`SdesTransport`]: one socket pair per SDP section, keyed by SDES. A
//! WebRTC one (ICE, DTLS-SRTP, BUNDLE, demultiplexing by `mid` and SSRC)
//! sits beside it later, delivering into the same [`Inbound`] channel.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use bytes::Bytes;
use forge_rtp::rtp::RtpPacket;
use forge_rtp::srtp::SrtpContext;
use parking_lot::Mutex;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::call::MediaSockets;
use crate::capture::Direction;

/// A packet that arrived, already plain.
#[derive(Debug)]
pub(crate) enum Inbound {
    Rtp { packet: RtpPacket, at: Instant },
    Rtcp { packet: Bytes },
}

/// Where a session's packets go out.
#[async_trait]
pub(crate) trait MediaTransport: Send + Sync {
    async fn send_rtp(&self, packet: Bytes);
    async fn send_rtcp(&self, packet: Bytes);
}

/// How many inbound packets wait for their session before more are
/// dropped: a second of 720p and then some.
pub(crate) const INBOUND_QUEUE: usize = 512;

/// SIP's transport for one video section: its RTP and RTCP sockets, the
/// far end's addresses from its SDP, and the section's own SDES keys.
pub(crate) struct SdesTransport {
    sockets: MediaSockets,
    remote: Mutex<Remote>,
    srtp: Option<Arc<Mutex<SrtpContext>>>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

#[derive(Debug, Clone, Copy)]
struct Remote {
    rtp: SocketAddr,
    rtcp: SocketAddr,
    /// Where the far end's RTCP came from: answered there.
    rtcp_latched: Option<SocketAddr>,
}

impl SdesTransport {
    /// Start reading `sockets`, delivering what arrives to `inbound`.
    pub(crate) fn start(
        sockets: MediaSockets,
        remote_rtp: SocketAddr,
        srtp: Option<SrtpContext>,
        inbound: mpsc::Sender<Inbound>,
    ) -> Arc<Self> {
        let srtp = srtp.map(|c| Arc::new(Mutex::new(c)));
        let transport = Arc::new(SdesTransport {
            sockets: sockets.clone(),
            remote: Mutex::new(Remote {
                rtp: remote_rtp,
                rtcp: crate::rtcp::remote_rtcp_addr(remote_rtp),
                rtcp_latched: None,
            }),
            srtp: srtp.clone(),
            tasks: Mutex::new(Vec::new()),
        });
        let mut tasks = vec![tokio::spawn(read_rtp(
            Arc::clone(&sockets.rtp),
            srtp.clone(),
            sockets.capture_slot(),
            inbound.clone(),
        ))];
        if let Some(rtcp) = &sockets.rtcp {
            tasks.push(tokio::spawn(read_rtcp(
                Arc::clone(rtcp),
                srtp,
                sockets.capture_slot(),
                inbound,
                Arc::downgrade(&transport),
            )));
        }
        *transport.tasks.lock() = tasks;
        transport
    }

    /// The far end moved (a re-INVITE naming another address).
    pub(crate) fn set_remote(&self, rtp: SocketAddr) {
        let mut r = self.remote.lock();
        if r.rtp != rtp {
            *r = Remote {
                rtp,
                rtcp: crate::rtcp::remote_rtcp_addr(rtp),
                rtcp_latched: None,
            };
        }
    }

    /// New keys (a re-INVITE carrying another of theirs).
    pub(crate) fn rekey(&self, ctx: SrtpContext) {
        if let Some(srtp) = &self.srtp {
            *srtp.lock() = ctx;
        }
    }
}

impl Drop for SdesTransport {
    fn drop(&mut self) {
        for t in self.tasks.lock().iter() {
            t.abort();
        }
    }
}

#[async_trait]
impl MediaTransport for SdesTransport {
    async fn send_rtp(&self, packet: Bytes) {
        let to = self.remote.lock().rtp;
        let Some(sealed) = seal(self.srtp.as_ref(), &packet, false) else {
            return;
        };
        match self.sockets.rtp.send_to(&sealed, to).await {
            Ok(_) => crate::capture::tap(
                &self.sockets.capture_slot(),
                Direction::Outbound,
                &self.sockets.rtp,
                to,
                &packet,
                None,
            ),
            Err(e) => tracing::debug!(%e, %to, "video rtp send failed"),
        }
    }

    async fn send_rtcp(&self, packet: Bytes) {
        let Some(socket) = &self.sockets.rtcp else {
            return;
        };
        let to = {
            let r = self.remote.lock();
            r.rtcp_latched.unwrap_or(r.rtcp)
        };
        let Some(sealed) = seal(self.srtp.as_ref(), &packet, true) else {
            return;
        };
        match socket.send_to(&sealed, to).await {
            Ok(_) => crate::capture::tap(
                &self.sockets.capture_slot(),
                Direction::Outbound,
                socket,
                to,
                &packet,
                None,
            ),
            Err(e) => tracing::debug!(%e, %to, "video rtcp send failed"),
        }
    }
}

fn seal(srtp: Option<&Arc<Mutex<SrtpContext>>>, packet: &[u8], rtcp: bool) -> Option<Bytes> {
    let Some(ctx) = srtp else {
        return Some(Bytes::copy_from_slice(packet));
    };
    let mut ctx = ctx.lock();
    let sealed = if rtcp {
        ctx.protect_rtcp(packet)
    } else {
        ctx.protect_rtp(packet)
    };
    match sealed {
        Ok(s) => Some(Bytes::from(s)),
        Err(e) => {
            tracing::warn!(%e, rtcp, "video srtp protect failed");
            None
        }
    }
}

async fn read_rtp(
    socket: Arc<tokio::net::UdpSocket>,
    srtp: Option<Arc<Mutex<SrtpContext>>>,
    capture: crate::capture::CaptureSlot,
    inbound: mpsc::Sender<Inbound>,
) {
    let mut buf = vec![0u8; 2048];
    loop {
        let (len, from) = match socket.recv_from(&mut buf).await {
            Ok(x) => x,
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => continue,
            Err(e) => {
                tracing::debug!(%e, "video rtp recv failed");
                return;
            }
        };
        let at = Instant::now();
        let plain = match &srtp {
            None => Bytes::copy_from_slice(&buf[..len]),
            Some(ctx) => match ctx.lock().unprotect_rtp(&buf[..len]) {
                Ok(p) => Bytes::from(p),
                Err(e) => {
                    tracing::debug!(%e, %from, "video srtp unprotect failed; dropping");
                    continue;
                }
            },
        };
        crate::capture::tap(&capture, Direction::Inbound, &socket, from, &plain, None);
        let Ok(packet) = RtpPacket::parse(plain) else {
            continue;
        };
        // Full: the session is behind, and a stale packet is worth less
        // than the next. Gone: the session ended.
        match inbound.try_send(Inbound::Rtp { packet, at }) {
            Err(mpsc::error::TrySendError::Closed(_)) => return,
            _ => continue,
        }
    }
}

async fn read_rtcp(
    socket: Arc<tokio::net::UdpSocket>,
    srtp: Option<Arc<Mutex<SrtpContext>>>,
    capture: crate::capture::CaptureSlot,
    inbound: mpsc::Sender<Inbound>,
    transport: std::sync::Weak<SdesTransport>,
) {
    let mut buf = vec![0u8; 1500];
    loop {
        let (len, from) = match socket.recv_from(&mut buf).await {
            Ok(x) => x,
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => continue,
            Err(e) => {
                tracing::debug!(%e, "video rtcp recv failed");
                return;
            }
        };
        let plain = match &srtp {
            None => Bytes::copy_from_slice(&buf[..len]),
            Some(ctx) => match ctx.lock().unprotect_rtcp(&buf[..len]) {
                Ok(p) => Bytes::from(p),
                Err(e) => {
                    tracing::debug!(%e, %from, "video srtcp unprotect failed; dropping");
                    continue;
                }
            },
        };
        crate::capture::tap(&capture, Direction::Inbound, &socket, from, &plain, None);
        if let Some(t) = transport.upgrade() {
            t.remote.lock().rtcp_latched = Some(from);
        }
        if inbound.send(Inbound::Rtcp { packet: plain }).await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
pub(crate) mod looped {
    //! A second transport, for the tests: two sessions joined by channels,
    //! with no socket and no SDP, and loss on demand. It keeps the streams
    //! honest about §2.4 before anything else needs them to be.

    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    pub(crate) struct LoopTransport {
        peer: mpsc::Sender<Inbound>,
        /// Drop every n-th RTP packet (0: none).
        drop_every: AtomicU32,
        sent: AtomicU32,
        /// RTP packets sent, by sequence number, and RTCP packets.
        pub(crate) rtp_log: Mutex<Vec<u16>>,
        pub(crate) rtcp_log: Mutex<Vec<Bytes>>,
    }

    impl LoopTransport {
        pub(crate) fn new(peer: mpsc::Sender<Inbound>) -> Arc<Self> {
            Arc::new(Self {
                peer,
                drop_every: AtomicU32::new(0),
                sent: AtomicU32::new(0),
                rtp_log: Mutex::default(),
                rtcp_log: Mutex::default(),
            })
        }

        pub(crate) fn drop_every(&self, n: u32) {
            self.drop_every.store(n, Ordering::Relaxed);
        }
    }

    #[async_trait]
    impl MediaTransport for LoopTransport {
        async fn send_rtp(&self, packet: Bytes) {
            let n = self.sent.fetch_add(1, Ordering::Relaxed) + 1;
            let every = self.drop_every.load(Ordering::Relaxed);
            let Ok(packet) = RtpPacket::parse(packet) else {
                return;
            };
            self.rtp_log.lock().push(packet.header.sequence_number);
            if every != 0 && n.is_multiple_of(every) {
                return;
            }
            let _ = self.peer.try_send(Inbound::Rtp {
                packet,
                at: Instant::now(),
            });
        }

        async fn send_rtcp(&self, packet: Bytes) {
            self.rtcp_log.lock().push(packet.clone());
            let _ = self.peer.send(Inbound::Rtcp { packet }).await;
        }
    }
}
