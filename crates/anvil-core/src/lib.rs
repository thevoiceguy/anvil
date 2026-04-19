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
pub(crate) mod call;
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

use std::net::IpAddr;
use std::sync::Arc;

use dashmap::DashMap;
use sip_core::SipUri;
use sip_dns::SipResolver;
use sip_sdp::profiles::MediaProfileBuilder;
use sip_transaction::TransactionManager;
use sip_uac::integrated::IntegratedUAC;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::call::CallEntry;
use crate::config::Codec;

/// Opaque identifier for an active call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CallId(pub u64);

/// Softphone runtime handle.
pub struct Anvil {
    uac: Arc<IntegratedUAC>,
    registrar_uri: SipUri,
    events: mpsc::Sender<Event>,
    calls: Arc<DashMap<CallId, CallEntry>>,
    /// IP used in SDP offers. Defaults to loopback; `AccountConfig::bind_addr`
    /// overrides when set to a specific host.
    media_ip: IpAddr,
    /// AOR user component, used as the SDP origin username.
    local_user: String,
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

        // Pick the media IP. For Phase 1 M2 we derive it from the bind address
        // if the caller pinned one; otherwise fall back to loopback. Production
        // deployments will need STUN or an explicit public IP (Phase 2).
        let media_ip = cfg
            .account
            .bind_addr
            .as_deref()
            .and_then(|s| s.rsplit_once(':').map(|(host, _)| host))
            .and_then(|host| host.parse::<IpAddr>().ok())
            .unwrap_or_else(call::default_media_ip);

        let local_user = local_uri
            .user()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "anvil".to_string());

        Ok((
            Self {
                uac,
                registrar_uri,
                events: event_tx,
                calls: Arc::new(DashMap::new()),
                media_ip,
                local_user,
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

    /// Place an outgoing call.
    ///
    /// Sends an INVITE with an SDP offer (G.711 µ-law + A-law), returns as soon
    /// as the transaction is started. `Event::CallRinging`, `CallEstablished`,
    /// and `CallEnded` follow asynchronously on the event stream.
    ///
    /// Phase 1 M2: no media flow. The RTP port advertised in the SDP offer is
    /// reserved by a bound UDP socket stored in the call entry, so the audio
    /// milestone can drop a media engine onto it without renegotiating SDP.
    pub async fn place_call(&self, target: &str) -> Result<CallId, AnvilError> {
        let target_uri = SipUri::parse(target)
            .map_err(|e| AnvilError::Config(format!("invalid target {target:?}: {e:?}")))?;

        let rtp_socket = call::bind_rtp_socket(self.media_ip)
            .await
            .map_err(|e| AnvilError::Media(format!("bind RTP socket: {e}")))?;
        let rtp_port = rtp_socket
            .local_addr()
            .map_err(|e| AnvilError::Media(format!("rtp local_addr: {e}")))?
            .port();

        // Minimal G.711 offer. Media direction defaults to sendrecv.
        let sdp = MediaProfileBuilder::audio_only()
            .add_audio_codec(0, "PCMU", 8000)
            .add_audio_codec(8, "PCMA", 8000)
            .telephone_event(true)
            .build(
                &self.local_user,
                &self.media_ip.to_string(),
                rtp_port,
                None,
            );
        let sdp_body = sdp.to_string();

        tracing::debug!(%target, rtp_port, "sending INVITE");
        let handle = self
            .uac
            .invite(target_uri, Some(&sdp_body))
            .await
            .map_err(|e| AnvilError::Transport(format!("INVITE: {e}")))?;

        let handle = Arc::new(handle);
        let call_id = call::next_call_id();

        self.calls.insert(
            call_id,
            CallEntry {
                handle: Arc::clone(&handle),
                negotiated_codec: None,
                rtp_socket,
            },
        );

        // Spawn a task to drive the call's state machine: drain provisionals,
        // await final, emit events, and clean up on termination.
        let events = self.events.clone();
        let calls = Arc::clone(&self.calls);
        tokio::spawn(async move {
            drive_outgoing_call(call_id, handle, calls, events).await;
        });

        Ok(call_id)
    }

    /// Answer an incoming call. Phase 2.
    pub async fn answer(&self, _call: CallId) -> Result<(), AnvilError> {
        Err(AnvilError::Internal("answer not implemented in Phase 1 M1".into()))
    }

    /// Reject an incoming call. Phase 2.
    pub async fn reject(&self, _call: CallId, _code: u16) -> Result<(), AnvilError> {
        Err(AnvilError::Internal("reject not implemented in Phase 1 M1".into()))
    }

    /// Hang up an active call. Sends BYE on a confirmed dialog; for an
    /// in-progress INVITE (no 2xx yet) this still issues BYE, which siphon-rs
    /// rejects with "Cannot BYE on early dialog" — callers should wait for
    /// `CallEstablished` before hanging up in Phase 1 M2. CANCEL support lands
    /// in Phase 2.
    pub async fn hangup(&self, call: CallId) -> Result<(), AnvilError> {
        let entry = self
            .calls
            .get(&call)
            .ok_or(AnvilError::NoSuchCall(call))?;

        let dialog = entry.handle.dialog.read().await.clone();
        drop(entry); // release the DashMap read guard before awaiting

        match self.uac.bye(&dialog).await {
            Ok(resp) if (200..300).contains(&resp.code()) => {
                self.calls.remove(&call);
                let _ = self
                    .events
                    .send(Event::CallEnded {
                        call,
                        reason: EndReason::LocalHangup,
                    })
                    .await;
                Ok(())
            }
            Ok(resp) => {
                let reason = format!("BYE rejected: {} {}", resp.code(), resp.reason());
                Err(AnvilError::Transport(reason))
            }
            Err(e) => Err(AnvilError::Transport(format!("BYE: {e}"))),
        }
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

/// Drives an outgoing call's lifecycle: drains provisional responses and
/// awaits the final response. Emits `CallRinging`, `CallEstablished`, and
/// `CallEnded` events; leaves the `CallEntry` in place for the app to `hangup`.
async fn drive_outgoing_call(
    call_id: CallId,
    handle: Arc<sip_uac::integrated::CallHandle>,
    calls: Arc<DashMap<CallId, CallEntry>>,
    events: mpsc::Sender<Event>,
) {
    // Drain provisional responses in parallel with await_final. We only care
    // about 180 Ringing at this milestone; other 1xx responses (100, 183)
    // are logged but not surfaced.
    let prov_events = events.clone();
    let prov_handle = Arc::clone(&handle);
    tokio::spawn(async move {
        while let Some(resp) = prov_handle.await_provisional().await {
            match resp.code() {
                180 => {
                    let _ = prov_events
                        .send(Event::CallRinging { call: call_id })
                        .await;
                }
                other => {
                    tracing::debug!(code = other, reason = %resp.reason(), "provisional");
                }
            }
        }
    });

    match handle.await_final().await {
        Ok(resp) if (200..300).contains(&resp.code()) => {
            // Extract the negotiated codec from the answer. The payload-type
            // table is small (0 = PCMU, 8 = PCMA) and both are mandatory for
            // RFC 3551-compliant endpoints, so for Phase 1 M2 we just check
            // the first audio format byte. Proper negotiation (matching rtpmap
            // names, dropping unknown PTs) arrives with the codec milestone.
            let codec = extract_codec_from_answer(resp.body().as_ref()).unwrap_or(Codec::Pcmu);

            if let Some(mut entry) = calls.get_mut(&call_id) {
                entry.negotiated_codec = Some(codec);
            }

            let _ = events
                .send(Event::CallEstablished { call: call_id, codec })
                .await;
        }
        Ok(resp) => {
            calls.remove(&call_id);
            let _ = events
                .send(Event::CallEnded {
                    call: call_id,
                    reason: EndReason::Rejected(resp.code()),
                })
                .await;
        }
        Err(e) => {
            calls.remove(&call_id);
            let _ = events
                .send(Event::CallEnded {
                    call: call_id,
                    reason: EndReason::Error(e.to_string()),
                })
                .await;
        }
    }
}

/// Best-effort codec detection from an SDP answer body. Returns `None` if the
/// body isn't parseable as SDP or contains no audio media line. Phase 1 M2
/// only distinguishes PCMU/PCMA/G722/Opus by payload-type name; Phase 2
/// replaces this with `sip_sdp::negotiate`.
fn extract_codec_from_answer(body: &[u8]) -> Option<Codec> {
    let text = std::str::from_utf8(body).ok()?;
    let sdp = sip_sdp::parse::parse_sdp(text).ok()?;
    let media = sdp
        .media
        .iter()
        .find(|m| m.media_type == sip_sdp::MediaType::Audio)?;
    // The first format in the m= line is the preferred codec, per RFC 3264 §6.1.
    let first_pt: u8 = media.formats.first()?.parse().ok()?;
    match first_pt {
        0 => Some(Codec::Pcmu),
        8 => Some(Codec::Pcma),
        9 => Some(Codec::G722),
        // Dynamic payload types — look up by rtpmap encoding name.
        _ => {
            let name = media.rtpmaps.get(&first_pt)?.encoding_name.as_str();
            match name.to_ascii_lowercase().as_str() {
                "opus"  => Some(Codec::Opus),
                "pcmu"  => Some(Codec::Pcmu),
                "pcma"  => Some(Codec::Pcma),
                "g722"  => Some(Codec::G722),
                _       => None,
            }
        }
    }
}
