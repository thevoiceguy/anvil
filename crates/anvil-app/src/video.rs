//! Video calls: the cameras and the one chosen, H.264 from Cisco's
//! OpenH264 where Anvil fetches it, and each call's video as Anvil's
//! events tell it (FCP's `docs/SOFTPHONE_VIDEO.md` §2.5). The pictures
//! themselves never pass through here: the platform's video host shows
//! them.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anvil_core::video::host::Resolution;
use anvil_core::video::VideoStreamDirection;
use anvil_core::{CallId, CallMedia, Event};

use crate::state::{CameraView, Change, H264View, VideoState};
use crate::{update_call, Inner, Phone, PhoneError};

/// Cisco's OpenH264, where the platform fetches it from Cisco (Linux):
/// H.264 is offered only while the user allows it, and the notice is
/// shown wherever it is.
pub trait OpenH264: Send + Sync {
    /// What must be shown about it (Cisco's terms).
    fn notice(&self) -> String;
    /// It is here and offered.
    fn ready(&self) -> bool;
    /// Fetch it if need be, then offer H.264.
    fn enable(&self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;
    /// Offer H.264 no more (calls using it keep it).
    fn disable(&self);
}

/// The most a call sends on cellular (FCP's `docs/SOFTPHONE_VIDEO.md` §2.5).
const CELLULAR_MAX: Resolution = Resolution {
    width: 640,
    height: 360,
};

/// Why our camera is withheld (a phone's), and the calls whose camera
/// starts again when it no longer is.
#[derive(Default)]
pub(crate) struct CameraHold {
    /// The app is out of sight: the camera is not the app's there.
    background: bool,
    /// The phone is on cellular.
    cellular: bool,
    /// The user sends video only on Wi-Fi.
    wifi_only: bool,
    /// While withheld, the calls to start it in again; `None` otherwise.
    later: Option<Vec<u64>>,
}

impl CameraHold {
    fn withheld(&self) -> bool {
        self.background || (self.cellular && self.wifi_only)
    }
}

impl Phone {
    /// Read the cameras and what can be sent again: a change is sent.
    pub(crate) async fn read_video(&self) {
        let Ok(anvil) = self.anvil().await else {
            return;
        };
        let (camera, allowed, wifi_only) = {
            let local = self.inner.local.lock();
            (local.camera.clone(), local.h264, local.wifi_only)
        };
        let cellular = self.inner.camera_hold.lock().cellular;
        let video = VideoState {
            available: anvil.has_video(),
            cameras: anvil
                .cameras()
                .into_iter()
                .map(|c| CameraView {
                    id: c.id,
                    name: c.name,
                })
                .collect(),
            camera,
            h264: self.inner.openh264.as_ref().map(|h| H264View {
                allowed,
                ready: h.ready(),
                notice: h.notice(),
            }),
            wifi_only,
            cellular,
        };
        drop(anvil);
        let changed = {
            let mut state = self.inner.state.lock();
            let changed = state.video != video;
            state.video = video.clone();
            changed
        };
        if changed {
            let _ = self.inner.changes.send(Change::Video { video });
        }
    }

    /// The camera video calls use (`None`, the system's first).
    pub(crate) async fn choose_camera(&self, camera: Option<String>) -> Result<(), PhoneError> {
        let anvil = self.anvil().await?;
        if let Some(id) = &camera {
            if !anvil.cameras().iter().any(|c| &c.id == id) {
                return Err(PhoneError::Refused(format!("no camera {id}")));
            }
        }
        anvil.set_camera(camera.as_deref());
        drop(anvil);
        self.inner.local.lock().camera = camera;
        self.inner.save_settings();
        self.read_video().await;
        Ok(())
    }

    /// Allow H.264 from OpenH264 (fetched from Cisco now if need be), or not.
    pub(crate) async fn allow_h264(&self, on: bool) -> Result<(), PhoneError> {
        let h264 = self
            .inner
            .openh264
            .clone()
            .ok_or_else(|| PhoneError::Refused("this device does not fetch OpenH264".into()))?;
        self.inner.local.lock().h264 = on;
        self.inner.save_settings();
        let done = if on {
            h264.enable().await.map_err(PhoneError::Refused)
        } else {
            h264.disable();
            Ok(())
        };
        self.read_video().await;
        done
    }

    /// At start: the camera chosen, and OpenH264 fetched when allowed (in
    /// the background: calls offer VP8 until it is here).
    pub(crate) async fn start_video(&self) {
        let (camera, wifi_only) = {
            let local = self.inner.local.lock();
            (local.camera.clone(), local.wifi_only)
        };
        self.inner.camera_hold.lock().wifi_only = wifi_only;
        if let (Some(camera), Ok(anvil)) = (camera, self.anvil().await) {
            if anvil.cameras().iter().any(|c| c.id == camera) {
                anvil.set_camera(Some(&camera));
            } else {
                tracing::info!(%camera, "the camera chosen is not here; using the first");
            }
        }
        self.read_video().await;
        let allowed = self.inner.local.lock().h264;
        if let (Some(h264), true) = (self.inner.openh264.clone(), allowed) {
            let phone = self.clone();
            tokio::spawn(async move {
                if let Err(e) = h264.enable().await {
                    tracing::warn!(%e, "no OpenH264; video calls offer VP8");
                }
                phone.read_video().await;
            });
        }
    }

    /// What a call is placed or answered with: video when asked and there
    /// is any. Not while the app is in the background (a phone's camera is
    /// not its there, nor its decoders on an iPhone); received without
    /// ours on cellular with video only on Wi-Fi.
    pub(crate) async fn media_for(&self, video: bool) -> Result<CallMedia, PhoneError> {
        let (away, withheld) = {
            let hold = self.inner.camera_hold.lock();
            (hold.background, hold.withheld())
        };
        Ok(CallMedia {
            video: video && !away && self.anvil().await?.has_video(),
            receive_only: withheld,
        })
    }

    /// Whether `id` is answered with video: as asked, else when it offers
    /// some.
    pub(crate) fn answer_wants_video(&self, id: u64, video: Option<bool>) -> bool {
        let offered = self
            .inner
            .state
            .lock()
            .call(id)
            .is_some_and(|c| c.video.offered);
        video.unwrap_or(offered) && offered
    }

    /// `call` was placed or answered wanting video while our camera was
    /// withheld: it starts when it no longer is.
    pub(crate) async fn camera_later(&self, call: u64, video: bool) {
        let can = video && self.anvil().await.is_ok_and(|a| a.has_video());
        if let (true, Some(later)) = (can, self.inner.camera_hold.lock().later.as_mut()) {
            if !later.contains(&call) {
                later.push(call);
            }
        }
    }

    /// The app went to the background, or came back.
    pub(crate) async fn set_background(&self, on: bool) {
        self.inner.camera_hold.lock().background = on;
        self.hold_camera().await;
    }

    /// The phone moved between cellular and Wi-Fi (or a wire): on
    /// cellular each call sends at most 360p, and none with video only on
    /// Wi-Fi.
    pub(crate) async fn set_network(&self, cellular: bool) {
        self.inner.camera_hold.lock().cellular = cellular;
        if let Ok(anvil) = self.anvil().await {
            anvil.set_send_limit(cellular.then_some(CELLULAR_MAX));
        }
        self.hold_camera().await;
        self.read_video().await;
    }

    /// Send video only on Wi-Fi, or on cellular too.
    pub(crate) async fn set_wifi_only(&self, on: bool) {
        self.inner.local.lock().wifi_only = on;
        self.inner.save_settings();
        self.inner.camera_hold.lock().wifi_only = on;
        self.hold_camera().await;
        self.read_video().await;
    }

    /// Our camera withheld or let go, as the hold now says. Withheld,
    /// every call sending our camera stops sending it (receiving still)
    /// and is remembered; let go, each remembered call starts it again, if
    /// it is still up and not held.
    async fn hold_camera(&self) {
        let (withheld, was) = {
            let hold = self.inner.camera_hold.lock();
            (hold.withheld(), hold.later.is_some())
        };
        if withheld && !was {
            let sending: Vec<u64> = {
                let mut hold = self.inner.camera_hold.lock();
                if hold.later.is_some() {
                    return;
                }
                hold.later = Some(Vec::new());
                self.inner
                    .state
                    .lock()
                    .calls
                    .iter()
                    .filter(|c| c.video.camera)
                    .map(|c| c.id)
                    .collect()
            };
            for id in sending {
                match self.set_video(Some(id), false).await {
                    Ok(()) => self.camera_later(id, true).await,
                    Err(e) => tracing::warn!(call = id, %e, "the camera did not stop"),
                }
            }
        } else if !withheld && was {
            let Some(later) = self.inner.camera_hold.lock().later.take() else {
                return;
            };
            for id in later {
                let up = self.inner.state.lock().call(id).is_some_and(|c| {
                    c.state == crate::CallState::Connected && !c.held && !c.video.camera
                });
                if up {
                    if let Err(e) = self.set_video(Some(id), true).await {
                        tracing::warn!(call = id, %e, "the camera did not start again");
                    }
                }
            }
        }
    }

    /// Our camera on or off in `call`.
    pub(crate) async fn set_video(&self, call: Option<u64>, on: bool) -> Result<(), PhoneError> {
        let id = self.pick(call, "no call for video", |c| {
            c.state == crate::CallState::Connected && !c.held
        })?;
        self.anvil().await?.set_video(CallId(id), on).await?;
        self.update_call(
            id,
            |c| {
                c.video.camera = on;
                if on {
                    c.video.requested = false;
                }
            },
            || crate::outgoing(id),
        );
        Ok(())
    }
}

/// A video event of Anvil's, on the call it is for; false for any other.
pub(crate) fn on_event(inner: &Arc<Inner>, event: &Event) -> bool {
    let known = |id: u64| inner.state.lock().call(id).is_some();
    match event {
        Event::VideoStarted {
            call,
            stream,
            direction,
            ..
        } if known(call.0) => {
            let (stream, direction) = (stream.0, *direction);
            update_call(
                inner,
                call.0,
                |c| match direction {
                    VideoStreamDirection::Incoming => {
                        if !c.video.streams.contains(&stream) {
                            c.video.streams.push(stream);
                        }
                    }
                    VideoStreamDirection::Outgoing => {
                        c.video.sending = Some(stream);
                        c.video.camera = true;
                    }
                },
                || crate::outgoing(call.0),
            );
        }
        Event::VideoStopped { call, stream } if known(call.0) => {
            let stream = stream.0;
            update_call(
                inner,
                call.0,
                |c| {
                    c.video.streams.retain(|s| *s != stream);
                    if c.video.sending == Some(stream) {
                        c.video.sending = None;
                    }
                },
                || crate::outgoing(call.0),
            );
        }
        Event::VideoRequested { call } if known(call.0) => update_call(
            inner,
            call.0,
            |c| c.video.requested = !c.video.camera,
            || crate::outgoing(call.0),
        ),
        Event::VideoStarted { .. } | Event::VideoStopped { .. } | Event::VideoRequested { .. } => {}
        _ => return false,
    }
    true
}
