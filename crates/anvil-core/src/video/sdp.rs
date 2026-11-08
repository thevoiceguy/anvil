//! A call's video sections in SDP: what we offer and answer, and what the
//! far end's sections say.
//!
//! This is the SIP transport's business alone (§2.4): what it reads here
//! becomes a [`StreamPlan`](super::StreamPlan), and nothing above the
//! transport sees SDP.
//!
//! Sections are read line by line, as `media::extract_direction` reads
//! audio's: `sip_sdp`'s parser has merged a property attribute into the
//! next attribute's name, and a video section is full of both.

use std::fmt::Write as _;
use std::net::IpAddr;

use super::host::{VideoCodec, VideoKind};
use crate::media::MediaDirection;
use crate::srtp::Crypto;

/// The payload types we offer: H.264, then VP8.
const H264_PT: u8 = 96;
const VP8_PT: u8 = 97;

/// Our H.264: constrained baseline, level 3.1, FU-A allowed.
const H264_PROFILE: &str = "42e01f";

pub(crate) use super::Feedback;

/// One codec in a section, with its payload type there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SectionCodec {
    pub codec: VideoCodec,
    pub pt: u8,
    /// H.264's `profile-level-id`, as offered.
    pub profile_level_id: Option<String>,
    pub feedback: Feedback,
}

/// The RTP profile a section is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Profile {
    Avp,
    Avpf,
    Savp,
    Savpf,
}

impl Profile {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "RTP/AVP" => Some(Self::Avp),
            "RTP/AVPF" => Some(Self::Avpf),
            "RTP/SAVP" => Some(Self::Savp),
            "RTP/SAVPF" => Some(Self::Savpf),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Avp => "RTP/AVP",
            Self::Avpf => "RTP/AVPF",
            Self::Savp => "RTP/SAVP",
            Self::Savpf => "RTP/SAVPF",
        }
    }

    pub(crate) fn secure(self) -> bool {
        matches!(self, Self::Savp | Self::Savpf)
    }

    /// The same profile, secure or not.
    fn with_security(self, secure: bool) -> Self {
        match (self, secure) {
            (Self::Avp | Self::Savp, false) => Self::Avp,
            (Self::Avp | Self::Savp, true) => Self::Savp,
            (Self::Avpf | Self::Savpf, false) => Self::Avpf,
            (Self::Avpf | Self::Savpf, true) => Self::Savpf,
        }
    }
}

/// A video section of ours, as it goes into an offer or an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LocalSection {
    pub kind: VideoKind,
    /// 0 declines the section.
    pub port: u16,
    pub profile: Profile,
    /// Offered: every codec we take. Answered: the one chosen.
    pub codecs: Vec<SectionCodec>,
    pub direction: MediaDirection,
    pub crypto: Option<Crypto>,
}

impl LocalSection {
    /// An offer of `codecs` (in order) on `port`.
    pub(crate) fn offer(
        kind: VideoKind,
        port: u16,
        codecs: &[VideoCodec],
        direction: MediaDirection,
        crypto: Option<Crypto>,
    ) -> Self {
        let codecs = codecs
            .iter()
            .map(|&codec| SectionCodec {
                codec,
                pt: match codec {
                    VideoCodec::H264 => H264_PT,
                    VideoCodec::Vp8 => VP8_PT,
                },
                profile_level_id: (codec == VideoCodec::H264).then(|| H264_PROFILE.to_string()),
                feedback: Feedback {
                    nack: true,
                    pli: true,
                    fir: true,
                    remb: true,
                },
            })
            .collect();
        Self {
            kind,
            port,
            profile: Profile::Avp.with_security(crypto.is_some()),
            codecs,
            direction,
            crypto,
        }
    }

    /// The answer declining `offered`: port 0, its own first format.
    pub(crate) fn declined(offered: &RemoteSection) -> Self {
        Self {
            kind: offered.kind,
            port: 0,
            profile: offered.profile,
            codecs: Vec::new(),
            direction: MediaDirection::Inactive,
            crypto: None,
        }
        .with_formats(offered.formats.first().cloned())
    }

    fn with_formats(mut self, first: Option<String>) -> Self {
        // A declined section still names a format (RFC 3264 §6); one it
        // was offered, unknown to us or not.
        if let Some(f) = first.and_then(|f| f.parse::<u8>().ok()) {
            self.codecs.push(SectionCodec {
                codec: VideoCodec::H264,
                pt: f,
                profile_level_id: None,
                feedback: Feedback::default(),
            });
        }
        self
    }

    /// The section's lines.
    fn write(&self, out: &mut String) {
        let pts: Vec<String> = self.codecs.iter().map(|c| c.pt.to_string()).collect();
        let pts = if pts.is_empty() {
            H264_PT.to_string()
        } else {
            pts.join(" ")
        };
        let _ = write!(
            out,
            "m=video {} {} {pts}\r\n",
            self.port,
            self.profile.as_str()
        );
        if self.port == 0 {
            return;
        }
        for c in &self.codecs {
            let name = match c.codec {
                VideoCodec::H264 => "H264",
                VideoCodec::Vp8 => "VP8",
            };
            let _ = write!(out, "a=rtpmap:{} {name}/90000\r\n", c.pt);
            if c.codec == VideoCodec::H264 {
                let profile = c.profile_level_id.as_deref().unwrap_or(H264_PROFILE);
                let _ = write!(
                    out,
                    "a=fmtp:{} profile-level-id={profile};packetization-mode=1;\
                     level-asymmetry-allowed=1\r\n",
                    c.pt
                );
            }
            let fb = c.feedback;
            if fb.nack {
                let _ = write!(out, "a=rtcp-fb:{} nack\r\n", c.pt);
            }
            if fb.pli {
                let _ = write!(out, "a=rtcp-fb:{} nack pli\r\n", c.pt);
            }
            if fb.fir {
                let _ = write!(out, "a=rtcp-fb:{} ccm fir\r\n", c.pt);
            }
            if fb.remb {
                let _ = write!(out, "a=rtcp-fb:{} goog-remb\r\n", c.pt);
            }
        }
        if self.kind == VideoKind::Content {
            out.push_str("a=content:slides\r\n");
        }
        if let Some(crypto) = &self.crypto {
            let _ = write!(out, "a=crypto:{}\r\n", crypto.attribute());
        }
        let _ = write!(out, "a={}\r\n", self.direction.as_sdp_attr());
    }
}

/// `sdp` (an audio session from `media::build_sdp`) with `sections` after
/// its audio.
pub(crate) fn with_sections(mut sdp: String, sections: &[LocalSection]) -> String {
    if !sdp.ends_with("\r\n") {
        sdp.push_str("\r\n");
    }
    for s in sections {
        s.write(&mut sdp);
    }
    sdp
}

/// One of the far end's video sections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteSection {
    pub kind: VideoKind,
    pub port: u16,
    /// The section's `c=`, else the session's.
    pub addr: Option<IpAddr>,
    pub profile: Profile,
    /// Its formats as listed.
    pub formats: Vec<String>,
    /// The codecs in it we can carry, in its order.
    pub codecs: Vec<SectionCodec>,
    pub direction: MediaDirection,
    pub crypto: Vec<Crypto>,
}

impl RemoteSection {
    /// Live: a port, and a profile we speak.
    pub(crate) fn live(&self) -> bool {
        self.port != 0
    }

    /// The first codec of `ours` (in our order) the section offers, as it
    /// names it.
    pub(crate) fn choose(&self, ours: &[VideoCodec]) -> Option<&SectionCodec> {
        ours.iter()
            .find_map(|want| self.codecs.iter().find(|c| c.codec == *want))
    }

    /// Our answer to this section: `codec` from it, `port`, `direction`
    /// (already mirrored), our `crypto`.
    pub(crate) fn answer(
        &self,
        codec: &SectionCodec,
        port: u16,
        direction: MediaDirection,
        crypto: Option<Crypto>,
    ) -> LocalSection {
        // Its payload type and profile, and only the feedback it named.
        let ours = codec.clone();
        LocalSection {
            kind: self.kind,
            port,
            profile: self.profile.with_security(crypto.is_some()),
            codecs: vec![ours],
            direction,
            crypto,
        }
    }
}

/// Every `m=video` section of `sdp`, in order.
pub(crate) fn sections(sdp: &[u8]) -> Vec<RemoteSection> {
    let text = String::from_utf8_lossy(sdp);
    let mut session_addr = None;
    let mut out = Vec::new();
    let mut current: Option<Raw> = None;
    for line in text.lines() {
        let line = line.trim_end();
        if let Some(m) = line.strip_prefix("m=") {
            if let Some(raw) = current.take() {
                out.extend(raw.finish(session_addr));
            }
            current = Some(Raw::new(m));
            continue;
        }
        match &mut current {
            None => {
                if let Some(c) = line.strip_prefix("c=") {
                    session_addr = connection_addr(c);
                }
            }
            Some(raw) => raw.line(line),
        }
    }
    if let Some(raw) = current.take() {
        out.extend(raw.finish(session_addr));
    }
    out
}

/// One section as read, before it is understood.
struct Raw {
    media: String,
    port: u16,
    profile: Option<Profile>,
    formats: Vec<String>,
    addr: Option<IpAddr>,
    attrs: Vec<(String, Option<String>)>,
}

impl Raw {
    fn new(m: &str) -> Self {
        let mut parts = m.split_whitespace();
        let media = parts.next().unwrap_or_default().to_string();
        let port = parts
            .next()
            .and_then(|p| p.split('/').next())
            .and_then(|p| p.parse().ok())
            .unwrap_or(0);
        let profile = parts.next().and_then(Profile::parse);
        let formats = parts.map(str::to_string).collect();
        Self {
            media,
            port,
            profile,
            formats,
            addr: None,
            attrs: Vec::new(),
        }
    }

    fn line(&mut self, line: &str) {
        if let Some(c) = line.strip_prefix("c=") {
            self.addr = connection_addr(c);
        } else if let Some(a) = line.strip_prefix("a=") {
            match a.split_once(':') {
                Some((name, value)) => self
                    .attrs
                    .push((name.trim().to_string(), Some(value.trim().to_string()))),
                None => self.attrs.push((a.trim().to_string(), None)),
            }
        }
    }

    fn values<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.attrs
            .iter()
            .filter(move |(n, _)| n == name)
            .filter_map(|(_, v)| v.as_deref())
    }

    fn finish(self, session_addr: Option<IpAddr>) -> Option<RemoteSection> {
        if self.media != "video" {
            return None;
        }
        // A profile we cannot speak (a WebRTC one, say) is not one to
        // answer with media.
        let (profile, port) = match self.profile {
            Some(p) => (p, self.port),
            None => (Profile::Avp, 0),
        };
        let kind = match self.values("content").next() {
            Some(v) if v.split(',').any(|c| c.trim() == "slides") => VideoKind::Content,
            _ => VideoKind::Camera,
        };
        let direction = self
            .attrs
            .iter()
            .filter(|(_, v)| v.is_none())
            .find_map(|(n, _)| MediaDirection::from_attr(n))
            .unwrap_or(MediaDirection::Sendrecv);
        let crypto = self.values("crypto").filter_map(Crypto::parse).collect();

        let mut codecs = Vec::new();
        for f in &self.formats {
            let Ok(pt) = f.parse::<u8>() else { continue };
            let Some(name) = self.values("rtpmap").find_map(|v| {
                let (p, rest) = v.split_once(' ')?;
                (p.parse::<u8>().ok()? == pt).then(|| rest.split('/').next().unwrap_or(""))
            }) else {
                continue;
            };
            let fmtp: Vec<(String, String)> = self
                .values("fmtp")
                .filter_map(|v| {
                    let (p, params) = v.split_once(' ')?;
                    (p.parse::<u8>().ok()? == pt).then_some(params)
                })
                .flat_map(|params| params.split(';'))
                .filter_map(|kv| {
                    let (k, v) = kv.split_once('=')?;
                    Some((k.trim().to_ascii_lowercase(), v.trim().to_string()))
                })
                .collect();
            let param = |k: &str| fmtp.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
            let (codec, profile_level_id) = match name.to_ascii_uppercase().as_str() {
                "H264" => {
                    // FU-A needs packetization mode 1; our encoders are
                    // baseline, so a profile they cannot meet is passed by.
                    if param("packetization-mode").as_deref() != Some("1") {
                        continue;
                    }
                    let profile = param("profile-level-id").unwrap_or_else(|| "42001f".into());
                    if !profile.to_ascii_lowercase().starts_with("42") {
                        continue;
                    }
                    (VideoCodec::H264, Some(profile))
                }
                "VP8" => (VideoCodec::Vp8, None),
                _ => continue,
            };
            let mut feedback = Feedback::default();
            for v in self.values("rtcp-fb") {
                let Some((p, kind)) = v.split_once(' ') else {
                    continue;
                };
                if p != "*" && p.parse::<u8>().ok() != Some(pt) {
                    continue;
                }
                match kind.trim() {
                    "nack" => feedback.nack = true,
                    "nack pli" => feedback.pli = true,
                    "ccm fir" => feedback.fir = true,
                    "goog-remb" => feedback.remb = true,
                    _ => {}
                }
            }
            codecs.push(SectionCodec {
                codec,
                pt,
                profile_level_id,
                feedback,
            });
        }
        Some(RemoteSection {
            kind,
            port,
            addr: self.addr.or(session_addr),
            profile,
            formats: self.formats,
            codecs,
            direction,
            crypto,
        })
    }
}

fn connection_addr(c: &str) -> Option<IpAddr> {
    // c=IN IP4 203.0.113.5[/ttl]
    let addr = c.split_whitespace().nth(2)?;
    addr.split('/').next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::srtp::Suite;

    fn audio() -> String {
        "v=0\r\no=- 1 1 IN IP4 192.0.2.1\r\ns=-\r\nc=IN IP4 192.0.2.1\r\nt=0 0\r\n\
         m=audio 4000 RTP/AVP 0\r\na=sendrecv\r\n"
            .to_string()
    }

    #[test]
    fn an_offer_of_ours_is_read_back_as_we_wrote_it() {
        let key = Crypto::generate(1, Suite::AesCm128HmacSha1_80);
        let camera = LocalSection::offer(
            VideoKind::Camera,
            5000,
            &[VideoCodec::H264, VideoCodec::Vp8],
            MediaDirection::Sendrecv,
            Some(key.clone()),
        );
        let content = LocalSection::offer(
            VideoKind::Content,
            5002,
            &[VideoCodec::H264],
            MediaDirection::Recvonly,
            None,
        );
        let sdp = with_sections(audio(), &[camera, content]);
        assert!(sdp.contains("m=video 5000 RTP/SAVP 96 97\r\n"), "{sdp}");
        assert!(sdp.contains(
            "a=fmtp:96 profile-level-id=42e01f;packetization-mode=1;level-asymmetry-allowed=1"
        ));
        assert!(sdp.contains("a=rtcp-fb:97 goog-remb\r\n"));

        let read = sections(sdp.as_bytes());
        assert_eq!(read.len(), 2, "the audio section is not video");
        let (cam, share) = (&read[0], &read[1]);
        assert_eq!(cam.kind, VideoKind::Camera);
        assert_eq!(cam.port, 5000);
        assert_eq!(cam.addr, Some("192.0.2.1".parse().unwrap()));
        assert_eq!(cam.profile, Profile::Savp);
        assert_eq!(cam.crypto, vec![key]);
        assert_eq!(
            cam.codecs.iter().map(|c| c.codec).collect::<Vec<_>>(),
            [VideoCodec::H264, VideoCodec::Vp8]
        );
        assert_eq!(
            cam.codecs[0].feedback,
            Feedback {
                nack: true,
                pli: true,
                fir: true,
                remb: true
            }
        );
        assert_eq!(share.kind, VideoKind::Content);
        assert_eq!(share.direction, MediaDirection::Recvonly);
        assert!(share.crypto.is_empty());
        assert_eq!(share.profile, Profile::Avp);
    }

    #[test]
    fn h264_is_taken_only_with_fu_a_and_a_baseline_profile() {
        let sdp = "v=0\r\nc=IN IP4 198.51.100.7\r\n\
            m=video 6000 RTP/AVPF 100 101 102 103\r\n\
            a=rtpmap:100 H264/90000\r\n\
            a=fmtp:100 profile-level-id=640c1f;packetization-mode=1\r\n\
            a=rtpmap:101 H264/90000\r\n\
            a=fmtp:101 profile-level-id=42e01f\r\n\
            a=rtpmap:102 H264/90000\r\n\
            a=fmtp:102 profile-level-id=42e01f; packetization-mode=1\r\n\
            a=rtpmap:103 VP8/90000\r\n\
            a=rtcp-fb:* nack\r\n\
            a=rtcp-fb:102 nack pli\r\n\
            a=sendonly\r\n";
        let read = sections(sdp.as_bytes());
        let s = &read[0];
        assert_eq!(s.profile, Profile::Avpf);
        assert_eq!(s.direction, MediaDirection::Sendonly);
        let pts: Vec<u8> = s.codecs.iter().map(|c| c.pt).collect();
        assert_eq!(pts, [102, 103], "high profile and mode 0 passed by");
        assert!(s.codecs[0].feedback.nack && s.codecs[0].feedback.pli);
        assert!(s.codecs[1].feedback.nack && !s.codecs[1].feedback.pli);
        // VP8 first when we prefer it; H.264 by its own payload type.
        assert_eq!(s.choose(&[VideoCodec::Vp8]).map(|c| c.pt), Some(103));
        assert_eq!(
            s.choose(&[VideoCodec::H264, VideoCodec::Vp8]).map(|c| c.pt),
            Some(102)
        );

        // The answer: its payload type and profile, what it takes, our
        // direction, the profile kept.
        let answer = s.answer(&s.codecs[0], 7000, MediaDirection::Recvonly, None);
        let text = with_sections(audio(), &[answer]);
        assert!(text.contains("m=video 7000 RTP/AVPF 102\r\n"), "{text}");
        assert!(text.contains("profile-level-id=42e01f;packetization-mode=1"));
        assert!(text.contains("a=rtcp-fb:102 nack pli\r\n"));
        assert!(
            !text.contains("goog-remb"),
            "not asked for what it never named"
        );
        assert!(text.contains("a=recvonly\r\n"));
    }

    #[test]
    fn a_declined_section_is_port_zero_with_one_of_its_formats() {
        let sdp = "v=0\r\nc=IN IP4 198.51.100.7\r\n\
            m=video 6000 RTP/AVP 120\r\na=rtpmap:120 AV1/90000\r\n\
            m=video 6002 UDP/TLS/RTP/SAVPF 96\r\n";
        let read = sections(sdp.as_bytes());
        assert!(
            read[0].live() && read[0].codecs.is_empty(),
            "nothing we carry"
        );
        assert!(!read[1].live(), "a profile we do not speak");
        let text = with_sections(audio(), &[LocalSection::declined(&read[0])]);
        assert!(text.ends_with("m=video 0 RTP/AVP 120\r\n"), "{text}");
    }
}
