//! # anvil-core
//!
//! Softphone core. Orchestrates SIP signaling (`siphon-rs`) and the media engine
//! (`forge-media`). See `docs/ARCHITECTURE.md` in the workspace root for the design.
//!
//! This crate is pre-alpha. The public surface below is the target shape;
//! implementations are stubs until Phase 1 of the roadmap lands.

#![forbid(unsafe_code)]
// Re-enable when the public API stabilises.
// #![warn(missing_docs)]

pub mod audio;
pub mod brand;
pub mod config;
pub mod error;
pub mod event;
pub(crate) mod transport;

pub use brand::{
    BrandAsset, BrandCache, BrandColors, BrandCredential, BrandFetchOutcome,
    BrandLinks, BrandProfile, BrandProvider, BrandRequest, DialPlan, Ringtone, Rgba,
};
pub use config::{AccountConfig, AnvilConfig, BrandConfig, MediaConfig, Transport};
pub use error::AnvilError;
pub use event::{EndReason, Event, EventStream, MediaStats, RegState};

use std::sync::Arc;

use sip_core::SipUri;
use sip_dns::SipResolver;
use sip_transaction::TransactionManager;
use sip_uac::integrated::IntegratedUAC;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Opaque identifier for an active call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CallId(pub u64);

/// Softphone runtime handle.
///
/// Phase 1 scope: registration only. Call methods panic with `unimplemented!()`
/// until their respective milestones land.
pub struct Anvil {
    uac: Arc<IntegratedUAC>,
    registrar_uri: SipUri,
    events: mpsc::Sender<Event>,
    /// Handles to the packet-pump task and any other long-running tasks.
    /// Dropped on shutdown.
    tasks: Vec<JoinHandle<()>>,
}

impl Anvil {
    /// Start the softphone. Returns the runtime handle and an event stream.
    pub async fn start(cfg: AnvilConfig) -> Result<(Self, EventStream), AnvilError> {
        let local_addr = cfg
            .account
            .bind_addr
            .as_deref()
            .unwrap_or("0.0.0.0:0");

        let (dispatcher, _udp_socket, mut inbound_rx) =
            transport::start_udp_transport(local_addr)
                .await
                .map_err(|e| AnvilError::Transport(format!("start UDP: {e}")))?;

        let transaction_mgr = Arc::new(TransactionManager::new(Arc::clone(&dispatcher)));

        let resolver = Arc::new(
            SipResolver::from_system()
                .map_err(|e| AnvilError::Internal(format!("DNS resolver init: {e}")))?,
        );

        let local_uri = SipUri::parse(&cfg.account.aor)
            .map_err(|e| AnvilError::Config(format!("invalid AOR {:?}: {e:?}", cfg.account.aor)))?;

        // Resolve the bound UDP port so we can advertise it in Contact.
        let bound_addr = _udp_socket
            .local_addr()
            .map_err(|e| AnvilError::Transport(format!("local_addr: {e}")))?;

        // Builder takes AsRef<str>; SipUri doesn't impl it, so pass the AOR string.
        let mut uac_builder = IntegratedUAC::builder()
            .local_uri(&cfg.account.aor)
            .local_addr(bound_addr.to_string())
            .map_err(|e| AnvilError::Config(format!("local_addr: {e}")))?
            .transaction_manager(Arc::clone(&transaction_mgr))
            .resolver(Arc::clone(&resolver))
            .dispatcher(Arc::clone(&dispatcher))
            .credentials(cfg.account.username.clone(), cfg.account.password.clone());

        if let Some(proxy) = cfg.account.outbound_proxy.as_deref() {
            uac_builder = uac_builder
                .public_addr(proxy)
                .map_err(|e| AnvilError::Config(format!("outbound_proxy: {e}")))?;
        }

        let _ = local_uri; // reserved for later use (Contact construction)

        let uac = Arc::new(
            uac_builder
                .build()
                .map_err(|e| AnvilError::Internal(format!("UAC build: {e}")))?,
        );

        // Inbound packet pump: feed every response into the transaction manager so
        // `uac.register(...).await` can complete. Requests (incoming INVITE etc.)
        // are ignored for Phase 1.
        let pump = {
            let transaction_mgr = Arc::clone(&transaction_mgr);
            tokio::spawn(async move {
                while let Some(packet) = inbound_rx.recv().await {
                    let payload = packet.payload();
                    if let Some(response) = sip_parse::parse_response(&payload) {
                        transaction_mgr.receive_response(response).await;
                    } else if sip_parse::parse_request(&payload).is_some() {
                        tracing::debug!("inbound request received — not handled in Phase 1");
                    } else {
                        tracing::trace!(len = payload.len(), "inbound packet not SIP (keep-alive?)");
                    }
                }
            })
        };

        let registrar_uri = SipUri::parse(&cfg.account.registrar).map_err(|e| {
            AnvilError::Config(format!("invalid registrar {:?}: {e:?}", cfg.account.registrar))
        })?;

        let (event_tx, event_rx) = mpsc::channel::<Event>(64);

        Ok((
            Self {
                uac,
                registrar_uri,
                events: event_tx,
                tasks: vec![pump],
            },
            event_rx,
        ))
    }

    /// Send REGISTER to the configured registrar with the account's default
    /// expiry.
    pub async fn register(&self) -> Result<(), AnvilError> {
        let _ = self
            .events
            .send(Event::RegistrationChanged {
                state: RegState::Registering,
                reason: None,
            })
            .await;

        let expires: u32 = 3600;
        match self
            .uac
            .register(self.registrar_uri.clone(), Some(expires))
            .await
        {
            Ok(resp) if (200..300).contains(&resp.code()) => {
                let _ = self
                    .events
                    .send(Event::RegistrationChanged {
                        state: RegState::Registered,
                        reason: None,
                    })
                    .await;
                Ok(())
            }
            Ok(resp) => {
                let reason = format!("{} {}", resp.code(), resp.reason());
                let _ = self
                    .events
                    .send(Event::RegistrationChanged {
                        state: RegState::Failed,
                        reason: Some(reason.clone()),
                    })
                    .await;
                Err(AnvilError::AuthRejected(reason))
            }
            Err(e) => {
                let reason = e.to_string();
                let _ = self
                    .events
                    .send(Event::RegistrationChanged {
                        state: RegState::Failed,
                        reason: Some(reason.clone()),
                    })
                    .await;
                Err(AnvilError::Transport(reason))
            }
        }
    }

    /// Unregister (REGISTER with Expires: 0).
    pub async fn unregister(&self) -> Result<(), AnvilError> {
        match self.uac.register(self.registrar_uri.clone(), Some(0)).await {
            Ok(resp) if (200..300).contains(&resp.code()) => {
                let _ = self
                    .events
                    .send(Event::RegistrationChanged {
                        state: RegState::Unregistered,
                        reason: None,
                    })
                    .await;
                Ok(())
            }
            Ok(resp) => Err(AnvilError::Transport(format!(
                "unregister: {} {}",
                resp.code(),
                resp.reason()
            ))),
            Err(e) => Err(AnvilError::Transport(e.to_string())),
        }
    }

    /// Place an outgoing call. Phase 2.
    pub async fn place_call(&self, _target: &str) -> Result<CallId, AnvilError> {
        Err(AnvilError::Internal("place_call not implemented in Phase 1 M1".into()))
    }

    /// Answer an incoming call. Phase 2.
    pub async fn answer(&self, _call: CallId) -> Result<(), AnvilError> {
        Err(AnvilError::Internal("answer not implemented in Phase 1 M1".into()))
    }

    /// Reject an incoming call. Phase 2.
    pub async fn reject(&self, _call: CallId, _code: u16) -> Result<(), AnvilError> {
        Err(AnvilError::Internal("reject not implemented in Phase 1 M1".into()))
    }

    /// Hang up an active call. Phase 2.
    pub async fn hangup(&self, _call: CallId) -> Result<(), AnvilError> {
        Err(AnvilError::Internal("hangup not implemented in Phase 1 M1".into()))
    }

    /// Send a DTMF digit. Phase 2.
    pub async fn send_dtmf(&self, _call: CallId, _digit: char) -> Result<(), AnvilError> {
        Err(AnvilError::Internal("send_dtmf not implemented in Phase 1 M1".into()))
    }

    /// Put a call on hold. Phase 2.
    pub async fn hold(&self, _call: CallId, _hold: bool) -> Result<(), AnvilError> {
        Err(AnvilError::Internal("hold not implemented in Phase 1 M1".into()))
    }

    /// Blind-transfer a call. Phase 2.
    pub async fn transfer(&self, _call: CallId, _target: &str) -> Result<(), AnvilError> {
        Err(AnvilError::Internal("transfer not implemented in Phase 1 M1".into()))
    }

    /// Gracefully stop the runtime. Aborts background tasks.
    pub async fn shutdown(self) -> Result<(), AnvilError> {
        for handle in self.tasks {
            handle.abort();
        }
        Ok(())
    }
}
