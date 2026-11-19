//! An incoming stream decoded on a thread of its own and shown in its
//! tile at the moment the core gives each frame.

use std::sync::mpsc;
use std::time::Instant;

use anvil_core::video::host::{CodedFrame, StreamId, VideoSink};

use crate::codec::Decoder;
use crate::tiles::{Received, TileKey, Tiles};

enum Message {
    Frame(CodedFrame, Instant),
    Lost,
}

pub(crate) struct RenderSink {
    tx: mpsc::Sender<Message>,
}

impl RenderSink {
    pub(crate) fn start(stream: StreamId, decoder: Box<dyn Decoder>, tiles: Tiles) -> Self {
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name(format!("anvil-video-recv-{}", stream.0))
            .spawn(move || run(TileKey::Stream(stream), decoder, &rx, &tiles));
        if let Err(e) = spawned {
            tracing::warn!("video receive thread: {e}");
        }
        Self { tx }
    }
}

impl VideoSink for RenderSink {
    fn frame(&mut self, frame: CodedFrame, at: Instant) {
        let _ = self.tx.send(Message::Frame(frame, at));
    }

    fn lost(&mut self) {
        let _ = self.tx.send(Message::Lost);
    }
}

fn run(key: TileKey, mut decoder: Box<dyn Decoder>, rx: &mpsc::Receiver<Message>, tiles: &Tiles) {
    // Nothing is shown before a keyframe: a decoder fed anything else
    // makes garbage.
    let mut wait_for_key = true;
    // What became of each frame, for whoever asks why the tile is empty.
    let mut seen = Received::default();
    tiles.set_received(key, seen.clone());
    while let Ok(message) = rx.recv() {
        match message {
            Message::Frame(frame, at) => {
                seen.frames += 1;
                seen.keyframes += u64::from(frame.keyframe);
                if wait_for_key && !frame.keyframe {
                    seen.skipped += 1;
                    if seen.skipped % 300 == 0 {
                        tracing::warn!(
                            ?key,
                            "video: {} frames waiting for a keyframe",
                            seen.skipped
                        );
                    }
                    tiles.set_received(key, seen.clone());
                    continue;
                }
                let decoded = decoder.decode(&frame.data);
                match &decoded {
                    Ok(Some(_)) => seen.pictures += 1,
                    Ok(None) => seen.empty += 1,
                    Err(e) => {
                        seen.failed += 1;
                        seen.error = Some(e.clone());
                    }
                }
                // The decoder's account while it shows nothing, and now and
                // then after (it is a string made each time).
                if seen.pictures <= 1 || seen.frames % 30 == 0 {
                    seen.decoder = decoder.report();
                }
                tiles.set_received(key, seen.clone());
                match decoded {
                    Ok(Some(mut picture)) => {
                        wait_for_key = false;
                        // Decoded early; shown on time (a late frame at once).
                        let now = Instant::now();
                        if at > now {
                            std::thread::sleep(at - now);
                        }
                        picture.captured = at;
                        tiles.present(key, picture);
                    }
                    Ok(None) => {}
                    Err(e) => {
                        if seen.failed == 1 || seen.failed % 100 == 0 {
                            tracing::warn!(?key, "video decoder ({} failed): {e}", seen.failed);
                        }
                        wait_for_key = true;
                        tiles.set_frozen(key);
                    }
                }
            }
            Message::Lost => {
                wait_for_key = true;
                tiles.set_frozen(key);
            }
        }
    }
    tiles.end(key);
}
