//! SIP transport setup for Anvil.
//!
//! siphon-rs exposes the `TransportDispatcher` trait and primitives (`run_udp`,
//! `send_udp`, etc.) but does not ship a concrete cross-transport dispatcher —
//! `siphond` implements its own. We port that implementation here so
//! `anvil-core` can stand up a UDP-first transport for the softphone with
//! TCP/TLS arriving alongside Phase 2 TLS work.
//!
//! Phase 1 scope: UDP only. The TCP and TLS arms return an error; the
//! registrar / target must be reachable over UDP.

use std::sync::Arc;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use bytes::Bytes;
use sip_transaction::{TransportContext, TransportDispatcher, TransportKind};
use sip_transport::{
    pool::ConnectionPool, run_udp, send_udp, DefaultTransportPolicy, InboundPacket,
    TransportPolicy,
};
use tokio::{net::UdpSocket, sync::mpsc};
use tracing::warn;

/// Bring up the local UDP listener and return a dispatcher that can send on it.
///
/// Callers drain the returned `mpsc::Receiver<InboundPacket>` and feed each
/// packet's payload into the `TransactionManager` via `receive_response` /
/// request dispatch.
pub(crate) async fn start_udp_transport(
    bind_addr: &str,
) -> Result<(
    Arc<dyn TransportDispatcher>,
    Arc<UdpSocket>,
    mpsc::Receiver<InboundPacket>,
)> {
    let (tx, rx) = mpsc::channel::<InboundPacket>(1024);

    let udp_socket = Arc::new(UdpSocket::bind(bind_addr).await?);

    let tcp_pool = Arc::new(ConnectionPool::new());

    let dispatcher: Arc<dyn TransportDispatcher> = Arc::new(AnvilTransportDispatcher {
        udp_socket: Arc::clone(&udp_socket),
        policy: Arc::new(DefaultTransportPolicy::default()),
        tcp_pool,
    });

    // Spawn the UDP inbound loop. The receiver end is returned to the caller.
    tokio::spawn({
        let recv_socket = Arc::clone(&udp_socket);
        async move {
            if let Err(e) = run_udp(recv_socket, tx).await {
                tracing::error!(%e, "UDP listener exited");
            }
        }
    });

    Ok((dispatcher, udp_socket, rx))
}

struct AnvilTransportDispatcher {
    udp_socket: Arc<UdpSocket>,
    policy: Arc<dyn TransportPolicy>,
    #[allow(dead_code)] // wired up when TCP lands
    tcp_pool: Arc<ConnectionPool>,
}

#[async_trait]
impl TransportDispatcher for AnvilTransportDispatcher {
    async fn dispatch(&self, ctx: &TransportContext, payload: Bytes) -> Result<()> {
        let desired = to_sip_transport(ctx.transport());
        let selected = self.policy.choose(
            desired,
            payload.len(),
            matches!(
                ctx.transport(),
                TransportKind::Tls | TransportKind::Wss | TransportKind::TlsSctp
            ),
        );

        let target = match selected {
            sip_transport::TransportKind::Tcp | sip_transport::TransportKind::Tls
                if ctx.stream().is_none() =>
            {
                warn!(
                    ?selected,
                    ?desired,
                    peer = %ctx.peer(),
                    "Policy requested stream transport but no stream available; falling back to desired"
                );
                desired
            }
            other => other,
        };

        match target {
            sip_transport::TransportKind::Udp => {
                send_udp(self.udp_socket.as_ref(), &ctx.peer(), &payload).await?;
                Ok(())
            }
            sip_transport::TransportKind::Tcp
            | sip_transport::TransportKind::Tls
            | sip_transport::TransportKind::Ws
            | sip_transport::TransportKind::Wss
            | sip_transport::TransportKind::Sctp
            | sip_transport::TransportKind::TlsSctp => {
                Err(anyhow!("transport {:?} not yet implemented in anvil-core (Phase 1 is UDP only)", target))
            }
        }
    }
}

fn to_sip_transport(kind: TransportKind) -> sip_transport::TransportKind {
    match kind {
        TransportKind::Udp => sip_transport::TransportKind::Udp,
        TransportKind::Tcp => sip_transport::TransportKind::Tcp,
        TransportKind::Tls => sip_transport::TransportKind::Tls,
        TransportKind::Ws => sip_transport::TransportKind::Ws,
        TransportKind::Wss => sip_transport::TransportKind::Wss,
        TransportKind::Sctp => sip_transport::TransportKind::Sctp,
        TransportKind::TlsSctp => sip_transport::TransportKind::TlsSctp,
    }
}
