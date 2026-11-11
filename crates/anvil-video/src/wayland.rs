//! Linux screens under Wayland: the desktop portal's ScreenCast (the
//! user picks a screen or a window in the desktop's own dialog) and the
//! PipeWire stream it opens, read on a thread of its own.
//!
//! A Wayland desktop shows no screen to an application but through the
//! portal, so there is one screen to list: the one chosen when the share
//! starts. The choice is remembered for the rest of the run (the portal's
//! restore token), so a second share in the same run asks again only if
//! the desktop does.

use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use anvil_core::video::host::{Resolution, ScreenInfo};
use parking_lot::Mutex;
use pipewire as pw;
use pw::spa;

use crate::camera::Capture;
use crate::picture::Picture;
use crate::screen::Screens;

/// The one screen a portal lists: the one the user chooses.
pub const PORTAL_SCREEN: &str = "portal";

/// The portal's screens: a Wayland desktop's.
#[derive(Debug, Clone, Copy, Default)]
pub struct PortalScreens;

/// Whether this session is Wayland's (its portal is then how a screen
/// is shared).
pub fn is_wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t.eq_ignore_ascii_case("wayland"))
}

/// The portal's token for the last choice, to offer it again.
static RESTORE: Mutex<Option<String>> = Mutex::new(None);

impl Screens for PortalScreens {
    fn list(&self) -> Vec<ScreenInfo> {
        vec![ScreenInfo {
            id: PORTAL_SCREEN.into(),
            name: "A screen or window (chosen when sharing)".into(),
        }]
    }

    fn open(
        &self,
        _id: Option<&str>,
        want: Resolution,
        fps: u32,
    ) -> Result<Box<dyn Capture>, String> {
        // The dialog waits on the user: the handshake runs on the capture's
        // own thread, at its first picture, not here.
        Ok(Box::new(PortalCapture {
            stream: None,
            want,
            every: Duration::from_secs(1) / fps.clamp(1, 30),
            next_at: Instant::now(),
            failed: false,
        }))
    }
}

/// What the portal gave: PipeWire's socket for this session, and the node
/// of the stream chosen.
struct Cast {
    fd: OwnedFd,
    node: u32,
}

/// Ask the portal for a screen or a window. Blocks until the user chooses
/// or cancels (an error).
fn ask_portal() -> Result<Cast, String> {
    use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
    use ashpd::desktop::PersistMode;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("a runtime for the portal: {e}"))?;
    rt.block_on(async {
        let proxy = Screencast::new()
            .await
            .map_err(|e| format!("no screen-cast portal: {e}"))?;
        let session = proxy
            .create_session(Default::default())
            .await
            .map_err(|e| format!("the portal's session: {e}"))?;
        let token = RESTORE.lock().clone();
        proxy
            .select_sources(
                &session,
                SelectSourcesOptions::default()
                    .set_cursor_mode(CursorMode::EMBEDDED)
                    .set_sources(SourceType::MONITOR | SourceType::WINDOW)
                    .set_multiple(false)
                    .set_persist_mode(PersistMode::Application)
                    .set_restore_token(token.as_deref()),
            )
            .await
            .map_err(|e| format!("the portal's sources: {e}"))?;
        let response = proxy
            .start(&session, None, Default::default())
            .await
            .map_err(|e| format!("the portal's start: {e}"))?
            .response()
            .map_err(|e| format!("no screen chosen: {e}"))?;
        if let Some(t) = response.restore_token() {
            *RESTORE.lock() = Some(t.to_string());
        }
        let node = response
            .streams()
            .first()
            .map(|s| s.pipe_wire_node_id())
            .ok_or("the portal gave no stream")?;
        let fd = proxy
            .open_pipe_wire_remote(&session, Default::default())
            .await
            .map_err(|e| format!("the portal's PipeWire: {e}"))?;
        // The session ends with the stream: PipeWire keeps the node while
        // its socket is open.
        std::mem::forget(session);
        Ok(Cast { fd, node })
    })
}

/// A share through the portal: the handshake at the first picture, then
/// the stream's latest picture at the asked rate.
struct PortalCapture {
    stream: Option<PipeWireStream>,
    want: Resolution,
    every: Duration,
    next_at: Instant,
    failed: bool,
}

impl Capture for PortalCapture {
    fn next(&mut self) -> Option<Picture> {
        if self.failed {
            return None;
        }
        if self.stream.is_none() {
            let started =
                ask_portal().and_then(|cast| PipeWireStream::start(Some(cast.fd), cast.node));
            match started {
                Ok(s) => self.stream = Some(s),
                Err(e) => {
                    tracing::info!("no screen to share: {e}");
                    self.failed = true;
                    return None;
                }
            }
        }
        let now = Instant::now();
        if self.next_at > now {
            std::thread::sleep(self.next_at - now);
        }
        self.next_at = (self.next_at + self.every).max(Instant::now());
        let _ = self.want;
        self.stream.as_mut()?.latest()
    }
}

/// What the stream's thread hands over: each picture as it comes.
type Frame = Picture;

/// A PipeWire video stream from `node`, read on its own thread: each
/// frame converted as it comes, the newest kept.
pub(crate) struct PipeWireStream {
    frames: mpsc::Receiver<Frame>,
    /// The stream ended (the user stopped it in the desktop, the screen
    /// went away, PipeWire failed).
    gone: Arc<AtomicBool>,
    quit: pw::channel::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
    last: Option<Picture>,
    ended: bool,
}

impl PipeWireStream {
    /// Connect to `node` — on the portal's socket `fd`, or on the user's
    /// PipeWire with none — and start reading.
    pub(crate) fn start(fd: Option<OwnedFd>, node: u32) -> Result<Self, String> {
        let (tx, frames) = mpsc::sync_channel::<Frame>(2);
        let (quit, quit_rx) = pw::channel::channel::<()>();
        let (ready_tx, ready) = mpsc::channel::<Result<(), String>>();
        let gone = Arc::new(AtomicBool::new(false));
        let ended = Arc::clone(&gone);
        let thread = std::thread::Builder::new()
            .name("anvil-pipewire".into())
            .spawn(move || {
                if let Err(e) = run(fd, node, tx, quit_rx, &ready_tx, &ended) {
                    tracing::info!("the shared screen's stream: {e}");
                    let _ = ready_tx.send(Err(e));
                }
                ended.store(true, Ordering::Relaxed);
            })
            .map_err(|e| format!("a PipeWire thread: {e}"))?;
        match ready.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Self {
                frames,
                gone,
                quit,
                thread: Some(thread),
                last: None,
                ended: false,
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err("PipeWire did not answer".into()),
        }
    }

    /// The newest picture, waiting for the first (or for one at all after
    /// the stream stalls) up to a second, else the last again: a screen
    /// that does not change sends no frames. `None` once it has ended.
    pub(crate) fn latest(&mut self) -> Option<Picture> {
        if self.ended {
            return None;
        }
        let mut newest = None;
        while let Ok(p) = self.frames.try_recv() {
            newest = Some(p);
        }
        if newest.is_none() && self.last.is_none() {
            newest = self.frames.recv_timeout(Duration::from_secs(1)).ok();
        }
        if self.gone.load(Ordering::Relaxed) {
            tracing::info!("the shared screen ended");
            self.ended = true;
            return None;
        }
        if let Some(p) = newest {
            self.last = Some(p);
        }
        let mut p = self.last.clone()?;
        p.captured = Instant::now();
        Some(p)
    }
}

impl Drop for PipeWireStream {
    fn drop(&mut self) {
        let _ = self.quit.send(());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The negotiated format: how to read a frame.
#[derive(Default, Clone, Copy)]
struct Format {
    width: u32,
    height: u32,
    /// Byte positions of R, G and B in a pixel; `None` before a format.
    rgb: Option<[usize; 3]>,
}

fn rgb_of(format: spa::param::video::VideoFormat) -> Option<[usize; 3]> {
    use spa::param::video::VideoFormat as F;
    match format {
        F::BGRx | F::BGRA => Some([2, 1, 0]),
        F::RGBx | F::RGBA => Some([0, 1, 2]),
        F::xRGB | F::ARGB => Some([1, 2, 3]),
        F::xBGR | F::ABGR => Some([3, 2, 1]),
        _ => None,
    }
}

/// The buffers a frame of `width` × `height` packed 32-bit pixels needs:
/// a few, one block each, in memory that maps (`MemPtr` or `MemFd`) — a
/// GPU's buffer (DMA-BUF) cannot be read as bytes, and a producer left to
/// choose may pick one.
fn buffers_param(width: u32, height: u32) -> Vec<u8> {
    use pw::spa::pod::{ChoiceValue, Object, Property, PropertyFlags, Value};
    use pw::spa::sys as s;
    use pw::spa::utils::{Choice, ChoiceEnum, ChoiceFlags};
    let stride = (width * 4) as i32;
    let int = |key: u32, v: i32| Property {
        key,
        flags: PropertyFlags::empty(),
        value: Value::Int(v),
    };
    let object = Object {
        type_: s::SPA_TYPE_OBJECT_ParamBuffers,
        id: s::SPA_PARAM_Buffers,
        properties: vec![
            Property {
                key: s::SPA_PARAM_BUFFERS_buffers,
                flags: PropertyFlags::empty(),
                value: Value::Choice(ChoiceValue::Int(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Range {
                        default: 4,
                        min: 2,
                        max: 16,
                    },
                ))),
            },
            int(s::SPA_PARAM_BUFFERS_blocks, 1),
            int(s::SPA_PARAM_BUFFERS_size, stride * height as i32),
            int(s::SPA_PARAM_BUFFERS_stride, stride),
            int(
                s::SPA_PARAM_BUFFERS_dataType,
                (1 << s::SPA_DATA_MemPtr) | (1 << s::SPA_DATA_MemFd),
            ),
        ],
    };
    pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &Value::Object(object),
    )
    .map(|(c, _)| c.into_inner())
    .unwrap_or_default()
}

fn run(
    fd: Option<OwnedFd>,
    node: u32,
    tx: mpsc::SyncSender<Frame>,
    quit: pw::channel::Receiver<()>,
    ready: &mpsc::Sender<Result<(), String>>,
    gone: &Arc<AtomicBool>,
) -> Result<(), String> {
    use pw::spa::pod::Pod;
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(|e| format!("PipeWire: {e}"))?;
    let context =
        pw::context::ContextRc::new(&mainloop, None).map_err(|e| format!("PipeWire: {e}"))?;
    let core = match fd {
        Some(fd) => context.connect_fd_rc(fd, None),
        None => context.connect_rc(None),
    }
    .map_err(|e| format!("PipeWire's connection: {e}"))?;
    // The node named as the target too: a session manager links a stream
    // to its `target.object` (the portal's own socket shows that node
    // alone either way).
    let stream = pw::stream::StreamBox::new(
        &core,
        "anvil-screen",
        pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
            "target.object" => node.to_string(),
        },
    )
    .map_err(|e| format!("PipeWire's stream: {e}"))?;

    let ended = Arc::clone(gone);
    let _listener = stream
        .add_local_listener_with_user_data(Format::default())
        .state_changed(move |_, _, _, new| {
            if let pw::stream::StreamState::Error(e) = &new {
                tracing::info!("the shared screen's stream failed: {e}");
            }
            if matches!(
                new,
                pw::stream::StreamState::Error(_) | pw::stream::StreamState::Unconnected
            ) {
                ended.store(true, Ordering::Relaxed);
            }
        })
        .param_changed(|stream, format, id, param| {
            let Some(param) = param else { return };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let mut info = spa::param::video::VideoInfoRaw::default();
            if info.parse(param).is_err() {
                return;
            }
            *format = Format {
                width: info.size().width,
                height: info.size().height,
                rgb: rgb_of(info.format()),
            };
            tracing::info!(
                width = format.width,
                height = format.height,
                format = ?info.format(),
                "sharing a Wayland screen"
            );
            // Buffers that hold a whole frame, in memory that maps.
            let buffers = buffers_param(format.width, format.height);
            if let Some(pod) = pw::spa::pod::Pod::from_bytes(&buffers) {
                if let Err(e) = stream.update_params(&mut [pod]) {
                    tracing::warn!("the shared screen's buffers: {e}");
                }
            }
        })
        .process(move |stream, format| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let (Some(rgb), w, h) = (format.rgb, format.width, format.height) else {
                return;
            };
            let datas = buffer.datas_mut();
            let Some(d) = datas.first_mut() else { return };
            let (offset, size, stride) = (
                d.chunk().offset() as usize,
                d.chunk().size() as usize,
                d.chunk().stride(),
            );
            let Some(bytes) = d.data() else { return };
            let stride = if stride > 0 {
                stride as usize
            } else {
                w as usize * 4
            };
            let end = (offset + size).min(bytes.len());
            let needed = stride * (h as usize).saturating_sub(1) + w as usize * 4;
            if h < 2
                || size == 0
                || end <= offset
                || stride < w as usize * 4
                || end - offset < needed
            {
                return;
            }
            let picture = Picture::from_rgb32(&bytes[offset..end], stride, w & !1, h & !1, rgb);
            // The reader takes the newest: a frame it has not read yet is
            // replaced, not queued behind.
            let _ = tx.try_send(picture);
        })
        .register()
        .map_err(|e| format!("PipeWire's listener: {e}"))?;

    // Packed 32-bit RGB in memory the stream maps: what a screen comes as,
    // and what a picture is made from without a GPU.
    let format = pw::spa::pod::object!(
        pw::spa::utils::SpaTypes::ObjectParamFormat,
        pw::spa::param::ParamType::EnumFormat,
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::MediaType,
            Id,
            pw::spa::param::format::MediaType::Video
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::MediaSubtype,
            Id,
            pw::spa::param::format::MediaSubtype::Raw
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            pw::spa::param::video::VideoFormat::BGRx,
            pw::spa::param::video::VideoFormat::BGRx,
            pw::spa::param::video::VideoFormat::BGRA,
            pw::spa::param::video::VideoFormat::RGBx,
            pw::spa::param::video::VideoFormat::RGBA,
            pw::spa::param::video::VideoFormat::xRGB,
            pw::spa::param::video::VideoFormat::xBGR
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            pw::spa::utils::Rectangle {
                width: 1920,
                height: 1080
            },
            pw::spa::utils::Rectangle {
                width: 16,
                height: 16
            },
            pw::spa::utils::Rectangle {
                width: 8192,
                height: 8192
            }
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            pw::spa::utils::Fraction { num: 30, denom: 1 },
            pw::spa::utils::Fraction { num: 0, denom: 1 },
            pw::spa::utils::Fraction { num: 120, denom: 1 }
        ),
    );
    let bytes: Vec<u8> = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(format),
    )
    .map_err(|e| format!("PipeWire's format: {e:?}"))?
    .0
    .into_inner();
    let mut params = [Pod::from_bytes(&bytes).ok_or("PipeWire's format")?];
    stream
        .connect(
            spa::utils::Direction::Input,
            Some(node),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(|e| format!("PipeWire's stream to node {node}: {e}"))?;

    // The screen going away (the user stopped it in the desktop, a window
    // closed) removes its node: the stream itself only pauses.
    let registry = core
        .get_registry_rc()
        .map_err(|e| format!("PipeWire's registry: {e}"))?;
    let removed = Arc::clone(gone);
    let _registry = registry
        .add_listener_local()
        .global_remove(move |id| {
            if id == node {
                removed.store(true, Ordering::Relaxed);
            }
        })
        .register();

    let ml = mainloop.clone();
    let _quit = quit.attach(mainloop.loop_(), move |()| ml.quit());
    let _ = ready.send(Ok(()));
    mainloop.run();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A screen as a desktop's compositor offers one: a PipeWire video
    /// source of solid BGRx `colour` (B, G, R), on its own thread. Its node
    /// id, once it has one.
    fn producer(colour: [u8; 3], quit: pw::channel::Receiver<()>, node: mpsc::Sender<u32>) {
        use pw::spa::pod::Pod;
        pw::init();
        let mainloop = pw::main_loop::MainLoopRc::new(None).unwrap();
        let context = pw::context::ContextRc::new(&mainloop, None).unwrap();
        let core = context.connect_rc(None).expect("a PipeWire daemon");
        let stream = pw::stream::StreamBox::new(
            &core,
            "anvil-test-screen",
            pw::properties::properties! {
                *pw::keys::MEDIA_CLASS => "Video/Source",
                *pw::keys::MEDIA_TYPE => "Video",
                *pw::keys::MEDIA_CATEGORY => "Source",
            },
        )
        .unwrap();
        let _listener = stream
            .add_local_listener_with_user_data(())
            .state_changed(move |stream, _, _, new| {
                if matches!(new, pw::stream::StreamState::Paused) {
                    let _ = node.send(stream.node_id());
                }
            })
            .process(move |stream, _| {
                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };
                let datas = buffer.datas_mut();
                let Some(d) = datas.first_mut() else { return };
                let len = 160 * 90 * 4;
                if let Some(bytes) = d.data() {
                    let n = len.min(bytes.len());
                    for px in bytes[..n].chunks_mut(4) {
                        px.copy_from_slice(&[colour[0], colour[1], colour[2], 255]);
                    }
                }
                let chunk = d.chunk_mut();
                *chunk.offset_mut() = 0;
                *chunk.stride_mut() = 160 * 4;
                *chunk.size_mut() = len as u32;
            })
            .register()
            .unwrap();
        let format = pw::spa::pod::object!(
            pw::spa::utils::SpaTypes::ObjectParamFormat,
            pw::spa::param::ParamType::EnumFormat,
            pw::spa::pod::property!(
                pw::spa::param::format::FormatProperties::MediaType,
                Id,
                pw::spa::param::format::MediaType::Video
            ),
            pw::spa::pod::property!(
                pw::spa::param::format::FormatProperties::MediaSubtype,
                Id,
                pw::spa::param::format::MediaSubtype::Raw
            ),
            pw::spa::pod::property!(
                pw::spa::param::format::FormatProperties::VideoFormat,
                Id,
                pw::spa::param::video::VideoFormat::BGRx
            ),
            pw::spa::pod::property!(
                pw::spa::param::format::FormatProperties::VideoSize,
                Rectangle,
                pw::spa::utils::Rectangle {
                    width: 160,
                    height: 90
                }
            ),
            pw::spa::pod::property!(
                pw::spa::param::format::FormatProperties::VideoFramerate,
                Fraction,
                pw::spa::utils::Fraction { num: 30, denom: 1 }
            ),
        );
        let bytes: Vec<u8> = pw::spa::pod::serialize::PodSerializer::serialize(
            std::io::Cursor::new(Vec::new()),
            &pw::spa::pod::Value::Object(format),
        )
        .unwrap()
        .0
        .into_inner();
        let buffers = buffers_param(160, 90);
        let mut params = [
            Pod::from_bytes(&bytes).unwrap(),
            Pod::from_bytes(&buffers).unwrap(),
        ];
        stream
            .connect(
                spa::utils::Direction::Output,
                None,
                pw::stream::StreamFlags::DRIVER | pw::stream::StreamFlags::MAP_BUFFERS,
                &mut params,
            )
            .unwrap();
        // A driver runs the graph: a frame each 33 ms, as a compositor
        // offers one each time the screen changes.
        let raw = stream.as_raw_ptr() as usize;
        let timer = mainloop.loop_().add_timer(move |_| {
            // SAFETY: the stream outlives the timer (both end with the loop
            // below, the timer first).
            unsafe {
                pw::sys::pw_stream_trigger_process(raw as *mut pw::sys::pw_stream);
            }
        });
        let _ = timer.update_timer(
            Some(Duration::from_millis(33)),
            Some(Duration::from_millis(33)),
        );
        let ml = mainloop.clone();
        let _quit = quit.attach(mainloop.loop_(), move |()| ml.quit());
        mainloop.run();
        drop(timer);
    }

    /// Against a PipeWire daemon with a session manager (CI runs one);
    /// skipped without `ANVIL_PIPEWIRE_TEST`.
    #[test]
    fn a_pipewire_screen_is_read_as_pictures_until_it_goes() {
        if std::env::var_os("ANVIL_PIPEWIRE_TEST").is_none() {
            eprintln!("skipped: no ANVIL_PIPEWIRE_TEST");
            return;
        }
        // Red, as BGRx.
        let (stop, quit) = pw::channel::channel::<()>();
        let (node_tx, node_rx) = mpsc::channel();
        let source = std::thread::spawn(move || producer([0, 0, 255], quit, node_tx));
        let node = node_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the source has a node");

        let mut screen = PipeWireStream::start(None, node).expect("the stream connects");
        let deadline = Instant::now() + Duration::from_secs(10);
        let picture = loop {
            if let Some(p) = screen.latest() {
                break p;
            }
            assert!(Instant::now() < deadline, "no picture from node {node}");
        };
        assert_eq!((picture.width, picture.height), (160, 90));
        // Red in BT.601 limited range: Y 81, U 90, V 240.
        let y = picture.y()[45 * 160 + 80];
        assert!(y.abs_diff(81) <= 2, "luma {y}");
        assert!(picture.v()[0] > 200, "red, not blue: V {}", picture.v()[0]);

        // The screen goes: the stream says so, and no picture comes after.
        stop.send(()).unwrap();
        source.join().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while screen.latest().is_some() {
            assert!(Instant::now() < deadline, "the stream outlived its screen");
        }
    }
}
