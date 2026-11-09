//! An incoming stream decoded on a thread of its own and shown in its
//! tile at the moment the core gives each frame.

use std::sync::mpsc;
use std::time::Instant;

use anvil_core::video::host::{CodedFrame, StreamId, VideoSink};

use crate::codec::Decoder;
use crate::tiles::{TileKey, Tiles};

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
    while let Ok(message) = rx.recv() {
        match message {
            Message::Frame(frame, at) => {
                if wait_for_key && !frame.keyframe {
                    continue;
                }
                match decoder.decode(&frame.data) {
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
                        tracing::debug!(?key, "video decoder: {e}");
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
