//! Ringing on this device: an incoming call rings the speaker for sounds
//! (on Windows the speakers, not the headset calls use) with the ringtone
//! chosen, and a second call while one is up is a short tone in the call
//! instead. A phone whose system rings for it (CallKit, ConnectionService)
//! does not ring here (`PhoneConfig::ring`).
//!
//! The ringtones are two of Anvil's own and the tenant's brand's
//! (`docs/BRANDING.md`: WAV, Ogg Vorbis or MP3 as FCP takes them).

use std::f32::consts::TAU;
use std::time::Duration;

use crate::clip::{decode_wav, Clip};

/// The sample rate Anvil's own sounds are made at.
const RATE: u32 = 16_000;

/// Anvil's own ringtones: id and name.
pub(crate) const BUILT_IN: [(&str, &str); 2] = [("anvil", "Anvil"), ("classic", "Classic")];

/// The ringtone when none is chosen and the brand names none.
pub(crate) const DEFAULT: &str = "anvil";

/// A brand ringtone's id: `brand:` and its name.
pub(crate) fn brand_id(name: &str) -> String {
    format!("brand:{name}")
}

/// What is sounding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sound {
    /// A call ringing in, on the ringing speaker.
    Ring,
    /// A call ringing in while another is up: a tone in the call.
    Waiting,
    /// A ringtone just chosen, heard once.
    Preview,
}

/// What should sound now: ringing while a call rings in and none is up,
/// the call waiting tone while one is.
pub(crate) fn wanted(calls: &[crate::CallView]) -> Option<Sound> {
    use crate::{CallState, Direction};
    let ringing_in = calls
        .iter()
        .any(|c| c.direction == Direction::Incoming && c.state == CallState::Ringing && !c.silent);
    if !ringing_in {
        return None;
    }
    let up = calls
        .iter()
        .any(|c| c.direction == Direction::Outgoing || c.state == CallState::Connected);
    Some(if up { Sound::Waiting } else { Sound::Ring })
}

/// A ringtone ready to play: the clip, and the silence after each pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Tone {
    pub clip: Clip,
    pub gap: Duration,
}

/// One of Anvil's own ringtones by id.
pub(crate) fn built_in(id: &str) -> Option<Tone> {
    let clip = match id {
        // Two bell notes, twice: a chime, not a telephone.
        "anvil" => {
            let mut s = Vec::new();
            for _ in 0..2 {
                s.extend(bell(1318.5, 0.4));
                s.extend(bell(987.8, 0.4));
            }
            s
        }
        // An electronic telephone's warble, in two bursts.
        "classic" => {
            let mut s = warble(0.4);
            s.extend(silence(0.2));
            s.extend(warble(0.4));
            s
        }
        _ => return None,
    };
    Some(Tone {
        clip: Clip {
            samples: clip,
            sample_rate: RATE,
        },
        gap: Duration::from_secs(2),
    })
}

/// The call waiting tone: two short beeps at 440 Hz, every ten seconds.
pub(crate) fn waiting() -> Tone {
    let mut s = sine(440.0, 0.2, 0.25);
    s.extend(silence(0.2));
    s.extend(sine(440.0, 0.2, 0.25));
    Tone {
        clip: Clip {
            samples: s,
            sample_rate: RATE,
        },
        gap: Duration::from_millis(9_400),
    }
}

/// The longest a brand ringtone plays before it starts again.
const LONGEST: Duration = Duration::from_secs(30);

/// A brand ringtone's asset, decoded: WAV, Ogg Vorbis or MP3.
pub(crate) fn decode(mime: &str, bytes: &[u8]) -> Result<Tone, String> {
    let mut clip = match mime {
        "audio/wav" | "audio/x-wav" | "audio/wave" => decode_wav(bytes)?,
        "audio/ogg" | "audio/mpeg" | "audio/mp3" => decode_compressed(mime, bytes)?,
        other => return Err(format!("a ringtone in {other} cannot be played")),
    };
    if !(8_000..=192_000).contains(&clip.sample_rate) {
        return Err(format!(
            "a ringtone at {} Hz cannot be played",
            clip.sample_rate
        ));
    }
    clip.samples
        .truncate((clip.sample_rate as u64 * LONGEST.as_secs()) as usize);
    if clip.samples.is_empty() {
        return Err("a ringtone with no audio".into());
    }
    Ok(Tone {
        clip,
        gap: Duration::from_secs(1),
    })
}

/// A ringtone added from a file: `file:` and its file name.
pub(crate) fn file_id(name: &str) -> String {
    format!("file:{name}")
}

/// The biggest sound file taken as a ringtone.
const BIGGEST_FILE: u64 = 20 * 1024 * 1024;

/// A sound file's type, by its extension.
fn mime_of(path: &std::path::Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "wav" | "wave" => "audio/wav",
        "ogg" | "oga" => "audio/ogg",
        "mp3" => "audio/mpeg",
        _ => return None,
    })
}

/// A ringtone file read and decoded: its name and its sound.
fn read_file(path: &std::path::Path) -> Result<(String, Tone), String> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("a ringtone file with no name")?
        .to_string();
    let mime = mime_of(path).ok_or("a ringtone must be a WAV, Ogg or MP3 file")?;
    let size = std::fs::metadata(path)
        .map_err(|e| format!("{name}: {e}"))?
        .len();
    if size > BIGGEST_FILE {
        return Err(format!("{name} is too big for a ringtone (20 MB at most)"));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{name}: {e}"))?;
    let tone = decode(mime, &bytes).map_err(|e| format!("{name}: {e}"))?;
    Ok((name, tone))
}

/// Take the sound file at `path` as a ringtone: decoded, so one that cannot
/// play is refused, then copied into `dir` under its own name (replacing
/// one of that name). Its name and its sound.
pub(crate) fn add_file(
    path: &std::path::Path,
    dir: &std::path::Path,
) -> Result<(String, Tone), String> {
    let (name, tone) = read_file(path)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let kept = dir.join(&name);
    if kept != path {
        std::fs::copy(path, &kept).map_err(|e| format!("{name} not kept: {e}"))?;
    }
    Ok((name, tone))
}

/// The ringtones added from files, kept in `dir`, by name. Those that
/// cannot be played are logged and left out.
pub(crate) fn from_dir(dir: &std::path::Path) -> Vec<(String, Tone)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut tones: Vec<(String, Tone)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter_map(|p| match read_file(&p) {
            Ok(t) => Some(t),
            Err(e) => {
                tracing::warn!(%e, "a ringtone file left out");
                None
            }
        })
        .collect();
    tones.sort_by_key(|a| a.0.to_lowercase());
    tones
}

/// A brand's ringtones decoded: name, whether the brand's default, the
/// sound. Those that cannot be played are logged and left out.
pub(crate) fn from_brand(ringtones: &[anvil_core::brand::Ringtone]) -> Vec<(String, bool, Tone)> {
    ringtones
        .iter()
        .filter(|r| !r.asset.bytes.is_empty())
        .filter_map(|r| match decode(&r.asset.mime, &r.asset.bytes) {
            Ok(tone) => Some((r.name.clone(), r.default, tone)),
            Err(e) => {
                tracing::warn!(ringtone = %r.name, %e, "a brand ringtone left out");
                None
            }
        })
        .collect()
}

/// Ogg Vorbis or MP3 through symphonia, mixed down to mono.
fn decode_compressed(mime: &str, bytes: &[u8]) -> Result<Clip, String> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
    use symphonia::core::errors::Error;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let source = MediaSourceStream::new(
        Box::new(std::io::Cursor::new(bytes.to_vec())),
        Default::default(),
    );
    let mut hint = Hint::new();
    hint.mime_type(mime);
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            source,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| format!("a ringtone not read: {e}"))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or("a ringtone with no audio track")?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("a ringtone not decoded: {e}"))?;
    let mut rate = track.codec_params.sample_rate.unwrap_or(0);
    let mut samples = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(Error::ResetRequired) => break,
            Err(e) => return Err(format!("a ringtone not read: {e}")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            // A damaged packet is skipped, as players do.
            Err(Error::DecodeError(_)) => continue,
            Err(e) => return Err(format!("a ringtone not decoded: {e}")),
        };
        let spec = *decoded.spec();
        rate = spec.rate;
        let channels = spec.channels.count().max(1);
        let mut buf = SampleBuffer::<i16>::new(decoded.capacity() as u64, spec);
        buf.copy_interleaved_ref(decoded);
        samples.extend(
            buf.samples().chunks(channels).map(|frame| {
                (frame.iter().map(|&s| s as i32).sum::<i32>() / channels as i32) as i16
            }),
        );
        if rate > 0 && samples.len() as u64 > rate as u64 * LONGEST.as_secs() {
            break;
        }
    }
    Ok(Clip {
        samples,
        sample_rate: rate,
    })
}

fn silence(secs: f32) -> Vec<i16> {
    vec![0; (RATE as f32 * secs) as usize]
}

/// A sine at `level` of full scale, its ends faded so it does not click.
fn sine(hz: f32, secs: f32, level: f32) -> Vec<i16> {
    let n = (RATE as f32 * secs) as usize;
    let fade = (RATE as usize / 200).min(n / 2);
    (0..n)
        .map(|i| {
            let edge = (i.min(n - 1 - i) as f32 / fade.max(1) as f32).min(1.0);
            let v = (TAU * hz * i as f32 / RATE as f32).sin() * level * edge;
            (v * i16::MAX as f32) as i16
        })
        .collect()
}

/// A struck bell: the note and its octave, dying away.
fn bell(hz: f32, secs: f32) -> Vec<i16> {
    let n = (RATE as f32 * secs) as usize;
    let attack = RATE as usize / 500;
    (0..n)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let rise = (i as f32 / attack as f32).min(1.0);
            let v = (TAU * hz * t).sin() * 0.6 + (TAU * hz * 2.0 * t).sin() * 0.2;
            let v = v * rise * (-6.0 * t).exp() * 0.6;
            (v * i16::MAX as f32) as i16
        })
        .collect()
}

/// Two tones swapped every 25 ms.
fn warble(secs: f32) -> Vec<i16> {
    let n = (RATE as f32 * secs) as usize;
    let fade = RATE as usize / 200;
    let mut phase = 0.0f32;
    (0..n)
        .map(|i| {
            let hz = if (i / (RATE as usize / 40)).is_multiple_of(2) {
                1000.0
            } else {
                1250.0
            };
            phase += TAU * hz / RATE as f32;
            let edge = (i.min(n - 1 - i) as f32 / fade as f32).min(1.0);
            ((phase.sin() * 0.35 * edge) * i16::MAX as f32) as i16
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CallState, CallView, Direction};

    fn call(id: u64, direction: Direction, state: CallState) -> CallView {
        CallView::new(id, direction, "sip:bob@example.com", state)
    }

    #[test]
    fn a_call_ringing_in_rings_and_a_call_up_makes_it_a_tone() {
        use CallState::*;
        use Direction::*;
        assert_eq!(wanted(&[]), None);
        assert_eq!(wanted(&[call(1, Incoming, Ringing)]), Some(Sound::Ring));
        assert_eq!(wanted(&[call(1, Incoming, Connected)]), None);
        assert_eq!(wanted(&[call(1, Outgoing, Ringing)]), None);
        assert_eq!(
            wanted(&[call(1, Incoming, Connected), call(2, Incoming, Ringing)]),
            Some(Sound::Waiting)
        );
        // Dialling out while a call rings in: it is not rung over.
        assert_eq!(
            wanted(&[call(1, Outgoing, Dialing), call(2, Incoming, Ringing)]),
            Some(Sound::Waiting)
        );
    }

    #[test]
    fn the_built_in_sounds_are_made_and_are_not_silent() {
        for (id, _) in BUILT_IN {
            let tone = built_in(id).unwrap();
            assert_eq!(tone.clip.sample_rate, RATE);
            assert!(tone.clip.samples.len() >= RATE as usize);
            assert!(tone.clip.samples.iter().any(|s| s.abs() > 8_000), "{id}");
        }
        assert!(built_in("brand:Acme").is_none());
        let w = waiting();
        assert!(w.clip.samples.iter().all(|s| s.abs() < 9_000));
        assert_eq!(
            w.gap + Duration::from_millis(w.clip.samples.len() as u64 * 1000 / RATE as u64),
            Duration::from_secs(10)
        );
    }

    /// A one-second 16-bit mono WAV of a 1 kHz sine at 8 kHz.
    fn wav() -> Vec<u8> {
        let samples: Vec<i16> = (0..8000)
            .map(|i| ((TAU * 1000.0 * i as f32 / 8000.0).sin() * 10_000.0) as i16)
            .collect();
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut out = Vec::new();
        out.extend(b"RIFF");
        out.extend((36 + data.len() as u32).to_le_bytes());
        out.extend(b"WAVEfmt ");
        out.extend(16u32.to_le_bytes());
        out.extend(1u16.to_le_bytes());
        out.extend(1u16.to_le_bytes());
        out.extend(8000u32.to_le_bytes());
        out.extend(16000u32.to_le_bytes());
        out.extend(2u16.to_le_bytes());
        out.extend(16u16.to_le_bytes());
        out.extend(b"data");
        out.extend((data.len() as u32).to_le_bytes());
        out.extend(data);
        out
    }

    #[test]
    fn a_brand_ringtone_is_decoded_or_refused() {
        let tone = decode("audio/wav", &wav()).unwrap();
        assert_eq!(tone.clip.sample_rate, 8000);
        assert_eq!(tone.clip.samples.len(), 8000);
        assert!(decode("audio/flac", &wav()).is_err());
        assert!(decode("audio/ogg", b"OggS but not really").is_err());
        assert!(decode("audio/mpeg", b"").is_err());
    }

    #[test]
    fn a_brands_ringtones_that_cannot_play_are_left_out() {
        let ringtone = |name: &str, mime: &str, bytes: Vec<u8>| {
            serde_json::from_value::<anvil_core::brand::Ringtone>(serde_json::json!({
                "name": name,
                "default": name == "Acme",
                "asset": {"url": "https://x/r", "mime": mime, "sha256": "00"},
            }))
            .map(|mut r| {
                r.asset.bytes = bytes;
                r
            })
            .unwrap()
        };
        let kept = from_brand(&[
            ringtone("Acme", "audio/wav", wav()),
            ringtone("Broken", "audio/ogg", b"OggS\0\x02anvil-ring".to_vec()),
            ringtone("Not fetched", "audio/wav", Vec::new()),
            ringtone(
                "Tune",
                "audio/mpeg",
                include_bytes!("../tests/data/ring.mp3").to_vec(),
            ),
        ]);
        let names: Vec<(&str, bool)> = kept.iter().map(|(n, d, _)| (n.as_str(), *d)).collect();
        assert_eq!(names, [("Acme", true), ("Tune", false)]);
    }

    #[test]
    fn a_ringtone_file_is_kept_beside_the_settings_and_read_back() {
        let dir = std::env::temp_dir().join(format!("anvil-ring-{}", std::process::id()));
        let from = dir.join("from");
        let kept = dir.join("ringtones");
        std::fs::create_dir_all(&from).unwrap();
        let mp3 = from.join("My Tone.MP3");
        std::fs::write(&mp3, include_bytes!("../tests/data/ring.mp3")).unwrap();
        let (name, tone) = add_file(&mp3, &kept).unwrap();
        assert_eq!(name, "My Tone.MP3");
        assert!(!tone.clip.samples.is_empty());
        let read = from_dir(&kept);
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].0, "My Tone.MP3");
        assert_eq!(file_id(&name), "file:My Tone.MP3");

        // Not a sound file, or not one that plays: refused, nothing kept.
        let text = from.join("notes.txt");
        std::fs::write(&text, "hello").unwrap();
        assert!(add_file(&text, &kept)
            .unwrap_err()
            .contains("WAV, Ogg or MP3"));
        let broken = from.join("broken.wav");
        std::fs::write(&broken, "not a wav").unwrap();
        assert!(add_file(&broken, &kept).is_err());
        assert_eq!(from_dir(&kept).len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_mp3_ringtone_is_decoded() {
        let tone = decode("audio/mpeg", include_bytes!("../tests/data/ring.mp3")).unwrap();
        assert!(tone.clip.sample_rate >= 8_000);
        assert!(tone.clip.samples.len() > tone.clip.sample_rate as usize / 2);
        assert!(tone.clip.samples.iter().any(|s| s.abs() > 1_000));
    }

    #[test]
    fn an_ogg_vorbis_ringtone_is_decoded() {
        let tone = decode("audio/ogg", include_bytes!("../tests/data/ring.ogg")).unwrap();
        assert!(tone.clip.sample_rate >= 8_000);
        assert!(tone.clip.samples.len() > tone.clip.sample_rate as usize / 2);
        assert!(tone.clip.samples.iter().any(|s| s.abs() > 1_000));
    }
}
