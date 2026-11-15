//! What the platform supplies for video: the camera, the codecs and the
//! screen (FCP's `docs/SOFTPHONE_VIDEO.md` §2.3).
//!
//! The core owns the network; the host owns everything that touches a
//! picture. They pass **coded frames**, because the codec that matters on
//! each platform is its hardware one (VideoToolbox, MediaCodec, Media
//! Foundation), which works on the platform's own buffers.
//!
//! Nothing here names a transport or a call's SDP: a sink is one incoming
//! stream, whichever transport it came over and whoever sent it.

use std::time::Instant;

use async_trait::async_trait;
use bytes::Bytes;

use crate::error::AnvilError;

/// A video codec the core can carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VideoCodec {
    /// H.264 constrained baseline, `packetization-mode=1` (RFC 6184): a
    /// coded frame is an Annex B byte stream.
    H264,
    /// VP8 (RFC 7741): a coded frame is the raw VP8 frame.
    Vp8,
}

impl VideoCodec {
    pub(crate) fn forge(self) -> forge_core::VideoCodec {
        match self {
            VideoCodec::H264 => forge_core::VideoCodec::H264,
            VideoCodec::Vp8 => forge_core::VideoCodec::VP8,
        }
    }
}

/// What a host does with one codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoCodecCaps {
    pub codec: VideoCodec,
    pub encode: bool,
    pub decode: bool,
}

/// A camera the host can open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraInfo {
    pub id: String,
    pub name: String,
}

/// A screen (or a window) the host can share.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenInfo {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
}

impl Resolution {
    pub const P720: Resolution = Resolution {
        width: 1280,
        height: 720,
    };
    pub const P1080: Resolution = Resolution {
        width: 1920,
        height: 1080,
    };
}

/// What a stream carries: a camera, or content (a shared screen, RFC 4796
/// `a=content:slides`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VideoKind {
    Camera,
    Content,
}

/// One of a source's encodings. A source has one, unless the call sends
/// simulcast (RFC 8853, FCP's `docs/FORWARDING_ROOMS.md` §2.4): then
/// several, of one capture, each with its own id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EncodingId(pub u8);

/// One encoding a source is asked for: the largest picture it carries
/// and the most it may spend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Encoding {
    pub id: EncodingId,
    pub max: Resolution,
    pub kbps: u32,
}

/// What the network allows one encoding of a simulcast source now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodingTarget {
    pub kbps: u32,
    pub max: Resolution,
    pub fps: u32,
}

/// One incoming stream, unique within the process (a host can show
/// several calls' streams at once).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StreamId(pub u32);

/// What to capture and how to encode it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendConfig {
    pub codec: VideoCodec,
    pub kind: VideoKind,
    /// The camera to open (`None`, the host's default); ignored for
    /// content, whose screen is [`VideoHost::set_screen`]'s.
    pub camera: Option<String>,
    pub max: Resolution,
    pub kbps: u32,
    pub fps: u32,
    /// The encodings wanted, largest first. The first is the picture
    /// `max` and `kbps` describe; more than one is simulcast, which the
    /// core asks for only of a host whose [`VideoHost::simulcast`] allows
    /// it. A host that cannot simulcast sends the first.
    pub encodings: Vec<Encoding>,
}

impl SendConfig {
    /// The one encoding of a source without simulcast.
    pub fn single(max: Resolution, kbps: u32) -> Vec<Encoding> {
        vec![Encoding {
            id: EncodingId(0),
            max,
            kbps,
        }]
    }
}

/// What an incoming stream is, for its decoder and its tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceiveConfig {
    pub codec: VideoCodec,
    pub kind: VideoKind,
}

/// One coded frame: what an encoder gives and a decoder takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodedFrame {
    /// Annex B for H.264, the raw frame for VP8.
    pub data: Bytes,
    pub keyframe: bool,
    /// When it was captured: the sender's clock for a frame sent, ours for
    /// a frame received (the moment it stands for, once mapped across).
    pub captured: Instant,
}

/// The platform's video: its codecs, its cameras, and a source or a sink
/// per stream.
pub trait VideoHost: Send + Sync {
    /// What it encodes and decodes.
    fn codecs(&self) -> Vec<VideoCodecCaps>;
    fn cameras(&self) -> Vec<CameraInfo>;
    /// The camera a send without one opens (`None`, the host's own
    /// default); a camera being captured switches to it, where the host
    /// can, starting again on a keyframe.
    fn set_camera(&self, _camera: Option<&str>) {}
    /// The screens a share can show (none where the host cannot share).
    fn screens(&self) -> Vec<ScreenInfo> {
        Vec::new()
    }
    /// The screen a share captures (`None`, the host's choice: the main
    /// screen, or the system's picker where it has one); a share running
    /// switches to it, where the host can.
    fn set_screen(&self, _screen: Option<&str>) {}
    /// How many encodings of one camera it can make at once, of every
    /// codec it encodes (simulcast); 1, none beyond the one.
    fn simulcast(&self) -> usize {
        1
    }
    /// A camera or a screen, captured and encoded.
    fn start_send(&self, cfg: SendConfig) -> Result<Box<dyn VideoSource>, AnvilError>;
    /// One per incoming stream: decoded and shown.
    fn start_receive(
        &self,
        stream: StreamId,
        cfg: ReceiveConfig,
    ) -> Result<Box<dyn VideoSink>, AnvilError>;
}

/// Capture and encode, on the host. Dropping it stops the capture.
#[async_trait]
pub trait VideoSource: Send {
    /// The next coded frame, of one of the encodings asked for; `None`
    /// when the source has ended.
    async fn next_frame(&mut self) -> Option<(EncodingId, CodedFrame)>;
    /// Make the next frame a keyframe (the far end lost the picture).
    fn request_keyframe(&self);
    /// What the network allows now.
    fn set_target(&self, kbps: u32, max: Resolution, fps: u32);
    /// Simulcast: make the next frame of `encoding` a keyframe. A host
    /// that cannot single one out makes one of every encoding.
    fn request_keyframe_of(&self, encoding: EncodingId) {
        let _ = encoding;
        self.request_keyframe();
    }
    /// Simulcast: what the network allows `encoding` now; `None` pauses
    /// it (nothing coded until it has a target again, starting on a
    /// keyframe). Called for every encoding whenever the share changes.
    fn set_encoding_target(&self, encoding: EncodingId, target: Option<EncodingTarget>) {
        if encoding == EncodingId(0) {
            if let Some(t) = target {
                self.set_target(t.kbps, t.max, t.fps);
            }
        }
    }
}

/// Decode and render, on the host. Dropping it removes the tile.
pub trait VideoSink: Send {
    /// A frame to show at `at`.
    fn frame(&mut self, frame: CodedFrame, at: Instant);
    /// The picture is broken until the next keyframe: freeze it.
    fn lost(&mut self);
}

/// Which codecs both sides of the host can do (encode and decode), in the
/// core's order of preference: H.264, then VP8.
pub(crate) fn usable_codecs(host: &dyn VideoHost) -> Vec<VideoCodec> {
    let caps = host.codecs();
    [VideoCodec::H264, VideoCodec::Vp8]
        .into_iter()
        .filter(|c| caps.iter().any(|k| k.codec == *c && k.encode && k.decode))
        .collect()
}
