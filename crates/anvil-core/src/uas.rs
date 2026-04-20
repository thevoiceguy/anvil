//! Incoming request handling.
//!
//! Wires `sip_uas::integrated::IntegratedUAS` into `Anvil`. On INVITE, we
//! allocate an RTP socket, build an SDP answer, stash a `PendingInbound`
//! in the calls map, send `180 Ringing`, and emit `Event::IncomingCall`.
//! The app later drives `Anvil::answer` / `Anvil::reject`, which we route
//! back through the stashed `ServerTransactionHandle` to finish the
//! transaction.
//!
//! Remote BYE is handled inline: tear down the pipeline, emit `CallEnded`,
//! reply `200 OK`. CANCEL is handled similarly for the pre-answer case.

use std::net::IpAddr;
use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use sip_core::Request;
use sip_dialog::Dialog;
use sip_transaction::{ServerTransactionHandle, TransportContext};
use sip_uas::integrated::UasRequestHandler;
use sip_uas::UserAgentServer;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;

use crate::audio::AudioHost;
use crate::call::{bind_rtp_socket, next_call_id, CallEntry, PendingInbound};
use crate::config::Codec;
use crate::event::{EndReason, Event};
use crate::media;
use crate::CallId;

pub(crate) struct AnvilUasHandler {
    pub events: mpsc::Sender<Event>,
    pub calls: Arc<DashMap<CallId, CallEntry>>,
    pub media_ip: IpAddr,
    pub local_user: String,
    /// Our own SIP bind address, used verbatim as the Contact URI host/port
    /// so remote peers send in-dialog requests (BYE, re-INVITE) to the port
    /// we're actually listening on. Omitting the port lets it default to
    /// 5060, which breaks every non-standard bind.
    pub local_sip_addr: SocketAddr,
}

#[async_trait]
impl UasRequestHandler for AnvilUasHandler {
    async fn on_invite(
        &self,
        request: &Request,
        handle: ServerTransactionHandle,
        _ctx: &TransportContext,
        dialog: Option<&Dialog>,
    ) -> anyhow::Result<()> {
        // Re-INVITE (mid-dialog). Not handled in M4; reject 488 so callers
        // know we refuse the re-negotiation rather than silently ignoring.
        if dialog.is_some() {
            let resp = UserAgentServer::create_response(
                request,
                488,
                "Not Acceptable Here (re-INVITE unsupported in M4)",
            );
            handle.send_final(resp).await;
            return Ok(());
        }

        // Allocate an RTP socket for this call so the SDP answer's port
        // number is actually bound.
        let rtp_socket = match bind_rtp_socket(self.media_ip).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(%e, "could not bind RTP socket for inbound call");
                let resp = UserAgentServer::create_response(request, 500, "Server Error");
                handle.send_final(resp).await;
                return Ok(());
            }
        };
        let rtp_port = match rtp_socket.local_addr() {
            Ok(a) => a.port(),
            Err(e) => {
                tracing::warn!(%e, "rtp local_addr");
                let resp = UserAgentServer::create_response(request, 500, "Server Error");
                handle.send_final(resp).await;
                return Ok(());
            }
        };

        // Negotiate codec from the offer against our preference list. If
        // the peer offered nothing we support, reject with 488 rather than
        // silently picking PCMU and hoping.
        let offered_body = request.body();
        let chosen = media::negotiate_answer_codec(offered_body, media::supported_codecs());
        let chosen = match chosen {
            Some(c) => c,
            None => {
                let resp = UserAgentServer::create_response(
                    request,
                    488,
                    "Not Acceptable Here (no common codec)",
                );
                handle.send_final(resp).await;
                return Ok(());
            }
        };

        // Build an answer SDP carrying just the chosen codec. A more liberal
        // implementation would echo back the full intersection; single-codec
        // answers are simpler and sufficient for the offer/answer model.
        let sdp_body = media::build_sdp(
            &[chosen],
            &self.local_user,
            &self.media_ip.to_string(),
            rtp_port,
        );

        // Build the 200 OK now (including dialog) so `answer()` can send it
        // without re-deriving anything. Note: `accept_invite` inserts the
        // dialog into the shared DialogManager as a side effect, which is
        // what lets `on_bye` later find it.
        //
        // We need a UserAgentServer instance to do this. IntegratedUAS owns
        // one internally but doesn't expose it, so we construct our own
        // against the same DialogManager via the From URI. Good enough for
        // M4 — Phase 2 will refactor to share one DialogManager end to end.
        let from_uri = request
            .headers()
            .get("From")
            .and_then(|f| extract_uri_from_nameaddr(f))
            .ok_or_else(|| anyhow::anyhow!("missing From URI"))?;
        let to_uri = request
            .headers()
            .get("To")
            .and_then(|t| extract_uri_from_nameaddr(t))
            .ok_or_else(|| anyhow::anyhow!("missing To URI"))?;

        let local = sip_core::SipUri::parse(&to_uri)
            .map_err(|e| anyhow::anyhow!("local URI parse: {e:?}"))?;
        // Contact must carry the SIP bind port, not the RTP port, so the
        // peer routes in-dialog requests (BYE / re-INVITE) back to us.
        let contact = sip_core::SipUri::parse(&format!(
            "sip:{}@{}:{}",
            self.local_user,
            self.local_sip_addr.ip(),
            self.local_sip_addr.port(),
        ))
        .map_err(|e| anyhow::anyhow!("contact URI parse: {e:?}"))?;
        let _ = rtp_port; // only used above for SDP answer's m= line
        let uas = UserAgentServer::new(local, contact);

        let (response, dialog) = match uas.accept_invite(request, Some(&sdp_body)) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(%e, "accept_invite failed");
                let resp = UserAgentServer::create_response(request, 500, "Server Error");
                handle.send_final(resp).await;
                return Ok(());
            }
        };

        // Send 180 Ringing so the caller knows we're alerting. We don't
        // loop the ring tone locally in M4; a UI would own that.
        let ringing = UserAgentServer::create_response(request, 180, "Ringing");
        handle.send_provisional(ringing).await;

        let from_display = display_name_from_header(request.headers().get("From"));
        let call_id = next_call_id();

        let pending = PendingInbound {
            request: request.clone(),
            answer_response: response,
            server_handle: handle.clone(),
            dialog,
            sdp_answer_sent: false,
            rtp_socket: Arc::clone(&rtp_socket),
        };

        self.calls.insert(
            call_id,
            CallEntry {
                outbound_handle: None,
                inbound: Some(pending),
                dialog: None,
                negotiated_codec: None,
                rtp_socket,
                pipeline: None,
            },
        );

        let _ = self
            .events
            .send(Event::IncomingCall {
                call: call_id,
                from: from_uri,
                display_name: from_display,
            })
            .await;

        // We don't call send_final here — `answer()` / `reject()` will.
        Ok(())
    }

    async fn on_ack(&self, _request: &Request, _dialog: &Dialog) -> anyhow::Result<()> {
        // 2xx ACK hits us here (non-2xx ACK is absorbed by the transaction
        // manager before dispatch). siphon-rs has already promoted the
        // dialog to Confirmed — we use ACK as the trigger to start media
        // if it hasn't started yet.
        //
        // Currently `answer()` starts the pipeline before sending 200 OK,
        // so by the time ACK arrives the media is already flowing. If we
        // later switch to starting media on ACK (more correct for
        // half-open NAT scenarios), this is where it'd land.
        Ok(())
    }

    async fn on_bye(
        &self,
        request: &Request,
        handle: ServerTransactionHandle,
        dialog: &Dialog,
    ) -> anyhow::Result<()> {
        let call_id = find_call_by_dialog_id(&self.calls, dialog.id().call_id());

        // Respond 200 OK to the BYE regardless of whether we can map it to
        // a local call — the peer deserves an answer and the transaction
        // manager would otherwise retransmit at them.
        let resp = UserAgentServer::create_response(request, 200, "OK");
        handle.send_final(resp).await;

        if let Some(call) = call_id {
            self.calls.remove(&call);
            let _ = self
                .events
                .send(Event::CallEnded {
                    call,
                    reason: EndReason::RemoteHangup,
                })
                .await;
        } else {
            tracing::debug!(call_id = %dialog.id().call_id(), "BYE for unknown call");
        }

        Ok(())
    }

    async fn on_cancel(
        &self,
        request: &Request,
        handle: ServerTransactionHandle,
    ) -> anyhow::Result<()> {
        // Ack the CANCEL immediately.
        let resp = UserAgentServer::create_response(request, 200, "OK");
        handle.send_final(resp).await;

        // Find the matching pending inbound by Call-ID and emit CallEnded.
        // The transaction manager already terminates the INVITE transaction
        // on CANCEL, so we don't send a 487 ourselves — IntegratedUAS does
        // that via `send_final` to the INVITE key after on_cancel returns.
        let call_id_str = request
            .headers()
            .get("Call-ID")
            .map(|s| s.to_string())
            .unwrap_or_default();

        if let Some(call) = find_call_by_dialog_id(&self.calls, &call_id_str) {
            self.calls.remove(&call);
            let _ = self
                .events
                .send(Event::CallEnded {
                    call,
                    reason: EndReason::RemoteHangup,
                })
                .await;
        }
        Ok(())
    }
}

/// Helpers: find a call whose inbound request or confirmed dialog carries the
/// given Call-ID string. O(n) but n is one in practice for a softphone.
pub(crate) fn find_call_by_dialog_id(
    calls: &DashMap<CallId, CallEntry>,
    call_id: &str,
) -> Option<CallId> {
    for entry in calls.iter() {
        if let Some(dlg) = entry.dialog.as_ref() {
            if dlg.id().call_id() == call_id {
                return Some(*entry.key());
            }
        }
        if let Some(inbound) = entry.inbound.as_ref() {
            if inbound.dialog.id().call_id() == call_id {
                return Some(*entry.key());
            }
        }
        if let Some(h) = entry.outbound_handle.as_ref() {
            // We only need this for matching inbound BYE to outbound dialog.
            if let Ok(d) = h.dialog.try_read() {
                if d.id().call_id() == call_id {
                    return Some(*entry.key());
                }
            }
        }
    }
    None
}

/// Pull a raw URI string out of a `name-addr` header value. Returns
/// `sip:alice@example.com` from ``Alice <sip:alice@example.com>;tag=123``.
/// Falls back to the input if it's a bare URI.
fn extract_uri_from_nameaddr(value: &str) -> Option<String> {
    if let Some(start) = value.find('<') {
        let rest = &value[start + 1..];
        if let Some(end) = rest.find('>') {
            return Some(rest[..end].to_string());
        }
    }
    // Bare URI form: ``sip:alice@example.com;tag=123``
    let bare = value.split(';').next()?.trim();
    if bare.starts_with("sip:") || bare.starts_with("sips:") {
        return Some(bare.to_string());
    }
    None
}

fn display_name_from_header(from: Option<&str>) -> Option<String> {
    let v = from?;
    let q1 = v.find('"')?;
    let q2 = v[q1 + 1..].find('"')?;
    Some(v[q1 + 1..q1 + 1 + q2].to_string())
}

/// Accept an inbound call: send the pre-built 200 OK and start media.
/// Returns the negotiated codec. Called by `Anvil::answer`.
pub(crate) async fn send_answer_and_start_media(
    call: CallId,
    calls: &DashMap<CallId, CallEntry>,
    audio: &Arc<dyn AudioHost>,
    events: &mpsc::Sender<Event>,
) -> Result<Codec, crate::error::AnvilError> {
    let (server_handle, answer_response, dialog, rtp_socket, offer_body) = {
        let mut entry = calls
            .get_mut(&call)
            .ok_or(crate::error::AnvilError::NoSuchCall(call))?;
        let pending = entry.inbound.as_mut().ok_or_else(|| {
            crate::error::AnvilError::Internal(format!("{call:?} is not an inbound call"))
        })?;
        if pending.sdp_answer_sent {
            return Err(crate::error::AnvilError::Internal(
                "inbound call already answered".into(),
            ));
        }
        pending.sdp_answer_sent = true;
        (
            pending.server_handle.clone(),
            pending.answer_response.clone(),
            pending.dialog.clone(),
            Arc::clone(&pending.rtp_socket),
            pending.request.body().to_vec(),
        )
    };

    let remote = media::extract_remote_rtp_addr(&offer_body);
    // Codec was already chosen when on_invite built the answer SDP; cheaper
    // to re-derive from the offer here than to plumb it through.
    let codec = media::negotiate_answer_codec(&offer_body, media::supported_codecs())
        .unwrap_or(Codec::Pcmu);

    // Start the pipeline before sending 200 OK so that when the caller's
    // first RTP packet lands we already have a socket reading it. Losing the
    // first few packets isn't audible but is avoidable.
    let pipeline = if let Some(remote) = remote {
        start_media_inbound(call, codec, audio, rtp_socket, remote, events.clone()).await
    } else {
        tracing::warn!("no remote RTP address in offer; answering without media");
        None
    };

    server_handle.send_final(answer_response).await;

    // Promote the entry: move the confirmed dialog out of `inbound`, attach
    // the pipeline, record the codec.
    if let Some(mut entry) = calls.get_mut(&call) {
        entry.dialog = Some(dialog);
        entry.pipeline = pipeline;
        entry.negotiated_codec = Some(codec);
        entry.inbound = None; // no longer pending
    }

    Ok(codec)
}

/// Reject an inbound call with the given SIP response code.
pub(crate) async fn reject_inbound(
    call: CallId,
    code: u16,
    calls: &DashMap<CallId, CallEntry>,
) -> Result<(), crate::error::AnvilError> {
    let (server_handle, request) = {
        let mut entry = calls
            .get_mut(&call)
            .ok_or(crate::error::AnvilError::NoSuchCall(call))?;
        let pending = entry.inbound.as_mut().ok_or_else(|| {
            crate::error::AnvilError::Internal(format!("{call:?} is not an inbound call"))
        })?;
        (pending.server_handle.clone(), pending.request.clone())
    };

    let reason = match code {
        486 => "Busy Here",
        487 => "Request Terminated",
        603 => "Decline",
        _ => "Declined",
    };
    let resp = UserAgentServer::create_response(&request, code, reason);
    server_handle.send_final(resp).await;

    calls.remove(&call);
    Ok(())
}

async fn start_media_inbound(
    call: CallId,
    codec: Codec,
    audio: &Arc<dyn AudioHost>,
    rtp_socket: Arc<UdpSocket>,
    remote: std::net::SocketAddr,
    events: mpsc::Sender<Event>,
) -> Option<media::MediaPipeline> {
    let fmt = media::CodecSpec::for_codec(codec).audio_format();
    let capture = audio.make_capture(fmt).ok()?;
    let playback = audio.make_playback(fmt).ok()?;
    let sink = media::StatsSink {
        call,
        codec,
        interval: std::time::Duration::from_secs(2),
        events,
    };
    media::start_pipeline(codec, rtp_socket, remote, capture, playback, Some(sink)).ok()
}
