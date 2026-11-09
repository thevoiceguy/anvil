//! A video host without a camera or a codec: numbered coded frames out,
//! and a record of what came in. The core's tests, the CLI and CI use it.
//!
//! A frame is valid for its codec's payload format (H.264 Annex B with an
//! SPS and a PPS before each IDR; VP8 with the key-frame bit), so it is
//! packetized and reassembled as a real one is. It carries its number as
//! eight hex digits after the header; the rest is padding, sized so a
//! keyframe is split across several packets.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::{BufMut, Bytes, BytesMut};
use parking_lot::Mutex;

use super::host::{
    CameraInfo, CodedFrame, EncodingId, ReceiveConfig, Resolution, SendConfig, StreamId,
    VideoCodec, VideoCodecCaps, VideoHost, VideoSink, VideoSource,
};
use crate::error::AnvilError;

/// A host that sends numbered frames and records what it receives.
/// Clones share one record.
#[derive(Clone)]
pub struct SyntheticVideoHost {
    codecs: Vec<VideoCodec>,
    fps: u32,
    /// Fixed sizes; `None`, sized by the target as an encoder would.
    sizes: Option<(usize, usize)>,
    record: Arc<Mutex<SyntheticRecord>>,
}

/// What a synthetic host saw.
#[derive(Debug, Default, Clone)]
pub struct SyntheticRecord {
    /// Each source started, in order.
    pub sent: Vec<SentSource>,
    /// Each incoming stream, by id.
    pub received: BTreeMap<StreamId, ReceivedStream>,
}

#[derive(Debug, Clone)]
pub struct SentSource {
    pub config: SendConfig,
    /// Frames handed to the core.
    pub frames: u32,
    /// Keyframes asked for by the core (a PLI or FIR from the far end).
    pub keyframe_requests: u32,
    /// Each target the core set: (kbps, max, fps).
    pub targets: Vec<(u32, Resolution, u32)>,
    pub stopped: bool,
}

#[derive(Debug, Clone)]
pub struct ReceivedStream {
    pub config: ReceiveConfig,
    /// Each frame's number and whether it was a keyframe, as shown.
    pub frames: Vec<(u32, bool)>,
    /// For each frame, how long after it was handed over it was to be
    /// shown, in ms (playout delay and lip sync).
    pub waits_ms: Vec<f32>,
    /// Frames that were not ours (not numbered as this host numbers).
    pub unreadable: u32,
    /// Times the core said the picture was lost.
    pub lost: u32,
    /// The tile is gone (the stream ended).
    pub stopped: bool,
}

impl ReceivedStream {
    /// The frame numbers shown, in order.
    pub fn numbers(&self) -> Vec<u32> {
        self.frames.iter().map(|(n, _)| *n).collect()
    }
}

impl Default for SyntheticVideoHost {
    fn default() -> Self {
        Self::new(&[VideoCodec::H264, VideoCodec::Vp8])
    }
}

impl SyntheticVideoHost {
    /// A host that encodes and decodes `codecs`, at 30 frames a second,
    /// each frame sized to the target bit rate (a keyframe four times
    /// the rest), as an encoder's rate control would.
    pub fn new(codecs: &[VideoCodec]) -> Self {
        Self {
            codecs: codecs.to_vec(),
            fps: 30,
            sizes: None,
            record: Arc::default(),
        }
    }

    pub fn with_fps(mut self, fps: u32) -> Self {
        self.fps = fps.max(1);
        self
    }

    /// The size of a keyframe and of any other frame, in bytes, whatever
    /// the target.
    pub fn with_frame_sizes(mut self, keyframe: usize, frame: usize) -> Self {
        self.sizes = Some((keyframe.max(32), frame.max(32)));
        self
    }

    /// What it has seen so far.
    pub fn record(&self) -> SyntheticRecord {
        self.record.lock().clone()
    }
}

impl VideoHost for SyntheticVideoHost {
    fn codecs(&self) -> Vec<VideoCodecCaps> {
        self.codecs
            .iter()
            .map(|&codec| VideoCodecCaps {
                codec,
                encode: true,
                decode: true,
            })
            .collect()
    }

    fn cameras(&self) -> Vec<CameraInfo> {
        vec![CameraInfo {
            id: "synthetic".into(),
            name: "Synthetic camera".into(),
        }]
    }

    fn start_send(&self, cfg: SendConfig) -> Result<Box<dyn VideoSource>, AnvilError> {
        if !self.codecs.contains(&cfg.codec) {
            return Err(AnvilError::Codec(format!("no {:?} encoder", cfg.codec)));
        }
        let index = {
            let mut record = self.record.lock();
            record.sent.push(SentSource {
                config: cfg.clone(),
                frames: 0,
                keyframe_requests: 0,
                targets: Vec::new(),
                stopped: false,
            });
            record.sent.len() - 1
        };
        let mut tick = tokio::time::interval(Duration::from_millis(1000 / u64::from(self.fps)));
        // Ticks keep to the schedule (a late wake does not push the rest
        // back), so the rate is the one asked for wherever timers are
        // coarse: Windows wakes a timer on its 15.6 ms tick.
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Burst);
        Ok(Box::new(SyntheticSource {
            codec: cfg.codec,
            encoding: cfg.encodings.first().copied().unwrap_or(EncodingId(0)),
            tick,
            next: 0,
            keyframe: Arc::new(AtomicBool::new(true)),
            fps: self.fps,
            sizes: self.sizes,
            target_kbps: Arc::new(AtomicU32::new(cfg.kbps.max(1))),
            record: Arc::clone(&self.record),
            index,
            requests: Arc::new(AtomicU32::new(0)),
        }))
    }

    fn start_receive(
        &self,
        stream: StreamId,
        cfg: ReceiveConfig,
    ) -> Result<Box<dyn VideoSink>, AnvilError> {
        if !self.codecs.contains(&cfg.codec) {
            return Err(AnvilError::Codec(format!("no {:?} decoder", cfg.codec)));
        }
        self.record.lock().received.insert(
            stream,
            ReceivedStream {
                config: cfg,
                frames: Vec::new(),
                waits_ms: Vec::new(),
                unreadable: 0,
                lost: 0,
                stopped: false,
            },
        );
        Ok(Box::new(SyntheticSink {
            codec: cfg.codec,
            stream,
            record: Arc::clone(&self.record),
        }))
    }
}

struct SyntheticSource {
    codec: VideoCodec,
    encoding: EncodingId,
    tick: tokio::time::Interval,
    next: u32,
    keyframe: Arc<AtomicBool>,
    fps: u32,
    sizes: Option<(usize, usize)>,
    target_kbps: Arc<AtomicU32>,
    record: Arc<Mutex<SyntheticRecord>>,
    index: usize,
    requests: Arc<AtomicU32>,
}

/// A keyframe at least this often, as an encoder's GOP would.
const KEYFRAME_EVERY: u32 = 300;

#[async_trait]
impl VideoSource for SyntheticSource {
    async fn next_frame(&mut self) -> Option<(EncodingId, CodedFrame)> {
        self.tick.tick().await;
        let number = self.next;
        self.next += 1;
        let keyframe =
            self.keyframe.swap(false, Ordering::Relaxed) || number.is_multiple_of(KEYFRAME_EVERY);
        let (key_bytes, frame_bytes) = self.sizes.unwrap_or_else(|| {
            // A second's bytes over a GOP of one keyframe at four times
            // the others' size, spread across the frames.
            let per_frame =
                self.target_kbps.load(Ordering::Relaxed) as usize * 125 / self.fps.max(1) as usize;
            (per_frame * 4, per_frame.max(64))
        });
        let size = if keyframe { key_bytes } else { frame_bytes };
        let data = frame(self.codec, number, keyframe, size);
        if let Some(s) = self.record.lock().sent.get_mut(self.index) {
            s.frames += 1;
            s.keyframe_requests = self.requests.load(Ordering::Relaxed);
        }
        Some((
            self.encoding,
            CodedFrame {
                data,
                keyframe,
                captured: Instant::now(),
            },
        ))
    }

    fn request_keyframe(&self) {
        self.keyframe.store(true, Ordering::Relaxed);
        let n = self.requests.fetch_add(1, Ordering::Relaxed) + 1;
        if let Some(s) = self.record.lock().sent.get_mut(self.index) {
            s.keyframe_requests = n;
        }
    }

    fn set_target(&self, kbps: u32, max: Resolution, fps: u32) {
        self.target_kbps.store(kbps.max(1), Ordering::Relaxed);
        if let Some(s) = self.record.lock().sent.get_mut(self.index) {
            s.targets.push((kbps, max, fps));
        }
    }
}

impl Drop for SyntheticSource {
    fn drop(&mut self) {
        if let Some(s) = self.record.lock().sent.get_mut(self.index) {
            s.stopped = true;
        }
    }
}

struct SyntheticSink {
    codec: VideoCodec,
    stream: StreamId,
    record: Arc<Mutex<SyntheticRecord>>,
}

impl VideoSink for SyntheticSink {
    fn frame(&mut self, frame: CodedFrame, at: Instant) {
        let now = Instant::now();
        let wait = if at >= now {
            at.duration_since(now).as_secs_f32()
        } else {
            -now.duration_since(at).as_secs_f32()
        };
        let number = number_of(self.codec, &frame.data);
        let mut record = self.record.lock();
        let Some(s) = record.received.get_mut(&self.stream) else {
            return;
        };
        match number {
            Some(n) => {
                s.frames.push((n, frame.keyframe));
                s.waits_ms.push(wait * 1000.0);
            }
            None => s.unreadable += 1,
        }
    }

    fn lost(&mut self) {
        if let Some(s) = self.record.lock().received.get_mut(&self.stream) {
            s.lost += 1;
        }
    }
}

impl Drop for SyntheticSink {
    fn drop(&mut self) {
        if let Some(s) = self.record.lock().received.get_mut(&self.stream) {
            s.stopped = true;
        }
    }
}

const START: [u8; 4] = [0, 0, 0, 1];
/// Padding: never zero, so it can hold no start code.
const PAD: u8 = b'.';

/// Frame `number` of `codec`, `size` bytes or so.
pub(crate) fn frame(codec: VideoCodec, number: u32, keyframe: bool, size: usize) -> Bytes {
    let label = format!("{number:08x}");
    let mut out = BytesMut::with_capacity(size + 16);
    match codec {
        VideoCodec::H264 => {
            if keyframe {
                // SPS (7) and PPS (8): constrained baseline, level 3.1.
                out.put_slice(&START);
                out.put_slice(&[0x67, 0x42, 0xE0, 0x1F, 0xDA]);
                out.put_slice(&START);
                out.put_slice(&[0x68, 0xCE, 0x3C, 0x80]);
            }
            out.put_slice(&START);
            // IDR (5) or a non-IDR slice (1), nal_ref_idc 3 or 2.
            out.put_u8(if keyframe { 0x65 } else { 0x41 });
        }
        VideoCodec::Vp8 => {
            // The frame tag's P bit (bit 0) is clear on a key frame; the
            // rest of the first byte is set so it is never zero.
            out.put_u8(if keyframe { 0x10 } else { 0x11 });
        }
    }
    out.put_slice(label.as_bytes());
    while out.len() < size {
        out.put_u8(PAD);
    }
    out.freeze()
}

/// The number a frame made by [`frame`] carries.
pub(crate) fn number_of(codec: VideoCodec, data: &[u8]) -> Option<u32> {
    let body = match codec {
        VideoCodec::H264 => {
            let nal = *forge_rtp::video::payload::annexb_nal_units(data).last()?;
            nal.get(1..)?
        }
        VideoCodec::Vp8 => data.get(1..)?,
    };
    let label = std::str::from_utf8(body.get(..8)?).ok()?;
    u32::from_str_radix(label, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_carries_its_number_through_its_own_payload_format() {
        for codec in [VideoCodec::H264, VideoCodec::Vp8] {
            for keyframe in [true, false] {
                let f = frame(codec, 0x1234, keyframe, 3000);
                assert_eq!(number_of(codec, &f), Some(0x1234), "{codec:?} {keyframe}");
                let payloads = forge_rtp::video::packetize(
                    codec.forge(),
                    &forge_rtp::video::CodedFrame {
                        timestamp: 0,
                        keyframe,
                        data: f.clone(),
                    },
                    1200,
                )
                .unwrap();
                assert!(payloads.len() > 1, "split across packets");
                let first = forge_rtp::video::inspect(codec.forge(), &payloads[0]);
                assert!(first.frame_start);
                assert_eq!(first.keyframe, keyframe, "{codec:?}");
                let refs: Vec<&[u8]> = payloads.iter().map(|p| p.as_ref()).collect();
                let back = forge_rtp::video::depacketize(codec.forge(), &refs).unwrap();
                assert_eq!(number_of(codec, &back), Some(0x1234));
            }
        }
    }
}
