//! Video on a call (FCP's `docs/SOFTPHONE_VIDEO.md` §2.3–2.4).
//!
//! Three layers, so that a second transport (WebRTC, for rooms that
//! forward each participant's stream) adds itself and rewrites nothing:
//!
//! - [`host`]: what the platform supplies — cameras, codecs, sinks.
//! - [`session`]: a source going out and any number of streams coming in,
//!   over a [`transport::MediaTransport`], described by a [`StreamPlan`].
//!   It never reads SDP or opens a socket.
//! - SIP's transport: [`sdp`] for the sections, [`transport::SdesTransport`]
//!   for a socket pair and SDES keys per section, and [`CallVideo`] here,
//!   which turns a call's offer and answer into plans.

pub(crate) mod bwe;
pub mod host;
pub(crate) mod playout;
pub(crate) mod sdp;
pub(crate) mod session;
pub mod synthetic;
pub(crate) mod transport;

use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::AtomicU32;
use std::sync::Arc;

use tokio::sync::mpsc;

use self::host::{EncodingId, Resolution, SendConfig, StreamId, VideoCodec, VideoHost, VideoKind};
use self::playout::AudioClock;
use self::sdp::{LocalSection, RemoteSection};
use self::session::{SessionLinks, VideoSession};
use self::transport::SdesTransport;
use crate::call::MediaSockets;
use crate::config::SrtpMode;
use crate::event::Event;
use crate::media::MediaDirection;
use crate::srtp::{self, Crypto, Suite};
use crate::CallId;

/// What a call carries besides audio.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CallMedia {
    /// Offer (or accept) video, where the softphone has a video host.
    pub video: bool,
}

impl CallMedia {
    pub const AUDIO: CallMedia = CallMedia { video: false };
    pub const VIDEO: CallMedia = CallMedia { video: true };
}

/// Which way a stream goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoStreamDirection {
    Incoming,
    Outgoing,
}

/// One video stream's last second ([`Event::VideoStats`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoStreamStats {
    pub direction: VideoStreamDirection,
    pub kind: VideoKind,
    pub codec: VideoCodec,
    /// On the wire, headers included.
    pub kbps: f32,
    pub fps: f32,
    /// Incoming: packets the network lost, before any were sent again.
    /// Outgoing: the share of ours the far end last reported lost. `None`
    /// until there is something to say.
    pub loss_pct: Option<f32>,
    /// Incoming: packets we asked for again; outgoing: those asked of us.
    pub nacks: u32,
    /// Incoming: keyframes we asked for; outgoing: those asked of us.
    pub keyframe_requests: u32,
    /// Outgoing: what the encoder aims for (the far end's REMB, within
    /// what the call was set up to send).
    pub target_kbps: Option<u32>,
    /// Incoming: what we tell the far end it may send (REMB), for all of
    /// the section's incoming streams together.
    pub estimate_kbps: Option<u32>,
    /// Incoming: RFC 3550's interarrival jitter, in ms.
    pub jitter_ms: Option<f32>,
    /// Incoming: how long a frame waits before it is shown.
    pub playout_delay_ms: Option<f32>,
    /// Incoming: how far the picture is behind its audio, in ms, once
    /// sender reports tie the two (+: late; within a frame or two when
    /// the call is in sync).
    pub av_offset_ms: Option<f32>,
    /// Outgoing: the round trip, from the far end's reports.
    pub rtt_ms: Option<f32>,
}

/// What the far end takes of RTCP feedback (RFC 4585, RFC 5104, REMB).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Feedback {
    pub nack: bool,
    pub pli: bool,
    pub fir: bool,
    pub remb: bool,
}

/// A session as its transport agreed it: no SDP, no sockets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StreamPlan {
    pub kind: VideoKind,
    pub codec: VideoCodec,
    /// The payload type both sides use for it.
    pub pt: u8,
    pub direction: MediaDirection,
    pub feedback: Feedback,
    /// What to capture and encode; `None`, receive only.
    pub send: Option<SendConfig>,
}

/// Tells the app of a call's streams.
#[derive(Clone)]
pub(crate) struct VideoEvents {
    pub call: CallId,
    pub tx: mpsc::Sender<Event>,
}

impl VideoEvents {
    pub(crate) fn started(
        &self,
        stream: StreamId,
        direction: VideoStreamDirection,
        kind: VideoKind,
        codec: VideoCodec,
    ) {
        let _ = self.tx.try_send(Event::VideoStarted {
            call: self.call,
            stream,
            direction,
            kind,
            codec,
        });
    }

    pub(crate) fn stats(&self, stream: StreamId, stats: VideoStreamStats) {
        let _ = self.tx.try_send(Event::VideoStats {
            call: self.call,
            stream,
            stats,
        });
    }

    pub(crate) fn stopped(&self, stream: StreamId) {
        let _ = self.tx.try_send(Event::VideoStopped {
            call: self.call,
            stream,
        });
    }
}

/// What a camera sends until the network (REMB) or the server says less.
const DEFAULT_KBPS: u32 = 1500;
const DEFAULT_FPS: u32 = 30;

/// One video section of a call over SIP.
struct CallSection {
    /// What we last said of it.
    local: LocalSection,
    /// What the far end last said of it.
    remote: Option<RemoteSection>,
    sockets: Option<MediaSockets>,
    /// Their key, once known.
    theirs: Option<Crypto>,
    transport: Option<Arc<SdesTransport>>,
    session: Option<VideoSession>,
}

impl CallSection {
    fn stop(&mut self) {
        self.session = None;
        self.transport = None;
        self.sockets = None;
        self.local.port = 0;
    }
}

/// A call's video over SIP: its sections, in SDP order after the audio.
pub(crate) struct CallVideo {
    sections: Vec<CallSection>,
    host: Arc<dyn VideoHost>,
    /// Numbers the call's streams.
    streams: Arc<AtomicU32>,
    capture: crate::capture::CaptureSlot,
    /// The call's audio, once its pipeline runs: lip sync and the CNAME.
    audio: Option<Arc<dyn AudioClock>>,
}

impl CallVideo {
    /// The sections of an offer of ours: a camera, sending and receiving.
    pub(crate) async fn offer(
        host: Arc<dyn VideoHost>,
        media_ip: IpAddr,
        srtp: SrtpMode,
        capture: crate::capture::CaptureSlot,
    ) -> Option<Self> {
        let codecs = host::usable_codecs(host.as_ref());
        if codecs.is_empty() {
            return None;
        }
        let sockets = match crate::call::bind_rtp_socket(media_ip).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(%e, "no socket for video; offering audio only");
                return None;
            }
        };
        let port = sockets.rtp.local_addr().ok()?.port();
        let crypto =
            (srtp != SrtpMode::Off).then(|| Crypto::generate(1, Suite::AesCm128HmacSha1_80));
        let local = LocalSection::offer(
            VideoKind::Camera,
            port,
            &codecs,
            MediaDirection::Sendrecv,
            crypto,
        );
        Some(Self {
            sections: vec![CallSection {
                local,
                remote: None,
                sockets: Some(sockets),
                theirs: None,
                transport: None,
                session: None,
            }],
            host,
            streams: Arc::default(),
            capture,
            audio: None,
        })
    }

    /// Our answer's sections to `offer`, each taken where we can and
    /// declined where we cannot. `None` when it offers no video. Nothing
    /// starts until [`start`](Self::start).
    pub(crate) async fn answer(
        offer: &[u8],
        host: Arc<dyn VideoHost>,
        media_ip: IpAddr,
        srtp: SrtpMode,
        capture: crate::capture::CaptureSlot,
    ) -> Option<Self> {
        let offered = sdp::sections(offer);
        if offered.is_empty() {
            return None;
        }
        let codecs = host::usable_codecs(host.as_ref());
        let mut sections = Vec::with_capacity(offered.len());
        for remote in offered {
            let taken = match Self::take(&remote, &codecs, srtp) {
                Some((codec, crypto)) => match crate::call::bind_rtp_socket(media_ip).await {
                    Ok(sockets) => sockets
                        .rtp
                        .local_addr()
                        .ok()
                        .map(|a| (codec, crypto, sockets, a.port())),
                    Err(e) => {
                        tracing::warn!(%e, "no socket for a video section; declining it");
                        None
                    }
                },
                None => None,
            };
            let section = match taken {
                Some((codec, (ours, theirs), sockets, port)) => CallSection {
                    local: remote.answer(&codec, port, remote.direction.mirror(), ours),
                    theirs,
                    remote: Some(remote),
                    sockets: Some(sockets),
                    transport: None,
                    session: None,
                },
                None => CallSection {
                    local: LocalSection::declined(&remote),
                    remote: Some(remote),
                    sockets: None,
                    theirs: None,
                    transport: None,
                    session: None,
                },
            };
            sections.push(section);
        }
        Some(Self {
            sections,
            host,
            streams: Arc::default(),
            capture,
            audio: None,
        })
    }

    /// The codec and keys (ours, theirs) to take `remote` with, or `None`
    /// to decline it.
    #[allow(clippy::type_complexity)]
    fn take(
        remote: &RemoteSection,
        codecs: &[VideoCodec],
        mode: SrtpMode,
    ) -> Option<(sdp::SectionCodec, (Option<Crypto>, Option<Crypto>))> {
        if !remote.live() {
            return None;
        }
        let codec = remote.choose(codecs)?.clone();
        let keys = match (mode, remote.profile.secure()) {
            (SrtpMode::Off, true) | (SrtpMode::Required, false) => return None,
            (_, true) => {
                let (ours, theirs) = srtp::answer_to(&remote.crypto)?;
                (Some(ours), Some(theirs))
            }
            (_, false) => (None, None),
        };
        Some((codec, keys))
    }

    /// The call's audio is running: video keeps time with it.
    pub(crate) fn set_audio(&mut self, audio: Option<Arc<dyn AudioClock>>) {
        self.audio = audio;
    }

    /// Whether any section is (to be) carried.
    pub(crate) fn any_live(&self) -> bool {
        self.sections.iter().any(|s| s.local.port != 0)
    }

    /// Decline every section (an audio-only answer).
    pub(crate) fn decline_all(&mut self) {
        for s in &mut self.sections {
            s.stop();
            s.local = match &s.remote {
                Some(r) => LocalSection::declined(r),
                None => LocalSection {
                    port: 0,
                    ..s.local.clone()
                },
            };
        }
    }

    /// What goes in our SDP, in order.
    pub(crate) fn sdp_sections(&self) -> Vec<LocalSection> {
        self.sections.iter().map(|s| s.local.clone()).collect()
    }

    /// Our sections as an offer with `direction` on each live one (hold).
    pub(crate) fn offer_with(&self, direction: MediaDirection) -> Vec<LocalSection> {
        self.sections
            .iter()
            .map(|s| {
                let mut l = s.local.clone();
                if l.port != 0 {
                    l.direction = direction;
                }
                l
            })
            .collect()
    }

    /// The far end answered our offer: start each section it took.
    pub(crate) fn answered(&mut self, answer: &[u8], events: Option<VideoEvents>) {
        let answered = sdp::sections(answer);
        for (i, s) in self.sections.iter_mut().enumerate() {
            let Some(remote) = answered.get(i).cloned() else {
                s.stop();
                continue;
            };
            let ours: Vec<VideoCodec> = s.local.codecs.iter().map(|c| c.codec).collect();
            let chosen = remote
                .live()
                .then(|| remote.choose(&ours).cloned())
                .flatten();
            let theirs = match &s.local.crypto {
                None if remote.profile.secure() => None,
                None => Some(None),
                Some(key) => srtp::matching(key, &remote.crypto).cloned().map(Some),
            };
            match (chosen, theirs) {
                (Some(codec), Some(theirs)) => {
                    s.theirs = theirs;
                    // Its payload type and what it takes; our codec list
                    // becomes the one agreed.
                    s.local.codecs.retain(|c| c.codec == codec.codec);
                    if let Some(c) = s.local.codecs.first_mut() {
                        c.pt = codec.pt;
                    }
                    let direction = s.local.direction.and(remote.direction.mirror());
                    s.remote = Some(remote);
                    Self::start_section(
                        s,
                        &codec,
                        direction,
                        &self.host,
                        SessionLinks {
                            streams: Arc::clone(&self.streams),
                            events: events.clone(),
                            audio: self.audio.clone(),
                        },
                        &self.capture,
                    );
                }
                _ => {
                    tracing::info!(section = i, "video section declined in the answer");
                    s.remote = Some(remote);
                    s.stop();
                }
            }
        }
    }

    /// Start the sections of our answer (the app answered the call).
    pub(crate) fn start(&mut self, events: Option<VideoEvents>) {
        for s in &mut self.sections {
            if s.local.port == 0 || s.session.is_some() {
                continue;
            }
            let Some(codec) = s.local.codecs.first().cloned() else {
                continue;
            };
            let direction = s.local.direction;
            Self::start_section(
                s,
                &codec,
                direction,
                &self.host,
                SessionLinks {
                    streams: Arc::clone(&self.streams),
                    events: events.clone(),
                    audio: self.audio.clone(),
                },
                &self.capture,
            );
        }
    }

    fn start_section(
        s: &mut CallSection,
        codec: &sdp::SectionCodec,
        direction: MediaDirection,
        host: &Arc<dyn VideoHost>,
        links: SessionLinks,
        capture: &crate::capture::CaptureSlot,
    ) {
        let (Some(sockets), Some(remote)) = (s.sockets.clone(), s.remote.as_ref()) else {
            s.stop();
            return;
        };
        let Some(addr) = remote.addr.map(|ip| SocketAddr::new(ip, remote.port)) else {
            tracing::info!("a video section without an address; declining it");
            s.stop();
            return;
        };
        sockets.set_capture(capture.lock().clone());
        let ctx = match (&s.local.crypto, &s.theirs) {
            (Some(ours), Some(theirs)) => match srtp::context(ours, theirs) {
                Some(c) => Some(c),
                None => {
                    s.stop();
                    return;
                }
            },
            _ => None,
        };
        let encodes = host
            .codecs()
            .iter()
            .any(|c| c.codec == codec.codec && c.encode);
        let plan = StreamPlan {
            kind: s.local.kind,
            codec: codec.codec,
            pt: codec.pt,
            direction,
            feedback: codec.feedback,
            send: encodes.then(|| SendConfig {
                codec: codec.codec,
                kind: s.local.kind,
                camera: None,
                max: Resolution::P720,
                kbps: DEFAULT_KBPS,
                fps: DEFAULT_FPS,
                encodings: vec![EncodingId(0)],
            }),
        };
        let (tx, rx) = mpsc::channel(transport::INBOUND_QUEUE);
        let t = SdesTransport::start(sockets, addr, ctx, tx);
        tracing::info!(
            %addr,
            codec = ?plan.codec,
            ?direction,
            encrypted = s.local.crypto.is_some(),
            "video session started"
        );
        s.session = Some(VideoSession::start(
            plan,
            Arc::clone(host),
            t.clone(),
            rx,
            links,
        ));
        s.transport = Some(t);
    }

    /// Our answer's sections to a re-INVITE's offer: each section we carry
    /// follows its new direction, address and key; one it drops stops; one
    /// it adds is declined (taking video up mid-call is `set_video`'s).
    pub(crate) fn reoffered(&mut self, offer: &[u8]) -> Vec<LocalSection> {
        let offered = sdp::sections(offer);
        let mut out = Vec::with_capacity(offered.len());
        for (i, remote) in offered.into_iter().enumerate() {
            let Some(s) = self.sections.get_mut(i) else {
                out.push(LocalSection::declined(&remote));
                continue;
            };
            if s.session.is_none() || !remote.live() {
                s.stop();
                let declined = LocalSection::declined(&remote);
                s.local = declined.clone();
                s.remote = Some(remote);
                out.push(declined);
                continue;
            }
            if let (Some(t), Some(ip)) = (&s.transport, remote.addr) {
                t.set_remote(SocketAddr::new(ip, remote.port));
            }
            if let Some(ours) = &s.local.crypto {
                let new = srtp::answer_to(&remote.crypto).map(|(_, theirs)| theirs);
                if let Some(theirs) = new.filter(|t| Some(t) != s.theirs.as_ref()) {
                    if let (Some(t), Some(ctx)) = (&s.transport, srtp::context(ours, &theirs)) {
                        t.rekey(ctx);
                    }
                    s.theirs = Some(theirs);
                }
            }
            let direction = remote.direction.mirror();
            if let Some(session) = &s.session {
                session.set_direction(direction);
            }
            s.local.direction = direction;
            s.remote = Some(remote);
            out.push(s.local.clone());
        }
        out
    }

    /// The far end answered our re-INVITE offering `offered` on each
    /// section: each session takes what both allow.
    pub(crate) fn reanswered(&mut self, answer: &[u8], offered: MediaDirection) {
        let answered = sdp::sections(answer);
        for (i, s) in self.sections.iter_mut().enumerate() {
            let Some(session) = &s.session else { continue };
            match answered.get(i) {
                Some(r) if r.live() => {
                    let d = offered.and(r.direction.mirror());
                    session.set_direction(d);
                    s.local.direction = offered;
                }
                _ => s.stop(),
            }
        }
    }

    /// The direction each running section has now (tests, stats).
    #[allow(dead_code)]
    pub(crate) fn directions(&self) -> Vec<Option<MediaDirection>> {
        self.sections
            .iter()
            .map(|s| s.session.as_ref().map(|v| v.direction()))
            .collect()
    }
}
