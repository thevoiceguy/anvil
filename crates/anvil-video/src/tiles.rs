//! Where pictures are shown: a tile per incoming stream and one for the
//! self-view. The host fills tiles; the app attaches a [`Renderer`] (a
//! Flutter texture) to the ones on its screen. A tile keeps its last
//! picture, so one attached late, or a frozen picture, still shows.

use std::collections::HashMap;
use std::sync::Arc;

use anvil_core::video::host::StreamId;
use parking_lot::Mutex;

use crate::picture::Picture;

/// Which tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TileKey {
    /// An incoming stream (`Event::VideoStarted` names it).
    Stream(StreamId),
    /// Our camera, as it is captured.
    SelfView,
    /// Our shared screen, as it is captured.
    ShareView,
}

/// Something that puts pictures on a screen. Called on the host's
/// threads, at the moment each picture is to be seen.
pub trait Renderer: Send + Sync {
    fn present(&self, picture: &Picture);
}

/// What a tile has shown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TileStats {
    pub width: u32,
    pub height: u32,
    /// Pictures shown.
    pub presented: u64,
    /// The picture is frozen until a keyframe.
    pub frozen: bool,
    /// Its stream has ended (the tile is kept until it is detached).
    pub ended: bool,
}

struct Tile {
    renderer: Option<Arc<dyn Renderer>>,
    last: Option<Arc<Picture>>,
    stats: TileStats,
}

/// Every tile, shared by the host's sources and sinks and the app.
#[derive(Clone, Default)]
pub struct Tiles {
    inner: Arc<Mutex<HashMap<TileKey, Tile>>>,
}

impl Tiles {
    /// Show `renderer` on the tile (made empty if it has no picture yet);
    /// the tile's last picture is presented at once.
    pub fn attach(&self, key: TileKey, renderer: Arc<dyn Renderer>) {
        let last = {
            let mut tiles = self.inner.lock();
            let tile = tiles.entry(key).or_insert_with(Tile::empty);
            tile.renderer = Some(Arc::clone(&renderer));
            tile.last.clone()
        };
        if let Some(p) = last {
            renderer.present(&p);
        }
    }

    /// Stop showing the tile; an ended stream's tile is forgotten.
    pub fn detach(&self, key: TileKey) {
        let mut tiles = self.inner.lock();
        if let Some(tile) = tiles.get_mut(&key) {
            tile.renderer = None;
            if tile.stats.ended {
                tiles.remove(&key);
            }
        }
    }

    /// The tile's last picture.
    pub fn last(&self, key: TileKey) -> Option<Arc<Picture>> {
        self.inner.lock().get(&key).and_then(|t| t.last.clone())
    }

    pub fn stats(&self, key: TileKey) -> Option<TileStats> {
        self.inner.lock().get(&key).map(|t| t.stats)
    }

    /// The tiles there are.
    pub fn keys(&self) -> Vec<TileKey> {
        self.inner.lock().keys().copied().collect()
    }

    /// A picture to show now.
    pub(crate) fn present(&self, key: TileKey, picture: Picture) {
        let picture = Arc::new(picture);
        let renderer = {
            let mut tiles = self.inner.lock();
            let tile = tiles.entry(key).or_insert_with(Tile::empty);
            tile.stats.width = picture.width;
            tile.stats.height = picture.height;
            tile.stats.presented += 1;
            tile.stats.frozen = false;
            tile.stats.ended = false;
            tile.last = Some(Arc::clone(&picture));
            tile.renderer.clone()
        };
        if let Some(r) = renderer {
            r.present(&picture);
        }
    }

    pub(crate) fn set_frozen(&self, key: TileKey) {
        if let Some(t) = self.inner.lock().get_mut(&key) {
            t.stats.frozen = true;
        }
    }

    /// The stream has ended: the tile goes once nothing shows it.
    pub(crate) fn end(&self, key: TileKey) {
        let mut tiles = self.inner.lock();
        if let Some(t) = tiles.get_mut(&key) {
            if t.renderer.is_none() {
                tiles.remove(&key);
            } else {
                t.stats.ended = true;
            }
        }
    }
}

impl Tile {
    fn empty() -> Self {
        Self {
            renderer: None,
            last: None,
            stats: TileStats::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[derive(Default)]
    struct Count(AtomicU32);
    impl Renderer for Count {
        fn present(&self, _: &Picture) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn a_renderer_attached_late_sees_the_last_picture_and_an_ended_tile_goes_when_detached() {
        let tiles = Tiles::default();
        let key = TileKey::Stream(StreamId(7));
        tiles.present(key, Picture::black(4, 4));
        let r = Arc::new(Count::default());
        tiles.attach(key, r.clone());
        assert_eq!(r.0.load(Ordering::Relaxed), 1);
        tiles.present(key, Picture::black(4, 4));
        assert_eq!(r.0.load(Ordering::Relaxed), 2);
        tiles.end(key);
        assert!(tiles.stats(key).unwrap().ended, "kept while shown");
        tiles.detach(key);
        assert!(tiles.stats(key).is_none());
        // Unshown, an ended tile goes at once.
        tiles.present(key, Picture::black(4, 4));
        tiles.end(key);
        assert!(tiles.keys().is_empty());
    }
}
