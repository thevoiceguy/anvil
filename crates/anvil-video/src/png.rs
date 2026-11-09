//! A picture written as a PNG, uncompressed (stored deflate blocks): for
//! looking at what a call showed, from the CLI and in tests.

use std::path::Path;

use crate::picture::Picture;

fn crc32(chunks: &[&[u8]]) -> u32 {
    let mut crc = !0u32;
    for chunk in chunks {
        for &b in *chunk {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
    }
    !crc
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&[kind, data]).to_be_bytes());
}

/// `picture` as PNG bytes (RGB, 8 bits).
pub fn encode(picture: &Picture) -> Vec<u8> {
    let (w, h) = (picture.width as usize, picture.height as usize);
    let mut rgba = Vec::new();
    picture.write_rgba(&mut rgba);
    // Each row: filter type 0, then the pixels.
    let mut raw = Vec::with_capacity(h * (1 + w * 3));
    for r in 0..h {
        raw.push(0);
        for px in rgba[r * w * 4..(r + 1) * w * 4].as_chunks::<4>().0 {
            raw.extend_from_slice(&px[..3]);
        }
    }
    // zlib: a header, stored blocks, Adler-32.
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (i, block) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        let len = block.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &raw {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&picture.width.to_be_bytes());
    ihdr.extend_from_slice(&picture.height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// `picture` written to `path` as a PNG.
pub fn write(path: &Path, picture: &Picture) -> std::io::Result<()> {
    std::fs::write(path, encode(picture))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_checksums_are_right() {
        // The CRC of "IEND" with no data, as every PNG ends.
        assert_eq!(crc32(&[b"IEND"]), 0xAE42_6082);
        let png = encode(&Picture::black(2, 2));
        assert!(png.starts_with(b"\x89PNG"));
        assert!(png.ends_with(&[0xAE, 0x42, 0x60, 0x82]));
    }
}
