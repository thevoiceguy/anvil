//! A limit on the picture we send, across every call: a phone on cellular
//! sends at most 360p (FCP's `docs/SOFTPHONE_VIDEO.md` §2.5). The platform's
//! host is wrapped so each source it opens keeps to the limit, and a source
//! already sending takes a new limit at its next frame.

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use tokio::sync::watch;

use super::host::{
    CameraInfo, CodedFrame, EncodingId, ReceiveConfig, Resolution, ScreenInfo, SendConfig,
    StreamId, VideoCodecCaps, VideoHost, VideoSink, VideoSource,
};
use crate::AnvilError;

/// `max`, within `limit` (none: as it is). A limit is a size either way up,
/// as the hosts fit pictures.
fn within(max: Resolution, limit: Option<Resolution>) -> Resolution {
    match limit {
        Some(l) if l.width * l.height < max.width * max.height => l,
        _ => max,
    }
}

/// The platform's host, its sources kept within the limit.
pub(crate) struct LimitedHost {
    inner: Arc<dyn VideoHost>,
    limit: watch::Receiver<Option<Resolution>>,
}

impl LimitedHost {
    pub(crate) fn new(
        inner: Arc<dyn VideoHost>,
        limit: watch::Receiver<Option<Resolution>>,
    ) -> Self {
        Self { inner, limit }
    }
}

impl VideoHost for LimitedHost {
    fn codecs(&self) -> Vec<VideoCodecCaps> {
        self.inner.codecs()
    }

    fn cameras(&self) -> Vec<CameraInfo> {
        self.inner.cameras()
    }

    fn set_camera(&self, camera: Option<&str>) {
        self.inner.set_camera(camera);
    }

    fn screens(&self) -> Vec<ScreenInfo> {
        self.inner.screens()
    }

    fn set_screen(&self, screen: Option<&str>) {
        self.inner.set_screen(screen);
    }

    fn start_send(&self, cfg: SendConfig) -> Result<Box<dyn VideoSource>, AnvilError> {
        let mut limit = self.limit.clone();
        let now = *limit.borrow_and_update();
        let asked = (cfg.kbps, cfg.max, cfg.fps);
        let inner = self.inner.start_send(SendConfig {
            max: within(cfg.max, now),
            ..cfg
        })?;
        Ok(Box::new(LimitedSource {
            inner,
            limit,
            asked: Mutex::new(asked),
        }))
    }

    fn start_receive(
        &self,
        stream: StreamId,
        cfg: ReceiveConfig,
    ) -> Result<Box<dyn VideoSink>, AnvilError> {
        self.inner.start_receive(stream, cfg)
    }
}

struct LimitedSource {
    inner: Box<dyn VideoSource>,
    limit: watch::Receiver<Option<Resolution>>,
    /// The target the session last set, before the limit.
    asked: Mutex<(u32, Resolution, u32)>,
}

impl LimitedSource {
    fn apply(&self) {
        let (kbps, max, fps) = *self.asked.lock();
        self.inner
            .set_target(kbps, within(max, *self.limit.borrow()), fps);
    }
}

#[async_trait]
impl VideoSource for LimitedSource {
    async fn next_frame(&mut self) -> Option<(EncodingId, CodedFrame)> {
        if self.limit.has_changed().unwrap_or(false) {
            self.limit.borrow_and_update();
            self.apply();
        }
        self.inner.next_frame().await
    }

    fn request_keyframe(&self) {
        self.inner.request_keyframe();
    }

    fn set_target(&self, kbps: u32, max: Resolution, fps: u32) {
        *self.asked.lock() = (kbps, max, fps);
        self.apply();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::host::{VideoCodec, VideoKind};
    use crate::video::synthetic::SyntheticVideoHost;

    const P360: Resolution = Resolution {
        width: 640,
        height: 360,
    };

    fn cfg() -> SendConfig {
        SendConfig {
            codec: VideoCodec::Vp8,
            kind: VideoKind::Camera,
            camera: None,
            max: Resolution::P720,
            kbps: 1500,
            fps: 30,
            encodings: vec![EncodingId(0)],
        }
    }

    #[tokio::test]
    async fn a_source_keeps_to_the_limit_and_takes_a_new_one_at_its_next_frame() {
        let synthetic = SyntheticVideoHost::default();
        let (tx, rx) = watch::channel(Some(P360));
        let host = LimitedHost::new(Arc::new(synthetic.clone()), rx);
        let mut source = host.start_send(cfg()).unwrap();
        assert_eq!(
            synthetic.record().sent[0].config.max,
            P360,
            "opened within it"
        );

        // The session's ladder asks for 720p: it gets 360p.
        source.set_target(1500, Resolution::P720, 30);
        // Lower than the limit stays as asked.
        source.set_target(
            300,
            Resolution {
                width: 320,
                height: 180,
            },
            30,
        );
        // The limit lifted: the last ask, as it was, at the next frame.
        tx.send(None).unwrap();
        source.next_frame().await;
        source.set_target(1500, Resolution::P720, 30);
        assert_eq!(
            synthetic.record().sent[0].targets,
            vec![
                (1500, P360, 30),
                (
                    300,
                    Resolution {
                        width: 320,
                        height: 180
                    },
                    30
                ),
                (
                    300,
                    Resolution {
                        width: 320,
                        height: 180
                    },
                    30
                ),
                (1500, Resolution::P720, 30),
            ]
        );
        // Put back: a sending source steps down at its next frame.
        tx.send(Some(P360)).unwrap();
        source.next_frame().await;
        assert_eq!(
            synthetic.record().sent[0].targets.last(),
            Some(&(1500, P360, 30))
        );
    }
}
