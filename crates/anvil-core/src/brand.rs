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

/// RGBA color, components 0–255.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

/// Color tokens a UI needs to theme the shell. Modelled loosely on Material 3
/// so Flutter / Jetpack hosts can map directly; native hosts (SwiftUI, AppKit)
/// pick the subset they use.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrandColors {
    pub primary: Rgba,
    pub accent: Rgba,
    pub background: Rgba,
    pub surface: Rgba,
    pub on_primary: Rgba,
    pub on_surface: Rgba,
    pub error: Rgba,
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
    pub width: Option<u32>,
    pub height: Option<u32>,
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
    pub etag: String,
    /// Shown as the app title, in the about screen, and in OS call UI.
    pub app_name: String,
    /// Light-theme color tokens.
    pub colors: BrandColors,
    /// Dark-theme color tokens. Falls back to `colors` if absent.
    pub dark_colors: Option<BrandColors>,
    /// Horizontal logo for headers. SVG or PNG. Light-theme variant.
    pub logo: Option<BrandAsset>,
    /// Dark-theme logo variant.
    pub logo_dark: Option<BrandAsset>,
    /// App / taskbar icon. Square. PNG ≥ 512×512 preferred.
    pub icon: Option<BrandAsset>,
    /// Custom ringtones. The first entry with `default = true` is used
    /// unless the user has overridden in settings.
    pub ringtones: Vec<Ringtone>,
    /// Optional support + legal links surfaced in the settings screen.
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

/// Result of a fetch. `NotModified` preserves the cached profile.
pub enum BrandFetchOutcome {
    Updated(Box<BrandProfile>),
    NotModified,
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
}
