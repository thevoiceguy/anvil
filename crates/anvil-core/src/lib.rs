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
pub(crate) mod media;
mod registration;
pub(crate) mod stun;
pub(crate) mod transport;
pub(crate) mod uas;

pub use brand::{
    BrandAsset, BrandCache, BrandColors, BrandCredential, BrandFetchOutcome, BrandLinks,
    BrandProfile, BrandProvider, BrandRequest, DialPlan, Rgba, Ringtone,
};
pub use config::{AccountConfig, AnvilConfig, BrandConfig, MediaConfig, Transport};
pub use error::AnvilError;
pub use event::{EndReason, Event, EventStream, MediaStats, RegState};

use std::net::IpAddr;
use std::sync::Arc;

use dashmap::DashMap;
use sip_core::SipUri;
use sip_dns::SipResolver;
use sip_transaction::TransactionManager;
use sip_uac::integrated::IntegratedUAC;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::audio::AudioHost;
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
    audio: Arc<dyn AudioHost>,
    /// IP used in SDP offers. Defaults to loopback; `AccountConfig::bind_addr`
    /// overrides when set to a specific host.
    media_ip: IpAddr,
    /// AOR user component, used as the SDP origin username.
    local_user: String,
    /// Where peers reach us (the media IP with the SIP port).
    advertised: std::net::SocketAddr,
    /// The registration expiry asked for; the registrar may grant less.
    register_expires: u32,
    /// The codecs offered and accepted, in order of preference.
    codecs: Arc<[Codec]>,
    /// The task keeping the registration alive, while registered.
    refresh: parking_lot::Mutex<Option<JoinHandle<()>>>,
    /// The tenant's brand, when a provider is configured.
    branding: Option<Arc<brand::Branding>>,
    /// Handles to the packet-pump task and any other long-running tasks.
    /// Dropped on shutdown.
    tasks: Vec<JoinHandle<()>>,
}

impl Anvil {
    /// Start the softphone. Returns the runtime handle and an event stream.
    pub async fn start(cfg: AnvilConfig) -> Result<(Self, EventStream), AnvilError> {
        let local_addr = cfg.account.bind_addr.as_deref().unwrap_or("0.0.0.0:0");

        // Build the TLS client config if the account is configured for TLS.
        // Even if the account uses UDP, dispatching to a sips: target later
        // would still need the config — but we keep the policy simple: TLS
        // is on iff Transport::Tls.
        #[cfg(feature = "tls")]
        let tls_config: Option<transport::TlsConfigArc> =
            if matches!(cfg.account.transport, crate::config::Transport::Tls) {
                Some(
                    transport::build_tls_client_config(cfg.account.tls_extra_ca_pem.as_deref())
                        .map_err(|e| AnvilError::Transport(format!("TLS config: {e}")))?,
                )
            } else {
                None
            };

        let (dispatcher, _udp_socket, tcp_pool, mut inbound_rx) = transport::start_transports(
            local_addr,
            #[cfg(feature = "tls")]
            tls_config,
            #[cfg(not(feature = "tls"))]
            None,
        )
        .await
        .map_err(|e| AnvilError::Transport(format!("start transports: {e}")))?;

        // One TCP pool for the manager (which sends TCP itself) and the
        // dispatcher, reading responses into the packet pump.
        let transaction_mgr = Arc::new(TransactionManager::new_with_pool(
            Arc::clone(&dispatcher),
            tcp_pool,
        ));

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

        // Pick the media IP. Order:
        //   1. STUN reflexive address if `account.stun` is configured and
        //      reachable. Best for clients behind NAT.
        //   2. Pinned host from `account.bind_addr` (`1.2.3.4:5060` → 1.2.3.4).
        //   3. Loopback. Useful for tests; useless on the open net.
        let stun_ip = if let Some(server) = cfg.account.stun.as_deref() {
            match stun::discover_public_addr(server, std::time::Duration::from_secs(3)).await {
                Ok(addr) => {
                    tracing::info!(public_addr = %addr, server, "STUN discovered public IP");
                    Some(addr.ip())
                }
                Err(e) => {
                    tracing::warn!(%e, server, "STUN discovery failed; falling back to bind_addr");
                    None
                }
            }
        } else {
            None
        };
        let media_ip = stun_ip.unwrap_or_else(|| {
            cfg.account
                .bind_addr
                .as_deref()
                .and_then(|s| s.rsplit_once(':').map(|(host, _)| host))
                .and_then(|host| host.parse::<IpAddr>().ok())
                .unwrap_or_else(call::default_media_ip)
        });

        // Where peers reach us: the media IP with the SIP port, in REGISTER's
        // Contact and in our answers. The bound address may be 0.0.0.0.
        let advertised = std::net::SocketAddr::new(media_ip, bound_addr.port());

        // Builder takes AsRef<str>; SipUri doesn't impl it, so pass the AOR string.
        let mut uac_builder = IntegratedUAC::builder()
            .local_uri(&cfg.account.aor)
            .map_err(|e| AnvilError::Config(format!("aor: {e}")))?
            .local_addr(bound_addr.to_string())
            .map_err(|e| AnvilError::Config(format!("local_addr: {e}")))?
            .transaction_manager(Arc::clone(&transaction_mgr))
            .resolver(Arc::clone(&resolver))
            .dispatcher(Arc::clone(&dispatcher))
            .contact_advertised_addr(advertised.to_string())
            .map_err(|e| AnvilError::Config(format!("contact address: {e}")))?
            .credentials(cfg.account.username.clone(), cfg.account.password.clone());

        // Out-of-dialog requests (REGISTER, a new INVITE, …) go through
        // the outbound proxy, the Request-URI left as the target: how an
        // account in a domain reaches its provider's server.
        if let Some(proxy) = cfg.account.outbound_proxy.as_deref() {
            uac_builder = uac_builder
                .outbound_proxy(proxy)
                .map_err(|e| AnvilError::Config(format!("outbound_proxy: {e}")))?;
        }
        if !cfg.account.user_agent.is_empty() {
            uac_builder = uac_builder.user_agent(cfg.account.user_agent.clone());
        }

        let uac = Arc::new(
            uac_builder
                .build()
                .map_err(|e| AnvilError::Internal(format!("UAC build: {e}")))?,
        );

        let registrar_uri = SipUri::parse(&cfg.account.registrar).map_err(|e| {
            AnvilError::Config(format!(
                "invalid registrar {:?}: {e:?}",
                cfg.account.registrar
            ))
        })?;

        let (event_tx, event_rx) = mpsc::channel::<Event>(64);

        let local_user = local_uri
            .user()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "anvil".to_string());

        // The configured codecs Anvil supports, in the configured order.
        let mut configured: Vec<Codec> = Vec::new();
        for codec in &cfg.media.codecs {
            if media::supported_codecs().contains(codec) && !configured.contains(codec) {
                configured.push(*codec);
            }
        }
        let codecs: Arc<[Codec]> = configured.into();
        if codecs.is_empty() {
            return Err(AnvilError::Config(
                "media.codecs names no codec Anvil supports".into(),
            ));
        }
        let register_expires = u32::try_from(cfg.account.register_expires.as_secs())
            .unwrap_or(u32::MAX)
            .max(1);

        let audio: Arc<dyn AudioHost> = Arc::from(cfg.audio);
        let branding = brand::Branding::new(
            cfg.brand,
            cfg.account.aor.clone(),
            cfg.account.provisioning_url.clone(),
            event_tx.clone(),
        )
        .map(Arc::new);
        let mut tasks = Vec::new();
        if let Some(branding) = &branding {
            // The cached profile at once, then a fresh one: concurrently with
            // REGISTER (docs/BRANDING.md §1).
            let branding = Arc::clone(branding);
            tasks.push(tokio::spawn(async move {
                branding.load_cached().await;
                let _ = branding.refresh().await;
            }));
        }
        let calls: Arc<DashMap<CallId, CallEntry>> = Arc::new(DashMap::new());

        // UAS handler — routed to directly from the packet pump rather than
        // via IntegratedUAS. IntegratedUAS builds its own DialogManager that
        // we can't share the created dialog with, so BYE lookups would miss.
        // Routing directly lets us use our CallEntry map as the source of
        // truth for dialog-to-call matching.
        let uas_handler = Arc::new(uas::AnvilUasHandler {
            events: event_tx.clone(),
            calls: Arc::clone(&calls),
            media_ip,
            local_user: local_user.clone(),
            codecs: Arc::clone(&codecs),
            local_sip_addr: advertised,
        });

        let pump = {
            let transaction_mgr = Arc::clone(&transaction_mgr);
            let uas_handler = Arc::clone(&uas_handler);
            tokio::spawn(async move {
                while let Some(packet) = inbound_rx.recv().await {
                    let payload = packet.payload();
                    if let Some(response) = sip_parse::parse_response(payload) {
                        transaction_mgr.receive_response(response).await;
                    } else if let Some(request) = sip_parse::parse_request(payload) {
                        let ctx = sip_transaction::TransportContext::new(
                            map_transport(packet.transport()),
                            packet.peer(),
                            packet.stream().cloned(),
                        );
                        dispatch_request(&transaction_mgr, &uas_handler, request, ctx).await;
                    } else {
                        tracing::trace!(
                            len = payload.len(),
                            "inbound packet not SIP (keep-alive?)"
                        );
                    }
                }
            })
        };

        Ok((
            Self {
                uac,
                registrar_uri,
                events: event_tx,
                calls,
                audio,
                media_ip,
                local_user,
                advertised,
                register_expires,
                codecs,
                refresh: parking_lot::Mutex::new(None),
                branding,
                tasks: {
                    tasks.push(pump);
                    tasks
                },
            },
            event_rx,
        ))
    }

    /// Fetch the tenant's brand again now (`docs/BRANDING.md` §7): a
    /// changed one arrives as [`Event::BrandUpdated`]. Nothing happens
    /// without a brand provider or a provisioning URL.
    pub async fn refresh_brand(&self) -> Result<(), AnvilError> {
        match &self.branding {
            Some(branding) => branding.refresh().await,
            None => Ok(()),
        }
    }

    /// Where peers reach this softphone: its advertised SIP address, the
    /// host and port of its Contact (UDP and TCP both listen there).
    pub fn sip_address(&self) -> std::net::SocketAddr {
        self.advertised
    }

    /// Register with the configured registrar, and keep the registration
    /// alive: it is refreshed before the expiry the registrar granted, and
    /// retried with a growing delay if a refresh fails, until
    /// [`unregister`](Self::unregister) or [`shutdown`](Self::shutdown).
    pub async fn register(&self) -> Result<(), AnvilError> {
        self.stop_refresh();
        let _ = self
            .events
            .send(Event::RegistrationChanged {
                state: RegState::Registering,
                reason: None,
            })
            .await;
        let granted =
            match register_once(&self.uac, &self.registrar_uri, self.register_expires).await {
                Ok((granted, provisioning_url)) => {
                    learn_brand_url(self.branding.as_ref(), provisioning_url.as_deref());
                    granted
                }
                Err(e) => {
                    let _ = self
                        .events
                        .send(Event::RegistrationChanged {
                            state: RegState::Failed,
                            reason: Some(e.to_string()),
                        })
                        .await;
                    return Err(e);
                }
            };
        let _ = self
            .events
            .send(Event::RegistrationChanged {
                state: RegState::Registered,
                reason: None,
            })
            .await;
        let task = tokio::spawn(keep_registered(
            Arc::clone(&self.uac),
            self.registrar_uri.clone(),
            self.register_expires,
            granted,
            self.events.clone(),
            self.branding.clone(),
        ));
        *self.refresh.lock() = Some(task);
        Ok(())
    }

    /// Unregister (REGISTER with Expires: 0), and stop refreshing.
    pub async fn unregister(&self) -> Result<(), AnvilError> {
        self.stop_refresh();
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

    fn stop_refresh(&self) {
        if let Some(task) = self.refresh.lock().take() {
            task.abort();
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

        // Offer everything we can drive today, in preference order.
        let sdp_body = media::build_sdp(
            &self.codecs,
            &self.local_user,
            &self.media_ip.to_string(),
            rtp_port,
            media::MediaDirection::Sendrecv,
        );

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
                outbound_handle: Some(Arc::clone(&handle)),
                inbound: None,
                dialog: None,
                negotiated_codec: None,
                rtp_socket: Arc::clone(&rtp_socket),
                pipeline: None,
            },
        );

        // Spawn a task to drive the call's state machine: drain provisionals,
        // await final, start the media pipeline,
        // emit events.
        let events = self.events.clone();
        let calls = Arc::clone(&self.calls);
        let audio = Arc::clone(&self.audio);
        tokio::spawn(async move {
            drive_outgoing_call(call_id, handle, rtp_socket, audio, calls, events).await;
        });

        Ok(call_id)
    }

    /// Answer an incoming call that was signalled via `Event::IncomingCall`.
    pub async fn answer(&self, call: CallId) -> Result<(), AnvilError> {
        let codec = uas::send_answer_and_start_media(
            call,
            &self.calls,
            &self.audio,
            &self.events,
            &self.codecs,
        )
        .await?;
        let _ = self
            .events
            .send(Event::CallEstablished { call, codec })
            .await;
        Ok(())
    }

    /// Reject an incoming call with the given SIP response code
    /// (e.g. 486 Busy Here, 603 Decline).
    pub async fn reject(&self, call: CallId, code: u16) -> Result<(), AnvilError> {
        uas::reject_inbound(call, code, &self.calls).await?;
        let _ = self
            .events
            .send(Event::CallEnded {
                call,
                reason: EndReason::LocalHangup,
            })
            .await;
        Ok(())
    }

    /// Hang up a call: BYE on an answered one, CANCEL on an outgoing one
    /// not yet answered, and 603 Decline on an incoming one still ringing.
    pub async fn hangup(&self, call: CallId) -> Result<(), AnvilError> {
        // Pull out the dialog source, then release the DashMap guard before
        // awaiting.
        #[allow(clippy::large_enum_variant)] // a moment on the stack
        enum DialogSource {
            Confirmed(sip_dialog::Dialog),
            Outbound(Arc<sip_uac::integrated::CallHandle>),
            Ringing,
        }
        let src = {
            let entry = self.calls.get(&call).ok_or(AnvilError::NoSuchCall(call))?;
            if let Some(dlg) = entry.dialog.clone() {
                DialogSource::Confirmed(dlg)
            } else if let Some(h) = entry.outbound_handle.clone() {
                DialogSource::Outbound(h)
            } else if entry.inbound.is_some() {
                DialogSource::Ringing
            } else {
                return Err(AnvilError::Internal(format!("{call:?} has no dialog")));
            }
        };
        let dialog = match src {
            DialogSource::Confirmed(d) => d,
            DialogSource::Ringing => return self.reject(call, 603).await,
            DialogSource::Outbound(h) => {
                let dialog = h.dialog.read().await.clone();
                if dialog.state() != sip_dialog::DialogStateType::Confirmed {
                    return self.cancel(call, &h).await;
                }
                dialog
            }
        };

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

    /// CANCEL an outgoing call not yet answered. The call ends here; the
    /// 487 that follows is not reported again.
    async fn cancel(
        &self,
        call: CallId,
        handle: &sip_uac::integrated::CallHandle,
    ) -> Result<(), AnvilError> {
        if let Some((_, entry)) = self.calls.remove(&call) {
            drop(entry);
            let _ = self
                .events
                .send(Event::CallEnded {
                    call,
                    reason: EndReason::LocalHangup,
                })
                .await;
        }
        match handle.cancel().await {
            Ok(resp) if (200..300).contains(&resp.code()) => Ok(()),
            // The 200 OK crossed the CANCEL: the call was answered after
            // all, and is ended with a BYE.
            Ok(resp) if resp.code() == 481 => {
                let dialog = handle.dialog.read().await.clone();
                self.uac
                    .bye(&dialog)
                    .await
                    .map(|_| ())
                    .map_err(|e| AnvilError::Transport(e.to_string()))
            }
            Ok(resp) => Err(AnvilError::Transport(format!(
                "CANCEL answered {} {}",
                resp.code(),
                resp.reason()
            ))),
            Err(e) => Err(AnvilError::Transport(e.to_string())),
        }
    }

    /// Send a DTMF digit on an active call as an RFC 2833 event burst
    /// (1 start + 3 continue + 3 end packets, ~140 ms total). The call must
    /// have an active media pipeline; queue-pre-call digits aren't supported
    /// in M4.
    pub async fn send_dtmf(&self, call: CallId, digit: char) -> Result<(), AnvilError> {
        let tx = {
            let entry = self.calls.get(&call).ok_or(AnvilError::NoSuchCall(call))?;
            entry
                .pipeline
                .as_ref()
                .map(|p| p.dtmf_tx.clone())
                .ok_or_else(|| AnvilError::Internal(format!("{call:?} has no media")))?
        };
        tx.send(digit)
            .await
            .map_err(|_| AnvilError::Internal("DTMF channel closed".into()))
    }

    /// Put a call on hold (`on = true`) or resume it (`on = false`). Sends
    /// an in-dialog re-INVITE with the appropriate direction attribute and
    /// applies the state locally once the peer confirms with a 2xx.
    pub async fn hold(&self, call: CallId, on: bool) -> Result<(), AnvilError> {
        // Pull out what we need under the DashMap guard, then release it
        // before awaiting.
        struct Bundle {
            dialog_src: DialogSource,
            hold_state: Arc<media::HoldState>,
            rtp_port: u16,
            codec: Codec,
        }
        #[allow(clippy::large_enum_variant)] // a moment on the stack
        enum DialogSource {
            Confirmed(sip_dialog::Dialog),
            Outbound(Arc<sip_uac::integrated::CallHandle>),
        }

        let bundle = {
            let entry = self.calls.get(&call).ok_or(AnvilError::NoSuchCall(call))?;
            let pipeline = entry
                .pipeline
                .as_ref()
                .ok_or_else(|| AnvilError::Internal(format!("{call:?} has no media")))?;
            let hold_state = Arc::clone(&pipeline.hold);
            let codec = entry
                .negotiated_codec
                .ok_or_else(|| AnvilError::Internal(format!("{call:?} has no codec")))?;
            let rtp_port = entry
                .rtp_socket
                .local_addr()
                .map_err(|e| AnvilError::Media(format!("rtp local_addr: {e}")))?
                .port();
            let dialog_src = if let Some(dlg) = entry.dialog.clone() {
                DialogSource::Confirmed(dlg)
            } else if let Some(h) = entry.outbound_handle.clone() {
                DialogSource::Outbound(h)
            } else {
                return Err(AnvilError::Internal(format!("{call:?} has no dialog")));
            };
            Bundle {
                dialog_src,
                hold_state,
                rtp_port,
                codec,
            }
        };

        let mut dialog = match bundle.dialog_src {
            DialogSource::Confirmed(d) => d,
            DialogSource::Outbound(h) => h.dialog.read().await.clone(),
        };

        let direction = if on {
            media::MediaDirection::Sendonly
        } else {
            media::MediaDirection::Sendrecv
        };

        let sdp_body = media::build_sdp(
            &[bundle.codec],
            &self.local_user,
            &self.media_ip.to_string(),
            bundle.rtp_port,
            direction,
        );

        let handle = self
            .uac
            .reinvite(&mut dialog, Some(&sdp_body))
            .await
            .map_err(|e| AnvilError::Transport(format!("re-INVITE: {e}")))?;

        let resp = handle
            .await_final()
            .await
            .map_err(|e| AnvilError::Transport(format!("re-INVITE await: {e}")))?;
        if !(200..300).contains(&resp.code()) {
            return Err(AnvilError::Transport(format!(
                "re-INVITE rejected: {} {}",
                resp.code(),
                resp.reason()
            )));
        }

        // Apply locally. Per RFC 3264, our direction in the offer is what
        // we want to do; the answer confirms it and mirrors the peer's.
        bundle.hold_state.apply(direction);
        Ok(())
    }

    /// Blind-transfer a call. Phase 2.
    pub async fn transfer(&self, _call: CallId, _target: &str) -> Result<(), AnvilError> {
        Err(AnvilError::Internal(
            "transfer not implemented in Phase 1 M1".into(),
        ))
    }

    /// Gracefully stop the runtime. Aborts background tasks.
    pub async fn shutdown(self) -> Result<(), AnvilError> {
        self.stop_refresh();
        for handle in self.tasks {
            handle.abort();
        }
        Ok(())
    }
}

/// A provisioning URL a REGISTER 200 OK named, learned by the branding,
/// and a first fetch from it when it is new.
fn learn_brand_url(branding: Option<&Arc<brand::Branding>>, url: Option<&str>) {
    if let (Some(branding), Some(url)) = (branding, url) {
        if branding.learn_url(url) {
            let branding = Arc::clone(branding);
            tokio::spawn(async move {
                let _ = branding.refresh().await;
            });
        }
    }
}

/// One REGISTER; the expiry the registrar granted, in seconds, and the
/// provisioning URL its 200 OK named (`X-FCP-Provisioning-Url`). A 423
/// Interval Too Brief is asked again with the registrar's `Min-Expires`
/// (RFC 3261 §10.2.8).
async fn register_once(
    uac: &IntegratedUAC,
    registrar: &SipUri,
    expires: u32,
) -> Result<(u32, Option<String>), AnvilError> {
    let mut asked = expires;
    for _ in 0..2 {
        match uac.register(registrar.clone(), Some(asked)).await {
            Ok(resp) if (200..300).contains(&resp.code()) => {
                let provisioning_url = resp
                    .headers()
                    .get("X-FCP-Provisioning-Url")
                    .map(|v| v.trim().to_string())
                    .filter(|v| v.starts_with("https://") || v.starts_with("http://"));
                return Ok((
                    registration::granted_expires(&resp, asked),
                    provisioning_url,
                ));
            }
            Ok(resp) if resp.code() == 423 => match registration::min_expires(&resp) {
                Some(min) if min > asked => asked = min,
                _ => return Err(AnvilError::AuthRejected("423 Interval Too Brief".into())),
            },
            Ok(resp) => {
                return Err(AnvilError::AuthRejected(format!(
                    "{} {}",
                    resp.code(),
                    resp.reason()
                )))
            }
            Err(e) => return Err(AnvilError::Transport(e.to_string())),
        }
    }
    Err(AnvilError::AuthRejected("423 Interval Too Brief".into()))
}

/// Refresh a registration before it lapses; on a failed refresh, say so and
/// retry with a growing delay, and say so again when it comes back.
async fn keep_registered(
    uac: Arc<IntegratedUAC>,
    registrar: SipUri,
    expires: u32,
    mut granted: u32,
    events: mpsc::Sender<Event>,
    branding: Option<Arc<brand::Branding>>,
) {
    let mut failures: u32 = 0;
    loop {
        let wait = if failures == 0 {
            registration::refresh_after(granted)
        } else {
            registration::retry_after(failures)
        };
        tokio::time::sleep(wait).await;
        match register_once(&uac, &registrar, expires).await {
            Ok((g, provisioning_url)) => {
                granted = g;
                learn_brand_url(branding.as_ref(), provisioning_url.as_deref());
                // Opportunistically, at most hourly (docs/BRANDING.md §7).
                if let Some(branding) = &branding {
                    let branding = Arc::clone(branding);
                    tokio::spawn(async move { branding.refresh_if_stale().await });
                }
                if failures > 0 {
                    let _ = events
                        .send(Event::RegistrationChanged {
                            state: RegState::Registered,
                            reason: None,
                        })
                        .await;
                }
                failures = 0;
            }
            Err(e) => {
                failures = failures.saturating_add(1);
                tracing::warn!(%e, failures, "registration refresh failed");
                if failures == 1 {
                    let _ = events
                        .send(Event::RegistrationChanged {
                            state: RegState::Failed,
                            reason: Some(e.to_string()),
                        })
                        .await;
                }
            }
        }
    }
}

/// Minimal UAS dispatch — invoke the right AnvilUasHandler method for each
/// incoming request method. Replaces `IntegratedUAS::dispatch` because we
/// need to own the DialogManager (see uas.rs for context).
async fn dispatch_request(
    transaction_mgr: &Arc<sip_transaction::TransactionManager>,
    handler: &Arc<uas::AnvilUasHandler>,
    request: sip_core::Request,
    ctx: sip_transaction::TransportContext,
) {
    use sip_uas::integrated::UasRequestHandler;
    use sip_uas::UserAgentServer;

    // ACK doesn't create / update a server transaction in the same way and
    // must be fed to the transaction manager directly so the INVITE's 2xx
    // retransmission stops.
    let handle = transaction_mgr
        .receive_request(request.clone(), ctx.clone())
        .await;

    let method = request.method().as_str().to_string();
    match method.as_str() {
        "INVITE" => {
            // 100 Trying so the caller stops retransmitting.
            let trying = UserAgentServer::create_response(&request, 100, "Trying");
            handle.send_provisional(trying).await;
            // If we already have a confirmed dialog for this Call-ID, this
            // is a mid-dialog re-INVITE (hold / resume). Pass it through so
            // on_invite can branch into the hold handler.
            let call_id_str = request
                .headers()
                .get("Call-ID")
                .map(|s| s.to_string())
                .unwrap_or_default();
            let existing_dialog = uas::find_call_by_dialog_id(&handler.calls, &call_id_str)
                .and_then(|id| handler.calls.get(&id).and_then(|e| e.dialog.clone()));
            if let Err(e) = handler
                .on_invite(&request, handle, &ctx, existing_dialog.as_ref())
                .await
            {
                tracing::warn!(%e, "on_invite failed");
            }
        }
        "ACK" => {
            // No response. Transaction manager has already promoted state.
            let _ = handle;
        }
        "BYE" => {
            let call_id = request
                .headers()
                .get("Call-ID")
                .map(|s| s.to_string())
                .unwrap_or_default();
            if let Some(call) = uas::find_call_by_dialog_id(&handler.calls, &call_id) {
                // The call's dialog: an incoming call's, or an outgoing
                // call's from its handle (the callee hanging up on us).
                let (dialog_opt, outbound) = {
                    let entry = handler.calls.get(&call);
                    (
                        entry.as_ref().and_then(|e| e.dialog.clone()),
                        entry.as_ref().and_then(|e| e.outbound_handle.clone()),
                    )
                };
                let dialog_opt = match (dialog_opt, outbound) {
                    (Some(dialog), _) => Some(dialog),
                    (None, Some(handle)) => Some(handle.dialog.read().await.clone()),
                    (None, None) => None,
                };
                if let Some(dialog) = dialog_opt {
                    if let Err(e) = handler.on_bye(&request, handle, &ctx, &dialog).await {
                        tracing::warn!(%e, "on_bye failed");
                    }
                } else {
                    // Haven't stored a confirmed dialog — shouldn't happen
                    // post-answer; respond 481 to be safe.
                    let resp = UserAgentServer::create_response(
                        &request,
                        481,
                        "Call/Transaction Does Not Exist",
                    );
                    handle.send_final(resp).await;
                }
            } else {
                let resp = UserAgentServer::create_response(
                    &request,
                    481,
                    "Call/Transaction Does Not Exist",
                );
                handle.send_final(resp).await;
            }
        }
        "CANCEL" => {
            if let Err(e) = handler.on_cancel(&request, handle, &ctx).await {
                tracing::warn!(%e, "on_cancel failed");
            }
        }
        "OPTIONS" => {
            let resp = UserAgentServer::create_response(&request, 200, "OK");
            handle.send_final(resp).await;
        }
        _ => {
            let resp = UserAgentServer::create_response(&request, 405, "Method Not Allowed");
            handle.send_final(resp).await;
        }
    }
}

fn map_transport(kind: sip_transport::TransportKind) -> sip_transaction::TransportKind {
    match kind {
        sip_transport::TransportKind::Udp => sip_transaction::TransportKind::Udp,
        sip_transport::TransportKind::Tcp => sip_transaction::TransportKind::Tcp,
        sip_transport::TransportKind::Tls => sip_transaction::TransportKind::Tls,
        sip_transport::TransportKind::Sctp => sip_transaction::TransportKind::Sctp,
        sip_transport::TransportKind::TlsSctp => sip_transaction::TransportKind::TlsSctp,
        sip_transport::TransportKind::Ws => sip_transaction::TransportKind::Ws,
        sip_transport::TransportKind::Wss => sip_transaction::TransportKind::Wss,
    }
}

/// Drives an outgoing call's lifecycle: drains provisional responses and
/// awaits the final response. Emits `CallRinging`, `CallEstablished`, and
/// `CallEnded` events; leaves the `CallEntry` in place for the app to `hangup`.
async fn drive_outgoing_call(
    call_id: CallId,
    handle: Arc<sip_uac::integrated::CallHandle>,
    rtp_socket: Arc<tokio::net::UdpSocket>,
    audio: Arc<dyn AudioHost>,
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
                    let _ = prov_events.send(Event::CallRinging { call: call_id }).await;
                }
                other => {
                    tracing::debug!(code = other, reason = %resp.reason(), "provisional");
                }
            }
        }
    });

    // The final response. siphon-rs answers a 401/407 itself, retrying the
    // INVITE with the account's credentials, so a challenge reaches here
    // only when the credentials were refused.
    let final_resp = handle.await_final().await;

    match final_resp {
        // Answered after the user hung up (a 200 OK crossing the CANCEL):
        // `cancel` ends it with a BYE, and nothing is started or reported.
        Ok(resp) if (200..300).contains(&resp.code()) && !calls.contains_key(&call_id) => {
            tracing::debug!(code = resp.code(), "answer for a call already hung up");
        }
        Ok(resp) if (200..300).contains(&resp.code()) => {
            // Extract the negotiated codec from the answer. The payload-type
            // table is small (0 = PCMU, 8 = PCMA) and both are mandatory for
            // RFC 3551-compliant endpoints, so for Phase 1 M2 we just check
            // the first audio format byte. Proper negotiation (matching rtpmap
            // names, dropping unknown PTs) arrives with the codec milestone.
            let answer_body = resp.body();
            let body_slice: &[u8] = answer_body.as_ref();
            let codec = extract_codec_from_answer(body_slice).unwrap_or(Codec::Pcmu);
            let remote_rtp = media::extract_remote_rtp_addr(body_slice);

            // Start media: open audio host streams for this call, spawn the
            // send/receive tasks. Failures are logged but don't tear down the
            // call — a half-deaf call is still better UX than a cryptic panic
            // before `CallEstablished` fires. Real AEC / renegotiation lands
            // in Phase 2.
            let pipeline = match remote_rtp {
                Some(remote) => {
                    start_media(call_id, codec, &audio, rtp_socket, remote, events.clone()).await
                }
                None => {
                    tracing::warn!("no remote RTP address in SDP answer; no media");
                    None
                }
            };

            if let Some(mut entry) = calls.get_mut(&call_id) {
                entry.negotiated_codec = Some(codec);
                entry.pipeline = pipeline;
            }

            let _ = events
                .send(Event::CallEstablished {
                    call: call_id,
                    codec,
                })
                .await;
        }
        Ok(resp) => {
            // Gone already when the user hung up first (a CANCEL's 487).
            if calls.remove(&call_id).is_some() {
                let _ = events
                    .send(Event::CallEnded {
                        call: call_id,
                        reason: EndReason::Rejected(resp.code()),
                    })
                    .await;
            }
        }
        Err(e) => {
            // Gone already when the user hung up first (a CANCEL's 487).
            if calls.remove(&call_id).is_some() {
                let _ = events
                    .send(Event::CallEnded {
                        call: call_id,
                        reason: EndReason::Error(e.to_string()),
                    })
                    .await;
            }
        }
    }
}

/// Opens audio host streams and starts the RTP pipeline for the given codec.
/// Returns `None` if capture or playback cannot be opened; callers surface
/// that as "no media" rather than failing the call.
async fn start_media(
    call: CallId,
    codec: Codec,
    audio: &Arc<dyn AudioHost>,
    rtp_socket: Arc<tokio::net::UdpSocket>,
    remote: std::net::SocketAddr,
    events: mpsc::Sender<Event>,
) -> Option<media::MediaPipeline> {
    let fmt = media::CodecSpec::for_codec(codec).audio_format();
    let capture = match audio.make_capture(fmt) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(%e, "audio capture open failed");
            return None;
        }
    };
    let playback = match audio.make_playback(fmt) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(%e, "audio playback open failed");
            return None;
        }
    };

    let sink = media::StatsSink {
        call,
        codec,
        interval: std::time::Duration::from_secs(2),
        events,
    };
    match media::start_pipeline(codec, rtp_socket, remote, capture, playback, Some(sink)) {
        Ok(p) => {
            tracing::info!(%remote, ?codec, "media pipeline started");
            Some(p)
        }
        Err(e) => {
            tracing::warn!(%e, ?codec, "media pipeline failed to start");
            None
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
                "opus" => Some(Codec::Opus),
                "pcmu" => Some(Codec::Pcmu),
                "pcma" => Some(Codec::Pcma),
                "g722" => Some(Codec::G722),
                _ => None,
            }
        }
    }
}
