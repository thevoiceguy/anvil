//! VP8 through libvpx: real-time, constant bit rate, no lag, error
//! resilient, a keyframe only when asked for (the far end's PLI).

use std::mem::MaybeUninit;
use std::os::raw::c_int;
use std::ptr;

#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    clippy::all
)]
mod vpx {
    include!(concat!(env!("OUT_DIR"), "/vpx.rs"));
}

use crate::codec::{Decoder, Encoded, Encoder};
use crate::picture::Picture;

fn check(what: &str, err: vpx::vpx_codec_err_t) -> Result<(), String> {
    if err == vpx::vpx_codec_err_t::VPX_CODEC_OK {
        Ok(())
    } else {
        Err(format!("libvpx {what}: {err:?}"))
    }
}

struct Context {
    ctx: vpx::vpx_codec_ctx_t,
    cfg: vpx::vpx_codec_enc_cfg_t,
    size: (u32, u32),
}

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: the context was initialised (a Context exists only then).
        unsafe {
            vpx::vpx_codec_destroy(&mut self.ctx);
        }
    }
}

// SAFETY: a libvpx context is used by one thread at a time (the encoder
// or decoder that owns it); it holds no thread-bound state.
unsafe impl Send for Context {}

pub(crate) struct Vp8Encoder {
    ctx: Option<Box<Context>>,
    kbps: u32,
    fps: u32,
    pts: i64,
}

impl Vp8Encoder {
    pub(crate) fn new(kbps: u32, fps: u32) -> Self {
        Self {
            ctx: None,
            kbps: kbps.max(30),
            fps: fps.max(1),
            pts: 0,
        }
    }

    fn make(&self, width: u32, height: u32) -> Result<Box<Context>, String> {
        // SAFETY: libvpx's documented sequence: a default configuration,
        // filled in, then an encoder initialised from it; the context is
        // boxed so it does not move after initialisation.
        unsafe {
            let iface = vpx::vpx_codec_vp8_cx();
            let mut cfg = MaybeUninit::<vpx::vpx_codec_enc_cfg_t>::zeroed();
            check(
                "default configuration",
                vpx::vpx_codec_enc_config_default(iface, cfg.as_mut_ptr(), 0),
            )?;
            let mut cfg = cfg.assume_init();
            cfg.g_w = width;
            cfg.g_h = height;
            cfg.g_timebase = vpx::vpx_rational { num: 1, den: 1000 };
            cfg.rc_target_bitrate = self.kbps;
            cfg.rc_end_usage = vpx::vpx_rc_mode::VPX_CBR;
            cfg.g_pass = vpx::vpx_enc_pass::VPX_RC_ONE_PASS;
            cfg.g_lag_in_frames = 0;
            cfg.g_error_resilient = 1;
            cfg.g_threads =
                std::thread::available_parallelism().map_or(1, |n| n.get().min(4)) as u32;
            cfg.rc_min_quantizer = 4;
            cfg.rc_max_quantizer = 56;
            cfg.rc_undershoot_pct = 100;
            cfg.rc_overshoot_pct = 15;
            cfg.rc_buf_initial_sz = 500;
            cfg.rc_buf_optimal_sz = 600;
            cfg.rc_buf_sz = 1000;
            cfg.rc_dropframe_thresh = 30;
            cfg.kf_mode = vpx::vpx_kf_mode::VPX_KF_AUTO;
            cfg.kf_max_dist = 3000;
            let mut ctx = Box::new(Context {
                ctx: MaybeUninit::zeroed().assume_init(),
                cfg,
                size: (width, height),
            });
            check(
                "encoder",
                vpx::vpx_codec_enc_init_ver(
                    &mut ctx.ctx,
                    iface,
                    &ctx.cfg,
                    0,
                    vpx::VPX_ENCODER_ABI_VERSION as c_int,
                ),
            )?;
            // Fast enough for real time on a laptop's cores.
            vpx::vpx_codec_control_(
                &mut ctx.ctx,
                vpx::vp8e_enc_control_id::VP8E_SET_CPUUSED as c_int,
                -6 as c_int,
            );
            Ok(ctx)
        }
    }
}

impl Encoder for Vp8Encoder {
    fn encode(&mut self, picture: &Picture, keyframe: bool) -> Result<Option<Encoded>, String> {
        let size = (picture.width, picture.height);
        if self.ctx.as_ref().is_none_or(|c| c.size != size) {
            self.ctx = Some(self.make(size.0, size.1)?);
        }
        let Some(ctx) = self.ctx.as_mut() else {
            return Ok(None);
        };
        let pts = self.pts;
        self.pts += i64::from(1000 / self.fps.max(1));
        let flags = if keyframe {
            vpx::VPX_EFLAG_FORCE_KF as vpx::vpx_enc_frame_flags_t
        } else {
            0
        };
        // SAFETY: the image wraps the picture's planes (I420, tightly
        // packed), which outlive the call; libvpx only reads them. Each
        // packet's buffer is copied out before the next call.
        unsafe {
            let mut img = MaybeUninit::<vpx::vpx_image_t>::zeroed();
            if vpx::vpx_img_wrap(
                img.as_mut_ptr(),
                vpx::vpx_img_fmt::VPX_IMG_FMT_I420,
                picture.width,
                picture.height,
                1,
                picture.data.as_ptr().cast_mut(),
            )
            .is_null()
            {
                return Err("libvpx: the picture".into());
            }
            let mut img = img.assume_init();
            // vpx_img_wrap assumes chroma strides of half the luma's,
            // rounded up, which is how a Picture is laid out.
            let c = picture.chroma_width() as c_int;
            img.stride = [picture.width as c_int, c, c, 0];
            let luma = (picture.width * picture.height) as usize;
            let chroma = picture.u().len();
            let base = picture.data.as_ptr().cast_mut();
            img.planes = [
                base,
                base.add(luma),
                base.add(luma + chroma),
                ptr::null_mut(),
            ];
            check(
                "encode",
                vpx::vpx_codec_encode(&mut ctx.ctx, &img, pts, 1, flags, vpx::VPX_DL_REALTIME as _),
            )?;
            let mut out: Option<Encoded> = None;
            let mut iter: vpx::vpx_codec_iter_t = ptr::null();
            loop {
                let pkt = vpx::vpx_codec_get_cx_data(&mut ctx.ctx, &mut iter);
                if pkt.is_null() {
                    break;
                }
                if (*pkt).kind != vpx::vpx_codec_cx_pkt_kind::VPX_CODEC_CX_FRAME_PKT {
                    continue;
                }
                let frame = (*pkt).data.frame;
                let bytes = std::slice::from_raw_parts(frame.buf as *const u8, frame.sz);
                let key = frame.flags & vpx::VPX_FRAME_IS_KEY != 0;
                match out.as_mut() {
                    Some(o) => {
                        o.data.extend_from_slice(bytes);
                        o.keyframe |= key;
                    }
                    None => {
                        out = Some(Encoded {
                            data: bytes.to_vec(),
                            keyframe: key,
                        })
                    }
                }
            }
            Ok(out)
        }
    }

    fn set_rate(&mut self, kbps: u32, fps: u32) {
        let (kbps, fps) = (kbps.max(30), fps.max(1));
        self.fps = fps;
        if kbps == self.kbps {
            return;
        }
        self.kbps = kbps;
        if let Some(ctx) = self.ctx.as_mut() {
            ctx.cfg.rc_target_bitrate = kbps;
            // SAFETY: an initialised encoder and its own configuration.
            let err = unsafe { vpx::vpx_codec_enc_config_set(&mut ctx.ctx, &ctx.cfg) };
            if let Err(e) = check("rate", err) {
                tracing::warn!("{e}");
            }
        }
    }
}

pub(crate) struct Vp8Decoder {
    ctx: Box<vpx::vpx_codec_ctx_t>,
}

// SAFETY: as for Context: one owner at a time.
unsafe impl Send for Vp8Decoder {}

impl Vp8Decoder {
    pub(crate) fn new() -> Result<Self, String> {
        // SAFETY: a zeroed context initialised by libvpx, boxed so it
        // does not move.
        unsafe {
            let mut ctx: Box<vpx::vpx_codec_ctx_t> = Box::new(MaybeUninit::zeroed().assume_init());
            check(
                "decoder",
                vpx::vpx_codec_dec_init_ver(
                    &mut *ctx,
                    vpx::vpx_codec_vp8_dx(),
                    ptr::null(),
                    0,
                    vpx::VPX_DECODER_ABI_VERSION as c_int,
                ),
            )?;
            Ok(Self { ctx })
        }
    }
}

impl Drop for Vp8Decoder {
    fn drop(&mut self) {
        // SAFETY: initialised in new().
        unsafe {
            vpx::vpx_codec_destroy(&mut *self.ctx);
        }
    }
}

impl Decoder for Vp8Decoder {
    fn decode(&mut self, data: &[u8]) -> Result<Option<Picture>, String> {
        // SAFETY: libvpx reads `data` during the call; the image it gives
        // back is valid until the next decode, and is copied out first.
        unsafe {
            check(
                "decode",
                vpx::vpx_codec_decode(
                    &mut *self.ctx,
                    data.as_ptr(),
                    data.len() as _,
                    ptr::null_mut(),
                    0,
                ),
            )?;
            let mut iter: vpx::vpx_codec_iter_t = ptr::null();
            let mut last = None;
            loop {
                let img = vpx::vpx_codec_get_frame(&mut *self.ctx, &mut iter);
                if img.is_null() {
                    break;
                }
                let img = &*img;
                if img.fmt != vpx::vpx_img_fmt::VPX_IMG_FMT_I420 {
                    return Err(format!("libvpx gave {:?}", img.fmt));
                }
                let (w, h) = (img.d_w, img.d_h);
                let ch = h.div_ceil(2) as usize;
                let plane = |i: usize, rows: usize| {
                    let stride = img.stride[i] as usize;
                    (
                        std::slice::from_raw_parts(img.planes[i], stride * rows),
                        stride,
                    )
                };
                last = Some(Picture::from_planes(
                    w,
                    h,
                    plane(0, h as usize),
                    plane(1, ch),
                    plane(2, ch),
                ));
            }
            Ok(last)
        }
    }
}
