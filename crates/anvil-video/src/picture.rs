//! A picture: what a camera captures, an encoder takes and a decoder
//! gives, as 8-bit I420 (BT.601, limited range) in one buffer.

use std::time::Instant;

use anvil_core::video::host::Resolution;

/// An I420 picture: the Y plane, then U, then V, each tightly packed.
#[derive(Clone, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    /// When it was captured (ours).
    pub captured: Instant,
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Picture({}x{})", self.width, self.height)
    }
}

/// The size of a chroma plane for a picture `w` × `h`.
fn chroma(w: u32, h: u32) -> (usize, usize) {
    (w.div_ceil(2) as usize, h.div_ceil(2) as usize)
}

impl Picture {
    /// A black picture.
    pub fn black(width: u32, height: u32) -> Self {
        let luma = (width * height) as usize;
        let (cw, ch) = chroma(width, height);
        let mut data = vec![16u8; luma + 2 * cw * ch];
        data[luma..].fill(128);
        Self {
            width,
            height,
            data,
            captured: Instant::now(),
        }
    }

    fn luma_len(&self) -> usize {
        (self.width * self.height) as usize
    }

    fn chroma_len(&self) -> usize {
        let (cw, ch) = chroma(self.width, self.height);
        cw * ch
    }

    pub fn chroma_width(&self) -> usize {
        chroma(self.width, self.height).0
    }

    pub fn y(&self) -> &[u8] {
        &self.data[..self.luma_len()]
    }

    pub fn u(&self) -> &[u8] {
        let l = self.luma_len();
        &self.data[l..l + self.chroma_len()]
    }

    pub fn v(&self) -> &[u8] {
        let l = self.luma_len() + self.chroma_len();
        &self.data[l..l + self.chroma_len()]
    }

    /// The three planes, to write.
    pub fn planes_mut(&mut self) -> (&mut [u8], &mut [u8], &mut [u8]) {
        let (l, c) = (self.luma_len(), self.chroma_len());
        let (y, rest) = self.data.split_at_mut(l);
        let (u, v) = rest.split_at_mut(c);
        (y, u, &mut v[..c])
    }

    /// From three planes with strides (a decoder's output).
    pub fn from_planes(
        width: u32,
        height: u32,
        (y, ys): (&[u8], usize),
        (u, us): (&[u8], usize),
        (v, vs): (&[u8], usize),
    ) -> Self {
        let mut p = Self::black(width, height);
        let (w, h) = (width as usize, height as usize);
        let (cw, ch) = chroma(width, height);
        let (py, pu, pv) = p.planes_mut();
        for r in 0..h {
            py[r * w..(r + 1) * w].copy_from_slice(&y[r * ys..r * ys + w]);
        }
        for r in 0..ch {
            pu[r * cw..(r + 1) * cw].copy_from_slice(&u[r * us..r * us + cw]);
            pv[r * cw..(r + 1) * cw].copy_from_slice(&v[r * vs..r * vs + cw]);
        }
        p
    }

    /// From packed YUYV (4:2:2), the format most webcams give uncompressed.
    pub fn from_yuyv(src: &[u8], stride: usize, width: u32, height: u32) -> Self {
        let mut p = Self::black(width, height);
        let (w, h) = (width as usize, height as usize);
        let cw = p.chroma_width();
        let (py, pu, pv) = p.planes_mut();
        for r in 0..h {
            let row = &src[r * stride..];
            for x in 0..w {
                py[r * w + x] = row[x * 2];
            }
            // Chroma from the even rows (4:2:2 to 4:2:0).
            if r % 2 == 0 {
                for cx in 0..cw {
                    let i = cx * 4;
                    pu[(r / 2) * cw + cx] = row.get(i + 1).copied().unwrap_or(128);
                    pv[(r / 2) * cw + cx] = row.get(i + 3).copied().unwrap_or(128);
                }
            }
        }
        p
    }

    /// From NV12 (a Y plane, then interleaved U and V).
    pub fn from_nv12(y: &[u8], ys: usize, uv: &[u8], uvs: usize, width: u32, height: u32) -> Self {
        let mut p = Self::black(width, height);
        let (w, h) = (width as usize, height as usize);
        let (cw, ch) = chroma(width, height);
        let (py, pu, pv) = p.planes_mut();
        for r in 0..h {
            py[r * w..(r + 1) * w].copy_from_slice(&y[r * ys..r * ys + w]);
        }
        for r in 0..ch {
            for cx in 0..cw {
                pu[r * cw + cx] = uv[r * uvs + cx * 2];
                pv[r * cw + cx] = uv[r * uvs + cx * 2 + 1];
            }
        }
        p
    }

    /// From Android's YUV_420_888: three planes, each with a row stride,
    /// and the chroma with a pixel stride too (1 is planar, 2 interleaved).
    /// Only `width` × `height` from the top left is read (a crop).
    pub fn from_yuv_420_888(
        width: u32,
        height: u32,
        (y, ys): (&[u8], usize),
        (u, v): (&[u8], &[u8]),
        (uvs, step): (usize, usize),
    ) -> Self {
        let mut p = Self::black(width, height);
        let (w, h) = (width as usize, height as usize);
        let (cw, ch) = chroma(width, height);
        let (py, pu, pv) = p.planes_mut();
        for r in 0..h {
            py[r * w..(r + 1) * w].copy_from_slice(&y[r * ys..r * ys + w]);
        }
        for r in 0..ch {
            for cx in 0..cw {
                let i = r * uvs + cx * step;
                pu[r * cw + cx] = u.get(i).copied().unwrap_or(128);
                pv[r * cw + cx] = v.get(i).copied().unwrap_or(128);
            }
        }
        p
    }

    /// Turned clockwise by `degrees` (0, 90, 180 or 270; others as the
    /// nearest below): a phone's sensor lies on its side.
    pub fn rotated(&self, degrees: u32) -> Picture {
        let quarter = (degrees / 90) % 4;
        if quarter == 0 {
            return self.clone();
        }
        let (w, h) = (self.width, self.height);
        let (nw, nh) = if quarter == 2 { (w, h) } else { (h, w) };
        let mut p = Self::black(nw, nh);
        p.captured = self.captured;
        let (cw, ch) = chroma(w, h);
        let (ncw, _) = chroma(nw, nh);
        let (sy, su, sv) = (self.y(), self.u(), self.v());
        let (dy, du, dv) = p.planes_mut();
        // Where the source pixel (x, r) of a plane w × h lands.
        let turn = |x: usize, r: usize, w: usize, h: usize| match quarter {
            1 => (h - 1 - r, x),
            2 => (w - 1 - x, h - 1 - r),
            _ => (r, w - 1 - x),
        };
        let (w, h, nw) = (w as usize, h as usize, nw as usize);
        for r in 0..h {
            for x in 0..w {
                let (nx, nr) = turn(x, r, w, h);
                dy[nr * nw + nx] = sy[r * w + x];
            }
        }
        for r in 0..ch {
            for x in 0..cw {
                let (nx, nr) = turn(x, r, cw, ch);
                du[nr * ncw + nx] = su[r * cw + x];
                dv[nr * ncw + nx] = sv[r * cw + x];
            }
        }
        p
    }

    /// From full-range 4:4:4 YCbCr, interleaved (a JPEG's colours): the
    /// range squeezed to video's, the chroma averaged over each 2×2.
    pub fn from_full_range_ycbcr(src: &[u8], width: u32, height: u32) -> Self {
        let mut p = Self::black(width, height);
        let (w, h) = (width as usize, height as usize);
        let (cw, ch) = chroma(width, height);
        let luma = |v: u8| (16 + (u32::from(v) * 219 + 127) / 255) as u8;
        let chroma_v = |sum: u32, n: u32| {
            let c = (sum + n / 2) / n;
            (128 + ((c as i32 - 128) * 224) / 255) as u8
        };
        let (py, pu, pv) = p.planes_mut();
        for r in 0..h {
            for x in 0..w {
                py[r * w + x] = luma(src[(r * w + x) * 3]);
            }
        }
        for cr in 0..ch {
            for cx in 0..cw {
                let (mut su, mut sv, mut n) = (0u32, 0u32, 0u32);
                for dy in 0..2 {
                    for dx in 0..2 {
                        let (y, x) = (cr * 2 + dy, cx * 2 + dx);
                        if y < h && x < w {
                            let i = (y * w + x) * 3;
                            su += u32::from(src[i + 1]);
                            sv += u32::from(src[i + 2]);
                            n += 1;
                        }
                    }
                }
                pu[cr * cw + cx] = chroma_v(su, n);
                pv[cr * cw + cx] = chroma_v(sv, n);
            }
        }
        p
    }

    /// The largest size within `max` with this picture's shape, in even
    /// numbers, never larger than the picture. A limit is a size either
    /// way up: an upright picture (a phone's) fits it turned.
    pub fn fit(&self, max: Resolution) -> (u32, u32) {
        let (w, h) = (self.width.max(1), self.height.max(1));
        let (mw, mh) = if (h > w) == (max.height > max.width) {
            (max.width, max.height)
        } else {
            (max.height, max.width)
        };
        let scale = (f64::from(mw) / f64::from(w))
            .min(f64::from(mh) / f64::from(h))
            .min(1.0);
        let even = |v: f64| ((v as u32) & !1).max(2);
        (even(f64::from(w) * scale), even(f64::from(h) * scale))
    }

    /// Scaled to `width` × `height` (bilinear).
    pub fn scaled(&self, width: u32, height: u32) -> Picture {
        if (width, height) == (self.width, self.height) {
            return self.clone();
        }
        let mut p = Self::black(width, height);
        p.captured = self.captured;
        let (scw, sch) = chroma(self.width, self.height);
        let (dcw, dch) = chroma(width, height);
        let (sy, su, sv) = (self.y(), self.u(), self.v());
        let (dy, du, dv) = p.planes_mut();
        scale_plane(
            sy,
            self.width as usize,
            self.height as usize,
            dy,
            width as usize,
            height as usize,
        );
        scale_plane(su, scw, sch, du, dcw, dch);
        scale_plane(sv, scw, sch, dv, dcw, dch);
        p
    }

    /// From 32-bit pixels in B, G, R order (the fourth byte ignored): a
    /// screen as X11, Windows and Core Graphics give one. `stride` is a
    /// row's length in bytes; each chroma sample averages its 2×2 block.
    pub fn from_bgrx(src: &[u8], stride: usize, width: u32, height: u32) -> Self {
        let mut p = Self::black(width, height);
        let (w, h) = (width as usize, height as usize);
        let cw = p.chroma_width();
        let (py, pu, pv) = p.planes_mut();
        let rgb = |x: usize, r: usize| {
            let px = &src[r * stride + x * 4..r * stride + x * 4 + 3];
            (i32::from(px[2]), i32::from(px[1]), i32::from(px[0]))
        };
        // BT.601, limited range, in 16.16 fixed point.
        for r in 0..h {
            for x in 0..w {
                let (rr, gg, bb) = rgb(x, r);
                py[r * w + x] =
                    (((16 << 16) + 16_829 * rr + 33_039 * gg + 6_416 * bb + 32_768) >> 16) as u8;
            }
        }
        for r in (0..h).step_by(2) {
            for x in (0..w).step_by(2) {
                let (mut sr, mut sg, mut sb, mut n) = (0, 0, 0, 0);
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    if x + dx < w && r + dy < h {
                        let (rr, gg, bb) = rgb(x + dx, r + dy);
                        (sr, sg, sb, n) = (sr + rr, sg + gg, sb + bb, n + 1);
                    }
                }
                let (rr, gg, bb) = (sr / n, sg / n, sb / n);
                let c = (r / 2) * cw + x / 2;
                pu[c] = (((128 << 16) - 9_714 * rr - 19_070 * gg + 28_784 * bb + 32_768) >> 16)
                    .clamp(16, 240) as u8;
                pv[c] = (((128 << 16) + 28_784 * rr - 24_103 * gg - 4_681 * bb + 32_768) >> 16)
                    .clamp(16, 240) as u8;
            }
        }
        p
    }

    /// RGBA, 4 bytes a pixel, rows `width * 4` long.
    pub fn write_rgba(&self, out: &mut Vec<u8>) {
        self.write_rgb32(out, [0, 1, 2]);
    }

    /// BGRA, 4 bytes a pixel (Windows' and Core Video's order).
    pub fn write_bgra(&self, out: &mut Vec<u8>) {
        self.write_rgb32(out, [2, 1, 0]);
    }

    fn write_rgb32(&self, out: &mut Vec<u8>, order: [usize; 3]) {
        let (w, h) = (self.width as usize, self.height as usize);
        let cw = self.chroma_width();
        out.clear();
        out.resize(w * h * 4, 255);
        let (y, u, v) = (self.y(), self.u(), self.v());
        for r in 0..h {
            let row = &mut out[r * w * 4..(r + 1) * w * 4];
            for x in 0..w {
                let c = (r / 2) * cw + x / 2;
                // BT.601, limited range, in 16.16 fixed point.
                let yy = (i32::from(y[r * w + x]) - 16) * 76_309;
                let uu = i32::from(u[c]) - 128;
                let vv = i32::from(v[c]) - 128;
                let rgb = [
                    yy + 104_597 * vv,
                    yy - 25_675 * uu - 53_279 * vv,
                    yy + 132_201 * uu,
                ];
                let px = &mut row[x * 4..x * 4 + 4];
                for (i, &ch) in order.iter().enumerate() {
                    px[i] = ((rgb[ch] + 32_768) >> 16).clamp(0, 255) as u8;
                }
            }
        }
    }

    /// A moving test picture for frame `n`: colour bars that slide, and a
    /// block whose position counts the frames, so a picture that froze or
    /// arrived out of order shows.
    pub fn pattern(width: u32, height: u32, n: u32) -> Picture {
        const BARS: [(u8, u8, u8); 8] = [
            (235, 128, 128),
            (210, 16, 146),
            (170, 166, 16),
            (145, 54, 34),
            (106, 202, 222),
            (81, 90, 240),
            (41, 240, 110),
            (16, 128, 128),
        ];
        let mut p = Self::black(width, height);
        let (w, h) = (width as usize, height as usize);
        let cw = p.chroma_width();
        let shift = (n as usize * 4) % w.max(1);
        let bar = |x: usize| BARS[((x + shift) * BARS.len() / w.max(1)) % BARS.len()];
        let (bx, by) = (
            (n as usize * 8) % w.saturating_sub(32).max(1),
            h.saturating_sub(48),
        );
        let (py, pu, pv) = p.planes_mut();
        for r in 0..h {
            for x in 0..w {
                let in_block = (bx..bx + 32).contains(&x) && (by..by + 32).contains(&r);
                py[r * w + x] = if in_block { 235 } else { bar(x).0 };
                if r % 2 == 0 && x % 2 == 0 {
                    let (_, cu, cv) = if in_block { (0, 128, 128) } else { bar(x) };
                    pu[(r / 2) * cw + x / 2] = cu;
                    pv[(r / 2) * cw + x / 2] = cv;
                }
            }
        }
        p
    }

    /// The peak signal-to-noise ratio of the Y plane against `other`, in
    /// dB (how close a decoded picture is to what was sent).
    pub fn psnr(&self, other: &Picture) -> f64 {
        if (self.width, self.height) != (other.width, other.height) {
            return 0.0;
        }
        let mse = self
            .y()
            .iter()
            .zip(other.y())
            .map(|(a, b)| {
                let d = f64::from(*a) - f64::from(*b);
                d * d
            })
            .sum::<f64>()
            / self.y().len().max(1) as f64;
        if mse == 0.0 {
            return 99.0;
        }
        10.0 * (255.0 * 255.0 / mse).log10()
    }
}

fn scale_plane(src: &[u8], sw: usize, sh: usize, dst: &mut [u8], dw: usize, dh: usize) {
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return;
    }
    // Bilinear, sampled at each destination pixel's centre (16.16).
    let fx = (sw << 16) / dw;
    let fy = (sh << 16) / dh;
    for r in 0..dh {
        let syf = ((r * fy) + fy / 2).saturating_sub(1 << 15);
        let y0 = (syf >> 16).min(sh - 1);
        let y1 = (y0 + 1).min(sh - 1);
        let wy = (syf & 0xFFFF) as u32;
        for x in 0..dw {
            let sxf = ((x * fx) + fx / 2).saturating_sub(1 << 15);
            let x0 = (sxf >> 16).min(sw - 1);
            let x1 = (x0 + 1).min(sw - 1);
            let wx = (sxf & 0xFFFF) as u32;
            let p = |yy: usize, xx: usize| u32::from(src[yy * sw + xx]);
            let top = p(y0, x0) * (65_536 - wx) + p(y0, x1) * wx;
            let bot = p(y1, x0) * (65_536 - wx) + p(y1, x1) * wx;
            let v = ((u64::from(top) * u64::from(65_536 - wy) + u64::from(bot) * u64::from(wy))
                + (1 << 31))
                >> 32;
            dst[r * dw + x] = v as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_screens_pixels_come_back_as_they_went() {
        // A picture to BGRA and back, as a screen gives it: the same
        // picture, within rounding (and chroma averaged per 2×2 block).
        let p = Picture::pattern(320, 180, 7);
        let mut bgra = Vec::new();
        p.write_bgra(&mut bgra);
        // A row padded as an X server or a GPU pads one.
        let stride = 320 * 4 + 64;
        let mut padded = vec![0u8; stride * 180];
        for r in 0..180 {
            padded[r * stride..r * stride + 320 * 4]
                .copy_from_slice(&bgra[r * 320 * 4..(r + 1) * 320 * 4]);
        }
        let back = Picture::from_bgrx(&padded, stride, 320, 180);
        assert_eq!((back.width, back.height), (320, 180));
        let psnr = back.psnr(&p);
        assert!(psnr > 40.0, "{psnr} dB");
        let (u, v) = (p.u().to_vec(), p.v().to_vec());
        let near = |a: &[u8], b: &[u8]| a.iter().zip(b).all(|(x, y)| x.abs_diff(*y) <= 3);
        assert!(near(back.u(), &u) && near(back.v(), &v), "chroma");
        // An odd size has a chroma sample for its last column and row.
        let odd = Picture::from_bgrx(&[255u8; 5 * 3 * 4], 5 * 4, 5, 3);
        assert!(odd.y().iter().all(|&y| y == 235), "white is 235");
    }

    #[test]
    fn colours_survive_the_trip_to_rgba() {
        // Limited-range white, black and the red bar.
        let mut p = Picture::black(2, 2);
        p.planes_mut().0.fill(235);
        let mut out = Vec::new();
        p.write_rgba(&mut out);
        assert_eq!(&out[..4], &[255, 255, 255, 255]);
        Picture::black(2, 2).write_rgba(&mut out);
        assert_eq!(&out[..4], &[0, 0, 0, 255]);
        let mut red = Picture::black(2, 2);
        let (y, u, v) = red.planes_mut();
        y.fill(81);
        u.fill(90);
        v.fill(240);
        red.write_bgra(&mut out);
        let (b, g, r) = (out[0], out[1], out[2]);
        assert!(r > 250 && g < 5 && b < 5, "{r} {g} {b}");
    }

    #[test]
    fn yuyv_and_nv12_become_the_same_picture() {
        let src = Picture::pattern(64, 32, 3);
        // As YUYV: chroma doubled vertically.
        let mut yuyv = vec![0u8; 64 * 2 * 32];
        let cw = src.chroma_width();
        for r in 0..32 {
            for x in 0..64 {
                yuyv[r * 128 + x * 2] = src.y()[r * 64 + x];
            }
            for cx in 0..cw {
                yuyv[r * 128 + cx * 4 + 1] = src.u()[(r / 2) * cw + cx];
                yuyv[r * 128 + cx * 4 + 3] = src.v()[(r / 2) * cw + cx];
            }
        }
        assert_eq!(Picture::from_yuyv(&yuyv, 128, 64, 32).data, src.data);
        let mut uv = vec![0u8; 64 * 16];
        for i in 0..src.u().len() {
            uv[i * 2] = src.u()[i];
            uv[i * 2 + 1] = src.v()[i];
        }
        assert_eq!(
            Picture::from_nv12(src.y(), 64, &uv, 64, 64, 32).data,
            src.data
        );
    }

    #[test]
    fn android_planes_planar_or_interleaved_become_the_same_picture() {
        let src = Picture::pattern(64, 32, 5);
        let cw = src.chroma_width();
        // Planar, rows padded to 80 (luma) and 40 (chroma).
        let mut y = vec![0u8; 80 * 32];
        for r in 0..32 {
            y[r * 80..r * 80 + 64].copy_from_slice(&src.y()[r * 64..r * 64 + 64]);
        }
        let (mut u, mut v) = (vec![0u8; 40 * 16], vec![0u8; 40 * 16]);
        for r in 0..16 {
            u[r * 40..r * 40 + cw].copy_from_slice(&src.u()[r * cw..(r + 1) * cw]);
            v[r * 40..r * 40 + cw].copy_from_slice(&src.v()[r * cw..(r + 1) * cw]);
        }
        let planar = Picture::from_yuv_420_888(64, 32, (&y, 80), (&u, &v), (40, 1));
        assert_eq!(planar.data, src.data);
        // Interleaved (NV12 as Android hands it: V one byte after U).
        let mut uv = vec![0u8; 80 * 16];
        for r in 0..16 {
            for cx in 0..cw {
                uv[r * 80 + cx * 2] = src.u()[r * cw + cx];
                uv[r * 80 + cx * 2 + 1] = src.v()[r * cw + cx];
            }
        }
        let semi = Picture::from_yuv_420_888(64, 32, (&y, 80), (&uv, &uv[1..]), (80, 2));
        assert_eq!(semi.data, src.data);
    }

    #[test]
    fn a_picture_turns_a_quarter_at_a_time() {
        let src = Picture::pattern(64, 32, 2);
        let right = src.rotated(90);
        assert_eq!((right.width, right.height), (32, 64));
        // The top left goes to the top right.
        assert_eq!(right.y()[31], src.y()[0]);
        assert_eq!(right.u()[15], src.u()[0]);
        // The bottom left goes to the top left.
        assert_eq!(right.y()[0], src.y()[31 * 64]);
        assert_eq!(src.rotated(180).y()[0], src.y()[32 * 64 - 1]);
        assert_eq!(src.rotated(270).y()[0], src.y()[63]);
        // Four turns, or two halves, are where it started.
        assert_eq!(
            right.rotated(90).rotated(180).data,
            src.rotated(270).rotated(90).data
        );
        assert_eq!(src.rotated(180).rotated(180).data, src.data);
        assert_eq!(right.rotated(270).data, src.data);
        assert_eq!(src.rotated(0).data, src.data);
    }

    #[test]
    fn a_picture_fits_its_limit_keeping_its_shape() {
        let p = Picture::black(1280, 720);
        assert_eq!(
            p.fit(Resolution {
                width: 640,
                height: 640
            }),
            (640, 360)
        );
        assert_eq!(p.fit(Resolution::P720), (1280, 720));
        assert_eq!(
            p.fit(Resolution {
                width: 1920,
                height: 1080
            }),
            (1280, 720),
            "never upscaled"
        );
        assert_eq!(
            Picture::black(720, 1280).fit(Resolution {
                width: 640,
                height: 360
            }),
            (360, 640),
            "upright"
        );
        let half = Picture::pattern(1280, 720, 0).scaled(640, 360);
        assert_eq!((half.width, half.height), (640, 360));
        let back = half.scaled(1280, 720);
        assert!(back.psnr(&Picture::pattern(1280, 720, 0)) > 20.0);
    }
}
