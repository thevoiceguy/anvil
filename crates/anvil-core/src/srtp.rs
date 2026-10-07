//! SDES-SRTP (RFC 4568, RFC 3711): the `a=crypto` attribute an offer and an
//! answer carry, and the SRTP context their keys make. The suites are the
//! two every SIP endpoint speaks, AES_CM_128_HMAC_SHA1_80 and _32.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use forge_rtp::srtp::{SrtpContext, SrtpKeyMaterial, SrtpProfile};
use rand::RngCore;

/// A crypto suite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Suite {
    AesCm128HmacSha1_80,
    AesCm128HmacSha1_32,
}

impl Suite {
    fn name(self) -> &'static str {
        match self {
            Suite::AesCm128HmacSha1_80 => "AES_CM_128_HMAC_SHA1_80",
            Suite::AesCm128HmacSha1_32 => "AES_CM_128_HMAC_SHA1_32",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        match name {
            "AES_CM_128_HMAC_SHA1_80" => Some(Suite::AesCm128HmacSha1_80),
            "AES_CM_128_HMAC_SHA1_32" => Some(Suite::AesCm128HmacSha1_32),
            _ => None,
        }
    }

    fn profile(self) -> SrtpProfile {
        match self {
            Suite::AesCm128HmacSha1_80 => SrtpProfile::Aes128CmHmacSha1_80,
            Suite::AesCm128HmacSha1_32 => SrtpProfile::Aes128CmHmacSha1_32,
        }
    }
}

const KEY_LEN: usize = 16;
const SALT_LEN: usize = 14;

/// One `a=crypto` line: a tag, a suite and the master key and salt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Crypto {
    pub tag: u32,
    pub suite: Suite,
    key: Vec<u8>,
    salt: Vec<u8>,
}

impl Crypto {
    /// A fresh key.
    pub fn generate(tag: u32, suite: Suite) -> Self {
        let mut rng = rand::thread_rng();
        let mut key = vec![0u8; KEY_LEN];
        let mut salt = vec![0u8; SALT_LEN];
        rng.fill_bytes(&mut key);
        rng.fill_bytes(&mut salt);
        Self {
            tag,
            suite,
            key,
            salt,
        }
    }

    /// The attribute's value: `1 AES_CM_128_HMAC_SHA1_80 inline:…`.
    pub fn attribute(&self) -> String {
        let mut keysalt = self.key.clone();
        keysalt.extend_from_slice(&self.salt);
        format!(
            "{} {} inline:{}",
            self.tag,
            self.suite.name(),
            STANDARD.encode(keysalt)
        )
    }

    /// The attribute from its value; `None` for a suite or key this does
    /// not take (a lifetime or MKI after the key is ignored).
    pub fn parse(value: &str) -> Option<Self> {
        let mut parts = value.split_whitespace();
        let tag = parts.next()?.parse().ok()?;
        let suite = Suite::from_name(parts.next()?)?;
        let inline = parts.next()?.strip_prefix("inline:")?;
        let keysalt = STANDARD.decode(inline.split('|').next()?).ok()?;
        if keysalt.len() != KEY_LEN + SALT_LEN {
            return None;
        }
        Some(Self {
            tag,
            suite,
            key: keysalt[..KEY_LEN].to_vec(),
            salt: keysalt[KEY_LEN..].to_vec(),
        })
    }

    fn key_material(&self) -> Option<SrtpKeyMaterial> {
        SrtpKeyMaterial::new(self.key.clone(), self.salt.clone(), self.suite.profile()).ok()
    }
}

/// Every `a=crypto` attribute in an SDP body that this takes.
pub(crate) fn offered(sdp: &[u8]) -> Vec<Crypto> {
    String::from_utf8_lossy(sdp)
        .lines()
        .filter_map(|line| line.trim().strip_prefix("a=crypto:"))
        .filter_map(Crypto::parse)
        .collect()
}

/// Whether the body's audio stream is the secure profile (`RTP/SAVP`).
pub(crate) fn is_secure(sdp: &[u8]) -> bool {
    String::from_utf8_lossy(sdp)
        .lines()
        .filter(|line| line.starts_with("m=audio"))
        .any(|line| line.split_whitespace().nth(2) == Some("RTP/SAVP"))
}

/// An answer to an offer's crypto: the first one this takes, answered with
/// a fresh key under the same tag and suite (RFC 4568 §7.1.1). Returns
/// (ours, theirs).
pub(crate) fn answer_to(offers: &[Crypto]) -> Option<(Crypto, Crypto)> {
    let theirs = offers.first()?.clone();
    Some((Crypto::generate(theirs.tag, theirs.suite), theirs))
}

/// The answer's crypto that goes with the one offered: the same tag and
/// suite.
pub(crate) fn matching<'a>(ours: &Crypto, answered: &'a [Crypto]) -> Option<&'a Crypto> {
    answered
        .iter()
        .find(|c| c.tag == ours.tag && c.suite == ours.suite)
}

/// The SRTP context for a call: what we send under our key, what we
/// receive under theirs.
pub(crate) fn context(ours: &Crypto, theirs: &Crypto) -> Option<SrtpContext> {
    Some(SrtpContext::with_keys(
        ours.key_material()?,
        theirs.key_material()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_attribute_survives_its_own_round_trip() {
        let c = Crypto::generate(1, Suite::AesCm128HmacSha1_80);
        let line = c.attribute();
        assert!(
            line.starts_with("1 AES_CM_128_HMAC_SHA1_80 inline:"),
            "{line}"
        );
        assert_eq!(Crypto::parse(&line), Some(c));
    }

    #[test]
    fn an_offer_is_read_as_other_endpoints_write_it() {
        // RFC 4568 §6's example, a lifetime and MKI after the key.
        let sdp = b"v=0\r\nm=audio 49170 RTP/SAVP 0\r\n\
            a=crypto:1 AES_CM_128_HMAC_SHA1_80 inline:WVNfX19zZW1jdGwgKCkgewkyMjA7fQp9CnVubGVz|2^20|1:4\r\n\
            a=crypto:2 F8_128_HMAC_SHA1_80 inline:MTIzNDU2Nzg5QUJDREUwMTIzNDU2Nzg5QUJjZGVm\r\n";
        let offers = offered(sdp);
        assert_eq!(offers.len(), 1, "the F8 suite is not taken");
        assert_eq!(offers[0].tag, 1);
        assert!(is_secure(sdp));
        assert!(!is_secure(b"m=audio 49170 RTP/AVP 0\r\n"));
    }

    #[test]
    fn two_ends_keyed_from_offer_and_answer_hear_each_other() {
        let offer = Crypto::generate(1, Suite::AesCm128HmacSha1_80);
        let (answer, seen) = answer_to(&offered(
            format!("a=crypto:{}\r\n", offer.attribute()).as_bytes(),
        ))
        .unwrap();
        assert_eq!(seen, offer);
        assert_eq!(
            matching(&offer, std::slice::from_ref(&answer)),
            Some(&answer)
        );

        let mut caller = context(&offer, &answer).unwrap();
        let mut callee = context(&answer, &offer).unwrap();
        let rtp = forge_rtp::rtp::RtpPacket::build(
            0,
            7,
            160,
            0x1234,
            bytes::Bytes::from_static(&[1, 2, 3, 4]),
            false,
        )
        .to_bytes();
        let sent = caller.protect_rtp(&rtp).unwrap();
        assert_ne!(sent[12..], rtp[12..], "the payload is encrypted");
        assert_eq!(callee.unprotect_rtp(&sent).unwrap(), rtp.to_vec());
    }
}
