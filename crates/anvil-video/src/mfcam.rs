//! Cameras on Windows, through Media Foundation: listed by their device
//! link, opened through a source reader that turns whatever the camera
//! sends (YUY2, MJPEG, NV12) into NV12 at the size nearest the one asked
//! for, and read on the source's thread.
//!
//! Windows asks nothing at the call: access is the user's setting
//! (Settings → Privacy & security → Camera), and a camera it refuses fails
//! to open, saying so.

use std::time::Instant;

use anvil_core::video::host::{CameraInfo, Resolution};
use windows::core::{GUID, PWSTR};
use windows::Win32::Media::MediaFoundation::{
    IMF2DBuffer, IMFActivate, IMFAttributes, IMFMediaSource, IMFSourceReader, MFCreateAttributes,
    MFCreateMediaType, MFCreateSourceReaderFromMediaSource, MFEnumDeviceSources, MFMediaType_Video,
    MFVideoFormat_NV12, MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
    MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
    MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK, MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE,
    MF_MT_SUBTYPE, MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READERF_ERROR,
    MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, MF_SOURCE_READER_FIRST_VIDEO_STREAM,
};
use windows::Win32::System::Com::CoTaskMemFree;

use crate::camera::{Cameras, Capture};
use crate::mf::start;
use crate::picture::Picture;

/// The PC's cameras.
#[derive(Debug, Clone, Copy, Default)]
pub struct MfCameras;

fn err(what: &str) -> impl Fn(windows::core::Error) -> String + '_ {
    move |e| format!("camera {what}: {e}")
}

const VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

/// The cameras there are, each as the activation object that opens it.
fn sources() -> Result<Vec<IMFActivate>, String> {
    start()?;
    // SAFETY: an attribute store asking for video capture devices, and the
    // array Media Foundation allocates for them, freed here.
    unsafe {
        let mut attributes: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attributes, 1).map_err(err("attributes"))?;
        let attributes = attributes.ok_or("no attributes")?;
        attributes
            .SetGUID(
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
            )
            .map_err(err("attributes"))?;
        let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count = 0u32;
        MFEnumDeviceSources(&attributes, &mut list, &mut count).map_err(err("list"))?;
        if list.is_null() {
            return Ok(Vec::new());
        }
        let found = std::slice::from_raw_parts_mut(list, count as usize)
            .iter_mut()
            .filter_map(Option::take)
            .collect();
        CoTaskMemFree(Some(list.cast()));
        Ok(found)
    }
}

fn string(a: &IMFActivate, key: &GUID) -> Option<String> {
    // SAFETY: a string Media Foundation allocates, read then freed.
    unsafe {
        let mut s = PWSTR::null();
        let mut len = 0u32;
        a.GetAllocatedString(key, &mut s, &mut len).ok()?;
        let out = s.to_string().ok();
        CoTaskMemFree(Some(s.0.cast()));
        out
    }
}

impl Cameras for MfCameras {
    fn list(&self) -> Vec<CameraInfo> {
        sources()
            .unwrap_or_default()
            .iter()
            .filter_map(|a| {
                Some(CameraInfo {
                    id: string(a, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK)?,
                    name: string(a, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME)
                        .unwrap_or_else(|| "Camera".into()),
                })
            })
            .collect()
    }

    fn open(
        &self,
        id: Option<&str>,
        want: Resolution,
        _fps: u32,
    ) -> Result<Box<dyn Capture>, String> {
        let all = sources()?;
        let activate = match id {
            Some(id) => all.into_iter().find(|a| {
                string(a, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK).as_deref()
                    == Some(id)
            }),
            None => all.into_iter().next(),
        }
        .ok_or_else(|| format!("no camera {}", id.unwrap_or("here")))?;
        // SAFETY: the camera's own activation; a reader over its source
        // that converts to NV12; its native sizes read, one chosen.
        unsafe {
            let source: IMFMediaSource = activate.ActivateObject().map_err(|e| {
                format!(
                    "the camera would not open ({e}): is it allowed in Settings → Privacy & security → Camera?"
                )
            })?;
            let mut attributes: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attributes, 1).map_err(err("attributes"))?;
            let attributes = attributes.ok_or("no attributes")?;
            attributes
                .SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)
                .map_err(err("attributes"))?;
            let reader = MfCapture::reader(&source, &attributes, want).inspect_err(|_| {
                let _ = source.Shutdown();
            })?;
            Ok(Box::new(MfCapture {
                reader: Owned(reader),
                source: Owned(source),
                activate: Owned(activate),
                size: None,
            }))
        }
    }
}

/// A COM object used from the one thread that owns its capture.
struct Owned<T>(T);

// SAFETY: made in the multithreaded apartment, whose objects may be used
// from any thread in it; each is used by one thread at a time.
unsafe impl<T> Send for Owned<T> {}

struct MfCapture {
    reader: Owned<IMFSourceReader>,
    source: Owned<IMFMediaSource>,
    activate: Owned<IMFActivate>,
    /// The size being read, once known.
    size: Option<(u32, u32)>,
}

impl Drop for MfCapture {
    fn drop(&mut self) {
        // SAFETY: ours; the camera, and its light, go off.
        unsafe {
            let _ = self.source.0.Shutdown();
            let _ = self.activate.0.ShutdownObject();
        }
    }
}

fn size_of(t: &windows::Win32::Media::MediaFoundation::IMFMediaType) -> Option<(u32, u32)> {
    // SAFETY: an attribute of a media type.
    let s = unsafe { t.GetUINT64(&MF_MT_FRAME_SIZE) }.ok()?;
    Some(((s >> 32) as u32, s as u32))
}

impl MfCapture {
    /// A reader giving NV12 at the camera's size nearest `want` (the
    /// largest not above it, else the smallest).
    unsafe fn reader(
        source: &IMFMediaSource,
        attributes: &IMFAttributes,
        want: Resolution,
    ) -> Result<IMFSourceReader, String> {
        // SAFETY: the caller's source and attributes; types read from the
        // reader and one set on it.
        unsafe {
            let reader =
                MFCreateSourceReaderFromMediaSource(source, attributes).map_err(err("reader"))?;
            let mut sizes = Vec::new();
            for i in 0.. {
                let Ok(t) = reader.GetNativeMediaType(VIDEO, i) else {
                    break;
                };
                if let Some(s) = size_of(&t) {
                    sizes.push(s);
                }
            }
            let fits = |(w, h): &(u32, u32)| *w <= want.width && *h <= want.height;
            let chosen = sizes
                .iter()
                .filter(|s| fits(s))
                .max_by_key(|(w, h)| w * h)
                .or_else(|| sizes.iter().min_by_key(|(w, h)| w * h))
                .copied();
            let t = MFCreateMediaType().map_err(err("type"))?;
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(err("type"))?;
            t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
                .map_err(err("type"))?;
            if let Some((w, h)) = chosen {
                t.SetUINT64(&MF_MT_FRAME_SIZE, (u64::from(w) << 32) | u64::from(h))
                    .map_err(err("type"))?;
            }
            reader
                .SetCurrentMediaType(VIDEO, None, &t)
                .map_err(err("NV12"))?;
            Ok(reader)
        }
    }
}

impl Capture for MfCapture {
    fn next(&mut self) -> Option<Picture> {
        let (w, h) = match self.size {
            Some(s) => s,
            None => {
                // SAFETY: the reader's own current type.
                let t = unsafe { self.reader.0.GetCurrentMediaType(VIDEO) }.ok()?;
                let s = size_of(&t)?;
                self.size = Some(s);
                s
            }
        };
        loop {
            let mut flags = 0u32;
            let mut sample = None;
            // SAFETY: a synchronous read of the video stream.
            let read = unsafe {
                self.reader
                    .0
                    .ReadSample(VIDEO, 0, None, Some(&mut flags), None, Some(&mut sample))
            };
            if let Err(e) = read {
                tracing::warn!("camera: {e}");
                return None;
            }
            if flags & (MF_SOURCE_READERF_ERROR.0 | MF_SOURCE_READERF_ENDOFSTREAM.0) as u32 != 0 {
                tracing::warn!(flags, "camera: the stream ended");
                return None;
            }
            // A gap in the stream gives no sample: read again.
            let Some(sample) = sample else { continue };
            // SAFETY: the sample's buffer, read while locked.
            let picture = unsafe {
                let Ok(buffer) = sample.ConvertToContiguousBuffer() else {
                    continue;
                };
                nv12(&buffer, w, h)
            };
            if let Some(mut p) = picture {
                p.captured = Instant::now();
                return Some(p);
            }
        }
    }
}

/// An NV12 buffer of `w` × `h` as a `Picture`: through its 2-D interface
/// for its pitch where it has one, else packed.
unsafe fn nv12(
    buffer: &windows::Win32::Media::MediaFoundation::IMFMediaBuffer,
    w: u32,
    h: u32,
) -> Option<Picture> {
    use windows::core::Interface;
    let (w, h) = (w & !1, h & !1);
    if w == 0 || h == 0 {
        return None;
    }
    let rows = h as usize;
    let chroma = rows.div_ceil(2);
    // SAFETY: the caller's buffer, locked for this read, each plane
    // within pitch × rows.
    unsafe {
        if let Ok(two) = buffer.cast::<IMF2DBuffer>() {
            let mut scan = std::ptr::null_mut();
            let mut pitch = 0i32;
            if two.Lock2D(&mut scan, &mut pitch).is_ok() {
                let picture = (pitch > 0).then(|| {
                    let pitch = pitch as usize;
                    let y = std::slice::from_raw_parts(scan, pitch * rows);
                    let uv = std::slice::from_raw_parts(scan.add(pitch * rows), pitch * chroma);
                    Picture::from_nv12(y, pitch, uv, pitch, w, h)
                });
                let _ = two.Unlock2D();
                if picture.is_some() {
                    return picture;
                }
            }
        }
        let mut at = std::ptr::null_mut();
        let mut len = 0u32;
        buffer.Lock(&mut at, None, Some(&mut len)).ok()?;
        let pitch = w as usize;
        let picture = (len as usize >= pitch * (rows + chroma)).then(|| {
            let y = std::slice::from_raw_parts(at, pitch * rows);
            let uv = std::slice::from_raw_parts(at.add(pitch * rows), pitch * chroma);
            Picture::from_nv12(y, pitch, uv, pitch, w, h)
        });
        let _ = buffer.Unlock();
        picture
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PC without a camera (CI's) lists none and opens none; one with
    /// cameras lists each with an id and a name.
    #[test]
    fn the_cameras_listed_are_the_ones_that_open() {
        let cameras = MfCameras.list();
        let want = Resolution::P720;
        if cameras.is_empty() {
            assert!(MfCameras.open(None, want, 30).is_err());
        }
        for c in &cameras {
            assert!(!c.id.is_empty() && !c.name.is_empty(), "{c:?}");
        }
        assert!(MfCameras.open(Some("no-such-camera"), want, 30).is_err());
    }
}
