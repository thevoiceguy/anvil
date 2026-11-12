//! Linux screens from the X server (x11rb: the protocol in Rust). Each
//! RandR monitor is a screen; without RandR, the whole root window is
//! one. Under Wayland only XWayland's windows are on the X server, so a
//! Wayland desktop shares through its portal instead (not here yet).

use std::time::{Duration, Instant};

use anvil_core::video::host::{Resolution, ScreenInfo};
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat, ImageOrder, Window};
use x11rb::rust_connection::RustConnection;

use crate::camera::Capture;
use crate::picture::Picture;
use crate::screen::Screens;

/// The X server's monitors (`$DISPLAY`'s).
#[derive(Debug, Clone, Copy, Default)]
pub struct X11Screens;

/// A part of the root window to capture.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Area {
    id: String,
    name: String,
    x: i16,
    y: i16,
    width: u16,
    height: u16,
}

/// The server's monitors, or the root window whole.
fn areas(conn: &RustConnection, root: Window, width: u16, height: u16) -> Vec<Area> {
    let whole = || {
        vec![Area {
            id: "root".into(),
            name: format!("Screen ({width}×{height})"),
            x: 0,
            y: 0,
            width,
            height,
        }]
    };
    let monitors = match conn
        .randr_get_monitors(root, true)
        .ok()
        .and_then(|c| c.reply().ok())
    {
        Some(m) if !m.monitors.is_empty() => m.monitors,
        _ => return whole(),
    };
    let mut out: Vec<(bool, Area)> = monitors
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let label = conn
                .get_atom_name(m.name)
                .ok()
                .and_then(|c| c.reply().ok())
                .map(|a| String::from_utf8_lossy(&a.name).into_owned())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| format!("monitor-{i}"));
            (
                m.primary,
                Area {
                    name: format!("{label} ({}×{})", m.width, m.height),
                    id: label,
                    x: m.x,
                    y: m.y,
                    width: m.width,
                    height: m.height,
                },
            )
        })
        .collect();
    // The primary monitor first: what a share without a choice shows.
    out.sort_by_key(|(primary, _)| !primary);
    out.into_iter().map(|(_, a)| a).collect()
}

fn connect() -> Result<(RustConnection, Window, Vec<Area>), String> {
    let (conn, n) = x11rb::connect(None).map_err(|e| format!("no X server: {e}"))?;
    let screen = &conn.setup().roots[n];
    let (root, w, h) = (screen.root, screen.width_in_pixels, screen.height_in_pixels);
    let depth = screen.root_depth;
    let bpp = conn
        .setup()
        .pixmap_formats
        .iter()
        .find(|f| f.depth == depth)
        .map(|f| f.bits_per_pixel);
    if !(matches!(depth, 24 | 32) && bpp == Some(32))
        || conn.setup().image_byte_order != ImageOrder::LSB_FIRST
    {
        return Err(format!(
            "the X server's pictures are {depth}-bit at {bpp:?} bits a pixel: not shareable"
        ));
    }
    let list = areas(&conn, root, w, h);
    Ok((conn, root, list))
}

impl Screens for X11Screens {
    fn list(&self) -> Vec<ScreenInfo> {
        match connect() {
            Ok((_, _, areas)) => areas
                .into_iter()
                .map(|a| ScreenInfo {
                    id: a.id,
                    name: a.name,
                })
                .collect(),
            Err(e) => {
                tracing::debug!("no X11 screens: {e}");
                Vec::new()
            }
        }
    }

    fn open(
        &self,
        id: Option<&str>,
        _want: Resolution,
        fps: u32,
    ) -> Result<Box<dyn Capture>, String> {
        let (conn, root, areas) = connect()?;
        let area = match id {
            Some(id) => areas.into_iter().find(|a| a.id == id),
            None => areas.into_iter().next(),
        }
        .ok_or_else(|| format!("no screen {id:?}"))?;
        tracing::info!(screen = %area.name, "sharing an X11 screen");
        Ok(Box::new(X11Capture {
            conn,
            root,
            area,
            every: Duration::from_secs(1) / fps.clamp(1, 30),
            next_at: Instant::now(),
        }))
    }
}

/// One monitor, captured with `GetImage` at the rate asked. The source
/// fits each picture to what the network allows.
struct X11Capture {
    conn: RustConnection,
    root: Window,
    area: Area,
    every: Duration,
    next_at: Instant,
}

impl Capture for X11Capture {
    fn next(&mut self) -> Option<Picture> {
        let now = Instant::now();
        if self.next_at > now {
            std::thread::sleep(self.next_at - now);
        }
        // A capture that fell behind does not try to catch up.
        self.next_at = (self.next_at + self.every).max(Instant::now());
        let a = &self.area;
        let image = match self
            .conn
            .get_image(
                ImageFormat::Z_PIXMAP,
                self.root,
                a.x,
                a.y,
                a.width,
                a.height,
                !0,
            )
            .map_err(|e| e.to_string())
            .and_then(|c| c.reply().map_err(|e| e.to_string()))
        {
            Ok(i) => i,
            Err(e) => {
                // The monitor went away, or the server did.
                tracing::info!(screen = %a.name, "X11 screen capture ended: {e}");
                return None;
            }
        };
        let (w, h) = (u32::from(a.width), u32::from(a.height));
        let stride = image.data.len() / usize::from(a.height.max(1));
        if stride < w as usize * 4 {
            tracing::warn!(stride, w, "an X11 image narrower than its screen");
            return None;
        }
        let mut p = Picture::from_bgrx(&image.data, stride, w & !1, h & !1);
        p.captured = Instant::now();
        Some(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Against `$DISPLAY` (CI runs it under Xvfb with `-noreset`: an X
    /// server resets when its last client leaves, and a connection made
    /// during the reset is refused); skipped without one.
    #[test]
    fn an_x_servers_screen_is_listed_and_captured() {
        if std::env::var_os("DISPLAY").is_none() {
            eprintln!("skipped: no DISPLAY");
            return;
        }
        let screens = X11Screens.list();
        assert!(!screens.is_empty(), "a screen to share");
        let mut capture = X11Screens
            .open(None, Resolution::P1080, 10)
            .expect("the screen opens");
        let first = capture.next().expect("a picture");
        assert!(first.width >= 320 && first.height >= 200, "{first:?}");
        let second = capture.next().expect("another");
        assert_eq!((first.width, first.height), (second.width, second.height));
        // A screen that is not there is an error, not a stream that never
        // starts.
        assert!(X11Screens
            .open(Some("no-such-screen"), Resolution::P1080, 10)
            .is_err());
    }
}
