//! Tenant branding.
//!
//! A company deploying FCP can brand the softphone with its logo, colors, app
//! name, and supporting links. The softphone fetches this profile on login —
//! before the first REGISTER — so the UI can switch from the unbranded login
//! screen to the branded shell without a flash.
//!
//! This module defines the data model and the provider/cache traits.
//! `anvil-brand` supplies the HTTP + filesystem implementations;
//! mobile hosts may supply their own (e.g. URLSession + Keychain on iOS).
//!
//! See `docs/BRANDING.md` for the wire protocol.

use std::time::SystemTime;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::AnvilError;

/// RGBA color, components 0–255. On the wire `#RRGGBB` or `#RRGGBBAA`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    /// `#RRGGBB` or `#RRGGBBAA`; `None` for anything else.
    pub fn parse(hex: &str) -> Option<Self> {
        let digits = hex.trim().strip_prefix('#')?;
        if !matches!(digits.len(), 6 | 8) || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).ok();
        Some(Self {
            r: byte(0)?,
            g: byte(2)?,
            b: byte(4)?,
            a: if digits.len() == 8 { byte(6)? } else { 255 },
        })
    }

    /// `#RRGGBB`, or `#RRGGBBAA` when not opaque.
    pub fn to_hex(self) -> String {
        if self.a == 255 {
            format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
        } else {
            format!("#{:02X}{:02X}{:02X}{:02X}", self.r, self.g, self.b, self.a)
        }
    }
}

impl Serialize for Rgba {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Rgba {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Rgba::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("not a color: {s}")))
    }
}

/// A color slot as the contract reads it: one that is not a color is
/// ignored, and the UI uses its own token (`docs/BRANDING.md` §5).
fn lenient_color<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Rgba>, D::Error> {
    let value = Option::<serde_json::Value>::deserialize(d)?;
    Ok(value.and_then(|v| v.as_str().and_then(Rgba::parse)))
}

/// Color tokens a UI needs to theme the shell. Modelled loosely on Material 3
/// so Flutter / Jetpack hosts can map directly; native hosts (SwiftUI, AppKit)
/// pick the subset they use. A slot the tenant left unset is `None`: the UI
/// keeps its own token for it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BrandColors {
    #[serde(
        default,
        deserialize_with = "lenient_color",
        skip_serializing_if = "Option::is_none"
    )]
    pub primary: Option<Rgba>,
    #[serde(
        default,
        deserialize_with = "lenient_color",
        skip_serializing_if = "Option::is_none"
    )]
    pub accent: Option<Rgba>,
    #[serde(
        default,
        deserialize_with = "lenient_color",
        skip_serializing_if = "Option::is_none"
    )]
    pub background: Option<Rgba>,
    #[serde(
        default,
        deserialize_with = "lenient_color",
        skip_serializing_if = "Option::is_none"
    )]
    pub surface: Option<Rgba>,
    #[serde(
        default,
        deserialize_with = "lenient_color",
        skip_serializing_if = "Option::is_none"
    )]
    pub on_primary: Option<Rgba>,
    #[serde(
        default,
        deserialize_with = "lenient_color",
        skip_serializing_if = "Option::is_none"
    )]
    pub on_surface: Option<Rgba>,
    #[serde(
        default,
        deserialize_with = "lenient_color",
        skip_serializing_if = "Option::is_none"
    )]
    pub error: Option<Rgba>,
}

/// A binary asset (logo, icon, ringtone) shipped with a brand profile.
///
/// `bytes` is populated after the provider has downloaded and verified the
/// asset against `sha256`. Before download it is empty and `url` is the
/// source of truth.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrandAsset {
    pub url: String,
    pub mime: String,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bytes: Vec<u8>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    /// Where the cache keeps the verified file, when a filesystem cache
    /// holds it: what a host that cannot take bytes (the FFI) opens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_path: Option<String>,
}

/// Dial-plan hints the brand server can push. Optional; the softphone falls
/// back to raw E.164 entry if these are absent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DialPlan {
    /// Prefixes stripped from user input before placing a call.
    pub prefix_strip: Vec<String>,
    /// Short codes expanded to full SIP URIs.
    pub short_codes: std::collections::BTreeMap<String, String>,
}

/// Full brand profile. Delivered as one event; the UI re-renders on receipt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrandProfile {
    /// Wire-format version. Currently `1`.
    pub schema_version: u32,
    /// Opaque tenant identifier. Stable for a given FCP deployment + customer.
    pub tenant_id: String,
    /// Opaque cache validator (HTTP ETag contents).
    #[serde(default)]
    pub etag: String,
    /// Shown as the app title, in the about screen, and in OS call UI.
    pub app_name: String,
    /// Light-theme color tokens.
    #[serde(default)]
    pub colors: BrandColors,
    /// Dark-theme color tokens. Falls back to `colors` if absent.
    #[serde(default)]
    pub dark_colors: Option<BrandColors>,
    /// Horizontal logo for headers. SVG or PNG. Light-theme variant.
    #[serde(default)]
    pub logo: Option<BrandAsset>,
    /// Dark-theme logo variant.
    #[serde(default)]
    pub logo_dark: Option<BrandAsset>,
    /// App / taskbar icon. Square. PNG ≥ 512×512 preferred.
    #[serde(default)]
    pub icon: Option<BrandAsset>,
    /// Custom ringtones. The first entry with `default = true` is used
    /// unless the user has overridden in settings.
    #[serde(default)]
    pub ringtones: Vec<Ringtone>,
    /// Optional support + legal links surfaced in the settings screen.
    #[serde(default)]
    pub links: BrandLinks,
    /// Optional dial-plan hints.
    #[serde(default)]
    pub dial_plan: DialPlan,
    /// Wall-clock time the profile was fetched. Populated by the provider.
    #[serde(skip)]
    pub fetched_at: Option<SystemTime>,
}

/// A named ringtone asset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ringtone {
    pub name: String,
    pub asset: BrandAsset,
    #[serde(default)]
    pub default: bool,
}

/// Optional external URLs a branded softphone surfaces.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BrandLinks {
    pub support: Option<String>,
    pub privacy: Option<String>,
    pub terms: Option<String>,
    pub website: Option<String>,
}

/// Where the provider should fetch a brand profile from, and how to authenticate.
#[derive(Debug, Clone)]
pub struct BrandRequest {
    /// Base URL of the provisioning service, e.g. `https://fcp.example.com`.
    pub provisioning_url: String,
    /// SIP address-of-record of the logged-in user; servers key branding by
    /// the user's tenant (the domain part) but we send the full AOR so
    /// per-user overrides are possible.
    pub aor: String,
    /// Bearer token or password for authentication. Scheme is provider-specific.
    pub credential: BrandCredential,
    /// ETag of a previously cached profile, for `If-None-Match`.
    pub if_none_match: Option<String>,
}

/// Authentication material for the brand fetch.
#[derive(Debug, Clone)]
pub enum BrandCredential {
    /// Bearer token issued by FCP (typical after SIP auth succeeds).
    Bearer(String),
    /// HTTP Basic using the same credentials as SIP Digest.
    Basic { username: String, password: String },
    /// No credentials. Tenants may expose branding publicly.
    None,
}

/// Result of a fetch. `NotModified` preserves the cached profile;
/// `NoBrand` (a 404) means the tenant has none, and the UI shows its own
/// theme without an error.
pub enum BrandFetchOutcome {
    Updated(Box<BrandProfile>),
    NotModified,
    NoBrand,
}

/// Fetches a brand profile from FCP. Default impl in `anvil-brand`;
/// mobile hosts can inject their own.
#[async_trait]
pub trait BrandProvider: Send + Sync {
    async fn fetch(&self, req: BrandRequest) -> Result<BrandFetchOutcome, AnvilError>;
}

/// Persists brand profiles between runs. Default impl in `anvil-brand` is
/// filesystem-backed; hosts with platform-native secure storage may override.
#[async_trait]
pub trait BrandCache: Send + Sync {
    async fn load(&self, tenant_id: &str) -> Result<Option<BrandProfile>, AnvilError>;
    async fn store(&self, profile: &BrandProfile) -> Result<(), AnvilError>;
    async fn clear(&self, tenant_id: &str) -> Result<(), AnvilError>;
    /// The tenant an account's brand came from last time, so a launch can
    /// show it before anything is fetched. `None` by default.
    async fn last_tenant(&self, _aor: &str) -> Result<Option<String>, AnvilError> {
        Ok(None)
    }
    /// Remember `tenant_id` as `aor`'s, for [`last_tenant`](Self::last_tenant).
    async fn remember_tenant(&self, _aor: &str, _tenant_id: &str) -> Result<(), AnvilError> {
        Ok(())
    }
}

/// How often a registration refresh may fetch the brand again
/// (`docs/BRANDING.md` §7).
const REFETCH_EVERY: std::time::Duration = std::time::Duration::from_secs(3600);

/// The branding lifecycle (`docs/BRANDING.md` §1): the cached profile
/// first, then a fetch with its ETag, each announced as
/// [`Event::BrandUpdated`](crate::Event::BrandUpdated); a tenant without a
/// brand as [`Event::BrandCleared`](crate::Event::BrandCleared).
pub(crate) struct Branding {
    provider: Box<dyn BrandProvider>,
    cache: Option<Box<dyn BrandCache>>,
    credential: BrandCredential,
    aor: String,
    /// Where to fetch from: configured, or learned from a REGISTER 200 OK.
    url: parking_lot::Mutex<Option<String>>,
    current: tokio::sync::Mutex<Option<BrandProfile>>,
    fetched: parking_lot::Mutex<Option<std::time::Instant>>,
    events: tokio::sync::mpsc::Sender<crate::Event>,
}

impl Branding {
    pub(crate) fn new(
        config: crate::config::BrandConfig,
        aor: String,
        url: Option<String>,
        events: tokio::sync::mpsc::Sender<crate::Event>,
    ) -> Option<Self> {
        let provider = config.provider?;
        Some(Self {
            provider,
            cache: config.cache,
            credential: config.credential,
            aor,
            url: parking_lot::Mutex::new(url),
            current: tokio::sync::Mutex::new(None),
            fetched: parking_lot::Mutex::new(None),
            events,
        })
    }

    /// The cached profile, if there is one, announced.
    pub(crate) async fn load_cached(&self) {
        let Some(cache) = &self.cache else { return };
        let tenant = match cache.last_tenant(&self.aor).await {
            Ok(Some(t)) => t,
            _ => return,
        };
        if let Ok(Some(profile)) = cache.load(&tenant).await {
            *self.current.lock().await = Some(profile.clone());
            let _ = self
                .events
                .send(crate::Event::BrandUpdated {
                    profile: Box::new(profile),
                })
                .await;
        }
    }

    /// A provisioning URL a REGISTER 200 OK named; whether it is new to us
    /// (a configured one wins).
    pub(crate) fn learn_url(&self, url: &str) -> bool {
        let mut known = self.url.lock();
        if known.is_some() {
            return false;
        }
        *known = Some(url.trim_end_matches('/').to_string());
        true
    }

    /// Fetch unless one was fetched within the hour.
    pub(crate) async fn refresh_if_stale(&self) {
        let stale = self
            .fetched
            .lock()
            .is_none_or(|at| at.elapsed() >= REFETCH_EVERY);
        if stale {
            let _ = self.refresh().await;
        }
    }

    /// Fetch now, with the ETag of what is held.
    pub(crate) async fn refresh(&self) -> Result<(), AnvilError> {
        let Some(url) = self.url.lock().clone() else {
            return Ok(());
        };
        let mut current = self.current.lock().await;
        let request = BrandRequest {
            provisioning_url: url,
            aor: self.aor.clone(),
            credential: self.credential.clone(),
            if_none_match: current
                .as_ref()
                .map(|p| p.etag.clone())
                .filter(|e| !e.is_empty()),
        };
        let outcome = self.provider.fetch(request).await;
        *self.fetched.lock() = Some(std::time::Instant::now());
        match outcome {
            Ok(BrandFetchOutcome::NotModified) => Ok(()),
            Ok(BrandFetchOutcome::Updated(profile)) => {
                if let Some(cache) = &self.cache {
                    if let Err(e) = cache.store(&profile).await {
                        tracing::warn!(%e, "could not cache the brand");
                    }
                    let _ = cache.remember_tenant(&self.aor, &profile.tenant_id).await;
                }
                let profile = match &self.cache {
                    // What the cache holds, with its files' paths.
                    Some(cache) => cache
                        .load(&profile.tenant_id)
                        .await
                        .ok()
                        .flatten()
                        .map(Box::new)
                        .unwrap_or(profile),
                    None => profile,
                };
                *current = Some((*profile).clone());
                let _ = self
                    .events
                    .send(crate::Event::BrandUpdated { profile })
                    .await;
                Ok(())
            }
            Ok(BrandFetchOutcome::NoBrand) => {
                if let Some(held) = current.take() {
                    if let Some(cache) = &self.cache {
                        let _ = cache.clear(&held.tenant_id).await;
                    }
                    let _ = self.events.send(crate::Event::BrandCleared).await;
                }
                Ok(())
            }
            Err(AnvilError::AuthRejected(reason)) => {
                // The contract's 401: drop what is held, and say so.
                if let Some(held) = current.take() {
                    if let Some(cache) = &self.cache {
                        let _ = cache.clear(&held.tenant_id).await;
                    }
                    let _ = self.events.send(crate::Event::BrandCleared).await;
                }
                let error = AnvilError::AuthRejected(reason);
                let _ = self
                    .events
                    .send(crate::Event::Error {
                        call: None,
                        error: error.clone(),
                    })
                    .await;
                Err(error)
            }
            Err(e) => {
                tracing::warn!(%e, "brand fetch failed; keeping what is held");
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_are_read_as_the_contract_has_them() {
        assert_eq!(
            Rgba::parse("#1A73E8"),
            Some(Rgba {
                r: 0x1A,
                g: 0x73,
                b: 0xE8,
                a: 255
            })
        );
        assert_eq!(Rgba::parse("#1a73e880").map(|c| c.a), Some(0x80));
        assert_eq!(Rgba::parse("blue"), None);
        assert_eq!(Rgba::parse("#12345"), None);
        let colors: BrandColors =
            serde_json::from_str(r##"{"primary":"#1A73E8","accent":"red","surface":7}"##).unwrap();
        assert_eq!(colors.primary.map(Rgba::to_hex).as_deref(), Some("#1A73E8"));
        assert_eq!(colors.accent, None, "an invalid color is ignored");
        assert_eq!(colors.surface, None);
        assert_eq!(colors.background, None, "an unset slot is the UI's own");
    }

    #[test]
    fn a_profile_as_fcp_serves_it() {
        let json = r##"{
            "schema_version": 1, "tenant_id": "tenant-1", "etag": "abc",
            "app_name": "Acme Voice",
            "colors": {"primary": "#1A73E8"},
            "logo": {"url": "https://pbx.test/brand/v1/assets/aa", "mime": "image/svg+xml", "sha256": "aa"},
            "icon": {"url": "https://pbx.test/brand/v1/assets/bb", "mime": "image/png", "sha256": "bb", "width": 512, "height": 512},
            "ringtones": [{"name": "Acme", "default": true,
                           "asset": {"url": "https://pbx.test/brand/v1/assets/cc", "mime": "audio/ogg", "sha256": "cc"}}],
            "links": {"support": "https://acme.example/support"},
            "dial_plan": {"prefix_strip": ["+1"], "short_codes": {"1000": "sip:frontdesk@acme.example"}}
        }"##;
        let p: BrandProfile = serde_json::from_str(json).unwrap();
        assert_eq!(p.app_name, "Acme Voice");
        assert!(p.dark_colors.is_none() && p.logo_dark.is_none());
        assert_eq!(p.icon.unwrap().width, Some(512));
        assert!(p.ringtones[0].default);
        assert_eq!(
            p.dial_plan.short_codes["1000"],
            "sip:frontdesk@acme.example"
        );
        // Only the essentials are required.
        let bare: BrandProfile =
            serde_json::from_str(r#"{"schema_version":1,"tenant_id":"t","app_name":"A"}"#).unwrap();
        assert!(bare.ringtones.is_empty() && bare.colors.primary.is_none());
    }
}
