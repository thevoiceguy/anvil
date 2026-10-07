//! SIP transport setup for Anvil.
//!
//! UDP is always on. TLS is opt-in via `Anvil`'s start flow and the `tls`
//! feature; when enabled, outbound TLS dispatches go through siphon-rs's
//! `TlsPool`, which keeps connections alive (RFC 5626 flow re-use) and
//! routes inbound responses on the same socket back into our packet pump.
//!
//! TCP listens on the UDP socket's port, so a server may send a request too
//! big for UDP (RFC 3261 §18.1.1) over TCP, and outbound TCP goes through a
//! connection pool. A request that arrived on a connection is answered on
//! it. No listening TLS socket — the softphone is a client, not a server.

use std::sync::Arc;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use bytes::Bytes;
use sip_transaction::{TransportContext, TransportDispatcher, TransportKind};
use sip_transport::{
    bind_tcp, pool::ConnectionPool, run_udp, send_stream, send_udp, serve_tcp,
    DefaultTransportPolicy, InboundPacket, TransportPolicy,
};
use tokio::{net::UdpSocket, sync::mpsc};

#[cfg(feature = "tls")]
use sip_transport::pool::{TlsClientConfig, TlsPool};

/// TLS client configuration. Wraps `Arc<rustls::ClientConfig>` so callers
/// don't need to depend on rustls themselves.
#[cfg(feature = "tls")]
pub(crate) type TlsConfigArc = Arc<TlsClientConfig>;

/// Build a TLS client config trusting the system root CAs (via
/// `webpki-roots`). If `extra_ca_pem` is supplied, its certs are added to
/// the trust store on top of the defaults — useful for self-signed PBX
/// environments.
#[cfg(feature = "tls")]
pub(crate) fn build_tls_client_config(extra_ca_pem: Option<&[u8]>) -> Result<TlsConfigArc> {
    use rustls::{ClientConfig, RootCertStore};

    // rustls 0.23 requires a CryptoProvider be installed process-wide
    // before any ClientConfig is built. We have only the `ring` provider
    // compiled in. Install once; subsequent calls are no-ops.
    install_default_crypto_provider();

    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    if let Some(pem) = extra_ca_pem {
        let mut cursor = std::io::BufReader::new(pem);
        for cert in rustls_pemfile::certs(&mut cursor) {
            let cert = cert.map_err(|e| anyhow!("parsing extra CA cert PEM: {e}"))?;
            roots
                .add(cert)
                .map_err(|e| anyhow!("adding extra CA cert: {e}"))?;
        }
    }

    let config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();

    Ok(Arc::new(config))
}

#[cfg(feature = "tls")]
fn install_default_crypto_provider() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Bring up the local UDP socket, the TCP listener and pool, and the
/// optional TLS pool; return a dispatcher that handles them all, and the
/// TCP pool for the transaction manager to share (it sends TCP itself). Inbound packets from either transport
/// land on the returned `mpsc::Receiver<InboundPacket>` so the packet
/// pump only has to read one channel.
pub(crate) async fn start_transports(
    bind_addr: &str,
    #[cfg(feature = "tls")] tls_config: Option<TlsConfigArc>,
    #[cfg(not(feature = "tls"))] _tls_config: Option<()>,
) -> Result<(
    Arc<dyn TransportDispatcher>,
    Arc<UdpSocket>,
    Arc<ConnectionPool>,
    mpsc::Receiver<InboundPacket>,
)> {
    let (tx, rx) = mpsc::channel::<InboundPacket>(1024);

    let udp_socket = Arc::new(UdpSocket::bind(bind_addr).await?);
    let tcp_pool = Arc::new(ConnectionPool::new());
    // Responses on connections we open come back to the same pump.
    tcp_pool.set_inbound_tx(tx.clone()).await;

    // TCP on the same address and port as UDP, bound before this returns:
    // a peer calling the moment Anvil has started must find it listening.
    let tcp_bind = udp_socket.local_addr()?.to_string();
    let tcp_listener = bind_tcp(&tcp_bind)?;
    tokio::spawn({
        let tx = tx.clone();
        async move {
            if let Err(e) = serve_tcp(tcp_listener, tx).await {
                tracing::warn!(%e, bind = %tcp_bind, "TCP listener exited");
            }
        }
    });

    #[cfg(feature = "tls")]
    let tls_pool: Option<Arc<TlsPool>> = if tls_config.is_some() {
        let pool = Arc::new(TlsPool::new());
        // Route inbound TLS responses (200 OK to REGISTER, etc.) back into
        // the same packet pump as UDP.
        pool.set_inbound_tx(tx.clone()).await;
        Some(pool)
    } else {
        None
    };

    let dispatcher: Arc<dyn TransportDispatcher> = Arc::new(AnvilTransportDispatcher {
        udp_socket: Arc::clone(&udp_socket),
        policy: Arc::new(DefaultTransportPolicy::default()),
        tcp_pool: Arc::clone(&tcp_pool),
        #[cfg(feature = "tls")]
        tls_pool,
        #[cfg(feature = "tls")]
        tls_config,
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

    Ok((dispatcher, udp_socket, tcp_pool, rx))
}

struct AnvilTransportDispatcher {
    udp_socket: Arc<UdpSocket>,
    policy: Arc<dyn TransportPolicy>,
    tcp_pool: Arc<ConnectionPool>,
    #[cfg(feature = "tls")]
    tls_pool: Option<Arc<TlsPool>>,
    #[cfg(feature = "tls")]
    tls_config: Option<TlsConfigArc>,
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

        // On the connection the request came in on, if it did.
        if let Some(stream) = ctx.stream() {
            if matches!(
                selected,
                sip_transport::TransportKind::Tcp | sip_transport::TransportKind::Tls
            ) {
                return send_stream(selected, stream, payload).await;
            }
        }

        let target = match selected {
            sip_transport::TransportKind::Tcp | sip_transport::TransportKind::Tls
                if ctx.stream().is_none() =>
            {
                // siphon-rs's policy may pick a stream transport for big
                // payloads; if there's no existing stream we still want to
                // open one, so don't fall back. Drop through to the pool.
                selected
            }
            other => other,
        };

        match target {
            sip_transport::TransportKind::Udp => {
                send_udp(self.udp_socket.as_ref(), &ctx.peer(), &payload).await?;
                Ok(())
            }
            #[cfg(feature = "tls")]
            sip_transport::TransportKind::Tls => {
                let pool = self
                    .tls_pool
                    .as_ref()
                    .ok_or_else(|| anyhow!("TLS dispatch requested but tls_pool is unset"))?;
                let cfg = self
                    .tls_config
                    .clone()
                    .ok_or_else(|| anyhow!("TLS dispatch requested but tls_config is unset"))?;
                let server_name = ctx
                    .server_name()
                    .map(String::from)
                    .unwrap_or_else(|| ctx.peer().ip().to_string());
                pool.send_tls(ctx.peer(), server_name, cfg, payload).await?;
                Ok(())
            }
            sip_transport::TransportKind::Tcp => {
                self.tcp_pool.send_tcp(ctx.peer(), payload).await?;
                Ok(())
            }
            sip_transport::TransportKind::Ws
            | sip_transport::TransportKind::Wss
            | sip_transport::TransportKind::Sctp
            | sip_transport::TransportKind::TlsSctp => Err(anyhow!(
                "transport {:?} not implemented in anvil-core",
                target
            )),
            #[cfg(not(feature = "tls"))]
            sip_transport::TransportKind::Tls => Err(anyhow!(
                "TLS dispatch requested but anvil-core was built without the `tls` feature"
            )),
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
