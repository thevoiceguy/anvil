//! Configuration types for `Anvil::start`.

use std::ops::RangeInclusive;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::audio::AudioHost;
use crate::brand::{BrandCache, BrandCredential, BrandProvider};

/// SIP transport protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Transport {
    Udp,
    Tcp,
    Tls,
}

/// SIP account / registrar credentials.
#[derive(Debug, Clone)]
pub struct AccountConfig {
    /// Address-of-record, e.g. `sip:alice@example.com`.
    pub aor: String,
    /// Registrar host or SIP URI.
    pub registrar: String,
    pub username: String,
    pub password: String,
    pub transport: Transport,
    /// A SIP URI every out-of-dialog request is sent through, e.g.
    /// `sip:pbx.example.com:5060;transport=tcp` (RFC 3261 §8.1.2).
    pub outbound_proxy: Option<String>,
    pub stun: Option<String>,
    pub register_expires: Duration,
    pub user_agent: String,
    /// Local UDP bind address. `None` defaults to `0.0.0.0:0` (any interface,
    /// ephemeral port). Set this if the user wants to pin the SIP port.
    pub bind_addr: Option<String>,
    /// Optional PEM-encoded extra CA certificates to trust for TLS,
    /// in addition to the system root store. Useful for self-signed
    /// PBX deployments. Only consulted when `transport == Transport::Tls`.
    pub tls_extra_ca_pem: Option<Vec<u8>>,
    /// Base URL of the FCP provisioning service. If `None`, Anvil attempts
    /// auto-discovery from the REGISTER 200 OK `X-FCP-Provisioning-Url`
    /// header; if that's also absent, branding is skipped and the UI falls
    /// back to its built-in default theme.
    pub provisioning_url: Option<String>,
    /// This install's instance (RFC 5626 `+sip.instance`), e.g.
    /// `<urn:uuid:…>`: the same on every start, so a registrar replaces
    /// the install's old binding rather than adding to it. `None` registers
    /// without outbound.
    pub instance_id: Option<String>,
    /// Where this install is woken when it sleeps (RFC 8599): carried by
    /// every REGISTER's Contact. Changed later by
    /// [`Anvil::set_push`](crate::Anvil::set_push).
    pub push: Option<PushAddress>,
}

/// A push notification service an app is woken through (RFC 8599
/// `pn-provider`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PushProvider {
    /// Apple's (PushKit VoIP pushes on iOS).
    Apns,
    /// Firebase Cloud Messaging (Android).
    Fcm,
}

impl PushProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Apns => "apns",
            Self::Fcm => "fcm",
        }
    }
}

/// An app's push address (RFC 8599 §4.1): the provider, the app at it
/// (`pn-param`: APNs `<team id>.<topic>`, FCM the project id) and this
/// install's token (`pn-prid`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushAddress {
    pub provider: PushProvider,
    pub param: String,
    pub prid: String,
}

impl PushAddress {
    /// The Contact URI parameters a REGISTER carries.
    pub fn contact_params(&self) -> String {
        format!(
            ";pn-provider={};pn-param={};pn-prid={}",
            self.provider.as_str(),
            self.param,
            self.prid
        )
    }
}

/// DTMF transmission mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DtmfMode {
    Rfc2833,
    Inband,
    Both,
}

/// SRTP policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SrtpMode {
    Off,
    Optional,
    Required,
}

/// Audio codec identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Codec {
    Pcmu,
    Pcma,
    G722,
    Opus,
}

/// Media-plane configuration.
#[derive(Debug, Clone)]
pub struct MediaConfig {
    pub codecs: Vec<Codec>,
    pub dtmf: DtmfMode,
    pub rtp_port_range: RangeInclusive<u16>,
    pub srtp: SrtpMode,
    pub jitter_buffer_ms: u32,
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            codecs: vec![Codec::Opus, Codec::G722, Codec::Pcmu, Codec::Pcma],
            dtmf: DtmfMode::Rfc2833,
            rtp_port_range: 16384..=32767,
            srtp: SrtpMode::Off,
            jitter_buffer_ms: 60,
        }
    }
}

/// Brand-provisioning configuration. If `provider` is `None`, Anvil does not
/// fetch branding and hosts render with their built-in theme.
pub struct BrandConfig {
    pub provider: Option<Box<dyn BrandProvider>>,
    pub cache: Option<Box<dyn BrandCache>>,
    /// How to authenticate to the provisioning endpoint. Typically derived
    /// from the account credentials, but exposed separately because some
    /// deployments hand out a short-lived token after SIP auth.
    pub credential: BrandCredential,
}

impl Default for BrandConfig {
    fn default() -> Self {
        Self {
            provider: None,
            cache: None,
            credential: BrandCredential::None,
        }
    }
}

/// Top-level configuration.
pub struct AnvilConfig {
    pub account: AccountConfig,
    pub media: MediaConfig,
    pub audio: Box<dyn AudioHost>,
    pub brand: BrandConfig,
}
