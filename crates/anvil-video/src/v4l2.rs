//! Linux cameras through V4L2: every `/dev/video*` that captures YUYV,
//! NV12 or MJPEG, opened in the mode nearest what the call wants that
//! reaches its frame rate.

use std::borrow::Cow;
use std::time::Instant;

use anvil_core::video::host::{CameraInfo, Resolution};
use v4l::buffer::Type;
use v4l::frameinterval::FrameIntervalEnum;
use v4l::framesize::FrameSizeEnum;
use v4l::io::mmap::Stream;
use v4l::io::traits::CaptureStream;
use v4l::video::Capture as _;
use v4l::{Device, FourCC, Fraction};

use crate::camera::{Cameras, Capture};
use crate::picture::Picture;

const YUYV: [u8; 4] = *b"YUYV";
const NV12: [u8; 4] = *b"NV12";
const MJPG: [u8; 4] = *b"MJPG";
/// In order of preference at equal size and rate: no decoding first.
const FORMATS: [[u8; 4]; 3] = [YUYV, NV12, MJPG];

/// The cameras V4L2 has.
#[derive(Debug, Clone, Copy, Default)]
pub struct V4l2Cameras;

/// One way a camera can capture.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Mode {
    fourcc: [u8; 4],
    width: u32,
    height: u32,
    /// The highest rate at this size, frames a second.
    fps: f64,
}

fn modes(dev: &Device) -> Vec<Mode> {
    let mut out = Vec::new();
    let Ok(formats) = dev.enum_formats() else {
        return out;
    };
    for f in formats {
        let fourcc = f.fourcc.repr;
        if !FORMATS.contains(&fourcc) {
            continue;
        }
        let Ok(sizes) = dev.enum_framesizes(f.fourcc) else {
            continue;
        };
        for s in sizes {
            let dims: Vec<(u32, u32)> = match s.size {
                FrameSizeEnum::Discrete(d) => vec![(d.width, d.height)],
                FrameSizeEnum::Stepwise(st) => [(640, 360), (640, 480), (1280, 720)]
                    .into_iter()
                    .filter(|&(w, h)| {
                        (st.min_width..=st.max_width).contains(&w)
                            && (st.min_height..=st.max_height).contains(&h)
                    })
                    .collect(),
            };
            for (width, height) in dims {
                let fps = dev
                    .enum_frameintervals(f.fourcc, width, height)
                    .unwrap_or_default()
                    .iter()
                    .map(|i| match &i.interval {
                        FrameIntervalEnum::Discrete(fr) => rate(fr),
                        FrameIntervalEnum::Stepwise(st) => rate(&st.min),
                    })
                    .fold(0.0, f64::max);
                out.push(Mode {
                    fourcc,
                    width,
                    height,
                    fps,
                });
            }
        }
    }
    out
}

fn rate(interval: &Fraction) -> f64 {
    f64::from(interval.denominator) / f64::from(interval.numerator.max(1))
}

/// The mode for a call: one that reaches the frame rate if any does;
/// then the largest within `want` (or the smallest above it); then the
/// format needing least work.
fn choose(modes: &[Mode], want: Resolution, fps: u32) -> Option<Mode> {
    let fits = |m: &Mode| m.width <= want.width && m.height <= want.height;
    modes.iter().copied().max_by(|a, b| {
        let key = |m: &Mode| {
            let area = i64::from(m.width) * i64::from(m.height);
            (
                m.fps + 0.5 >= f64::from(fps),
                fits(m),
                if fits(m) { area } else { -area },
                -(FORMATS.iter().position(|f| *f == m.fourcc).unwrap_or(9) as i64),
            )
        };
        key(a)
            .partial_cmp(&key(b))
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}

impl Cameras for V4l2Cameras {
    fn list(&self) -> Vec<CameraInfo> {
        let mut nodes = v4l::context::enum_devices();
        nodes.sort_by_key(|n| n.index());
        nodes
            .into_iter()
            .filter_map(|node| {
                let dev = Device::with_path(node.path()).ok()?;
                // Metadata nodes of the same camera capture none of ours.
                if modes(&dev).is_empty() {
                    return None;
                }
                let name = dev
                    .query_caps()
                    .map(|c| c.card)
                    .ok()
                    .or_else(|| node.name())
                    .unwrap_or_else(|| node.path().display().to_string());
                Some(CameraInfo {
                    id: node.path().display().to_string(),
                    name,
                })
            })
            .collect()
    }

    fn open(
        &self,
        id: Option<&str>,
        want: Resolution,
        fps: u32,
    ) -> Result<Box<dyn Capture>, String> {
        let path = match id {
            Some(id) => id.to_owned(),
            None => self.list().first().ok_or("no camera")?.id.clone(),
        };
        let dev = Device::with_path(&path).map_err(|e| format!("{path}: {e}"))?;
        let mode =
            choose(&modes(&dev), want, fps).ok_or_else(|| format!("{path}: no usable mode"))?;
        let mut format = dev.format().map_err(|e| format!("{path}: {e}"))?;
        format.width = mode.width;
        format.height = mode.height;
        format.fourcc = FourCC::new(&mode.fourcc);
        let format = dev
            .set_format(&format)
            .map_err(|e| format!("{path}: {e}"))?;
        let mut params = dev.params().map_err(|e| format!("{path}: {e}"))?;
        params.interval = Fraction::new(1, fps.clamp(1, 60));
        if let Err(e) = dev.set_params(&params) {
            tracing::debug!(path, "frame rate not set: {e}");
        }
        let stream = Stream::with_buffers(&dev, Type::VideoCapture, 4)
            .map_err(|e| format!("{path}: {e}"))?;
        tracing::info!(
            path,
            fourcc = %String::from_utf8_lossy(&format.fourcc.repr),
            width = format.width,
            height = format.height,
            "camera open"
        );
        Ok(Box::new(V4l2Capture {
            stream,
            fourcc: format.fourcc.repr,
            width: format.width,
            height: format.height,
            stride: format.stride.max(format.width * 2) as usize,
            _dev: dev,
        }))
    }
}

struct V4l2Capture {
    stream: Stream<'static>,
    fourcc: [u8; 4],
    width: u32,
    height: u32,
    stride: usize,
    _dev: Device,
}

impl Capture for V4l2Capture {
    fn next(&mut self) -> Option<Picture> {
        loop {
            let (buf, meta) = match self.stream.next() {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!("camera: {e}");
                    return None;
                }
            };
            let data = &buf[..(meta.bytesused as usize).min(buf.len())];
            let picture = match self.fourcc {
                YUYV if data.len() >= self.stride * self.height as usize => Some(
                    Picture::from_yuyv(data, self.stride, self.width, self.height),
                ),
                NV12 => {
                    let ys = self.width as usize;
                    let luma = ys * self.height as usize;
                    (data.len() >= luma + luma / 2).then(|| {
                        Picture::from_nv12(
                            &data[..luma],
                            ys,
                            &data[luma..],
                            ys,
                            self.width,
                            self.height,
                        )
                    })
                }
                MJPG => decode_mjpeg(data),
                _ => None,
            };
            // A torn or empty frame is skipped, not the end of the camera.
            if let Some(mut p) = picture {
                p.captured = Instant::now();
                return Some(p);
            }
        }
    }
}

/// A webcam's MJPEG frame: often without Huffman tables, which every
/// such frame shares (the JPEG standard's, Annex K), so they are put in.
pub(crate) fn decode_mjpeg(data: &[u8]) -> Option<Picture> {
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;

    let jpeg = with_huffman_tables(data)?;
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::YCbCr);
    let mut decoder =
        zune_jpeg::JpegDecoder::new_with_options(std::io::Cursor::new(&jpeg[..]), options);
    let pixels = decoder.decode().ok()?;
    let (w, h) = decoder.dimensions()?;
    if pixels.len() < w * h * 3 {
        return None;
    }
    Some(Picture::from_full_range_ycbcr(&pixels, w as u32, h as u32))
}

/// `data` with the standard Huffman tables before its scan, if it has
/// none of its own; `None` if it is not a JPEG.
fn with_huffman_tables(data: &[u8]) -> Option<Cow<'_, [u8]>> {
    if data.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut i = 2;
    while i + 4 <= data.len() {
        if data[i] != 0xFF {
            return None;
        }
        let marker = data[i + 1];
        if marker == 0xC4 {
            return Some(Cow::Borrowed(data));
        }
        if marker == 0xDA {
            let mut out = Vec::with_capacity(data.len() + 432);
            out.extend_from_slice(&data[..i]);
            out.extend_from_slice(&standard_dht());
            out.extend_from_slice(&data[i..]);
            return Some(Cow::Owned(out));
        }
        let len = usize::from(u16::from_be_bytes([data[i + 2], data[i + 3]]));
        i += 2 + len;
    }
    None
}

/// A DHT segment with the four tables of the JPEG standard, Annex K.3.
fn standard_dht() -> Vec<u8> {
    const DC_LUMA: [u8; 16] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
    const DC_CHROMA: [u8; 16] = [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0];
    const AC_LUMA: [u8; 16] = [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7d];
    const AC_CHROMA: [u8; 16] = [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 0x77];
    const AC_LUMA_VALUES: [u8; 162] = [
        0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61,
        0x07, 0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xa1, 0x08, 0x23, 0x42, 0xb1, 0xc1, 0x15, 0x52,
        0xd1, 0xf0, 0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0a, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x25,
        0x26, 0x27, 0x28, 0x29, 0x2a, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45,
        0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64,
        0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x83,
        0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99,
        0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6,
        0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3,
        0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8,
        0xe9, 0xea, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
    ];
    const AC_CHROMA_VALUES: [u8; 162] = [
        0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61,
        0x71, 0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91, 0xa1, 0xb1, 0xc1, 0x09, 0x23, 0x33,
        0x52, 0xf0, 0x15, 0x62, 0x72, 0xd1, 0x0a, 0x16, 0x24, 0x34, 0xe1, 0x25, 0xf1, 0x17, 0x18,
        0x19, 0x1a, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44,
        0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63,
        0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a,
        0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97,
        0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4,
        0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca,
        0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7,
        0xe8, 0xe9, 0xea, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
    ];
    let dc_values: Vec<u8> = (0..12).collect();
    let tables: [(u8, &[u8; 16], &[u8]); 4] = [
        (0x00, &DC_LUMA, &dc_values),
        (0x10, &AC_LUMA, &AC_LUMA_VALUES),
        (0x01, &DC_CHROMA, &dc_values),
        (0x11, &AC_CHROMA, &AC_CHROMA_VALUES),
    ];
    let body: usize = tables.iter().map(|(_, _, v)| 17 + v.len()).sum();
    let mut out = vec![0xFF, 0xC4];
    out.extend_from_slice(&((body + 2) as u16).to_be_bytes());
    for (class_id, bits, values) in tables {
        out.push(class_id);
        out.extend_from_slice(bits);
        out.extend_from_slice(values);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(fourcc: [u8; 4], width: u32, height: u32, fps: f64) -> Mode {
        Mode {
            fourcc,
            width,
            height,
            fps,
        }
    }

    #[test]
    fn the_mode_reaches_the_rate_first_then_the_size() {
        // A typical USB 2 webcam: uncompressed 720p only at 10 fps.
        let cam = [
            mode(YUYV, 640, 480, 30.0),
            mode(YUYV, 1280, 720, 10.0),
            mode(MJPG, 640, 480, 30.0),
            mode(MJPG, 1280, 720, 30.0),
            mode(MJPG, 1920, 1080, 30.0),
        ];
        assert_eq!(choose(&cam, Resolution::P720, 30), Some(cam[3]));
        let small = Resolution {
            width: 640,
            height: 480,
        };
        assert_eq!(
            choose(&cam, small, 30),
            Some(cam[0]),
            "no decoding at equal size"
        );
        // Only larger than wanted: the smallest of those.
        let tiny = Resolution {
            width: 320,
            height: 240,
        };
        assert_eq!(choose(&cam[2..], tiny, 30), Some(cam[2]));
    }

    #[test]
    fn the_standard_tables_are_well_formed_and_put_before_the_scan() {
        let dht = standard_dht();
        assert_eq!(dht.len(), 2 + 2 + (17 + 12) * 2 + (17 + 162) * 2);
        assert_eq!(
            usize::from(u16::from_be_bytes([dht[2], dht[3]])),
            dht.len() - 2
        );
        // SOI, an APP0 of two bytes' length, then SOS.
        let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x02, 0xFF, 0xDA, 0x00, 0x02];
        let fixed = with_huffman_tables(&jpeg).unwrap();
        assert_eq!(&fixed[6..8], &[0xFF, 0xC4]);
        assert_eq!(&fixed[6 + dht.len()..6 + dht.len() + 2], &[0xFF, 0xDA]);
        assert!(with_huffman_tables(b"not a jpeg").is_none());
    }
}
