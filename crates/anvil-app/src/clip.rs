//! A voicemail message's audio as FCP serves it (WAV), made mono PCM to
//! play: 8-, 16-, 24- and 32-bit integer PCM, 32-bit float, A-law and μ-law,
//! any channel count mixed down.

/// Mono 16-bit samples and their rate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clip {
    pub samples: Vec<i16>,
    pub sample_rate: u32,
}

/// Decode a WAV file.
pub fn decode_wav(bytes: &[u8]) -> Result<Clip, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a WAV file".into());
    }
    let mut format: Option<(u16, u16, u32, u16)> = None;
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = &bytes[at + 8..(at + 8 + len).min(bytes.len())];
        match id {
            b"fmt " => {
                if body.len() < 16 {
                    return Err("a WAV file with a short format chunk".into());
                }
                let mut tag = u16::from_le_bytes([body[0], body[1]]);
                let channels = u16::from_le_bytes([body[2], body[3]]);
                let rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
                let bits = u16::from_le_bytes([body[14], body[15]]);
                // WAVE_FORMAT_EXTENSIBLE names the real format in its GUID.
                if tag == 0xFFFE && body.len() >= 26 {
                    tag = u16::from_le_bytes([body[24], body[25]]);
                }
                format = Some((tag, channels.max(1), rate, bits));
            }
            b"data" => {
                let (tag, channels, rate, bits) =
                    format.ok_or("a WAV file with its data before its format")?;
                let mono = mix_down(decode(tag, bits, body)?, channels as usize);
                return Ok(Clip {
                    samples: mono,
                    sample_rate: rate,
                });
            }
            _ => {}
        }
        // Chunks are padded to an even length.
        at += 8 + len + (len & 1);
    }
    Err("a WAV file with no audio".into())
}

fn decode(tag: u16, bits: u16, data: &[u8]) -> Result<Vec<i16>, String> {
    Ok(match (tag, bits) {
        (1, 8) => data.iter().map(|&b| ((b as i16) - 128) << 8).collect(),
        (1, 16) => data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect(),
        (1, 24) => data
            .as_chunks::<3>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes([c[1], c[2]]))
            .collect(),
        (1, 32) => data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes([c[2], c[3]]))
            .collect(),
        (3, 32) => data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| {
                let f = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                (f.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
            })
            .collect(),
        (6, 8) => data.iter().map(|&b| alaw(b)).collect(),
        (7, 8) => data.iter().map(|&b| ulaw(b)).collect(),
        _ => return Err(format!("WAV format {tag} at {bits} bits cannot be played")),
    })
}

fn mix_down(samples: Vec<i16>, channels: usize) -> Vec<i16> {
    if channels == 1 {
        return samples;
    }
    samples
        .chunks_exact(channels)
        .map(|f| (f.iter().map(|&s| s as i32).sum::<i32>() / channels as i32) as i16)
        .collect()
}

/// G.711 μ-law to linear.
fn ulaw(b: u8) -> i16 {
    let b = !b;
    let exponent = (b >> 4) & 0x07;
    let mantissa = (b & 0x0F) as i16;
    let magnitude = ((mantissa << 3) + 0x84) << exponent;
    let value = magnitude - 0x84;
    if b & 0x80 != 0 {
        -value
    } else {
        value
    }
}

/// G.711 A-law to linear.
fn alaw(b: u8) -> i16 {
    let b = b ^ 0x55;
    let exponent = (b >> 4) & 0x07;
    let mantissa = (b & 0x0F) as i16;
    let magnitude = if exponent == 0 {
        (mantissa << 4) + 8
    } else {
        ((mantissa << 4) + 0x108) << (exponent - 1)
    };
    if b & 0x80 != 0 {
        magnitude
    } else {
        -magnitude
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(tag: u16, channels: u16, rate: u32, bits: u16, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(4 + 8 + 16 + 8 + 2 + 8 + data.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * (bits as u32 / 8) * channels as u32).to_le_bytes());
        out.extend_from_slice(&(channels * bits / 8).to_le_bytes());
        out.extend_from_slice(&bits.to_le_bytes());
        // An odd-length chunk FCP's writer might add, padded.
        out.extend_from_slice(b"LIST");
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        out
    }

    #[test]
    fn sixteen_bit_pcm_is_read_and_stereo_mixed_down() {
        let data: Vec<u8> = [100i16, 300, -2000, -4000]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        let clip = decode_wav(&wav(1, 2, 16000, 16, &data)).unwrap();
        assert_eq!(clip.sample_rate, 16000);
        assert_eq!(clip.samples, vec![200, -3000]);
    }

    #[test]
    fn g711_is_expanded() {
        // Silence in each law, then the loudest positive and negative codes.
        let clip = decode_wav(&wav(7, 1, 8000, 8, &[0xFF, 0x80, 0x00])).unwrap();
        assert_eq!(clip.samples, vec![0, 32124, -32124]);
        let clip = decode_wav(&wav(6, 1, 8000, 8, &[0xD5, 0xAA, 0x2A])).unwrap();
        assert_eq!(clip.samples, vec![8, 32256, -32256]);
    }

    #[test]
    fn what_is_not_a_playable_wav_says_so() {
        assert!(decode_wav(b"OggS....").is_err());
        assert!(decode_wav(&wav(2, 1, 8000, 4, &[1, 2])).is_err());
    }
}
